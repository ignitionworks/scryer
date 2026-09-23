use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData as McpError;
use scryer_core::history::{append_event, EventRow, HistoryEvent};
use scryer_core::{Kind, ModelLock, ModelRef, Node, Responsibility, ScryModel};
use std::collections::HashMap;

/// Acquire the exclusive model write lock, or return an error result to surface
/// to the agent. Hold the returned guard for the whole read-modify-write of a
/// write tool so concurrent writers (parallel sessions, the canvas) serialize.
pub(crate) fn lock_or_err(model_ref: &ModelRef) -> Result<ModelLock, CallToolResult> {
    scryer_core::lock_model(model_ref).map_err(|e| CallToolResult::error(vec![Content::text(e)]))
}

/// Strip empty values from a JSON tree to keep MCP responses compact.
pub(crate) fn strip_fields_compact(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::Object(map) => {
            map.retain(|_, v| !matches!(v, serde_json::Value::String(s) if s.is_empty()));
            map.retain(|_, v| !v.is_null());
            map.retain(|_, v| !matches!(v, serde_json::Value::Array(a) if a.is_empty()));
            map.retain(|_, v| !matches!(v, serde_json::Value::Object(m) if m.is_empty()));
            for (_, v) in map.iter_mut() {
                strip_fields_compact(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                strip_fields_compact(v);
            }
        }
        _ => {}
    }
}

pub(crate) fn parse_kind(s: &str) -> Result<Kind, McpError> {
    match s {
        "person" => Ok(Kind::Person),
        "system" => Ok(Kind::System),
        "container" => Ok(Kind::Container),
        "component" => Ok(Kind::Component),
        // "schema" is the legacy name for a symbol that carries only properties.
        "symbol" | "schema" => Ok(Kind::Symbol),
        _ => Err(McpError::invalid_params(
            format!(
                "Invalid kind '{}'. Must be: person, system, container, component, symbol",
                s
            ),
            None,
        )),
    }
}

pub(crate) fn kind_str(k: &Kind) -> &'static str {
    match k {
        Kind::Person => "person",
        Kind::System => "system",
        Kind::Container => "container",
        Kind::Component => "component",
        Kind::Symbol => "symbol",
    }
}

/// Build a denormalized graph view of a node for MCP responses:
/// adds `childIds`, `incomingLinks`, `outgoingLinks` to the node JSON.
pub(crate) fn denormalize_node(node: &Node, model: &ScryModel) -> serde_json::Value {
    let mut val = serde_json::to_value(node).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(map) = &mut val {
        let child_ids: Vec<&str> = model
            .nodes
            .iter()
            .filter(|n| n.parent_id.as_deref() == Some(&node.id))
            .map(|n| n.id.as_str())
            .collect();
        let incoming: Vec<&str> = model
            .links
            .iter()
            .filter(|l| l.dst == node.id)
            .map(|l| l.id.as_str())
            .collect();
        let outgoing: Vec<&str> = model
            .links
            .iter()
            .filter(|l| l.src == node.id)
            .map(|l| l.id.as_str())
            .collect();
        map.insert("childIds".to_string(), serde_json::json!(child_ids));
        map.insert("incomingLinks".to_string(), serde_json::json!(incoming));
        map.insert("outgoingLinks".to_string(), serde_json::json!(outgoing));
    }
    val
}

/// Build a compact outline tree of the model: each node carries its id, name,
/// kind, a one-line description, and responsibility/property COUNTS (not
/// bodies), plus its children nested under it. Lets an agent grasp a model's
/// shape without materializing every responsibility, property, link, and
/// source-map entry. Roots are nodes with no parent. When `include_symbols` is
/// false the code level is omitted — the architecture overview (drill into a
/// component with `read_model {node}` to see its symbols).
pub(crate) fn outline_tree(model: &ScryModel, include_symbols: bool) -> Vec<serde_json::Value> {
    let mut children_of: HashMap<Option<&str>, Vec<&Node>> = HashMap::new();
    for n in &model.nodes {
        if !include_symbols && n.kind == Kind::Symbol {
            continue;
        }
        children_of
            .entry(n.parent_id.as_deref())
            .or_default()
            .push(n);
    }

    fn build(node: &Node, children_of: &HashMap<Option<&str>, Vec<&Node>>) -> serde_json::Value {
        let kids: Vec<serde_json::Value> = children_of
            .get(&Some(node.id.as_str()))
            .map(|cs| cs.iter().map(|c| build(c, children_of)).collect())
            .unwrap_or_default();
        let mut v = serde_json::json!({
            "id": node.id,
            "name": node.name,
            "kind": kind_str(&node.kind),
            "description": node.description,
            "nResp": node.responsibilities.len(),
            "nProps": node.properties.len(),
            "children": kids,
        });
        strip_fields_compact(&mut v);
        v
    }

    children_of
        .get(&None)
        .map(|roots| roots.iter().map(|r| build(r, &children_of)).collect())
        .unwrap_or_default()
}

/// Breadcrumb path from a root down to `node_id`, by node name, joined with
/// " / ". Used to give search hits a location without the caller re-walking the
/// tree.
pub(crate) fn breadcrumb(model: &ScryModel, node_id: &str) -> String {
    let by_id: HashMap<&str, &Node> = model.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut names = Vec::new();
    let mut cur = by_id.get(node_id).copied();
    let mut guard = 0;
    while let Some(n) = cur {
        names.push(n.name.as_str());
        cur = n.parent_id.as_deref().and_then(|p| by_id.get(p).copied());
        guard += 1;
        if guard > 64 {
            break; // cycle guard
        }
    }
    names.reverse();
    names.join(" / ")
}

/// Directives are user-authored and read-only to the AI's ordinary writes —
/// both a responsibility's `directives` and a node's own node-level
/// `directives`. Before committing any AI write, force each back to whatever
/// the prior on-disk model held for that id; ids with no prior entry get none.
/// This lets the AI create, edit, and move responsibilities and nodes while
/// leaving directives entirely under the user's control. A directive's own
/// `cites` travels inside the directive, so it is restored with it; a CLAIM's
/// `cites` is the agent's to write (`update_claim`, `update_nodes`) and is
/// deliberately left alone here. (The interactive
/// patch path can't reach them — they're `schemars(skip)` — but the whole-node
/// generation primitives `replace_model`/`replace_subtree` rebuild nodes from JSON and
/// would otherwise drop them.) Not applied to `move_responsibilities`, which
/// preserves directives across a deliberate responsibility-id rename — nor to
/// `set_directives`, the one deliberate write path, reserved for edits the
/// user explicitly asked for.
pub(crate) fn enforce_readonly_directives(model: &mut ScryModel, prior: &ScryModel) {
    let prior_resps = prior
        .nodes
        .iter()
        .flat_map(|n| n.responsibilities.iter())
        .chain(prior.groups.iter().flat_map(|g| g.responsibilities.iter()));
    let prior_dir: HashMap<&str, &Vec<scryer_core::Directive>> = prior_resps
        .map(|r| (r.id.as_str(), &r.directives))
        .collect();
    let restore = |r: &mut Responsibility| {
        r.directives = prior_dir
            .get(r.id.as_str())
            .map(|d| (*d).clone())
            .unwrap_or_default();
    };
    // Node-level directives, keyed by node id (same read-only guarantee).
    let prior_node_dir: HashMap<&str, &Vec<scryer_core::Directive>> = prior
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), &n.directives))
        .collect();
    for n in &mut model.nodes {
        n.responsibilities.iter_mut().for_each(&restore);
        n.directives = prior_node_dir
            .get(n.id.as_str())
            .map(|d| (*d).clone())
            .unwrap_or_default();
    }
    for g in &mut model.groups {
        g.responsibilities.iter_mut().for_each(&restore);
    }
}

