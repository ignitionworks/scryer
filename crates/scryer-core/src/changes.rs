//! The change ledger — named partitions of the plan.
//!
//! The plan is one draft file; without this module it is a single anonymous
//! transaction ("git's working tree without branches"). A **change** gives a
//! slice of that draft a name and a rationale — the dev's original sentence,
//! the one artifact that otherwise dies in the chat log — so that pending work
//! can be listed, reviewed, folded, and resumed *per task* instead of as one
//! pile.
//!
//! The representation is a side-map, not a field on every element (mirroring
//! `source_map`/`boundaries`): [`ScryModel::change_map`] maps an element key
//! (see [`element_key`]) to a change id, and [`ScryModel::changes`] is the
//! registry of open changes. Both live ONLY in the plan layer — the committed
//! model never carries change state ([`crate::write_model_at`] strips it), and
//! a change's durable record after it closes is a [`crate::history`] event.
//! A side-map also covers what a per-element field cannot: a planned
//! *deletion* has no element left to tag, but its key can still map to the
//! change that ordered it.
//!
//! Lifecycle: a change opens with a rationale ([`open_change`]), writes tag
//! the elements they author ([`tag`]), and the plan↔committed diff remains the
//! single source of what is pending — the map holds no state of its own beyond
//! the grouping. That is enforced by the GC invariant: **every map key must
//! correspond to a current diff entry** ([`gc`] prunes the rest). A change
//! closes when a prune takes its last key — folding an element removes it from
//! the diff (implemented), and so does reverting it (abandoned); either way
//! the change's record is appended to history ([`record_closed`]) and the
//! registry entry is dropped. "If it's committed, it's done" — there is no
//! separate close verb.
//!
//! An open change with NO tagged elements yet is legitimate (just opened, or
//! all its work still unwritten) and is never GC'd: [`gc`] closes only
//! changes whose keys the prune itself removed.

use crate::diff::{diff, ElementChange, ElementKind};
use crate::drift::now_secs;
use crate::history::{append_event, EventKind, EventRow, HistoryEvent};
use crate::{ModelRef, ScryModel};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Project-level policy on how changes are approved.
///
/// Unlike everything else in this module, a policy is NOT plan state: it is
/// the project's own setting, persisted with the COMMITTED model
/// (`.scryer/model.scry`, key `policy`) so it is git-tracked and shared by
/// everyone working the repo, and so a plan draft cannot switch it off.
/// [`crate::write_model_at`] strips `changes`/`change_map` on the way to
/// committed but leaves this standing, and the fold reads it from committed.
///
/// ABSENT MEANS EVERY POLICY OFF. A model written before the field existed —
/// upstream's, and a solo user's — loads and behaves exactly as before; only a
/// project that opts in by writing the key sees a gate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// `true` = a fold refuses a change that no team member OTHER THAN its
    /// author has signed off ([`crate::ScryModel::policy`], read by the fold
    /// gate). Off by default: the sign-off gates ask "has the plan drifted
    /// since approval", and only a team that opts in also asks "was it
    /// approved by someone else".
    #[serde(default, skip_serializing_if = "is_false")]
    pub require_countersigned_folds: bool,
    /// `true` = opening a change without a title is refused ([`open_change_titled`]).
    /// Off by default: a title is optional, and a change that has none reads by
    /// the first line of its rationale ([`title_of`]). A project whose surfaces
    /// show changes by name opts in, and then every session against the project
    /// is held to it — which is the point: a check one caller can forget is not
    /// a check.
    #[serde(default, skip_serializing_if = "is_false")]
    pub require_change_titles: bool,
}

/// `skip_serializing_if` for a bool that defaults to false — off stays absent
/// from the file, so opting out leaves no trace and the diff shows only what a
/// project actually turned on.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Whether this project requires a fold to be countersigned by a team member
/// other than the change's author. False for every model that carries no
/// policy — the default, and the only behaviour upstream has.
pub fn requires_countersigned_folds(model: &ScryModel) -> bool {
    model
        .policy
        .as_ref()
        .is_some_and(|p| p.require_countersigned_folds)
}

/// Whether this project refuses to open a change with no title. False for every
/// model that carries no policy — the default, and the only behaviour upstream
/// has.
pub fn requires_change_titles(model: &ScryModel) -> bool {
    model
        .policy
        .as_ref()
        .is_some_and(|p| p.require_change_titles)
}

/// The longest a change's title may be, in CHARACTERS — not bytes, so the limit
/// means the same thing in every script.
pub const MAX_TITLE_LEN: usize = 80;

/// Check a title and return it trimmed. Length is always checked; whether an
/// ABSENT title is refused is the project's policy ([`requires_change_titles`]),
/// because a title is a thing a reader needs, not a thing the ledger needs.
pub fn validate_title(title: &str) -> Result<String, String> {
    let t = title.trim();
    if t.is_empty() {
        return Err("a title is empty — give the change a short name, or pass none".to_string());
    }
    let n = t.chars().count();
    if n > MAX_TITLE_LEN {
        return Err(format!(
            "title is {n} characters; at most {MAX_TITLE_LEN} — say what the change is, \
             not what it does (the rationale is where that goes)"
        ));
    }
    Ok(t.to_string())
}

/// What a reader shows for a change: its title, or — for a change opened before
/// titles existed, or by a caller that gave none — the FIRST LINE of its
/// rationale. Never empty for a change with either, so no surface has to decide
/// what to draw when a title is missing.
pub fn title_of(meta: &ChangeMeta) -> &str {
    if let Some(t) = meta.title.as_deref() {
        let t = t.trim();
        if !t.is_empty() {
            return t;
        }
    }
    meta.rationale
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// One open change in the plan's registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeMeta {
    /// Stable id, minted `chg-N`.
    pub id: String,
    /// The dev's original sentence — why this change exists. Survives the fold
    /// as the history record's text.
    pub rationale: String,
    /// A short name for the change, at most [`MAX_TITLE_LEN`] characters — what
    /// a person calls it, where the rationale is what they said about it.
    /// Absent on every change opened before the field existed and on every
    /// caller that passes none; [`title_of`] then reads the rationale's first
    /// line, so a reader always has something short to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Unix seconds.
    pub created_at: u64,
    /// The developer's sign-off, when given: a snapshot of every entry tagged
    /// to the change at that moment. Anything the AGENT changes about the plan
    /// afterwards is classified against it ([`classify_against_signoff`]) —
    /// an amendment or addition is a proposal awaiting the developer's
    /// verdict, never intent that folds silently. `None` = unsigned (today's
    /// serial behaviour: every plan write folds as intent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_off: Option<SignOff>,
}

/// One sign-off snapshot: when it was stamped, and the content of each tagged
/// entry as it stood.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignOff {
    /// Unix seconds.
    pub at: u64,
    /// WHO gave the go-ahead, when the caller named an actor. An opaque string
    /// — the ledger knows nothing about people, sessions or teams. Absent on a
    /// sign-off nobody signed, and on every file written before the field
    /// existed, so upstream's models still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The PERSON the signature was given for, when [`SignOff::by`] signed as
    /// their proxy — a host whose agent signs for a developer records the
    /// agent in `by` and the developer here. Two names, never one: the proxy
    /// counts as its own signer (`by` is the identity any countersignature
    /// test compares) yet the signature is never read as the person's own.
    /// Absent on a direct sign-off, and on every file written before the field
    /// existed, so upstream's models still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<String>,
    /// WHOSE hand moved the plan on since this signature: the snapshot below is
    /// still the text [`SignOff::by`] approved, but it is no longer the text
    /// the plan holds, and the signer has not seen the difference
    /// ([`restamp_signoffs_as`]). A surface shows it so a re-request can
    /// follow; nothing gates on it.
    ///
    /// The name IS the flag ([`SignOff::is_stale`]) — one field, so "it went
    /// stale" and "who moved it" cannot drift apart, and a reader is never told
    /// their approval is out of date without being told by whom.
    ///
    /// ABSENT IS THE ONLY STATE MOST PROJECTS SEE: it takes a write naming an
    /// actor OTHER than the signer to set, and a solo user has one hand while
    /// an unattributed write — the desktop's canvas save — is the behaviour
    /// that shipped before this existed. Neither ever writes the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staled_by: Option<String>,
    /// Element key ([`element_key`]) → the entry's signed content.
    #[serde(default)]
    pub entries: BTreeMap<String, SignedEntry>,
}

impl SignOff {
    /// Whether the plan moved on under someone else's hand since this
    /// signature. The name and the fact are one field, so this is the only
    /// place that asks the question and there is nothing to keep in sync.
    pub fn is_stale(&self) -> bool {
        self.staled_by.is_some()
    }
}

/// What a sign-off remembered about one entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignedEntry {
    /// [`entry_hash`] of the entry's truth-bearing fields at sign-off.
    pub hash: String,
    /// For a responsibility: the approved statement, so a rejected amendment
    /// can be restored verbatim and a review shows approved vs amended text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    /// For a responsibility: the host it sat on at sign-off (a move is an
    /// amendment too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// How one entry of a signed-off change stands against the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Classification {
    /// Matches the snapshot — the developer's intent, folds normally.
    Untouched,
    /// Existed at sign-off, content differs now — a reword, move, or repoint
    /// the developer did not approve.
    Amended,
    /// Absent at sign-off — scope the agent invented after the go-ahead.
    Added,
    /// Present at sign-off, gone from the plan (or reverted) now.
    Dropped,
}

impl Classification {
    /// The `vagrant_origin` value the fold stamps on a withheld claim.
    pub fn origin(self) -> Option<&'static str> {
        match self {
            Classification::Amended => Some("amendment"),
            Classification::Added => Some("addition"),
            _ => None,
        }
    }
}

