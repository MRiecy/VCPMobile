//! Thin Distributed adapter for the canonical VCPMobileCLI protocol and Runtime.
//!
//! This module owns transport translation only. Manifest schema, validation, operation replay,
//! jobs and ProcessHost lifecycle remain owned by `vcp_modules::cli`.

use async_trait::async_trait;
use serde_json::value::RawValue;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::distributed::tool_registry::{OneShotTool, ToolExecutionContext};
use crate::distributed::types::{InvocationCommand, ToolManifest};
use crate::vcp_modules::cli::manifest::{
    serialize_vcp_mobile_cli_manifest, vcp_mobile_cli_manifest,
};
use crate::vcp_modules::cli::protocol::{
    validate_distributed_vcp_cli_args, validate_distributed_vcp_context, VcpCliProtocolError,
};
use crate::vcp_modules::cli::result::project_vcp_plugin_outcome;
use crate::vcp_modules::cli::runtime::{ExecuteVcpMobileCliRequest, MobileCliRuntimeState};
use crate::vcp_modules::settings_manager::{freeze_mobile_cli_agent_route, MobileCliAgentRoute};

const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_REMOTE_IDENTITY_BYTES: usize = 256;

pub struct VcpMobileCliTool;

#[async_trait]
impl OneShotTool for VcpMobileCliTool {
    fn manifest(&self) -> ToolManifest {
        let manifest = vcp_mobile_cli_manifest();
        ToolManifest {
            name: manifest.name,
            description: manifest.description,
            display_name: manifest.display_name,
            placeholder: None,
            invocation_commands: manifest
                .capabilities
                .invocation_commands
                .into_iter()
                .map(|command| InvocationCommand {
                    command_identifier: command.command_identifier,
                    description: command.description,
                    example: command.example,
                })
                .collect(),
        }
    }

    fn registration_manifest(&self) -> Result<Box<RawValue>, String> {
        let canonical = serialize_vcp_mobile_cli_manifest().map_err(|error| {
            format!("cannot serialize canonical VCPMobileCLI manifest: {error}")
        })?;
        RawValue::from_string(canonical)
            .map_err(|error| format!("cannot build canonical VCPMobileCLI manifest: {error}"))
    }

    async fn is_publishable(&self, app: &AppHandle) -> Result<bool, String> {
        if !cfg!(target_os = "android") {
            return Ok(false);
        }
        if !publication_route_enabled(true, current_mobile_cli_route(app).await?) {
            return Ok(false);
        }
        let runtime = app.try_state::<MobileCliRuntimeState>().ok_or_else(|| {
            "runtime_unavailable: managed VCPMobileCLI Runtime is missing".to_string()
        })?;
        runtime.ensure_ready_for_registration(app).await?;
        // Provision can take longer than a settings update. Re-read the sole route owner before
        // handing the manifest to the registry so a queued localLoopback switch withdraws it.
        Ok(publication_route_enabled(
            true,
            current_mobile_cli_route(app).await?,
        ))
    }

    async fn execute(&self, _args: Value, _app: &AppHandle) -> Result<Value, String> {
        Err("invalid_request: authenticated Distributed execution context is required".to_string())
    }

    async fn execute_with_context(
        &self,
        args: Value,
        app: &AppHandle,
        context: Option<ToolExecutionContext>,
    ) -> Result<Value, String> {
        if !cfg!(target_os = "android") {
            return Err(
                "runtime_unavailable: VCPMobileCLI Distributed execution requires Android"
                    .to_string(),
            );
        }
        require_vcp_plugin_route(app).await?;

        let context = context.ok_or_else(|| {
            "invalid_request: authenticated Distributed execution context is required".to_string()
        })?;
        validate_execution_context(&context)?;
        validate_distributed_vcp_context(context.vcp_context.as_ref())
            .map_err(format_protocol_error)?;
        let validated = validate_distributed_vcp_cli_args(&args).map_err(format_protocol_error)?;

        // ink/archery are already owned by the upstream VCP turn. The canonical capability gate
        // above validates their frozen values; only the typed shell/job action reaches Runtime.
        let operation_id = distributed_operation_id(&context.remote_identity, &context.request_id);
        let session_id = distributed_session_id(&context.remote_identity);
        let runtime = app.try_state::<MobileCliRuntimeState>().ok_or_else(|| {
            "runtime_unavailable: managed VCPMobileCLI Runtime is missing".to_string()
        })?;
        let response = runtime
            .execute(
                app,
                ExecuteVcpMobileCliRequest {
                    operation_id: operation_id.clone(),
                    action: validated.action,
                    session_id: Some(session_id),
                    river_projection: None,
                    vref_projection: None,
                },
            )
            .await
            .map_err(|error| format!("internal_error: Runtime execution failed: {error}"))?;
        if response.operation_id != operation_id {
            return Err("internal_error: Runtime response operation identity mismatch".to_string());
        }
        project_vcp_plugin_outcome(response.envelope)
    }
}

