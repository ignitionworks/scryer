//! Durable committed-model history — an append-only event log living alongside
//! the model at `.scryer/history.jsonl`.
//!
//! The committed `model.scry` only ever changes through an agent operation — a
//! fold (`mark_implemented`), a drift reconcile, a build, a structural move. Each
//! such operation appends one [`HistoryEvent`] here, so the node page's History
//! tab can show a real timeline ("implemented · 2 days ago") rather than a
//! session-only journal. The log is git-tracked like the model itself: it is not
//! regenerable, so it is the source of truth for what happened when.
//!
//! One kind is not a committed-model event at all: [`EventKind::Plan`] records a
//! plan WRITE — what was proposed rather than what was built. It rides the same
//! log so one timeline answers "when was this claimed?" as well as "when was it
//! implemented?", and it keeps its own kind so a reader can tell the two apart.
//!
//! Append-only JSONL (one event per line) keeps writes cheap and crash-safe — a
//! torn final line drops exactly one event instead of corrupting the whole log.

use crate::{changes, diff, ModelRef, ScryModel, SourceLocation};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;

/// What kind of committed-model event this is — drives the timeline glyph/colour.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    /// Plan claims folded into the committed model + code (`mark_implemented`).
    Impl,
    /// Drift reconciled — code-side reality folded back (`reconcile_drift`).
    Drift,
    /// Structural move — reparent / repoint (`move_nodes`, `move_responsibilities`).
    Move,
    /// A node first entered the committed model (`fill_container`).
    Born,
    /// A plan change closed — its last pending entry folded (or was reverted).
    /// The one event kind that spans nodes: `node_id` is empty, `change_id`
    /// names the change, and the rows carry its rationale.
    Change,
    /// A plan WRITE changed what the plan claims — a canvas save or an agent's
    /// authoring tool. The only kind that is not a fold or a structural move:
    /// it records what was PROPOSED, not what was built, so a reader can show
    /// the two apart. Rows are the touched claims (`+` added, `−` removed,
    /// `!` reworded); `driver` is the change the edits are tagged to, or
    /// `plan` when they are untagged.
    Plan,
}

/// One diff row inside an event: a marker glyph, its text, and an optional source
/// anchor (e.g. an `impl` row pointing at the code that now discharges the claim).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    /// Single-char marker — `+` added, `−` removed, `!` stale, `→` moved.
    pub marker: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLocation>,
}

impl EventRow {
    pub fn new(marker: &str, text: impl Into<String>) -> Self {
        Self { marker: marker.to_string(), text: text.into(), source: None }
    }

    pub fn with_source(mut self, source: SourceLocation) -> Self {
        self.source = Some(source);
        self
    }
}

/// One durable event in the committed-model timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEvent {
    /// Unix seconds.
    pub at: u64,
    /// Who drove it. Upstream writes only the agent here; a host that names an
    /// ACTOR on the write puts that opaque string here instead
    /// ([`HistoryEvent::by_actor`]). Defaulted so an event written without the
    /// field still loads.
    #[serde(default = "agent")]
    pub by: String,
    /// Short driver/intent label shown beside the actor, e.g. "fill", "build",
    /// "took code".
    pub driver: String,
    pub kind: EventKind,
    /// The node this event is about — the per-node History tab filters on it.
    /// Empty for [`EventKind::Change`] events, which span nodes.
    pub node_id: String,
    /// The plan change this event belonged to, when its work was tagged — how
    /// "which change introduced this claim?" gets answered after the fold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<EventRow>,
}

impl HistoryEvent {
    pub fn new(at: u64, kind: EventKind, node_id: impl Into<String>, driver: &str) -> Self {
        Self {
            at,
            by: "agent".to_string(),
            driver: driver.to_string(),
            kind,
            node_id: node_id.into(),
            change_id: None,
            rows: Vec::new(),
        }
    }

    pub fn with_rows(mut self, rows: Vec<EventRow>) -> Self {
        self.rows = rows;
        self
    }

