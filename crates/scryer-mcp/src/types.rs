use scryer_core::{Responsibility, SchemaProperty, Source, SourceLocation};
use serde::Deserialize;

/// Which layer of the model a read returns.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// The editable draft you author and the canvas shows (default).
    #[default]
    Plan,
    /// The committed model the code is believed to satisfy.
    Committed,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DescopeRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    /// Node ids to remove from the model; the code stays.
    pub node_ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadModelRequest {
    pub project: Option<String>,
    /// Node id to read as a full subtree; omit for the overview down to components.
    pub node: Option<String>,
    /// "plan" (default) or "committed".
    #[serde(default)]
    pub layer: Layer,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LocateRequest {
    pub project: Option<String>,
    /// Project-relative source file to look up.
    pub file: String,
    /// Identifier to narrow to; returns only the claims anchored to that symbol when any are.
    pub symbol: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchModelRequest {
    pub project: Option<String>,
    /// Text to find; space-separated terms must ALL match on the node (name, description, technology, statements, labels).
    pub query: String,
    /// Optional kind filter: "person", "system", "container", "component", or "symbol".
    pub kind: Option<String>,
    /// "plan" (default) or "committed".
    #[serde(default)]
    pub layer: Layer,
}

/// One predicate: a `field`, an `op`, and (except for exists/absent) a `value`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryCondition {
    /// One of kind, name, description, technology (strings); external, empty, vagrant (booleans); responsibilityCount, propertyCount, childCount (numbers).
    pub field: String,
    /// eq, ne (any type); gt, gte, lt, lte (numbers); contains (substring); exists, absent (no value).
    pub op: String,
    /// Value to compare against; omit for exists/absent.
    #[serde(default)]
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryModelRequest {
    pub project: Option<String>,
    /// Predicates that must ALL hold (AND); at least one.
    #[serde(rename = "where", alias = "conditions")]
    pub conditions: Vec<QueryCondition>,
    /// Restrict to the subtree rooted at this node id.
    pub under: Option<String>,
    /// "plan" (default) or "committed".
    #[serde(default)]
    pub layer: Layer,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetPendingRequest {
    pub project: Option<String>,
    /// A change id, or "unfiled", to filter the queue to one task.
    pub change: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenChangeRequest {
    pub project: Option<String>,
    /// Open a NEW change: the task in one sentence, as the dev put it.
    pub rationale: Option<String>,
    /// A short name, at most 80 characters. With `change_id`, names that change.
    pub title: Option<String>,
    /// Resume an EXISTING open change by id instead.
    pub change_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SignOffRequest {
    pub project: Option<String>,
    /// The change to sign off; defaults to the session's current one.
    pub change_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloseChangeRequest {
    pub project: Option<String>,
    /// The open change to close; refused while it has entries unless `drop_entries`.
    pub change_id: String,
    /// Abandon a change that still HAS entries: each is taken back to what the
    /// committed model says. Discards authored intent — ask for it by name.
    #[serde(default)]
    pub drop_entries: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AbandonChangeRequest {
    pub project: Option<String>,
    /// The open change to abandon: its planned entries go with it.
    pub change_id: String,
    /// Why the work is not going to happen. Required: the rationale leaves the
    /// ledger with the change, so one with no reason cannot be read after.
    pub why: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RefileRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    /// Bare ids of pending work to MOVE: node/group (carrier + everything under it), responsibility/link, a change id, or "unfiled".
    pub ids: Vec<String>,
    /// Destination: a change id or "unfiled"; defaults to the session's change.
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetDriftRequest {
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NodeMove {
    pub node_id: String,
    /// The new parent; omit to make the node top-level (system/person only).
    pub new_parent_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveNodesRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub moves: Vec<NodeMove>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetHealthRequest {
    pub project: Option<String>,
    /// Scope the report to one node's subtree; omit for the whole-model summary.
    pub node_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReconcileDriftRequest {
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IngestTestReportRequest {
    pub project: Option<String>,
    /// The JUnit XML report file, absolute or project-relative.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetTestRadiusRequest {
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProbeClaimRequest {
    pub project: Option<String>,
    /// The claim to probe; it needs an attached test with a current passing verdict.
    pub resp_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EndProbeRequest {
    pub project: Option<String>,
    pub resp_id: String,
    /// How many deliberate breaks you tried in total, survivors included.
    pub probes: u32,
    /// One entry per break the test did NOT catch, saying what you changed; empty means every break was caught.
    #[serde(default)]
    pub survivors: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MarkImplementedRequest {
    pub project: Option<String>,
    /// The node whose planned work you implemented; optional when folding only link_ids / group_ids or a change.
    pub node_id: Option<String>,
    /// Fold only these responsibilities (needs node_id); omit to fold everything planned on the node.
    pub responsibility_ids: Option<Vec<String>>,
    /// Fold only these property labels (needs node_id).
    pub property_labels: Option<Vec<String>>,
    /// Link ids to fold; the only way to commit a standalone link change or deletion.
    pub link_ids: Option<Vec<String>>,
    /// Group ids to fold; the only way to commit a standalone group change or deletion.
    pub group_ids: Option<Vec<String>>,
    /// Also commit the node's plan-only ancestors structure-only first (design-first models).
    pub commit_ancestors: Option<bool>,
    /// Fold a testable claim without a passing verdict; recorded as unverified. Never the default.
    pub force: Option<bool>,
    /// Anchor the folded claims in the same call; same shape as update_source_map `entries`.
    pub anchors: Option<Vec<SourceMapEntry>>,
    /// Attach tests to the folded claims in the same call; same shape, `pattern` = test file, `symbol` = test name.
    pub tests: Option<Vec<SourceMapEntry>>,
    /// Fold an ENTIRE change by id, every entry in dependency order; standalone, not with node_id.
    pub change: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OrientRequest {
    pub project: Option<String>,
    /// The task in a few words; give this, `files`, or both.
    pub task: Option<String>,
    /// Project-relative files the task touches.
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRulesRequest {
    /// Rule slug(s) to fetch in full, comma-separated.
    pub id: Option<String>,
    /// Free-text topic matched against titles, tags, and slugs.
    pub topic: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadCodebaseRequest {
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ValidateModelRequest {
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetModelRequest {
    pub project: Option<String>,
    /// The complete model as a JSON string (version, nodes, links, groups).
    pub data: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateGroupItem {
    pub group_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Replacement member ids (2+ children of the group's parent).
    pub member_ids: Option<Vec<String>>,
    /// Replacement responsibilities; empty clears.
    pub responsibilities: Option<Vec<Responsibility>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateGroupRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    /// Groups to patch by id; only fields present change.
    pub items: Vec<UpdateGroupItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateNodeItem {
    pub node_id: String,
    /// Node kind: "person", "system", "container", "component", or "symbol".
    pub kind: Option<String>,
    pub name: Option<String>,
    /// New description; empty string clears.
    pub description: Option<String>,
    /// Short badge naming the stack, a few words; empty string clears.
    pub technology: Option<String>,
    /// Pass false to clear the external marking.
    pub external: Option<bool>,
    /// Full replacement of responsibilities; empty clears. Vagrant claims survive omission.
    pub responsibilities: Option<Vec<Responsibility>>,
    /// Full replacement of a data-shape symbol's fields; empty clears.
    pub properties: Option<Vec<SchemaProperty>>,
    /// New parent node id (reparent).
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveResponsibilityItem {
    pub responsibility_id: String,
    pub from_node_id: String,
    pub to_node_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveResponsibilitiesRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub moves: Vec<MoveResponsibilityItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateNodeRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub nodes: Vec<UpdateNodeItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateClaimRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    /// The node (or group) that holds the claim. Named, not guessed: it is half
    /// the claim's identity, and a mismatch is refused.
    #[schemars(description = "The node (or group) that holds the claim.")]
    pub node_id: String,
    /// The claim's id.
    #[schemars(description = "The responsibility id to reword.")]
    pub claim_id: String,
    /// The claim's new statement; omit to leave it alone.
    #[schemars(description = "New EARS statement; omit to leave the wording alone.")]
    pub statement: Option<String>,
    /// The claim's new concern slug; empty string clears it; omit to leave it.
    #[schemars(description = "New concern slug; \"\" clears it; omit to leave it.")]
    pub concern: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetDirectivesItem {
    /// Node id whose node-level directives (binding its subtree) are replaced; exactly one of node_id / responsibility_id.
    pub node_id: Option<String>,
    /// Responsibility id whose directives are replaced; exactly one of node_id / responsibility_id.
    pub responsibility_id: Option<String>,
    /// Full replacement list of "must"/"never" directives; empty clears.
    pub directives: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetDirectivesRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub items: Vec<SetDirectivesItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetNodeRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub node_id: String,
    /// JSON `{nodes, links}`; nodes are the descendants rooted at node_id, replacing the existing ones.
    pub data: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteNodeRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub node_ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddLinkItem {
    pub src: String,
    pub dst: String,
    /// Short relationship label (max 30 characters).
    pub label: String,
    /// Method/protocol annotation, e.g. REST/JSON.
    pub method: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddLinkRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub links: Vec<AddLinkItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateLinkItem {
    pub link_id: String,
    pub label: Option<String>,
    pub method: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateLinkRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub links: Vec<UpdateLinkItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteLinkRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub link_ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SourceMapEntry {
    pub responsibility_id: String,
    /// Source locations; empty clears the entry.
    pub locations: Vec<SourceLocation>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BoundaryEntry {
    pub node_id: String,
    /// Boundary globs the node owns; empty clears.
    pub sources: Vec<Source>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SchemaSourceEntry {
    pub node_id: String,
    /// Declaration location: `pattern` = file, `symbol` = the type name; empty clears.
    pub locations: Vec<SourceLocation>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateSourceMapRequest {
    pub project: Option<String>,
    #[serde(default)]
    pub entries: Vec<SourceMapEntry>,
    /// Attached tests keyed by responsibility id; `pattern` = test file, `symbol` = test name; empty clears.
    #[serde(default)]
    pub test_entries: Vec<SourceMapEntry>,
    #[serde(default)]
    pub schemas: Vec<SchemaSourceEntry>,
    #[serde(default)]
    pub boundaries: Vec<BoundaryEntry>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetGroupsRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    /// JSON: one group or an array; each with name, memberIds, and parentNodeId (the members' parent).
    pub data: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteGroupRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub group_id: String,
}

// --- Intent write tools ---
//
// These build nodes from INTENT: the agent supplies meaning (name, plain
// responsibility statements, the source location it already has from the
// codebase context), and the tool mints the node id + responsibility ids,
// fixes the kind from the parent, and (for
// symbols) writes the source map. The agent never constructs the JSON shape.

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PersonItem {
    pub name: String,
    pub description: Option<String>,
    /// Responsibility statements, each a plain string or `{statement, concern?}`.
    #[serde(default)]
    pub responsibilities: Vec<StatementInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddPersonRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub items: Vec<PersonItem>,
}

/// System to add at the top level: the one being modeled, or an external it depends on.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SystemItem {
    pub name: String,
    pub description: Option<String>,
    /// Technology badge, mainly for externals; omit for the system being modeled.
    pub technology: Option<String>,
    /// true for a third-party system your system depends on; omit for the system being modeled.
    #[serde(default)]
    pub external: bool,
    /// Responsibility statements, each a plain string or `{statement, concern?}`.
    #[serde(default)]
    pub responsibilities: Vec<StatementInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddSystemRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub items: Vec<SystemItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContainerItem {
    pub parent_id: String,
    pub name: String,
    /// What it IS as software, a short badge (e.g. PostgreSQL 16).
    pub technology: Option<String>,
    pub description: Option<String>,
    /// true for an external/third-party container.
    #[serde(default)]
    pub external: bool,
    /// Responsibility statements, each a plain string or `{statement, concern?}`.
    #[serde(default)]
    pub responsibilities: Vec<StatementInput>,
    /// Project-relative directory the container owns; sets its boundary glob.
    pub boundary_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddContainerRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub items: Vec<ContainerItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ComponentItem {
    pub parent_id: String,
    pub name: String,
    pub description: Option<String>,
    /// Responsibility statements, each a plain string or `{statement, concern?}`.
    #[serde(default)]
    pub responsibilities: Vec<StatementInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddComponentRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub items: Vec<ComponentItem>,
}

/// A group: sibling nodes that ship or package together — a SECONDARY axis, never a substitute for decomposition.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GroupItem {
    /// Id of the node whose children are being grouped.
    pub parent_id: String,
    pub name: String,
    pub description: Option<String>,
    /// Node ids to enclose: 2+ children of parent_id.
    #[serde(default)]
    pub member_ids: Vec<String>,
    /// Unit-level responsibility statements, each a plain string or `{statement, concern?}`.
    #[serde(default)]
    pub responsibilities: Vec<StatementInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddGroupRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub items: Vec<GroupItem>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PropertyInput {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// A responsibility: a plain string or `{statement, concern?, line?, endLine?}` (see statement-ears).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ResponsibilityInput {
    Rich {
        statement: String,
        /// ONE kebab-case concern slug; omit for core domain flow.
        concern: Option<String>,
        line: Option<u32>,
        #[serde(alias = "endLine")]
        end_line: Option<u32>,
    },
    Plain(String),
}

impl ResponsibilityInput {
    pub fn statement(&self) -> &str {
        match self {
            Self::Rich { statement, .. } => statement,
            Self::Plain(s) => s,
        }
    }
    pub fn concern(&self) -> Option<&str> {
        match self {
            Self::Rich { concern, .. } => concern.as_deref(),
            Self::Plain(_) => None,
        }
    }
    pub fn line(&self) -> Option<u32> {
        match self {
            Self::Rich { line, .. } => *line,
            Self::Plain(_) => None,
        }
    }
    pub fn end_line(&self) -> Option<u32> {
        match self {
            Self::Rich { end_line, .. } => *end_line,
            Self::Plain(_) => None,
        }
    }
}

/// A responsibility: a plain string or `{statement, concern?}` (see statement-ears).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum StatementInput {
    Rich {
        statement: String,
        /// ONE kebab-case concern slug; omit for core domain flow.
        concern: Option<String>,
    },
    Plain(String),
}

impl StatementInput {
    pub fn statement(&self) -> &str {
        match self {
            Self::Rich { statement, .. } => statement,
            Self::Plain(s) => s,
        }
    }
    pub fn concern(&self) -> Option<&str> {
        match self {
            Self::Rich { concern, .. } => concern.as_deref(),
            Self::Plain(_) => None,
        }
    }
}

impl From<&str> for StatementInput {
    fn from(s: &str) -> Self {
        Self::Plain(s.to_string())
    }
}

impl From<String> for StatementInput {
    fn from(s: String) -> Self {
        Self::Plain(s)
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolItem {
    pub parent_id: String,
    pub name: String,
    /// Project-relative file the symbol is defined in; the source map is anchored for you.
    pub source_file: String,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    /// Responsibilities, each a plain string or `{statement, line?, endLine?}`.
    #[serde(default)]
    pub responsibilities: Vec<ResponsibilityInput>,
    /// Fields when the symbol declares a data shape, one per field.
    #[serde(default)]
    pub properties: Vec<PropertyInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddSymbolRequest {
    pub project: Option<String>,
    /// The `basis` answered by the read this edit was made against. The engine
    /// refuses a write whose basis no longer matches the relevant set, naming
    /// what changed rather than merging. Opaque — pass it back verbatim.
    #[serde(default)]
    #[schemars(
        description = "The `basis` the read this edit was based on; opaque, pass it verbatim."
    )]
    pub basis: Option<String>,
    pub items: Vec<SymbolItem>,
}

// --- Atomic codebase-to-model generation ---

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProposedComponent {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    /// Responsibilities at the component's C4 altitude — each a plain string or `{statement, concern?}`.
    #[serde(default)]
    pub responsibilities: Vec<StatementInput>,
    /// Symbols owned by this component; code-bearing components need at least one.
    pub symbols: Vec<ProposedSymbol>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProposedSymbol {
    pub key: String,
    pub name: String,
    pub source_file: String,
    pub line: Option<u32>,
    pub end_line: Option<u32>,
    #[serde(default)]
    pub responsibilities: Vec<ResponsibilityInput>,
    #[serde(default)]
    pub properties: Vec<PropertyInput>,
}

/// Optional cross-boundary link the dependency graph cannot infer; endpoints are keys or node ids.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProposedLink {
    pub src: String,
    pub dst: String,
    pub label: String,
    pub method: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProposedGroup {
    pub name: String,
    pub description: Option<String>,
    pub member_keys: Vec<String>,
    /// Unit-level responsibility statements — each a plain string or `{statement, concern?}`.
    #[serde(default)]
    pub responsibilities: Vec<StatementInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CommitContainerModelRequest {
    pub project: Option<String>,
    pub container_id: String,
    pub components: Vec<ProposedComponent>,
    #[serde(default)]
    pub links: Vec<ProposedLink>,
    #[serde(default)]
    pub groups: Vec<ProposedGroup>,
}

/// A behaviour the code has that no responsibility describes — semantic drift.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UndescribedItem {
    pub statement: String,
    pub source_file: String,
    pub symbol: Option<String>,
    pub line: Option<u32>,
    #[serde(alias = "endLine")]
    pub end_line: Option<u32>,
    /// Existing node id to home the finding on; omit to route by symbol/file.
    #[serde(default, alias = "nodeId")]
    pub node_id: Option<String>,
    /// Key of a `newNodes` entry to home the finding on; wins over node_id.
    #[serde(default, alias = "nodeKey")]
    pub node_key: Option<String>,
}

/// A declared field no property describes; lands as a vagrant property.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UndescribedProperty {
    pub label: String,
    /// What the field holds; omit if self-evident.
    #[serde(default)]
    pub description: String,
    pub source_file: String,
    /// Enclosing type name, the anchor that routes the field.
    pub symbol: Option<String>,
    /// Existing node id to home the field on; omit to route by symbol/file.
    #[serde(default, alias = "nodeId")]
    pub node_id: Option<String>,
    /// Key of a `newNodes` entry to home the finding on; wins over node_id.
    #[serde(default, alias = "nodeKey")]
    pub node_key: Option<String>,
}

/// A property whose backing field is gone or changed, addressed by node + label.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StaleProperty {
    pub node_id: String,
    pub label: String,
    pub reason: String,
}

/// A node minted vagrant to home code the model has no node for.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NewNode {
    /// Temporary key unique within this call, referenced by nodeKey / parentKey.
    pub key: String,
    pub kind: String,
    pub name: String,
    /// Existing parent node id; exactly one of parent_id / parent_key.
    #[serde(default, alias = "parentId")]
    pub parent_id: Option<String>,
    /// Key of a shallower node minted in this call; list ancestors first.
    #[serde(default, alias = "parentKey")]
    pub parent_key: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub technology: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StaleResponsibility {
    pub responsibility_id: String,
    pub reason: String,
    /// Corrected wording when the behaviour diverged rather than vanished (see drift-directions).
    #[serde(default, alias = "proposedStatement")]
    pub proposed_statement: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StaleNode {
    pub node_id: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FlagDriftRequest {
    pub project: Option<String>,
    pub node_id: String,
    /// Behaviours no responsibility describes; each lands as a vagrant claim.
    #[serde(default)]
    pub undescribed: Vec<UndescribedItem>,
    /// Nodes to mint (vagrant) to home findings the model has no node for.
    #[serde(default, alias = "newNodes")]
    pub new_nodes: Vec<NewNode>,
    /// Declared fields no property describes; each lands as a vagrant property.
    #[serde(default, alias = "undescribedProperties")]
    pub undescribed_properties: Vec<UndescribedProperty>,
    /// Responsibilities whose code no longer discharges them.
    #[serde(default)]
    pub stale: Vec<StaleResponsibility>,
    /// Properties whose backing field is gone or changed.
    #[serde(default, alias = "staleProperties")]
    pub stale_properties: Vec<StaleProperty>,
    /// Nodes whose backing code is entirely gone.
    #[serde(default, alias = "staleNodes")]
    pub stale_nodes: Vec<StaleNode>,
}