async fn current_mobile_cli_route(app: &AppHandle) -> Result<MobileCliAgentRoute, String> {
    freeze_mobile_cli_agent_route(app)
        .await
        .map_err(|error| format!("runtime_unavailable: cannot read Mobile CLI route: {error}"))
}

async fn require_vcp_plugin_route(app: &AppHandle) -> Result<(), String> {
    match current_mobile_cli_route(app).await? {
        MobileCliAgentRoute::VcpPlugin => Ok(()),
        MobileCliAgentRoute::LocalLoopback => {
            Err("user_disabled: VCPMobileCLI Distributed route is not selected".to_string())
        }
    }
}

fn publication_route_enabled(is_android: bool, route: MobileCliAgentRoute) -> bool {
    is_android && route == MobileCliAgentRoute::VcpPlugin
}

fn validate_execution_context(context: &ToolExecutionContext) -> Result<(), String> {
    if context.request_id.is_empty() || context.request_id.len() > MAX_REQUEST_ID_BYTES {
        return Err("invalid_request: requestId must contain 1..=128 UTF-8 bytes".to_string());
    }
    if context.remote_identity.is_empty()
        || context.remote_identity.len() > MAX_REMOTE_IDENTITY_BYTES
        || !context.remote_identity.is_ascii()
    {
        return Err("invalid_request: remote identity must be bounded non-empty ASCII".to_string());
    }
    if context.connection_epoch == 0 || context.server_id.is_empty() || context.client_id.is_empty()
    {
        return Err("remote_disconnected: Distributed ACK identity is incomplete".to_string());
    }
    Ok(())
}

fn distributed_operation_id(remote_identity: &str, request_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(remote_identity.as_bytes());
    hasher.update([0]);
    hasher.update(request_id.as_bytes());
    format!("dist:{}", hex::encode(hasher.finalize()))
}

fn distributed_session_id(remote_identity: &str) -> String {
    format!(
        "dist-session:{}",
        hex::encode(Sha256::digest(remote_identity.as_bytes()))
    )
}

fn format_protocol_error(error: VcpCliProtocolError) -> String {
    format!("{}: {error}", error.code.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distributed::types::OutgoingMessage;
    use crate::vcp_modules::cli::manifest::VCP_MOBILE_CLI_TOOL_NAME;

    #[test]
    fn registration_manifest_is_the_canonical_byte_source() {
        let tool = VcpMobileCliTool;
        let raw = tool
            .registration_manifest()
            .expect("build canonical registration manifest");
        assert_eq!(
            raw.get(),
            serialize_vcp_mobile_cli_manifest().expect("serialize canonical manifest")
        );
        let outer = serde_json::to_string(&OutgoingMessage::RegisterTools {
            server_name: "mobile".to_string(),
            tools: vec![raw],
        })
        .expect("serialize register_tools wire");
        let canonical = serialize_vcp_mobile_cli_manifest().expect("serialize canonical manifest");
        assert!(
            outer.contains(&format!("\"tools\":[{canonical}]")),
            "RawValue must preserve the canonical manifest bytes inside tools[]"
        );

        let metadata = tool.manifest();
        assert_eq!(metadata.name, VCP_MOBILE_CLI_TOOL_NAME);
        assert_eq!(
            metadata.invocation_commands[0].command_identifier,
            VCP_MOBILE_CLI_TOOL_NAME
        );
    }

    #[test]
    fn publication_requires_android_and_the_vcp_plugin_route() {
        assert!(!publication_route_enabled(
            false,
            MobileCliAgentRoute::VcpPlugin
        ));
        assert!(!publication_route_enabled(
            true,
            MobileCliAgentRoute::LocalLoopback
        ));
        assert!(publication_route_enabled(
            true,
            MobileCliAgentRoute::VcpPlugin
        ));
    }

    #[test]
    fn durable_identity_ignores_connection_epoch_but_binds_remote_and_request() {
        let first = distributed_operation_id("remote-a", "request-1");
        let reconnect = distributed_operation_id("remote-a", "request-1");
        assert_eq!(first, reconnect);
        assert_ne!(first, distributed_operation_id("remote-a", "request-2"));
        assert_ne!(first, distributed_operation_id("remote-b", "request-1"));
        assert!(first.starts_with("dist:"));
        assert!(first.is_ascii());

        let session = distributed_session_id("remote-a");
        assert_eq!(session, distributed_session_id("remote-a"));
        assert_ne!(session, distributed_session_id("remote-b"));
    }

    #[test]
    fn context_rejects_missing_ack_identity() {
        let context = ToolExecutionContext {
            request_id: "request-1".to_string(),
            remote_identity: "remote-a".to_string(),
            connection_epoch: 0,
            server_id: String::new(),
            client_id: String::new(),
            vcp_context: None,
        };
        assert!(validate_execution_context(&context)
            .expect_err("incomplete ACK must fail")
            .starts_with("remote_disconnected:"));
    }
}
