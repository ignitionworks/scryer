//! The fold's gates — what `mark_implemented` refuses to commit, and why.
//!
//! 0. **Countersignature** (opt-in, off by default): while the project sets
//!    `policy.requireCountersignedFolds`, a change no team member OTHER THAN
//!    its author has signed off does not fold at all. The two gates below are
//!    relative to a signature — they ask whether the plan drifted since
//!    approval, never whether it was approved — so a team that wants the
//!    second question asked turns this one on. Unlike the others it refuses
//!    the WHOLE call: there is nothing claim-by-claim about "nobody approved
//!    this", and a partial fold would leave the change half-landed on an
//!    approval that does not exist. A project that has not opted in never
//!    reaches it.
//! 1. **Sign-off** (forward vagrancy): a claim the agent reworded, moved, or
//!    added AFTER the developer signed off its change is a proposal, not
//!    intent. It is flagged `vagrant` with a `vagrant_origin` and the approved
//!    text, left in the plan, and reported as awaiting the developer's verdict.
//!    A signed-off claim the agent dropped is restored as pending intent.
//! 2. **Evidence**: a testable (When/While/If) claim on a code-backed host
//!    folds only with a test attached AND a current passing verdict
//!    (`scryer_extract::test_status::claim_evidence`). Otherwise it stays in the
//!    plan and the response names the missing fact and the test files to run.
//!    `force` bypasses this gate visibly (an `unverified` history event).
//!
//! Gates 1 and 2 return a WITHHOLD set the fold engine honours
//! (`commit_element_withholding`), so the rest of the fold proceeds — leaving
//! a claim pending is a legitimate, honest exit, never a loop.

use scryer_core::changes::{self, Classification};
use scryer_core::diff::{self, ElementKind as EK};
use scryer_core::history::{EventKind, HistoryEvent};
use scryer_core::refusals::Refusal;
use scryer_core::{ears, Kind, ModelRef, Responsibility, ScryModel};
use scryer_extract::test_status::{claim_evidence, Evidence};
use std::collections::{BTreeSet, HashMap, HashSet};

/// What the gates decided for one fold call.
#[derive(Debug, Default)]
pub(crate) struct GateOutcome {
    /// Claims the fold must leave in the plan.
    pub withhold: HashSet<String>,
    /// The ledger entries to record for them.
    pub refusals: Vec<Refusal>,
    /// Response lines, in order.
    pub lines: Vec<String>,
    /// Claims that failed the evidence gate but fold anyway under `force`.
    pub forced: Vec<String>,
    /// Whether the gates wrote to the plan (vagrant flags set, dropped claims
    /// restored) and the caller must persist it before folding.
    pub plan_dirty: bool,
}

/// The claims PENDING on `node_id` — the ones a whole-node fold would actually
/// change in committed — excluding vagrants (they never fold) and claims tagged
/// to a different change than the node (another task's work, which the fold
/// engine leaves behind on its own).
pub(crate) fn pending_claims_on(
    committed: &ScryModel,
    planned: &ScryModel,
    node_id: &str,
) -> Vec<String> {
    let host_key = changes::element_key(EK::Node, None, node_id);
    let vagrant: HashSet<&str> = planned
        .nodes
        .iter()
        .flat_map(|n| n.responsibilities.iter())
        .filter(|r| r.vagrant == Some(true))
        .map(|r| r.id.as_str())
        .collect();
    diff::diff(committed, planned)
        .changes
        .iter()
        .filter(|ch| {
            ch.kind == EK::Responsibility
                && ch.owner_id.as_deref() == Some(node_id)
                && !ch.changes.contains(&diff::Change::Deleted)
                && !vagrant.contains(ch.id.as_str())
                && !changes::foreign_to_host(
                    &planned.change_map,
                    &host_key,
                    &changes::element_key(EK::Responsibility, None, &ch.id),
                )
        })
        .map(|ch| ch.id.clone())
        .collect()
}

fn find_resp_mut<'a>(
    model: &'a mut ScryModel,
    id: &str,
) -> Option<(String, &'a mut Responsibility)> {
    for n in &mut model.nodes {
        if let Some(r) = n.responsibilities.iter_mut().find(|r| r.id == id) {
            return Some((n.id.clone(), r));
        }
    }
    for g in &mut model.groups {
        if let Some(r) = g.responsibilities.iter_mut().find(|r| r.id == id) {
            return Some((g.id.clone(), r));
        }
    }
    None
}

