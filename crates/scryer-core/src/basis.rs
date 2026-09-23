//! The BASIS — what a model read was based on, so a model write can be checked
//! against it.
//!
//! Model writes are node-granular and, until this module existed, carried no
//! version check at all: two writers reading the same node and each writing
//! their own edit produced a LOST UPDATE, the second write silently replacing
//! the first. A from→to check on the one claim being written would catch that
//! one shape and miss WRITE SKEW — writer A rewords claim 1, writer B (on the
//! set it read before A wrote) rewords claim 2 on the same node, and B's
//! whole-node array write drops A's claim 1 without either from-value ever
//! disagreeing.
//!
//! So the check is the industry's OPTIMISTIC CONCURRENCY CONTROL WITH READ-SET
//! VALIDATION: every read a writer can base an edit on answers a `basis` over
//! the RELEVANT SET it showed; every write names the basis it was made against;
//! the engine re-derives the set and refuses the write if anything in it moved,
//! naming what changed. First committer wins, the second re-reads.
//!
//! THE RELEVANT SET is exactly the facts a writer bases an edit on, and nothing
//! else: the governing nodes' claims (committed AND planned — statement, host
//! and flags), their binding directives, and the pending entries in scope. It
//! is NOT the drift scopes, the best-match rankings, the rule slugs, the phase
//! and state lines or the `untested` flag: each of those moves for reasons that
//! have nothing to do with the write — a test report lands, a file changes on
//! disk, a task sentence scores differently — and a refusal on one of them
//! would be a check that cries wolf until nobody reads it.
//!
//! The basis is OPAQUE to its holder — a version token, presented the way an
//! `If-Match` ETag is — and means nothing but "the relevant set as I read it".
//! It is self-describing rather than a handle into engine state: it carries its
//! own scope and its own per-element fingerprints, so a basis works across
//! processes, survives a restart, and lets the refusal name the diff rather
//! than say only that something moved.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::changes::fnv1a64;
use crate::diff::{ElementChange, ElementKind};
use crate::{Group, Node, Responsibility, ScryModel};

/// The prefix every basis token carries: the format's name and version, so a
/// token minted by an older engine is refused as unreadable rather than
/// misread.
const TOKEN_PREFIX: &str = "b1.";

/// What a read showed, named as the scope its relevant set is derived from.
///
/// Field names are one letter on the wire because every byte here is echoed
/// back by the caller in the write; the Rust names are the readable ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// The governing nodes whose CLAIMS and binding directives the read showed.
    #[serde(default, rename = "n", skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    /// Groups whose claims the read showed — a group holds claims exactly as a
    /// node does, and a subtree read carries the groups hanging off it.
    #[serde(default, rename = "g", skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// Nodes the read NAMED but whose claims it did not show — the overview
    /// tree, which is names, kinds and counts. Read-set validation validates
    /// what was read: a writer cannot have based a claim's wording on a read
    /// that never carried it, so the outline contributes the node's own
    /// skeleton facts and no claim.
    #[serde(default, rename = "o", skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
    /// Pending entries beyond those owned by `nodes` / `groups`: a change id,
    /// `"unfiled"`, or `"*"` for every pending entry — which is what an
    /// unfiltered `get_pending` showed.
    #[serde(default, rename = "p", skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
}

impl Scope {
    pub fn nodes(ids: impl IntoIterator<Item = String>) -> Self {
        Scope {
            nodes: ids.into_iter().collect(),
            ..Default::default()
        }
    }

    /// Sorted and deduplicated — two reads that showed the same thing must
    /// derive the same basis whatever order their ids arrived in.
    fn normalized(mut self) -> Self {
        for v in [&mut self.nodes, &mut self.groups, &mut self.outline] {
            v.sort();
            v.dedup();
        }
        // A node whose claims the read showed is not also an outline entry.
        let deep: BTreeSet<&String> = self.nodes.iter().collect();
        self.outline = self
            .outline
            .iter()
            .filter(|id| !deep.contains(id))
            .cloned()
            .collect();
        self
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.groups.is_empty()
            && self.outline.is_empty()
            && self.pending.is_none()
    }
}

