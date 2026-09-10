//! The session-hook endpoint, and the touches it turns into events.
//!
//! Copied from `src-tauri/src/hooks.rs` (upstream 0.4.12). The desktop shell
//! keeps its own copy — this crate is additive and never edits that file — so
//! each rebase diffs the two and carries the delta across.
//!
//! Two things differ from the desktop's version:
//!  - the endpoint publishes on an [`EventSink`] rather than a Tauri window,
//!    so any host attached to the service hears the same signal;
//!  - a touch is RESOLVED before it is published: the claims and nodes it
//!    reaches are looked up against the model and travel with it, so a host
//!    never has to resolve a code location into the model itself. That
//!    resolution is what makes "a session is working here" a model-level
//!    signal instead of a file path.
//!
//! Everything here is opaque to the service. A session id is whatever string
//! the hook sent; the service never asks whose session it is.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::events::{Event, EventSink, Touch};

/// Ceiling on concurrently-handled requests. A `/status` or `/close` does a full
/// model read + anchor scan (seconds on a big repo), so handling requests inline
/// on the accept loop lets one slow call — or a slow/half-open client — stall
/// every other session's hooks behind it. Each accepted connection gets its own
/// worker thread up to this cap; past it we serve inline as backpressure so the
/// thread count can never run away. Loopback, low volume — a small cap is ample.
const MAX_INFLIGHT: usize = 8;

/// Decrements the in-flight counter when a worker finishes (or panics).
struct InflightGuard(Arc<AtomicUsize>);
impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How long a recorded touch stays live. A single agent run is minutes; a
/// resumed session (same id, reconnecting hours later) must not re-gate on the
/// prior run's edits, so touches older than this are pruned on every touch and
/// close. Long enough not to forget a genuinely long session, short enough that
/// a stale run's touches age out.
const TOUCH_TTL: Duration = Duration::from_secs(2 * 3600);

/// Everything the endpoint remembers about live sessions, behind one lock.
#[derive(Default)]
struct SessionLog {
    /// Files each session has edited, with when — see [`TOUCH_TTL`].
    touches: Vec<(Instant, Touch)>,
    /// Sessions already handed a close gate. The gate fires ONCE per session:
    /// its whole point is to make the agent look at claims it touched, and the
    /// verdict may legitimately be "the claim still describes the code" — which
    /// writes nothing, so a re-derived gate would fire again on the next stop,
    /// forever. Claude Code's `stop_hook_active` flag guards its own side of
    /// that, but Copilot sends no such flag, so the promise has to be kept
    /// here, where the session is already known, rather than per harness.
    gated: Vec<(Instant, String)>,
    /// The overlay each session last saw per file, as `(when, session, file,
    /// FNV-1a hash of the payload)`. Agents re-read files constantly — offset
    /// reads, re-reads after an edit, reads to confirm a change — and an
    /// identical block injected every time costs context and teaches the model
    /// to skim the channel. A repeat request whose payload hashes the same is
    /// answered with an empty overlay; the hash is of the payload, not the
    /// rendered text, so the overlay re-fires exactly when its content changed
    /// (a reworded claim, a folded pending entry, a flagged anchor) and the
    /// client stays a dumb renderer. Silence means "same as last time", never
    /// "nothing here".
    overlays: Vec<(Instant, String, String, u64)>,
}

/// Drop touches, close-gate marks and overlay marks older than [`TOUCH_TTL`].
fn prune_touches(log: &mut SessionLog, now: Instant) {
    log.touches
        .retain(|(at, _)| now.saturating_duration_since(*at) < TOUCH_TTL);
    log.gated
        .retain(|(at, _)| now.saturating_duration_since(*at) < TOUCH_TTL);
    log.overlays
        .retain(|(at, ..)| now.saturating_duration_since(*at) < TOUCH_TTL);
}

/// FNV-1a, 64-bit. Not cryptographic — it only has to tell "same payload as
/// last time" from "different", and a collision costs one skipped overlay.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes
        .iter()
        .fold(OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
}

/// The live endpoint for one open project. Dropping it stops the listener
/// thread and removes the discovery file, so hooks fall silent the moment the
/// project closes.
pub struct HookServer {
    pub port: u16,
    project: PathBuf,
    shutdown: Arc<AtomicBool>,
}

