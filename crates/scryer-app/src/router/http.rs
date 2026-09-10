//! The HTTP + SSE router a host mounts, or the serve binary runs.
//!
//! Three routes and nothing else:
//!  - `POST /api/cmd/{name}` — the JSON body is the command's arguments, the
//!    response its result. Refusals carry a `kind` a client can branch on and
//!    the status that matches it (409 for a stale plan write, 404 for a name
//!    that does not exist, 501 for one not served yet).
//!  - `GET  /api/events?project=…` — server-sent events, one `event:` per
//!    event name, filtered to the projects the query names. A client that
//!    names none gets an open stream and nothing on it, never everything.
//!  - `GET  /api/projects` — what this service serves.
//!
//! There is no authentication here and there will not be. The service leaves
//! identity, authorisation and audience to whatever host mounts it (the
//! directive on `Engine Service`). The one thing it does do is refuse to take
//! a caller's WORD for who they are unless the caller is on this machine: the
//! `X-Actor` header is honoured for a loopback peer and ignored for anyone
//! else, so a service accidentally exposed to a network cannot be used to
//! write history under someone else's name.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::Stream;

use crate::commands::{dispatch, COMMANDS};
use crate::events::{Broadcast, Event};
use crate::state::{revision, AppState};

/// What the router needs: the registry to run commands against and the
/// broadcast to read the event stream from.
#[derive(Clone)]
pub struct ServiceState {
    pub app: AppState,
    pub events: Arc<Broadcast>,
}

/// The service's routes, ready to mount under any prefix a host likes.
pub fn router(app: AppState, events: Arc<Broadcast>) -> Router {
    Router::new()
        .route("/api/cmd/{name}", post(post_command))
        .route("/api/events", get(get_events))
        .route("/api/projects", get(get_projects))
        .with_state(ServiceState { app, events })
}

/// The identity a caller claims, read from `X-Actor` — and only believed when
/// the caller is on this machine.
///
/// A remote peer's `X-Actor` is DROPPED rather than trusted: over a network
/// the header is an unverified assertion, and a write recorded under a name
/// nobody checked is worse than one recorded under no name at all. A host that
/// has verified an identity of its own mounts the router in-process and passes
/// the actor directly, never through this header.
pub struct ActorHeader(pub Option<String>);

impl ActorHeader {
    pub fn read(headers: &HeaderMap, peer: Option<IpAddr>) -> Self {
        let claimed = headers
            .get("x-actor")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|a| !a.is_empty());
        match (claimed, peer) {
            (Some(a), Some(ip)) if ip.is_loopback() => Self(Some(a.to_string())),
            _ => Self(None),
        }
    }
}

async fn post_command(
    State(state): State<ServiceState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // An empty body is a command with no arguments, not a malformed call.
    let args: serde_json::Value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "message": format!("body is not JSON: {e}") })),
                )
                    .into_response()
            }
        }
    };
    let peer = extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(a)| a.ip());
    let ActorHeader(actor) = ActorHeader::read(&headers, peer);

    // Health and drift passes parse the whole repo — seconds on a big project.
    // Off the async runtime's worker, like the desktop's spawn_blocking.
    let result =
        tokio::task::spawn_blocking(move || dispatch(&state.app, &name, &args, actor.as_deref()))
            .await;

    match result {
        Ok(Ok(value)) => (StatusCode::OK, Json(value)).into_response(),
        Ok(Err(e)) => (
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(serde_json::json!({ "error": e, "message": e.to_string() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "message": format!("command task failed: {e}") })),
        )
            .into_response(),
    }
}

/// Which projects a client wants events for, from a repeatable `project`
/// query parameter (`?project=/a&project=/b`). Parsed by hand because a
/// repeated key is a LIST, and the usual query deserializer reads the first
/// one and calls the rest a type error.
fn wanted_projects(query: Option<&str>) -> Vec<String> {
    let Some(query) = query else {
        return Vec::new();
    };
    form_urlencoded::parse(query.as_bytes())
        .filter(|(k, _)| k == "project")
        .map(|(_, v)| v.to_string())
        .collect()
}

async fn get_events(
    State(state): State<ServiceState>,
    RawQuery(query): RawQuery,
) -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    // Canonicalize what the client asked for so `/proj` and `/proj/` and a
    // symlinked path all name the same project the watcher publishes under.
    let wanted: Vec<String> = wanted_projects(query.as_deref())
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .canonicalize()
                .unwrap_or_else(|_| std::path::PathBuf::from(p))
                .to_string_lossy()
                .to_string()
        })
        .collect();

    let rx = state.events.subscribe();
    let stream = async_stream::stream(rx, wanted);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The broadcast, filtered to the projects the client named and rendered as
/// SSE. A lagged client silently resumes rather than dropping the stream: the
/// model on disk is the truth, and the next `model-changed` puts it right.
mod async_stream {
    use super::*;
    use futures::stream::StreamExt;
    use tokio::sync::broadcast::error::RecvError;

