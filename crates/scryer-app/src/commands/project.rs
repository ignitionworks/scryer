//! The model read/write commands.
//!
//! Written against `scryer-core` directly, mirroring the bodies in
//! `src-tauri/src/project.rs`. That file is left exactly as upstream ships it:
//! the desktop shell keeps its own copies, and each rebase diffs the two and
//! carries the deltas across.
//!
//! Two things differ from the desktop's versions, both because a service has
//! many callers where a window had one:
//!  - every write takes an opaque `actor` and threads it into core;
//!  - `write_planned` takes the base revision the caller read at, and refuses
//!    a write that would land on top of somebody else's.

use crate::error::{CommandError, CommandResult};
use crate::state::{revision, AppState};

/// True if the project's `model.scry` predates the current v0.3 schema.
/// Mirrors `src-tauri/src/project.rs::is_legacy_model`.
pub fn is_legacy_model(project_path: &str) -> bool {
    scryer_core::is_legacy_model(std::path::Path::new(project_path))
}

/// Register a project with the service and start watching its `.scryer/`.
/// The service's answer to the desktop's `watch_project`: same effect — a
/// watch that turns file changes into events — but many projects at once, and
/// the events name which one.
pub fn watch_project(state: &AppState, project_path: &str) -> CommandResult<String> {
    let project = state.add_project(std::path::Path::new(project_path))?;
    Ok(project.ref_string())
}

/// The committed model, raw. Mirrors `project.rs::read_model`.
pub fn read_model(state: &AppState, project_path: &str) -> CommandResult<String> {
    let r = state.model_ref(project_path)?;
    Ok(scryer_core::read_model_raw_at(&r)?)
}

/// The plan the canvas edits, raw, WITH the revision it is at.
///
/// The desktop returns bytes alone, because only that window writes them back.
/// A service hands back the revision too: the caller names it on the write,
/// and a write whose base has moved on is refused rather than silently
/// clobbering whoever wrote in between. Mirrors `project.rs::read_planned`,
/// including the shadow-draft heal before the read.
pub fn read_planned(state: &AppState, project_path: &str) -> CommandResult<PlannedRead> {
    let r = state.model_ref(project_path)?;
    // Heal legacy shadow drafts before the plan loads: whatever a client
    // loads it echoes back on save, so a pre-seeding draft would keep
    // re-minting its shadow anchors forever. No-op (and lock-free) when clean.
    let _ = scryer_core::heal_shadow_draft(&r);
    let data = scryer_core::read_planned_raw_at(&r)?;
    Ok(PlannedRead {
        revision: revision(&r),
        data,
    })
}

/// The plan and the revision it was read at.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedRead {
    pub revision: String,
    pub data: String,
}

/// What a plan write settled at.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedWrite {
    /// The revision the plan is at now — what the caller passes as its base on
    /// its next write.
    pub revision: String,
}