/// Canvas placements (`Node::position`) are user-authored and read-only to the
/// AI, exactly like directives — the typed patch tools can't reach them
/// (`schemars(skip)`), but the whole-node generation primitives
/// `replace_model`/`replace_subtree` rebuild nodes from JSON and would otherwise drop
/// them. Force each node's position back to whatever the FIRST prior model
/// containing that node held; nodes new to every prior get none (the AI never
/// places nodes — auto-layout does). Priors are ordered most-authoritative
/// first: the plan layer is where the canvas writes, so it precedes committed.
pub(crate) fn restore_node_positions(model: &mut ScryModel, priors: &[&ScryModel]) {
    let prior_pos: HashMap<&str, Option<scryer_core::Position>> = priors
        .iter()
        .rev()
        .flat_map(|m| m.nodes.iter())
        .map(|n| (n.id.as_str(), n.position))
        .collect();
    for n in &mut model.nodes {
        n.position = prior_pos.get(n.id.as_str()).copied().flatten();
    }
}

/// Project root from request param, active model, or cwd.
/// Build a committed-model event diff row for a responsibility — its statement,
/// anchored to the first source location the model maps it to (if any).
pub(crate) fn resp_event_row(marker: &str, model: &ScryModel, resp: &Responsibility) -> EventRow {
    let row = EventRow::new(marker, resp.statement.clone());
    match model.source_map.get(&resp.id).and_then(|locs| locs.first()) {
        Some(loc) => row.with_source(loc.clone()),
        None => row,
    }
}

/// The ACTOR this MCP process writes as, from `SCRYER_ACTOR`. A host that runs
/// the agent on someone's behalf sets it; unset means unattributed, which is
/// every plain `scryer-mcp` invocation. Opaque — the server never interprets it.
pub(crate) fn env_actor() -> Option<String> {
    std::env::var("SCRYER_ACTOR")
        .ok()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
}

/// The PERSON this MCP process signs FOR, from `SCRYER_ON_BEHALF_OF` — set by
/// a host whose agent approves a change on a developer's behalf, beside the
/// `SCRYER_ACTOR` that names the agent itself. Env-scoped like the actor, and
/// for the same reason: who an agent is, and who it speaks for, are the host's
/// facts to assert, never the agent's to claim in a tool call. Unset is a
/// direct sign-off, which is every plain `scryer-mcp` invocation.
pub(crate) fn env_on_behalf_of() -> Option<String> {
    std::env::var("SCRYER_ON_BEHALF_OF")
        .ok()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
}

/// Record a committed-model history event, best-effort: a logging failure must
/// never abort the model operation that produced it (see [`scryer_core::history`]).
///
/// Names the actor this process writes as AND, when the host runs it on
/// someone's behalf, the PERSON it acted for — the same pair a sign-off
/// already records. Without the person, a fold an agent made on a developer's
/// say-so is indistinguishable in the timeline from one it made on its own.
pub(crate) fn record_event(model_ref: &ModelRef, ev: HistoryEvent) {
    let _ = append_event(
        model_ref,
        &ev.by_actor(env_actor().as_deref())
            .for_person(env_on_behalf_of().as_deref()),
    );
}

/// THE MCP plan-write seam: every plan this server writes goes through here, so
/// the plan events it appends name the actor this process writes as rather than
/// the bare agent — the same `SCRYER_ACTOR` that already names the committed
/// events [`record_event`] records.
///
/// This is what lets a change authored through the MCP seam have a KNOWN
/// author: the countersign gate reads a change's author off the earliest plan
/// event tagged to it, so an event stuck at "agent" leaves every agent-authored
/// change authorless and the gate unable to bite.
///
/// Unset `SCRYER_ACTOR` is unattributed and lands as the agent, which is what a
/// plain `scryer-mcp` invocation is. `SCRYER_ON_BEHALF_OF` rides along beside
/// it, so a plan a host's agent edited on a developer's say-so records both —
/// the same pair the sign-off records, for the same reason.
pub(crate) fn write_planned(
    model_ref: &ModelRef,
    model: &scryer_core::ScryModel,
) -> Result<(), String> {
    scryer_core::write_planned_for(
        model_ref,
        model,
        env_actor().as_deref(),
        env_on_behalf_of().as_deref(),
    )
}

pub(crate) fn resolve_model_ref(req_project: Option<&str>) -> Result<ResolvedProject, McpError> {
    let named = req_project.map(str::trim).filter(|p| !p.is_empty());
    let path = match named {
        Some(p) => std::path::PathBuf::from(p),
        None => std::env::current_dir()
            .map_err(|e| McpError::internal_error(format!("cannot read cwd: {}", e), None))?,
    };
    Ok(ResolvedProject {
        model_ref: ModelRef::ProjectLocal(path),
        defaulted: named.is_none(),
    })
}

/// WHICH model an answer is about, and how this process decided that.
///
/// A tool call that names `project` is unambiguous. One that does not falls
/// back to the process's working directory — and a working directory is a
/// fact about how the session was launched, not about what it meant. An agent
/// running in a per-change worktree, or anywhere but the checkout it is
/// reasoning about, then reads that directory's `.scryer/` and is answered
/// truthfully about the wrong model: nodes folded elsewhere are "not found",
/// and health is real but stale. Nothing is broken and nothing says so.
///
/// So the fallback is recorded rather than hidden, and every answer carries
/// both facts ([`ResolvedProject::stamp`], [`ResolvedProject::note`]). The
/// path was always knowable; what was missing was anyone saying it out loud.
///
/// Derefs to the [`ModelRef`], so the whole tool surface goes on passing
/// `&model_ref` to core exactly as before.
pub(crate) struct ResolvedProject {
    model_ref: ModelRef,
    /// The path came from the process's working directory: the call named none.
    defaulted: bool,
}

impl std::ops::Deref for ResolvedProject {
    type Target = ModelRef;
    fn deref(&self) -> &ModelRef {
        &self.model_ref
    }
}

/// Reads as the [`ModelRef`] it wraps — the provenance is carried beside the
/// path, never folded into how the path prints.
impl std::fmt::Display for ResolvedProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.model_ref.fmt(f)
    }
}

impl ResolvedProject {
    /// The resolved project path, as a reader should see it.
    pub(crate) fn path(&self) -> String {
        self.model_ref.project_path().to_string_lossy().to_string()
    }

