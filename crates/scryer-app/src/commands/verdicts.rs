//! The eleven developer VERDICTS on plan and drift findings: adopt / reject /
//! drop / reimplement a responsibility or a property, reword a claim, drop or
//! reimplement a node.
//!
//! Copied from `src-tauri/src/verdicts.rs` (upstream 0.4.12) with the Tauri
//! command attributes dropped, `cwd` renamed to `project_path`, and an opaque
//! `actor` threaded onto every history event each verdict writes. The desktop
//! keeps its own copy — this crate is additive and never edits that file — so
//! each rebase diffs the two and carries the delta across.
//!
//! Every verdict holds the model lock across its whole read-modify-write, the
//! same as the desktop's: seeding turned these reads into writers, so a verdict
//! that did not lock would race the agent's MCP process.

use crate::error::CommandResult;
use crate::state::AppState;

/// The minted vagrant chain at and above `host_id`: the host plus each ancestor
/// that is itself vagrant, ordered root→leaf. Walks up while nodes are vagrant,
/// stopping at the first committed ancestor. Empty when the host is already a
/// committed node (the finding was routed onto an existing node, not a fresh
/// mint).
fn vagrant_chain(planned: &scryer_core::ScryModel, host_id: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut cur = Some(host_id.to_string());
    while let Some(id) = cur {
        match planned.nodes.iter().find(|n| n.id == id) {
            Some(n) if n.vagrant == Some(true) => {
                chain.push(n.id.clone());
                cur = n.parent_id.clone();
            }
            _ => break,
        }
    }
    chain.reverse();
    chain
}

/// What a vagrant fold captured — the host node, the claim's text and source
/// anchor for the timeline, and the minted chain it committed (for reject's plan
/// cleanup).
struct FoldedVagrant {
    host_id: String,
    statement: String,
    source: Option<scryer_core::SourceLocation>,
    chain: Vec<String>,
}

/// Clear the vagrant flags on a code-discovered responsibility and its minted
/// host chain, then FOLD the chain (root→leaf) and the responsibility into the
/// committed model. Shared by adopt (which keeps it) and reject (which then
/// schedules its deletion). The chain must commit before the responsibility, per
/// `commit_element`'s host-must-exist rule — a freshly minted symbol/component
/// has no committed home until its rungs are folded first.
fn fold_vagrant(model_ref: &scryer_core::ModelRef, resp_id: &str) -> Result<FoldedVagrant, String> {
    use scryer_core::diff::ElementKind;

    // Clear the vagrant flag in the plan and capture host + statement, so the
    // copy `commit_element` folds is a clean, adopted claim (it folds verbatim).
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;
    let mut host_id = None;
    let mut statement = None;
    for n in &mut planned.nodes {
        if let Some(r) = n.responsibilities.iter_mut().find(|r| r.id == resp_id) {
            r.vagrant = None;
            host_id = Some(n.id.clone());
            statement = Some(r.statement.clone());
            break;
        }
    }
    if host_id.is_none() {
        for g in &mut planned.groups {
            if let Some(r) = g.responsibilities.iter_mut().find(|r| r.id == resp_id) {
                r.vagrant = None;
                host_id = Some(g.id.clone());
                statement = Some(r.statement.clone());
                break;
            }
        }
    }
    let (Some(host_id), Some(statement)) = (host_id, statement) else {
        return Err(format!("Responsibility '{resp_id}' not found in the plan"));
    };
    let source = planned
        .source_map
        .get(resp_id)
        .and_then(|locs| locs.first())
        .cloned();
    // The minted rungs above the responsibility (a new component, the symbol for
    // a new function, …) — clear their vagrant flag so they fold as clean nodes.
    let chain = vagrant_chain(&planned, &host_id);
    for id in &chain {
        if let Some(n) = planned.nodes.iter_mut().find(|n| &n.id == id) {
            n.vagrant = None;
        }
    }
    scryer_core::write_planned_at(model_ref, &planned)?;

    // Fold the chain root→leaf (each parent committed before its child), then the
    // responsibility onto its now-committed host.
    for id in &chain {
        scryer_core::commit_element(model_ref, ElementKind::Node, None, id)?;
    }
    scryer_core::commit_element(model_ref, ElementKind::Responsibility, None, resp_id)?;

    Ok(FoldedVagrant {
        host_id,
        statement,
        source,
        chain,
    })
}

/// Whether `resp_id` is a POST-SIGN-OFF amendment/addition awaiting a verdict
/// (as opposed to a code-discovered vagrant): the plan copy carries a
/// `vagrant_origin`. Returns the host id and the origin.
fn amendment_of(planned: &scryer_core::ScryModel, resp_id: &str) -> Option<(String, String)> {
    planned
        .nodes
        .iter()
        .flat_map(|n| n.responsibilities.iter().map(move |r| (n.id.as_str(), r)))
        .chain(
            planned
                .groups
                .iter()
                .flat_map(|g| g.responsibilities.iter().map(move |r| (g.id.as_str(), r))),
        )
        .find(|(_, r)| r.id == resp_id && r.vagrant == Some(true))
        .and_then(|(host, r)| r.vagrant_origin.clone().map(|o| (host.to_string(), o)))
}

