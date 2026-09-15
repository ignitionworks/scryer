//! The `scryer-mcp` binary: a thin dispatcher over the library. The tools, the
//! server handler and the CLI subcommands all live in `scryer_mcp` (see
//! `EMBEDDING.md`), so an embedder that calls the handlers in-process runs the
//! same code this binary serves over stdio.

use rmcp::ServiceExt;
use scryer_mcp::ScryerServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Handle subcommands
    match std::env::args().nth(1).as_deref() {
        Some("init") => {
            let mut statusline = false;
            for a in std::env::args().skip(2) {
                match a.as_str() {
                    "--statusline" => statusline = true,
                    other => {
                        eprintln!(
                            "unknown argument '{other}'\nusage: scryer-mcp init [--statusline]"
                        );
                        std::process::exit(2);
                    }
                }
            }
            return scryer_mcp::init::init_project(statusline);
        }
        // Agent session hook: event JSON on stdin, hook JSON on stdout. Takes
        // `--copilot` because Copilot's tool names and reply shape differ and
        // the event can't be sniffed for them — the install writes the flag.
        // Silent no-op unless the Scryer app has this project open.
        Some("hook") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            return scryer_mcp::run_hook_client(&args);
        }
        // Loop-state one-liner for humans, straight from disk (no app needed).
        Some("status") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            return scryer_mcp::cli::run_status(&args);
        }
        // Claude Code statusline command: session JSON on stdin, one line out.
        // Prints nothing when no model is found.
        Some("statusline") => return scryer_mcp::cli::run_statusline(),
        // Opt-in CI gate: exit 0 clean, 1 findings, 2 unusable.
        Some("check") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            return scryer_mcp::cli::run_check(&args);
        }
        _ => {}
    }

    let service = ScryerServer::new()
        .serve(rmcp::transport::io::stdio())
        .await
        .inspect_err(|e| eprintln!("MCP server error: {}", e))?;
    service.waiting().await?;
    Ok(())
}
