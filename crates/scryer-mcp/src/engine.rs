//! The in-process entry point: the tool handlers without a transport.
//!
//! [`Engine::call`] is the ONE call path. The stdio server's `call_tool` goes
//! through it too, so a tool cannot answer one thing to an MCP client and
//! another to an embedder. The catalogue is built from the crate's own tool
//! router, so it never drifts from what `tools/list` advertises.

use crate::server::{slim_schema, ScryerServer};
use crate::types::*;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::ErrorData as McpError;
use std::path::PathBuf;
use std::sync::OnceLock;

/// What a tool does to the model. An embedder that guards its tools reads this
/// rather than keeping its own list of names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffect {
    /// Reads only; no write to the model, the plan or the verdicts.
    Read,
    /// Writes the plan: the authoring tools, the change ledger, drift, probes
    /// and the source map.
    PlanWrite,
    /// The acts the rules reserve for the user's own word — `sign_off`,
    /// `ingest_test_report` (a verdict) and `mark_implemented` (the fold).
    UserWord,
}

/// One tool as this crate offers it.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// The wire name, e.g. `"read_model"`.
    pub name: &'static str,
    /// The full description an MCP client sees, ending in its `Rules:` line.
    pub description: String,
    /// The JSON Schema for `arguments`, exactly as `tools/list` advertises it.
    pub input_schema: serde_json::Value,
    /// The rule slugs the description's trailing `Rules:` line names, split.
    pub rules: Vec<&'static str>,
    /// What the tool does to the model.
    pub effect: ToolEffect,
}

/// What a tool answered. Every tool in this crate returns text; most of it is
/// JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The tool ran but reported a failure the agent should read (a missing
    /// node, a refused write). `text` says why.
    pub is_error: bool,
    /// Every text content block the tool returned, joined by newlines.
    pub text: String,
    /// The structured result, when the tool set one.
    pub structured: Option<serde_json::Value>,
}

/// Why a call could not be made, or could not run at all. A tool that ran and
/// refused reports that in a `ToolOutcome` with `is_error`, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// No such tool. [`catalogue`] is the list.
    UnknownTool(String),
    /// The arguments did not fit the tool's schema.
    BadArguments { tool: String, message: String },
    /// The tool could not run at all (e.g. no project path could be resolved).
    Failed { tool: String, message: String },
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::UnknownTool(name) => write!(f, "unknown tool `{name}`"),
            ToolError::BadArguments { tool, message } => {
                write!(
                    f,
                    "`{tool}`: arguments do not fit the tool's schema: {message}"
                )
            }
            ToolError::Failed { tool, message } => write!(f, "`{tool}`: {message}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<ToolError> for McpError {
    fn from(e: ToolError) -> Self {
        match &e {
            ToolError::UnknownTool(_) => McpError::invalid_params(e.to_string(), None),
            ToolError::BadArguments { .. } => McpError::invalid_params(e.to_string(), None),
            ToolError::Failed { .. } => McpError::internal_error(e.to_string(), None),
        }
    }
}

/// The tool handlers, in-process. One `Engine` is one agent SESSION: the only
/// state it holds is the change that session is writing into.
#[derive(Clone, Default)]
pub struct Engine {
    server: ScryerServer,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            server: ScryerServer::new(),
        }
    }

    /// Call a tool by name with its MCP `arguments` object. `Value::Null` means
    /// "no arguments".
    pub fn call(&self, name: &str, arguments: serde_json::Value) -> Result<ToolOutcome, ToolError> {
        dispatch(&self.server, name, arguments).map(ToolOutcome::from)
    }

    /// The project and change id this session is writing into, if any.
    pub fn open_change(&self) -> Option<(PathBuf, String)> {
        self.server.current_change()
    }

    /// Set the session's open change directly — for an embedder that persists
    /// the pointer across a restart of its own process.
    pub fn set_open_change(&self, value: Option<(PathBuf, String)>) {
        self.server.set_session_change(value);
    }

    /// The `rmcp` server handler behind this engine, for an embedder that
    /// wants to serve it on a transport of its own.
    pub fn server(&self) -> &ScryerServer {
        &self.server
    }
}

