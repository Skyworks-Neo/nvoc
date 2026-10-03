//! MCP tool handlers — the control-plane surface an agent can drive.
//!
//! Every tool maps onto an existing srv mutation (the same registry/controller
//! calls the HTTP routes use); no new control logic lives here. Control tools
//! bind the caller to an nvoc consumer session in the MCP band
//! ([`super::ensure_session`]) and declare a claim; reads do not.

use super::ensure_session;
use crate::config::{ControlMode, TARGET_C_MAX, TARGET_C_MIN};
use crate::http::ServerState;
use crate::runtime::{ServiceCmd, lock};
use crate::session::{self, Claim};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars::JsonSchema;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;
use std::sync::Arc;

/// MCP server handler. One instance per MCP session; it holds only a shared
/// handle to srv state, so construction never mutates the control plane.
#[derive(Clone)]
pub struct NvocMcp {
    state: Arc<ServerState>,
}

impl NvocMcp {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }

    fn config(&self) -> crate::runtime::SharedConfig {
        self.state.config.clone()
    }

    /// Declare this session's claim and apply it immediately.
    fn apply_claim(&self, id: &str, claim: Claim) -> Result<(), String> {
        session::claim(id, claim)?;
        let _ = session::sweep_and_apply(&self.state.config);
        Ok(())
    }

    /// Drop this session's claim (yield the band) and re-arbitrate.
    fn apply_release(&self, id: &str) -> Result<(), String> {
        session::release(id)?;
        let _ = session::sweep_and_apply(&self.state.config);
        Ok(())
    }

    /// The nvoc session for the calling MCP session, opened/renewed on demand.
    fn bound_session(&self, ctx: &RequestContext<RoleServer>) -> String {
        ensure_session(&self.config(), &mcp_id(ctx))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetModeArgs {
    /// Control mode: "auto" (driver fan curve), "pid" (closed loop), or
    /// "manual" (pinned duty).
    mode: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetPidTargetArgs {
    /// PID setpoint in °C (30–110).
    target_c: f32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetFanManualArgs {
    /// Pinned fan duty, 0–100 %.
    percent: u32,
}

#[tool_router]
impl NvocMcp {
    /// Current mode, loop, setpoint and per-GPU control status.
    #[tool(
        description = "Read nvoc-srv control status: mode, loop kind, PID setpoint, and per-GPU temperature/duty/failsafe state."
    )]
    async fn nvoc_status(&self) -> Result<CallToolResult, ErrorData> {
        let (mode, loop_kind, interval_ms, target_c) = {
            let cfg = lock(&self.state.config);
            (cfg.mode, cfg.loop_kind, cfg.interval_ms, cfg.pid.target_c)
        };
        let gpus = serde_json::to_value(&*lock(&self.state.status)).unwrap_or_default();
        let owner = session::snapshot().0;
        ok_json(serde_json::json!({
            "mode": mode,
            "loop_kind": loop_kind,
            "interval_ms": interval_ms,
            "target_c": target_c,
            "owner": owner,
            "gpus": gpus,
        }))
    }

    /// Registered consumers and the current control owner.
    #[tool(
        description = "List registered control consumers and which one currently owns the control loop."
    )]
    async fn nvoc_sessions(&self) -> Result<CallToolResult, ErrorData> {
        let (owner, views) = session::snapshot();
        ok_json(serde_json::json!({ "owner": owner, "sessions": views }))
    }

    /// The effective runtime configuration.
    #[tool(
        description = "Read the effective nvoc-srv runtime configuration (ports, PID/freq params, GPU selection, auth, MCP)."
    )]
    async fn nvoc_get_config(&self) -> Result<CallToolResult, ErrorData> {
        let cfg = lock(&self.state.config);
        ok_json(serde_json::to_value(&*cfg).unwrap_or_default())
    }

    /// Set this session's control mode.
    #[tool(
        description = "Claim the control loop for this MCP session and set its mode (auto|pid|manual)."
    )]
    async fn nvoc_set_mode(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(args): Parameters<SetModeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let mode = parse_mode(&args.mode)
            .ok_or_else(|| ErrorData::invalid_params("mode must be auto|pid|manual", None))?;
        let id = self.bound_session(&ctx);
        self.apply_claim(&id, Claim::new(mode, None, None))
            .map_err(|e| ErrorData::internal_error(e, None))?;
        ok_json(serde_json::json!({ "mode": mode, "session": id }))
    }

    /// Set the PID target temperature and switch to the pid loop.
    #[tool(
        description = "Claim the control loop and set the PID target temperature (°C, 30–110); switches mode to pid."
    )]
    async fn nvoc_set_pid_target(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(args): Parameters<SetPidTargetArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        if !args.target_c.is_finite() || !(TARGET_C_MIN..=TARGET_C_MAX).contains(&args.target_c) {
            return Err(ErrorData::invalid_params(
                format!("target_c must be {TARGET_C_MIN}–{TARGET_C_MAX} °C"),
                None,
            ));
        }
        let id = self.bound_session(&ctx);
        self.apply_claim(&id, Claim::new(ControlMode::Pid, Some(args.target_c), None))
            .map_err(|e| ErrorData::internal_error(e, None))?;
        ok_json(serde_json::json!({ "mode": "pid", "target_c": args.target_c, "session": id }))
    }

    /// Pin a manual fan duty.
    #[tool(description = "Claim the control loop and pin a manual fan duty (0–100 %).")]
    async fn nvoc_set_fan_manual(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(args): Parameters<SetFanManualArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        if args.percent > 100 {
            return Err(ErrorData::invalid_params("percent must be 0–100", None));
        }
        let id = self.bound_session(&ctx);
        self.apply_claim(
            &id,
            Claim::new(ControlMode::Manual, None, Some(args.percent)),
        )
        .map_err(|e| ErrorData::internal_error(e, None))?;
        ok_json(serde_json::json!({ "mode": "manual", "percent": args.percent, "session": id }))
    }

    /// Release this session's control intent back to the driver.
    #[tool(
        description = "Release this MCP session's control claim; when no other consumer holds control the fan returns to the driver's own curve (auto)."
    )]
    async fn nvoc_restore_auto(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = self.bound_session(&ctx);
        self.apply_release(&id)
            .map_err(|e| ErrorData::internal_error(e, None))?;
        ok_json(serde_json::json!({ "mode": "auto", "session": id }))
    }

    /// Echo this MCP session's identity and ownership.
    #[tool(
        description = "Report this MCP session's bound nvoc consumer id, priority band, and whether it currently owns control."
    )]
    async fn nvoc_whoami(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = self.bound_session(&ctx);
        let (owner, _) = session::snapshot();
        ok_json(serde_json::json!({
            "mcp_session": mcp_id(&ctx),
            "nvoc_session": id,
            "priority": session::PRIORITY_MCP,
            "owner": owner.as_deref() == Some(id.as_str()),
        }))
    }

    /// Gracefully stop the service (fans restored to the driver first).
    #[tool(
        description = "Request a graceful nvoc-srv shutdown; fans are restored to the driver before the control loop exits."
    )]
    async fn nvoc_shutdown(&self) -> Result<CallToolResult, ErrorData> {
        self.state.cmd_tx.send(ServiceCmd::Shutdown).map_err(|e| {
            ErrorData::internal_error(format!("cannot deliver shutdown: {e}"), None)
        })?;
        ok_json(serde_json::json!({ "ok": true }))
    }
}

#[tool_handler]
impl ServerHandler for NvocMcp {}

/// MCP transport session id (stable per conversation) used to key the nvoc
/// consumer session. Falls back to a single shared key if the header is absent.
fn mcp_id(ctx: &RequestContext<RoleServer>) -> String {
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|p| p.headers.get("mcp-session-id"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default")
        .to_string()
}

fn parse_mode(s: &str) -> Option<ControlMode> {
    match s.to_ascii_lowercase().as_str() {
        "auto" => Some(ControlMode::Auto),
        "pid" => Some(ControlMode::Pid),
        "manual" => Some(ControlMode::Manual),
        _ => None,
    }
}

fn ok_json(v: serde_json::Value) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
    )]))
}

#[cfg(test)]
mod tests {
    use super::parse_mode;
    use crate::config::ControlMode;

    #[test]
    fn parse_mode_accepts_known_tokens_case_insensitively() {
        assert_eq!(parse_mode("auto"), Some(ControlMode::Auto));
        assert_eq!(parse_mode("PID"), Some(ControlMode::Pid));
        assert_eq!(parse_mode("Manual"), Some(ControlMode::Manual));
        assert_eq!(parse_mode("bogus"), None);
    }
}