/// The relevant set of one read: its scope, and a fingerprint per element in
/// it. The fingerprints are what a stale basis is detected by; the keys are
/// what the refusal names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadSet {
    #[serde(rename = "s")]
    pub scope: Scope,
    #[serde(rename = "k")]
    pub entries: BTreeMap<String, String>,
}

/// One element of the relevant set that moved since the basis was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// `changed`, `new` or `gone` — the three ways a read set can move.
    pub kind: &'static str,
    /// The element in the model's own words: what it is, where it lives, and
    /// enough of its text to recognise.
    pub what: String,
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.what)
    }
}

// --- Deriving the set ---

/// Derive the relevant set for `scope` from the two layers as they now stand.
pub fn read_set(committed: &ScryModel, planned: &ScryModel, scope: Scope) -> ReadSet {
    let scope = scope.normalized();
    let mut entries: BTreeMap<String, String> = BTreeMap::new();

    for id in &scope.nodes {
        // A node's binding directives are its own plus every ancestor's, and
        // both layers carry them — one entry over all of it, since a directive
        // set is read as one thing and a writer honours it as one thing.
        entries.insert(
            format!("d:{id}"),
            directive_fingerprint(committed, planned, id),
        );
        for (layer, model) in [("c", committed), ("p", planned)] {
            if let Some(n) = model.nodes.iter().find(|n| &n.id == id) {
                claim_entries(&mut entries, layer, &n.id, &n.responsibilities);
            }
        }
        if let Some(n) = planned.nodes.iter().find(|n| &n.id == id) {
            outline_entry(&mut entries, n);
        }
    }

    for id in &scope.groups {
        for (layer, model) in [("c", committed), ("p", planned)] {
            if let Some(g) = model.groups.iter().find(|g: &&Group| &g.id == id) {
                claim_entries(&mut entries, layer, &g.id, &g.responsibilities);
            }
        }
    }

    for id in &scope.outline {
        if let Some(n) = planned.nodes.iter().find(|n| &n.id == id) {
            outline_entry(&mut entries, n);
        }
    }

    // Pending entries IN SCOPE. The engine's own queue is the source — so
    // vagrant (code-discovered) elements stay out of the set exactly as they
    // stay out of every read's `pending` list.
    let in_scope_nodes: BTreeSet<&String> = scope.nodes.iter().collect();
    let in_scope_groups: BTreeSet<&String> = scope.groups.iter().collect();
    for ch in crate::diff::pending_elements(committed, planned) {
        let owned = match ch.kind {
            ElementKind::Node => in_scope_nodes.contains(&ch.id),
            ElementKind::Responsibility | ElementKind::Property => ch
                .owner_id
                .as_ref()
                .is_some_and(|o| in_scope_nodes.contains(o) || in_scope_groups.contains(o)),
            ElementKind::Group => in_scope_groups.contains(&ch.id),
            ElementKind::Link => {
                let touches =
                    |end: &Option<String>| end.as_ref().is_some_and(|e| in_scope_nodes.contains(e));
                touches(&ch.from) || touches(&ch.to)
            }
        };
        let filed = match scope.pending.as_deref() {
            None => false,
            Some("*") => true,
            Some("unfiled") => !planned
                .change_map
                .contains_key(&crate::changes::key_for(&ch)),
            Some(cid) => {
                planned
                    .change_map
                    .get(&crate::changes::key_for(&ch))
                    .map(String::as_str)
                    == Some(cid)
            }
        };
        if !owned && !filed {
            continue;
        }
        let key = format!("e:{}", crate::changes::key_for(&ch));
        entries.insert(key, pending_fingerprint(planned, &ch));
    }

    ReadSet { scope, entries }
}