/// After a verdict on an amendment, the claim as it now stands IS the
/// developer's intent: re-stamp its entry in the change's sign-off snapshot so
/// it reads as untouched at the next fold (and never as an amendment again).
fn restamp_signed_entry(planned: &mut scryer_core::ScryModel, resp_id: &str) {
    use scryer_core::diff::ElementKind;
    let key = scryer_core::changes::element_key(ElementKind::Responsibility, None, resp_id);
    let Some(cid) = planned.change_map.get(&key).cloned() else {
        return;
    };
    let entry = scryer_core::changes::entry_hash(planned, &key);
    if let Some(snap) = planned
        .changes
        .iter_mut()
        .find(|c| c.id == cid)
        .and_then(|c| c.signed_off.as_mut())
    {
        match entry {
            Some(e) => {
                snap.entries.insert(key, e);
            }
            None => {
                snap.entries.remove(&key);
            }
        }
    }
}

/// Timeline record for an amendment verdict: what was approved, what the
/// agent built, and what the developer decided.
fn log_amendment(
    model_ref: &scryer_core::ModelRef,
    host_id: &str,
    driver: &str,
    approved: Option<&str>,
    amended: &str,
) {
    let mut rows = Vec::new();
    if let Some(a) = approved {
        rows.push(scryer_core::history::EventRow::new("−", a.to_string()));
    }
    rows.push(scryer_core::history::EventRow::new(
        "+",
        amended.to_string(),
    ));
    let _ = scryer_core::history::append_event(
        model_ref,
        &scryer_core::history::HistoryEvent::new(
            scryer_core::drift::now_secs(),
            scryer_core::history::EventKind::Impl,
            host_id,
            driver,
        )
        .with_rows(rows),
    );
}

/// Adopt a POST-SIGN-OFF amendment or addition: the amended text becomes the
/// intent. It stays PENDING in the plan for the agent to fold through the
/// evidence gate — unless it is already anchored AND verified (test attached,
/// current passing verdict), in which case adopting folds it, exactly as a
/// code-discovered vagrant's adopt does. History reads "asked for A, built B,
/// accepted". Returns true when it folded.
fn adopt_amendment(
    model_ref: &scryer_core::ModelRef,
    resp_id: &str,
    host_id: &str,
) -> Result<bool, String> {
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;
    let (approved, amended) = {
        let r = planned
            .nodes
            .iter_mut()
            .flat_map(|n| n.responsibilities.iter_mut())
            .chain(
                planned
                    .groups
                    .iter_mut()
                    .flat_map(|g| g.responsibilities.iter_mut()),
            )
            .find(|r| r.id == resp_id)
            .ok_or_else(|| format!("Responsibility '{resp_id}' not found in the plan"))?;
        r.vagrant = None;
        r.vagrant_origin = None;
        (r.approved_statement.take(), r.statement.clone())
    };
    restamp_signed_entry(&mut planned, resp_id);
    scryer_core::write_planned_at(model_ref, &planned)?;
    log_amendment(
        model_ref,
        host_id,
        "adopted amendment",
        approved.as_deref(),
        &amended,
    );

    // Built AND verified → fold now; otherwise it waits for the agent's fold.
    let committed = scryer_core::read_model_at(model_ref).unwrap_or_default();
    let anchored =
        planned.source_map.contains_key(resp_id) || committed.source_map.contains_key(resp_id);
    let verified = scryer_extract::test_status::claim_evidence(model_ref, &[resp_id.to_string()])
        .ok()
        .and_then(|m| m.get(resp_id).map(|e| e.verified()))
        .unwrap_or(false);
    if anchored && verified {
        scryer_core::commit_element(
            model_ref,
            scryer_core::diff::ElementKind::Responsibility,
            None,
            resp_id,
        )?;
        return Ok(true);
    }
    Ok(false)
}

