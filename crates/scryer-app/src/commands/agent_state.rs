//! Per-project state for the things that run PROCESSES: an agent runtime with
//! its cancel flag, and the preview sidecar.
//!
//! The desktop keeps one of each in Tauri-managed state (`src-tauri/src/
//! state.rs`), which is right for a window that has one project open. A
//! service serves many at once, so each project gets its own — cancelling a
//! build in one project must not stop an agent in another, and two projects'
//! preview servers are two `node` processes, not one fought over.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// The agent runtime for one project, and the durable cancel flag beside it.
///
/// The flag is set DIRECTLY by `cancel_agent_session`, not only through the
/// runtime: orchestrators reset it at start and check it at every wave
/// boundary, so a stop pressed in a no-session gap — or just before a queued
/// parallel session starts — is still honoured. Without it, cancellation is
/// edge-triggered on live sessions and gets silently lost in those gaps.
/// (Mirrors `src-tauri/src/state.rs::AcpState`.)
#[derive(Default)]
pub struct AgentState {
    runtime: Mutex<Option<scryer_acp::AcpRuntime>>,
    cancelled: Arc<AtomicBool>,
}

impl AgentState {
    /// The project's runtime, started on first use — an agent runtime owns a
    /// thread, so a project nobody runs an agent in never pays for one.
    pub fn runtime(&self) -> scryer_acp::AcpRuntime {
        let mut rt = self.runtime.lock().unwrap();
        if rt.is_none() {
            *rt = Some(scryer_acp::AcpRuntime::new());
        }
        rt.clone().unwrap()
    }

    /// The runtime only if one has already started — for a cancel, which must
    /// not bring a runtime up just to stop it.
    pub fn started(&self) -> Option<scryer_acp::AcpRuntime> {
        self.runtime.lock().unwrap().clone()
    }

    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    /// Raise the flag. Every orchestrator checks it at each boundary.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Lower it, as a run starts. A previous run's stop must not kill the next.
    pub fn arm(&self) {
        self.cancelled
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The project's preview sidecar: one shared Vite dev server, whose stdin this
/// process holds open — the sidecar exits when the pipe closes, so it can never
/// outlive us. (Mirrors `src-tauri/src/state.rs::PreviewState`.)
#[derive(Default)]
pub struct PreviewState(pub tokio::sync::Mutex<Option<PreviewServer>>);

pub struct PreviewServer {
    pub cwd: String,
    pub url: String,
    pub child: tokio::process::Child,
}
