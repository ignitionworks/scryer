//! The preview sidecar, and the agent run that repairs a failed render.
//!
//! Copied from `src-tauri/src/preview.rs` (upstream 0.4.12). The desktop keeps
//! its own copy — this crate is additive and never edits that file — so each
//! rebase diffs the two and carries the delta across.
//!
//! Differences: the sidecar and the agent runtime are PER PROJECT rather than
//! one apiece for the whole app (see `agent_state.rs`), and agent events go to
//! the [`EventSink`] instead of a Tauri window, named with the project they
//! came from.

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{CommandError, CommandResult};
use crate::events::{Event, EventSink};
use crate::state::AppState;

/// Resolve (model, effort) for the agent kind about to launch, from its
/// per-agent settings. An empty model means "use the agent CLI's own default".
pub(crate) fn config_for_launch(
    s: &scryer_core::SubagentSettings,
    launch: &scryer_acp::AgentLaunch,
) -> (String, String) {
    match launch {
        scryer_acp::AgentLaunch::Cli {
            kind: scryer_acp::AgentKind::ClaudeCode,
            ..
        } => (s.claude.model.clone(), s.claude.effort.clone()),
        scryer_acp::AgentLaunch::Cli {
            kind: scryer_acp::AgentKind::Codex,
            ..
        } => (s.codex.model.clone(), s.codex.effort.clone()),
        scryer_acp::AgentLaunch::Acp {
            kind: scryer_acp::AcpKind::Copilot,
            ..
        } => (s.copilot.model.clone(), s.copilot.effort.clone()),
        _ => (String::new(), "medium".to_string()),
    }
}

/// The `scryer-mcp` binary the agent is given, found beside this executable or
/// on PATH. Mirrors `src-tauri/src/mcp_setup.rs::find_scryer_mcp`.
pub(crate) fn find_scryer_mcp() -> Option<String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(sibling) = exe.parent().map(|p| p.join("scryer-mcp")) {
            if sibling.exists() {
                return Some(sibling.to_string_lossy().to_string());
            }
        }
    }
    which_on_path("scryer-mcp")
}

/// A bare `which`, so the service does not pull a crate in for one lookup.
pub(crate) fn which_on_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate
            .is_file()
            .then(|| candidate.to_string_lossy().to_string())
    })
}

/// The preview sidecar's sources, embedded at compile time and written to
/// `{project}/.scryer/preview/server/` before launch, so the spawned `node`
/// process always runs the version matching this binary. The sidecar has no
/// npm dependencies of its own — it resolves `vite` and `typescript` from the
/// target project's node_modules.
const PREVIEW_SIDECAR: &[(&str, &str)] = &[
    ("server.mjs", include_str!("../../../../preview/server.mjs")),
    ("plugin.mjs", include_str!("../../../../preview/plugin.mjs")),
    ("props.mjs", include_str!("../../../../preview/props.mjs")),
];

/// Start (or reuse) the project's preview dev server and return its base URL.
/// Deterministic rendering: any component export it discovers is viewable at
/// `{url}/__preview?file=…&export=…` with no agent involvement.
pub async fn ensure_preview_server(state: &AppState, project_path: &str) -> CommandResult<String> {
    use tokio::io::AsyncBufReadExt;

    let project = state.add_project(std::path::Path::new(project_path))?;
    let cwd = project.path().to_string_lossy().to_string();
    let mut guard = project.preview().0.lock().await;

    // Reuse a live server for this project; replace a dead one.
    if let Some(srv) = guard.as_mut() {
        if matches!(srv.child.try_wait(), Ok(None)) && srv.cwd == cwd {
            return Ok(srv.url.clone());
        }
        let _ = srv.child.kill().await;
        *guard = None;
    }

    let dir = PathBuf::from(&cwd)
        .join(".scryer")
        .join("preview")
        .join("server");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (name, source) in PREVIEW_SIDECAR {
        std::fs::write(dir.join(name), source).map_err(|e| e.to_string())?;
    }

    let mut child = tokio::process::Command::new("node")
        .arg(dir.join("server.mjs"))
        .arg(&cwd)
        .arg("--exit-on-stdin-close")
        .current_dir(&cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            CommandError::failed(format!(
                "failed to launch preview server (is node installed?): {e}"
            ))
        })?;

    // Collect a stderr tail in the background so a startup failure can say WHY
    // the sidecar died, not just that it did. Keeps draining for the server's
    // lifetime so the pipe never fills.
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "preview server has no stderr".to_string())?;
    let stderr_tail = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        let mut tail = std::collections::VecDeque::with_capacity(20);
        while let Ok(Some(line)) = lines.next_line().await {
            if tail.len() >= 20 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "preview server has no stdout".to_string())?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let url = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(url) = line.strip_prefix("SCRYER_PREVIEW_URL=") {
                return Ok(url.trim().to_string());
            }
        }
        Err("preview server exited before reporting its URL".to_string())
    })
    .await
    .unwrap_or_else(|_| Err("preview server startup timed out".to_string()));
    let url = match url {
        Ok(url) => url,
        Err(e) => {
            let _ = child.kill().await;
            let tail = tokio::time::timeout(std::time::Duration::from_secs(2), stderr_tail)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
            return Err(CommandError::failed(if tail.is_empty() {
                e
            } else {
                format!("{e}\n{tail}")
            }));
        }
    };

    // Keep draining stdout so the sidecar never blocks (or dies on EPIPE)
    // writing logs after we stop caring about them.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

    *guard = Some(crate::commands::agent_state::PreviewServer {
        cwd,
        url: url.clone(),
        child,
    });
    Ok(url)
}