/// Adopt a code-discovered (vagrant) responsibility: clear its `vagrant` flag in
/// the plan and FOLD it — together with any minted host chain (a new component,
/// the symbol for a new function) — straight into the committed model. Ordinary
/// plan edits are committed only by the agent (`mark_implemented`), but a vagrant
/// claim is source-anchored to code that ALREADY EXISTS — adopting it IS the
/// commit, there is nothing left to implement. This is the one sanctioned case of
/// the canvas writing the committed model, because it is itself a
/// reconcile-to-existing-code (the same direction as `reconcile_drift`), not the
/// human authoring intent ahead of the code. The file watcher then refreshes both
/// layers in the UI.
fn adopt_responsibility_at(
    model_ref: &scryer_core::ModelRef,
    resp_id: String,
) -> Result<(), String> {
    // Seeding turned this verdict's read into a writer, so it must hold the model
    // lock across the whole read-modify-write — otherwise the canvas races the
    // agent's MCP process and the two writers clobber each other.
    let _lock = scryer_core::lock_model(model_ref)?;
    // An amendment is not code the model missed — it is the agent's proposal
    // to change the intent. Adopting it makes the text the intent; it folds
    // only if built and verified (see `adopt_amendment`).
    if let Some((host, _)) = amendment_of(&scryer_core::read_planned_at(model_ref)?, &resp_id) {
        let _ =
            scryer_core::refusals::update_refusals(model_ref, &[], std::slice::from_ref(&resp_id));
        adopt_amendment(model_ref, &resp_id, &host)?;
        return Ok(());
    }
    let folded = fold_vagrant(model_ref, &resp_id)?;

    // Keep the legacy baseline in step and log the fold as a "took code" event,
    // mirroring `mark_implemented`'s Impl event so it lands on the History tab.
    if let Ok(after) = scryer_core::read_model_at(model_ref) {
        let _ = scryer_core::save_baseline_at(model_ref, &after);
    }
    let mut row = scryer_core::history::EventRow::new("+", folded.statement);
    if let Some(loc) = folded.source {
        row = row.with_source(loc);
    }
    let _ = scryer_core::history::append_event(
        model_ref,
        &scryer_core::history::HistoryEvent::new(
            scryer_core::drift::now_secs(),
            scryer_core::history::EventKind::Impl,
            &folded.host_id,
            "took code",
        )
        .with_rows(vec![row]),
    );
    Ok(())
}

/// Reject a code-discovered (vagrant) responsibility: the behaviour should not be
/// in the model. Rather than silently dropping it from the plan — which leaves the
/// code untouched for the next drift check to re-propose — we FOLD it (and any
/// minted host chain) into the committed model, then remove it from the plan.
/// That turns it into an ordinary deletion work item (committed has it, the plan
/// does not → `toDelete`), anchored to the code the agent should remove. Folding
/// it also stops drift re-surfacing it: the committed model now describes the
/// behaviour, so it is no longer "undescribed".
fn reject_responsibility_at(
    model_ref: &scryer_core::ModelRef,
    resp_id: String,
) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    // Rejecting an AMENDMENT restores the approved text and keeps the entry
    // pending: the agent implemented something other than what was asked, and
    // the work is still open. Rejecting an ADDITION (nothing was approved)
    // removes the claim from the plan.
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;
    if let Some((host, origin)) = amendment_of(&planned, &resp_id) {
        let _ =
            scryer_core::refusals::update_refusals(model_ref, &[], std::slice::from_ref(&resp_id));
        let mut amended = String::new();
        let mut approved: Option<String> = None;
        let mut keep = false;
        for resps in planned
            .nodes
            .iter_mut()
            .map(|n| &mut n.responsibilities)
            .chain(planned.groups.iter_mut().map(|g| &mut g.responsibilities))
        {
            if let Some(r) = resps.iter_mut().find(|r| r.id == resp_id) {
                amended = r.statement.clone();
                approved = r.approved_statement.take();
                r.vagrant = None;
                r.vagrant_origin = None;
                if let Some(a) = &approved {
                    r.statement = a.clone();
                    r.last_touched_at = Some(scryer_core::drift::now_secs());
                    keep = true;
                }
                break;
            }
        }
        if !keep {
            for n in &mut planned.nodes {
                n.responsibilities.retain(|r| r.id != resp_id);
            }
            for g in &mut planned.groups {
                g.responsibilities.retain(|r| r.id != resp_id);
            }
            planned.source_map.remove(&resp_id);
            planned.test_map.remove(&resp_id);
        } else {
            restamp_signed_entry(&mut planned, &resp_id);
        }
        scryer_core::write_planned_at(model_ref, &planned)?;
        log_amendment(
            model_ref,
            &host,
            &format!("rejected {origin}"),
            approved.as_deref(),
            &amended,
        );
        return Ok(());
    }
    drop(planned);
    let folded = fold_vagrant(model_ref, &resp_id)?;

    // Schedule the deletion: drop the responsibility and the minted chain from the
    // plan, so the committed-vs-plan diff reads as a deletion to carry out.
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;
    for n in &mut planned.nodes {
        n.responsibilities.retain(|r| r.id != resp_id);
    }
    for g in &mut planned.groups {
        g.responsibilities.retain(|r| r.id != resp_id);
    }
    planned.source_map.remove(&resp_id);
    for id in &folded.chain {
        planned.nodes.retain(|n| &n.id != id);
        planned.source_map.remove(id);
    }
    scryer_core::write_planned_at(model_ref, &planned)?;

    if let Ok(after) = scryer_core::read_model_at(model_ref) {
        let _ = scryer_core::save_baseline_at(model_ref, &after);
    }
    let mut row = scryer_core::history::EventRow::new("−", folded.statement);
    if let Some(loc) = folded.source {
        row = row.with_source(loc);
    }
    let _ = scryer_core::history::append_event(
        model_ref,
        &scryer_core::history::HistoryEvent::new(
            scryer_core::drift::now_secs(),
            scryer_core::history::EventKind::Impl,
            &folded.host_id,
            "rejected — marked for deletion",
        )
        .with_rows(vec![row]),
    );
    Ok(())
}

