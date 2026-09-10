//! The service's event stream: the same events the desktop shell emits to its
//! webview, published to whatever host is attached instead.
//!
//! The desktop calls `app.emit(name, payload)` (`src-tauri/src/project.rs`,
//! `build.rs`, `preview.rs`). A service has many hosts, so the emit becomes a
//! trait — [`EventSink`] — with a broadcast implementation behind it. Every
//! event names the PROJECT it belongs to, which the desktop never had to,
//! because a window only ever showed one.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

/// One event, named exactly as the frontend's `listen(...)` expects it.
///
/// The four the desktop emits for the model — `model-changed`,
/// `test-results-changed`, `agent-event`, `build-active-node` — plus
/// `hook-touch` and `hook-close-gate`, which the desktop emits from its
/// session-hook endpoint. A host demultiplexes on `name`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// The event name the frontend listens on.
    pub name: String,
    /// Absolute path of the project this event is about. A host filters on it;
    /// a client that names no project sees nothing (see the SSE route).
    pub project: String,
    /// The payload the desktop would have passed to `emit`.
    pub payload: serde_json::Value,
}

impl Event {
    pub fn new(name: &str, project: &Path, payload: serde_json::Value) -> Self {
        Self {
            name: name.to_string(),
            project: project.to_string_lossy().to_string(),
            payload,
        }
    }

    /// The model files on disk changed. Payload is the model ref string, as
    /// the desktop's watcher emits it.
    pub fn model_changed(project: &Path) -> Self {
        let r = scryer_core::ModelRef::ProjectLocal(project.to_path_buf());
        Self::new(
            "model-changed",
            project,
            serde_json::json!(r.to_ref_string()),
        )
    }

    /// The test-status cache was rewritten — a report landed mid-session.
    pub fn test_results_changed(project: &Path) -> Self {
        let r = scryer_core::ModelRef::ProjectLocal(project.to_path_buf());
        Self::new(
            "test-results-changed",
            project,
            serde_json::json!(r.to_ref_string()),
        )
    }

    /// One event from a running agent session.
    pub fn agent_event(project: &Path, payload: serde_json::Value) -> Self {
        Self::new("agent-event", project, payload)
    }

    /// Which node ids a running build is working on right now.
    pub fn build_active_node(project: &Path, node_ids: Vec<String>) -> Self {
        Self::new("build-active-node", project, serde_json::json!(node_ids))
    }

    /// A session touched code. Carries the touch AS RECEIVED plus the claims
    /// and nodes it reaches, resolved by the engine — the "a session is
    /// working here" signal a host draws on.
    pub fn hook_touch(project: &Path, touch: &Touch) -> Self {
        Self::new(
            "hook-touch",
            project,
            serde_json::to_value(touch).unwrap_or_default(),
        )
    }

    /// A session's close gate fired with items needing reconcile.
    pub fn hook_close_gate(project: &Path, payload: serde_json::Value) -> Self {
        Self::new("hook-close-gate", project, payload)
    }
}

/// One session's edit, resolved to the model.
///
/// The session-hook endpoint receives the first three fields (see
/// `src-tauri/src/hooks.rs`, `Touch`); the service adds the last two, so a
/// host never has to resolve code locations into the model itself. Everything
/// here is opaque to the service: `session` is whatever string the hook sent.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Touch {
    /// The agent session that made the edit, as the hook reported it.
    pub session: String,
    /// Project-relative file the session touched.
    pub file: String,
    /// The definition within the file, when the hook could name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// Responsibility ids the touch reaches, resolved against the model.
    #[serde(default)]
    pub resp_ids: Vec<String>,
    /// Node ids the touch reaches, resolved against the model.
    #[serde(default)]
    pub node_ids: Vec<String>,
}

/// Where the service publishes its events. A host mounting the service
/// in-process implements this to forward into its own stream; the serve binary
/// uses [`Broadcast`], which fans out to every attached SSE client.
pub trait EventSink: Send + Sync {
    fn publish(&self, event: Event);
}

/// Publishes nothing. For a host that only posts commands, and for tests.
pub struct NullSink;

impl EventSink for NullSink {
    fn publish(&self, _event: Event) {}
}

/// Fans every event out to every attached subscriber. A subscriber that falls
/// too far behind loses the oldest events rather than blocking the writer —
/// the model on disk, not the stream, is the source of truth, and a host that
/// missed a `model-changed` re-reads on the next one.
pub struct Broadcast {
    tx: tokio::sync::broadcast::Sender<Event>,
}

impl Broadcast {
    pub fn new(capacity: usize) -> Arc<Self> {
        let (tx, _rx) = tokio::sync::broadcast::channel(capacity);
        Arc::new(Self { tx })
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl EventSink for Broadcast {
    fn publish(&self, event: Event) {
        // No subscribers is the normal case for a desktop-less service; a
        // send error means exactly that and is not a failure.
        let _ = self.tx.send(event);
    }
}