/// FNV-1a 64 as 16 hex chars — the same cheap, dependency-free digest the
/// anchor baseline uses for span content.
pub fn fnv1a64(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The signed content of one plan entry: a hash over its TRUTH-BEARING fields
/// only, so the things a developer or canvas touches cosmetically — concern
/// tags, positions, icons, directives, the fossilization stamp — never make an
/// untouched entry read as amended (the same exclusions `last_touched_at`
/// applies). `None` when the key names nothing in the plan (a pending
/// deletion, or a malformed key).
pub fn entry_hash(model: &ScryModel, key: &str) -> Option<SignedEntry> {
    let (kind, owner, id) = parse_key(key)?;
    let hash = |s: String| fnv1a64(s.as_bytes());
    match kind {
        ElementKind::Responsibility => {
            let (host, r) = model
                .nodes
                .iter()
                .flat_map(|n| n.responsibilities.iter().map(move |r| (&n.id, r)))
                .chain(
                    model
                        .groups
                        .iter()
                        .flat_map(|g| g.responsibilities.iter().map(move |r| (&g.id, r))),
                )
                .find(|(_, r)| r.id == id)?;
            Some(SignedEntry {
                hash: hash(format!("resp|{host}|{}", r.statement)),
                statement: Some(r.statement.clone()),
                host: Some(host.clone()),
            })
        }
        ElementKind::Property => {
            let owner = owner?;
            let p = model
                .nodes
                .iter()
                .find(|n| n.id == owner)?
                .properties
                .iter()
                .find(|p| p.label == id)?;
            Some(SignedEntry {
                hash: hash(format!("prop|{owner}|{}|{}", p.label, p.description)),
                statement: None,
                host: Some(owner),
            })
        }
        ElementKind::Node => {
            let n = model.nodes.iter().find(|n| n.id == id)?;
            Some(SignedEntry {
                hash: hash(format!(
                    "node|{:?}|{}|{}|{}|{}|{}",
                    n.kind,
                    n.name,
                    n.parent_id.as_deref().unwrap_or(""),
                    n.description.as_deref().unwrap_or(""),
                    n.technology.as_deref().unwrap_or(""),
                    n.external.unwrap_or(false)
                )),
                statement: None,
                host: None,
            })
        }
        ElementKind::Link => {
            let l = model.links.iter().find(|l| l.id == id)?;
            Some(SignedEntry {
                hash: hash(format!(
                    "link|{}|{}|{}|{}",
                    l.src,
                    l.dst,
                    l.label,
                    l.method.as_deref().unwrap_or("")
                )),
                statement: None,
                host: None,
            })
        }
        ElementKind::Group => {
            let g = model.groups.iter().find(|g| g.id == id)?;
            let mut members = g.member_ids.clone();
            members.sort();
            Some(SignedEntry {
                hash: hash(format!(
                    "group|{}|{}|{}|{}|{}",
                    g.name,
                    g.description.as_deref().unwrap_or(""),
                    members.join(","),
                    g.parent_group_id.as_deref().unwrap_or(""),
                    g.parent_node_id.as_deref().unwrap_or("")
                )),
                statement: None,
                host: None,
            })
        }
    }
}

/// The sign-off content of one key as the snapshot stores it: a live entry's
/// [`entry_hash`], or the literal `deleted` marker for a tagged key whose
/// element the plan has already removed (a pending deletion IS signed intent).
fn signed_entry(model: &ScryModel, key: &str) -> SignedEntry {
    entry_hash(model, key).unwrap_or(SignedEntry {
        hash: "deleted".to_string(),
        statement: None,
        host: None,
    })
}

/// Stamp `change_id` as signed off: snapshot every entry currently tagged to
/// it. Re-stamping replaces the snapshot (the developer approved the plan as
/// it now stands). Returns the number of entries captured. The caller persists
/// the plan.
pub fn sign_off(model: &mut ScryModel, change_id: &str, now: u64) -> Result<usize, String> {
    sign_off_as(model, change_id, now, None)
}

/// [`sign_off`], naming the ACTOR who gave the go-ahead. `None` re-stamps
/// without disturbing whoever signed before — a canvas save re-stamps every
/// signed change ([`restamp_signoffs`]) and must never erase the signature.
pub fn sign_off_as(
    model: &mut ScryModel,
    change_id: &str,
    now: u64,
    actor: Option<&str>,
) -> Result<usize, String> {
    sign_off_for(model, change_id, now, actor, None)
}

/// [`sign_off_as`], naming the PERSON the signature is FOR when `actor` gave
/// it as their proxy. `actor` is the signer, `on_behalf_of` the person; a
/// direct sign-off passes `None` and leaves [`SignOff::on_behalf_of`] unset.
///
/// The pair is atomic. A named `actor` takes the `on_behalf_of` handed in with
/// it — whoever signs last owns both halves of the attribution. `actor: None`
/// is the canvas's unattributed re-stamp ([`restamp_signoffs`]) and carries
/// BOTH forward: erasing either half would turn a recorded proxy signature
/// into someone's own, or into nobody's.
pub fn sign_off_for(
    model: &mut ScryModel,
    change_id: &str,
    now: u64,
    actor: Option<&str>,
    on_behalf_of: Option<&str>,
) -> Result<usize, String> {
    let keys: Vec<String> = model
        .change_map
        .iter()
        .filter(|(_, v)| v.as_str() == change_id)
        .map(|(k, _)| k.clone())
        .collect();
    let entries: BTreeMap<String, SignedEntry> = keys
        .iter()
        .map(|k| (k.clone(), signed_entry(model, k)))
        .collect();
    let n = entries.len();
    let meta = model
        .changes
        .iter_mut()
        .find(|c| c.id == change_id)
        .ok_or_else(|| format!("no open change '{change_id}'"))?;
    let named = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
    };
    let (by, on_behalf_of) = match named(actor) {
        // A named signer owns the whole attribution, proxy or not.
        Some(by) => (Some(by), named(on_behalf_of)),
        // Unattributed re-stamp: keep whoever signed, and who for. With
        // nobody signed before either, there is no proxy to record — "on
        // behalf of" says nothing without the actor it qualifies.
        None => match meta.signed_off.as_ref() {
            Some(prev) => (prev.by.clone(), prev.on_behalf_of.clone()),
            None => (None, None),
        },
    };
    // A NAMED signature is a fresh approval and clears staleness: whoever is
    // signing has just looked. An unattributed re-stamp carries it, for the
    // same reason it carries the signer — an anonymous save must not be able
    // to launder away the fact that someone else moved the plan.
    let staled_by = match (named(actor), meta.signed_off.as_ref()) {
        (None, Some(prev)) => prev.staled_by.clone(),
        _ => None,
    };
    meta.signed_off = Some(SignOff {
        at: now,
        by,
        on_behalf_of,
        staled_by,
        entries,
    });
    Ok(n)
}

/// Re-stamp every signed-off change against the plan as it now stands — the
/// developer's own edits (canvas saves) are intent by definition, so they
/// must never read as amendments. A no-op for unsigned changes. Returns how
/// many changes were re-stamped.
pub fn restamp_signoffs(model: &mut ScryModel, now: u64) -> usize {
    let signed: Vec<String> = model
        .changes
        .iter()
        .filter(|c| c.signed_off.is_some())
        .map(|c| c.id.clone())
        .collect();
    for cid in &signed {
        let _ = sign_off(model, cid, now);
    }
    signed.len()
}

/// What [`restamp_signoffs_as`] did, change by change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restamp {
    /// Changes re-stamped against the plan as written — the writer's own
    /// approvals, which their edit is by definition.
    pub restamped: Vec<String>,
    /// Changes whose signature the write made [`SignOff::stale`] instead.
    pub staled: Vec<String>,
}

/// [`restamp_signoffs`], asking WHOSE hand wrote the plan.
///
/// The re-stamp exists because a developer's own edit is intent: the snapshot
/// follows it, so their next fold sees no amendment. That reasoning holds for
/// exactly one person — the one who signed. A plan a COLLEAGUE rewrote is a
/// plan the signer has not seen, and re-stamping it would silently extend
/// their approval over someone else's sentences. So a write by any other named
/// actor leaves the snapshot where it is and marks the signature stale, which
/// a surface shows and a re-request answers.
///
/// UNCHANGED FOR ONE PAIR OF HANDS. It takes two named, different identities
/// to stale anything: a write with no actor (the desktop canvas, and every
/// plain `scryer-core` caller) re-stamps as it always has, and so does a
/// signature nobody signed by name. A solo user never meets this.
pub fn restamp_signoffs_as(model: &mut ScryModel, now: u64, actor: Option<&str>) -> Restamp {
    fn named(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|a| !a.is_empty())
    }
    let writer = named(actor).map(str::to_string);
    let signed: Vec<(String, Option<String>)> = model
        .changes
        .iter()
        .filter_map(|c| c.signed_off.as_ref().map(|s| (c.id.clone(), s.by.clone())))
        .collect();
    let mut out = Restamp::default();
    for (cid, signer) in signed {
        let someone_else = matches!(
            (writer.as_deref(), named(signer.as_deref())),
            (Some(w), Some(s)) if w != s
        );
        if someone_else {
            if let Some(snap) = model
                .changes
                .iter_mut()
                .find(|c| c.id == cid)
                .and_then(|c| c.signed_off.as_mut())
            {
                snap.staled_by = writer.clone();
            }
            out.staled.push(cid);
        } else {
            let _ = sign_off_as(model, &cid, now, writer.as_deref());
            out.restamped.push(cid);
        }
    }
    out
}

/// Every entry of a signed-off change that does NOT read as untouched intent:
/// `(key, classification, snapshot)`. Keys tagged now: in the snapshot with
/// the same hash → untouched (omitted); different hash → `Amended`; not in the
/// snapshot → `Added`. Snapshot keys no longer tagged → `Dropped` (the element
/// left the plan, or was reverted to committed and GC'd). Empty for an
/// unsigned change — nothing to compare against.
pub fn classify_against_signoff(
    model: &ScryModel,
    meta: &ChangeMeta,
) -> Vec<(String, Classification, Option<SignedEntry>)> {
    let Some(snap) = &meta.signed_off else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut tagged: Vec<&String> = model
        .change_map
        .iter()
        .filter(|(_, v)| v.as_str() == meta.id)
        .map(|(k, _)| k)
        .collect();
    tagged.sort();
    for key in &tagged {
        let now = signed_entry(model, key);
        match snap.entries.get(*key) {
            Some(was) if was.hash == now.hash => {}
            Some(was) => out.push(((*key).clone(), Classification::Amended, Some(was.clone()))),
            None => out.push(((*key).clone(), Classification::Added, None)),
        }
    }
    for (key, was) in &snap.entries {
        if !tagged.contains(&key) {
            out.push((key.clone(), Classification::Dropped, Some(was.clone())));
        }
    }
    out
}

/// The classification of ONE key against its change's sign-off, for the fold:
/// `None` when the key is unfiled, its change is unsigned, or it is untouched.
pub fn classify_key(
    model: &ScryModel,
    key: &str,
) -> Option<(String, Classification, Option<SignedEntry>)> {
    let cid = model.change_map.get(key)?;
    let meta = model.changes.iter().find(|c| &c.id == cid)?;
    let snap = meta.signed_off.as_ref()?;
    let now = signed_entry(model, key);
    match snap.entries.get(key) {
        Some(was) if was.hash == now.hash => None,
        Some(was) => Some((cid.clone(), Classification::Amended, Some(was.clone()))),
        None => Some((cid.clone(), Classification::Added, None)),
    }
}

/// Canonical change-map key for an element. Kind-prefixed so a key classifies
/// itself back to a diff element without consulting either layer; properties
/// have no id, so they key by `(owner node, label)` exactly as the diff does.
/// Owner ids must not contain `:` (minted ids never do) — the label may.
pub fn element_key(kind: ElementKind, owner_id: Option<&str>, id: &str) -> String {
    match kind {
        ElementKind::Node => format!("node:{id}"),
        ElementKind::Link => format!("link:{id}"),
        ElementKind::Group => format!("group:{id}"),
        ElementKind::Responsibility => format!("resp:{id}"),
        ElementKind::Property => format!("prop:{}:{id}", owner_id.unwrap_or("")),
    }
}

/// The key of a diff entry — the join point between `change_map` and the plan
/// diff ([`gc`]'s validity test, and how surfaces group pending entries).
pub fn key_for(change: &ElementChange) -> String {
    element_key(change.kind, change.owner_id.as_deref(), &change.id)
}

/// Decompose a map key back into the `(kind, owner, id)` triple
/// `commit_element` consumes — how "fold *this change*" expands into element
/// folds. Returns None for a malformed key.
pub fn parse_key(key: &str) -> Option<(ElementKind, Option<String>, String)> {
    let (kind, rest) = key.split_once(':')?;
    match kind {
        "node" => Some((ElementKind::Node, None, rest.to_string())),
        "link" => Some((ElementKind::Link, None, rest.to_string())),
        "group" => Some((ElementKind::Group, None, rest.to_string())),
        "resp" => Some((ElementKind::Responsibility, None, rest.to_string())),
        "prop" => {
            let (owner, label) = rest.split_once(':')?;
            Some((
                ElementKind::Property,
                Some(owner.to_string()),
                label.to_string(),
            ))
        }
        _ => None,
    }
}

/// Open a new change: mint the next `chg-N` id (seeded past every id the plan
/// has seen, registry or map, so a re-open never collides with a tag left by
/// a closed twin) and register it. The caller persists the plan.
pub fn open_change(model: &mut ScryModel, rationale: &str, now: u64) -> String {
    open_change_titled(model, None, rationale, now)
        .expect("a change with no title is refused only under a policy this path cannot reach")
}

/// [`open_change`] with the change's TITLE. `None` opens an untitled change,
/// which reads by its rationale's first line ([`title_of`]) — unless the project
/// requires titles ([`requires_change_titles`]), when it is refused. A title
/// longer than [`MAX_TITLE_LEN`] is refused whatever the policy.
///
/// The policy is read from the model this writes, so a caller holding only the
/// plan gets the plan's copy; the fold reads committed for the same reason
/// [`requires_countersigned_folds`] does.
pub fn open_change_titled(
    model: &mut ScryModel,
    title: Option<&str>,
    rationale: &str,
    now: u64,
) -> Result<String, String> {
    let title = match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => Some(validate_title(t)?),
        None if requires_change_titles(model) => {
            return Err(format!(
                "this project requires a change to have a title: pass one of at most \
                 {MAX_TITLE_LEN} characters beside the rationale"
            ));
        }
        None => None,
    };
    let id = crate::ids::mint_id_from(
        "chg",
        model
            .changes
            .iter()
            .map(|c| c.id.as_str())
            .chain(model.change_map.values().map(|s| s.as_str())),
    );
    model.changes.push(ChangeMeta {
        id: id.clone(),
        rationale: rationale.trim().to_string(),
        title,
        created_at: now,
        signed_off: None,
    });
    Ok(id)
}

