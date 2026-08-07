//! Provider client construction and the [`crate::AiAssistant`] implementation.

use std::time::Duration;

use genai::{
    Client, ModelIden, ServiceTarget, WebConfig,
    adapter::AdapterKind,
    chat::{ChatMessage, ChatOptions, ChatRequest, Tool, ToolChoice},
    resolver::{AuthData, Endpoint, ServiceTargetResolver},
};
use serde_json::json;

use cadx_config::ProviderConfig;

use crate::{
    AiAssistant, AiError, AiFuture, AiRequest, DomainAiFuture, DomainAiPlan, DomainAiRequest,
};

use super::{
    error::provider_error_message,
    prompt::{DOMAIN_SYSTEM_PROMPT, SYSTEM_PROMPT, build_prompt},
    schema::cad_plan_tool,
};

#[derive(Debug, Clone)]
pub struct GenAiAssistant {
    client: Client,
    model: String,
}

impl GenAiAssistant {
    /// Creates an assistant with an explicit model and no environment-backed
    /// credentials. Prefer [`Self::from_provider_config`] for production use.
    ///
    /// # Panics
    ///
    /// Panics only if the built-in provider configuration is internally
    /// inconsistent, which indicates a programming error.
    pub fn new(model: impl Into<String>) -> Self {
        let config = ProviderConfig {
            model: model.into(),
            ..ProviderConfig::default()
        };
        Self::from_provider_config(&config).expect("built-in AI configuration must be valid")
    }

    /// Creates an assistant from the validated CADX provider configuration.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::Configuration`] when the configured adapter name is
    /// unsupported.
    pub fn from_provider_config(config: &ProviderConfig) -> Result<Self, AiError> {
        let model = config.model.trim().to_owned();
        let adapter = match config.adapter.as_deref() {
            Some(name) => {
                AdapterKind::from_lower_str(&name.to_ascii_lowercase()).ok_or_else(|| {
                    AiError::Configuration(format!("unsupported provider.adapter '{name}'"))
                })?
            }
            None if config.endpoint.is_some() => AdapterKind::OpenAI,
            None => AdapterKind::from_model(&model)
                .map_err(|error| AiError::Configuration(error.to_string()))?,
        };
        let endpoint = config
            .endpoint
            .as_deref()
            .map(normalize_endpoint_base_url)
            .transpose()?;
        let api_key = config.api_key.clone();
        let resolver = ServiceTargetResolver::from_resolver_fn(
            move |target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
                Ok(ServiceTarget {
                    endpoint: endpoint
                        .clone()
                        .map_or(target.endpoint, Endpoint::from_owned),
                    auth: api_key
                        .clone()
                        .map_or(AuthData::None, AuthData::from_single),
                    model: ModelIden::new(adapter, target.model.model_name),
                })
            },
        );
        let client = Client::builder()
            .with_adapter_kind(adapter)
            .with_service_target_resolver(resolver)
            .with_web_config(
                WebConfig::default().with_timeout(Duration::from_secs(config.timeout_seconds)),
            )
            .build();
        Ok(Self { client, model })
    }
}

impl AiAssistant for GenAiAssistant {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn plan(&self, request: AiRequest) -> AiFuture {
        let client = self.client.clone();
        let model = self.model.clone();
        Box::pin(async move {
            let prompt = build_prompt(&request)?;
            let chat_request = ChatRequest::new(vec![
                ChatMessage::system(SYSTEM_PROMPT),
                ChatMessage::user(prompt),
            ])
            .with_tools([cad_plan_tool()]);

            let response = client
                .exec_chat(&model, chat_request, None)
                .await
                .map_err(|error| AiError::Request(provider_error_message(&error.to_string())))?;
            let fallback_text = response
                .first_text()
                .unwrap_or("no response text")
                .to_owned();
            let tool_call = response
                .tool_calls()
                .into_iter()
                .find(|call| call.fn_name == "apply_cad_plan")
                .ok_or(AiError::MissingToolCall(fallback_text))?;

            serde_json::from_value(tool_call.fn_arguments.clone())
                .map_err(|error| AiError::InvalidPlan(error.to_string()))
        })
    }