impl HookServer {
    pub fn project(&self) -> &Path {
        &self.project
    }
}

impl Drop for HookServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(discovery_path(&self.project));
    }
}

fn discovery_path(project: &Path) -> PathBuf {
    project.join(".scryer").join("hook.json")
}

/// A 128-bit unguessable token from two independently seeded SipHash states.
/// Loopback-only defense: another local user can reach 127.0.0.1, but without
/// the token (readable only from the project's own `.scryer/`) requests bounce.
fn mint_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let word = |seed: u64| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(seed);
        h.finish()
    };
    format!("{:016x}{:016x}", word(1), word(2))
}

/// Start the endpoint for `project`: bind an ephemeral loopback port, write the
/// discovery file, and serve until the returned server is dropped. Every touch
/// it records is resolved against the model and published on `sink`, so a host
/// hears "this session is changing these claims" as it happens.
pub fn start(project: &Path, sink: Arc<dyn EventSink>) -> Result<HookServer, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = mint_token();

    let scryer_dir = project.join(".scryer");
    std::fs::create_dir_all(&scryer_dir).map_err(|e| e.to_string())?;
    let discovery = serde_json::json!({
        "port": port,
        "token": token,
        "pid": std::process::id(),
    });
    std::fs::write(
        discovery_path(project),
        serde_json::to_string_pretty(&discovery).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let touches: Arc<Mutex<SessionLog>> = Arc::new(Mutex::new(SessionLog::default()));
    // Shared across the accept loop and every worker thread it spawns.
    let project = Arc::new(project.to_path_buf());
    let token = Arc::new(token);
    let inflight = Arc::new(AtomicUsize::new(0));

    {
        let shutdown = shutdown.clone();
        let project = project.clone();
        std::thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nodelay(true);
                        // Under the cap, hand the connection to a worker so a slow
                        // request or half-open client can't block the next accept;
                        // at the cap, serve inline as backpressure.
                        if inflight.fetch_add(1, Ordering::SeqCst) < MAX_INFLIGHT {
                            let project = project.clone();
                            let token = token.clone();
                            let touches = touches.clone();
                            let sink = sink.clone();
                            let guard = InflightGuard(inflight.clone());
                            std::thread::spawn(move || {
                                let _guard = guard;
                                handle_request(stream, &project, &token, &touches, sink.as_ref());
                            });
                        } else {
                            inflight.fetch_sub(1, Ordering::SeqCst);
                            handle_request(stream, &project, &token, &touches, sink.as_ref());
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        });
    }

    Ok(HookServer {
        port,
        project: project.to_path_buf(),
        shutdown,
    })
}

/// Look up the claims and nodes a touch reaches and hang them on it.
///
/// The engine already knows how to answer "what intent governs this file?" —
/// that is the `/overlay` route, and `locate_at` behind it. A touch asks the
/// same question, so it gets the same answer: the responsibilities anchored at
/// (file, symbol) and the nodes hosting them, plus the owner chain, so a host
/// can light a component when a symbol under it is edited.
///
/// Best-effort. A project with no model, or a file nothing maps, resolves to
/// nothing — the touch still publishes, carrying what the hook sent. A session
/// working in unmodeled code is a fact worth showing, not an error.
fn resolve(project: &Path, touch: &mut Touch) {
    let r = scryer_core::ModelRef::ProjectLocal(project.to_path_buf());
    let Ok(report) = scryer_core::locate::locate_at(&r, &touch.file, touch.symbol.as_deref())
    else {
        return;
    };

    let mut resp_ids = Vec::new();
    let mut node_ids = Vec::new();
    for claim in &report.result.claims {
        if !resp_ids.contains(&claim.id) {
            resp_ids.push(claim.id.clone());
        }
        if !node_ids.contains(&claim.host_id) {
            node_ids.push(claim.host_id.clone());
        }
    }
    // The owner chain is what makes a container light up when a symbol three
    // levels down is edited — the same rollup the tree draws.
    for owner in &report.result.owner_chain {
        if !node_ids.contains(&owner.id) {
            node_ids.push(owner.id.clone());
        }
    }
    if let Some(b) = &report.result.boundary_owner {
        if !node_ids.contains(&b.id) {
            node_ids.push(b.id.clone());
        }
    }
    touch.resp_ids = resp_ids;
    touch.node_ids = node_ids;
}

