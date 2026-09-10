//! The engine service's own state: which projects it serves, what each one's
//! plan currently hashes to, and a watcher per project that turns file changes
//! into events on the sink.
//!
//! The desktop app watches exactly one project at a time (`src-tauri/src/
//! project.rs`, `watch_project`) because a window shows one. A service has
//! many hosts and many projects at once, so the registry is a map rather than
//! a slot, and every event it publishes names the project it belongs to.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notify::{recommended_watcher, EventKind, RecursiveMode, Watcher};

use crate::events::{Event, EventSink};

/// One project the service serves.
pub struct Project {
    pub model_ref: scryer_core::ModelRef,
    /// Kept alive for as long as the project is registered; dropping it stops
    /// the watch.
    _watcher: Option<Box<dyn Watcher + Send + Sync>>,
    /// The project's session-hook endpoint. Dropping it removes the discovery
    /// file, so the project's hooks fall silent the moment it is unregistered
    /// — the same opt-in/opt-out the desktop app has.
    hooks: Option<crate::hooks::HookServer>,
    /// The project's agent runtime and cancel flag. Per project, so a stop in
    /// one never reaches another's session.
    agents: crate::commands::agent_state::AgentState,
    /// The project's preview sidecar — one `node` process per project.
    preview: crate::commands::agent_state::PreviewState,
}

impl Project {
    /// The loopback port the session-hook endpoint is listening on, if it came
    /// up. A host needs it for nothing — hooks discover it through the
    /// project's own `.scryer/hook.json` — but a test does.
    pub fn hook_port(&self) -> Option<u16> {
        self.hooks.as_ref().map(|h| h.port)
    }

    pub fn agents(&self) -> &crate::commands::agent_state::AgentState {
        &self.agents
    }

    pub fn preview(&self) -> &crate::commands::agent_state::PreviewState {
        &self.preview
    }
}

impl Project {
    pub fn path(&self) -> &Path {
        self.model_ref.project_path()
    }

    pub fn ref_string(&self) -> String {
        self.model_ref.to_ref_string()
    }
}

/// The service's registry: the projects it serves and the sink its watchers
/// publish on. Cloneable — every clone shares one registry.
#[derive(Clone)]
pub struct AppState {
    projects: Arc<Mutex<BTreeMap<PathBuf, Arc<Project>>>>,
    sink: Arc<dyn EventSink>,
}

impl AppState {
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            projects: Arc::new(Mutex::new(BTreeMap::new())),
            sink,
        }
    }

    pub fn sink(&self) -> &Arc<dyn EventSink> {
        &self.sink
    }

    /// Register a project and start watching its `.scryer/` directory, if it
    /// is not registered already. Idempotent: re-registering the same path
    /// leaves the running watcher alone.
    pub fn add_project(&self, project_path: &Path) -> Result<Arc<Project>, String> {
        let canonical = project_path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", project_path.display()))?;
        {
            let projects = self.projects.lock().unwrap();
            if let Some(p) = projects.get(&canonical) {
                return Ok(p.clone());
            }
        }
        let model_ref = scryer_core::ModelRef::ProjectLocal(canonical.clone());
        let watcher = self.watch(&model_ref);
        // Registering a project brings its session-hook endpoint up, the way
        // opening one does in the desktop app. A failure here is not fatal:
        // the model surface works without hooks, and the reason is worth
        // saying out loud rather than refusing the project.
        let hooks = match crate::hooks::start(&canonical, self.sink.clone()) {
            Ok(server) => {
                eprintln!(
                    "[hooks] {} on 127.0.0.1:{}",
                    canonical.display(),
                    server.port
                );
                Some(server)
            }
            Err(e) => {
                eprintln!("[hooks] {}: endpoint not started: {e}", canonical.display());
                None
            }
        };
        let project = Arc::new(Project {
            model_ref,
            _watcher: watcher,
            hooks,
            agents: Default::default(),
            preview: Default::default(),
        });
        self.projects
            .lock()
            .unwrap()
            .insert(canonical, project.clone());
        Ok(project)
    }

    /// The project registered at this path, if any. The path need not be
    /// canonical — a host may name a project however its config spells it.
    pub fn project(&self, project_path: &Path) -> Option<Arc<Project>> {
        let canonical = project_path.canonicalize().ok()?;
        self.projects.lock().unwrap().get(&canonical).cloned()
    }

    /// Every registered project path, in registration-independent order.
    pub fn project_paths(&self) -> Vec<PathBuf> {
        self.projects.lock().unwrap().keys().cloned().collect()
    }

    /// Resolve a project path to its `ModelRef`, registering the project if it
    /// is not known yet. A host that only ever posts commands never has to
    /// call `watch_project` first — but until it does, nothing is watched, so
    /// no events flow for it.
    pub fn model_ref(&self, project_path: &str) -> Result<scryer_core::ModelRef, String> {
        let path = Path::new(project_path);
        if let Some(p) = self.project(path) {
            return Ok(p.model_ref.clone());
        }
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(scryer_core::ModelRef::ProjectLocal(canonical))
    }

    /// Watch `{project}/.scryer/` and publish what changes. Copied in shape
    /// from `src-tauri/src/project.rs` (`watch_project`) — same filters, same
    /// event names — minus the desktop's report-directory ingestion, which is
    /// the shell's job, not the service's.
    fn watch(&self, model_ref: &scryer_core::ModelRef) -> Option<Box<dyn Watcher + Send + Sync>> {
        let dir = model_ref.dir();
        let _ = std::fs::create_dir_all(&dir);
        let sink = self.sink.clone();
        let project = model_ref.project_path().to_path_buf();

        let mut watcher = recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                return;
            }
            for path in &event.paths {
                if path.file_name().is_some_and(|n| n == ".test-results.json") {
                    sink.publish(Event::test_results_changed(&project));
                    continue;
                }
                if path.extension().is_none_or(|e| e != "scry") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if stem.ends_with(".baseline") || stem.starts_with(".tmp") {
                    continue;
                }
                sink.publish(Event::model_changed(&project));
            }
        })
        .ok()?;
        watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
        Some(Box::new(watcher))
    }
}