    pub fn stream(
        rx: tokio::sync::broadcast::Receiver<Event>,
        wanted: Vec<String>,
    ) -> impl Stream<Item = Result<SseEvent, std::convert::Infallible>> {
        tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |item| {
            let wanted = wanted.clone();
            async move {
                let event = match item {
                    Ok(e) => e,
                    Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                        return None
                    }
                };
                if !wanted.iter().any(|p| p == &event.project) {
                    return None;
                }
                let data = serde_json::to_string(&event).ok()?;
                Some(Ok(SseEvent::default().event(&event.name).data(data)))
            }
        })
    }

    // Named so the unused import above reads as deliberate.
    #[allow(dead_code)]
    fn _recv_error(_: RecvError) {}
}

/// One project the service serves, and the revision its plan is at.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectInfo {
    path: String,
    /// The model ref string upstream's frontend passes back on every call.
    model_ref: String,
    /// The plan's current revision — a client's base for its first write.
    revision: String,
}

async fn get_projects(State(state): State<ServiceState>) -> impl IntoResponse {
    let projects: Vec<ProjectInfo> = state
        .app
        .project_paths()
        .into_iter()
        .map(|path| {
            let r = scryer_core::ModelRef::ProjectLocal(path.clone());
            ProjectInfo {
                path: path.to_string_lossy().to_string(),
                model_ref: r.to_ref_string(),
                revision: revision(&r),
            }
        })
        .collect();
    Json(serde_json::json!({ "projects": projects, "commands": COMMANDS }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSink;
    use futures::StreamExt;
    use std::net::Ipv4Addr;

    fn headers(actor: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-actor", actor.parse().unwrap());
        h
    }

    /// The service takes a caller's word for who they are only when the caller
    /// is on this machine. A remote peer's `X-Actor` is dropped: over a
    /// network it is an unverified assertion, and a history event written
    /// under a name nobody checked is worse than one written under no name.
    #[test]
    fn an_actor_header_is_believed_from_loopback_and_dropped_from_anywhere_else() {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let remote = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7));

        assert_eq!(
            ActorHeader::read(&headers("jesseh"), Some(loopback))
                .0
                .as_deref(),
            Some("jesseh")
        );
        assert_eq!(ActorHeader::read(&headers("jesseh"), Some(remote)).0, None);
        // No peer to check is no ground to believe it either.
        assert_eq!(ActorHeader::read(&headers("jesseh"), None).0, None);
        // A blank claim names nobody.
        assert_eq!(ActorHeader::read(&headers("   "), Some(loopback)).0, None);
        assert_eq!(ActorHeader::read(&HeaderMap::new(), Some(loopback)).0, None);
    }

    /// A client holding the stream gets every event for the projects it named
    /// — the model, test, agent and build events alike — and nothing for any
    /// other project on the same service.
    #[tokio::test]
    async fn the_stream_forwards_every_event_for_the_projects_a_client_names() {
        let events = Broadcast::new(64);
        let mine = std::path::PathBuf::from("/tmp/mine");
        let theirs = std::path::PathBuf::from("/tmp/theirs");

        let mut stream = Box::pin(async_stream::stream(
            events.subscribe(),
            vec![mine.to_string_lossy().to_string()],
        ));

        events.publish(Event::model_changed(&mine));
        events.publish(Event::model_changed(&theirs));
        events.publish(Event::test_results_changed(&mine));
        events.publish(Event::build_active_node(&mine, vec!["node-1".into()]));
        events.publish(Event::agent_event(&theirs, serde_json::json!({ "t": "x" })));
        events.publish(Event::agent_event(&mine, serde_json::json!({ "t": "x" })));

        let mut seen = Vec::new();
        for _ in 0..4 {
            let item = stream.next().await.unwrap().unwrap();
            let rendered = format!("{item:?}");
            seen.push(rendered);
        }
        assert_eq!(seen.len(), 4, "four events for my project, in order");
        assert!(seen[0].contains("model-changed"));
        assert!(seen[1].contains("test-results-changed"));
        assert!(seen[2].contains("build-active-node"));
        assert!(seen[3].contains("agent-event"));
        assert!(
            seen.iter().all(|s| !s.contains("/tmp/theirs")),
            "another project's events never reach this client: {seen:?}"
        );
    }

    /// A repeated `project` parameter is a list, and no parameter is an empty
    /// one — the two cases the stream's filter turns on.
    #[test]
    fn the_project_filter_reads_a_repeated_query_parameter_as_a_list() {
        assert_eq!(wanted_projects(None), Vec::<String>::new());
        assert_eq!(wanted_projects(Some("")), Vec::<String>::new());
        assert_eq!(wanted_projects(Some("project=/a")), vec!["/a".to_string()]);
        assert_eq!(
            wanted_projects(Some("project=/a&other=x&project=%2Ftmp%2Fb")),
            vec!["/a".to_string(), "/tmp/b".to_string()]
        );
    }

    /// A client that names NO project gets an open stream and nothing on it.
    /// Streaming everything to a caller who asked for nothing is how a service
    /// leaks one host's projects to another; the quiet answer is the safe one.
    #[tokio::test]
    async fn a_client_naming_no_project_is_streamed_nothing() {
        let events = Broadcast::new(64);
        let project = std::path::PathBuf::from("/tmp/anything");
        let mut stream = Box::pin(async_stream::stream(events.subscribe(), Vec::new()));

        events.publish(Event::model_changed(&project));
        events.publish(Event::test_results_changed(&project));

        let next = tokio::time::timeout(std::time::Duration::from_millis(200), stream.next()).await;
        assert!(next.is_err(), "the stream stays open and silent");
    }
}