/// Write the plan, refusing one whose base revision has moved on, and keeping
/// the change registry on disk authoritative.
///
/// `base_revision` is what the caller last read (or wrote). When it names a
/// revision that is not current, the write is refused with
/// [`CommandError::StaleRevision`] carrying what IS current, so the caller
/// reloads, re-applies its edit, and writes again. `None` skips the check —
/// the desktop's behaviour, for a caller that knows it is alone.
///
/// The registry is not the caller's to send. A client never authors `changes`:
/// a change is opened by the agent over MCP and only ever altered by
/// [`sign_off_change`] and [`close_change`]. What a client DOES author is the
/// tagging — `change_map`, which files each canvas edit under a change — so
/// that half is taken from the body and the registry is taken from disk. Skip
/// this and a client echoing a document it read before a sign-off silently
/// drops the snapshot, and a change the agent opened in between vanishes with
/// it; `None` for `base_revision` leaves nothing else to catch it.
///
/// A DELIBERATE DELTA from upstream: `src-tauri`'s `write_planned` has neither
/// guard, so the desktop keeps the race (its canvas learns of a sign-off only
/// when the file watcher reloads, and a save that beats the reload clobbers
/// it). Additive — the desktop's own command is untouched.
///
/// Everything happens under the model lock, so read-merge-write cannot
/// interleave with the agent's MCP writer.
pub fn write_planned(
    state: &AppState,
    project_path: &str,
    data: &str,
    base_revision: Option<&str>,
    actor: Option<&str>,
) -> CommandResult<PlannedWrite> {
    let r = state.model_ref(project_path)?;
    let _lock = scryer_core::lock_model(&r)?;

    let current = revision(&r);
    if let Some(base) = base_revision {
        if base != current {
            return Err(CommandError::StaleRevision { current });
        }
    }

    // Refused rather than written through: a body that will not parse is one
    // whose registry cannot be merged, and writing it verbatim is exactly the
    // clobber this exists to prevent.
    let mut plan: scryer_core::ScryModel =
        serde_json::from_str(data).map_err(|e| CommandError::BadArguments {
            command: "write_planned".to_string(),
            message: format!("`data` is not a model document: {e}"),
        })?;

    plan.changes = scryer_core::read_planned_seeded_at(&r)?.changes;

    // A canvas save is the DEVELOPER editing the plan — intent by definition.
    // Re-stamp every signed-off change's snapshot, against the plan AS MERGED,
    // so their edits never read as the agent's amendments at the next fold,
    // naming the actor who saved.
    let signed: Vec<String> = plan
        .changes
        .iter()
        .filter(|c| c.signed_off.is_some())
        .map(|c| c.id.clone())
        .collect();
    if !signed.is_empty() {
        let now = scryer_core::drift::now_secs();
        for cid in &signed {
            let _ = scryer_core::changes::sign_off_as(&mut plan, cid, now, actor);
        }
    }

    let json = serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?;
    scryer_core::write_planned_raw_at(&r, &json)?;
    Ok(PlannedWrite {
        revision: revision(&r),
    })
}

/// Sign off a change, recording WHO gave the go-ahead. Mirrors
/// `project.rs::sign_off_change`, plus the actor.
pub fn sign_off_change(
    state: &AppState,
    project_path: &str,
    change_id: &str,
    actor: Option<&str>,
) -> CommandResult<usize> {
    let r = state.model_ref(project_path)?;
    let _lock = scryer_core::lock_model(&r)?;
    let mut plan = scryer_core::read_planned_seeded_at(&r)?;
    let n = scryer_core::changes::sign_off_as(
        &mut plan,
        change_id,
        scryer_core::drift::now_secs(),
        actor,
    )?;
    scryer_core::write_planned_at(&r, &plan)?;
    Ok(n)
}

/// Close an EMPTY open change, recording it as abandoned. Mirrors
/// `project.rs::close_change`; the actor lands on the history event.
pub fn close_change(
    state: &AppState,
    project_path: &str,
    change_id: &str,
    actor: Option<&str>,
) -> CommandResult<()> {
    let r = state.model_ref(project_path)?;
    let _lock = scryer_core::lock_model(&r)?;
    let before = scryer_core::history::read_history(&r).len();
    scryer_core::changes::close_change(&r, change_id)?;
    // core appends the "abandoned" record itself and takes no actor; re-stamp
    // the event it just wrote rather than duplicating close_change's body here.
    attribute_last_event(&r, before, actor);
    Ok(())
}

/// Name `actor` on every history event appended since the log held `before`
/// entries. The log is append-only JSONL, so this rewrites the tail lines in
/// place. Best-effort: a failure here must never undo the operation that
/// produced the events.
pub(crate) fn attribute_last_event(r: &scryer_core::ModelRef, before: usize, actor: Option<&str>) {
    let Some(actor) = actor.map(str::trim).filter(|a| !a.is_empty()) else {
        return;
    };
    let mut events = scryer_core::history::read_history(r);
    if events.len() <= before {
        return;
    }
    for ev in events.iter_mut().skip(before) {
        ev.by = actor.to_string();
    }
    let mut out = String::new();
    for ev in &events {
        match serde_json::to_string(ev) {
            Ok(line) => {
                out.push_str(&line);
                out.push('\n');
            }
            Err(_) => return,
        }
    }
    let _ = std::fs::write(r.history_path(), out);
}