/// Clear the vagrant flag on a code-discovered PROPERTY (addressed by its owning
/// node + label, since properties carry no id) and its minted host chain, then
/// FOLD the chain (root→leaf) and the property into the committed model. The
/// property-level twin of [`fold_vagrant`]; shared by adopt and reject. A property
/// has no source anchor of its own (the data node bears it), so `source` is None.
fn fold_vagrant_property(
    model_ref: &scryer_core::ModelRef,
    node_id: &str,
    label: &str,
) -> Result<FoldedVagrant, String> {
    use scryer_core::diff::ElementKind;

    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;
    let cleared = planned
        .nodes
        .iter_mut()
        .find(|n| n.id == node_id)
        .and_then(|n| n.properties.iter_mut().find(|p| p.label == label))
        .map(|p| p.vagrant = None)
        .is_some();
    if !cleared {
        return Err(format!(
            "Property '{label}' on node '{node_id}' not found in the plan"
        ));
    }
    // The property may have landed on a freshly minted data symbol; fold that
    // chain first so the host exists in committed before the property folds onto it.
    let chain = vagrant_chain(&planned, node_id);
    for id in &chain {
        if let Some(n) = planned.nodes.iter_mut().find(|n| &n.id == id) {
            n.vagrant = None;
        }
    }
    scryer_core::write_planned_at(model_ref, &planned)?;

    for id in &chain {
        scryer_core::commit_element(model_ref, ElementKind::Node, None, id)?;
    }
    scryer_core::commit_element(model_ref, ElementKind::Property, Some(node_id), label)?;

    Ok(FoldedVagrant {
        host_id: node_id.to_string(),
        statement: label.to_string(),
        source: None,
        chain,
    })
}

/// Adopt a code-discovered (vagrant) property — the property-level twin of
/// [`adopt_responsibility`]. The field already exists in code, so adopting it IS
/// the commit: fold it (and any minted host chain) into the committed model.
fn adopt_property_at(
    model_ref: &scryer_core::ModelRef,
    node_id: String,
    label: String,
) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let folded = fold_vagrant_property(model_ref, &node_id, &label)?;

    if let Ok(after) = scryer_core::read_model_at(model_ref) {
        let _ = scryer_core::save_baseline_at(model_ref, &after);
    }
    let _ = scryer_core::history::append_event(
        model_ref,
        &scryer_core::history::HistoryEvent::new(
            scryer_core::drift::now_secs(),
            scryer_core::history::EventKind::Impl,
            &folded.host_id,
            "took code",
        )
        .with_rows(vec![scryer_core::history::EventRow::new(
            "+",
            folded.statement,
        )]),
    );
    Ok(())
}

/// Reject a code-discovered (vagrant) property — the property-level twin of
/// [`reject_responsibility`]. Fold it (and any minted host chain) into committed,
/// then drop it from the plan so the diff reads as a deletion work item anchored
/// to the field the agent should remove; folding also stops drift re-proposing it.
fn reject_property_at(
    model_ref: &scryer_core::ModelRef,
    node_id: String,
    label: String,
) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let folded = fold_vagrant_property(model_ref, &node_id, &label)?;

    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;
    if let Some(n) = planned.nodes.iter_mut().find(|n| n.id == node_id) {
        n.properties.retain(|p| p.label != label);
    }
    for id in &folded.chain {
        planned.nodes.retain(|n| &n.id != id);
        planned.source_map.remove(id);
    }
    scryer_core::write_planned_at(model_ref, &planned)?;

    if let Ok(after) = scryer_core::read_model_at(model_ref) {
        let _ = scryer_core::save_baseline_at(model_ref, &after);
    }
    let _ = scryer_core::history::append_event(
        model_ref,
        &scryer_core::history::HistoryEvent::new(
            scryer_core::drift::now_secs(),
            scryer_core::history::EventKind::Impl,
            &folded.host_id,
            "rejected — marked for deletion",
        )
        .with_rows(vec![scryer_core::history::EventRow::new(
            "−",
            folded.statement,
        )]),
    );
    Ok(())
}

// ---- Stale (take-model) verdicts: the mirror of adopt/reject. ----
//
// A stale claim/node means the model still asserts something the code stopped
// doing. The flag rides the PLANNED draft (where the UI reads it). Two verdicts,
// mirroring the take-code pair:
//   • DROP        — the code is right (removed on purpose) → delete the claim /
//                   subtree from BOTH layers. Mirror of adopt: the model gives
//                   way to reality.
//   • RE-IMPLEMENT — the model is right (code regressed) → remove from committed
//                   while the plan keeps it, so the diff reads it as an `Added`
//                   to-do the agent rebuilds (folding back via mark_implemented).
//                   Mirror of reject's toDelete, but in the build direction.

/// Remove a responsibility wherever it lives (a node or a group), returning
/// (host_id, statement) and GC'ing its source anchor. None if absent.
fn take_responsibility(
    model: &mut scryer_core::ScryModel,
    resp_id: &str,
) -> Option<(String, String)> {
    for n in &mut model.nodes {
        if let Some(pos) = n.responsibilities.iter().position(|r| r.id == resp_id) {
            let r = n.responsibilities.remove(pos);
            model.source_map.remove(resp_id);
            return Some((n.id.clone(), r.statement));
        }
    }
    for g in &mut model.groups {
        if let Some(pos) = g.responsibilities.iter().position(|r| r.id == resp_id) {
            let r = g.responsibilities.remove(pos);
            model.source_map.remove(resp_id);
            return Some((g.id.clone(), r.statement));
        }
    }
    None
}