/// A claim's truth for basis purposes: its host, its statement, and the flags a
/// writer's edit stands or falls on. Never its tests, never its anchors, never
/// `lastTouchedAt` — a test report or a cosmetic re-stamp must not refuse a
/// write.
fn claim_entries(
    entries: &mut BTreeMap<String, String>,
    layer: &str,
    host: &str,
    claims: &[Responsibility],
) {
    for r in claims {
        entries.insert(
            format!("{layer}:{}", r.id),
            short(&format!(
                "{host}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
                // The TITLE is part of what a read saw: a claim retitled since
                // the read is a claim the writer would now point at by another
                // name, and an edit grounded on the old reading is grounded on
                // a name that has moved.
                r.title.as_deref().unwrap_or(""),
                r.statement,
                r.concern.as_deref().unwrap_or(""),
                r.vagrant.unwrap_or(false),
                r.vagrant_origin.as_deref().unwrap_or(""),
                r.approved_statement.as_deref().unwrap_or(""),
                r.stale.unwrap_or(false),
                r.stale_proposal.as_deref().unwrap_or(""),
                crate::directives_rendered(&r.directives),
                // A citation is truth-bearing beside the statement, so a basis
                // read before one arrived is stale for a write made after it.
                r.cites.join("\u{2}"),
            )),
        );
    }
}

/// A node's own + inherited directives across both layers, as one fingerprint.
fn directive_fingerprint(committed: &ScryModel, planned: &ScryModel, node_id: &str) -> String {
    let mut buf = String::new();
    for model in [committed, planned] {
        if let Some(n) = model.nodes.iter().find(|n| n.id == node_id) {
            buf.push_str(&crate::directives_rendered(&n.directives));
        }
        buf.push('\u{1}');
        for inh in crate::inherited_directives(model, node_id) {
            buf.push_str(&inh.node_id);
            buf.push('\u{3}');
            buf.push_str(&crate::directives_rendered(&inh.directives));
            buf.push('\u{1}');
        }
    }
    short(&buf)
}

/// What the OVERVIEW showed of a node: the skeleton and the counts, which is
/// all a caller who has not drilled in can have read.
fn outline_entry(entries: &mut BTreeMap<String, String>, n: &Node) {
    entries.insert(
        format!("o:{}", n.id),
        short(&format!(
            "{}\u{1}{:?}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
            n.name,
            n.kind,
            n.parent_id.as_deref().unwrap_or(""),
            n.description.as_deref().unwrap_or(""),
            n.responsibilities.len(),
            n.properties.len(),
        )),
    );
}

/// A pending entry's truth: what the plan says to do to the element, and which
/// change it is filed under.
fn pending_fingerprint(planned: &ScryModel, ch: &ElementChange) -> String {
    let filed = planned
        .change_map
        .get(&crate::changes::key_for(ch))
        .map(String::as_str)
        .unwrap_or("");
    short(&format!(
        "{filed}\u{1}{}",
        serde_json::to_string(&ch.changes).unwrap_or_default()
    ))
}

/// 32 bits of FNV-1a, as 8 hex chars. A per-key comparison, so the only
/// collision that could hide a change is one element's new value colliding with
/// its own old value.
fn short(s: &str) -> String {
    fnv1a64(s.as_bytes())[..8].to_string()
}

// --- The token ---

/// Encode a relevant set as the opaque token a read answers and a write names.
pub fn encode(set: &ReadSet) -> String {
    let json = serde_json::to_vec(set).unwrap_or_else(|_| b"{}".to_vec());
    let mut deflater = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    use std::io::Write;
    let packed = match deflater.write_all(&json).and_then(|()| deflater.finish()) {
        Ok(bytes) => bytes,
        // Deflate over an in-memory buffer cannot fail in practice; if it ever
        // did, an unpacked token still round-trips.
        Err(_) => json.clone(),
    };
    use base64::Engine as _;
    format!(
        "{TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&packed)
    )
}

/// Read a token back. `Err` names what is wrong with it in the caller's terms —
/// a token from another engine, a truncated paste — because the answer is the
/// same either way: re-read and repeat the write.
pub fn decode(token: &str) -> Result<ReadSet, String> {
    let body = token.strip_prefix(TOKEN_PREFIX).ok_or_else(|| {
        format!(
            "this is not a basis this engine minted (a basis starts \"{TOKEN_PREFIX}\"); \
             re-read the model and use the `basis` that read answered"
        )
    })?;
    use base64::Engine as _;
    let packed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|e| format!("the basis is not readable ({e}); re-read the model"))?;
    let mut json = Vec::new();
    use std::io::Read;
    flate2::read::DeflateDecoder::new(&packed[..])
        .read_to_end(&mut json)
        .map_err(|e| format!("the basis is not readable ({e}); re-read the model"))?;
    serde_json::from_slice(&json)
        .map_err(|e| format!("the basis is not readable ({e}); re-read the model"))
}