/// Find a node's anchored source file: the node-id source-map entry (data
/// shapes) or the first responsibility's source location.
fn node_source_file(model: &scryer_core::ScryModel, node_id: &str) -> String {
    let from_node = model
        .source_map
        .get(node_id)
        .and_then(|locs| locs.first())
        .map(|loc| loc.pattern.clone());
    from_node
        .or_else(|| {
            model
                .nodes
                .iter()
                .find(|n| n.id == node_id)
                .and_then(|node| {
                    node.responsibilities
                        .iter()
                        .find_map(|r| model.source_map.get(&r.id))
                        .and_then(|locs| locs.first())
                        .map(|loc| loc.pattern.clone())
                })
        })
        .unwrap_or_default()
}

/// Repair path for a failed deterministic render. The preview server renders
/// components with synthesized placeholder props; when that comes out empty or
/// crashes, this launches an agent that authors realistic data — primarily a
/// shared, type-keyed fixture set (`.scryer/preview/fixtures/`) reused across
/// every component touching a type, with a per-node override as fallback. The
/// preview server picks the files up automatically — no build step.
pub async fn start_preview_fixture_session(
    state: &AppState,
    project_path: &str,
    node_id: &str,
    render_status: &str,
    render_error: Option<&str>,
) -> CommandResult<String> {
    let project = state.add_project(std::path::Path::new(project_path))?;
    let cwd = project.path().to_string_lossy().to_string();
    let model_ref = project.model_ref.clone();

    let mcp_binary = find_scryer_mcp().ok_or_else(|| "scryer-mcp binary not found".to_string())?;
    let settings = scryer_core::read_subagent_settings();
    let launch = scryer_acp::detect_available_agent_pref(&settings.agent).ok_or_else(|| {
        "No AI agent found. Install Claude Code, Codex or Copilot CLI first.".to_string()
    })?;

    let model = scryer_core::read_model_at(&model_ref)?;
    let node = model
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .ok_or_else(|| CommandError::failed(format!("Node '{node_id}' not found in model")))?;
    let node_name = node.name.clone();

    let source_file = node_source_file(&model, node_id);
    let source_lines = if source_file.is_empty() {
        String::new()
    } else {
        std::fs::read_to_string(PathBuf::from(&cwd).join(&source_file)).unwrap_or_default()
    };

    let prompt = scryer_acp::prompt::preview_fixture_prompt(
        &cwd,
        node_id,
        &node_name,
        &source_file,
        &source_lines,
        render_status,
        render_error.unwrap_or(""),
    );
    let (model_name, effort) = config_for_launch(&settings, &launch);

    let runtime = project.agents().runtime();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
    forward_agent_events(state.sink().clone(), project.path().to_path_buf(), event_rx);

    let (agent_binary, mode) = match launch {
        scryer_acp::AgentLaunch::Cli { binary, kind } => {
            (binary, scryer_acp::runtime::LaunchMode::Cli { kind })
        }
        scryer_acp::AgentLaunch::Acp { binary, kind } => {
            (binary, scryer_acp::runtime::LaunchMode::Acp { kind })
        }
    };
    let allowed_tools = vec![
        "mcp__scryer__*".into(),
        "Write".into(),
        "Edit".into(),
        "Bash".into(),
    ];

    Ok(runtime
        .start_session(
            agent_binary,
            mode,
            cwd,
            model_name,
            effort,
            mcp_binary,
            prompt,
            format!("Preview fixture: {node_name}"),
            allowed_tools,
            event_tx,
        )
        .await?)
}

/// Pump one agent run's events onto the service's stream, each named with the
/// project it came from — the desktop's `app.emit("agent-event", …)` for a
/// service that has more than one project in flight.
pub(crate) fn forward_agent_events(
    sink: Arc<dyn EventSink>,
    project: PathBuf,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<scryer_acp::AgentEvent>,
) {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let payload = serde_json::to_value(&event).unwrap_or_default();
            sink.publish(Event::agent_event(&project, payload));
        }
    });
}

/// Stop whatever the project's agent runtime is doing.
///
/// Raises the durable cancel flag FIRST so orchestrators stop launching new
/// sessions even if the runtime currently has none (a wave gap) or a queued
/// session is about to start; then best-effort kills any live session. A
/// project whose runtime never started has nothing to stop and says so quietly.
pub async fn cancel_agent_session(state: &AppState, project_path: &str) -> CommandResult<()> {
    let project = state.add_project(std::path::Path::new(project_path))?;
    project.agents().cancel();
    if let Some(runtime) = project.agents().started() {
        let _ = runtime.cancel().await;
    }
    Ok(())
}