    /// What a WRITE's status line says about which model it wrote — and only
    /// when there is something to say.
    ///
    /// `None` for a call that named its project, because every session reads
    /// this line after every write, and a line that is always there is one
    /// nobody reads. The guessed case is the one that has to be loud, so it is
    /// the only one that speaks, and it names the remedy: a session that did
    /// not know it was guessing does not know what to do about it either.
    pub(crate) fn note(&self) -> Option<String> {
        if !self.defaulted {
            return None;
        }
        Some(format!(
            "project DEFAULTED to {} from this process's working directory — no `project` was \
             named; pass it to be sure which model you are writing to",
            self.path()
        ))
    }

    /// Stamp the same two facts onto a JSON answer, for the tools that speak
    /// JSON rather than prose. Absent `projectDefaulted` means the caller
    /// named the path, so an answer about the right model stays quiet.
    pub(crate) fn stamp(&self, payload: &mut serde_json::Value) {
        if let serde_json::Value::Object(o) = payload {
            o.insert("project".into(), serde_json::Value::String(self.path()));
            if self.defaulted {
                o.insert("projectDefaulted".into(), serde_json::Value::Bool(true));
            }
        }
    }
}

/// The `basis` a read answers: one opaque fingerprint over the RELEVANT SET the
/// read showed, which a model write names so the engine can refuse it when that
/// set has moved (`scryer_core::basis`). Derived from BOTH layers whatever
/// layer the read itself returned — a writer's edit stands on the committed
/// claims and the planned ones together.
///
/// `None` where the read showed nothing a writer could base an edit on, so an
/// empty basis that would validate against anything is never minted.
pub(crate) fn basis_of(
    committed: &ScryModel,
    planned: &ScryModel,
    scope: scryer_core::basis::Scope,
) -> Option<String> {
    scryer_core::basis::derive(committed, planned, scope)
}

/// [`basis_of`] for a read that does not already hold both layers. A model that
/// cannot be read answers no basis rather than failing the read: the basis is
/// an addition to the answer, never a new way for a read to die.
pub(crate) fn basis_at(model_ref: &ModelRef, scope: scryer_core::basis::Scope) -> Option<String> {
    let committed = scryer_core::read_model_at(model_ref).ok()?;
    let planned = scryer_core::read_planned_at(model_ref).unwrap_or_else(|_| committed.clone());
    basis_of(&committed, &planned, scope)
}

/// THE BIN, as a line a refusal can end with — the counterpart to
/// [`open_changes_line`]. An act on the bin that names the wrong id is told
/// what the bin actually holds, because "no change 'chg-x'" alone leaves a
/// caller guessing whether it was already deleted or never binned.
pub(crate) fn bin_line(m: &ScryModel) -> String {
    let bin = scryer_core::changes::bin_entries(m);
    if bin.is_empty() {
        return "The bin is empty.".to_string();
    }
    let mut s = String::from("In the bin:");
    for e in bin {
        s.push_str(&format!(
            "\n  {} — \"{}\"{}",
            e.id,
            e.label.unwrap_or_default(),
            match e.why {
                Some(w) => format!(" ({w})"),
                None => String::new(),
            }
        ));
    }
    s
}

/// What an answer says when a change went to the bin — one wording, whichever
/// door it came through, because the fact a reader needs is the same one and it
/// is the OPPOSITE of what abandoning used to mean: the work is still there.
pub(crate) fn bin_message(cid: &str, act: &scryer_core::changes::BinAct) -> String {
    let n = act.kept;
    let mut msg = format!(
        "Binned {cid} — \"{}\": {n} planned entr{} moved to the bin WITH it, kept exactly as \
         they were — nothing reverted, nothing dropped. It is gone from every listing and from \
         the plan's diff, so its entries are no longer pending work and cannot block a fold. \
         Restore it and it comes back as it was; deleting it for good is a separate act, \
         reached only from the bin.",
        scryer_core::changes::title_of(&act.meta),
        if n == 1 { "y" } else { "ies" }
    );
    match act.state.expires_at {
        Some(at) => msg.push_str(&format!(
            "\nExpires at {at} — emptying the bin may delete it."
        )),
        None => msg.push_str("\nNo expiry was set, so emptying the bin will not sweep it."),
    }
    msg
}

/// The OPEN changes, as a line a refusal can end with. Reads
/// `changes::open_changes`, so a change the bin holds is absent from it — this
/// was five identical closures in misc.rs, and five places to forget the bin.
pub(crate) fn open_changes_line(m: &ScryModel) -> String {
    let open: Vec<&scryer_core::changes::ChangeMeta> =
        scryer_core::changes::open_changes(m).collect();
    if open.is_empty() {
        return "No open changes.".to_string();
    }
    let mut s = String::from("Open changes:");
    for c in open {
        let entries = m.change_map.values().filter(|v| *v == &c.id).count();
        s.push_str(&format!(
            "\n  {} — \"{}\" ({} tagged entr{})",
            c.id,
            c.rationale,
            entries,
            if entries == 1 { "y" } else { "ies" }
        ));
    }
    s
}

/// Whether this engine REQUIRES a `basis` on a model write.
///
/// THE SWITCH, and why there is one. A stale basis is refused unconditionally —
/// a caller that names one is asking to be checked, and honouring that costs
/// nobody anything. But REFUSING A WRITE THAT NAMES NONE is a change to the
/// contract of every existing caller, and the callers do not all move on the
/// same day: team-mdd's own lanes and its orchestrator's proxy batches call
/// `update_nodes` with no basis today, and they learn to pass one when the
/// host's `model.*` tools thread it through and the standing prompt says so.
/// So the requirement rides `SCRYER_REQUIRE_BASIS` (`1`/`true`/`yes`/`on`),
/// OFF by default: a pin bump lands the mechanism without breaking a caller,
/// and the switch turns the gate on in one act once every writer is ready.
/// This is a migration switch, not a project setting — when every caller
/// passes a basis it goes away and the requirement is simply the contract.
pub(crate) fn basis_required() -> bool {
    #[cfg(test)]
    if let Some(forced) = basis_switch_override() {
        return forced;
    }
    // READ ONCE, at the first write of the process. A migration switch is a
    // property of how the engine was started, not something that changes under
    // a running session — and reading it per call made it process-global STATE
    // that a test setting it could leak into whatever ran beside it (two tests
    // went red that way, under nothing but cargo's thread parallelism). The
    // parser is tested on its own strings below; the tests drive the switch
    // through a thread-local, which cannot leak.
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| basis_switch_reads(std::env::var("SCRYER_REQUIRE_BASIS").ok().as_deref()))
}