/// Derive a basis token for `scope` — what a read answers.
pub fn derive(committed: &ScryModel, planned: &ScryModel, scope: Scope) -> Option<String> {
    let scope = scope.normalized();
    if scope.is_empty() {
        // A read that showed nothing a writer could base an edit on answers no
        // basis rather than an empty one that would validate against anything.
        return None;
    }
    Some(encode(&read_set(committed, planned, scope)))
}

// --- The check ---

/// What moved in the relevant set between the basis and the model as it now
/// stands. Empty means the basis is current and the write may land.
pub fn differences(
    committed: &ScryModel,
    planned: &ScryModel,
    basis: &ReadSet,
    now: &ReadSet,
) -> Vec<Difference> {
    let mut out: Vec<Difference> = Vec::new();
    let keys: BTreeSet<&String> = basis.entries.keys().chain(now.entries.keys()).collect();
    for key in keys {
        let was = basis.entries.get(key);
        let is = now.entries.get(key);
        let kind = match (was, is) {
            (Some(a), Some(b)) if a == b => continue,
            (Some(_), Some(_)) => "changed",
            (None, Some(_)) => "new",
            (Some(_), None) => "gone",
            (None, None) => continue,
        };
        out.push(Difference {
            kind,
            what: describe(committed, planned, key),
        });
    }
    out
}

/// Name one element of the relevant set the way the model speaks of it.
fn describe(committed: &ScryModel, planned: &ScryModel, key: &str) -> String {
    let Some((tag, id)) = key.split_once(':') else {
        return key.to_string();
    };
    match tag {
        "c" | "p" => {
            let layer = if tag == "c" { "committed" } else { "planned" };
            match find_claim(committed, planned, id) {
                Some((host, r)) => format!(
                    "the {layer} claim {id} on {host} (\"{}\")",
                    ellipsis(&r.statement)
                ),
                None => format!("the {layer} claim {id} (no longer in the model)"),
            }
        }
        "d" => format!("the binding directives of {id}"),
        "o" => format!("the outline of {id} (its name, kind, parent or claim count)"),
        "e" => format!("the pending entry {id}"),
        _ => key.to_string(),
    }
}

fn find_claim<'a>(
    committed: &'a ScryModel,
    planned: &'a ScryModel,
    resp_id: &str,
) -> Option<(String, &'a Responsibility)> {
    for model in [planned, committed] {
        let hit = model
            .nodes
            .iter()
            .flat_map(|n| n.responsibilities.iter().map(move |r| (n.id.clone(), r)))
            .chain(
                model
                    .groups
                    .iter()
                    .flat_map(|g| g.responsibilities.iter().map(move |r| (g.id.clone(), r))),
            )
            .find(|(_, r)| r.id == resp_id);
        if hit.is_some() {
            return hit;
        }
    }
    None
}

fn ellipsis(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 70 {
        return flat;
    }
    let head: String = flat.chars().take(70).collect();
    format!("{head}…")
}