    pub fn with_change(mut self, change_id: impl Into<String>) -> Self {
        self.change_id = Some(change_id.into());
        self
    }

    /// Name the ACTOR behind this event. `None` leaves the event unattributed —
    /// it keeps reading as the agent's, which is what every write without a
    /// named actor is. The string is opaque: the model knows nothing about
    /// people, sessions or teams, only that something signed the write.
    pub fn by_actor(mut self, actor: Option<&str>) -> Self {
        if let Some(a) = actor.map(str::trim).filter(|a| !a.is_empty()) {
            self.by = a.to_string();
        }
        self
    }
}

/// Serde default for [`HistoryEvent::by`] — the writer when none is named.
fn agent() -> String {
    "agent".to_string()
}

/// Append one event to the JSONL log, creating `.scryer/` and the file as needed.
/// Best-effort: a failure here must never abort the model operation that produced
/// the event, so callers ignore the result.
pub fn append_event(r: &ModelRef, ev: &HistoryEvent) -> Result<(), String> {
    let dir = r.dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let line = serde_json::to_string(ev).map_err(|e| e.to_string())?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(r.history_path())
        .map_err(|e| e.to_string())?;
    writeln!(f, "{}", line).map_err(|e| e.to_string())
}

/// The plan events one plan write earns — one per node or group whose CLAIMS
/// the write changed, rows carrying each claim it added, reworded or removed.
///
/// "What the plan claims" is the responsibilities, not the structure: a rename,
/// a reparent or a repointed link is a structural edit with its own event kind,
/// and an anchor or a verdict landing on the draft is bookkeeping. So only
/// [`diff::ElementKind::Responsibility`] entries earn a row, and a write that
/// touches nothing else returns an EMPTY vec — the "a write that changes
/// nothing appends nothing" half, which falls out rather than being special-
/// cased.
///
/// A claim that only MOVED keeps its words, so it earns no row here either; if
/// the move came with a reword, the reword is what shows.
///
/// `driver` names the change the edits are tagged to — read off `after`'s
/// change map, falling back to `before`'s for a claim the write deleted (the
/// ledger GC may already have retired its tag). One event spans one node, so it
/// can only name one change: when the node's touched claims disagree, or none
/// is tagged, the driver is the bare `plan`.
///
/// Deterministic: the diff indexes by `BTreeMap`, and events come back sorted
/// by owner, so the same write always writes the same lines.
pub fn plan_events(before: &ScryModel, after: &ScryModel, at: u64) -> Vec<HistoryEvent> {
    use std::collections::BTreeMap;

    let mut by_owner: BTreeMap<&str, (Vec<EventRow>, Option<&str>, bool)> = BTreeMap::new();
    let plan_diff = diff::diff(before, after);
    for ch in &plan_diff.changes {
        if ch.kind != diff::ElementKind::Responsibility {
            continue;
        }
        let Some(marker) = claim_marker(&ch.changes) else {
            continue;
        };
        let Some(owner) = ch.owner_id.as_deref() else {
            continue;
        };
        let key = changes::key_for(ch);
        let tag = after.change_map.get(&key).or_else(|| before.change_map.get(&key));
        let slot = by_owner.entry(owner).or_insert((Vec::new(), None, false));
        slot.0.push(EventRow::new(marker, ch.label.clone()));
        match (tag.map(String::as_str), slot.1) {
            (Some(cid), None) if !slot.2 => slot.1 = Some(cid),
            (Some(cid), Some(prev)) if cid != prev => {
                slot.1 = None; // two changes in one node: neither speaks for the event
                slot.2 = true;
            }
            _ => {}
        }
    }

    by_owner
        .into_iter()
        .map(|(owner, (rows, cid, _))| {
            let ev = HistoryEvent::new(at, EventKind::Plan, owner, cid.unwrap_or(PLAN_DRIVER))
                .with_rows(rows);
            match cid {
                Some(c) => ev.with_change(c),
                None => ev,
            }
        })
        .collect()
}