/// Serve one request: parse the minimal HTTP exchange, check the token, route.
fn handle_request(
    mut stream: std::net::TcpStream,
    project: &Path,
    token: &str,
    touches: &Arc<Mutex<SessionLog>>,
    sink: &dyn EventSink,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));

    // Read until the header/body split, then the Content-Length'd body.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head, mut body) = loop {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(split) = find_header_end(&buf) {
                    let head = String::from_utf8_lossy(&buf[..split]).to_string();
                    break (head, buf[split + 4..].to_vec());
                }
                if buf.len() > 64 * 1024 {
                    return; // no legitimate hook request is this large
                }
            }
            Err(_) => return,
        }
    };

    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    let mut req_token = String::new();
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "x-scryer-token" => req_token = value.trim().to_string(),
            "content-length" => content_length = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    while body.len() < content_length.min(64 * 1024) {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => return,
        }
    }

    if req_token != token {
        respond(
            &mut stream,
            401,
            &serde_json::json!({ "error": "bad or missing x-scryer-token" }),
        );
        return;
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target.as_str(), ""),
    };
    let param = |key: &str| -> Option<String> {
        query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == key).then(|| percent_decode(v))
        })
    };

    match (method.as_str(), path) {
        ("GET", "/status") => match status_payload(project) {
            Ok(v) => respond(&mut stream, 200, &v),
            Err(e) => respond(&mut stream, 500, &serde_json::json!({ "error": e })),
        },
        ("GET", "/overlay") => {
            let Some(file) = param("file") else {
                respond(
                    &mut stream,
                    400,
                    &serde_json::json!({ "error": "missing ?file=" }),
                );
                return;
            };
            let file = relativize(project, &file);
            let r = scryer_core::ModelRef::ProjectLocal(project.to_path_buf());
            match scryer_core::locate::locate_at(&r, &file, param("symbol").as_deref()) {
                Ok(report) => {
                    let mut v = serde_json::to_value(&report).unwrap_or_default();
                    if let serde_json::Value::Object(map) = &mut v {
                        map.insert("file".into(), serde_json::json!(file));
                    }
                    // With a session named, dedupe: the same payload already
                    // served to this session for this file is answered with an
                    // overlay carrying no claims, directives or pending work,
                    // which the client renders as nothing. No session (Copilot
                    // sends none on some events) → always inject, no bookkeeping.
                    let session = param("session").filter(|s| !s.is_empty());
                    if let Some(session) = session {
                        let hash = fnv1a64(&serde_json::to_vec(&v).unwrap_or_default());
                        let now = Instant::now();
                        let mut log = touches.lock().unwrap();
                        prune_touches(&mut log, now);
                        let seen = log
                            .overlays
                            .iter_mut()
                            .find(|(_, s, f, _)| *s == session && *f == file);
                        match seen {
                            Some((_, _, _, prior)) if *prior == hash => {
                                drop(log);
                                respond(&mut stream, 200, &serde_json::json!({ "file": file }));
                                return;
                            }
                            Some(entry) => *entry = (now, session, file, hash),
                            None => log.overlays.push((now, session, file, hash)),
                        }
                    }
                    respond(&mut stream, 200, &v)
                }
                Err(e) => respond(&mut stream, 500, &serde_json::json!({ "error": e })),
            }
        }
        ("POST", "/touch") => {
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else {
                respond(
                    &mut stream,
                    400,
                    &serde_json::json!({ "error": "body must be JSON" }),
                );
                return;
            };
            let (Some(session), Some(file)) = (v["session"].as_str(), v["file"].as_str()) else {
                respond(
                    &mut stream,
                    400,
                    &serde_json::json!({ "error": "body needs {session, file, symbol?}" }),
                );
                return;
            };
            let mut touch = Touch {
                session: session.to_string(),
                file: relativize(project, file),
                symbol: v["symbol"].as_str().map(str::to_string),
                resp_ids: Vec::new(),
                node_ids: Vec::new(),
            };
            let now = Instant::now();
            let mut log = touches.lock().unwrap();
            prune_touches(&mut log, now);
            let already = log.touches.iter().any(|(_, t)| {
                t.session == touch.session && t.file == touch.file && t.symbol == touch.symbol
            });
            if !already {
                // Resolve BEFORE publishing: the claims and nodes the edit
                // reaches are what a host draws on, and looking them up here
                // means every host gets the same answer from one model read.
                resolve(project, &mut touch);
                sink.publish(Event::hook_touch(project, &touch));
                log.touches.push((now, touch));
            }
            respond(
                &mut stream,
                200,
                &serde_json::json!({ "recorded": log.touches.len() }),
            );
        }
        ("GET", "/close") => {
            let session = param("session").unwrap_or_default();
            let now = Instant::now();
            let mut log = touches.lock().unwrap();
            prune_touches(&mut log, now);
            // Already gated once — say nothing, and skip the model read the
            // answer would need. The session was told what to reconcile; a
            // second gate on the same touches would be a loop, not a reminder.
            if log.gated.iter().any(|(_, s)| *s == session) {
                drop(log);
                respond(&mut stream, 200, &empty_close_payload());
                return;
            }
            let touched: Vec<Touch> = log
                .touches
                .iter()
                .filter(|(_, t)| session.is_empty() || t.session == session)
                .map(|(_, t)| t.clone())
                .collect();
            drop(log);

            let payload = close_payload(project, &touched);
            // Only a gate that actually fires burns the session's one shot: a
            // clean close leaves it armed for the edits still to come.
            if payload["needsReconcile"]
                .as_array()
                .is_some_and(|n| !n.is_empty())
            {
                touches.lock().unwrap().gated.push((now, session.clone()));
                // The gate's items are review work for the developer too: hand
                // them to the app (the inbox shows them live) alongside the
                // session that owes them.
                let mut ev = payload.clone();
                if let serde_json::Value::Object(map) = &mut ev {
                    map.insert("session".into(), serde_json::json!(session));
                }
                sink.publish(Event::hook_close_gate(project, ev.clone()));
            }
            respond(&mut stream, 200, &payload);
        }
        _ => respond(
            &mut stream,
            404,
            &serde_json::json!({
                "error": format!("unknown route {method} {path}"),
                "routes": ["GET /status", "GET /overlay?file=&symbol=&session=", "POST /touch", "GET /close?session="],
            }),
        ),
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Hooks pass whatever path the harness gave them — normalize to the model's
/// project-relative, `/`-separated convention.
fn relativize(project: &Path, file: &str) -> String {
    let file = file.replace('\\', "/");
    let root = project.to_string_lossy().replace('\\', "/");
    let rel = file
        .strip_prefix(root.as_str())
        .map(|r| r.trim_start_matches('/'))
        .unwrap_or(&file);
    rel.trim_start_matches("./").to_string()
}

fn respond(stream: &mut std::net::TcpStream, status: u16, body: &serde_json::Value) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let body = serde_json::to_string_pretty(body).unwrap_or_else(|_| "{}".into());
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    let _ = stream.flush();
}