/// Check a write's `basis` against the model as it now stands. `Ok(ReadSet)`
/// is the basis as read back, so the caller can re-derive over the same scope
/// once its write has landed; `Err` is the refusal text, naming the diff.
pub fn check(committed: &ScryModel, planned: &ScryModel, token: &str) -> Result<ReadSet, String> {
    let basis = decode(token).map_err(|e| format!("REFUSED: {e}."))?;
    let now = read_set(committed, planned, basis.scope.clone());
    let moved = differences(committed, planned, &basis, &now);
    if moved.is_empty() {
        return Ok(basis);
    }
    let mut msg = String::from(
        "REFUSED: the `basis` this write names is stale — the relevant set moved since you \
         read it, so writing now would overwrite somebody else's edit. Nothing was merged and \
         nothing was written: the first committer wins and the second re-reads. Re-read the \
         model, reconcile your edit with what changed, and repeat the write with the new \
         `basis`. What changed since your basis:",
    );
    for d in moved.iter().take(20) {
        msg.push_str(&format!("\n- {d}"));
    }
    if moved.len() > 20 {
        msg.push_str(&format!("\n- … and {} more", moved.len() - 20));
    }
    Err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    fn claim(id: &str, statement: &str) -> Responsibility {
        Responsibility {
            title: None,
            cites: Vec::new(),
            id: id.into(),
            statement: statement.into(),
            concern: None,
            vagrant: None,
            vagrant_origin: None,
            approved_statement: None,
            stale: None,
            stale_proposal: None,
            directives: Vec::new(),
            last_touched_at: None,
        }
    }

    fn node(id: &str, claims: Vec<Responsibility>) -> Node {
        Node {
            id: id.into(),
            kind: Kind::Component,
            name: id.into(),
            parent_id: None,
            external: None,
            technology: None,
            description: None,
            vagrant: None,
            stale: None,
            responsibilities: claims,
            properties: Vec::new(),
            icon: None,
            notes: None,
            position: None,
            directives: Vec::new(),
        }
    }

    fn model(nodes: Vec<Node>) -> ScryModel {
        ScryModel {
            version: crate::SCRY_VERSION.to_string(),
            nodes,
            links: Vec::new(),
            groups: Vec::new(),
            source_map: Default::default(),
            test_map: Default::default(),
            boundaries: Default::default(),
            concerns: Vec::new(),
            changes: Vec::new(),
            change_map: Default::default(),
            policy: None,
        }
    }

    #[test]
    fn a_basis_round_trips_through_its_token() {
        let m = model(vec![node("n1", vec![claim("r1", "**Answers** the call")])]);
        let token = derive(&m, &m, Scope::nodes(["n1".to_string()])).unwrap();
        assert!(token.starts_with(TOKEN_PREFIX));
        let back = decode(&token).unwrap();
        assert_eq!(back.scope.nodes, vec!["n1".to_string()]);
        assert_eq!(back, read_set(&m, &m, Scope::nodes(["n1".to_string()])));
    }

    #[test]
    fn a_reword_elsewhere_in_the_scope_is_the_write_skew_the_basis_catches() {
        let before = model(vec![node(
            "n1",
            vec![
                claim("r1", "**Answers** the call"),
                claim("r2", "**Writes** the log"),
            ],
        )]);
        let token = derive(&before, &before, Scope::nodes(["n1".to_string()])).unwrap();

        // Another writer rewords r1. Our writer is about to write r2.
        let after = model(vec![node(
            "n1",
            vec![
                claim("r1", "**Answers** the call twice"),
                claim("r2", "**Writes** the log"),
            ],
        )]);
        let refusal = check(&after, &after, &token).unwrap_err();
        assert!(refusal.contains("stale"), "{refusal}");
        assert!(
            refusal.contains("r1"),
            "the refusal names claim r1: {refusal}"
        );
        assert!(!refusal.contains("r2"), "r2 did not move: {refusal}");
    }

    #[test]
    fn a_claim_added_to_the_scope_moves_the_basis() {
        let before = model(vec![node("n1", vec![claim("r1", "**Answers** the call")])]);
        let token = derive(&before, &before, Scope::nodes(["n1".to_string()])).unwrap();
        let after = model(vec![node(
            "n1",
            vec![
                claim("r1", "**Answers** the call"),
                claim("r2", "**Writes** the log"),
            ],
        )]);
        let refusal = check(&before, &after, &token).unwrap_err();
        assert!(
            refusal.contains("new:"),
            "an added claim is new in the set: {refusal}"
        );
        assert!(refusal.contains("r2"), "{refusal}");
    }

    #[test]
    fn the_untested_flag_the_tests_and_the_anchors_are_not_in_the_relevant_set() {
        let m = model(vec![node("n1", vec![claim("r1", "**Answers** the call")])]);
        let token = derive(&m, &m, Scope::nodes(["n1".to_string()])).unwrap();

        // A test attachment and an anchor land — the claim reads `untested` no
        // longer. Neither is a fact a writer's edit stands on.
        let mut after = m.clone();
        after.test_map.insert(
            "r1".into(),
            vec![crate::SourceLocation {
                pattern: "tests/a.rs".into(),
                symbol: Some("resp_r1".into()),
                line: None,
                end_line: None,
            }],
        );
        after.source_map.insert(
            "r1".into(),
            vec![crate::SourceLocation {
                pattern: "src/a.rs".into(),
                symbol: Some("answer".into()),
                line: None,
                end_line: None,
            }],
        );
        assert!(check(&after, &after, &token).is_ok());
    }

    #[test]
    fn a_directive_above_the_node_moves_the_basis_and_a_cosmetic_stamp_does_not() {
        let mut parent = node("p1", vec![]);
        parent.kind = Kind::Container;
        let mut child = node("n1", vec![claim("r1", "**Answers** the call")]);
        child.parent_id = Some("p1".into());
        let before = model(vec![parent.clone(), child.clone()]);
        let token = derive(&before, &before, Scope::nodes(["n1".to_string()])).unwrap();

        // A cosmetic re-stamp of the claim: not a fact anybody based an edit on.
        let mut stamped = before.clone();
        stamped.nodes[1].responsibilities[0].last_touched_at = Some(1_789_000_000);
        stamped.nodes[1].position = Some(crate::Position { x: 4.0, y: 2.0 });
        assert!(check(&stamped, &stamped, &token).is_ok());

        // A directive on the PARENT binds the child, so it is in the child's set.
        let mut directed = before.clone();
        directed.nodes[0].directives = vec!["Never log a token".into()];
        let refusal = check(&directed, &directed, &token).unwrap_err();
        assert!(refusal.contains("binding directives of n1"), "{refusal}");
    }

    /// resp-b00631 — a CITATION is in the claim's fingerprint, and a
    /// directive's citation is in its holder's. The basis is the engine's
    /// compare-and-swap: a field it leaves out is a field two writers can
    /// clobber without either being told. So a read taken before an anchor
    /// arrived is refused as the ground for an edit made after it — at claim
    /// altitude and at directive altitude both.
    #[test]
    fn resp_b00631_a_citation_is_part_of_what_the_basis_fingerprints() {
        let before = model(vec![node("n1", vec![claim("r1", "**Answers** the call")])]);
        let token = derive(&before, &before, Scope::nodes(["n1".to_string()])).unwrap();

        // (1) The claim gains an anchor. Nothing else about it moved.
        let mut cited = before.clone();
        cited.nodes[0].responsibilities[0].cites = vec!["doc-intro".into()];
        let refusal = check(&cited, &cited, &token).unwrap_err();
        assert!(
            refusal.contains("r1"),
            "the refusal names the claim whose citation arrived: {refusal}"
        );

        // (2) A DIRECTIVE gains an anchor with its prose held still — the case a
        // fingerprint over the words alone cannot see.
        let mut worded = before.clone();
        worded.nodes[0].directives = vec!["Never log a token".into()];
        let token = derive(&worded, &worded, Scope::nodes(["n1".to_string()])).unwrap();
        let mut anchored = worded.clone();
        anchored.nodes[0].directives = vec![crate::Directive::cited(
            "Never log a token",
            vec!["doc-rules".into()],
        )];
        let refusal = check(&anchored, &anchored, &token).unwrap_err();
        assert!(
            refusal.contains("directives of n1"),
            "an anchor on a directive moves the binding set: {refusal}"
        );

        // (3) And a basis taken WITH the citations in place still passes — a
        // citation is fingerprinted, not merely feared.
        let token = derive(&anchored, &anchored, Scope::nodes(["n1".to_string()])).unwrap();
        assert!(check(&anchored, &anchored, &token).is_ok());
    }

    #[test]
    fn the_token_is_opaque_and_a_foreign_one_is_refused_with_the_way_out() {
        let m = model(vec![node("n1", vec![claim("r1", "**Answers** the call")])]);
        assert!(check(&m, &m, "whatever-i-made-up")
            .unwrap_err()
            .contains("re-read"));
        assert!(check(&m, &m, "b1.not-base64!!")
            .unwrap_err()
            .contains("re-read"));
        // A read that showed nothing answers no basis at all.
        assert!(derive(&m, &m, Scope::default()).is_none());
    }
}