    fn plan_domain(&self, request: DomainAiRequest) -> DomainAiFuture {
        let client = self.client.clone();
        let model = self.model.clone();
        Box::pin(async move {
            let DomainAiRequest {
                prompt,
                domain,
                context,
                tools,
            } = request;
            if tools.is_empty() {
                return Err(AiError::InvalidDomainToolCall(format!(
                    "no AI tools are registered for {}",
                    domain.slug()
                )));
            }

            let provider_tools = tools
                .iter()
                .map(|binding| {
                    Tool::new(binding.ai_tool.id)
                        .with_description(binding.ai_tool.description)
                        .with_schema(binding.parameter_schema())
                        .with_strict(true)
                })
                .collect::<Vec<_>>();
            let prompt = serde_json::to_string(&json!({
                "user_request": prompt,
                "active_domain": domain,
                "document_context": context,
            }))
            .map_err(|error| AiError::InvalidPlan(error.to_string()))?;
            let chat_request = ChatRequest::new(vec![
                ChatMessage::system(DOMAIN_SYSTEM_PROMPT),
                ChatMessage::user(prompt),
            ])
            .with_tools(provider_tools);
            let options = ChatOptions::default().with_tool_choice(ToolChoice::Required);
            let response = client
                .exec_chat(&model, chat_request, Some(&options))
                .await
                .map_err(|error| AiError::Request(provider_error_message(&error.to_string())))?;
            let fallback_text = response
                .first_text()
                .unwrap_or("no response text")
                .to_owned();
            let calls = response.tool_calls();
            if calls.len() != 1 {
                return Err(AiError::InvalidDomainToolCall(format!(
                    "expected exactly one offered tool call, received {}; {fallback_text}",
                    calls.len()
                )));
            }
            let call = calls[0];
            let binding = tools
                .iter()
                .find(|binding| binding.ai_tool.id == call.fn_name)
                .ok_or_else(|| {
                    AiError::InvalidDomainToolCall(format!(
                        "tool {} was not offered for {}",
                        call.fn_name,
                        domain.slug()
                    ))
                })?;
            let parameters = binding
                .decode_parameters(&call.fn_arguments)
                .map_err(AiError::InvalidDomainToolCall)?;

            Ok(DomainAiPlan {
                domain,
                ai_tool_id: binding.ai_tool.id.into(),
                executable_tool_id: binding.executable_tool.id.into(),
                parameters,
            })
        })
    }
}

fn normalize_endpoint_base_url(endpoint: &str) -> Result<String, AiError> {
    let mut url = url::Url::parse(endpoint.trim()).map_err(|error| {
        AiError::Configuration(format!("provider.endpoint is not a valid URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(AiError::Configuration(
            "provider.endpoint must be an HTTP(S) base URL".into(),
        ));
    }
    if url.fragment().is_some() {
        return Err(AiError::Configuration(
            "provider.endpoint must not contain a URL fragment".into(),
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_configuration_controls_model_without_environment() {
        let assistant = GenAiAssistant::from_provider_config(&ProviderConfig {
            endpoint: Some("https://example.test/v1".into()),
            model: "custom-model".into(),
            api_key: Some("secret".into()),
            adapter: Some("openai".into()),
            timeout_seconds: 12,
        })
        .unwrap();
        assert_eq!(assistant.model_name(), "custom-model");
    }

    #[test]
    fn provider_endpoint_preserves_a_path_base_without_a_trailing_slash() {
        assert_eq!(
            normalize_endpoint_base_url("https://example.test/v1").unwrap(),
            "https://example.test/v1/"
        );
        assert_eq!(
            normalize_endpoint_base_url("https://example.test/gateway/v1?tenant=cadx").unwrap(),
            "https://example.test/gateway/v1/?tenant=cadx"
        );
    }

    #[test]
    fn provider_endpoint_rejects_non_http_and_fragment_urls() {
        for endpoint in [
            "file:///tmp/provider",
            "not a URL",
            "https://example.test/v1#chat",
        ] {
            assert!(matches!(
                normalize_endpoint_base_url(endpoint),
                Err(AiError::Configuration(_))
            ));
        }
    }

    #[test]
    fn invalid_provider_adapter_is_rejected_before_network_use() {
        let error = GenAiAssistant::from_provider_config(&ProviderConfig {
            adapter: Some("not-a-provider".into()),
            ..ProviderConfig::default()
        })
        .unwrap_err();
        assert!(matches!(error, AiError::Configuration(_)));
    }
}
