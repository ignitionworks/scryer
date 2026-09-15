//! `scryer-mcp init` — write the project-scoped MCP config the installed agent
//! CLIs read, so they discover this server when working in the project. Lives
//! in the library so the binary stays a thin dispatcher over it.

use crate::cli;

/// Write project-scoped MCP config files in the current directory so that the
/// installed agent CLIs discover scryer-mcp when working in this project. Only
/// writes config for tools that are actually installed. With `statusline`, also
/// register the model's status one-liner as Claude Code's statusline (never
/// clobbering a foreign one).
pub fn init_project(statusline: bool) -> Result<(), Box<dyn std::error::Error>> {
    let binary_path = std::env::current_exe()?
        .canonicalize()?
        .to_string_lossy()
        .to_string();

    let cwd = std::env::current_dir()?;

    let has_claude = which("claude");
    let has_codex = which("codex");
    let has_copilot = which("copilot");

    if !has_claude && !has_codex && !has_copilot {
        eprintln!("None of `claude`, `codex` or `copilot` found in PATH.");
        eprintln!("Install Claude Code, OpenAI Codex or GitHub Copilot CLI first, then re-run `scryer-mcp init`.");
        std::process::exit(1);
    }

    let mut wrote_any = false;

    // One `.mcp.json` serves both Claude Code and Copilot CLI — they read the
    // same file, so this is written once when either is installed.
    if has_claude || has_copilot {
        init_mcp_json(&cwd, &binary_path)?;
        wrote_any = true;
    }

    if has_claude && statusline {
        match cli::install_statusline(&cwd, &binary_path)? {
            cli::StatuslineInstall::Installed(path) => {
                eprintln!("Registered the scryer statusline in {}", path.display());
            }
            // statusLine is a single slot (whole-line replacement), so a
            // foreign entry is composed with by hand, never clobbered.
            cli::StatuslineInstall::ForeignExists(path) => {
                eprintln!(
                    "A statusLine is already configured in {} — left untouched.",
                    path.display()
                );
                eprintln!("To add scryer to it, append this to your statusline script's output:");
                eprintln!("  \"{binary_path}\" statusline");
            }
        }
    } else if statusline {
        eprintln!("--statusline is a Claude Code integration; `claude` was not found in PATH.");
    }

    if has_codex {
        init_codex(&cwd, &binary_path)?;
        wrote_any = true;
    }

    if wrote_any {
        let tools: Vec<&str> = [
            if has_claude {
                Some("Claude Code")
            } else {
                None
            },
            if has_codex { Some("Codex") } else { None },
            if has_copilot {
                Some("Copilot CLI")
            } else {
                None
            },
        ]
        .into_iter()
        .flatten()
        .collect();
        eprintln!(
            "\nDone. {} will use scryer in this project.",
            tools.join(" and ")
        );
        if has_claude {
            eprintln!("\nTo auto-approve scryer tools in Claude Code, add to .claude/settings.local.json:");
            eprintln!("  \"permissions\": {{ \"allow\": [\"mcp__scryer\"] }}");
        }
        if has_claude && !statusline {
            eprintln!("\nTip: `scryer-mcp init --statusline` puts the model's status line in Claude Code's prompt.");
        }
    }

    Ok(())
}

fn which(name: &str) -> bool {
    // Check PATH for the given binary
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(name);
                candidate.is_file() || dir.join(format!("{name}.exe")).is_file()
            })
        })
        .unwrap_or(false)
}

/// Write `.mcp.json`, merging with any existing config. Both Claude Code and
/// Copilot CLI read this one file, so it is written once for either. Copilot
/// treats `"stdio"` as an alias of its own `"local"` type and defaults an entry
/// with no `tools` key to every tool, so the Claude-shaped entry serves both
/// verbatim.
fn init_mcp_json(
    cwd: &std::path::Path,
    binary_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mcp_json_path = cwd.join(".mcp.json");
    let mut root: serde_json::Value = if mcp_json_path.exists() {
        let contents = std::fs::read_to_string(&mcp_json_path)?;
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !root.get("mcpServers").is_some_and(|v| v.is_object()) {
        root["mcpServers"] = serde_json::json!({});
    }
    root["mcpServers"]["scryer"] = serde_json::json!({
        "type": "stdio",
        "command": binary_path,
        "args": [],
    });

    std::fs::write(&mcp_json_path, serde_json::to_string_pretty(&root)?)?;
    eprintln!("Wrote {}", mcp_json_path.display());
    Ok(())
}

/// Write .codex/config.toml for OpenAI Codex, merging with any existing config.
fn init_codex(cwd: &std::path::Path, binary_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let codex_dir = cwd.join(".codex");
    let config_toml_path = codex_dir.join("config.toml");

    let mut doc: toml_edit::DocumentMut = if config_toml_path.exists() {
        std::fs::read_to_string(&config_toml_path)?
            .parse()
            .unwrap_or_default()
    } else {
        toml_edit::DocumentMut::new()
    };

    if !doc.contains_table("mcp_servers") {
        doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let mut server = toml_edit::Table::new();
    server.insert("command", toml_edit::value(binary_path));
    server.insert("args", toml_edit::value(toml_edit::Array::new()));
    doc["mcp_servers"]["scryer"] = toml_edit::Item::Table(server);

    std::fs::create_dir_all(&codex_dir)?;
    std::fs::write(&config_toml_path, doc.to_string())?;
    eprintln!("Wrote {}", config_toml_path.display());
    Ok(())
}