impl From<CallToolResult> for ToolOutcome {
    fn from(r: CallToolResult) -> Self {
        let text = r
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        ToolOutcome {
            is_error: r.is_error.unwrap_or(false),
            text,
            structured: r.structured_content,
        }
    }
}

/// The connect-time instructions — what the engine's own server says to an
/// agent on connect. An embedder that fronts these tools says the same.
pub fn preamble() -> &'static str {
    crate::instructions::INSTRUCTIONS
}

/// Every tool, with the description, schema and rule slugs a client sees.
pub fn catalogue() -> &'static [ToolSpec] {
    static CATALOGUE: OnceLock<Vec<ToolSpec>> = OnceLock::new();
    CATALOGUE.get_or_init(build_catalogue)
}

/// One tool by name.
pub fn tool(name: &str) -> Option<&'static ToolSpec> {
    catalogue().iter().find(|t| t.name == name)
}

fn build_catalogue() -> Vec<ToolSpec> {
    let mut specs: Vec<ToolSpec> = ScryerServer::new()
        .advertised_tools()
        .iter()
        .map(|t| {
            let name = handler_name(&t.name).unwrap_or_else(|| {
                panic!(
                    "tool `{}` is advertised but has no in-process handler",
                    t.name
                )
            });
            let description = t.description.as_deref().unwrap_or_default().to_string();
            let mut schema = serde_json::Value::Object((*t.input_schema).clone());
            slim_schema(&mut schema);
            ToolSpec {
                rules: rule_slugs(name, &description),
                effect: effect_of(name),
                name,
                description,
                input_schema: schema,
            }
        })
        .collect();
    specs.sort_by_key(|s| s.name);
    specs
}

/// The slugs a description's trailing `Rules:` line names. Every description in
/// this crate carries one (a test in `server.rs` holds that), so a missing line
/// is a bug, not an empty list to paper over.
fn rule_slugs(name: &'static str, description: &str) -> Vec<&'static str> {
    let Some(last) = description.lines().last() else {
        panic!("tool `{name}`: empty description");
    };
    let Some(rest) = last.strip_prefix("Rules: ") else {
        panic!("tool `{name}`: description has no trailing `Rules:` line");
    };
    rest.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        // The slugs are `&'static str` in scryer_core::rules; resolving through
        // it keeps the catalogue's lifetimes static and proves each one exists.
        .map(|s| {
            scryer_core::rules::get(s)
                .unwrap_or_else(|| panic!("tool `{name}` cites unknown rule slug `{s}`"))
                .slug
        })
        .collect()
}

/// A tool answers from ONE place. Each arm names the request type the handler
/// takes, so the schema the catalogue advertises and the type the call
/// deserialises into cannot disagree.
macro_rules! dispatch_table {
    ($($name:ident => $req:ty),* $(,)?) => {
        /// The handler for `name`, or `None` if this crate has no such tool.
        fn dispatch(
            server: &ScryerServer,
            name: &str,
            arguments: serde_json::Value,
        ) -> Result<CallToolResult, ToolError> {
            let arguments = match arguments {
                serde_json::Value::Null => serde_json::Value::Object(Default::default()),
                other => other,
            };
            let result = match name {
                $(
                    stringify!($name) => {
                        let req: $req = serde_json::from_value(arguments).map_err(|e| {
                            ToolError::BadArguments {
                                tool: name.to_string(),
                                message: e.to_string(),
                            }
                        })?;
                        server.$name(Parameters(req))
                    }
                )*
                _ => return Err(ToolError::UnknownTool(name.to_string())),
            };
            result.map_err(|e| ToolError::Failed {
                tool: name.to_string(),
                message: e.message.to_string(),
            })
        }

        /// The dispatch table's own name for `name`, as a `&'static str`.
        fn handler_name(name: &str) -> Option<&'static str> {
            match name {
                $(stringify!($name) => Some(stringify!($name)),)*
                _ => None,
            }
        }

        /// Every tool this crate dispatches, in table order.
        #[cfg(test)]
        fn dispatched_names() -> Vec<&'static str> {
            vec![$(stringify!($name)),*]
        }
    };
}