/// Remove a set of nodes and everything that hangs off them — descendant claims'
/// anchors, the nodes' own declaration anchors and boundaries, links touching
/// them, and dead group memberships. Mirrors the MCP `delete_nodes` cleanup.
fn prune_nodes(model: &mut scryer_core::ScryModel, ids: &std::collections::HashSet<String>) {
    let resp_ids: std::collections::HashSet<String> = model
        .nodes
        .iter()
        .filter(|n| ids.contains(&n.id))
        .flat_map(|n| n.responsibilities.iter().map(|r| r.id.clone()))
        .collect();
    model
        .source_map
        .retain(|k, _| !resp_ids.contains(k) && !ids.contains(k));
    model.boundaries.retain(|k, _| !ids.contains(k));
    model.nodes.retain(|n| !ids.contains(&n.id));
    model
        .links
        .retain(|l| !ids.contains(&l.src) && !ids.contains(&l.dst));
    for g in &mut model.groups {
        g.member_ids.retain(|m| !ids.contains(m));
    }
}

/// Append a take-model resolution event (the committed model changed). `marker`
/// is the diff glyph for the row (`−` dropped, `+` re-implement to-do).
fn log_take_model(
    model_ref: &scryer_core::ModelRef,
    host_id: &str,
    driver: &str,
    marker: &str,
    text: String,
    source: Option<scryer_core::SourceLocation>,
) {
    let mut row = scryer_core::history::EventRow::new(marker, text);
    if let Some(loc) = source {
        row = row.with_source(loc);
    }
    let _ = scryer_core::history::append_event(
        model_ref,
        &scryer_core::history::HistoryEvent::new(
            scryer_core::drift::now_secs(),
            scryer_core::history::EventKind::Impl,
            host_id,
            driver,
        )
        .with_rows(vec![row]),
    );
}

/// DROP a stale responsibility: the code legitimately no longer does this, so the
/// claim leaves the model entirely (both layers) and its anchor is GC'd. Mirror
/// of `adopt_responsibility`.
fn drop_responsibility_at(
    model_ref: &scryer_core::ModelRef,
    resp_id: String,
) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let mut committed = scryer_core::read_model_at(model_ref)?;
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;

    let source = committed
        .source_map
        .get(&resp_id)
        .or_else(|| planned.source_map.get(&resp_id))
        .and_then(|l| l.first())
        .cloned();

    let from_c = take_responsibility(&mut committed, &resp_id);
    let from_p = take_responsibility(&mut planned, &resp_id);
    let (host_id, statement) = from_c
        .or(from_p)
        .ok_or_else(|| format!("Responsibility '{resp_id}' not found"))?;

    scryer_core::write_model_at(model_ref, &committed)?;
    scryer_core::write_planned_at(model_ref, &planned)?;
    let _ = scryer_core::save_baseline_at(model_ref, &committed);
    log_take_model(
        model_ref,
        &host_id,
        "dropped — removed from code",
        "−",
        statement,
        source,
    );
    Ok(())
}

/// RE-IMPLEMENT a stale responsibility: the model is right and the code must be
/// rebuilt. Remove it from the committed model (which should only hold claims the
/// code satisfies) while the plan keeps a clean, anchored copy — so the diff
/// reads it as an `Added` to-do the agent implements, folding it back in via
/// `mark_implemented`. Mirror of `reject_responsibility`, in the build direction.
fn reimplement_responsibility_at(
    model_ref: &scryer_core::ModelRef,
    resp_id: String,
) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let mut committed = scryer_core::read_model_at(model_ref)?;
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;

    let source = committed
        .source_map
        .get(&resp_id)
        .or_else(|| planned.source_map.get(&resp_id))
        .and_then(|l| l.first())
        .cloned();
    let committed_anchor = committed.source_map.get(&resp_id).cloned();

    // Remove from committed — the code regressed, so it no longer holds.
    let removed = take_responsibility(&mut committed, &resp_id);

    // Keep a clean to-do in the plan: clear stale, ensure it's present + anchored.
    let mut host_id = None;
    let mut statement = None;
    let in_plan = planned
        .nodes
        .iter_mut()
        .flat_map(|n| {
            let nid = n.id.clone();
            n.responsibilities.iter_mut().map(move |r| (nid.clone(), r))
        })
        .chain(planned.groups.iter_mut().flat_map(|g| {
            let gid = g.id.clone();
            g.responsibilities.iter_mut().map(move |r| (gid.clone(), r))
        }))
        .find(|(_, r)| r.id == resp_id);
    if let Some((hid, r)) = in_plan {
        r.stale = None;
        r.stale_proposal = None;
        host_id = Some(hid);
        statement = Some(r.statement.clone());
    } else if let Some((chost, cstmt)) = &removed {
        // The plan had dropped it — reconstruct from committed so the to-do exists.
        if let Some(n) = planned.nodes.iter_mut().find(|n| &n.id == chost) {
            n.responsibilities.push(scryer_core::Responsibility {
                concern: None,
                id: resp_id.clone(),
                statement: cstmt.clone(),
                vagrant: None,
                stale: None,
                stale_proposal: None,
                directives: Vec::new(),
                last_touched_at: None,
                vagrant_origin: None,
                approved_statement: None,
            });
            host_id = Some(chost.clone());
            statement = Some(cstmt.clone());
        }
    }
    if let Some(anchor) = committed_anchor {
        planned.source_map.entry(resp_id.clone()).or_insert(anchor);
    }

    let (Some(host_id), Some(statement)) = (host_id, statement) else {
        return Err(format!("Responsibility '{resp_id}' not found"));
    };

    scryer_core::write_model_at(model_ref, &committed)?;
    scryer_core::write_planned_at(model_ref, &planned)?;
    let _ = scryer_core::save_baseline_at(model_ref, &committed);
    log_take_model(
        model_ref,
        &host_id,
        "re-implement — code regressed",
        "+",
        statement,
        source,
    );
    Ok(())
}

