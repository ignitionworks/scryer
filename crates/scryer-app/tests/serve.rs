//! `scryer-serve` end to end: start the binary against a real project, then
//! drive it the way a host does — commands over HTTP, events over SSE.
//!
//! These are the container/binary-level tests: the unit tests prove each piece
//! in isolation, this proves the piece a host actually talks to.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A running `scryer-serve` on a port of its own, killed when the test ends.
struct Serve {
    child: Child,
    port: u16,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Serve {
    fn start(project: &Path) -> Self {
        // Port 0 is not an option (the binary reports its address, not picks
        // one), so take a free port from the OS and hand it over.
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let child = Command::new(env!("CARGO_BIN_EXE_scryer-serve"))
            .args([
                "--bind",
                &format!("127.0.0.1:{port}"),
                "--project",
                &project.to_string_lossy(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("scryer-serve starts");

        let serve = Serve { child, port };
        serve.wait_until_listening();
        serve
    }

    fn wait_until_listening(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("scryer-serve never bound 127.0.0.1:{}", self.port);
    }

    fn connect(&self) -> TcpStream {
        connect(self.port)
    }

    /// `POST /api/cmd/{name}`; returns `(status, body)`.
    fn post(&self, name: &str, body: &str, actor: Option<&str>) -> (u16, String) {
        post(self.port, name, body, actor)
    }

    fn get(&self, path: &str) -> (u16, String) {
        let mut s = self.connect();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).unwrap();
        split_response(&raw)
    }
}

fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s
}

/// `POST /api/cmd/{name}` against a port, so a test can call the service from
/// a thread of its own without borrowing the [`Serve`] that owns the process.
fn post(port: u16, name: &str, body: &str, actor: Option<&str>) -> (u16, String) {
    let mut s = connect(port);
    let actor_line = actor
        .map(|a| format!("X-Actor: {a}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /api/cmd/{name} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n{actor_line}Connection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    split_response(&raw)
}

fn split_response(raw: &str) -> (u16, String) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    // Every route answers with a content length, so the body is the whole
    // tail; a chunked answer would break the JSON parse loudly rather than
    // being silently mis-read.
    (status, body.to_string())
}

fn project() -> tempfile::TempDir {
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
    scryer_core::ensure_planned_at(&r).unwrap();
    dir
}

/// Started against a project, the binary serves the command surface and the
/// event stream on the address it was given: a host can list what it serves,
/// call a command, and hold the stream — all on one port.
#[test]
fn serve_answers_commands_and_lists_what_it_serves() {
    let dir = project();
    let serve = Serve::start(dir.path());
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let (status, body) = serve.get("/api/projects");
    assert_eq!(status, 200, "{body}");
    let listed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(listed["projects"][0]["path"], serde_json::json!(path));
    assert!(!listed["projects"][0]["revision"]
        .as_str()
        .unwrap()
        .is_empty());
    assert_eq!(listed["commands"].as_array().unwrap().len(), 39);

    let (status, body) = serve.post(
        "read_model",
        &serde_json::json!({ "cwd": path }).to_string(),
        None,
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Acme"), "{body}");
}

/// A name the service does not serve is a 404 that lists the ones it does; a
/// command it knows but does not run yet is a 501; a plan write on a stale
/// base is a 409 carrying the current revision.
#[test]
fn serve_refuses_with_the_status_that_matches_the_refusal() {
    let dir = project();
    let serve = Serve::start(dir.path());
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let args = serde_json::json!({ "cwd": path }).to_string();

    let (status, body) = serve.post("read_the_room", &args, None);
    assert_eq!(status, 404, "{body}");
    assert!(
        body.contains("read_model"),
        "the refusal names the real commands: {body}"
    );

    let (status, body) = serve.post("start_model_build", &args, None);
    assert_eq!(status, 501, "{body}");

    let (status, body) = serve.post("read_planned", &args, None);
    assert_eq!(status, 200, "{body}");
    let read: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = read["data"].as_str().unwrap().to_string();

    let (status, _) = serve.post(
        "write_planned",
        &serde_json::json!({
            "cwd": path, "data": data, "baseRevision": "0000000000000000"
        })
        .to_string(),
        None,
    );
    assert_eq!(status, 409, "a stale base revision is a conflict");
}

/// A host holding the stream is told when the project's model files change —
/// the announcement arrives without anybody polling.
#[test]
fn serve_streams_a_model_change_to_a_host_holding_the_stream() {
    let dir = project();
    let serve = Serve::start(dir.path());
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let mut stream = serve.connect();
    write!(
        stream,
        "GET /api/events?project={path} HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n"
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    // Drain the response head so the next read is the stream itself.
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
    }

    // Let the watcher settle, then write the plan the canvas would have.
    std::thread::sleep(Duration::from_millis(300));
    let r = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());
    let mut plan = scryer_core::read_planned_at(&r).unwrap();
    plan.nodes[0].description = Some("changed".into());
    scryer_core::write_planned_at(&r, &plan).unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut saw = false;
    while Instant::now() < deadline && !saw {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line.starts_with("event: model-changed") {
            saw = true;
        }
    }
    assert!(saw, "the model change reached the host's stream");
}

/// While it serves a project, the service takes the SAME model lock the
/// desktop app and the agent's MCP process take — so a write cannot land
/// while another writer holds it, and the two never clobber each other.
///
/// The lock is an advisory OS file lock, so holding it from this test process
/// is exactly what a desktop app or an agent session holding it looks like.
#[test]
fn a_write_waits_for_whoever_else_holds_the_model_lock() {
    let dir = project();
    let serve = Serve::start(dir.path());
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let r = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());

    let (status, body) = serve.post(
        "read_planned",
        &serde_json::json!({ "cwd": path }).to_string(),
        None,
    );
    assert_eq!(status, 200, "{body}");
    let read: serde_json::Value = serde_json::from_str(&body).unwrap();
    let mut plan: scryer_core::ScryModel =
        serde_json::from_str(read["data"].as_str().unwrap()).unwrap();
    plan.nodes[0].description = Some("written under the lock".into());
    let args = serde_json::json!({
        "cwd": path,
        "data": serde_json::to_string(&plan).unwrap(),
        "baseRevision": read["revision"],
    })
    .to_string();

    // Somebody else — the desktop, or an agent's MCP process — is mid-write.
    let held = scryer_core::lock_model(&r).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let port = serve.port;
    let writer = std::thread::spawn(move || {
        let out = post(port, "write_planned", &args, None);
        let _ = tx.send(());
        out
    });

    // The write does not land while the lock is held.
    assert!(
        rx.recv_timeout(Duration::from_millis(700)).is_err(),
        "the write went through while another writer held the lock"
    );
    let still = scryer_core::read_planned_at(&r).unwrap();
    assert_ne!(
        still.nodes[0].description.as_deref(),
        Some("written under the lock")
    );

    // The other writer finishes; ours goes through.
    drop(held);
    let (status, body) = writer.join().unwrap();
    assert_eq!(status, 200, "{body}");
    let landed = scryer_core::read_planned_at(&r).unwrap();
    assert_eq!(
        landed.nodes[0].description.as_deref(),
        Some("written under the lock")
    );
}