fn find_resp<'a>(model: &'a ScryModel, id: &str) -> Option<(&'a str, &'a Responsibility)> {
    model
        .nodes
        .iter()
        .flat_map(|n| n.responsibilities.iter().map(move |r| (n.id.as_str(), r)))
        .chain(
            model
                .groups
                .iter()
                .flat_map(|g| g.responsibilities.iter().map(move |r| (g.id.as_str(), r))),
        )
        .find(|(_, r)| r.id == id)
}

/// Whether a host expects tests at all: a person or an external system never
/// does; a group's claims are discharged by its members (ungated, like
/// structural nodes' own claims are advisory — see the plan). Everything else
/// is code-backed.
fn code_backed_host(model: &ScryModel, host_id: &str) -> bool {
    match model.nodes.iter().find(|n| n.id == host_id) {
        Some(n) => n.kind != Kind::Person && n.external != Some(true),
        None => false, // a group
    }
}

/// The change's AUTHOR, for the countersignature test.
///
/// [`changes::ChangeMeta`] records no opener — `open_change` mints an id and
/// keeps the dev's rationale, nothing more — so authorship lives where the
/// plan writes left it: the history log. The author is the actor on the
/// EARLIEST plan event tagged to the change, which is whoever authored its
/// entries. A plan write that named no actor lands as `agent`, still an
/// identity a countersignature must differ from, so a host that attributes
/// nothing meets the gate rather than slipping past it.
///
/// `None` only when the change authored nothing the log saw — and a change
/// that wrote no claim has nothing to fold either.
fn change_author(history: &[HistoryEvent], change_id: &str) -> Option<String> {
    history
        .iter()
        .find(|e| e.kind == EventKind::Plan && e.change_id.as_deref() == Some(change_id))
        .map(|e| e.by.clone())
}

/// Gate 0 — the countersignature, for a project that opted in.
///
/// The test is on the ACTOR ID and nothing else: the fold passes when some
/// actor other than the author has signed the change off. Proxy-ness is
/// recorded, never gated — a sign-off an agent made on a developer's behalf
/// counts exactly when its `by` differs, because the question the policy asks
/// is "did a second party look at this", and a second party is a second
/// identity. Answering it by inspecting `onBehalfOf` would refuse precisely
/// the review a host is set up to perform.
///
/// `Err` refuses the WHOLE fold: nothing folds, the plan is untouched (this
/// runs before the gates that write to it), and the message names what is
/// missing. Returns silently for a project with no policy, which is every
/// project that has not opted in.
fn countersign_gate(
    model_ref: &ModelRef,
    committed: &ScryModel,
    planned: &ScryModel,
    in_fold: &BTreeSet<String>,
    out: &mut GateOutcome,
) -> Result<(), String> {
    if in_fold.is_empty() || !changes::requires_countersigned_folds(committed) {
        return Ok(());
    }
    let history = scryer_core::history::read_history(model_ref);
    for cid in in_fold {
        let Some(meta) = planned.changes.iter().find(|c| &c.id == cid) else { continue };
        let author = change_author(&history, cid);
        let signature = meta.signed_off.as_ref();
        let signer = signature.and_then(|s| s.by.as_deref());
        let missing = match (signer, author.as_deref()) {
            // Signed by somebody who is not the author: countersigned.
            (Some(by), a) if Some(by) != a => None,
            (Some(by), _) => Some(format!("is signed off only by its own author, {by}")),
            (None, _) if signature.is_none() => Some("carries no sign-off at all".to_string()),
            (None, _) => {
                Some("carries a sign-off that names no actor, so nobody is on record as \
                      having approved it"
                    .to_string())
            }
        };
        let Some(missing) = missing else {
            // Passed — say by whom, and say when it was a proxy, so the fold's
            // own transcript carries the approval it folded on.
            let by = signer.unwrap_or_default();
            let proxy = signature
                .and_then(|s| s.on_behalf_of.as_deref())
                .map(|p| format!(" on behalf of {p}"))
                .unwrap_or_default();
            out.lines.push(format!(
                "COUNTERSIGNED {cid} by {by}{proxy} (authored by {}) — this project requires a \
                 fold to be signed off by a team member other than the author",
                author.as_deref().unwrap_or("nobody on record")
            ));
            continue;
        };
        let whose = match author.as_deref() {
            Some(a) => format!("its author ({a})"),
            None => "its author".to_string(),
        };
        return Err(format!(
            "REFUSED: nothing was folded. This project requires a fold to be countersigned, and \
             {cid} {missing}. A team member other than {whose} must sign {cid} off — have them \
             run `sign_off {{change_id: \"{cid}\"}}` under their own identity (the MCP server \
             reads it from SCRYER_ACTOR) — then fold again. The test is on the signing ACTOR, so \
             a sign-off made on someone's behalf counts whenever the actor differs. The policy is \
             `policy.requireCountersignedFolds` in .scryer/model.scry; a project that clears it \
             folds as before."
        ));
    }
    Ok(())
}