/// The driver on a plan event whose edits are untagged — a canvas save outside
/// any change, or a node whose touched claims name two.
const PLAN_DRIVER: &str = "plan";

/// The row marker for one claim's divergence, or `None` when the change is not
/// one the plan-event rows speak: `+` added, `−` removed, `!` reworded.
fn claim_marker(changes: &[diff::Change]) -> Option<&'static str> {
    if changes.iter().any(|c| matches!(c, diff::Change::Added)) {
        return Some("+");
    }
    if changes.iter().any(|c| matches!(c, diff::Change::Deleted)) {
        return Some("−");
    }
    if changes.iter().any(|c| matches!(c, diff::Change::Reworded { .. })) {
        return Some("!");
    }
    None
}

/// Append [`plan_events`] for one plan write, naming `actor` on each. Called at
/// the two seams every plan write passes — [`crate::write_planned_at`] (the
/// agent's authoring tools) and the service's own `write_planned` (the canvas
/// save) — and NOT at [`crate::write_planned_raw_at`], which the fold also uses
/// to rewrite the draft: a fold is not a proposal, and an event there would
/// shadow every `impl` it sits beside.
///
/// Best-effort like [`append_event`]: a log failure never aborts the write.
pub fn append_plan_events(
    r: &ModelRef,
    before: &ScryModel,
    after: &ScryModel,
    actor: Option<&str>,
) {
    for ev in plan_events(before, after, crate::drift::now_secs()) {
        let _ = append_event(r, &ev.by_actor(actor));
    }
}

