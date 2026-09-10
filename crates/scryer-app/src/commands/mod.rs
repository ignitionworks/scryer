//! The command surface: the thirty-nine commands the desktop's webview
//! invokes, written once as plain functions with no window or shell behind
//! them, plus the by-name dispatch a host calls them through.
//!
//! The functions in the submodules are the surface. [`dispatch`] is a
//! convenience over it — a host that speaks a name and a bag of arguments
//! (`invoke(cmd, args)` in upstream's frontend, `POST /api/cmd/{name}` over
//! HTTP) gets the same functions without knowing their signatures.

pub mod agents;
pub(crate) mod highlight;
pub mod observability;
pub mod project;
pub mod source_view;
pub(crate) mod symbols;
pub mod verdicts;

use crate::error::{CommandError, CommandResult};
use crate::state::AppState;

/// Every command name the service answers to, in the order
/// `src-tauri/src/lib.rs` lists them in its `invoke_handler`. A name absent
/// from here does not exist; a name present may still refuse with
/// `notImplemented` (see [`agents`]).
pub const COMMANDS: &[&str] = &[
    "watch_project",
    "is_legacy_model",
    "read_model",
    "read_planned",
    "write_planned",
    "close_change",
    "sign_off_change",
    "read_fold_refusals",
    "read_history",
    "open_in_editor",
    "read_source_span",
    "verify_anchor",
    "detect_ai_tools",
    "setup_mcp_integration",
    "create_blank_model",
    "get_subagent_settings",
    "set_subagent_settings",
    "ensure_preview_server",
    "start_preview_fixture_session",
    "start_model_build",
    "start_drift_check",
    "get_drift_status",
    "get_model_health",
    "get_test_statuses",
    "get_probe_statuses",
    "reconcile_drift",
    "reconcile_drift_node",
    "adopt_responsibility",
    "reject_responsibility",
    "drop_responsibility",
    "reimplement_responsibility",
    "adopt_property",
    "reject_property",
    "drop_property",
    "reimplement_property",
    "reword_responsibility",
    "drop_node",
    "reimplement_node",
    "cancel_agent_session",
];

/// The identity the caller claims, threaded onto every write. Opaque: the
/// service never parses it, never checks it, and never asks what it means. A
/// host decides who may write and under what name — see the `Engine Service`
/// directive on leaving identity, authorisation and audience to the host.
pub type Actor<'a> = Option<&'a str>;

