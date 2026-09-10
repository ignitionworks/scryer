//! `scryer-serve` — the engine service on its own, with no desktop app.
//!
//! ```text
//! scryer-serve --bind 127.0.0.1:7400 --project /path/to/repo [--project …] [--ui <dir>]
//! ```
//!
//! Binds loopback by default on purpose: the service has no identity of its
//! own and never will (see the `Engine Service` directive), so the safe
//! default audience is this machine. Serving it wider is a host's decision,
//! made by putting a host in front of it.

use std::net::SocketAddr;
use std::path::PathBuf;

use scryer_app::events::Broadcast;
use scryer_app::state::AppState;

struct Options {
    bind: SocketAddr,
    projects: Vec<PathBuf>,
    ui: Option<PathBuf>,
}

const USAGE: &str = "\
scryer-serve — Scryer's command surface over HTTP + SSE

    scryer-serve [--bind ADDR] --project PATH [--project PATH …] [--ui DIR]

    --bind ADDR     address to listen on (default 127.0.0.1:7400)
    --project PATH  a project to serve; repeat for more than one
    --ui DIR        also serve this directory as static files at /
";

fn parse_args() -> Result<Options, String> {
    let mut bind: SocketAddr = "127.0.0.1:7400".parse().unwrap();
    let mut projects = Vec::new();
    let mut ui = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                let v = args.next().ok_or("--bind needs an address")?;
                bind = v.parse().map_err(|e| format!("--bind {v}: {e}"))?;
            }
            "--project" => {
                projects.push(PathBuf::from(args.next().ok_or("--project needs a path")?));
            }
            "--ui" => {
                ui = Some(PathBuf::from(args.next().ok_or("--ui needs a directory")?));
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}")),
        }
    }
    if projects.is_empty() {
        return Err(format!("no --project given\n\n{USAGE}"));
    }
    Ok(Options { bind, projects, ui })
}

#[tokio::main]
async fn main() {
    let options = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    let events = Broadcast::new(1024);
    let app = AppState::new(events.clone());

    for project in &options.projects {
        match app.add_project(project) {
            Ok(p) => eprintln!("[serve] {} ({})", p.path().display(), p.ref_string()),
            Err(e) => {
                eprintln!("[serve] {}: {e}", project.display());
                std::process::exit(1);
            }
        }
    }

    let mut router = scryer_app::router::router(app, events);
    if let Some(dir) = &options.ui {
        // A single-page app: unknown paths fall back to its index so the
        // frontend's own routing works, while /api stays the service's.
        router = router.fallback_service(
            tower_http::services::ServeDir::new(dir)
                .fallback(tower_http::services::ServeFile::new(dir.join("index.html"))),
        );
        eprintln!("[serve] ui from {}", dir.display());
    }

    let listener = match tokio::net::TcpListener::bind(options.bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[serve] cannot bind {}: {e}", options.bind);
            std::process::exit(1);
        }
    };
    eprintln!("[serve] listening on http://{}", options.bind);

    // `into_make_service_with_connect_info` is what puts the peer address in
    // front of the router — the loopback check on `X-Actor` needs it.
    if let Err(e) = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    {
        eprintln!("[serve] {e}");
        std::process::exit(1);
    }
}