/// The fold-refusal ledger. Mirrors `project.rs::read_fold_refusals`.
pub fn read_fold_refusals(
    state: &AppState,
    project_path: &str,
) -> CommandResult<Vec<scryer_core::refusals::Refusal>> {
    let r = state.model_ref(project_path)?;
    Ok(scryer_core::refusals::read_refusals(&r))
}

/// The durable committed-model history log, oldest first. Mirrors
/// `project.rs::read_history`.
pub fn read_history(state: &AppState, project_path: &str) -> CommandResult<String> {
    let r = state.model_ref(project_path)?;
    let events = scryer_core::history::read_history(&r);
    Ok(serde_json::to_string(&events).map_err(|e| e.to_string())?)
}

/// Create a blank project-local model. Mirrors `project.rs::create_blank_model`.
pub fn create_blank_model(project_path: &str) -> CommandResult<String> {
    let project = std::path::Path::new(project_path);
    if !project.exists() || !project.is_dir() {
        return Err(CommandError::failed(format!(
            "Project path does not exist or is not a directory: {project_path}"
        )));
    }
    let r = scryer_core::ModelRef::ProjectLocal(project.to_path_buf());
    let _lock = scryer_core::lock_model(&r)?;
    let model = scryer_core::ScryModel::new();
    scryer_core::write_model_at(&r, &model)?;
    Ok(r.to_ref_string())
}

/// The user's agent settings. Machine-wide, not per project — mirrors
/// `project.rs::get_subagent_settings`.
pub fn get_subagent_settings() -> scryer_core::SubagentSettings {
    scryer_core::read_subagent_settings()
}

