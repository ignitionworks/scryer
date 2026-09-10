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
//! Append-only JSONL (one event per line) keeps writes cheap and crash-safe — a
//! torn final line drops exactly one event instead of corrupting the whole log.

use crate::{ModelRef, SourceLocation};
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
}