/// What the switch's environment variable means. `1`, `true`, `yes` and `on`
/// (in any case, with space around them) turn it on; ABSENT and anything else
/// leave it off — a migration switch defaults to the behaviour every existing
/// caller already has.
fn basis_switch_reads(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

#[cfg(test)]
thread_local! {
    /// The tests' handle on the switch: per-thread, so a test that turns it on
    /// cannot refuse a write in the test running beside it.
    static BASIS_SWITCH: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn basis_switch_override() -> Option<bool> {
    BASIS_SWITCH.with(|c| c.get())
}

/// Run `body` with the basis requirement forced on or off, on this thread only.
#[cfg(test)]
pub(crate) fn with_basis_switch<T>(on: bool, body: impl FnOnce() -> T) -> T {
    let prior = BASIS_SWITCH.with(|c| c.replace(Some(on)));
    let out = body();
    BASIS_SWITCH.with(|c| c.set(prior));
    out
}

/// The basis check every model write passes: refuse a write that names none
/// (while the switch is on), and refuse one whose basis no longer matches the
/// relevant set — naming what changed, never merging.
///
/// `Ok` carries the validated read set, so the caller can re-derive a fresh
/// basis over THE SAME SCOPE once its write has landed (resp-0fqnf3): a
/// session's own successive writes against one set must not refuse themselves.
pub(crate) fn check_basis(
    model_ref: &ModelRef,
    basis: Option<&str>,
) -> Result<Option<scryer_core::basis::ReadSet>, String> {
    let Some(token) = basis.map(str::trim).filter(|t| !t.is_empty()) else {
        if basis_required() {
            return Err(
                "REFUSED: this model write names no `basis`, so the engine cannot tell whether                  what you read is still what stands — and a write made on a read set that has                  moved is a lost update. Read the model first (orient, read_model, locate and                  get_pending each answer a `basis`) and pass that value back as `basis`. It is                  opaque: pass it verbatim, do not compose one."
                    .to_string(),
            );
        }
        return Ok(None);
    };
    let committed =
        scryer_core::read_model_at(model_ref).map_err(|e| read_fail("model", model_ref, &e))?;
    let planned = scryer_core::read_planned_at(model_ref).unwrap_or_else(|_| committed.clone());
    scryer_core::basis::check(&committed, &planned, token).map(Some)
}

/// The basis a write ANSWERS: the same scope, re-derived from the model as the
/// write has just left it. `None` where the write named no basis — a caller
/// that did not ask to be checked is told nothing it did not ask for.
pub(crate) fn renewed_basis(
    model_ref: &ModelRef,
    checked: Option<scryer_core::basis::ReadSet>,
) -> Option<String> {
    basis_at(model_ref, checked?.scope)
}

/// Put the renewed basis on a prose answer, the way the tag warnings ride it.
pub(crate) fn say_basis(msg: &mut String, basis: Option<String>) {
    if let Some(b) = basis {
        msg.push_str(&format!("\nbasis: {b}"));
    }
}

/// Put the basis on a JSON answer, beside `project`.
pub(crate) fn stamp_basis(payload: &mut serde_json::Value, basis: Option<String>) {
    if let (serde_json::Value::Object(o), Some(b)) = (payload, basis) {
        o.insert("basis".into(), serde_json::Value::String(b));
    }
}

#[cfg(test)]
mod basis_switch_tests {
    /// resp-azc2d9's switch, on its own strings — which is the whole of what
    /// the environment variable contributes. Testing it here rather than by
    /// setting the variable is the point: the variable is process-global, and
    /// a test that sets it changes the answer for whatever runs beside it.
    #[test]
    fn the_switch_reads_the_words_that_mean_on_and_nothing_else() {
        for on in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(super::basis_switch_reads(Some(on)), "{on:?} means on");
        }
        for off in ["0", "false", "", " ", "maybe", "2", "off"] {
            assert!(!super::basis_switch_reads(Some(off)), "{off:?} does not");
        }
        assert!(
            !super::basis_switch_reads(None),
            "and absent is off — a migration switch defaults to what every \
             existing caller already does"
        );
    }
}

/// Error text for a failed model/plan read. When there really is NO MODEL yet,
/// steer at the bootstrap path rather than strand the agent with an IO error.
///
/// Whether the model is absent is ASKED, not inferred from the error's wording.
/// It used to be read off `e.contains("os error 2")`, which is true of ANY
/// missing file — a lock, a temp file, a change's piece — so a read that
/// stumbled over something else told the caller to start a model that already
/// existed (judgement 535). The engine now names the file and the act in the
/// error itself, so the guess has nothing left to do and the question has an
/// answer: look.
pub(crate) fn read_fail(layer: &str, model_ref: &ModelRef, e: &str) -> String {
    if !model_ref.model_path().exists() {
        format!(
            "No model exists at {model_ref} yet ({layer} file is absent). Start with \
             `read_codebase` to see the codebase, then build top-down: `add_system` / \
             `add_container` / `fill_container`. If you meant a different project, pass \
             its absolute path as `project`."
        )
    } else {
        format!("Failed to read {layer} at {model_ref}: {e}")
    }
}

/// True when `id` is a server-minted responsibility id (`resp-N`).
fn is_minted_resp_id(id: &str) -> bool {
    scryer_core::is_minted_id(id, "resp")
}

/// Re-mints caller-supplied responsibility ids in raw-write payloads.
///
/// The raw-write tools (`update_nodes`, `update_group`, `replace_subtree`,
/// `replace_groups`, `replace_model`) accept full `Responsibility` structs, so a
/// caller that doesn't know the next free id invents one ("new", "", "temp")
/// — and every lookup in the system (`find_responsibility`, source_map, fold)
/// keys on that id being globally unique. Three ways a payload id fails that,
/// each re-minted with a reported reason:
///
///  1. **Invented** — unknown to both layers and not minted-format ("new").
///     Echoing an EXISTING claim keeps its identity whatever its id looks like
///     (legacy models carry ids the minters never issued, and renaming one
///     would orphan its source anchors), and a hand-written `resp-N` that
///     collides with nothing stands.
///  2. **Repeated** — the same id twice in one payload. Never legitimate: the
///     second claim would silently overwrite the first.
///  3. **Wrong host** — an id that lives on a DIFFERENT node/group in either
///     layer. This is the stale-snapshot failure: an agent working from an old
///     read picks `resp-712` for a NEW claim, and because that id is
///     minted-format and known, the write used to sail through and hijack the
///     real resp-712's identity — its directives, anchors, attached tests and
///     change tag — while leaving two claims sharing one id. Only the
///     patch-shaped tools guard on host ([`RespIdReminter::new`]); the
///     whole-payload writers ([`RespIdReminter::for_replacement`]) legitimately
///     relocate claims, so for them a host change is a move, not a collision.
///
/// Seed with BOTH layers (plan and committed), for the same union guard the
/// intent tools use: a plan-deleted claim's id must not be re-issued while
/// committed still holds it — and `enforce_readonly_directives` would staple
/// the dead claim's user directives onto the unrelated new one.
pub(crate) struct RespIdReminter {
    /// Every responsibility id the floor layers already hold → the host ids it
    /// lives on. An incoming id found here is an existing claim being echoed
    /// (never an invention) — and the hosts say whether it is being echoed
    /// where it actually lives.
    known: std::collections::HashMap<String, std::collections::HashSet<String>>,
    /// Ids this payload has already consumed — the within-write duplicate guard.
    used: std::collections::HashSet<String>,
    /// Whether a claim appearing on a host it doesn't live on is a collision
    /// (patch writes) or a legitimate relocation (whole-payload writes).
    guard_host: bool,
    /// One `host: 'old' → new (why)` line per re-mint, for the tool response —
    /// the caller has to learn the real ids it should use from now on, and WHY
    /// its own id was refused.
    minted: Vec<String>,
}

impl RespIdReminter {
    /// For the patch-shaped writers (`update_nodes`, `update_group`,
    /// `replace_groups`), where a claim cannot change host: moving one is
    /// `move_responsibilities`' job, so an id arriving on a foreign host is a
    /// collision.
    pub(crate) fn new(floors: &[&ScryModel]) -> Self {
        Self::build(floors, true)
    }

    /// For the whole-payload writers (`replace_model`, `replace_subtree`), which restate
    /// an entire model or subtree and may legitimately carry a claim to a new
    /// host. Duplicate and invented ids are still re-minted.
    pub(crate) fn for_replacement(floors: &[&ScryModel]) -> Self {
        Self::build(floors, false)
    }

    fn build(floors: &[&ScryModel], guard_host: bool) -> Self {
        let mut me = Self {
            known: std::collections::HashMap::new(),
            used: std::collections::HashSet::new(),
            guard_host,
            minted: Vec::new(),
        };
        for m in floors {
            me.absorb(
                m.nodes
                    .iter()
                    .flat_map(|n| n.responsibilities.iter())
                    .chain(m.groups.iter().flat_map(|g| g.responsibilities.iter())),
            );
            let hosted = m
                .nodes
                .iter()
                .flat_map(|n| n.responsibilities.iter().map(move |r| (&n.id, r)))
                .chain(
                    m.groups
                        .iter()
                        .flat_map(|g| g.responsibilities.iter().map(move |r| (&g.id, r))),
                );
            for (host, r) in hosted {
                me.known
                    .entry(r.id.clone())
                    .or_default()
                    .insert(host.clone());
            }
        }
        me
    }

    /// Reserve every id in `resps` so nothing minted later in this write can
    /// collide with it. Run this over the WHOLE payload before the first
    /// `remint` call: a payload may carry a hand-written id that a fresh draw
    /// would otherwise be free to repeat inside the very write being repaired.
    pub(crate) fn absorb<'a>(&mut self, resps: impl Iterator<Item = &'a Responsibility>) {
        for r in resps {
            self.known.entry(r.id.clone()).or_default();
        }
    }

    /// Re-mint every id in `resps` that cannot safely stand — invented,
    /// repeated within this payload, or (patch writes only) belonging to a
    /// claim that lives on another host. `host` is the node or group being
    /// written; it names the report line and is what the host guard compares
    /// against.
    pub(crate) fn remint<'a>(
        &mut self,
        host: &str,
        resps: impl Iterator<Item = &'a mut Responsibility>,
    ) {
        for r in resps {
            // Ids `absorb` reserved without a host are not "known claims" —
            // only an id with at least one real home is an echo of one.
            let homes = self.known.get(&r.id).filter(|h| !h.is_empty());
            let reason = if self.used.contains(&r.id) {
                Some("repeated in this write")
            } else if homes.is_some_and(|h| self.guard_host && !h.contains(host)) {
                // The stale-snapshot collision: this id is a REAL claim, and it
                // lives somewhere else. Taking it here would hijack that claim.
                Some("that id belongs to a claim on another node")
            } else if homes.is_none() && !is_minted_resp_id(&r.id) {
                Some("ids are server-assigned")
            } else {
                None
            };
            let Some(reason) = reason else {
                self.used.insert(r.id.clone());
                continue;
            };
            let fresh = scryer_core::mint_id_from(
                "resp",
                self.known
                    .keys()
                    .map(String::as_str)
                    .chain(self.used.iter().map(String::as_str)),
            );
            self.minted
                .push(format!("{host}: '{}' → {fresh} ({reason})", r.id));
            self.used.insert(fresh.clone());
            r.id = fresh;
        }
    }

    /// Append the re-mint report to a tool response. Silent when nothing was
    /// re-minted — the common case must not grow a no-op section.
    pub(crate) fn report_into(&self, msg: &mut String) {
        if self.minted.is_empty() {
            return;
        }
        msg.push_str(&format!(
            "\n{} caller-supplied responsibility id(s) re-minted (ids are server-assigned; \
             use the new ids from here on):",
            self.minted.len()
        ));
        for line in &self.minted {
            msg.push_str(&format!("\n- {line}"));
        }
    }
}

