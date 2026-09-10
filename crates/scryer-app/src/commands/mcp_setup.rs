//! Wiring an agent CLI up to this project's model: which tools are installed,
//! which already know about scryer, and the config edits that connect them.
//!
//! Copied from `src-tauri/src/mcp_setup.rs` (upstream 0.4.12). The desktop
//! keeps its own copy — this crate is additive and never edits that file — so
//! each rebase diffs the two and carries the delta across.
//!
//! These commands touch config in and around the project directory, which is
//! why a host must decide for itself whether to expose them: the service will
//! run them, and answers to whoever asked.

// These are byte-for-byte copies of upstream's bodies, kept that way so a
// rebase diff against `src-tauri/` shows upstream's change and nothing else.
// Rewriting its idioms to satisfy a lint would bury that signal.
#![allow(clippy::needless_return)]

use std::path::{Path, PathBuf};

use super::preview::{find_scryer_mcp, which_on_path};

/// Check if a project has .mcp.json with a scryer entry.
fn check_mcp_json(project_path: &str) -> bool {
    has_mcp_scryer_entry(&PathBuf::from(project_path).join(".mcp.json"))
}

/// Copilot reads the SAME `.mcp.json` Claude Code does — the one scryer already
/// writes — so its MCP setup needs no file of its own. It also accepts a
/// committed `.github/mcp.json`, which `.mcp.json` overrides; a project wired
/// up by hand there is already set up, so detection honours both and the setup
/// offer stays quiet. Omitting `tools` is fine: Copilot defaults a server to
/// all tools, and it treats `"stdio"` as an alias of its own `"local"`.
fn check_copilot_mcp(project_path: &str) -> bool {
    let root = PathBuf::from(project_path);
    has_mcp_scryer_entry(&root.join(".mcp.json"))
        || has_mcp_scryer_entry(&root.join(".github").join("mcp.json"))
}

fn has_mcp_scryer_entry(path: &Path) -> bool {
    if let Ok(contents) = std::fs::read_to_string(path) {
        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) {
            return root.pointer("/mcpServers/scryer").is_some();
        }
    }
    false
}

/// Server-wide allow entry. Claude Code treats a bare `mcp__<server>` as
/// "auto-approve every tool from that server", so this one string covers all
/// scryer tools — reads and model writes alike, plus any added later. Safe
/// because scryer tools only ever mutate the git-tracked model under `.scryer/`
/// (reviewable in scryer's own diff), never source, the shell, or the network.
const SCRYER_MCP_ALLOW: &str = "mcp__scryer";