/// Give an open change a title, or replace the one it has — how a change opened
/// before the field existed stops reading by its rationale's first line.
pub fn set_title(model: &mut ScryModel, change_id: &str, title: &str) -> Result<(), String> {
    let title = validate_title(title)?;
    let Some(meta) = model.changes.iter_mut().find(|c| c.id == change_id) else {
        return Err(format!("no open change '{change_id}'"));
    };
    meta.title = Some(title);
    Ok(())
}

/// Tag plan elements as belonging to `change_id`. Last writer wins — a re-tag
/// replaces the old owner — but each key that was already claimed by a
/// DIFFERENT change is returned as `(key, previous change id)` so the caller
/// can surface the collision: two changes rewording the same claim is exactly
/// the conflict the ledger exists to catch before the code merges.
pub fn tag(model: &mut ScryModel, keys: &[String], change_id: &str) -> Vec<(String, String)> {
    let mut conflicts = Vec::new();
    for k in keys {
        if let Some(prev) = model.change_map.get(k) {
            if prev != change_id {
                conflicts.push((k.clone(), prev.clone()));
            }
        }
        model.change_map.insert(k.clone(), change_id.to_string());
    }
    conflicts
}

/// What a [`retag`] pass moved, for the caller's report.
#[derive(Debug, Default)]
pub struct Retag {
    /// Element keys whose change tag changed, with where they came from —
    /// `(key, previous change id or None for unfiled)`.
    pub moved: Vec<(String, Option<String>)>,
    /// Targets that named nothing pending. Never an error on their own (an id
    /// whose work is already folded is a no-op, not a mistake), but always
    /// reported: silence here is how a caller concludes a move happened when
    /// it didn't.
    pub unmatched: Vec<String>,
}

/// The host a pending element files under — the carrier the Changes page and
/// `get_pending` show it beneath, so "retag this node" means the same set of
/// elements the caller was just looking at. A claim/property files under its
/// owner, a link under the side that performs it, a node/group under itself.
fn host_of(planned: &ScryModel, committed: &ScryModel, ec: &ElementChange) -> Option<String> {
    match ec.kind {
        ElementKind::Node | ElementKind::Group => Some(ec.id.clone()),
        ElementKind::Responsibility | ElementKind::Property => ec.owner_id.clone(),
        ElementKind::Link => planned
            .links
            .iter()
            .chain(committed.links.iter())
            .find(|l| l.id == ec.id)
            .map(|l| l.src.clone()),
    }
}

/// Move pending work between changes — the ledger's only re-filing verb.
///
/// Tags are otherwise a side-effect of writing: whatever a session touches
/// lands in whatever change that session selected. That is right until it
/// isn't (work filed under the wrong change, or one task's plan turning out to
/// be two), and without this the only repair is re-writing the elements
/// themselves under a different selection — editing the spec to fix its
/// bookkeeping.
///
/// `targets` are the BARE ids the caller already holds, resolved against the
/// CURRENT plan diff (the gc invariant: a tag with no pending entry is dead):
///   - a node or group id — that carrier AND every pending element under it,
///     the same unit `get_pending` shows;
///   - a responsibility or link id — that element alone;
///   - a `chg-N` id — everything currently filed under it;
///   - `"unfiled"` — every pending element with no tag.
///
/// `to` is the destination change, or None to detach to unfiled. Idempotent:
/// an element already filed where it is being sent is not reported as moved.
pub fn retag(
    committed: &ScryModel,
    planned: &mut ScryModel,
    targets: &[String],
    to: Option<&str>,
) -> Result<Retag, String> {
    if let Some(dst) = to {
        if !planned.changes.iter().any(|c| c.id == dst) {
            return Err(format!("no open change '{dst}'"));
        }
    }
    let pending = crate::diff::pending_elements(committed, planned);
    let mut out = Retag::default();
    for target in targets {
        let mut hit = false;
        for ec in &pending {
            let key = key_for(ec);
            let current = planned.change_map.get(&key).cloned();
            let matches = if target == "unfiled" {
                current.is_none()
            } else if target.starts_with("chg-") {
                current.as_deref() == Some(target.as_str())
            } else {
                // Direct id, or the carrier this element files under. A
                // property's `id` is its label, never an id — it moves only
                // through its owner.
                (ec.kind != ElementKind::Property && &ec.id == target)
                    || host_of(planned, committed, ec).as_ref() == Some(target)
            };
            if !matches {
                continue;
            }
            hit = true;
            if current.as_deref() == to {
                continue;
            }
            match to {
                Some(dst) => planned.change_map.insert(key.clone(), dst.to_string()),
                None => planned.change_map.remove(&key),
            };
            out.moved.push((key, current));
        }
        if !hit {
            out.unmatched.push(target.clone());
        }
    }
    Ok(out)
}

/// Whether folding the element at `host_key` must NOT carry the element at
/// `elem_key` along: the element belongs to a different change than its host,
/// so it is another task's pending work — a whole-node fold leaves it in the
/// plan exactly as it leaves vagrants. Untagged elements ride any fold (the
/// unfiled serial workflow), and an element always rides its own change.
pub fn foreign_to_host(
    map: &std::collections::BTreeMap<String, String>,
    host_key: &str,
    elem_key: &str,
) -> bool {
    match map.get(elem_key) {
        None => false,
        Some(c) => map.get(host_key) != Some(c),
    }
}

/// What a [`gc`] pass did: how many dead keys it pruned, and which changes
/// that pruning finished (removed from the registry; the caller records them
/// via [`record_closed`] and persists the plan when `pruned > 0`).
#[derive(Debug, Default)]
pub struct Gc {
    pub pruned: usize,
    pub closed: Vec<ChangeMeta>,
}

/// The tags a change carries that name nothing pending — the keys [`gc`] would
/// prune, in the order they are stored.
///
/// A tag goes dead two ways: its element folded into committed, or it was
/// edited back to its committed form. A key that was never canonical (see
/// [`element_key`]) is dead on arrival for the same reason — the diff has no
/// entry by that name. Returned rather than counted so a caller can SAY which
/// tags are dead; a malformed key is invisible otherwise, and reads exactly
/// like a ledger that vanished on its own.
pub fn dead_tags(committed: &ScryModel, planned: &ScryModel, change_id: &str) -> Vec<String> {
    let valid: HashSet<String> = diff(committed, planned)
        .changes
        .iter()
        .map(key_for)
        .collect();
    planned
        .change_map
        .iter()
        .filter(|(key, value)| value.as_str() == change_id && !valid.contains(key.as_str()))
        .map(|(key, _)| key.clone())
        .collect()
}

/// Whether [`gc`] would close this change as abandoned on the next plan write:
/// it carries tags, and every one of them is dead.
///
/// A change carrying NO tags is not abandoned — it was just opened and its work
/// is not written yet — which is why this asks for one live tag rather than for
/// any tag at all.
pub fn would_abandon(committed: &ScryModel, planned: &ScryModel, change_id: &str) -> bool {
    let tagged = planned
        .change_map
        .iter()
        .filter(|(_, value)| value.as_str() == change_id)
        .count();
    tagged > 0 && dead_tags(committed, planned, change_id).len() == tagged
}

/// Enforce the ledger invariant: every `change_map` key corresponds to a
/// current plan-diff entry. A key goes stale two ways — its element folded
/// into committed (implemented) or was edited back to its committed form
/// (abandoned) — and in both cases the pending entry it named no longer
/// exists, so the tag is dead. Prune the dead keys, then close every change
/// the prune emptied. Changes that simply HAVE no keys are left alone (just
/// opened, work not yet written): only a change whose last key died in this
/// pass closes here.
pub fn gc(committed: &ScryModel, planned: &mut ScryModel) -> Gc {
    if planned.change_map.is_empty() && planned.changes.is_empty() {
        return Gc::default();
    }
    let valid: HashSet<String> = diff(committed, planned)
        .changes
        .iter()
        .map(key_for)
        .collect();
    let before = planned.change_map.len();
    let mut candidates: HashSet<String> = HashSet::new();
    planned.change_map.retain(|k, v| {
        let keep = valid.contains(k);
        if !keep {
            candidates.insert(v.clone());
        }
        keep
    });
    let live: HashSet<&String> = planned.change_map.values().collect();
    let closed: Vec<ChangeMeta> = planned
        .changes
        .iter()
        .filter(|c| candidates.contains(&c.id) && !live.contains(&c.id))
        .cloned()
        .collect();
    planned
        .changes
        .retain(|c| !closed.iter().any(|x| x.id == c.id));
    Gc {
        pruned: before - planned.change_map.len(),
        closed,
    }
}

/// Close an EMPTY open change by hand — the escape hatch for a stranded
/// ledger (opened, but its work ended up tagged or folded elsewhere), which
/// [`gc`] deliberately never touches because "no keys yet" is also what a
/// freshly opened change looks like. Refuses a change that still has tagged
/// entries: those close by folding or reverting the entries themselves, never
/// by discarding the grouping. The close is recorded as "abandoned" so the
/// rationale survives. The caller must hold the model lock.
pub fn close_change(r: &ModelRef, change_id: &str) -> Result<ChangeMeta, String> {
    let mut plan = crate::read_planned_at(r)?;
    let Some(pos) = plan.changes.iter().position(|c| c.id == change_id) else {
        return Err(format!("no open change '{change_id}'"));
    };
    let entries = plan.change_map.values().filter(|v| *v == change_id).count();
    if entries > 0 {
        return Err(format!(
            "{change_id} still has {entries} tagged entr{} — fold or revert them; \
             the change closes itself when its last entry goes",
            if entries == 1 { "y" } else { "ies" }
        ));
    }
    let meta = plan.changes.remove(pos);
    crate::write_planned_at(r, &plan)?;
    record_closed(r, &meta, "abandoned");
    Ok(meta)
}

/// One planned entry an abandonment dropped, for the caller's report and the
/// history row.
#[derive(Debug, Clone, PartialEq)]
pub struct DroppedEntry {
    /// The element key ([`element_key`]) that was tagged to the change.
    pub key: String,
    /// The element's name or statement, as the plan had it.
    pub label: String,
    /// What the plan was doing to it — "added", "reworded", "deleted", "moved".
    pub what: String,
}

/// What [`abandon_change`] took out of the plan.
#[derive(Debug, Clone)]
pub struct Abandoned {
    pub meta: ChangeMeta,
    pub dropped: Vec<DroppedEntry>,
}

/// The single word for what the plan is doing to an element, for a reader.
fn what_of(ec: &ElementChange) -> &'static str {
    use crate::diff::Change as C;
    if ec.changes.iter().any(|c| matches!(c, C::Added)) {
        "added"
    } else if ec.changes.iter().any(|c| matches!(c, C::Deleted)) {
        "deleted"
    } else if ec.changes.iter().any(|c| matches!(c, C::Moved { .. })) {
        "moved"
    } else {
        "reworded"
    }
}

fn is_added(ec: &ElementChange) -> bool {
    ec.changes
        .iter()
        .any(|c| matches!(c, crate::diff::Change::Added))
}