/// Re-mint payload NODE ids that name a node living outside the region this
/// write replaces — the node-level twin of [`RespIdReminter`]'s wrong-host
/// guard, and the same stale-snapshot failure: an agent reading the model,
/// then writing a subtree minted against that old snapshot, picks `node-41`
/// for something new while `node-41` has since been taken. `replace_subtree`
/// would push it in beside the real one, leaving two nodes sharing an id —
/// after which every id lookup in the system silently picks whichever comes
/// first.
///
/// `replaced` is the id set this write legitimately owns (the subtree being
/// swapped out); anything else live in either layer is taken. Renamed ids are
/// repointed inside the payload — a child's `parentId` and both endpoints of
/// every payload link — so the incoming shape survives the rename intact.
/// Ids repeated WITHIN the payload are re-minted too, but not repointed: which
/// of the twins a reference meant is not knowable.
pub(crate) fn remint_colliding_node_ids(
    nodes: &mut [Node],
    links: &mut [scryer_core::Link],
    replaced: &std::collections::HashSet<String>,
    floors: &[&ScryModel],
) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    let taken: HashSet<&str> = floors
        .iter()
        .flat_map(|m| m.nodes.iter().map(|n| n.id.as_str()))
        .filter(|id| !replaced.contains(*id))
        .collect();
    // Mint clear of every id either layer has seen AND every id this payload
    // carries, so a fresh id can't collide with a sibling later in the batch.
    let mut reserved: HashSet<String> = floors
        .iter()
        .flat_map(|m| m.nodes.iter().map(|n| n.id.clone()))
        .chain(nodes.iter().map(|n| n.id.clone()))
        .collect();

    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut report = Vec::new();
    for n in nodes.iter_mut() {
        let reason = if !seen.insert(n.id.clone()) {
            "repeated in this write"
        } else if taken.contains(n.id.as_str()) {
            "that id belongs to a node outside this subtree"
        } else {
            continue;
        };
        let fresh = scryer_core::mint_id_from("node", reserved.iter().map(String::as_str));
        reserved.insert(fresh.clone());
        report.push(format!("'{}' → {fresh} ({reason})", n.id));
        // Only a first-seen collision can be repointed; a duplicate's
        // references are ambiguous, so they are left pointing at the twin.
        if reason.starts_with("that id") {
            renamed.insert(n.id.clone(), fresh.clone());
        }
        seen.insert(fresh.clone());
        n.id = fresh;
    }
    if !renamed.is_empty() {
        for n in nodes.iter_mut() {
            if let Some(p) = n.parent_id.as_ref().and_then(|p| renamed.get(p)) {
                n.parent_id = Some(p.clone());
            }
        }
        for l in links.iter_mut() {
            if let Some(s) = renamed.get(&l.src) {
                l.src = s.clone();
            }
            if let Some(d) = renamed.get(&l.dst) {
                l.dst = d.clone();
            }
        }
    }
    report
}