/// The ambient session status: pending plan entries, drift scopes, and anchor
/// health — as counts plus the one-liner hooks inject verbatim.
fn status_payload(project: &Path) -> Result<serde_json::Value, String> {
    let r = scryer_core::ModelRef::ProjectLocal(project.to_path_buf());
    let model = scryer_core::read_model_at(&r)?;
    let planned = scryer_core::read_planned_at(&r)?;

    // Both altitudes: the element queue (what get_pending hands the agent) and
    // the node/group carriers the canvas draws. One without the other is how
    // this endpoint used to disagree with the agent's own count.
    let pending = scryer_core::diff::pending_element_count(&model, &planned);
    let carriers = scryer_core::diff::plan_carrier_count(&model, &planned);

    // A model never reconciled has no anchor to measure against — reporting
    // "everything drifted" would be the false alarm this endpoint exists to
    // avoid (get_drift/get_health seed the anchor; a status probe must not).
    let drift = if r.sync_path().exists() {
        let sync = scryer_core::read_sync_state(&r);
        scryer_core::drift::drifted_scopes(&model, project, &sync).len()
    } else {
        0
    };

    // Anchor states from the git-free fingerprint check (may silently re-anchor
    // moved symbols, exactly like get_health).
    let (broken, changed) = match scryer_extract::anchors::check_anchors(&r) {
        Ok(check) => {
            use scryer_extract::anchors::AnchorState;
            let broken = check
                .observations
                .iter()
                .filter(|o| matches!(o.state, AnchorState::Broken | AnchorState::FileMissing))
                .count();
            let changed = check
                .observations
                .iter()
                .filter(|o| matches!(o.state, AnchorState::Changed))
                .count();
            (broken, changed)
        }
        Err(_) => (0, 0),
    };

    // Same phrasing as `scryer-mcp statusline` — one sentence for the model's
    // standing state, wherever it is read.
    let plural = if carriers == 1 { "" } else { "s" };
    let work = if pending == 0 {
        "0 pending".to_string()
    } else {
        format!("{pending} pending across {carriers} node{plural}")
    };
    let status_line = format!(
        "scryer: {work} · {drift} drift scope(s) · anchors: {broken} broken, {changed} changed"
    );
    Ok(serde_json::json!({
        "pending": pending,
        "carriers": carriers,
        "driftScopes": drift,
        "anchorsBroken": broken,
        "anchorsChanged": changed,
        "statusLine": status_line,
    }))
}