/// Call one command by name with a JSON bag of arguments, returning its result
/// as JSON.
///
/// The argument names are upstream's, so a host can forward the webview's
/// `invoke(cmd, args)` payload verbatim. The desktop's `cwd` argument is
/// accepted under its own name and under `projectPath`, because upstream spells
/// the same thing both ways across its commands.
pub fn dispatch(
    state: &AppState,
    command: &str,
    args: &serde_json::Value,
    actor: Actor<'_>,
) -> CommandResult<serde_json::Value> {
    let a = Args { command, args };
    match command {
        // ── reads ───────────────────────────────────────────────────────────
        "watch_project" => json(project::watch_project(state, &a.project()?)?),
        "is_legacy_model" => json(project::is_legacy_model(&a.project()?)),
        "read_model" => json(project::read_model(state, &a.project()?)?),
        "read_planned" => json(project::read_planned(state, &a.project()?)?),
        "read_history" => json(project::read_history(state, &a.project()?)?),
        "read_fold_refusals" => json(project::read_fold_refusals(state, &a.project()?)?),
        "get_subagent_settings" => json(project::get_subagent_settings()),
        "get_drift_status" => json(observability::get_drift_status(state, &a.project()?)?),
        "get_model_health" => json(observability::get_model_health(state, &a.project()?)?),
        "get_test_statuses" => json(observability::get_test_statuses(state, &a.project()?)?),
        "get_probe_statuses" => json(observability::get_probe_statuses(state, &a.project()?)?),
        "read_source_span" => json(source_view::read_source_span(
            a.project()?,
            a.string("file")?,
            a.opt_string("symbol"),
            a.opt_u32("line"),
            a.opt_u32("endLine"),
        )?),
        "verify_anchor" => json(source_view::verify_anchor(
            a.project()?,
            a.string("file")?,
            a.opt_string("symbol"),
            a.opt_u32("line"),
        )),

        // ── writes ──────────────────────────────────────────────────────────
        "create_blank_model" => json(project::create_blank_model(&a.project()?)?),
        "set_subagent_settings" => json(project::set_subagent_settings(&a.parse("settings")?)?),
        "write_planned" => json(project::write_planned(
            state,
            &a.project()?,
            &a.string("data")?,
            a.opt_string("baseRevision").as_deref(),
            actor,
        )?),
        "close_change" => json(project::close_change(
            state,
            &a.project()?,
            &a.string("changeId")?,
            actor,
        )?),
        "sign_off_change" => json(project::sign_off_change(
            state,
            &a.project()?,
            &a.string("changeId")?,
            actor,
        )?),
        "reconcile_drift" => json(observability::reconcile_drift(state, &a.project()?, actor)?),
        "reconcile_drift_node" => json(observability::reconcile_drift_node(
            state,
            &a.project()?,
            &a.string("nodeId")?,
            actor,
        )?),

        // ── the eleven verdicts ─────────────────────────────────────────────
        "adopt_responsibility" => json(verdicts::adopt_responsibility(
            state,
            &a.project()?,
            a.string("respId")?,
            actor,
        )?),
        "reject_responsibility" => json(verdicts::reject_responsibility(
            state,
            &a.project()?,
            a.string("respId")?,
            actor,
        )?),
        "drop_responsibility" => json(verdicts::drop_responsibility(
            state,
            &a.project()?,
            a.string("respId")?,
            actor,
        )?),
        "reimplement_responsibility" => json(verdicts::reimplement_responsibility(
            state,
            &a.project()?,
            a.string("respId")?,
            actor,
        )?),
        "adopt_property" => json(verdicts::adopt_property(
            state,
            &a.project()?,
            a.string("nodeId")?,
            a.string("label")?,
            actor,
        )?),
        "reject_property" => json(verdicts::reject_property(
            state,
            &a.project()?,
            a.string("nodeId")?,
            a.string("label")?,
            actor,
        )?),
        "drop_property" => json(verdicts::drop_property(
            state,
            &a.project()?,
            a.string("nodeId")?,
            a.string("label")?,
            actor,
        )?),
        "reimplement_property" => json(verdicts::reimplement_property(
            state,
            &a.project()?,
            a.string("nodeId")?,
            a.string("label")?,
            actor,
        )?),
        "reword_responsibility" => json(verdicts::reword_responsibility(
            state,
            &a.project()?,
            a.string("respId")?,
            a.string("statement")?,
            actor,
        )?),
        "drop_node" => json(verdicts::drop_node(
            state,
            &a.project()?,
            a.string("nodeId")?,
            actor,
        )?),
        "reimplement_node" => json(verdicts::reimplement_node(
            state,
            &a.project()?,
            a.string("nodeId")?,
            actor,
        )?),

        // ── named, not served yet ────────────────────────────────────────────
        "start_model_build" => agents::start_model_build(command),
        "start_drift_check" => agents::start_drift_check(command),
        "cancel_agent_session" => agents::cancel_agent_session(command),
        "ensure_preview_server" => agents::ensure_preview_server(command),
        "start_preview_fixture_session" => agents::start_preview_fixture_session(command),
        "detect_ai_tools" => agents::detect_ai_tools(command),
        "setup_mcp_integration" => agents::setup_mcp_integration(command),
        "open_in_editor" => agents::open_in_editor(command),

        _ => Err(CommandError::UnknownCommand {
            command: command.to_string(),
            available: COMMANDS.to_vec(),
        }),
    }
}

fn json<T: serde::Serialize>(value: T) -> CommandResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| CommandError::failed(e.to_string()))
}

/// A command's argument bag, with the shape checks that turn a malformed call
/// into a `badArguments` refusal rather than a panic.
struct Args<'a> {
    command: &'a str,
    args: &'a serde_json::Value,
}