/// Write the plan, first tagging what THIS write changed to the session's
/// current change (see `ScryerServer::session_change`) — computed as the diff
/// between the plan on disk and the model being written, so every authoring
/// tool gets attribution without bespoke bookkeeping, deletions included. No
/// current change (or one this plan's registry doesn't know) writes unfiled,
/// exactly as before.
///
/// A key already tagged by a DIFFERENT, LIVE change is two tasks touching the
/// same element — the collision the ledger exists to catch before the code
/// merges — and the write is REFUSED naming the change that holds the entry
/// (resp-jvz3ge). Only the collisions that name nobody live remain warnings:
/// a dead tag, and a binned holder, whose entries are not pending work.
/// A plan write's answer: the tag warnings, and the basis the write leaves
/// behind for the caller's next write against the same set.
pub(crate) struct TaggedWrite {
    pub warnings: Vec<String>,
    pub basis: Option<String>,
}

pub(crate) fn write_planned_tagged(
    model_ref: &ModelRef,
    model: &mut ScryModel,
    change_id: Option<&str>,
    basis: Option<&str>,
) -> Result<TaggedWrite, String> {
    // The basis check comes FIRST, before the change guard and before anything
    // is written: a write on a read set that has moved is refused whether or
    // not it also forgot to open a change, and a refusal must leave the plan
    // exactly as it found it.
    let checked = check_basis(model_ref, basis)?;

    // The guard: every plan write belongs to a change. A session that has
    // not opened one — or points at a change that has since closed — is told
    // exactly which call fixes that, and nothing is written.
    // A change the BIN holds is not a ledger a write can land in, so the guard
    // reads the OPEN changes: a session whose change was binned under it is
    // told to open or resume one, exactly as a session that opened none is.
    let open: Vec<&scryer_core::changes::ChangeMeta> =
        scryer_core::changes::open_changes(model).collect();
    let Some(cid) = change_id.filter(|c| open.iter().any(|m| m.id == *c)) else {
        let hint = if open.is_empty() {
            "open_change {rationale: \"<the task in one sentence>\"}".to_string()
        } else {
            format!(
                "open_change {{rationale: \"<the task in one sentence>\"}}, or resume one of: {}",
                open.iter()
                    .map(|c| format!("{} (\"{}\")", c.id, c.rationale))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(format!(
            "REFUSED: no change is open in this session, so this plan write has no ledger to \
             land in. Open one first — {hint} — then repeat the write."
        ));
    };
    let mut warnings = Vec::new();
    {
        {
            let before = scryer_core::read_planned_at(model_ref).unwrap_or_default();
            let keys: Vec<String> = scryer_core::diff::diff(&before, model)
                .changes
                .iter()
                .map(scryer_core::changes::key_for)
                .collect();

            // AN ENTRY ANOTHER CHANGE HOLDS IS NOT THIS WRITE'S TO TAKE.
            // Tagging is last-writer-wins, so before this refusal a write that
            // touched a neighbouring change's entry simply re-tagged it to the
            // writer's change and said so in a warning nobody had to read:
            // the entry left the change that was doing the work, and the
            // silence is what made it invisible (judgement 1216 — four silent
            // re-tags and two wiped citation lists in one day, none with an
            // error). Refused HERE, before `tag` mutates anything and before
            // `write_planned` persists, so a refused write leaves the plan
            // exactly as it found it.
            //
            // Not refused: the writer's OWN entries, an untagged entry, an
            // entry whose holder the ledger no longer knows (a dead tag names
            // nobody), and an entry held by a BINNED change — the bin's
            // entries are deliberately not pending work (`diff::open_plan`),
            // so a live change is not blocked by abandoned work. Those still
            // warn, as every collision did.
            let held: Vec<(String, String, String)> = keys
                .iter()
                .filter_map(|k| {
                    let prev = model.change_map.get(k)?;
                    if prev == cid {
                        return None;
                    }
                    let meta = model.changes.iter().find(|c| &c.id == prev)?;
                    meta.binned.is_none().then(|| {
                        (
                            k.clone(),
                            prev.clone(),
                            scryer_core::changes::title_of(meta).to_string(),
                        )
                    })
                })
                .collect();
            if !held.is_empty() {
                let entries = held
                    .iter()
                    .map(|(key, prev, title)| {
                        format!("\n  - {key} is held by {prev} (\"{title}\")")
                    })
                    .collect::<String>();
                let holders: Vec<&str> = held
                    .iter()
                    .map(|(_, prev, _)| prev.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                return Err(format!(
                    "REFUSED: this write would move plan entries that another change already \
                     holds into {cid}, and nothing was written. Two changes editing one entry \
                     is the collision the ledger exists to catch — moving it silently is how \
                     one change's work disappears into another's.{entries}\n\
                     Either resume the change that holds them ({}) and write there, or move \
                     those entries deliberately with refile, then repeat this write.",
                    holders.join(", ")
                ));
            }

            for (key, prev) in scryer_core::changes::tag(model, &keys, cid) {
                let rationale = model
                    .changes
                    .iter()
                    .find(|c| c.id == prev)
                    .map(|c| c.rationale.clone())
                    .unwrap_or_default();
                let binned = model
                    .changes
                    .iter()
                    .any(|c| c.id == prev && c.binned.is_some());
                let bin = if binned { ", which the bin holds" } else { "" };
                warnings.push(format!(
                    "conflict: {key} was tagged by {prev} (\"{rationale}\"){bin} and is now \
                     retagged to {cid} — two changes are touching the same element"
                ));
            }
            // Forward vagrancy: against a SIGNED-OFF change, say which of the
            // touched entries now diverge from what the developer approved.
            // The write succeeds regardless — the agent must be able to record
            // what it did — but the divergence is named here and withheld at
            // the fold.
            if let Some(meta) = model.changes.iter().find(|c| c.id == cid) {
                if meta.signed_off.is_some() {
                    use scryer_core::changes::Classification as C;
                    for (key, class, snap) in
                        scryer_core::changes::classify_against_signoff(model, meta)
                    {
                        if !keys.contains(&key) && class != C::Dropped {
                            continue; // an older divergence, already reported
                        }
                        let what = match class {
                            C::Amended => format!(
                                "AMENDMENT: {key} differs from what was signed off in {cid} \
                                 (approved: \"{}\") — it will not fold; it lands as vagrant for \
                                 the developer's verdict at mark_implemented",
                                snap.as_ref()
                                    .and_then(|s| s.statement.as_deref())
                                    .unwrap_or("?")
                            ),
                            C::Added => format!(
                                "ADDITION: {key} was not in {cid} at sign-off — it will not \
                                 fold; it lands as vagrant for the developer's verdict at \
                                 mark_implemented"
                            ),
                            C::Dropped => format!(
                                "DROPPED: {key} was signed off in {cid} and this plan no longer \
                                 carries it — the fold restores it as pending intent"
                            ),
                            C::Untouched => continue,
                        };
                        warnings.push(what);
                    }
                }
            }
        }
    }
    crate::helpers::write_planned(model_ref, model)?;
    let basis = renewed_basis(model_ref, checked);
    Ok(TaggedWrite { warnings, basis })
}

/// Plan-diff element count with vagrants excluded — the same queue
/// `get_pending` reports. Lives in `scryer_core::diff` so the desktop app's
/// hook endpoint counts identically; re-exported here for the call sites that
/// have always spelled it this way.
pub(crate) use scryer_core::diff::pending_elements as pending_changes;

pub(crate) fn pending_change_count(committed: &ScryModel, planned: &ScryModel) -> usize {
    pending_changes(committed, planned).len()
}

/// The loop-state counts behind every ambient status line — shared by the MCP
/// response headers and the `status`/`statusline` CLI subcommands.
pub(crate) struct StatusCounts {
    /// The agent's implement queue: one per diverging element (a reworded
    /// claim, an added property…), vagrants excluded — the same list
    /// `get_pending` returns. What the agent-facing header reports.
    pub pending: usize,
    /// Plan-change carriers: element diffs folded under the owning node/group,
    /// the number the Changes page and tree lens show. What the human-facing
    /// status line reports, so it agrees with the canvas.
    pub carriers: usize,
    /// Open changes in the plan's ledger — named workstreams in flight.
    pub open_changes: usize,
    /// Testable claims with no test attached, whole-model (health's
    /// `untested` total) — the primary work signal, kept ambient in the
    /// header so the agent sees it move without re-polling get_health.
    pub untested: u32,
    /// None until a reconcile baseline exists — drift and anchor states have
    /// nothing to measure against, and reporting zeros would fake certainty.
    pub baseline: Option<BaselineCounts>,
    /// None until a test report has been ingested — with no recorded verdicts
    /// there is no test state to report, and zeros would fake certainty.
    pub tests: Option<TestCounts>,
    /// Claims whose last probe round left a survivor: the attached test went
    /// green on a deliberate break. Never a count of the UNPROBED — that
    /// population is nearly everything, and a standing nag for it would be
    /// noise the reader learns to skip. Findings speak; absence does not.
    pub probe_survivors: usize,
}

pub(crate) struct BaselineCounts {
    pub drift_scopes: usize,
    pub anchors_changed: usize,
    pub anchors_broken: usize,
}

/// Recorded test-verdict counts. Verified-green is the norm and stays silent
/// in the headers; `failing` (a current verdict that is red — rare, and an
/// alarm precisely because the loop normally fixes red before it lands) and
/// `stale` (verdicts the code has moved past — the standing signal that
/// feeds `get_test_radius`) are what get spoken.
pub(crate) struct TestCounts {
    pub failing: usize,
    pub stale: usize,
    pub recorded: usize,
}

/// Compute [`StatusCounts`] straight from disk. Best-effort: None when there
/// is no committed model to report on. MUST be called with the model lock
/// RELEASED — the anchor check takes the lock itself (it re-anchors
/// moved-but-unchanged symbols in place).
pub(crate) fn status_counts(model_ref: &ModelRef) -> Option<StatusCounts> {
    let committed = scryer_core::read_model_at(model_ref).ok()?;
    let planned = scryer_core::read_planned_at(model_ref).ok()?;
    let pending = pending_change_count(&committed, &planned);
    let carriers = scryer_core::diff::plan_carrier_count(&committed, &planned);
    let open_changes = scryer_core::changes::open_changes(&planned).count();
    let untested = scryer_core::health::compute_health(&committed, Some(&planned), None)
        .totals
        .untested;
    // Recorded test verdicts, re-verified against the tree (fingerprint
    // compare with an mtime fast path — cheap enough for every response).
    let verdicts = scryer_extract::test_status::test_statuses(model_ref).unwrap_or_default();
    let tests = (!verdicts.is_empty()).then(|| TestCounts {
        failing: verdicts
            .iter()
            .filter(|s| {
                !s.stale
                    && matches!(
                        s.outcome,
                        scryer_core::test_results::TestOutcome::Failed
                            | scryer_core::test_results::TestOutcome::Errored
                    )
            })
            .count(),
        stale: verdicts.iter().filter(|s| s.stale).count(),
        recorded: verdicts.len(),
    });
    // A stale probe result describes code that has since moved on, so it is
    // not a live finding — the next round decides.
    let probe_survivors = scryer_extract::test_status::probe_statuses(model_ref)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !p.stale && p.survived > 0)
        .count();
    if !model_ref.sync_path().exists() {
        return Some(StatusCounts {
            pending,
            carriers,
            open_changes,
            untested,
            baseline: None,
            tests,
            probe_survivors,
        });
    }
    let sync = scryer_core::read_sync_state(model_ref);
    let scopes =
        scryer_core::drift::drifted_scopes(&committed, model_ref.project_path(), &sync).len();
    let check = scryer_extract::anchors::check_anchors(model_ref).unwrap_or_default();
    let broken = check
        .observations
        .iter()
        .filter(|o| !matches!(o.state, scryer_extract::anchors::AnchorState::Changed))
        .count();
    let changed = check.observations.len() - broken;
    Some(StatusCounts {
        pending,
        carriers,
        open_changes,
        untested,
        baseline: Some(BaselineCounts {
            drift_scopes: scopes,
            anchors_changed: changed,
            anchors_broken: broken,
        }),
        tests,
        probe_survivors,
    })
}

/// The header/statusline fragment for recorded test verdicts — empty unless
/// something is failing or stale. Verified-green is the norm; the line only
/// speaks when there is work: red left behind, or verdicts the code moved
/// past (run `get_test_radius` for exactly what to re-run).
pub(crate) fn tests_phrase(c: &StatusCounts) -> String {
    match &c.tests {
        Some(t) if t.failing > 0 && t.stale > 0 => {
            format!(" · tests: {} failing, {} stale", t.failing, t.stale)
        }
        Some(t) if t.failing > 0 => format!(" · tests: {} failing", t.failing),
        Some(t) if t.stale > 0 => format!(" · tests: {} stale", t.stale),
        _ => String::new(),
    }
}

/// The header fragment for probe findings — empty unless a probe left a
/// survivor. Deliberately asymmetric with `untested`: a claim that has never
/// been probed is the normal state of almost every claim, so counting it here
/// would bury the one thing worth reacting to. A survivor is different — it
/// says a test the model reports as green does not actually hold its claim.
pub(crate) fn probes_phrase(c: &StatusCounts) -> String {
    match c.probe_survivors {
        0 => String::new(),
        1 => " · probes: 1 claim with a surviving break".to_string(),
        n => format!(" · probes: {n} claims with surviving breaks"),
    }
}

/// One-line loop-state header for write responses — `plan: N pending ·
/// untested: N · drift: N scope(s) · anchors: N changed, N broken` — so the
/// model's state stays ambient across a coding session without the agent
/// re-polling the orientation tools. `untested` (testable claims with no test
/// attached) rides directly after the plan count because it is the primary
/// work signal. Same locking contract as [`status_counts`].
pub(crate) fn status_header(model_ref: &ModelRef) -> Option<String> {
    let c = status_counts(model_ref)?;
    // Open changes ride the header only when the ledger is in use — the
    // serial (unfiled) workflow keeps its unchanged line.
    let changes = if c.open_changes > 0 {
        format!(" · changes: {} open", c.open_changes)
    } else {
        String::new()
    };
    // Test verdicts speak only when red or stale — verified-green is silence.
    let tests = tests_phrase(&c);
    // A surviving break is a finding about a test the model calls green.
    let probes = probes_phrase(&c);
    Some(match c.baseline {
        // Never reconciled: drift/anchors have no baseline to report against.
        None => format!(
            "plan: {} pending · untested: {} · drift: no reconcile anchor yet{tests}{probes}{changes}",
            c.pending, c.untested
        ),
        Some(b) => format!(
            "plan: {} pending · untested: {} · drift: {} scope(s) · anchors: {} changed, {} broken{tests}{probes}{changes}",
            c.pending, c.untested, b.drift_scopes, b.anchors_changed, b.anchors_broken
        ),
    })
}

/// [`status_header`] for a write response, saying when the model written was
/// not the one the caller named — because the caller named none.
///
/// The loop state is only half of "where am I": a session that resolved the
/// wrong project gets a perfectly accurate header about a model it did not
/// mean to touch. A call that named its project already knows the answer and
/// is told nothing; the guessed case is the whole point, and adding it to
/// every write instead would bury it in a line that never changes.
pub(crate) fn status_header_named(project: &ResolvedProject) -> Option<String> {
    let head = status_header(project)?;
    match project.note() {
        Some(note) => Some(format!("{head} · {note}")),
        None => Some(head),
    }
}

/// Which claim-keyed anchor map a batch of entries writes: implementation
/// anchors (`source_map`) or attached tests (`test_map`).
#[derive(Clone, Copy)]
pub(crate) enum RespAnchorDim {
    Source,
    Test,
}

impl RespAnchorDim {
    fn map_of<'m>(
        &self,
        m: &'m mut ScryModel,
    ) -> &'m mut std::collections::BTreeMap<String, Vec<scryer_core::SourceLocation>> {
        match self {
            RespAnchorDim::Source => &mut m.source_map,
            RespAnchorDim::Test => &mut m.test_map,
        }
    }
}

