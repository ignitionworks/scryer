//! The commands that drive an agent session or the preview sidecar.
//!
//! Not served yet. They are the desktop's `build.rs`, `preview.rs` and
//! `mcp_setup.rs` — an ACP client, a spawned sidecar, and a config writer,
//! each of which needs its own process management inside a service that has
//! many projects and many hosts. They are named here so the dispatch table
//! carries the full command surface, and every one of them refuses with a
//! `notImplemented` a client can tell apart from a typo (`unknownCommand`).

use crate::error::{CommandError, CommandResult};

fn not_yet(command: &str, what: &str) -> CommandError {
    CommandError::NotImplemented {
        command: command.to_string(),
        message: format!("not yet served by the engine service — {what}"),
    }
}

pub fn start_model_build(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(command, "agent runs land with the hook endpoint"))
}

pub fn start_drift_check(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(command, "agent runs land with the hook endpoint"))
}

pub fn cancel_agent_session(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(command, "agent runs land with the hook endpoint"))
}

pub fn ensure_preview_server(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(
        command,
        "the preview sidecar is managed by the host for now",
    ))
}

pub fn start_preview_fixture_session(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(
        command,
        "the preview sidecar is managed by the host for now",
    ))
}

pub fn detect_ai_tools(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(
        command,
        "AI-tool integration is configured on the machine, not over HTTP",
    ))
}

pub fn setup_mcp_integration(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(
        command,
        "AI-tool integration is configured on the machine, not over HTTP",
    ))
}

pub fn open_in_editor(command: &str) -> CommandResult<serde_json::Value> {
    Err(not_yet(
        command,
        "a service never launches an editor on the machine it runs on; a host renders its own file view",
    ))
}