/// Check if Claude Code has auto-approved scryer tools in project settings.
fn check_claude_approved(project_path: &str) -> bool {
    // Check both settings.local.json and settings.json
    for filename in &["settings.local.json", "settings.json"] {
        let path = PathBuf::from(project_path).join(".claude").join(filename);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(allow) = root
                    .pointer("/permissions/allow")
                    .and_then(|v| v.as_array())
                {
                    if allow.iter().any(|v| v.as_str() == Some(SCRYER_MCP_ALLOW)) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// The Claude Code hook events scryer registers, with the matcher each needs.
/// One client command serves all of them (it dispatches on the event JSON).
const SCRYER_HOOK_EVENTS: &[(&str, Option<&str>, u64)] = &[
    ("SessionStart", None, 10),
    ("PostToolUse", Some("Read"), 10),
    ("PostToolUse", Some("Edit|Write|NotebookEdit"), 10),
    ("Stop", None, 15),
];

/// The Codex hook events, served by the same client command — Codex's hook
/// payloads use the same field names as Claude Code's. Reads fire no hooks
/// there, so the intent overlay rides PreToolUse on the patch instead; the
/// matcher covers both the native apply_patch tool and the Bash heredoc
/// route (the client no-ops on Bash commands with no patch envelope).
/// Timeouts stay explicit: Codex's default is 600 s.
const SCRYER_CODEX_HOOK_EVENTS: &[(&str, Option<&str>, u64)] = &[
    ("SessionStart", None, 10),
    ("PreToolUse", Some("apply_patch|Bash"), 10),
    ("PostToolUse", Some("apply_patch|Bash"), 10),
    ("Stop", None, 15),
];

/// The Copilot CLI hook events. Copilot accepts Claude Code's PascalCase event
/// names as an explicit compatibility mode, which also switches its payloads to
/// the snake_case field names the one hook client already reads, so the events
/// line up — but the tool VOCABULARY is its own (`view` to read, `create` /
/// `edit` / `str_replace_editor` / native `apply_patch` to write), and matchers
/// test the runtime name. Reads fire here like they do on Claude Code, so the
/// overlay rides post-read rather than the edit. `bash` is deliberately absent:
/// Copilot has a native patch tool, so unlike Codex there is no heredoc route
/// worth spawning this client for on every shell command.
const SCRYER_COPILOT_HOOK_EVENTS: &[(&str, Option<&str>, u64)] = &[
    ("SessionStart", None, 10),
    ("PostToolUse", Some("view"), 10),
    (
        "PostToolUse",
        Some("create|edit|str_replace_editor|apply_patch"),
        10,
    ),
    ("Stop", None, 15),
];

/// Does the command in this hook entry invoke scryer's hook client? The marker
/// `install` writes, and the only thing that identifies an entry as ours.
fn is_scryer_hook_command(command: &str) -> bool {
    command.contains("scryer-mcp")
        && command
            .trim_end()
            .trim_end_matches(" --copilot")
            .ends_with(" hook")
}

/// Does this hook entry belong to scryer? Claude Code and Codex nest the
/// commands under a `hooks` array; Copilot puts the command on the entry
/// itself. Both shapes are checked so one predicate serves every install.
fn is_scryer_hook_entry(entry: &serde_json::Value) -> bool {
    if entry["command"]
        .as_str()
        .is_some_and(is_scryer_hook_command)
    {
        return true;
    }
    entry["hooks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["command"].as_str())
        .any(is_scryer_hook_command)
}

/// Check if Claude Code has scryer's session hooks installed for the project.
fn check_claude_hooks(project_path: &str) -> bool {
    for filename in &["settings.local.json", "settings.json"] {
        let path = PathBuf::from(project_path).join(".claude").join(filename);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) {
                let installed = root
                    .pointer("/hooks/SessionStart")
                    .and_then(|v| v.as_array())
                    .is_some_and(|entries| entries.iter().any(is_scryer_hook_entry));
                if installed {
                    return true;
                }
            }
        }
    }
    false
}

/// Is this `statusLine` entry ours? Same marker as the CLI's `install_statusline`
/// (mirrored here, like `is_scryer_hook_entry`): the command invokes the
/// scryer-mcp binary's `statusline` subcommand.
fn is_scryer_statusline(entry: &serde_json::Value) -> bool {
    entry["command"]
        .as_str()
        .is_some_and(|c| c.contains("scryer-mcp") && c.trim_end().ends_with(" statusline"))
}

/// The project's Claude Code `statusLine` state as `(ours, foreign)`. Unlike
/// hooks (a merging list), `statusLine` is a SINGLE slot — a whole-line
/// replacement — so a foreign entry is never clobbered: `foreign` lets the UI
/// surface it instead of offering an install that would overwrite it.
fn check_claude_statusline(project_path: &str) -> (bool, bool) {
    for filename in &["settings.local.json", "settings.json"] {
        let path = PathBuf::from(project_path).join(".claude").join(filename);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) {
                match root.get("statusLine") {
                    Some(entry) if entry.is_null() => continue,
                    Some(entry) if is_scryer_statusline(entry) => return (true, false),
                    Some(_) => return (false, true),
                    None => continue,
                }
            }
        }
    }
    (false, false)
}

/// Check if Codex has scryer's session hooks installed for the project.
fn check_codex_hooks(project_path: &str) -> bool {
    let path = PathBuf::from(project_path)
        .join(".codex")
        .join("hooks.json");
    if let Ok(contents) = std::fs::read_to_string(&path) {
        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) {
            return root
                .pointer("/hooks/SessionStart")
                .and_then(|v| v.as_array())
                .is_some_and(|entries| entries.iter().any(is_scryer_hook_entry));
        }
    }
    false
}

/// Where Copilot's hook registration goes. `.github/hooks/` is the only
/// project-scoped location Copilot actually loads: the `hooks` key in
/// `.github/copilot/settings.local.json` is documented but inert as of 1.0.61
/// (verified against real sessions), and the user-level hooks directory is
/// global to every project rather than an opt-in for this one.
///
/// It is a COMMITTED path, unlike Claude Code's `settings.local.json` — so this
/// registration is shared with the checkout, the same way `.codex/hooks.json`
/// already is. That costs teammates nothing: the registered command exits in
/// milliseconds unless the Scryer app has this project open, so a checkout
/// without Scryer — including a CI run of the Copilot cloud agent, which reads
/// exactly this directory — sees no behaviour at all.
///
/// Scryer owns this file outright (hence its own name in a directory Copilot
/// reads whole), which is what lets it be written wholesale rather than merged.
fn copilot_hooks_path(project_path: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(".github")
        .join("hooks")
        .join("scryer.json")
}