/// Apply responsibility anchor entries (the `entries` shape of
/// `update_source_map`) to their SINGLE home: the committed model owns every
/// committed claim's anchor; the planned draft holds anchors only for claims it
/// ADDS. `dim` picks the dimension — implementation anchors or attached tests —
/// which share the routing and normalization verbatim. Whole-symbol line ranges
/// are normalized to symbol-only anchors (the honest encoding for "this whole
/// definition" — for a test entry, "this whole test"), reported in the
/// returned notes. Mutates the models in place; the CALLER validates ids
/// beforehand and persists both layers afterwards (writing `committed` only
/// when the returned flag is true). Shared by `update_source_map` and
/// `mark_implemented`'s fold-time `anchors`/`tests`.
pub(crate) fn apply_resp_anchor_entries(
    project: &std::path::Path,
    planned: &mut ScryModel,
    committed: &mut Option<ScryModel>,
    mut entries: Vec<crate::types::SourceMapEntry>,
    dim: RespAnchorDim,
) -> (Vec<String>, bool) {
    let mut normalized: Vec<String> = Vec::new();
    {
        let mut resolver = scryer_extract::anchors::ExtentResolver::new(project);
        for entry in &mut entries {
            for loc in &mut entry.locations {
                let (Some(sym), Some(line)) = (loc.symbol.clone(), loc.line) else {
                    continue;
                };
                let end = loc.end_line.unwrap_or(line);
                let Some(extent) = resolver.extent(&loc.pattern, &sym, Some(line)) else {
                    continue;
                };
                if scryer_extract::anchors::covers_extent(line, end, extent) {
                    loc.line = None;
                    loc.end_line = None;
                    normalized.push(format!(
                        "{}: {} L{}-{} covered the whole symbol `{}` (L{}-{})",
                        entry.responsibility_id, loc.pattern, line, end, sym, extent.0, extent.1
                    ));
                }
            }
        }
    }

    let committed_resp_ids: std::collections::HashSet<String> = match committed.as_ref() {
        Some(c) => c
            .nodes
            .iter()
            .flat_map(|n| n.responsibilities.iter())
            .chain(c.groups.iter().flat_map(|g| g.responsibilities.iter()))
            .map(|r| r.id.clone())
            .collect(),
        None => Default::default(),
    };
    let mut committed_dirty = false;
    for entry in entries {
        let key = entry.responsibility_id;
        if entry.locations.is_empty() {
            dim.map_of(planned).remove(&key);
            if committed_resp_ids.contains(&key) {
                if let Some(c) = committed.as_mut() {
                    committed_dirty |= dim.map_of(c).remove(&key).is_some();
                }
            }
        } else if committed_resp_ids.contains(&key) {
            dim.map_of(planned).remove(&key);
            if let Some(c) = committed.as_mut() {
                dim.map_of(c).insert(key, entry.locations);
                committed_dirty = true;
            }
        } else {
            dim.map_of(planned).insert(key, entry.locations);
        }
    }
    (normalized, committed_dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// read_fail decides "there is no model yet" by LOOKING, not by reading the
    /// error's wording. The wording test was true of any missing file, so a
    /// read that stumbled over a lock or a temp file answered with the
    /// bootstrap speech and told the caller to start a model that was already
    /// there (judgement 535).
    #[test]
    fn read_fail_asks_whether_the_model_is_there_rather_than_reading_the_errors_wording() {
        let dir = tempfile::tempdir().unwrap();
        let model_ref = ModelRef::ProjectLocal(dir.path().to_path_buf());

        // No model: the bootstrap answer is the right one.
        let absent = read_fail("model", &model_ref, "could not read x: os error 2");
        assert!(absent.contains("No model exists at"), "{absent}");

        // A model IS there, and something ELSE was missing — the engine's error
        // already names which file and which act, so say it.
        scryer_core::write_model_at(&model_ref, &ScryModel::new()).unwrap();
        let other = format!(
            "could not read {}: No such file or directory (os error 2)",
            model_ref.lock_path().display()
        );
        let said = read_fail("plan", &model_ref, &other);
        assert!(
            !said.contains("No model exists at"),
            "the model is right there: {said}"
        );
        assert!(
            said.contains(&other),
            "the error is passed through whole: {said}"
        );
        assert!(
            said.contains(&model_ref.lock_path().display().to_string()),
            "and it names the file that was actually missing: {said}"
        );
    }
}