pub fn set_subagent_settings(settings: &scryer_core::SubagentSettings) -> CommandResult<()> {
    Ok(scryer_core::write_subagent_settings(settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NullSink;
    use std::sync::Arc;

    fn project() -> (tempfile::TempDir, AppState, String) {
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
        let state = AppState::new(Arc::new(NullSink));
        (dir, state, r.project_path().to_string_lossy().to_string())
    }

    /// A write that names the revision it read at lands; one that names a
    /// revision somebody else has already written past is REFUSED, and the
    /// refusal carries the revision that IS current — so the caller reloads,
    /// re-applies its edit, and writes again rather than clobbering.
    #[test]
    fn a_plan_write_on_a_stale_base_revision_is_refused_with_the_current_one() {
        let (_dir, state, path) = project();

        let first = read_planned(&state, &path).unwrap();

        // Somebody else writes, moving the revision on.
        let mut plan: scryer_core::ScryModel = serde_json::from_str(&first.data).unwrap();
        plan.nodes[0].description = Some("theirs".into());
        let write = write_planned(
            &state,
            &path,
            &serde_json::to_string(&plan).unwrap(),
            Some(&first.revision),
            None,
        )
        .unwrap();
        assert_ne!(
            write.revision, first.revision,
            "a write moves the revision on"
        );

        // Our edit still names the FIRST revision: it is stale.
        let mut plan: scryer_core::ScryModel = serde_json::from_str(&first.data).unwrap();
        plan.nodes[0].name = "Someone else's edit".into();
        let theirs = serde_json::to_string(&plan).unwrap();
        let err = write_planned(&state, &path, &theirs, Some(&first.revision), None).unwrap_err();
        match &err {
            CommandError::StaleRevision { current } => {
                assert_eq!(
                    current, &write.revision,
                    "the refusal names what IS current"
                );
            }
            other => panic!("expected staleRevision, got {other:?}"),
        }
        assert_eq!(err.status(), 409);

        // The refused write did not land.
        let now = read_planned(&state, &path).unwrap();
        assert!(!now.data.contains("Someone else's edit"));

        // Reload, re-apply, write again: it lands.
        let mut plan: scryer_core::ScryModel = serde_json::from_str(&now.data).unwrap();
        plan.nodes[0].name = "Someone else's edit".into();
        write_planned(
            &state,
            &path,
            &serde_json::to_string(&plan).unwrap(),
            Some(&now.revision),
            None,
        )
        .unwrap();
        assert!(read_planned(&state, &path)
            .unwrap()
            .data
            .contains("Someone else's edit"));
    }

    /// The revision is the CONTENT's, so writing the same bytes back is a
    /// no-op that leaves everyone else's base valid — a save that changed
    /// nothing never invalidates a concurrent editor.
    #[test]
    fn writing_the_same_plan_back_leaves_the_revision_where_it_was() {
        let (_dir, state, path) = project();
        let read = read_planned(&state, &path).unwrap();
        let write = write_planned(&state, &path, &read.data, Some(&read.revision), None).unwrap();
        assert_eq!(write.revision, read.revision);
    }

    /// A caller that names no base revision is not revision-checked at all —
    /// the desktop's behaviour, for a caller that knows it is alone.
    #[test]
    fn a_write_naming_no_base_revision_is_not_checked() {
        let (_dir, state, path) = project();
        let first = read_planned(&state, &path).unwrap();
        write_planned(&state, &path, &first.data, None, None).unwrap();
        write_planned(&state, &path, &first.data, None, None).unwrap();
    }

    /// A write that names an ACTOR records it: the sign-off carries who gave
    /// the go-ahead, and closing a change names them on the history event. A
    /// write with no actor is recorded unattributed rather than refused.
    #[test]
    fn a_write_naming_an_actor_records_it_and_one_without_stays_unattributed() {
        let (_dir, state, path) = project();
        let r = state.model_ref(&path).unwrap();

        let mut plan = scryer_core::read_planned_at(&r).unwrap();
        plan.nodes[0].responsibilities.push(
            serde_json::from_value(
                serde_json::json!({ "id": "resp-1", "statement": "**When** asked, **answer**" }),
            )
            .unwrap(),
        );
        let cid = scryer_core::changes::open_change(&mut plan, "the change", 100);
        scryer_core::changes::tag(&mut plan, &["resp:resp-1".to_string()], &cid);
        scryer_core::write_planned_at(&r, &plan).unwrap();

        sign_off_change(&state, &path, &cid, Some("jesseh")).unwrap();
        let plan = scryer_core::read_planned_at(&r).unwrap();
        assert_eq!(
            plan.changes[0].signed_off.as_ref().unwrap().by.as_deref(),
            Some("jesseh")
        );

        // Closing an empty change names the actor on the history record.
        let mut plan = scryer_core::read_planned_at(&r).unwrap();
        let empty = scryer_core::changes::open_change(&mut plan, "never started", 200);
        scryer_core::write_planned_at(&r, &plan).unwrap();
        close_change(&state, &path, &empty, Some("jesseh")).unwrap();
        let log = scryer_core::history::read_history(&r);
        let abandoned = log.iter().find(|e| e.driver == "abandoned").unwrap();
        assert_eq!(abandoned.by, "jesseh");

        // And with no actor: recorded, unattributed, never refused.
        let mut plan = scryer_core::read_planned_at(&r).unwrap();
        let another = scryer_core::changes::open_change(&mut plan, "also never started", 300);
        scryer_core::write_planned_at(&r, &plan).unwrap();
        close_change(&state, &path, &another, None).unwrap();
        let log = scryer_core::history::read_history(&r);
        assert_eq!(log.last().unwrap().by, "agent");
    }

    /// The plan round-trips through the surface, and the read carries the
    /// revision the write must name.
    #[test]
    fn the_plan_round_trips_and_the_read_carries_its_revision() {
        let (_dir, state, path) = project();
        let read = read_planned(&state, &path).unwrap();
        assert!(!read.revision.is_empty());

        let mut plan: scryer_core::ScryModel = serde_json::from_str(&read.data).unwrap();
        plan.nodes[0].description = Some("revised".into());
        let write = write_planned(
            &state,
            &path,
            &serde_json::to_string(&plan).unwrap(),
            Some(&read.revision),
            None,
        )
        .unwrap();

        let back = read_planned(&state, &path).unwrap();
        assert!(back.data.contains("revised"));
        assert_eq!(
            back.revision, write.revision,
            "the write reports where it landed"
        );
    }
}