/// Check if Copilot CLI has scryer's session hooks installed for the project.
fn check_copilot_hooks(project_path: &str) -> bool {
    if let Ok(contents) = std::fs::read_to_string(copilot_hooks_path(project_path)) {
        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) {
            return root
                .pointer("/hooks/SessionStart")
                .and_then(|v| v.as_array())
                .is_some_and(|entries| entries.iter().any(is_scryer_hook_entry));
        }
    }
    false
}

/// Check if a project has .codex/config.toml with a scryer MCP entry.
fn check_codex_toml(project_path: &str) -> bool {
    let path = PathBuf::from(project_path)
        .join(".codex")
        .join("config.toml");
    if let Ok(contents) = std::fs::read_to_string(&path) {
        if let Ok(doc) = contents.parse::<toml_edit::DocumentMut>() {
            return doc
                .get("mcp_servers")
                .and_then(|t| t.as_table())
                .map(|t| t.contains_key("scryer"))
                .unwrap_or(false);
        }
    }
    false
}

pub fn detect_ai_tools(project_path: Option<String>) -> serde_json::Value {
    let has_claude = which_on_path("claude").is_some();
    let has_codex = which_on_path("codex").is_some();
    let has_copilot = which_on_path("copilot").is_some();

    let claude_mcp = project_path.as_deref().map(check_mcp_json).unwrap_or(false);
    let codex_mcp = project_path
        .as_deref()
        .map(check_codex_toml)
        .unwrap_or(false);
    let copilot_mcp = project_path
        .as_deref()
        .map(check_copilot_mcp)
        .unwrap_or(false);
    let claude_approved = project_path
        .as_deref()
        .map(check_claude_approved)
        .unwrap_or(false);
    let claude_hooks = project_path
        .as_deref()
        .map(check_claude_hooks)
        .unwrap_or(false);
    let codex_hooks = project_path
        .as_deref()
        .map(check_codex_hooks)
        .unwrap_or(false);
    let copilot_hooks = project_path
        .as_deref()
        .map(check_copilot_hooks)
        .unwrap_or(false);
    let (claude_statusline, claude_statusline_foreign) = project_path
        .as_deref()
        .map(check_claude_statusline)
        .unwrap_or((false, false));

    serde_json::json!({
        "claude": has_claude,
        "codex": has_codex,
        "copilot": has_copilot,
        "claudeMcpEnabled": claude_mcp,
        "codexMcpEnabled": codex_mcp,
        "copilotMcpEnabled": copilot_mcp,
        "claudeApproved": claude_approved,
        "claudeHooksEnabled": claude_hooks,
        "codexHooksEnabled": codex_hooks,
        "copilotHooksEnabled": copilot_hooks,
        "claudeStatuslineEnabled": claude_statusline,
        "claudeStatuslineForeign": claude_statusline_foreign,
    })
}