/// Remove a property by (node, label), returning it. None if absent. Properties
/// have no source anchor of their own, so nothing else to GC. Mirror of
/// [`take_responsibility`] for the data-shape layer.
fn take_property(
    model: &mut scryer_core::ScryModel,
    node_id: &str,
    label: &str,
) -> Option<scryer_core::SchemaProperty> {
    let n = model.nodes.iter_mut().find(|n| n.id == node_id)?;
    let pos = n.properties.iter().position(|p| p.label == label)?;
    Some(n.properties.remove(pos))
}

/// DROP a stale property: the code legitimately removed this field, so the property
/// leaves the model entirely (both layers). Property-level twin of
/// [`drop_responsibility`].
fn drop_property_at(
    model_ref: &scryer_core::ModelRef,
    node_id: String,
    label: String,
) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let mut committed = scryer_core::read_model_at(model_ref)?;
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;

    let removed = take_property(&mut committed, &node_id, &label)
        .or_else(|| take_property(&mut planned, &node_id, &label))
        .ok_or_else(|| format!("Property '{label}' on node '{node_id}' not found"))?;
    // Make sure it's gone from BOTH layers regardless of which one matched first.
    take_property(&mut committed, &node_id, &label);
    take_property(&mut planned, &node_id, &label);

    scryer_core::write_model_at(model_ref, &committed)?;
    scryer_core::write_planned_at(model_ref, &planned)?;
    let _ = scryer_core::save_baseline_at(model_ref, &committed);
    log_take_model(
        model_ref,
        &node_id,
        "dropped — removed from code",
        "−",
        removed.label,
        None,
    );
    Ok(())
}

/// RE-IMPLEMENT a stale property: the model is right and the field must be rebuilt.
/// Remove it from committed while the plan keeps a clean copy (stale cleared), so
/// the diff reads it as an `Added` to-do. Property-level twin of
/// [`reimplement_responsibility`].
fn reimplement_property_at(
    model_ref: &scryer_core::ModelRef,
    node_id: String,
    label: String,
) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let mut committed = scryer_core::read_model_at(model_ref)?;
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;

    let removed = take_property(&mut committed, &node_id, &label);

    // Keep a clean to-do in the plan: clear stale, or reconstruct from committed.
    let in_plan = planned
        .nodes
        .iter_mut()
        .find(|n| n.id == node_id)
        .and_then(|n| n.properties.iter_mut().find(|p| p.label == label));
    if let Some(p) = in_plan {
        p.stale = None;
    } else if let Some(prop) = &removed {
        if let Some(n) = planned.nodes.iter_mut().find(|n| n.id == node_id) {
            n.properties.push(scryer_core::SchemaProperty {
                label: prop.label.clone(),
                description: prop.description.clone(),
                vagrant: None,
                stale: None,
                last_touched_at: None,
            });
        }
    } else {
        return Err(format!("Property '{label}' on node '{node_id}' not found"));
    }

    scryer_core::write_model_at(model_ref, &committed)?;
    scryer_core::write_planned_at(model_ref, &planned)?;
    let _ = scryer_core::save_baseline_at(model_ref, &committed);
    log_take_model(
        model_ref,
        &node_id,
        "re-implement — code regressed",
        "+",
        label,
        None,
    );
    Ok(())
}

/// Set a responsibility's statement wherever it lives (nodes or groups), clearing
/// the drift flags and stamping the edit. Returns the host node/group id.
fn reword_in_model(
    model: &mut scryer_core::ScryModel,
    resp_id: &str,
    statement: &str,
    now: u64,
) -> Option<String> {
    let host = model
        .nodes
        .iter_mut()
        .map(|n| (n.id.clone(), &mut n.responsibilities))
        .chain(
            model
                .groups
                .iter_mut()
                .map(|g| (g.id.clone(), &mut g.responsibilities)),
        )
        .find_map(|(hid, resps)| resps.iter_mut().find(|r| r.id == resp_id).map(|r| (hid, r)));
    let (host_id, r) = host?;
    r.statement = statement.to_string();
    r.stale = None;
    r.stale_proposal = None;
    r.last_touched_at = Some(now);
    Some(host_id)
}