/// Undo ONE pending entry: take the plan back to what committed says about that
/// element. An entry the plan ADDED is removed; anything else — reworded, moved,
/// deleted — is restored from committed.
///
/// Scoped to the element the key names and nothing else: a node's own fields are
/// restored while its claims and properties are left alone, because each of
/// those is a pending entry in its own right and may belong to another change.
fn revert_one(plan: &mut ScryModel, committed: &ScryModel, ec: &ElementChange) {
    let added = is_added(ec);
    match ec.kind {
        ElementKind::Responsibility => {
            for n in &mut plan.nodes {
                n.responsibilities.retain(|r| r.id != ec.id);
            }
            for g in &mut plan.groups {
                g.responsibilities.retain(|r| r.id != ec.id);
            }
            if !added {
                for n in &committed.nodes {
                    if let Some(r) = n.responsibilities.iter().find(|r| r.id == ec.id) {
                        if let Some(target) = plan.nodes.iter_mut().find(|p| p.id == n.id) {
                            target.responsibilities.push(r.clone());
                        }
                    }
                }
                for cg in &committed.groups {
                    if let Some(r) = cg.responsibilities.iter().find(|r| r.id == ec.id) {
                        if let Some(target) = plan.groups.iter_mut().find(|p| p.id == cg.id) {
                            target.responsibilities.push(r.clone());
                        }
                    }
                }
            }
        }
        ElementKind::Property => {
            // A property has no id of its own: the diff names it by LABEL on the
            // node `owner_id` names (diff.rs), and `element_key` keys it that way.
            let Some(owner) = ec.owner_id.as_deref() else {
                return;
            };
            if let Some(n) = plan.nodes.iter_mut().find(|n| n.id == owner) {
                n.properties.retain(|p| p.label != ec.id);
            }
            if !added {
                let restored = committed
                    .nodes
                    .iter()
                    .find(|n| n.id == owner)
                    .and_then(|n| n.properties.iter().find(|p| p.label == ec.id))
                    .cloned();
                if let (Some(prop), Some(n)) =
                    (restored, plan.nodes.iter_mut().find(|n| n.id == owner))
                {
                    n.properties.push(prop);
                }
            }
        }
        ElementKind::Link => {
            plan.links.retain(|l| l.id != ec.id);
            if !added {
                if let Some(l) = committed.links.iter().find(|l| l.id == ec.id) {
                    plan.links.push(l.clone());
                }
            }
        }
        ElementKind::Group => {
            plan.groups.retain(|g| g.id != ec.id);
            if !added {
                if let Some(g) = committed.groups.iter().find(|g| g.id == ec.id) {
                    plan.groups.push(g.clone());
                }
            }
        }
        ElementKind::Node => {
            if added {
                plan.nodes.retain(|n| n.id != ec.id);
                plan.links.retain(|l| l.src != ec.id && l.dst != ec.id);
                for g in &mut plan.groups {
                    g.member_ids.retain(|m| m != &ec.id);
                }
                plan.boundaries.remove(&ec.id);
            } else if let Some(c) = committed.nodes.iter().find(|n| n.id == ec.id) {
                // The node's OWN fields only. Its claims and properties are
                // their own keys and may be another change's work.
                if let Some(n) = plan.nodes.iter_mut().find(|n| n.id == ec.id) {
                    n.kind = c.kind;
                    n.name = c.name.clone();
                    n.parent_id = c.parent_id.clone();
                    n.external = c.external;
                    n.technology = c.technology.clone();
                    n.description = c.description.clone();
                    n.directives = c.directives.clone();
                    n.icon = c.icon.clone();
                    n.notes = c.notes.clone();
                    n.position = c.position;
                } else {
                    plan.nodes.push(c.clone());
                }
            }
        }
    }
}

/// Close a change that STILL CARRIES planned entries, dropping them with it —
/// the caller having said so explicitly. Every entry the change owns is taken
/// back to what committed says ([`revert_one`]), the tags go, the registry entry
/// goes, and the close is recorded in history as an abandonment naming what was
/// dropped.
///
/// This is the deliberate counterpart to [`close_change`], which refuses exactly
/// this case: the plan is somebody's authored intent, so discarding it is an act
/// a caller asks for by name, never a fallback. Refused if dropping an ADDED
/// node would strand plan children this change does not own — a dangling parent
/// is worse than a refusal. The caller must hold the model lock.
///
/// `why` is the caller's reason for dropping the work, recorded beside the
/// change's own rationale: the rationale leaves the ledger with the change, and
/// "it was abandoned" with no reason is the one shape of this a reader cannot
/// make sense of afterwards.
pub fn abandon_change(
    r: &ModelRef,
    change_id: &str,
    why: Option<&str>,
) -> Result<Abandoned, String> {
    let committed = crate::read_model_at(r)?;
    let mut plan = crate::read_planned_seeded_at(r)?;
    let Some(pos) = plan.changes.iter().position(|c| c.id == change_id) else {
        return Err(format!("no open change '{change_id}'"));
    };

    let keys: HashSet<String> = plan
        .change_map
        .iter()
        .filter(|(_, v)| v.as_str() == change_id)
        .map(|(k, _)| k.clone())
        .collect();

    let d = diff(&committed, &plan);
    let mine: Vec<ElementChange> = d
        .changes
        .into_iter()
        .filter(|c| keys.contains(&key_for(c)))
        .collect();

    for ec in &mine {
        if ec.kind == ElementKind::Node && is_added(ec) {
            let stranded: Vec<String> = plan
                .nodes
                .iter()
                .filter(|n| n.parent_id.as_deref() == Some(ec.id.as_str()))
                .filter(|n| !keys.contains(&element_key(ElementKind::Node, None, &n.id)))
                .map(|n| n.name.clone())
                .collect();
            if !stranded.is_empty() {
                return Err(format!(
                    "{change_id} adds '{}', and {} under it {} not this change's to drop: {}. \
                     Refile or fold those first.",
                    ec.label,
                    stranded.len(),
                    if stranded.len() == 1 { "is" } else { "are" },
                    stranded.join(", ")
                ));
            }
        }
    }

    let mut dropped: Vec<DroppedEntry> = Vec::new();
    for ec in &mine {
        revert_one(&mut plan, &committed, ec);
        dropped.push(DroppedEntry {
            key: key_for(ec),
            label: ec.label.clone(),
            what: what_of(ec).to_string(),
        });
    }

    plan.change_map.retain(|k, _| !keys.contains(k));
    let meta = plan.changes.remove(pos);
    crate::write_planned_at(r, &plan)?;
    record_abandoned(r, &meta, &dropped, why);
    Ok(Abandoned { meta, dropped })
}

/// The history record of a change being OPENED — the first event of its life,
/// in the same stream as its sign-off and its close, so a change's whole life is
/// one record rather than a ledger entry that takes its beginning with it when
/// it closes. Stamped at the change's OWN moment ([`ChangeMeta::created_at`]),
/// not at the write, so the interval between this and the fold is the change's
/// real age.
///
/// Deliberately a [`EventKind::Change`] event with `driver` "opened", beside
/// "signed off", "folded" and "abandoned" — NOT a new event kind. `read_history`
/// drops a line it cannot parse, so a reader built before this existed would
/// silently lose an unknown kind; a new driver word is a string it already
/// displays.
///
/// Best-effort like every history append: a log failure must never abort the
/// open it describes.
pub fn record_opened(r: &ModelRef, meta: &ChangeMeta, actor: Option<&str>, person: Option<&str>) {
    let ev = HistoryEvent::new(meta.created_at, EventKind::Change, "", "opened")
        .with_change(&meta.id)
        .with_change_title(title_of(meta))
        .with_rows(vec![EventRow::new("+", meta.rationale.clone())])
        .by_actor(actor)
        .for_person(person);
    let _ = append_event(r, &ev);
}

/// The changes `after` holds that `before` did not — what a plan write OPENED.
/// The mirror of [`gc`]'s closed set, and read the same way: on the authoring
/// path, so no tool has to remember to announce its own open.
pub fn opened_by(before: &ScryModel, after: &ScryModel) -> Vec<ChangeMeta> {
    let known: HashSet<&str> = before.changes.iter().map(|c| c.id.as_str()).collect();
    after
        .changes
        .iter()
        .filter(|c| !known.contains(c.id.as_str()))
        .cloned()
        .collect()
}

/// The history record of an abandonment: the change, its title and rationale,
/// a row per entry that went with it, and the caller's reason when one was
/// given — so "what did this change hold when it was dropped, and why?" has an
/// answer after the registry entry is gone.
fn record_abandoned(r: &ModelRef, meta: &ChangeMeta, dropped: &[DroppedEntry], why: Option<&str>) {
    let mut rows = vec![EventRow::new("✓", meta.rationale.clone())];
    // The reason rides as a said-thing beside the rationale, labelled in its
    // own text: two different facts, and a reader has to be able to tell the
    // change's purpose from the reason it was dropped.
    if let Some(why) = why.map(str::trim).filter(|w| !w.is_empty()) {
        rows.push(EventRow::new("✓", format!("why: {why}")));
    }
    for d in dropped {
        rows.push(EventRow::new("−", format!("{} ({})", d.label, d.what)));
    }
    let ev = HistoryEvent::new(now_secs(), EventKind::Change, "", "abandoned")
        .with_change(&meta.id)
        .with_change_title(title_of(meta))
        .with_rows(rows);
    let _ = append_event(r, &ev);
}

/// Append a closed change's durable record to the history log — the rationale
/// finally survives the fold ("which change introduced this claim?" has an
/// answer). `driver` says how it closed: "folded" (its entries reached
/// committed) or "abandoned" (they were reverted). Best-effort like every
/// history append: a log failure must never abort the model operation.
/// Append a sign-off's durable record to the history log: an approval is a
/// decision, and a decision with no trace is one nobody can audit after the
/// change closes and takes its snapshot with it.
///
/// Names WHO signed, and — when the signature was given as someone's proxy —
/// who FOR. Both, because the two cases are different facts: an agent a host
/// runs for a developer signing on their say-so is not the developer signing,
/// and a reader who cannot tell them apart cannot tell whether a person ever
/// looked. Best-effort like every history append: a log failure must never
/// abort the sign-off it describes.
pub fn record_signed_off(r: &ModelRef, meta: &ChangeMeta) {
    let Some(snap) = meta.signed_off.as_ref() else {
        return;
    };
    let ev = HistoryEvent::new(snap.at, EventKind::Change, "", "signed off")
        .with_change(&meta.id)
        .with_rows(vec![EventRow::new("✓", meta.rationale.clone())])
        .by_actor(snap.by.as_deref())
        .for_person(snap.on_behalf_of.as_deref());
    let _ = append_event(r, &ev);
}