impl Args<'_> {
    fn bad(&self, message: impl Into<String>) -> CommandError {
        CommandError::BadArguments {
            command: self.command.to_string(),
            message: message.into(),
        }
    }

    /// The project this command is about. Upstream names it `cwd` on the
    /// observability and verdict commands, `projectPath` on the source-view
    /// ones, and `refStr` (a model ref, not a path) on the model reads — all
    /// three name one project, so all three are accepted.
    fn project(&self) -> CommandResult<String> {
        for key in ["cwd", "projectPath", "project", "projectPath"] {
            if let Some(v) = self.args.get(key).and_then(|v| v.as_str()) {
                return Ok(v.to_string());
            }
        }
        if let Some(v) = self.args.get("refStr").and_then(|v| v.as_str()) {
            let r = scryer_core::ModelRef::parse(v).map_err(|e| self.bad(e))?;
            return Ok(r.project_path().to_string_lossy().to_string());
        }
        Err(self.bad("no project: pass `cwd`, `projectPath` or `refStr`"))
    }

    fn string(&self, key: &str) -> CommandResult<String> {
        self.args
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| self.bad(format!("`{key}` is required and must be a string")))
    }

    fn opt_string(&self, key: &str) -> Option<String> {
        self.args
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    fn opt_u32(&self, key: &str) -> Option<u32> {
        self.args
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
    }

    fn parse<T: serde::de::DeserializeOwned>(&self, key: &str) -> CommandResult<T> {
        let raw = self
            .args
            .get(key)
            .ok_or_else(|| self.bad(format!("`{key}` is required")))?;
        serde_json::from_value(raw.clone())
            .map_err(|e| self.bad(format!("`{key}` has the wrong shape: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NullSink;
    use std::sync::Arc;

    /// A project with one committed claim and an empty plan.
    fn project() -> (tempfile::TempDir, AppState, String) {
        let dir = tempfile::tempdir().unwrap();
        let r = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());
        let mut m = scryer_core::ScryModel::new();
        m.nodes.push(
            serde_json::from_value(
                serde_json::json!({ "id": "node-1", "kind": "system", "name": "Acme" }),
            )
            .unwrap(),
        );
        scryer_core::write_model_at(&r, &m).unwrap();
        let state = AppState::new(Arc::new(NullSink));
        let path = dir.path().to_string_lossy().to_string();
        (dir, state, path)
    }

    /// A named command reaches the function behind it and its result comes
    /// back — the whole point of the surface. `read_model` is the shortest
    /// round trip through it that touches real state.
    #[test]
    fn a_named_command_reaches_its_function_and_returns_its_result() {
        let (_dir, state, path) = project();

        let out = dispatch(
            &state,
            "read_model",
            &serde_json::json!({ "cwd": path }),
            None,
        )
        .unwrap();
        let raw = out
            .as_str()
            .expect("read_model returns the model's raw bytes");
        assert!(raw.contains("\"Acme\""), "the model came back: {raw}");

        // A different command, a different shape: the verdict of the surface
        // is the function's own return value, not a uniform envelope.
        let legacy = dispatch(
            &state,
            "is_legacy_model",
            &serde_json::json!({ "projectPath": path }),
            None,
        )
        .unwrap();
        assert_eq!(legacy, serde_json::json!(false));
    }

    /// A name the service does not serve is refused, and the refusal names the
    /// ones it does — so a client built against a different build can see what
    /// it should have asked for instead of guessing.
    #[test]
    fn an_unknown_command_is_refused_naming_the_ones_that_exist() {
        let (_dir, state, path) = project();

        let err = dispatch(
            &state,
            "read_the_room",
            &serde_json::json!({ "cwd": path }),
            None,
        )
        .unwrap_err();

        match &err {
            CommandError::UnknownCommand { command, available } => {
                assert_eq!(command, "read_the_room");
                assert!(
                    available.contains(&"read_model"),
                    "the real names are listed"
                );
                assert_eq!(available.len(), COMMANDS.len());
            }
            other => panic!("expected unknownCommand, got {other:?}"),
        }
        assert_eq!(err.status(), 404);
        assert!(err.to_string().contains("read_model"));
    }

    /// A command that exists but this build does not serve refuses as
    /// `notImplemented`, which a client can tell apart from a typo.
    #[test]
    fn a_command_not_served_yet_refuses_as_not_implemented() {
        let (_dir, state, path) = project();
        let err = dispatch(
            &state,
            "start_model_build",
            &serde_json::json!({ "cwd": path }),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, CommandError::NotImplemented { .. }),
            "{err:?}"
        );
        assert_eq!(err.status(), 501);
    }

    /// The surface names every command the desktop's `invoke_handler` does —
    /// thirty-nine, no duplicates.
    #[test]
    fn the_surface_names_all_thirty_nine_desktop_commands() {
        let unique: std::collections::BTreeSet<_> = COMMANDS.iter().collect();
        assert_eq!(unique.len(), COMMANDS.len(), "no name is listed twice");
        assert_eq!(COMMANDS.len(), 39);
    }

    /// A call missing an argument the command needs is a `badArguments`
    /// refusal, never a panic.
    #[test]
    fn a_malformed_call_is_refused_not_panicked() {
        let (_dir, state, _path) = project();
        let err = dispatch(&state, "read_model", &serde_json::json!({}), None).unwrap_err();
        assert!(matches!(err, CommandError::BadArguments { .. }), "{err:?}");
        assert_eq!(err.status(), 400);
    }
}
