use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use url::Url;

use crate::ConfigError;
use crate::paths::{
    EGRESS_POLICY_FILE_NAME, ensure_default_working_directory, open_private_config_file,
};

pub const CURRENT_EGRESS_POLICY_VERSION: u32 = 1;
pub const MAX_EGRESS_POLICY_BYTES: u64 = 64 * 1024;
pub const MAX_EGRESS_RULES: usize = 128;
pub const MAX_EGRESS_ENDPOINT_BYTES: usize = 2 * 1024;
pub const MAX_EGRESS_MODEL_BYTES: usize = 256;

/// A user-scoped source that reloads the policy for every authorization check.
/// Reloading keeps revocation effective for already-created planners and workers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressPolicyEnforcer {
    path: PathBuf,
}

impl EgressPolicyEnforcer {
    pub fn from_default_path() -> Result<Self, ConfigError> {
        Ok(Self {
            path: ensure_default_working_directory()?.join(EGRESS_POLICY_FILE_NAME),
        })
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn authorize(&self, endpoint: &str, model: &str) -> Result<(), ConfigError> {
        EgressPolicy::load(&self.path)?.authorize(endpoint, model)
    }
}

/// Parsed, validated local egress policy. It is intentionally independent from
/// `config.yaml`, which supplies the endpoint and credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressPolicy {
    version: u32,
    allowed: BTreeSet<AllowedProvider>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AllowedProvider {
    endpoint: CanonicalEndpoint,
    model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CanonicalEndpoint {
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EgressPolicyDocument {
    version: u32,
    #[serde(default)]
    allowed_providers: Vec<EgressRuleDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EgressRuleDocument {
    endpoint: String,
    models: Vec<String>,
}

impl EgressPolicy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let (mut file, metadata) = open_private_config_file(path)?;
        if metadata.len() > MAX_EGRESS_POLICY_BYTES {
            return Err(ConfigError::ConfigTooLarge {
                path: path.into(),
                limit: MAX_EGRESS_POLICY_BYTES,
            });
        }
        let mut contents = Vec::with_capacity(metadata.len() as usize);
        file.by_ref()
            .take(MAX_EGRESS_POLICY_BYTES + 1)
            .read_to_end(&mut contents)
            .map_err(|error| ConfigError::io(path, error))?;
        if contents.len() as u64 > MAX_EGRESS_POLICY_BYTES {
            return Err(ConfigError::ConfigTooLarge {
                path: path.into(),
                limit: MAX_EGRESS_POLICY_BYTES,
            });
        }
        let document = serde_yaml::from_slice::<EgressPolicyDocument>(&contents)
            .map_err(|_| ConfigError::InvalidYaml(path.into()))?;
        Self::from_document(document)
    }

    pub fn authorize(&self, endpoint: &str, model: &str) -> Result<(), ConfigError> {
        let endpoint =
            canonicalize_provider_endpoint(endpoint).map_err(ConfigError::InvalidProvider)?;
        let model = canonicalize_model(model).map_err(ConfigError::InvalidProvider)?;
        let display_endpoint = endpoint_string(endpoint.clone());
        if self.allowed.contains(&AllowedProvider {
            endpoint,
            model: model.clone(),
        }) {
            Ok(())
        } else {
            Err(ConfigError::ProviderEgressDenied {
                endpoint: display_endpoint,
                model,
            })
        }
    }

    fn from_document(document: EgressPolicyDocument) -> Result<Self, ConfigError> {
        if document.version != CURRENT_EGRESS_POLICY_VERSION {
            return Err(ConfigError::UnsupportedVersion(document.version));
        }
        if document.allowed_providers.len() > MAX_EGRESS_RULES {
            return Err(ConfigError::InvalidEgressPolicy(
                "egress policy contains too many provider rules",
            ));
        }
        let mut allowed = BTreeSet::new();
        for rule in document.allowed_providers {
            if rule.models.is_empty() {
                return Err(ConfigError::InvalidEgressPolicy(
                    "egress policy rules must list at least one model",
                ));
            }
            let endpoint = canonicalize_provider_endpoint(&rule.endpoint)
                .map_err(ConfigError::InvalidEgressPolicy)?;
            for model in rule.models {
                let model = canonicalize_model(&model).map_err(ConfigError::InvalidEgressPolicy)?;
                if allowed.len() >= MAX_EGRESS_RULES {
                    return Err(ConfigError::InvalidEgressPolicy(
                        "egress policy contains too many endpoint/model rules",
                    ));
                }
                if !allowed.insert(AllowedProvider {
                    endpoint: endpoint.clone(),
                    model,
                }) {
                    return Err(ConfigError::InvalidEgressPolicy(
                        "egress policy contains a duplicate provider rule",
                    ));
                }
            }
        }
        Ok(Self {
            version: CURRENT_EGRESS_POLICY_VERSION,
            allowed,
        })
    }
}

pub(crate) fn canonicalize_provider_endpoint(
    value: &str,
) -> Result<CanonicalEndpoint, &'static str> {
    if value.is_empty()
        || value.len() > MAX_EGRESS_ENDPOINT_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err("provider endpoint has an invalid length");
    }
    let endpoint = Url::parse(value).map_err(|_| "provider endpoint must be an absolute URL")?;
    let host = endpoint
        .host_str()
        .ok_or("provider endpoint must include a host")?;
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || authority_contains_userinfo(&endpoint)
    {
        return Err("provider endpoint must not contain userinfo");
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        return Err("provider endpoint must not contain query or fragment data");
    }
    let local_http = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && local_http) {
        return Err("provider endpoint must use HTTPS, except for a loopback HTTP endpoint");
    }
    let path = endpoint.path();
    if path.contains('%') || path.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("provider endpoint path must not contain encoded or control characters");
    }
    let mut canonical_path = path.to_owned();
    while canonical_path.len() > 1 && canonical_path.ends_with('/') {
        canonical_path.pop();
    }
    let port = endpoint
        .port_or_known_default()
        .ok_or("provider endpoint must use a supported HTTP(S) scheme")?;
    Ok(CanonicalEndpoint {
        scheme: endpoint.scheme().to_owned(),
        host: host.to_owned(),
        port,
        path: canonical_path,
    })
}

pub(crate) fn canonicalize_model(value: &str) -> Result<String, &'static str> {
    if value.is_empty() || value.len() > MAX_EGRESS_MODEL_BYTES || value.trim() != value {
        return Err("provider model has an invalid length or surrounding whitespace");
    }
    if value.chars().any(char::is_control) {
        return Err("provider model must not contain control characters");
    }
    Ok(value.to_owned())
}

fn endpoint_string(endpoint: CanonicalEndpoint) -> String {
    let host = if endpoint.host.contains(':') {
        format!("[{}]", endpoint.host)
    } else {
        endpoint.host
    };
    let default_port = (endpoint.scheme == "https" && endpoint.port == 443)
        || (endpoint.scheme == "http" && endpoint.port == 80);
    let port = if default_port {
        String::new()
    } else {
        format!(":{}", endpoint.port)
    };
    format!("{}://{}{}{}", endpoint.scheme, host, port, endpoint.path)
}

fn authority_contains_userinfo(endpoint: &Url) -> bool {
    let Some((_, authority)) = endpoint.as_str().split_once("://") else {
        return false;
    };
    authority
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|authority| authority.contains('@'))
}
