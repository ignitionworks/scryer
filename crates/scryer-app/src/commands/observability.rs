//! Drift, health, test and probe status, and the two reconcile verdicts.
//!
//! Mirrors `src-tauri/src/observability.rs`, which stays as upstream ships it.
//! The bodies are the same work; what differs is that the desktop's
//! `spawn_blocking` wrappers are gone — the caller decides what thread to run
//! a seconds-long extraction pass on, and the HTTP router puts it on a
//! blocking task of its own.

use crate::commands::project::attribute_last_event;
use crate::error::CommandResult;
use crate::state::AppState;

/// Cheap, agent-free drift status. Mirrors `observability.rs::get_drift_status`,
/// including the seed-and-stay-quiet path for a model that has never been
/// reconciled (with no baseline, every file reads as changed, which is noise,
/// not drift).
pub fn get_drift_status(
    state: &AppState,
    project_path: &str,
) -> CommandResult<Vec<scryer_core::drift::DriftScope>> {
    let r = state.model_ref(project_path)?;
    let project = r.project_path().to_path_buf();

    if !r.sync_path().exists() {
        let _ = scryer_core::write_sync_state(
            &r,
            &scryer_core::drift::SyncState::anchored_now(scryer_core::drift::head_commit(&project)),
        );
        let _ = scryer_extract::anchors::write_baseline(&r);
        return Ok(Vec::new());
    }

    Ok(scryer_extract::anchors::out_of_plan_scopes(&r)?)
}

/// Everything the observability surfaces read, in one deterministic pass.
/// Mirrors `observability.rs::ModelHealthReport`, field for field, so a host
/// can hand it to upstream's frontend unchanged.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelHealthReport {
    pub health: scryer_core::health::ModelHealth,
    /// Per-node build completeness (anchored primitives over authored ones).
    pub completeness: std::collections::BTreeMap<String, scryer_core::health::Completeness>,
    /// Anchors whose code changed or broke since the last reconcile.
    pub anchors: Vec<scryer_extract::anchors::AnchorObservation>,
    /// Anchors silently healed this pass (symbol moved, content unchanged).
    pub reanchored: usize,
    pub derived: scryer_core::build_edges::DerivedGraph,
}

/// Mirrors `observability.rs::get_model_health`. Runs the extractor over the
/// whole repo — seconds on a big project — so call it off any thread that
/// must stay responsive.
pub fn get_model_health(state: &AppState, project_path: &str) -> CommandResult<ModelHealthReport> {
    let r = state.model_ref(project_path)?;
    let project = r.project_path().to_path_buf();

    // Check (and self-heal) anchors first: re-anchoring may update sourceMap
    // line ranges, and the health/evidence below should see the healed model.
    let check = scryer_extract::anchors::check_anchors(&r)?;
    let model = scryer_core::read_model_at(&r)?;

    let (ctx, _) = scryer_extract::extract_context_with_stats(&project)?;
    let files: std::collections::BTreeSet<String> =
        ctx.files.iter().map(|f| f.rel_path.clone()).collect();
    let edges = scryer_core::build_edges::BuildEdges {
        symbol_edges: ctx
            .symbol_edges
            .iter()
            .map(|e| scryer_core::build_edges::CachedEdge {
                src: e.src.clone(),
                dst: e.dst.clone(),
            })
            .collect(),
    };
    // Keep the cross-process cache fresh for the MCP commit tool. Best-effort.
    let _ = scryer_core::build_edges::write_build_edges(&project, &edges);

    // Completeness spans the authored model, so resolve the plan against real
    // files. Boundary globs can own files the parser skips (config/assets), so
    // match against the full inventory, not just the parsed source set.
    let planned = scryer_core::read_planned_at(&r).unwrap_or_else(|_| model.clone());
    let all_files = scryer_extract::list_project_files(&project);
    let dead: std::collections::HashSet<&str> = check
        .observations
        .iter()
        .filter(|o| {
            matches!(
                o.state,
                scryer_extract::anchors::AnchorState::Broken
                    | scryer_extract::anchors::AnchorState::FileMissing
            )
        })
        .map(|o| o.key.as_str())
        .collect();
    let completeness =
        scryer_core::health::resolve_completeness(&model, &planned, &all_files, &dead);

    Ok(ModelHealthReport {
        health: scryer_core::health::compute_health(&model, Some(&planned), Some(&files)),
        completeness,
        anchors: check.observations,
        reanchored: check.reanchored,
        derived: scryer_core::build_edges::derive_graph(&model, &edges),
    })
}

/// Per-claim test verdicts, re-verified against the working tree. Mirrors
/// `observability.rs::get_test_statuses`.
pub fn get_test_statuses(
    state: &AppState,
    project_path: &str,
) -> CommandResult<Vec<scryer_extract::test_status::ClaimTestStatus>> {
    let r = state.model_ref(project_path)?;
    Ok(scryer_extract::test_status::test_statuses(&r)?)
}

/// Per-claim probe results. Mirrors `observability.rs::get_probe_statuses`.
pub fn get_probe_statuses(
    state: &AppState,
    project_path: &str,
) -> CommandResult<Vec<scryer_extract::test_status::ClaimProbeStatus>> {
    let r = state.model_ref(project_path)?;
    Ok(scryer_extract::test_status::probe_statuses(&r)?)
}

/// Dismiss the drift nudge project-wide: advance the reconcile anchor to now
/// and re-fingerprint. Mirrors `observability.rs::reconcile_drift`, plus the
/// actor on the history event core writes.
pub fn reconcile_drift(
    state: &AppState,
    project_path: &str,
    actor: Option<&str>,
) -> CommandResult<()> {
    let r = state.model_ref(project_path)?;
    let project = r.project_path().to_path_buf();
    let before = scryer_core::history::read_history(&r).len();
    scryer_core::write_sync_state(
        &r,
        &scryer_core::drift::SyncState::anchored_now(scryer_core::drift::head_commit(&project)),
    )?;
    // "Reconciled" means the anchors as they stand are the truth.
    scryer_extract::anchors::write_baseline(&r)?;
    attribute_last_event(&r, before, actor);
    Ok(())
}

/// Reconcile one node and its whole subtree without moving the project-wide
/// anchor. Mirrors `observability.rs::reconcile_drift_node`.
pub fn reconcile_drift_node(
    state: &AppState,
    project_path: &str,
    node_id: &str,
    actor: Option<&str>,
) -> CommandResult<()> {
    let r = state.model_ref(project_path)?;
    let project = r.project_path().to_path_buf();
    let model = scryer_core::read_model_at(&r)?;
    let mut sync = scryer_core::read_sync_state(&r);
    // Deletions the dismissal reconciles: inventory files currently absent.
    // They stop counting for this subtree while other owners still see them.
    let missing: std::collections::BTreeSet<String> = sync
        .files
        .iter()
        .filter(|f| !project.join(f).exists())
        .cloned()
        .collect();
    let anchor =
        scryer_core::drift::NodeAnchor::now(scryer_core::drift::head_commit(&project), missing);
    for id in scryer_core::drift::subtree_ids(&model, node_id) {
        sync.nodes.insert(id, anchor.clone());
    }
    let before = scryer_core::history::read_history(&r).len();
    scryer_core::write_sync_state(&r, &sync)?;
    attribute_last_event(&r, before, actor);
    Ok(())
}