/// A project's current plan REVISION: the hash of `planned.scry` exactly as it
/// sits on disk. A client reads the plan, edits it, and names the revision it
/// started from when it writes back; a revision that no longer matches means
/// somebody else wrote in between (see `commands::project::write_planned`).
///
/// Hashing the bytes rather than tracking a counter means the revision is
/// derivable by anyone holding the file, survives restarts of the service, and
/// needs no state of its own. A project with no plan yet has the revision of
/// empty bytes, so "no plan" is a base revision like any other.
pub fn revision(model_ref: &scryer_core::ModelRef) -> String {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(model_ref.planned_path()).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    format!("{:x}", hasher.finalize())[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use std::sync::mpsc;

    /// A sink that records what it was handed, so a test can watch the
    /// announcements a host would receive.
    struct Recorder(mpsc::Sender<Event>);

    impl EventSink for Recorder {
        fn publish(&self, event: Event) {
            let _ = self.0.send(event);
        }
    }

    fn project_with(model: &scryer_core::ModelRef) {
        let mut m = scryer_core::ScryModel::new();
        m.nodes.push(
            serde_json::from_value(
                serde_json::json!({ "id": "node-1", "kind": "system", "name": "Acme" }),
            )
            .unwrap(),
        );
        scryer_core::write_model_at(model, &m).unwrap();
    }

    /// When a project's model files change on disk, the change is ANNOUNCED —
    /// a host that never polls still learns the model moved, and the
    /// announcement names which project it was.
    #[test]
    fn a_model_file_changing_on_disk_is_announced_to_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let r = scryer_core::ModelRef::ProjectLocal(dir.path().canonicalize().unwrap());
        project_with(&r);

        let (tx, rx) = mpsc::channel();
        let state = AppState::new(Arc::new(Recorder(tx)));
        state.add_project(dir.path()).unwrap();

        // The plan write the canvas would have made.
        let mut plan = scryer_core::read_model_at(&r).unwrap();
        plan.nodes[0].description = Some("changed".into());
        scryer_core::write_planned_at(&r, &plan).unwrap();

        let event = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the watcher announced the change");
        assert_eq!(event.name, "model-changed");
        assert_eq!(event.project, r.project_path().to_string_lossy());
    }

    /// The revision is the plan's content, so it moves when the plan does and
    /// stands still when it doesn't. A project with no plan yet has one too —
    /// "no plan" is a base revision like any other.
    #[test]
    fn the_revision_tracks_the_plans_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let r = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());
        let empty = revision(&r);
        assert!(
            !empty.is_empty(),
            "a project with no plan still has a revision"
        );

        project_with(&r);
        scryer_core::ensure_planned_at(&r).unwrap();
        let seeded = revision(&r);
        assert_ne!(seeded, empty);
        assert_eq!(seeded, revision(&r), "reading twice does not move it");

        let mut plan = scryer_core::read_planned_at(&r).unwrap();
        plan.nodes[0].description = Some("changed".into());
        scryer_core::write_planned_at(&r, &plan).unwrap();
        assert_ne!(revision(&r), seeded);
    }

    /// Registering the same project twice leaves the running watcher alone,
    /// and the registry reports what it serves.
    #[test]
    fn registering_a_project_twice_is_one_registration() {
        let dir = tempfile::tempdir().unwrap();
        let r = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());
        project_with(&r);

        let state = AppState::new(Arc::new(crate::events::NullSink));
        let first = state.add_project(dir.path()).unwrap();
        let again = state.add_project(dir.path()).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(state.project_paths().len(), 1);
    }
}