/// Read the whole log in file order (oldest first). Skips blank or malformed
/// lines so a torn write can never break the timeline; returns empty when absent.
pub fn read_history(r: &ModelRef) -> Vec<HistoryEvent> {
    let raw = match fs::read_to_string(r.history_path()) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn append_then_read_round_trips_in_order() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());

        // Empty log reads as no events.
        assert!(read_history(&r).is_empty());

        let born = HistoryEvent::new(100, EventKind::Born, "n1", "build")
            .with_rows(vec![EventRow::new("+", "3 responsibilities · component")]);
        let impld = HistoryEvent::new(200, EventKind::Impl, "n1", "fill").with_rows(vec![
            EventRow::new("+", "Charges the card via Stripe.").with_source(SourceLocation {
                pattern: "api/payment/handler.rs".into(),
                symbol: Some("charge".into()),
                line: Some(40),
                end_line: Some(78),
            }),
        ]);
        append_event(&r, &born).unwrap();
        append_event(&r, &impld).unwrap();

        let log = read_history(&r);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].kind, EventKind::Born);
        assert_eq!(log[1].kind, EventKind::Impl);
        assert_eq!(log[1].rows[0].source.as_ref().unwrap().line, Some(40));

        // An event written before the actor field existed still loads: `by`
        // defaults to the agent rather than the whole line being dropped.
        fs::write(
            r.history_path(),
            r#"{"at":300,"driver":"fill","kind":"born","nodeId":"n2"}"#.to_string() + "\n",
        )
        .unwrap();
        let legacy = read_history(&r);
        assert_eq!(legacy.len(), 1, "an event without `by` still loads");
        assert_eq!(legacy[0].by, "agent");

        // A garbage line is skipped, surrounding events survive.
        fs::write(
            r.history_path(),
            format!(
                "{}\n%%not json%%\n{}\n",
                serde_json::to_string(&born).unwrap(),
                serde_json::to_string(&impld).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(read_history(&r).len(), 2);
    }

    /// An appended event that carries an ACTOR names that actor as the one who
    /// drove it; one that carries none stays the agent's — unattributed, never
    /// refused. The actor is opaque: blank and whitespace-only name nobody.
    #[test]
    fn an_event_carrying_an_actor_names_it_instead_of_the_agent() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());

        append_event(
            &r,
            &HistoryEvent::new(100, EventKind::Impl, "n1", "build").by_actor(Some("jesseh")),
        )
        .unwrap();
        append_event(&r, &HistoryEvent::new(200, EventKind::Impl, "n1", "build")).unwrap();
        append_event(
            &r,
            &HistoryEvent::new(300, EventKind::Impl, "n1", "build").by_actor(Some("   ")),
        )
        .unwrap();

        let log = read_history(&r);
        assert_eq!(log[0].by, "jesseh", "the named actor drove it");
        assert_eq!(log[1].by, "agent", "no actor supplied: unattributed");
        assert_eq!(log[2].by, "agent", "a blank actor names nobody");
    }

    /// While the timeline is read, a plan event comes back as its OWN kind —
    /// never collapsed into the fold beside it — so a reader can show what was
    /// proposed apart from what was built. The wire name is `plan`.
    #[test]
    fn resp_hmesby_a_plan_event_reads_back_as_its_own_kind() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());

        let proposed = HistoryEvent::new(100, EventKind::Plan, "n1", "chg-7")
            .with_change("chg-7")
            .with_rows(vec![EventRow::new("+", "**When** the card is charged, **record** it")]);
        let built = HistoryEvent::new(200, EventKind::Impl, "n1", "fill");
        append_event(&r, &proposed).unwrap();
        append_event(&r, &built).unwrap();

        let log = read_history(&r);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].kind, EventKind::Plan, "the proposal keeps its own kind");
        assert_ne!(log[0].kind, log[1].kind, "a plan event is not the fold beside it");
        assert_eq!(log[0].change_id.as_deref(), Some("chg-7"));
        assert_eq!(log[0].rows[0].marker, "+");

        // The serialised name every other reader keys on.
        let line = serde_json::to_string(&proposed).unwrap();
        assert!(line.contains(r#""kind":"plan""#), "serialises as `plan`: {line}");
    }

    /// A reader that has never heard of an event kind SKIPS that line — the
    /// same rule that carries a torn write — so `plan` landing in an old
    /// project's log cannot take the timeline down with it. Simulated by a kind
    /// no build knows: today's reader is tomorrow's old reader.
    #[test]
    fn resp_hmesby_an_event_of_an_unknown_kind_is_skipped_not_fatal() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());

        let before = HistoryEvent::new(100, EventKind::Born, "n1", "build");
        let after = HistoryEvent::new(300, EventKind::Plan, "n1", "plan");
        append_event(&r, &before).unwrap(); // creates `.scryer/`; overwritten below
        fs::write(
            r.history_path(),
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&before).unwrap(),
                r#"{"at":200,"by":"agent","driver":"plan","kind":"quorum","nodeId":"n1"}"#,
                serde_json::to_string(&after).unwrap(),
            ),
        )
        .unwrap();

        let log = read_history(&r);
        assert_eq!(log.len(), 2, "the unknown kind is dropped, its neighbours survive");
        assert_eq!(log[0].kind, EventKind::Born);
        assert_eq!(log[1].kind, EventKind::Plan);
    }

    // --- plan events ---

    /// A plan with one node carrying the given claims.
    fn plan_of(claims: &[(&str, &str)]) -> ScryModel {
        let mut m = ScryModel::new();
        let mut node: crate::Node = serde_json::from_value(
            serde_json::json!({ "id": "node-1", "kind": "system", "name": "Acme" }),
        )
        .unwrap();
        for (id, statement) in claims {
            node.responsibilities.push(
                serde_json::from_value(serde_json::json!({ "id": id, "statement": statement }))
                    .unwrap(),
            );
        }
        m.nodes.push(node);
        m
    }

    /// One event per touched node, with the claims it added, reworded and
    /// removed as rows under the existing markers — and the change the edits
    /// are tagged to as the driver.
    #[test]
    fn resp_ag8ngf_a_plan_write_earns_one_event_per_touched_node() {
        let before = plan_of(&[("resp-1", "**When** asked, **answer**"), ("resp-2", "**Log** it")]);
        let mut after = plan_of(&[
            ("resp-1", "**When** asked politely, **answer**"),
            ("resp-3", "**Retry** once"),
        ]);
        after.change_map.insert("resp:resp-1".into(), "chg-7".into());
        after.change_map.insert("resp:resp-3".into(), "chg-7".into());
        after.change_map.insert("resp:resp-2".into(), "chg-7".into());

        let events = plan_events(&before, &after, 500);
        assert_eq!(events.len(), 1, "one node touched, one event");
        let ev = &events[0];
        assert_eq!(ev.kind, EventKind::Plan);
        assert_eq!(ev.node_id, "node-1");
        assert_eq!(ev.driver, "chg-7", "the change the edits are tagged to");
        assert_eq!(ev.change_id.as_deref(), Some("chg-7"));

        let rows: Vec<(&str, &str)> =
            ev.rows.iter().map(|r| (r.marker.as_str(), r.text.as_str())).collect();
        assert!(rows.contains(&("!", "**When** asked politely, **answer**")), "reworded: {rows:?}");
        assert!(rows.contains(&("+", "**Retry** once")), "added: {rows:?}");
        assert!(rows.contains(&("−", "**Log** it")), "removed: {rows:?}");
        assert_eq!(rows.len(), 3);
    }

    /// A write that changes nothing the plan CLAIMS appends nothing — an
    /// identical plan, and a plan whose only edit is bookkeeping (an anchor
    /// landing, a tag retiring) rather than a claim.
    #[test]
    fn resp_ag8ngf_a_write_that_changes_no_claim_appends_nothing() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        let before = plan_of(&[("resp-1", "**When** asked, **answer**")]);

        assert!(plan_events(&before, &before, 500).is_empty(), "an identical plan is silent");

        let mut after = before.clone();
        after.source_map.insert(
            "resp-1".into(),
            vec![SourceLocation {
                pattern: "src/lib.rs".into(),
                symbol: Some("answer".into()),
                line: None,
                end_line: None,
            }],
        );
        after.change_map.insert("resp:resp-1".into(), "chg-7".into());
        assert!(
            plan_events(&before, &after, 500).is_empty(),
            "an anchor landing and a tag are not claims"
        );

        append_plan_events(&r, &before, &after, Some("jesseh"));
        assert!(read_history(&r).is_empty(), "nothing appended, so no log at all");
    }

    /// The driver falls back to the bare `plan` when the edits carry no tag —
    /// and when one node's touched claims name two different changes, since a
    /// single event cannot speak for both.
    #[test]
    fn resp_ag8ngf_untagged_or_split_edits_drive_as_plan() {
        let before = plan_of(&[]);
        let after = plan_of(&[("resp-1", "**Retry** once"), ("resp-2", "**Log** it")]);

        let untagged = plan_events(&before, &after, 500);
        assert_eq!(untagged[0].driver, "plan");
        assert_eq!(untagged[0].change_id, None);

        let mut split = after.clone();
        split.change_map.insert("resp:resp-1".into(), "chg-7".into());
        split.change_map.insert("resp:resp-2".into(), "chg-8".into());
        let split = plan_events(&before, &split, 500);
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].driver, "plan", "two changes in one node: neither speaks for it");
        assert_eq!(split[0].change_id, None);
        assert_eq!(split[0].rows.len(), 2, "both claims are still on the record");
    }

    /// The actor who drove the write names the plan event, exactly as it names
    /// every other kind; a write with no actor stays the agent's.
    #[test]
    fn resp_ag8ngf_the_actor_on_the_write_names_the_plan_event() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        let before = plan_of(&[]);
        let after = plan_of(&[("resp-1", "**Retry** once")]);

        append_plan_events(&r, &before, &after, Some("jesseh"));
        append_plan_events(&r, &before, &after, None);

        let log = read_history(&r);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].by, "jesseh");
        assert_eq!(log[1].by, "agent");
        assert!(log.iter().all(|e| e.kind == EventKind::Plan));
    }
}