/// The close-gate view, anchor-informed so it gates only what is genuinely
/// out of sync. Touched files partition three ways:
///
/// - `needsReconcile` — files carrying a claim the check can't vouch for:
///   either a committed anchor whose fingerprint reports changed / broken /
///   missing, OR a plan-added / glob-pattern anchor that has no baseline to
///   fingerprint at all. Both mean "the session touched modeled code that isn't
///   verified clean" — the cases worth blocking on. Plan-layer and glob anchors
///   are exactly the blindness `8ad39fc` closed in completeness.
/// - `cleanModeled` — files whose only claims are committed anchors that hash
///   clean: the session edited around the modeled behaviour and owes nothing.
/// - `unmodeled` — files the model doesn't map at all.
///
/// The fingerprint check compares against the last reconcile baseline, so a
/// long-unreconciled file may surface pre-session changes too — still the
/// right call: the claim needs a look and this session just worked there.
/// The close view for a session that has nothing to answer for — same shape as
/// [`close_payload`], reached without a model read.
fn empty_close_payload() -> serde_json::Value {
    serde_json::json!({
        "needsReconcile": [],
        "cleanModeled": [],
        "unmodeled": [],
    })
}

fn close_payload(project: &Path, touched: &[Touch]) -> serde_json::Value {
    let r = scryer_core::ModelRef::ProjectLocal(project.to_path_buf());

    // Out-of-sync anchors across the project (may silently re-anchor moved
    // symbols, exactly like get_health). No baseline yet → no observations →
    // the gate stays silent rather than crying wolf on a fresh model.
    let observations = scryer_extract::anchors::check_anchors(&r)
        .map(|c| c.observations)
        .unwrap_or_default();

    // The working view names hosts and statements for observation keys, and —
    // crucially — carries the PLAN-layer source map, the anchors the committed-
    // only fingerprint check above can't see. `committed` is kept separately to
    // tell fingerprint-checkable anchors from the rest. Both built once.
    let committed = scryer_core::read_model_at(&r).ok();
    let working = match (&committed, scryer_core::read_planned_at(&r)) {
        (Some(c), Ok(p)) => Some(scryer_core::working_view(c, &p)),
        _ => committed.clone(),
    };
    let statement_of = |key: &str| -> Option<String> {
        // A test-anchor observation names the claim its test backs.
        let key = scryer_core::test_resp_id(key).unwrap_or(key);
        let w = working.as_ref()?;
        w.nodes
            .iter()
            .flat_map(|n| n.responsibilities.iter())
            .chain(w.groups.iter().flat_map(|g| g.responsibilities.iter()))
            .find(|resp| resp.id == key)
            .map(|resp| resp.statement.clone())
    };
    let host_name_of = |key: &str| -> Option<String> {
        let w = working.as_ref()?;
        for n in &w.nodes {
            if n.id == key || n.responsibilities.iter().any(|resp| resp.id == key) {
                return Some(n.name.clone());
            }
        }
        w.groups
            .iter()
            .find(|g| g.responsibilities.iter().any(|resp| resp.id == key))
            .map(|g| g.name.clone())
    };
    let loc_matches = |pattern: &str, file: &str| -> bool {
        pattern == file || glob::Pattern::new(pattern).is_ok_and(|p| p.matches(file))
    };
    // The fingerprint baseline covers only committed sourceMap keys with an EXACT
    // location for the file. Anything else on the file — a plan-added anchor (no
    // baseline yet) or a glob-pattern location (never fingerprinted, since the
    // baseline reads the pattern as a literal path) — can't be verified, so a
    // touch surfaces it for a look rather than passing it as clean.
    let committed_exact = |key: &str, file: &str| -> bool {
        committed.as_ref().is_some_and(|c| {
            c.source_map
                .get(key)
                .is_some_and(|locs| locs.iter().any(|l| l.pattern == file))
        })
    };

    let mut files: Vec<&str> = Vec::new();
    for t in touched {
        if !files.contains(&t.file.as_str()) {
            files.push(&t.file);
        }
    }

    let mut needs: Vec<serde_json::Value> = Vec::new();
    let mut clean_modeled: Vec<&str> = Vec::new();
    let mut unmodeled: Vec<&str> = Vec::new();
    for file in files {
        // 1) Committed anchors the fingerprint check flagged changed/broken/missing.
        let mut dirty: Vec<serde_json::Value> = observations
            .iter()
            .filter(|o| o.file == file)
            .map(|o| {
                let mut v = serde_json::json!({
                    "id": o.key,
                    "host": o.host_name,
                    "symbol": o.symbol,
                    "state": o.state,
                    "statement": statement_of(&o.key),
                });
                if let serde_json::Value::Object(map) = &mut v {
                    map.retain(|_, val| !val.is_null());
                }
                v
            })
            .collect();

        // 2) Plan-added and glob anchors on this file — unverifiable, so a touch
        //    surfaces them (mirrors how completeness resolves plan-layer anchors).
        if let Some(w) = working.as_ref() {
            let mut keys: Vec<&String> = w.source_map.keys().collect();
            keys.sort();
            for key in keys {
                if committed_exact(key, file) {
                    continue; // fingerprint-checkable — handled in (1) or genuinely clean
                }
                if let Some(loc) = w.source_map[key]
                    .iter()
                    .find(|l| loc_matches(&l.pattern, file))
                {
                    let mut v = serde_json::json!({
                        "id": key,
                        "host": host_name_of(key),
                        "symbol": loc.symbol,
                        "state": "unreconciled",
                        "statement": statement_of(key),
                    });
                    if let serde_json::Value::Object(map) = &mut v {
                        map.retain(|_, val| !val.is_null());
                    }
                    dirty.push(v);
                }
            }
        }

        if !dirty.is_empty() {
            needs.push(serde_json::json!({ "file": file, "claims": dirty }));
        } else {
            let modeled = scryer_core::locate::locate_at(&r, file, None)
                .map(|rep| !rep.result.claims.is_empty())
                .unwrap_or(false);
            if modeled {
                clean_modeled.push(file);
            } else {
                unmodeled.push(file);
            }
        }
    }

    serde_json::json!({
        "touched": touched,
        "needsReconcile": needs,
        "cleanModeled": clean_modeled,
        "unmodeled": unmodeled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use std::sync::mpsc;

    /// Records what the endpoint published, so a test can read the events a
    /// host would have received.
    struct Recorder(mpsc::Sender<Event>);

    impl EventSink for Recorder {
        fn publish(&self, event: Event) {
            let _ = self.0.send(event);
        }
    }

    /// A project whose claim `r-1` is anchored at `src/auth.rs` / `verify`,
    /// under `sys / API`. Mirrors the fixture in `src-tauri/src/hooks.rs`.
    fn temp_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let r = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());
        let mut m = scryer_core::ScryModel::new();
        m.nodes.push(
            serde_json::from_value(serde_json::json!({
                "id": "sys", "kind": "system", "name": "Acme"
            }))
            .unwrap(),
        );
        let mut api: scryer_core::Node = serde_json::from_value(serde_json::json!({
            "id": "api", "kind": "container", "name": "API", "parentId": "sys"
        }))
        .unwrap();
        api.responsibilities = vec![serde_json::from_value(
            serde_json::json!({ "id": "r-1", "statement": "serves requests" }),
        )
        .unwrap()];
        m.nodes.push(api);
        m.source_map.insert(
            "r-1".into(),
            vec![serde_json::from_value(
                serde_json::json!({ "pattern": "src/auth.rs", "symbol": "verify" }),
            )
            .unwrap()],
        );
        scryer_core::write_model_at(&r, &m).unwrap();
        dir
    }

    fn token_for(project: &Path) -> String {
        let disc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(project.join(".scryer/hook.json")).unwrap(),
        )
        .unwrap();
        disc["token"].as_str().unwrap().to_string()
    }

    fn request(
        port: u16,
        token: &str,
        method: &str,
        target: &str,
        body: &str,
    ) -> (u16, serde_json::Value) {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "{method} {target} HTTP/1.1\r\nHost: localhost\r\nx-scryer-token: {token}\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let status: u16 = out
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let start = out.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        (
            status,
            serde_json::from_str(&out[start..]).unwrap_or(serde_json::Value::Null),
        )
    }

    /// A recorded touch is published on the event stream RESOLVED: it carries
    /// the session that made it and the claims and nodes it reaches, so a host
    /// can light the model where a session is working without resolving code
    /// locations itself.
    #[test]
    fn a_recorded_touch_is_published_carrying_the_claims_and_nodes_it_reaches() {
        let dir = temp_project();
        let (tx, rx) = mpsc::channel();
        let server = start(dir.path(), Arc::new(Recorder(tx))).unwrap();
        let token = token_for(dir.path());

        let (status, _) = request(
            server.port,
            &token,
            "POST",
            "/touch",
            r#"{"session":"sess-7","file":"src/auth.rs","symbol":"verify"}"#,
        );
        assert_eq!(status, 200);

        let event = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the touch was published");
        assert_eq!(event.name, "hook-touch");
        assert_eq!(event.project, dir.path().to_string_lossy());

        let touch: Touch = serde_json::from_value(event.payload).unwrap();
        assert_eq!(touch.session, "sess-7");
        assert_eq!(touch.file, "src/auth.rs");
        assert_eq!(touch.symbol.as_deref(), Some("verify"));
        assert_eq!(
            touch.resp_ids,
            vec!["r-1".to_string()],
            "the claim it reaches"
        );
        assert!(
            touch.node_ids.contains(&"api".to_string()),
            "the claim's host: {:?}",
            touch.node_ids
        );
        assert!(
            touch.node_ids.contains(&"sys".to_string()),
            "and the chain above it, so a container lights up too: {:?}",
            touch.node_ids
        );
    }

    /// A session editing code nothing models still publishes its touch — it
    /// just resolves to nothing. "Working somewhere unmodeled" is a fact worth
    /// showing, not a reason to drop the signal.
    #[test]
    fn a_touch_in_unmodeled_code_publishes_with_nothing_resolved() {
        let dir = temp_project();
        let (tx, rx) = mpsc::channel();
        let server = start(dir.path(), Arc::new(Recorder(tx))).unwrap();
        let token = token_for(dir.path());

        request(
            server.port,
            &token,
            "POST",
            "/touch",
            r#"{"session":"sess-7","file":"scripts/build.sh"}"#,
        );

        let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let touch: Touch = serde_json::from_value(event.payload).unwrap();
        assert_eq!(touch.file, "scripts/build.sh");
        assert!(touch.resp_ids.is_empty());
        assert!(touch.node_ids.is_empty());
    }

    /// The token gate survives the copy: a request without the project's token
    /// is refused, and nothing is published for it.
    #[test]
    fn a_request_without_the_projects_token_is_refused_and_publishes_nothing() {
        let dir = temp_project();
        let (tx, rx) = mpsc::channel();
        let server = start(dir.path(), Arc::new(Recorder(tx))).unwrap();

        let (status, _) = request(
            server.port,
            "not-the-token",
            "POST",
            "/touch",
            r#"{"session":"s","file":"src/auth.rs"}"#,
        );
        assert_eq!(status, 401);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    /// Dropping the server removes the discovery file, so the project's hooks
    /// fall silent — unregistering a project is the opt-out.
    #[test]
    fn dropping_the_endpoint_removes_its_discovery_file() {
        let dir = temp_project();
        let (tx, _rx) = mpsc::channel();
        let server = start(dir.path(), Arc::new(Recorder(tx))).unwrap();
        assert!(dir.path().join(".scryer/hook.json").exists());
        drop(server);
        assert!(!dir.path().join(".scryer/hook.json").exists());
    }
}
