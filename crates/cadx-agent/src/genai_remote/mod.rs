//! OpenAI Responses-compatible remote planning through `rust-genai`.
//!
//! This adapter deliberately sends a small, disclosed text context and accepts
//! a narrow creation-only plan schema. It never gives the provider direct
//! access to a workspace, project archive, local file, image, or arbitrary CAD
//! command deserialization path.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use crate::error::AgentError;
use crate::provider::{ProviderConfig, RemoteContext, RemoteContextRequest, RemoteTaskPlanner};
use crate::remote_plan::{RemotePlanningDecision, decode_decision};
use cadx_config::EgressPolicyEnforcer;
use cadx_core::EntityId;
use genai::adapter::AdapterKind;
use genai::chat::{ChatOptions, ChatRequest, ChatResponseFormat};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget, WebConfig};

#[cfg(test)]
mod tests;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);

const SYSTEM_PROMPT: &str = r#"
You are a constrained CAD planning service. Return exactly one JSON object and no Markdown.
You can only propose new editable entities and parameters. Do not request files, images, network
access, shell commands, or hidden tools. Do not use IDs, layers, or entity references: CADX assigns
those locally.

The JSON schema is:
{
  "decision": "action",
  "action": {
      "intent": "short human-readable intent",
      "detail": "short human-readable implementation note",
      "operation": {
        "kind": "create_line | create_circle | create_rectangle | create_sketch_profile | create_wall | create_room | create_text | create_parameter | create_constrained_line",
        "name": "editable entity or parameter name",
        "...": "operation-specific finite numeric values"
      },
      "validation": [{"name": "short check", "detail": "short evidence", "status": "passed | warning"}]
  }
}

When the task is complete, return instead:
{"decision":"complete","summary":"short completion summary"}

Operation fields:
- create_line: start [x,y], end [x,y]
- create_circle: center [x,y], radius
- create_rectangle: origin [x,y], width, height
- create_sketch_profile: points [[x,y],...], closed
- create_wall: start [x,y], end [x,y], thickness
- create_room: boundary [[x,y],...]
- create_text: position [x,y], content
- create_parameter: exactly one of value (finite number in document units) or formula (restricted arithmetic over earlier parameter names)
- create_constrained_line: start [x,y], end [x,y], optional horizontal, optional vertical, optional length (restricted arithmetic over earlier parameter names). Supply at least one constraint, never both horizontal and vertical.

Create one parameter before a later round uses it in a formula or constrained line. `create_parameter`
requires parameters capability. `create_constrained_line` requires both drafting and mechanical
capability. Use only operations whose capability is listed in the user context. Return one decision.
"#;

/// A synchronous planner backed by an OpenAI Responses-compatible provider.
///
/// The existing [`RemoteTaskPlanner`] contract keeps calls behind exact
/// disclosure and project-grant checks. The credential comes from CADX's local
/// configuration boundary and is redacted from all debug output.
#[derive(Clone)]
pub struct GenAiRemotePlanner {
    config: ProviderConfig,
    api_key: String,
    egress_policy: EgressPolicyEnforcer,
    selected_entity_ids: Vec<EntityId>,
    timeout: Duration,
}

impl fmt::Debug for GenAiRemotePlanner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenAiRemotePlanner")
            .field("config", &self.config)
            .field("api_key", &"REDACTED")
            .field("egress_policy", &self.egress_policy.path())
            .field("selected_entity_ids", &self.selected_entity_ids)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl GenAiRemotePlanner {
    /// Creates a planner with a credential loaded by the caller from local
    /// configuration. The planner never reads provider environment variables.
    pub fn new(config: ProviderConfig, api_key: impl Into<String>) -> Result<Self, AgentError> {
        let egress_policy = EgressPolicyEnforcer::from_default_path().map_err(|error| {
            AgentError::Provider(format!("cannot resolve provider egress policy: {error}"))
        })?;
        Self::new_with_egress_policy(config, api_key, egress_policy)
    }

    /// Creates a planner with an explicit policy source. The policy is still
    /// reloaded on every authorization check before a provider call.
    pub fn new_with_egress_policy(
        config: ProviderConfig,
        api_key: impl Into<String>,
        egress_policy: EgressPolicyEnforcer,
    ) -> Result<Self, AgentError> {
        config.validate()?;
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(AgentError::Provider("provider API key is required".into()));
        }
        Ok(Self {
            config,
            api_key,
            egress_policy,
            selected_entity_ids: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Limits remote context to a canonical set of entity identifiers. Entity
    /// geometry is not sent by this initial provider contract.
    pub fn with_selected_entity_ids(
        mut self,
        selected_entity_ids: impl IntoIterator<Item = EntityId>,
    ) -> Self {
        self.selected_entity_ids = selected_entity_ids
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, AgentError> {
        if timeout.is_zero() {
            return Err(AgentError::Provider(
                "provider request timeout must be greater than zero".into(),
            ));
        }
        self.timeout = timeout;
        Ok(self)
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    pub fn authorize_egress(&self) -> Result<(), AgentError> {
        self.egress_policy
            .authorize(&self.config.endpoint, &self.config.model)
            .map_err(|error| AgentError::Provider(format!("provider egress denied: {error}")))
    }

    fn request_decision(
        &self,
        context: RemoteContext,
    ) -> Result<RemotePlanningDecision, AgentError> {
        self.authorize_egress()?;
        let prompt = build_prompt(&context);
        let endpoint = normalized_endpoint(&self.config.endpoint);
        let model = self.config.model.clone();
        let api_key = self.api_key.clone();
        let timeout = self.timeout;
        let body = request_response_body(endpoint, model, api_key, timeout, prompt)?;
        decode_decision(&body)
    }
}

fn request_response_body(
    endpoint: String,
    model: String,
    api_key: String,
    timeout: Duration,
    prompt: String,
) -> Result<String, AgentError> {
    let target = ServiceTarget {
        endpoint: Endpoint::from_owned(endpoint),
        auth: AuthData::from_single(api_key),
        model: ModelIden::new(AdapterKind::OpenAIResp, model),
    };
    let client = Client::builder()
        .with_adapter_kind(AdapterKind::OpenAIResp)
        .with_web_config(WebConfig::default().with_timeout(timeout))
        .build();
    let chat_request = ChatRequest::from_user(prompt)
        .with_system(SYSTEM_PROMPT)
        .with_store(false);
    let options = ChatOptions::default()
        .with_response_format(ChatResponseFormat::JsonMode)
        .with_max_tokens(4_096);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| AgentError::Provider("could not start provider runtime".into()))?;
    let response = runtime
        .block_on(client.exec_chat(target, chat_request, Some(&options)))
        .map_err(|_| AgentError::Provider("remote model request failed".into()))?;
    let body = response
        .content
        .into_joined_texts()
        .ok_or_else(|| AgentError::Provider("remote model returned no plan content".into()))?;
    Ok(body)
}

impl RemoteTaskPlanner for GenAiRemotePlanner {
    fn config(&self) -> &ProviderConfig {
        &self.config
    }

    fn authorize_egress(&self) -> Result<(), AgentError> {
        Self::authorize_egress(self)
    }

    fn context_request(&self) -> RemoteContextRequest {
        RemoteContextRequest::selected_entities(self.selected_entity_ids.iter().copied())
    }

    fn plan_remote(&self, context: RemoteContext) -> Result<RemotePlanningDecision, AgentError> {
        self.request_decision(context)
    }
}

fn normalized_endpoint(endpoint: &str) -> String {
    format!("{}/", endpoint.trim_end_matches('/'))
}

fn build_prompt(context: &RemoteContext) -> String {
    context.payload_json().into()
}