pub fn setup_mcp_integration(action: String, project_path: String) -> Result<String, String> {
    match action.as_str() {
        "mcp" => {
            let binary_path = find_scryer_mcp().ok_or("scryer-mcp binary not found")?;

            let mcp_path = PathBuf::from(&project_path).join(".mcp.json");
            let mut mcp_root: serde_json::Value = if mcp_path.exists() {
                let contents = std::fs::read_to_string(&mcp_path).map_err(|e| e.to_string())?;
                serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            if !mcp_root.get("mcpServers").is_some_and(|v| v.is_object()) {
                mcp_root["mcpServers"] = serde_json::json!({});
            }
            mcp_root["mcpServers"]["scryer"] = serde_json::json!({
                "type": "stdio",
                "command": binary_path,
                "args": [],
            });

            std::fs::write(
                &mcp_path,
                serde_json::to_string_pretty(&mcp_root).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;

            return Ok(mcp_path.to_string_lossy().to_string());
        }
        "mcp_codex" => {
            let binary_path = find_scryer_mcp().ok_or("scryer-mcp binary not found")?;

            let codex_dir = PathBuf::from(&project_path).join(".codex");
            let config_path = codex_dir.join("config.toml");

            let mut doc: toml_edit::DocumentMut = if config_path.exists() {
                std::fs::read_to_string(&config_path)
                    .map_err(|e| e.to_string())?
                    .parse()
                    .unwrap_or_default()
            } else {
                toml_edit::DocumentMut::new()
            };

            if !doc.contains_table("mcp_servers") {
                doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let mut server = toml_edit::Table::new();
            server.insert("command", toml_edit::value(&binary_path));
            server.insert("args", toml_edit::value(toml_edit::Array::new()));
            doc["mcp_servers"]["scryer"] = toml_edit::Item::Table(server);

            std::fs::create_dir_all(&codex_dir).map_err(|e| e.to_string())?;
            std::fs::write(&config_path, doc.to_string()).map_err(|e| e.to_string())?;

            return Ok(config_path.to_string_lossy().to_string());
        }
        "claude_approve" => {
            let claude_dir = PathBuf::from(&project_path).join(".claude");
            let settings_path = claude_dir.join("settings.local.json");

            let mut root: serde_json::Value = if settings_path.exists() {
                let contents =
                    std::fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
                serde_json::from_str(&contents).map_err(|e| {
                    format!(
                        "{} is not valid JSON ({e}); refusing to overwrite it — fix the file and retry.",
                        settings_path.display()
                    )
                })?
            } else {
                serde_json::json!({})
            };

            if !root
                .pointer("/permissions/allow")
                .is_some_and(|v| v.is_array())
            {
                root["permissions"] = serde_json::json!({ "allow": [] });
            }

            let allow = root
                .pointer_mut("/permissions/allow")
                .unwrap()
                .as_array_mut()
                .unwrap();
            if !allow.iter().any(|v| v.as_str() == Some(SCRYER_MCP_ALLOW)) {
                allow.push(serde_json::json!(SCRYER_MCP_ALLOW));
            }

            std::fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;
            std::fs::write(
                &settings_path,
                serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;

            return Ok(settings_path.to_string_lossy().to_string());
        }
        "claude_hooks" => {
            let binary_path = find_scryer_mcp().ok_or("scryer-mcp binary not found")?;
            return write_claude_hooks(&project_path, &binary_path);
        }
        "codex_hooks" => {
            let binary_path = find_scryer_mcp().ok_or("scryer-mcp binary not found")?;
            return write_codex_hooks(&project_path, &binary_path);
        }
        "copilot_hooks" => {
            let binary_path = find_scryer_mcp().ok_or("scryer-mcp binary not found")?;
            return write_copilot_hooks(&project_path, &binary_path);
        }
        "claude_statusline" => {
            let binary_path = find_scryer_mcp().ok_or("scryer-mcp binary not found")?;
            return write_claude_statusline(&project_path, &binary_path);
        }
        _ => Err(format!("Unknown action: {}", action)),
    }
}

/// Explicit, per-project opt-in: write scryer's session-hook registrations
/// into the personal settings file. The registered command no-ops in
/// milliseconds unless the app has this project open, so installed hooks
/// impose nothing on sessions where the user leaves Scryer closed.
fn write_claude_hooks(project_path: &str, binary_path: &str) -> Result<String, String> {
    let claude_dir = PathBuf::from(project_path).join(".claude");
    write_scryer_hooks(
        &claude_dir,
        &claude_dir.join("settings.local.json"),
        SCRYER_HOOK_EVENTS,
        binary_path,
    )
}

/// Register scryer's status one-liner as this project's Claude Code statusLine,
/// in the personal settings file (same conventions as the hook install: absolute
/// binary path, refuse to overwrite invalid JSON). A separate opt-in from the
/// session hooks because it's the only surface that survives Scryer being closed
/// — `scryer-mcp statusline` reads the model straight off disk. Mirrors
/// `install_statusline` in the scryer-mcp crate. `statusLine` is a SINGLE slot,
/// so a foreign entry is never clobbered: the write errors and the caller (which
/// detected the foreign line via `check_claude_statusline`) surfaces it instead.
fn write_claude_statusline(project_path: &str, binary_path: &str) -> Result<String, String> {
    let claude_dir = PathBuf::from(project_path).join(".claude");
    let settings_path = claude_dir.join("settings.local.json");

    let mut root = read_json_or_refuse(&settings_path)?;

    if root
        .get("statusLine")
        .is_some_and(|e| !e.is_null() && !is_scryer_statusline(e))
    {
        return Err(format!(
            "A status line is already configured in {} — left untouched.",
            settings_path.display()
        ));
    }

    root["statusLine"] = serde_json::json!({
        "type": "command",
        "command": format!("\"{binary_path}\" statusline"),
    });

    std::fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(settings_path.to_string_lossy().to_string())
}

/// The same opt-in for Codex: write the registrations into the project's
/// `.codex/hooks.json` (Codex merges it with any user-level hooks, and loads
/// it once the `.codex/` layer is trusted). The registered command is the
/// same `… hook` client, so the inert-while-Scryer-is-closed economics hold.
fn write_codex_hooks(project_path: &str, binary_path: &str) -> Result<String, String> {
    let codex_dir = PathBuf::from(project_path).join(".codex");
    write_scryer_hooks(
        &codex_dir,
        &codex_dir.join("hooks.json"),
        SCRYER_CODEX_HOOK_EVENTS,
        binary_path,
    )
}

/// The same opt-in for Copilot CLI. Copilot reads every `*.json` in
/// `.github/hooks/`, so scryer takes a file of its own and writes it whole —
/// no merge pass, and nothing of anyone else's to preserve or corrupt, which is
/// the one simplification this location buys over the shared settings files the
/// other two installs have to edit in place. Re-installing is therefore
/// idempotent by construction, and a previously corrupted file is repaired
/// rather than refused (it is only ever scryer's own).
///
/// The entry schema is Copilot's: FLAT, with the command on the entry rather
/// than nested under a `hooks` array. `timeout` is written rather than
/// Copilot's own `timeoutSec` because it normalises one to the other, keeping a
/// single vocabulary across the three installs. The registered command carries
/// `--copilot` so the client knows whose tool names and reply shape to speak.
fn write_copilot_hooks(project_path: &str, binary_path: &str) -> Result<String, String> {
    let path = copilot_hooks_path(project_path);
    let dir = path
        .parent()
        .ok_or("no parent directory for the Copilot hooks file")?
        .to_path_buf();
    let command = format!("\"{binary_path}\" hook --copilot");

    let mut hooks = serde_json::Map::new();
    for (event, matcher, timeout) in SCRYER_COPILOT_HOOK_EVENTS {
        let mut entry = serde_json::json!({
            "type": "command",
            "command": command,
            "timeout": timeout,
        });
        if let Some(m) = matcher {
            entry["matcher"] = serde_json::json!(m);
        }
        hooks
            .entry(event.to_string())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .unwrap()
            .push(entry);
    }
    let root = serde_json::json!({ "version": 1, "hooks": hooks });

    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(path.to_string_lossy().to_string())
}

/// Read a settings file we're about to merge into. A file that doesn't parse is
/// an ERROR, never a blank slate: a stray comma in the user's config must not
/// cost them everything else the file holds.
fn read_json_or_refuse(path: &Path) -> Result<serde_json::Value, String> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let contents = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&contents).map_err(|e| {
        format!(
            "{} is not valid JSON ({e}); refusing to overwrite it — fix the file and retry.",
            path.display()
        )
    })
}

/// Idempotent hook install into one file's `{"hooks": {...}}` block — Claude
/// Code and Codex use the same entry schema. Two passes because an event may
/// get two scryer entries: first strip every prior scryer entry per event (they
/// all share the `… hook` command marker; foreign hooks are kept), then
/// append the current set.
fn write_scryer_hooks(
    dir: &Path,
    file_path: &Path,
    events: &[(&str, Option<&str>, u64)],
    binary_path: &str,
) -> Result<String, String> {
    let command = format!("\"{}\" hook", binary_path);

    let mut root = read_json_or_refuse(file_path)?;

    if !root.get("hooks").is_some_and(|v| v.is_object()) {
        root["hooks"] = serde_json::json!({});
    }
    for (event, _, _) in events {
        let entries = root["hooks"]
            .as_object_mut()
            .unwrap()
            .entry(event.to_string())
            .or_insert_with(|| serde_json::json!([]));
        if !entries.is_array() {
            *entries = serde_json::json!([]);
        }
        entries
            .as_array_mut()
            .unwrap()
            .retain(|e| !is_scryer_hook_entry(e));
    }
    for (event, matcher, timeout) in events {
        let mut entry = serde_json::json!({
            "hooks": [{ "type": "command", "command": command, "timeout": timeout }],
        });
        if let Some(m) = matcher {
            entry["matcher"] = serde_json::json!(m);
        }
        root["hooks"][event].as_array_mut().unwrap().push(entry);
    }

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(
        file_path,
        serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(file_path.to_string_lossy().to_string())
}