dispatch_table! {
    // read.rs
    read_model => ReadModelRequest,
    search_model => SearchModelRequest,
    locate => LocateRequest,
    orient => OrientRequest,
    query_model => QueryModelRequest,
    get_drift => GetDriftRequest,
    get_pending => GetPendingRequest,
    get_rules => GetRulesRequest,
    read_codebase => ReadCodebaseRequest,
    validate_model => ValidateModelRequest,
    get_health => GetHealthRequest,
    // nodes.rs
    replace_model => SetModelRequest,
    update_nodes => UpdateNodeRequest,
    update_claim => UpdateClaimRequest,
    set_directives => SetDirectivesRequest,
    mark_implemented => MarkImplementedRequest,
    move_nodes => MoveNodesRequest,
    replace_subtree => SetNodeRequest,
    delete_nodes => DeleteNodeRequest,
    descope => DescopeRequest,
    move_responsibilities => MoveResponsibilitiesRequest,
    // intent.rs
    add_person => AddPersonRequest,
    add_system => AddSystemRequest,
    add_container => AddContainerRequest,
    add_component => AddComponentRequest,
    add_group => AddGroupRequest,
    add_symbol => AddSymbolRequest,
    flag_drift => FlagDriftRequest,
    reconcile_drift => ReconcileDriftRequest,
    // links.rs
    add_links => AddLinkRequest,
    update_links => UpdateLinkRequest,
    delete_links => DeleteLinkRequest,
    // misc.rs
    update_source_map => UpdateSourceMapRequest,
    replace_groups => SetGroupsRequest,
    update_group => UpdateGroupRequest,
    delete_group => DeleteGroupRequest,
    open_change => OpenChangeRequest,
    sign_off => SignOffRequest,
    close_change => CloseChangeRequest,
    abandon_change => AbandonChangeRequest,
    refile => RefileRequest,
    restore_change => RestoreChangeRequest,
    delete_change_permanently => DeleteChangePermanentlyRequest,
    // generation.rs
    fill_container => CommitContainerModelRequest,
    // testing.rs
    ingest_test_report => IngestTestReportRequest,
    get_test_radius => GetTestRadiusRequest,
    open_probe => ProbeClaimRequest,
    close_probe => EndProbeRequest,
}

/// The dispatch the server handler uses: the same path, in `CallToolResult`
/// terms.
pub(crate) fn call_for_server(
    server: &ScryerServer,
    name: &str,
    arguments: serde_json::Value,
) -> Result<CallToolResult, McpError> {
    dispatch(server, name, arguments).map_err(McpError::from)
}

/// What each tool does to the model. Exhaustive by construction: a tool with no
/// arm here fails `every_tool_has_an_effect`, rather than defaulting to
/// something that looks safe.
fn effect_of(name: &str) -> ToolEffect {
    match name {
        "read_model" | "search_model" | "locate" | "orient" | "query_model" | "get_drift"
        | "get_pending" | "get_rules" | "read_codebase" | "validate_model" | "get_health"
        | "get_test_radius" => ToolEffect::Read,
        "sign_off" | "ingest_test_report" | "mark_implemented" | "delete_change_permanently" => {
            ToolEffect::UserWord
        }
        _ => ToolEffect::PlanWrite,
    }
}