/// REWORD a stale responsibility: the code didn't lose the behaviour, it DIVERGED,
/// and drift proposed a corrected statement. Accepting it brings the model in line
/// with code that already exists — so the new wording lands in BOTH layers and the
/// stale/proposal flags clear, leaving the layers identical and thus no plan work
/// item. The reconcile mirror of `drop`/`adopt`: a model edit catching up to
/// reality, not a build to-do. `statement` is the accepted text (drift's proposal,
/// possibly edited by the user).
fn reword_responsibility_at(
    model_ref: &scryer_core::ModelRef,
    resp_id: String,
    statement: String,
) -> Result<(), String> {
    let statement = statement.trim().to_string();
    if statement.is_empty() {
        return Err("Reworded statement is empty".into());
    }
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let mut committed = scryer_core::read_model_at(model_ref)?;
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;

    // Rewording an AMENDMENT: the developer's own text replaces both the
    // approved and the amended statement in the PLAN only — it is intent,
    // still pending, and folds through the agent's gate. Committed is untouched.
    if let Some((host, origin)) = amendment_of(&planned, &resp_id) {
        let _ =
            scryer_core::refusals::update_refusals(model_ref, &[], std::slice::from_ref(&resp_id));
        let now = scryer_core::drift::now_secs();
        let mut approved: Option<String> = None;
        for resps in planned
            .nodes
            .iter_mut()
            .map(|n| &mut n.responsibilities)
            .chain(planned.groups.iter_mut().map(|g| &mut g.responsibilities))
        {
            if let Some(r) = resps.iter_mut().find(|r| r.id == resp_id) {
                approved = r.approved_statement.take();
                r.vagrant = None;
                r.vagrant_origin = None;
                r.statement = statement.clone();
                r.last_touched_at = Some(now);
                break;
            }
        }
        restamp_signed_entry(&mut planned, &resp_id);
        scryer_core::write_planned_at(model_ref, &planned)?;
        log_amendment(
            model_ref,
            &host,
            &format!("reworded {origin}"),
            approved.as_deref(),
            &statement,
        );
        return Ok(());
    }

    let source = committed
        .source_map
        .get(&resp_id)
        .or_else(|| planned.source_map.get(&resp_id))
        .and_then(|l| l.first())
        .cloned();

    let now = scryer_core::drift::now_secs();
    let in_c = reword_in_model(&mut committed, &resp_id, &statement, now);
    let in_p = reword_in_model(&mut planned, &resp_id, &statement, now);
    let host_id = in_c
        .or(in_p)
        .ok_or_else(|| format!("Responsibility '{resp_id}' not found"))?;

    scryer_core::write_model_at(model_ref, &committed)?;
    scryer_core::write_planned_at(model_ref, &planned)?;
    let _ = scryer_core::save_baseline_at(model_ref, &committed);
    log_take_model(
        model_ref,
        &host_id,
        "reworded — code diverged",
        "~",
        statement,
        source,
    );
    Ok(())
}

/// DROP a stale node: the whole subtree's backing code is gone on purpose, so the
/// node and every descendant (claims, links, group memberships, anchors) leaves
/// both layers. The node-level mirror of `drop_responsibility`.
fn drop_node_at(model_ref: &scryer_core::ModelRef, node_id: String) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let mut committed = scryer_core::read_model_at(model_ref)?;
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;

    let in_c = committed.nodes.iter().any(|n| n.id == node_id);
    let in_p = planned.nodes.iter().any(|n| n.id == node_id);
    if !in_c && !in_p {
        return Err(format!("Node '{node_id}' not found"));
    }

    // Name + parent for the timeline (the node itself is about to disappear).
    let (name, parent_id) = committed
        .nodes
        .iter()
        .chain(planned.nodes.iter())
        .find(|n| n.id == node_id)
        .map(|n| (n.name.clone(), n.parent_id.clone()))
        .unwrap_or_else(|| (node_id.clone(), None));

    // Subtree from each layer it lives in (a node may exist in only one).
    let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if in_c {
        ids.extend(scryer_core::drift::subtree_ids(&committed, &node_id));
    }
    if in_p {
        ids.extend(scryer_core::drift::subtree_ids(&planned, &node_id));
    }

    prune_nodes(&mut committed, &ids);
    prune_nodes(&mut planned, &ids);

    scryer_core::write_model_at(model_ref, &committed)?;
    scryer_core::write_planned_at(model_ref, &planned)?;
    let _ = scryer_core::save_baseline_at(model_ref, &committed);
    // Attach to the parent — the node id is gone.
    let host = parent_id.unwrap_or_else(|| node_id.clone());
    log_take_model(
        model_ref,
        &host,
        "dropped — removed from code",
        "−",
        format!("{name} (subtree)"),
        None,
    );
    Ok(())
}