/// Run the gates over `candidates` (the claims this fold is about to commit).
/// `tests_in_call` maps claim id → test files attached in the SAME call: an
/// attachment with no verdict yet still refuses (the verdict comes from a run
/// + ingest, which must precede the fold), but the refusal names those files.
/// `change` is the change `mark_implemented` was pointed at by name, when it
/// was — gate 0 needs it even for a fold whose candidates are empty.
pub(crate) fn gate(
    model_ref: &ModelRef,
    committed: &ScryModel,
    planned: &mut ScryModel,
    candidates: &[String],
    change: Option<&str>,
    tests_in_call: &HashMap<String, Vec<String>>,
    force: bool,
    now: u64,
) -> Result<GateOutcome, String> {
    let mut out = GateOutcome::default();

    // The changes this fold lands work under: whatever its candidates are
    // tagged to, plus the one it was pointed at by name — a `mark_implemented
    // {change}` whose tags are all carriers has no candidate to speak for it.
    let involved: BTreeSet<String> = candidates
        .iter()
        .filter_map(|id| {
            planned.change_map.get(&changes::element_key(EK::Responsibility, None, id)).cloned()
        })
        .collect();

    // ---- 0. Countersignature: opt-in, and it refuses the whole fold. -------
    // First, and before anything below writes to the plan: a fold nobody
    // approved must leave no trace at all.
    let mut in_fold = involved.clone();
    in_fold.extend(change.map(str::to_string));
    countersign_gate(model_ref, committed, planned, &in_fold, &mut out)?;

    // ---- 1. Sign-off: amendments and additions stay behind as vagrant. -----
    for id in candidates {
        let key = changes::element_key(EK::Responsibility, None, id);
        let Some((cid, class, snap)) = changes::classify_key(planned, &key) else { continue };
        let Some(origin) = class.origin() else { continue };
        let Some((host, r)) = find_resp_mut(planned, id) else { continue };
        let approved = snap.as_ref().and_then(|s| s.statement.clone());
        r.vagrant = Some(true);
        r.vagrant_origin = Some(origin.to_string());
        r.approved_statement = approved.clone();
        let reason = match class {
            Classification::Amended => format!(
                "reworded after sign-off of {cid} (approved: \"{}\")",
                approved.as_deref().unwrap_or("?")
            ),
            _ => format!("added after sign-off of {cid}"),
        };
        out.lines.push(format!(
            "AWAITING VERDICT {id} (stays in the plan, flagged vagrant/{origin}): {reason} — the \
             developer adopts, rejects, or rewords it from Needs Review; it does not fold"
        ));
        out.refusals.push(Refusal {
            resp_id: id.clone(),
            host_id: host,
            kind: origin.to_string(),
            reason,
            run: Vec::new(),
            at: now,
        });
        out.withhold.insert(id.clone());
        out.plan_dirty = true;
    }

    // Dropped signed-off claims come back as the original intent.
    for cid in &involved {
        let Some(meta) = planned.changes.iter().find(|c| &c.id == cid).cloned() else { continue };
        for (key, class, snap) in changes::classify_against_signoff(planned, &meta) {
            if class != Classification::Dropped {
                continue;
            }
            let Some((EK::Responsibility, _, rid)) = changes::parse_key(&key) else { continue };
            let Some(snap) = snap else { continue };
            // Folded, not dropped: the element stands in the plan exactly as
            // approved and only lost its tag because an earlier fold carried
            // it into committed. Nothing to restore.
            if changes::entry_hash(planned, &key).is_some_and(|now| now.hash == snap.hash) {
                continue;
            }
            let (Some(stmt), Some(host)) = (snap.statement.clone(), snap.host.clone()) else {
                continue;
            };
            let restored = match find_resp_mut(planned, &rid) {
                // Reverted in place (the tag was GC'd): put the approved text back.
                Some((_, r)) => {
                    r.statement = stmt.clone();
                    true
                }
                // Gone: re-insert on its approved host, if that host still exists.
                None => match planned.nodes.iter_mut().find(|n| n.id == host) {
                    Some(n) => {
                        n.responsibilities.push(Responsibility {
                            id: rid.clone(),
                            statement: stmt.clone(),
                            concern: None,
                            vagrant: None,
                            vagrant_origin: None,
                            approved_statement: None,
                            stale: None,
                            stale_proposal: None,
                            directives: Vec::new(),
                            last_touched_at: Some(now),
                        });
                        true
                    }
                    None => false,
                },
            };
            if restored {
                planned.change_map.insert(key.clone(), cid.clone());
                // Restored means PENDING: this fold must not carry it across.
                out.withhold.insert(rid.clone());
                out.plan_dirty = true;
                out.lines.push(format!(
                    "RESTORED {rid} as pending intent (\"{stmt}\") — it was signed off in {cid} \
                     and the plan no longer carried it; the agent's proposal to drop it needs \
                     the developer's verdict, so it stays in the queue"
                ));
            } else {
                out.lines.push(format!(
                    "DROPPED {rid} (\"{stmt}\") was signed off in {cid} and is gone from the plan \
                     along with its host — the developer should know the agent dropped it"
                ));
            }
        }
    }

    // ---- 2. Evidence: testable claims need a current passing verdict. --------
    let mut gated: Vec<String> = Vec::new();
    for id in candidates {
        if out.withhold.contains(id) {
            continue;
        }
        let Some((host, r)) = find_resp(planned, id) else { continue };
        if !code_backed_host(planned, host) {
            continue;
        }
        if !ears::classify(&r.statement).testable() {
            continue;
        }
        gated.push(id.clone());
    }
    if gated.is_empty() {
        return Ok(out);
    }
    let evidence = claim_evidence(model_ref, &gated)?;
    for id in &gated {
        let mut ev = evidence.get(id).cloned().unwrap_or(Evidence::NoTest);
        if let (Evidence::NoTest, Some(files)) = (&ev, tests_in_call.get(id)) {
            ev = Evidence::NoVerdict { tests: files.clone() };
        }
        if ev.verified() {
            continue;
        }
        let host = find_resp(planned, id).map(|(h, _)| h.to_string()).unwrap_or_default();
        let kind = match &ev {
            Evidence::NoTest => "no-test",
            Evidence::NoVerdict { .. } => "no-verdict",
            Evidence::Stale { .. } => "stale",
            Evidence::Failing { .. } => "failing",
            Evidence::Verified => unreachable!(),
        };
        let reason = ev.reason();
        if force {
            out.forced.push(id.clone());
            out.lines.push(format!(
                "UNVERIFIED {id} folded under force: {reason} — recorded in history; the claim \
                 reads as committed but unproven"
            ));
            continue;
        }
        let fix = match &ev {
            Evidence::NoTest => {
                " — write the test the statement already specifies, attach it (update_source_map \
                 `test_entries`), run it with the JUnit reporter on, ingest_test_report, then fold \
                 again"
            }
            _ => ", ingest_test_report the report, then fold again",
        };
        out.lines.push(format!("REFUSED {id} (stays in the plan): {reason}{fix}"));
        out.refusals.push(Refusal {
            resp_id: id.clone(),
            host_id: host,
            kind: kind.to_string(),
            reason,
            run: ev.tests().to_vec(),
            at: now,
        });
        out.withhold.insert(id.clone());
    }
    Ok(out)
}

/// The baseline keys a fold should refresh for the claims it landed: each
/// folded claim's implementation key and its `test:` key.
pub(crate) fn baseline_keys(folded: &[String]) -> BTreeSet<String> {
    folded
        .iter()
        .flat_map(|id| [id.clone(), scryer_core::test_key(id)])
        .collect()
}