/// The tools `effect_of` names explicitly. Anything else is a plan write; the
/// test below holds that every advertised tool is accounted for on purpose.
#[cfg(test)]
const CLASSIFIED: &[(&str, ToolEffect)] = &[
    ("read_model", ToolEffect::Read),
    ("search_model", ToolEffect::Read),
    ("locate", ToolEffect::Read),
    ("orient", ToolEffect::Read),
    ("query_model", ToolEffect::Read),
    ("get_drift", ToolEffect::Read),
    ("get_pending", ToolEffect::Read),
    ("get_rules", ToolEffect::Read),
    ("read_codebase", ToolEffect::Read),
    ("validate_model", ToolEffect::Read),
    ("get_health", ToolEffect::Read),
    ("get_test_radius", ToolEffect::Read),
    ("sign_off", ToolEffect::UserWord),
    ("ingest_test_report", ToolEffect::UserWord),
    ("mark_implemented", ToolEffect::UserWord),
    ("replace_model", ToolEffect::PlanWrite),
    ("update_nodes", ToolEffect::PlanWrite),
    ("update_claim", ToolEffect::PlanWrite),
    ("set_directives", ToolEffect::PlanWrite),
    ("move_nodes", ToolEffect::PlanWrite),
    ("replace_subtree", ToolEffect::PlanWrite),
    ("delete_nodes", ToolEffect::PlanWrite),
    ("descope", ToolEffect::PlanWrite),
    ("move_responsibilities", ToolEffect::PlanWrite),
    ("add_person", ToolEffect::PlanWrite),
    ("add_system", ToolEffect::PlanWrite),
    ("add_container", ToolEffect::PlanWrite),
    ("add_component", ToolEffect::PlanWrite),
    ("add_group", ToolEffect::PlanWrite),
    ("add_symbol", ToolEffect::PlanWrite),
    ("flag_drift", ToolEffect::PlanWrite),
    ("reconcile_drift", ToolEffect::PlanWrite),
    ("add_links", ToolEffect::PlanWrite),
    ("update_links", ToolEffect::PlanWrite),
    ("delete_links", ToolEffect::PlanWrite),
    ("update_source_map", ToolEffect::PlanWrite),
    ("replace_groups", ToolEffect::PlanWrite),
    ("update_group", ToolEffect::PlanWrite),
    ("delete_group", ToolEffect::PlanWrite),
    ("open_change", ToolEffect::PlanWrite),
    ("close_change", ToolEffect::PlanWrite),
    ("abandon_change", ToolEffect::PlanWrite),
    ("refile", ToolEffect::PlanWrite),
    ("restore_change", ToolEffect::PlanWrite),
    ("delete_change_permanently", ToolEffect::UserWord),
    ("fill_container", ToolEffect::PlanWrite),
    ("open_probe", ToolEffect::PlanWrite),
    ("close_probe", ToolEffect::PlanWrite),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue and the dispatch are two lists that must name the same
    /// tools: a tool added, renamed or removed in one and not the other fails
    /// here rather than going missing from an embedder's surface.
    #[test]
    fn the_catalogue_and_the_dispatch_name_the_same_tools() {
        let mut advertised: Vec<String> = ScryerServer::new()
            .advertised_tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        advertised.sort();
        let mut dispatched: Vec<String> =
            dispatched_names().into_iter().map(String::from).collect();
        dispatched.sort();
        assert_eq!(advertised, dispatched);

        let mut catalogued: Vec<String> = catalogue().iter().map(|s| s.name.to_string()).collect();
        catalogued.sort();
        assert_eq!(catalogued, dispatched);
    }

    /// Every tool's effect is a decision somebody made, not a default. A new
    /// upstream tool lands here before an embedder can guard it wrongly.
    #[test]
    fn every_tool_has_an_effect() {
        let classified: std::collections::HashMap<&str, ToolEffect> =
            CLASSIFIED.iter().copied().collect();
        for spec in catalogue() {
            let declared = classified.get(spec.name).unwrap_or_else(|| {
                panic!(
                    "tool `{}` has no entry in CLASSIFIED — decide what it does to the model",
                    spec.name
                )
            });
            assert_eq!(*declared, spec.effect, "tool `{}`", spec.name);
        }
        for (name, _) in CLASSIFIED {
            assert!(
                catalogue().iter().any(|s| s.name == *name),
                "CLASSIFIED names `{name}`, which is no longer a tool"
            );
        }
    }

    /// Each spec carries the schema and the rule slugs a client sees, so an
    /// embedder can advertise the tools without re-reading the descriptions.
    #[test]
    fn every_spec_carries_its_schema_and_rules() {
        for spec in catalogue() {
            assert!(
                !spec.description.is_empty(),
                "{}: no description",
                spec.name
            );
            assert!(
                spec.input_schema.get("type").is_some(),
                "{}: schema is not an object schema",
                spec.name
            );
            assert!(!spec.rules.is_empty(), "{}: no rule slugs", spec.name);
        }
        let read_model = tool("read_model").expect("read_model is a tool");
        assert_eq!(read_model.effect, ToolEffect::Read);
        assert!(read_model.rules.contains(&"model-layers"));
    }

    #[test]
    fn the_preamble_is_the_servers_own() {
        assert_eq!(preamble(), crate::instructions::INSTRUCTIONS);
    }

    #[test]
    fn an_unknown_tool_is_named_not_guessed() {
        let engine = Engine::new();
        assert_eq!(
            engine.call("reed_model", serde_json::json!({})),
            Err(ToolError::UnknownTool("reed_model".into()))
        );
    }

    #[test]
    fn arguments_that_do_not_fit_the_schema_are_refused() {
        let engine = Engine::new();
        let err = engine
            .call("read_model", serde_json::json!({"node": 7}))
            .unwrap_err();
        assert!(
            matches!(&err, ToolError::BadArguments { tool, .. } if tool == "read_model"),
            "{err:?}"
        );
    }

    /// The in-process call runs the real handler against a real model on disk.
    #[test]
    fn a_call_reads_the_model_in_process() {
        let dir = tempfile::tempdir().unwrap();
        let model_ref = scryer_core::ModelRef::ProjectLocal(dir.path().to_path_buf());
        let mut m = scryer_core::ScryModel::new();
        m.nodes.push(scryer_core::Node {
            id: "node-1".into(),
            kind: scryer_core::Kind::System,
            name: "Acme".into(),
            vagrant: None,
            stale: None,
            parent_id: None,
            external: None,
            technology: None,
            description: None,
            responsibilities: Vec::new(),
            properties: Vec::new(),
            icon: None,
            notes: None,
            position: None,
            directives: Vec::new(),
        });
        scryer_core::write_planned_at(&model_ref, &m).unwrap();

        let engine = Engine::new();
        let outcome = engine
            .call(
                "read_model",
                serde_json::json!({"project": dir.path().to_string_lossy()}),
            )
            .expect("read_model dispatches");
        assert!(!outcome.is_error, "{}", outcome.text);
        assert!(outcome.text.contains("Acme"), "{}", outcome.text);
    }

    /// Null arguments mean an empty object, so a tool whose fields are all
    /// optional can be called with nothing.
    #[test]
    fn null_arguments_are_an_empty_object() {
        let engine = Engine::new();
        let outcome = engine
            .call("get_rules", serde_json::Value::Null)
            .expect("get_rules takes no required argument");
        assert!(!outcome.is_error, "{}", outcome.text);
        assert!(outcome.text.contains("loop-orient"), "{}", outcome.text);
    }

    /// The server handler and an embedder are ONE path: `call_tool` dispatches
    /// through `call_for_server`, so the two cannot answer differently.
    #[test]
    fn the_server_path_and_the_in_process_path_are_the_same_call() {
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"project": dir.path().to_string_lossy()});
        let engine = Engine::new();
        let in_process = engine.call("get_health", args.clone()).unwrap();
        let over_the_server =
            ToolOutcome::from(call_for_server(engine.server(), "get_health", args).unwrap());
        assert_eq!(in_process, over_the_server);
    }

    /// The session's open change is the engine's only state, and it is per
    /// engine: two sessions do not share a pointer.
    #[test]
    fn the_open_change_is_per_engine() {
        let dir = tempfile::tempdir().unwrap();
        let a = Engine::new();
        let b = Engine::new();
        a.set_open_change(Some((dir.path().to_path_buf(), "chg-1".into())));
        assert_eq!(
            a.open_change(),
            Some((dir.path().to_path_buf(), "chg-1".into()))
        );
        assert_eq!(b.open_change(), None);
        let cloned = a.clone();
        assert_eq!(cloned.open_change(), a.open_change());
    }
}