/// RE-IMPLEMENT a stale node: the model is right and the whole subtree must be
/// rebuilt. Remove the subtree from the committed model while the plan keeps it
/// (stale cleared), so each node/claim reads as an `Added` to-do. The node-level
/// mirror of `reimplement_responsibility`.
fn reimplement_node_at(model_ref: &scryer_core::ModelRef, node_id: String) -> Result<(), String> {
    // Serialize the whole read-modify-write against the agent's MCP writer.
    let _lock = scryer_core::lock_model(model_ref)?;
    let mut committed = scryer_core::read_model_at(model_ref)?;
    let mut planned = scryer_core::read_planned_seeded_at(model_ref)?;

    let in_c = committed.nodes.iter().any(|n| n.id == node_id);
    let in_p = planned.nodes.iter().any(|n| n.id == node_id);
    if !in_c && !in_p {
        return Err(format!("Node '{node_id}' not found"));
    }

    let name = planned
        .nodes
        .iter()
        .chain(committed.nodes.iter())
        .find(|n| n.id == node_id)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| node_id.clone());

    let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    if in_c {
        ids.extend(scryer_core::drift::subtree_ids(&committed, &node_id));
    }
    if in_p {
        ids.extend(scryer_core::drift::subtree_ids(&planned, &node_id));
    }

    // Remove the subtree from committed → it becomes `Added` in the plan diff.
    prune_nodes(&mut committed, &ids);
    // Clear stale on the surviving plan subtree (nodes AND their claims) so it
    // reads as clean pending work, not drift.
    for n in &mut planned.nodes {
        if ids.contains(&n.id) {
            n.stale = None;
            for r in &mut n.responsibilities {
                r.stale = None;
            }
        }
    }

    scryer_core::write_model_at(model_ref, &committed)?;
    scryer_core::write_planned_at(model_ref, &planned)?;
    let _ = scryer_core::save_baseline_at(model_ref, &committed);
    log_take_model(
        model_ref,
        &node_id,
        "re-implement — code regressed",
        "+",
        format!("{name} (subtree)"),
        None,
    );
    Ok(())
}

// ── The eleven verdicts, as the service serves them ──────────────────────────
//
// Each resolves the project, records how long the history log was, runs the
// desktop's body verbatim, and then names the ACTOR on whatever events that
// body appended. The bodies stay untouched copies so a rebase diff against
// `src-tauri/src/verdicts.rs` reads as the upstream delta and nothing else.

/// ADOPT a code-discovered claim (or an agent's amendment): its text becomes the model's intent.
pub fn adopt_responsibility(
    state: &AppState,
    project_path: &str,
    resp_id: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = adopt_responsibility_at(&model_ref, resp_id);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// REJECT a claim: an amendment reverts to the approved text and stays pending; an addition leaves the plan.
pub fn reject_responsibility(
    state: &AppState,
    project_path: &str,
    resp_id: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = reject_responsibility_at(&model_ref, resp_id);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// ADOPT a code-discovered data field on a node.
pub fn adopt_property(
    state: &AppState,
    project_path: &str,
    node_id: String,
    label: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = adopt_property_at(&model_ref, node_id, label);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// REJECT a code-discovered data field: folded, then removed.
pub fn reject_property(
    state: &AppState,
    project_path: &str,
    node_id: String,
    label: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = reject_property_at(&model_ref, node_id, label);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// DROP a claim: the model gives it up and the code keeps whatever it does.
pub fn drop_responsibility(
    state: &AppState,
    project_path: &str,
    resp_id: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = drop_responsibility_at(&model_ref, resp_id);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// REIMPLEMENT a claim: the model keeps it and the code owes it again.
pub fn reimplement_responsibility(
    state: &AppState,
    project_path: &str,
    resp_id: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = reimplement_responsibility_at(&model_ref, resp_id);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// DROP a data field from the model.
pub fn drop_property(
    state: &AppState,
    project_path: &str,
    node_id: String,
    label: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = drop_property_at(&model_ref, node_id, label);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// REIMPLEMENT a data field: kept in the model, owed by the code again.
pub fn reimplement_property(
    state: &AppState,
    project_path: &str,
    node_id: String,
    label: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = reimplement_property_at(&model_ref, node_id, label);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// REWORD a claim in the developer's own words, in both layers.
pub fn reword_responsibility(
    state: &AppState,
    project_path: &str,
    resp_id: String,
    statement: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = reword_responsibility_at(&model_ref, resp_id, statement);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// DROP a node: the model gives up on modeling it.
pub fn drop_node(
    state: &AppState,
    project_path: &str,
    node_id: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = drop_node_at(&model_ref, node_id);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}

/// REIMPLEMENT a node: kept in the model, owed by the code again.
pub fn reimplement_node(
    state: &AppState,
    project_path: &str,
    node_id: String,
    actor: Option<&str>,
) -> CommandResult<()> {
    let model_ref = state.model_ref(project_path)?;
    let before = scryer_core::history::read_history(&model_ref).len();
    let out = reimplement_node_at(&model_ref, node_id);
    crate::commands::project::attribute_last_event(&model_ref, before, actor);
    Ok(out?)
}