pub fn record_closed(r: &ModelRef, meta: &ChangeMeta, driver: &str) {
    let ev = HistoryEvent::new(now_secs(), EventKind::Change, "", driver)
        .with_change(&meta.id)
        .with_change_title(title_of(meta))
        .with_rows(vec![EventRow::new("✓", meta.rationale.clone())]);
    let _ = append_event(r, &ev);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::read_history;
    use crate::{
        commit_element, fold_built_model, read_model_at, read_planned_at, write_model_at,
        write_planned_at,
    };
    use tempfile::tempdir;

    /// A change signed off by a NAMED actor records who beside the snapshot;
    /// one signed with no actor stays unattributed rather than being refused,
    /// and a later unattributed re-stamp (a canvas save) never erases the
    /// signature. A snapshot written before the field existed still loads.
    #[test]
    fn sign_off_records_the_actor_who_gave_the_go_ahead() {
        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new")]);
        let cid = open_change(&mut plan, "the change", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &cid,
        );

        sign_off_as(&mut plan, &cid, 200, Some("morgan")).unwrap();
        assert_eq!(
            plan.changes[0].signed_off.as_ref().unwrap().by.as_deref(),
            Some("morgan")
        );

        // A canvas save re-stamps with no actor: the signature survives.
        restamp_signoffs(&mut plan, 300);
        let snap = plan.changes[0].signed_off.as_ref().unwrap();
        assert_eq!(snap.at, 300);
        assert_eq!(
            snap.by.as_deref(),
            Some("morgan"),
            "a re-stamp never erases who signed"
        );

        // Unattributed sign-off: recorded, never refused.
        let mut plain = model_with_resps(&[("r1", "exists")]);
        let cid = open_change(&mut plain, "unsigned by anyone", 100);
        sign_off(&mut plain, &cid, 200).unwrap();
        assert!(plain.changes[0].signed_off.as_ref().unwrap().by.is_none());

        // Upstream's shape (no `by`) still loads.
        let legacy: SignOff = serde_json::from_str(r#"{"at":1,"entries":{}}"#).unwrap();
        assert!(legacy.by.is_none());
    }

    /// resp-31qtx9 — the project's countersigned-fold policy is carried WITH
    /// THE MODEL and defaults to off: a model with no `policy` (upstream's,
    /// and a solo user's) reads false and serializes no key at all, and a
    /// project that opts in round-trips through the committed layer, which
    /// strips the ledger but must never strip this.
    #[test]
    fn resp_31qtx9_the_countersigned_fold_policy_is_carried_with_the_model_and_defaults_off() {
        let dir = tempdir().unwrap();
        let r = crate::ModelRef::ProjectLocal(dir.path().to_path_buf());

        // Off is the ABSENCE of the field: nothing to opt out of.
        let mut model = model_with_resps(&[("r1", "exists")]);
        assert!(model.policy.is_none());
        assert!(!requires_countersigned_folds(&model), "no policy = off");
        write_model_at(&r, &model).unwrap();
        let raw = std::fs::read_to_string(r.model_path()).unwrap();
        assert!(
            !raw.contains("policy"),
            "an unset policy leaves no trace in the file: {raw}"
        );
        assert!(!requires_countersigned_folds(&read_model_at(&r).unwrap()));

        // Opting in survives the committed write that strips change state.
        model.policy = Some(Policy {
            require_change_titles: false,
            require_countersigned_folds: true,
        });
        let cid = open_change(&mut model, "not the committed layer's business", 100);
        tag(
            &mut model,
            &[element_key(ElementKind::Responsibility, None, "r1")],
            &cid,
        );
        write_model_at(&r, &model).unwrap();
        let back = read_model_at(&r).unwrap();
        assert!(back.changes.is_empty(), "the ledger is still stripped");
        assert!(requires_countersigned_folds(&back), "the policy is not");
        assert!(std::fs::read_to_string(r.model_path())
            .unwrap()
            .contains("\"requireCountersignedFolds\": true"));

        // Turning it back off is the absence again, not `false` on disk.
        model.policy = Some(Policy::default());
        write_model_at(&r, &model).unwrap();
        assert!(!requires_countersigned_folds(&read_model_at(&r).unwrap()));
        assert!(!std::fs::read_to_string(r.model_path())
            .unwrap()
            .contains("requireCountersigned"));

        // Upstream's shape (no `policy` key) still loads.
        let legacy: ScryModel =
            serde_json::from_str(r#"{"version":"1","nodes":[],"links":[]}"#).unwrap();
        assert!(!requires_countersigned_folds(&legacy));
    }

    /// resp-7xts3y — a sign-off leaves its own record on the timeline, naming
    /// the actor who signed and, when they signed as someone's proxy, the
    /// person it was for. Both names or the record lies: with only `by` an
    /// agent's proxy signature reads as the agent's own opinion, and with only
    /// the person it reads as though they looked at it themselves.
    ///
    /// It has to be its own event. A plan write records what the plan CLAIMS,
    /// and a sign-off changes no claim — so the approval would otherwise leave
    /// no trace at all, and the snapshot that holds it goes with the change
    /// when the change closes.
    #[test]
    fn resp_7xts3y_a_sign_off_records_who_signed_and_who_for() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        let signed_off = |r: &ModelRef| -> Vec<crate::history::HistoryEvent> {
            read_history(r)
                .into_iter()
                .filter(|e| e.driver == "signed off")
                .collect()
        };

        let mut plan = model_with_resps(&[("r1", "exists")]);
        let cid = open_change(&mut plan, "the rationale that outlives the change", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r1")],
            &cid,
        );

        // A host's agent signs for the developer.
        sign_off_for(
            &mut plan,
            &cid,
            200,
            Some("agent-session-7"),
            Some("morgan"),
        )
        .unwrap();
        record_signed_off(&r, &plan.changes[0]);
        let log = signed_off(&r);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].by, "agent-session-7", "the signer");
        assert_eq!(
            log[0].on_behalf_of.as_deref(),
            Some("morgan"),
            "the person it was for"
        );
        assert_eq!(log[0].change_id.as_deref(), Some(cid.as_str()));
        assert_eq!(
            log[0].at, 200,
            "stamped when the signature was, not when it was logged"
        );
        assert_eq!(
            log[0].rows[0].text,
            "the rationale that outlives the change"
        );

        // A direct sign-off is nobody's proxy, and says so by saying nothing.
        let mut direct = model_with_resps(&[("r1", "exists")]);
        let dir2 = tempdir().unwrap();
        let r2 = ModelRef::ProjectLocal(dir2.path().to_path_buf());
        let cid2 = open_change(&mut direct, "signed by the developer themselves", 100);
        tag(
            &mut direct,
            &[element_key(ElementKind::Responsibility, None, "r1")],
            &cid2,
        );
        sign_off_as(&mut direct, &cid2, 200, Some("morgan")).unwrap();
        record_signed_off(&r2, &direct.changes[0]);
        let log = signed_off(&r2);
        assert_eq!(log[0].by, "morgan");
        assert!(
            log[0].on_behalf_of.is_none(),
            "not a proxy, so there is nobody to name"
        );
        let raw = std::fs::read_to_string(r2.history_path()).unwrap();
        assert!(
            !raw.contains("onBehalfOf"),
            "a direct sign-off writes no key at all: {raw}"
        );

        // A change nobody signed records nothing — there is no decision yet.
        let dir3 = tempdir().unwrap();
        let r3 = ModelRef::ProjectLocal(dir3.path().to_path_buf());
        let mut unsigned = model_with_resps(&[("r1", "exists")]);
        let _ = open_change(&mut unsigned, "not approved yet", 100);
        record_signed_off(&r3, &unsigned.changes[0]);
        assert!(signed_off(&r3).is_empty());

        // An event written before the field existed still loads.
        let legacy: crate::history::HistoryEvent =
            serde_json::from_str(r#"{"at":1,"driver":"signed off","kind":"change","nodeId":""}"#)
                .unwrap();
        assert!(legacy.on_behalf_of.is_none());
    }

    /// resp-pt49rq — the staleness names the hand that caused it. A signer
    /// told only "your approval is out of date" has to go looking; told "sam's
    /// save moved it" they know who to ask, and the re-request writes itself.
    ///
    /// The name IS the flag, so the two cannot disagree: there is no way to
    /// mark a signature stale without saying by whom, and clearing it clears
    /// both at once.
    #[test]
    fn resp_pt49rq_a_staled_sign_off_names_the_write_that_staled_it() {
        let mut plan = model_with_resps(&[("r1", "the sentence morgan approved")]);
        let cid = open_change(&mut plan, "the change", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r1")],
            &cid,
        );
        sign_off_as(&mut plan, &cid, 200, Some("morgan")).unwrap();
        plan.nodes[0].responsibilities[0].statement = "a sentence they never read".into();

        restamp_signoffs_as(&mut plan, 300, Some("sam"));
        let snap = plan.changes[0].signed_off.as_ref().unwrap();
        assert!(snap.is_stale());
        assert_eq!(
            snap.staled_by.as_deref(),
            Some("sam"),
            "the hand that moved the plan"
        );
        assert_eq!(
            snap.by.as_deref(),
            Some("morgan"),
            "still morgan's approval, not sam's"
        );

        // The last hand to move it is the one named: a signer chasing this
        // wants whoever wrote most recently, not whoever wrote first.
        restamp_signoffs_as(&mut plan, 400, Some("bea"));
        assert_eq!(
            plan.changes[0]
                .signed_off
                .as_ref()
                .unwrap()
                .staled_by
                .as_deref(),
            Some("bea")
        );

        // Signing again clears the fact and the name together — one field, so
        // there is no way to leave a name behind on a fresh approval.
        sign_off_as(&mut plan, &cid, 500, Some("morgan")).unwrap();
        let snap = plan.changes[0].signed_off.as_ref().unwrap();
        assert!(!snap.is_stale());
        assert!(snap.staled_by.is_none());

        // And a project nobody staled writes no key at all, as before.
        let raw = serde_json::to_string(&plan).unwrap();
        assert!(!raw.contains("staledBy"), "{raw}");
    }

    /// resp-gc5m1s — a plan write re-stamps the signature it belongs to and
    /// stales the ones it does not. The signer's own edit is intent, exactly
    /// as before; a colleague's edit is a plan the signer has not seen, so the
    /// snapshot stays where their approval left it and the signature is
    /// flagged for a re-request. It takes TWO named, different identities:
    /// unattributed writes, and signatures nobody signed by name, re-stamp as
    /// they always have, which is every solo user and the desktop canvas.
    #[test]
    fn resp_gc5m1s_a_plan_write_by_someone_other_than_the_signer_stales_the_sign_off() {
        let approved = "the sentence morgan approved";
        let signed_plan = || {
            let mut plan = model_with_resps(&[("r1", approved)]);
            let cid = open_change(&mut plan, "the change", 100);
            tag(
                &mut plan,
                &[element_key(ElementKind::Responsibility, None, "r1")],
                &cid,
            );
            sign_off_as(&mut plan, &cid, 200, Some("morgan")).unwrap();
            (plan, cid)
        };
        let reworded = |plan: &mut ScryModel| {
            plan.nodes[0].responsibilities[0].statement = "a sentence they never read".into();
        };

        // The signer's own save: re-stamped, as it always was.
        let (mut mine, cid) = signed_plan();
        reworded(&mut mine);
        let out = restamp_signoffs_as(&mut mine, 300, Some("morgan"));
        assert_eq!(out.restamped, vec![cid.clone()]);
        assert!(out.staled.is_empty());
        let snap = mine.changes[0].signed_off.as_ref().unwrap();
        assert_eq!(snap.at, 300, "the snapshot followed the edit");
        assert!(!snap.is_stale(), "their own edit is intent, not a surprise");
        assert!(
            classify_against_signoff(&mine, &mine.changes[0]).is_empty(),
            "so nothing reads as an amendment at the next fold"
        );

        // A colleague's save: the approval stays where it was, and says so.
        let (mut theirs, cid) = signed_plan();
        reworded(&mut theirs);
        let out = restamp_signoffs_as(&mut theirs, 300, Some("sam"));
        assert_eq!(out.staled, vec![cid.clone()]);
        assert!(out.restamped.is_empty());
        let snap = theirs.changes[0].signed_off.as_ref().unwrap();
        assert!(snap.is_stale(), "the signature is flagged for a re-request");
        assert_eq!(
            snap.staled_by.as_deref(),
            Some("sam"),
            "and says whose write did it"
        );
        assert_eq!(
            snap.at, 200,
            "and not re-dated: morgan signed then, not now"
        );
        assert_eq!(
            snap.by.as_deref(),
            Some("morgan"),
            "nor re-attributed to the writer"
        );
        assert_eq!(
            snap.entries.values().next().unwrap().statement.as_deref(),
            Some(approved),
            "the snapshot still holds what they actually approved"
        );

        // morgan looks and signs again: a named signature is a fresh approval.
        sign_off_as(&mut theirs, &cid, 400, Some("morgan")).unwrap();
        assert!(!theirs.changes[0].signed_off.as_ref().unwrap().is_stale());

        // Neither hand named: today's behaviour, untouched. An anonymous save
        // does not stale, and does not launder an existing staleness either.
        let (mut solo, _) = signed_plan();
        reworded(&mut solo);
        let out = restamp_signoffs_as(&mut solo, 300, None);
        assert_eq!(out.restamped.len(), 1);
        assert!(!solo.changes[0].signed_off.as_ref().unwrap().is_stale());
        let (mut unsigned, ucid) = {
            let mut plan = model_with_resps(&[("r1", approved)]);
            let cid = open_change(&mut plan, "nobody signed by name", 100);
            tag(
                &mut plan,
                &[element_key(ElementKind::Responsibility, None, "r1")],
                &cid,
            );
            sign_off(&mut plan, &cid, 200).unwrap();
            (plan, cid)
        };
        assert_eq!(
            restamp_signoffs_as(&mut unsigned, 300, Some("sam")).restamped,
            vec![ucid]
        );
        assert!(!unsigned.changes[0].signed_off.as_ref().unwrap().is_stale());
        restamp_signoffs_as(&mut theirs, 500, Some("sam"));
        assert!(
            theirs.changes[0].signed_off.as_ref().unwrap().is_stale(),
            "staled again"
        );
        restamp_signoffs_as(&mut theirs, 600, None);
        assert!(
            theirs.changes[0].signed_off.as_ref().unwrap().is_stale(),
            "an unattributed save cannot clear what a named one set"
        );

        // Off is the ABSENCE of the key: a plan nobody staled is byte-identical
        // to one written before the field existed, and upstream's still loads.
        let json = serde_json::to_string(&solo).unwrap();
        assert!(!json.contains("staledBy"), "{json}");
        let legacy: SignOff = serde_json::from_str(r#"{"at":1,"entries":{}}"#).unwrap();
        assert!(!legacy.is_stale());
    }

    /// resp-k4yw29 — a sign-off one actor makes FOR another person records
    /// both names: the signer in `by` (so it counts as a different signer) and
    /// the person in `onBehalfOf` (so it is never read as their own). A direct
    /// sign-off leaves `onBehalfOf` unset, and a canvas re-stamp erases
    /// neither half.
    #[test]
    fn resp_k4yw29_a_proxy_sign_off_records_the_signer_and_the_person_it_is_for() {
        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new")]);
        let cid = open_change(&mut plan, "the change", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &cid,
        );

        // The host's agent signs for the developer.
        sign_off_for(
            &mut plan,
            &cid,
            200,
            Some("agent-session-7"),
            Some("morgan"),
        )
        .unwrap();
        let snap = plan.changes[0].signed_off.clone().unwrap();
        assert_eq!(
            snap.by.as_deref(),
            Some("agent-session-7"),
            "the SIGNER is the actor"
        );
        assert_eq!(
            snap.on_behalf_of.as_deref(),
            Some("morgan"),
            "the person it is for"
        );
        assert_eq!(snap.entries.len(), 1, "it is still a real snapshot");

        // A canvas save re-stamps unattributed: both halves survive.
        restamp_signoffs(&mut plan, 300);
        let snap = plan.changes[0].signed_off.as_ref().unwrap();
        assert_eq!(snap.at, 300);
        assert_eq!(snap.by.as_deref(), Some("agent-session-7"));
        assert_eq!(
            snap.on_behalf_of.as_deref(),
            Some("morgan"),
            "a re-stamp is not a disavowal"
        );

        // A named signer owns the whole attribution: signing directly over a
        // proxy signature clears the person, it does not inherit them.
        sign_off_for(&mut plan, &cid, 400, Some("morgan"), None).unwrap();
        let snap = plan.changes[0].signed_off.as_ref().unwrap();
        assert_eq!(snap.by.as_deref(), Some("morgan"));
        assert!(
            snap.on_behalf_of.is_none(),
            "a direct sign-off is nobody's proxy"
        );

        // The plain call is a direct sign-off.
        let mut direct = model_with_resps(&[("r1", "exists")]);
        let cid = open_change(&mut direct, "direct", 100);
        sign_off_as(&mut direct, &cid, 200, Some("morgan")).unwrap();
        assert!(direct.changes[0]
            .signed_off
            .as_ref()
            .unwrap()
            .on_behalf_of
            .is_none());

        // Upstream's shape (no `onBehalfOf`) still loads.
        let legacy: SignOff = serde_json::from_str(r#"{"at":1,"by":"morgan"}"#).unwrap();
        assert!(legacy.on_behalf_of.is_none());
    }

    /// A model whose single component `n1` carries the given responsibilities.
    fn model_with_resps(resps: &[(&str, &str)]) -> ScryModel {
        let resps: Vec<_> = resps
            .iter()
            .map(|(id, s)| serde_json::json!({ "id": id, "statement": s }))
            .collect();
        serde_json::from_value(serde_json::json!({
            "version": crate::SCRY_VERSION,
            "nodes": [{ "id": "n1", "kind": "component", "name": "C", "responsibilities": resps }],
            "links": [],
        }))
        .unwrap()
    }

    /// A title is a short name a reader uses; the rationale is what was said.
    /// A change with no title of its own still reads — by the rationale's first
    /// line — so no surface has to decide what to draw for one opened before the
    /// field existed.
    #[test]
    fn a_change_reads_by_its_title_or_its_rationales_first_line() {
        let mut plan = model_with_resps(&[("r1", "exists")]);

        let titled = open_change_titled(
            &mut plan,
            Some("  Give a change a title  "),
            "the long why",
            100,
        )
        .unwrap();
        let untitled = open_change(&mut plan, "first line\nsecond line", 200);

        fn by_id(plan: &ScryModel, id: &str) -> ChangeMeta {
            plan.changes.iter().find(|c| c.id == id).unwrap().clone()
        }
        assert_eq!(
            by_id(&plan, &titled).title.as_deref(),
            Some("Give a change a title")
        );
        assert_eq!(title_of(&by_id(&plan, &titled)), "Give a change a title");
        assert_eq!(by_id(&plan, &untitled).title, None);
        assert_eq!(title_of(&by_id(&plan, &untitled)), "first line");

        // A change opened before the field existed loads with no title and reads
        // the same way — the field is absent from the file, not null.
        let json = serde_json::to_string(&by_id(&plan, &untitled)).unwrap();
        assert!(!json.contains("title"), "{json}");

        // Naming it afterwards is how it stops reading by its rationale.
        set_title(&mut plan, &untitled, "Named later").unwrap();
        assert_eq!(title_of(&by_id(&plan, &untitled)), "Named later");
        assert_eq!(
            set_title(&mut plan, "chg-nope", "x"),
            Err("no open change 'chg-nope'".to_string())
        );
    }

    /// Length is always checked — 80 CHARACTERS, not bytes, so the limit means
    /// the same thing in every script. Whether an ABSENT title is refused is the
    /// project's policy, and a model carrying no policy never meets the gate.
    #[test]
    fn a_title_is_refused_when_too_long_and_when_absent_under_the_policy() {
        let mut plan = model_with_resps(&[("r1", "exists")]);

        let eighty = "x".repeat(MAX_TITLE_LEN);
        assert!(open_change_titled(&mut plan, Some(&eighty), "why", 100).is_ok());
        let over = "x".repeat(MAX_TITLE_LEN + 1);
        let err = open_change_titled(&mut plan, Some(&over), "why", 100).unwrap_err();
        assert!(err.contains("81 characters"), "{err}");

        // Multi-byte: 80 characters is 80 characters, whatever they weigh.
        let eighty_wide = "é".repeat(MAX_TITLE_LEN);
        assert_eq!(eighty_wide.len(), MAX_TITLE_LEN * 2);
        assert!(open_change_titled(&mut plan, Some(&eighty_wide), "why", 100).is_ok());

        // No policy: an untitled change opens, as it always has.
        assert!(!requires_change_titles(&plan));
        assert!(open_change_titled(&mut plan, None, "why", 100).is_ok());

        // The project asks for the check: absent is refused, present is not.
        plan.policy = Some(Policy {
            require_countersigned_folds: false,
            require_change_titles: true,
        });
        let err = open_change_titled(&mut plan, None, "why", 100).unwrap_err();
        assert!(err.contains("requires a change to have a title"), "{err}");
        let err = open_change_titled(&mut plan, Some("   "), "why", 100).unwrap_err();
        assert!(err.contains("requires a change to have a title"), "{err}");
        assert!(open_change_titled(&mut plan, Some("a name"), "why", 100).is_ok());
    }

    /// A change closed with its planned work still in it: every entry goes back
    /// to what committed says — an ADD removed, a REWORD restored — the tags and
    /// the registry entry go, and the history says what was dropped. The other
    /// change's entry is untouched.
    #[test]
    fn abandoning_a_change_drops_its_entries_and_records_what_went() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "as committed")])).unwrap();

        let mut plan =
            model_with_resps(&[("r1", "reworded by the doomed change"), ("r2", "added")]);
        let doomed = open_change_titled(&mut plan, Some("Doomed"), "why it existed", 100).unwrap();
        let other = open_change(&mut plan, "a neighbour", 110);
        tag(
            &mut plan,
            &[
                element_key(ElementKind::Responsibility, None, "r1"),
                element_key(ElementKind::Responsibility, None, "r2"),
            ],
            &doomed,
        );
        plan.nodes[0].responsibilities.push(
            serde_json::from_value(serde_json::json!({"id": "r3", "statement": "the neighbour's"}))
                .unwrap(),
        );
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r3")],
            &other,
        );
        write_planned_at(&r, &plan).unwrap();

        let abandoned = abandon_change(&r, &doomed, None).unwrap();

        assert_eq!(abandoned.meta.id, doomed);
        let mut what: Vec<(String, String)> = abandoned
            .dropped
            .iter()
            .map(|d| (d.key.clone(), d.what.clone()))
            .collect();
        what.sort();
        assert_eq!(
            what,
            vec![
                ("resp:r1".to_string(), "reworded".to_string()),
                ("resp:r2".to_string(), "added".to_string()),
            ]
        );

        let after = read_planned_at(&r).unwrap();
        let resp = |id: &str| {
            after.nodes[0]
                .responsibilities
                .iter()
                .find(|x| x.id == id)
                .map(|x| x.statement.clone())
        };
        assert_eq!(resp("r1").as_deref(), Some("as committed"), "reword undone");
        assert_eq!(resp("r2"), None, "the added claim went with the change");
        assert_eq!(
            resp("r3").as_deref(),
            Some("the neighbour's"),
            "another change's entry is not this one's to drop"
        );
        assert!(after.changes.iter().all(|c| c.id != doomed));
        assert!(after.changes.iter().any(|c| c.id == other));
        assert!(!after.change_map.contains_key("resp:r1"));
        assert!(after.change_map.contains_key("resp:r3"));

        let ev = read_history(&r)
            .into_iter()
            .find(|e| {
                e.kind == EventKind::Change
                    && e.driver == "abandoned"
                    && e.change_id.as_deref() == Some(doomed.as_str())
            })
            .expect("the abandonment is in the history");
        assert_eq!(ev.driver, "abandoned");
        assert_eq!(ev.change_title.as_deref(), Some("Doomed"));
        assert_eq!(ev.rows[0].text, "why it existed");
        let dropped_rows: Vec<&str> = ev.rows[1..].iter().map(|x| x.text.as_str()).collect();
        assert_eq!(dropped_rows.len(), 2, "{dropped_rows:?}");
        assert!(
            dropped_rows.iter().any(|t| t.contains("(added)"))
                && dropped_rows.iter().any(|t| t.contains("(reworded)")),
            "{dropped_rows:?}"
        );
    }

    /// Abandonment refuses rather than orphans: a node this change ADDED, with a
    /// child under it that belongs to somebody else, would leave that child
    /// hanging off a dead parent.
    #[test]
    fn abandoning_refuses_to_strand_another_changes_node() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();

        let mut plan = model_with_resps(&[("r1", "exists")]);
        plan.nodes.push(
            serde_json::from_value(serde_json::json!({
                "id": "n2", "kind": "component", "name": "Parent", "responsibilities": []
            }))
            .unwrap(),
        );
        plan.nodes.push(
            serde_json::from_value(serde_json::json!({
                "id": "n3", "kind": "component", "name": "Child",
                "parentId": "n2", "responsibilities": []
            }))
            .unwrap(),
        );
        let doomed = open_change(&mut plan, "adds the parent", 100);
        let other = open_change(&mut plan, "adds the child", 110);
        tag(
            &mut plan,
            &[element_key(ElementKind::Node, None, "n2")],
            &doomed,
        );
        tag(
            &mut plan,
            &[element_key(ElementKind::Node, None, "n3")],
            &other,
        );
        write_planned_at(&r, &plan).unwrap();

        let err = abandon_change(&r, &doomed, None).unwrap_err();
        assert!(err.contains("Child"), "{err}");
        assert!(err.contains("Refile or fold"), "{err}");

        // Nothing moved: a refusal leaves the plan exactly as it was.
        let after = read_planned_at(&r).unwrap();
        assert!(after.nodes.iter().any(|n| n.id == "n2"));
        assert!(after.changes.iter().any(|c| c.id == doomed));
        assert!(after.changes.iter().any(|c| c.id == other));
    }

    /// The empty-change close is unchanged, and still refuses a change with
    /// entries — abandonment is the deliberate other door, never a fallback.
    #[test]
    fn closing_still_refuses_a_change_that_carries_entries() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();

        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "added")]);
        let cid = open_change(&mut plan, "has work", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &cid,
        );
        write_planned_at(&r, &plan).unwrap();

        let err = close_change(&r, &cid).unwrap_err();
        assert!(err.contains("still has 1 tagged entry"), "{err}");
        assert!(read_planned_at(&r)
            .unwrap()
            .changes
            .iter()
            .any(|c| c.id == cid));
    }

    /// A change's whole life is one stream: the `opened` event lands the moment
    /// it is opened, naming it, its rationale, its actor and the person acted
    /// for, and the fold's close lands in the same stream — so the interval
    /// between them is a fact the history holds.
    #[test]
    fn opening_a_change_is_recorded_beside_its_close() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();
        write_planned_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();

        let mut plan = read_planned_at(&r).unwrap();
        let cid = open_change_titled(&mut plan, Some("The title"), "why it exists", 1_700_000_000)
            .unwrap();
        crate::write_planned_for(&r, &plan, Some("the-agent"), Some("morgan")).unwrap();

        let opened: Vec<_> = read_history(&r)
            .into_iter()
            .filter(|e| e.kind == EventKind::Change && e.driver == "opened")
            .collect();
        assert_eq!(opened.len(), 1, "exactly one open, once");
        let ev = &opened[0];
        assert_eq!(ev.change_id.as_deref(), Some(cid.as_str()));
        assert_eq!(ev.change_title.as_deref(), Some("The title"));
        assert_eq!(ev.rows[0].text, "why it exists");
        assert_eq!(ev.by, "the-agent");
        assert_eq!(ev.on_behalf_of.as_deref(), Some("morgan"));
        assert_eq!(
            ev.at, 1_700_000_000,
            "stamped at the change's own moment, not at the write"
        );

        // A second write that opens nothing announces nothing.
        let plan = read_planned_at(&r).unwrap();
        crate::write_planned_for(&r, &plan, None, None).unwrap();
        assert_eq!(
            read_history(&r)
                .iter()
                .filter(|e| e.driver == "opened")
                .count(),
            1,
            "an open is announced once, not on every later write"
        );

        // And the close lands in the same stream, so both ends are there.
        close_change(&r, &cid).unwrap();
        let life: Vec<String> = read_history(&r)
            .into_iter()
            .filter(|e| e.kind == EventKind::Change && e.change_id.as_deref() == Some(cid.as_str()))
            .map(|e| e.driver)
            .collect();
        assert_eq!(life, vec!["opened".to_string(), "abandoned".to_string()]);
    }

    /// The event rides the AUTHORING path, so a caller that opens a change
    /// without going through a tool still records one — and `opened_by` names
    /// exactly what a write added, never what it merely carried.
    #[test]
    fn what_a_write_opened_is_read_from_the_write_itself() {
        let mut before = model_with_resps(&[("r1", "exists")]);
        let first = open_change(&mut before, "already open", 100);

        let mut after = before.clone();
        let second = open_change(&mut after, "newly open", 200);

        let opened = opened_by(&before, &after);
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].id, second);
        assert!(opened_by(&before, &before).is_empty());
        assert_eq!(
            opened_by(&ScryModel::new(), &after).len(),
            2,
            "against an empty plan, both changes read as opened"
        );
        let _ = first;
    }

    /// The event is a `Change` event with a new DRIVER, not a new event kind: a
    /// reader built before it existed parses the line (`read_history` silently
    /// DROPS what it cannot parse, so an unknown kind would go missing without
    /// a word), and finds a driver string it already knows how to show.
    #[test]
    fn an_older_reader_can_still_parse_an_opened_event() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        let meta = ChangeMeta {
            id: "chg-1".into(),
            rationale: "why".into(),
            title: Some("A name".into()),
            created_at: 1_700_000_000,
            signed_off: None,
        };
        record_opened(&r, &meta, None, None);

        let line = std::fs::read_to_string(r.history_path()).unwrap();
        let raw: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(raw["kind"], "change", "an existing kind, not a new one");
        assert_eq!(raw["driver"], "opened");

        // What an older reader does: parse into the shape it knows. `changeTitle`
        // is ignored by a reader that predates it; the event still lands.
        #[derive(serde::Deserialize)]
        struct OldEvent {
            kind: EventKind,
            driver: String,
            #[serde(rename = "changeId")]
            change_id: Option<String>,
        }
        let old: OldEvent = serde_json::from_str(line.trim()).expect("an older reader parses it");
        assert_eq!(old.kind, EventKind::Change);
        assert_eq!(old.driver, "opened");
        assert_eq!(old.change_id.as_deref(), Some("chg-1"));
    }

    /// The full lifecycle: two changes tag pending claims; folding one claim
    /// closes its change (recorded "folded", rationale intact) while the other
    /// stays open; the committed layer never carries change state.
    #[test]
    fn fold_closes_the_emptied_change_and_records_its_rationale() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();

        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new A"), ("r3", "new B")]);
        let a = open_change(&mut plan, "track vagrant properties too", 100);
        let b = open_change(&mut plan, "second workstream", 200);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &a,
        );
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r3")],
            &b,
        );
        write_planned_at(&r, &plan).unwrap();

        commit_element(&r, ElementKind::Responsibility, None, "r2").unwrap();

        let planned = read_planned_at(&r).unwrap();
        assert_eq!(
            planned.changes.len(),
            1,
            "the emptied change left the registry"
        );
        assert_eq!(planned.changes[0].id, b);
        assert_eq!(
            planned.change_map.keys().collect::<Vec<_>>(),
            vec![&element_key(ElementKind::Responsibility, None, "r3")]
        );
        let committed = read_model_at(&r).unwrap();
        assert!(committed.changes.is_empty() && committed.change_map.is_empty());

        let closes: Vec<_> = read_history(&r)
            .into_iter()
            // A change now has TWO ends in this stream; this counts the closes.
            .filter(|e| e.kind == EventKind::Change && e.driver != "opened")
            .collect();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].change_id.as_deref(), Some(a.as_str()));
        assert_eq!(closes[0].driver, "folded");
        assert_eq!(closes[0].rows[0].text, "track vagrant properties too");
    }

    /// Reverting a tagged element on the authoring path kills its pending
    /// entry, so the change closes as "abandoned" — while a freshly opened
    /// change with no tags yet survives every write untouched.
    #[test]
    fn revert_abandons_the_change_but_an_untagged_open_change_survives() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();

        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new A")]);
        let tagged = open_change(&mut plan, "doomed", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &tagged,
        );
        let fresh = open_change(&mut plan, "not yet written", 150);
        write_planned_at(&r, &plan).unwrap();

        // Revert r2: the next write carries the map entry but no divergence.
        let mut reverted = read_planned_at(&r).unwrap();
        for n in &mut reverted.nodes {
            n.responsibilities.retain(|x| x.id != "r2");
        }
        write_planned_at(&r, &reverted).unwrap();

        let planned = read_planned_at(&r).unwrap();
        assert_eq!(planned.changes.len(), 1);
        assert_eq!(
            planned.changes[0].id, fresh,
            "the never-tagged change is not GC bait"
        );
        assert!(planned.change_map.is_empty());
        let closes: Vec<_> = read_history(&r)
            .into_iter()
            // A change now has TWO ends in this stream; this counts the closes.
            .filter(|e| e.kind == EventKind::Change && e.driver != "opened")
            .collect();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].change_id.as_deref(), Some(tagged.as_str()));
        assert_eq!(closes[0].driver, "abandoned");
    }

    /// A whole-node fold carries only its own change's claims: a claim tagged
    /// to a DIFFERENT change stays pending in the plan (it is another task's
    /// work), so folding change A can never silently complete change B.
    #[test]
    fn whole_node_fold_leaves_foreign_tagged_claims_in_the_plan() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        let committed: ScryModel = serde_json::from_value(serde_json::json!({
            "version": crate::SCRY_VERSION,
            "nodes": [{ "id": "n1", "kind": "system", "name": "S" }],
            "links": [],
        }))
        .unwrap();
        write_model_at(&r, &committed).unwrap();

        let mut plan: ScryModel = serde_json::from_value(serde_json::json!({
            "version": crate::SCRY_VERSION,
            "nodes": [
                { "id": "n1", "kind": "system", "name": "S" },
                { "id": "n2", "kind": "container", "name": "C", "parentId": "n1",
                  "responsibilities": [
                      { "id": "r2", "statement": "task A's claim" },
                      { "id": "r3", "statement": "task B's claim" },
                  ] },
            ],
            "links": [],
        }))
        .unwrap();
        let a = open_change(&mut plan, "task A", 100);
        let b = open_change(&mut plan, "task B", 200);
        tag(
            &mut plan,
            &[
                element_key(ElementKind::Node, None, "n2"),
                element_key(ElementKind::Responsibility, None, "r2"),
            ],
            &a,
        );
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r3")],
            &b,
        );
        write_planned_at(&r, &plan).unwrap();

        commit_element(&r, ElementKind::Node, None, "n2").unwrap();

        let committed = read_model_at(&r).unwrap();
        let n2 = committed.nodes.iter().find(|n| n.id == "n2").unwrap();
        assert!(n2.responsibilities.iter().any(|x| x.id == "r2"));
        assert!(
            !n2.responsibilities.iter().any(|x| x.id == "r3"),
            "task B's claim must not ride task A's fold"
        );
        let planned = read_planned_at(&r).unwrap();
        assert_eq!(planned.changes.len(), 1, "task B stays open");
        assert_eq!(planned.changes[0].id, b);
        assert_eq!(
            planned.change_map.keys().collect::<Vec<_>>(),
            vec![&element_key(ElementKind::Responsibility, None, "r3")]
        );
    }

    /// A whole-build fold closes every open change and re-seeds both layers
    /// clean of change state.
    #[test]
    fn build_fold_closes_all_changes_and_strips_both_layers() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();

        let mut built = model_with_resps(&[("r1", "exists"), ("r2", "built")]);
        let id = open_change(&mut built, "the build task", 100);
        tag(
            &mut built,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &id,
        );

        fold_built_model(&r, &built).unwrap();

        let committed = read_model_at(&r).unwrap();
        assert!(committed.changes.is_empty() && committed.change_map.is_empty());
        let planned = read_planned_at(&r).unwrap();
        assert!(planned.changes.is_empty() && planned.change_map.is_empty());
        let closes: Vec<_> = read_history(&r)
            .into_iter()
            // A change now has TWO ends in this stream; this counts the closes.
            .filter(|e| e.kind == EventKind::Change && e.driver != "opened")
            .collect();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].change_id.as_deref(), Some(id.as_str()));
    }

    /// The hand-close escape hatch: an empty (stranded) change closes and is
    /// recorded "abandoned"; a change with tagged entries refuses (it closes
    /// through its entries); an unknown id is an error.
    #[test]
    fn close_change_discards_only_empty_ledgers() {
        let tmp = tempdir().unwrap();
        let r = ModelRef::ProjectLocal(tmp.path().to_path_buf());
        write_model_at(&r, &model_with_resps(&[("r1", "exists")])).unwrap();

        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new A")]);
        let tagged = open_change(&mut plan, "real work", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &tagged,
        );
        let stranded = open_change(&mut plan, "opened then orphaned", 200);
        write_planned_at(&r, &plan).unwrap();

        assert!(close_change(&r, &tagged)
            .unwrap_err()
            .contains("1 tagged entry"));
        assert!(close_change(&r, "chg-99")
            .unwrap_err()
            .contains("no open change"));

        let meta = close_change(&r, &stranded).unwrap();
        assert_eq!(meta.rationale, "opened then orphaned");
        let planned = read_planned_at(&r).unwrap();
        assert_eq!(
            planned.changes.len(),
            1,
            "only the tagged change remains open"
        );
        assert_eq!(planned.changes[0].id, tagged);

        let closes: Vec<_> = read_history(&r)
            .into_iter()
            // A change now has TWO ends in this stream; this counts the closes.
            .filter(|e| e.kind == EventKind::Change && e.driver != "opened")
            .collect();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].change_id.as_deref(), Some(stranded.as_str()));
        assert_eq!(closes[0].driver, "abandoned");
        assert_eq!(closes[0].rows[0].text, "opened then orphaned");
    }

    #[test]
    fn element_keys_round_trip_and_self_classify() {
        for (kind, owner, id) in [
            (ElementKind::Node, None, "node-3"),
            (ElementKind::Link, None, "link-1"),
            (ElementKind::Group, None, "grp-2"),
            (ElementKind::Responsibility, None, "resp-9"),
            (ElementKind::Property, Some("node-3"), "odooMapping"),
        ] {
            let key = element_key(kind, owner, id);
            let (k, o, i) = parse_key(&key).unwrap();
            assert_eq!(k, kind);
            assert_eq!(o.as_deref(), owner);
            assert_eq!(i, id);
        }
        // A label may itself contain the separator; the owner side never does.
        let key = element_key(ElementKind::Property, Some("node-3"), "std::vec::Vec");
        let (_, o, i) = parse_key(&key).unwrap();
        assert_eq!(o.as_deref(), Some("node-3"));
        assert_eq!(i, "std::vec::Vec");
        assert!(parse_key("bogus").is_none());
        assert!(parse_key("widget:x").is_none());
    }

    #[test]
    fn open_change_mints_a_fresh_id_every_time() {
        let mut m = ScryModel::new();
        let a = open_change(&mut m, "  first task  ", 100);
        assert!(crate::ids::is_minted_id(&a, "chg"), "{a}");
        assert_eq!(m.changes[0].rationale, "first task");
        // A tag left by a closed change is still an id the mint must avoid;
        // and no two opens ever agree, however identical the snapshot.
        m.change_map.insert("node:n1".into(), "chg-7".into());
        let b = open_change(&mut m, "second", 200);
        assert!(crate::ids::is_minted_id(&b, "chg"), "{b}");
        assert_ne!(b, a);
        assert_ne!(b, "chg-7");
        assert_eq!(m.changes.len(), 2);
    }

    #[test]
    fn tag_reports_cross_change_collisions_and_last_writer_wins() {
        let mut m = ScryModel::new();
        let keys = vec!["resp:r1".to_string(), "resp:r2".to_string()];
        assert!(tag(&mut m, &keys, "chg-1").is_empty());
        // Same change re-tagging is not a conflict.
        assert!(tag(&mut m, &keys[..1], "chg-1").is_empty());
        let conflicts = tag(&mut m, &keys, "chg-2");
        assert_eq!(
            conflicts,
            vec![
                ("resp:r1".into(), "chg-1".into()),
                ("resp:r2".into(), "chg-1".into())
            ]
        );
        assert_eq!(m.change_map["resp:r1"], "chg-2");
    }

    /// Retag by the CARRIER: a node id takes the node's own pending change and
    /// every pending element under it — the unit `get_pending` shows — so
    /// re-filing a mis-filed task is one id, not a hand-assembled key list.
    #[test]
    fn retag_moves_a_carrier_with_everything_pending_under_it() {
        let committed = model_with_resps(&[("r1", "exists")]);
        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new A"), ("r3", "new B")]);
        let wrong = open_change(&mut plan, "the wrong home", 100);
        let right = open_change(&mut plan, "where it belongs", 200);
        tag(
            &mut plan,
            &[
                element_key(ElementKind::Responsibility, None, "r2"),
                element_key(ElementKind::Responsibility, None, "r3"),
            ],
            &wrong,
        );

        let out = retag(&committed, &mut plan, &["n1".into()], Some(&right)).unwrap();

        assert_eq!(
            out.moved.len(),
            2,
            "both claims under the node moved: {:?}",
            out.moved
        );
        assert!(out.unmatched.is_empty());
        assert_eq!(plan.change_map["resp:r2"], right);
        assert_eq!(plan.change_map["resp:r3"], right);
        assert!(out
            .moved
            .iter()
            .all(|(_, from)| from.as_deref() == Some(wrong.as_str())));
    }

    /// The other three target forms: one element by its own id, a whole change
    /// by `chg-N`, and `unfiled` for everything untagged. Detaching (`to:
    /// None`) drops the key rather than pointing it somewhere.
    #[test]
    fn retag_targets_elements_whole_changes_and_the_unfiled_bucket() {
        let committed = model_with_resps(&[("r1", "exists")]);
        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new A"), ("r3", "new B")]);
        let a = open_change(&mut plan, "change A", 100);
        let b = open_change(&mut plan, "change B", 200);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &a,
        );
        // r3 stays unfiled.

        // One element by id, into B.
        let out = retag(&committed, &mut plan, &["r3".into()], Some(&b)).unwrap();
        assert_eq!(out.moved, vec![("resp:r3".to_string(), None)]);

        // A whole change: everything filed under A joins B.
        let out = retag(&committed, &mut plan, std::slice::from_ref(&a), Some(&b)).unwrap();
        assert_eq!(out.moved, vec![("resp:r2".to_string(), Some(a.clone()))]);
        assert_eq!(plan.change_map["resp:r2"], b);

        // Detach everything under B back to unfiled.
        let out = retag(&committed, &mut plan, std::slice::from_ref(&b), None).unwrap();
        assert_eq!(out.moved.len(), 2);
        assert!(
            plan.change_map.is_empty(),
            "detached keys leave the map: {:?}",
            plan.change_map
        );

        // And now "unfiled" is what names them.
        let out = retag(&committed, &mut plan, &["unfiled".into()], Some(&a)).unwrap();
        assert_eq!(out.moved.len(), 2);
    }

    /// An id with no pending work is reported, never silently swallowed — and
    /// an already-correct filing is not counted as a move. A caller that reads
    /// "moved 0" must be able to tell "nothing to do" from "wrong id".
    #[test]
    fn retag_reports_unmatched_ids_and_stays_idempotent() {
        let committed = model_with_resps(&[("r1", "exists")]);
        let mut plan = model_with_resps(&[("r1", "exists"), ("r2", "new A")]);
        let a = open_change(&mut plan, "change A", 100);
        tag(
            &mut plan,
            &[element_key(ElementKind::Responsibility, None, "r2")],
            &a,
        );

        // r1 is committed and unchanged — it carries no pending entry.
        let out = retag(&committed, &mut plan, &["r1".into(), "r2".into()], Some(&a)).unwrap();
        assert_eq!(out.unmatched, vec!["r1".to_string()]);
        assert!(out.moved.is_empty(), "r2 was already filed under A");

        // A destination that does not exist is refused outright.
        assert!(retag(&committed, &mut plan, &["r2".into()], Some("chg-99")).is_err());
    }

    /// Sign-off snapshots the tagged entries; afterwards a reword reads as
    /// AMENDED, a fresh tag as ADDED, a removed entry as DROPPED, and an
    /// unchanged one is not reported at all.
    #[test]
    fn sign_off_classifies_later_edits_against_the_snapshot() {
        let mut plan = model_with_resps(&[("r1", "does one"), ("r2", "does two")]);
        let cid = open_change(&mut plan, "two claims", 1);
        tag(
            &mut plan,
            &["resp:r1".to_string(), "resp:r2".to_string()],
            &cid,
        );
        assert_eq!(sign_off(&mut plan, &cid, 2).unwrap(), 2);
        let meta = plan.changes[0].clone();
        assert_eq!(meta.signed_off.as_ref().unwrap().at, 2);
        assert_eq!(
            meta.signed_off.as_ref().unwrap().entries["resp:r1"]
                .statement
                .as_deref(),
            Some("does one")
        );
        assert!(
            classify_against_signoff(&plan, &meta).is_empty(),
            "nothing moved yet"
        );

        // Reword r1, add r3, drop r2.
        plan.nodes[0].responsibilities[0].statement = "does one differently".into();
        plan.nodes[0].responsibilities.retain(|r| r.id != "r2");
        plan.change_map.remove("resp:r2");
        plan.nodes[0].responsibilities.push(
            serde_json::from_value(serde_json::json!({ "id": "r3", "statement": "does three" }))
                .unwrap(),
        );
        tag(&mut plan, &["resp:r3".to_string()], &cid);

        let mut out = classify_against_signoff(&plan, &meta);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        let kinds: Vec<(String, Classification)> =
            out.iter().map(|(k, c, _)| (k.clone(), *c)).collect();
        assert_eq!(
            kinds,
            vec![
                ("resp:r1".to_string(), Classification::Amended),
                ("resp:r2".to_string(), Classification::Dropped),
                ("resp:r3".to_string(), Classification::Added),
            ]
        );
        // The snapshot travels with the amendment so a reject can restore it.
        assert_eq!(
            out[0].2.as_ref().unwrap().statement.as_deref(),
            Some("does one")
        );
        assert_eq!(
            classify_key(&plan, "resp:r1").unwrap().1,
            Classification::Amended
        );
        assert_eq!(
            classify_key(&plan, "resp:r3").unwrap().1,
            Classification::Added
        );
        assert!(classify_key(&plan, "resp:nope").is_none());
    }

    /// Concern tags, positions, icons, and directives are metadata: touching
    /// them after sign-off must not read as an amendment. Moving a claim to
    /// another host does.
    #[test]
    fn entry_hash_ignores_cosmetics_but_sees_a_move() {
        let mut plan = model_with_resps(&[("r1", "does one")]);
        plan.nodes.push(
            serde_json::from_value(
                serde_json::json!({ "id": "n2", "kind": "component", "name": "D" }),
            )
            .unwrap(),
        );
        let before = entry_hash(&plan, "resp:r1").unwrap();
        plan.nodes[0].responsibilities[0].concern = Some("auth".into());
        plan.nodes[0].responsibilities[0].directives = vec!["must log".into()];
        plan.nodes[0].responsibilities[0].last_touched_at = Some(99);
        plan.nodes[0].icon = Some("Box".into());
        assert_eq!(
            entry_hash(&plan, "resp:r1").unwrap().hash,
            before.hash,
            "cosmetic edits are not content"
        );

        let r = plan.nodes[0].responsibilities.remove(0);
        plan.nodes[1].responsibilities.push(r);
        let moved = entry_hash(&plan, "resp:r1").unwrap();
        assert_ne!(moved.hash, before.hash, "a move changes the signed content");
        assert_eq!(moved.host.as_deref(), Some("n2"));
        assert!(entry_hash(&plan, "resp:gone").is_none());
        assert_eq!(fnv1a64(b"foobar"), "85944171f73967e8");
    }

    /// A canvas save re-stamps every signed-off change: the developer's own
    /// edit becomes the new intent, and an unsigned change is left alone.
    #[test]
    fn restamp_signoffs_refreshes_signed_changes_only() {
        let mut plan = model_with_resps(&[("r1", "does one"), ("r2", "does two")]);
        let signed = open_change(&mut plan, "signed", 1);
        let unsigned = open_change(&mut plan, "unsigned", 1);
        tag(&mut plan, &["resp:r1".to_string()], &signed);
        tag(&mut plan, &["resp:r2".to_string()], &unsigned);
        sign_off(&mut plan, &signed, 2).unwrap();
        plan.nodes[0].responsibilities[0].statement = "does one, the dev's way".into();
        let meta = plan
            .changes
            .iter()
            .find(|c| c.id == signed)
            .cloned()
            .unwrap();
        assert_eq!(
            classify_against_signoff(&plan, &meta).len(),
            1,
            "diverges before the re-stamp"
        );

        assert_eq!(restamp_signoffs(&mut plan, 3), 1);
        let meta = plan
            .changes
            .iter()
            .find(|c| c.id == signed)
            .cloned()
            .unwrap();
        assert!(
            classify_against_signoff(&plan, &meta).is_empty(),
            "the dev's edit is intent"
        );
        assert_eq!(meta.signed_off.as_ref().unwrap().at, 3);
        assert!(plan
            .changes
            .iter()
            .find(|c| c.id == unsigned)
            .unwrap()
            .signed_off
            .is_none());
    }
}
