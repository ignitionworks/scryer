# Embedding the tool handlers

`scryer-mcp` is both a binary and a library. The binary is a thin stdio MCP
server; the library is the tool handlers themselves, so another process — an
editor, an agent host, a second MCP server that fronts several engines — can
call a tool in-process instead of spawning `scryer-mcp` and talking JSON-RPC to
it over a pipe.

The two paths are the same code: `ServerHandler::call_tool` dispatches through
the library's `call`, so a tool answers identically from the engine's own
server and from an embedder.

```toml
[dependencies]
scryer-mcp = { path = "…/crates/scryer-mcp" }   # or a git/version pin
```

## The three things an embedder needs

```rust
use scryer_mcp::{Engine, ToolEffect, ToolError, ToolOutcome, ToolSpec};

// 1. What to advertise.
let tools: &[ToolSpec] = scryer_mcp::catalogue();

// 2. What to say at connect time.
let preamble: &str = scryer_mcp::preamble();

// 3. How to call. One Engine per agent SESSION (see "Session state").
let engine = Engine::new();
let outcome: ToolOutcome = engine.call("read_model", serde_json::json!({
    "project": "/abs/path/to/project",
}))?;
```

## `ToolSpec` — the catalogue

```rust
pub struct ToolSpec {
    /// The tool's name, e.g. `"read_model"`. Stable; this is the wire name.
    pub name: &'static str,
    /// The full description an MCP client sees, ending in its `Rules:` line.
    pub description: String,
    /// The JSON Schema for `arguments`, exactly as `tools/list` advertises it
    /// (generator noise already stripped).
    pub input_schema: serde_json::Value,
    /// The rule slugs the description's trailing `Rules:` line names, already
    /// split. Fetchable with the `get_rules` tool.
    pub rules: Vec<&'static str>,
    /// What the tool does to the model — see `ToolEffect`.
    pub effect: ToolEffect,
}

pub enum ToolEffect {
    /// Reads only; no write to the model, the plan or the verdicts.
    Read,
    /// Writes the plan (the authoring tools: add, update, move, delete,
    /// replace, the change ledger, drift, probes, the source map).
    PlanWrite,
    /// The acts the rules reserve for the user's own word: `sign_off`,
    /// `ingest_test_report` (a verdict) and `mark_implemented` (the fold).
    UserWord,
}
```

`catalogue()` is built once from the crate's own tool router, so a tool this
crate adds, renames or removes appears in it with no second list to edit. The
effect classification is exhaustive: a tool with no entry fails a test in this
crate rather than defaulting to something safe-looking.

`scryer_mcp::tool(name) -> Option<&'static ToolSpec>` looks one up.

## Calling

```rust
impl Engine {
    pub fn new() -> Self;

    /// Call a tool by name with its MCP `arguments` object. `Value::Null` and
    /// an absent object both mean "no arguments".
    pub fn call(&self, name: &str, arguments: serde_json::Value)
        -> Result<ToolOutcome, ToolError>;
}

pub struct ToolOutcome {
    /// The tool ran but reported a failure the agent should read (a missing
    /// node, a refused write). The text says why.
    pub is_error: bool,
    /// Every text content block the tool returned, joined by newlines. Every
    /// tool in this crate returns text; most of it is JSON.
    pub text: String,
    /// The structured result, when the tool set one.
    pub structured: Option<serde_json::Value>,
}

pub enum ToolError {
    /// No such tool. The catalogue is the list.
    UnknownTool(String),
    /// The arguments did not fit the tool's schema.
    BadArguments { tool: String, message: String },
    /// The tool could not run at all (e.g. no project path could be resolved).
    Failed { tool: String, message: String },
}
```

`call` is **synchronous and blocking** — it reads and writes the model on disk.
From an async runtime, call it on a blocking thread (`tokio::task::spawn_blocking`).

`ToolError` converts into an `rmcp::ErrorData` with `From`, for an embedder that
is itself an MCP server.

## Session state

One `Engine` is one agent session. The only state it holds is the change the
session is currently writing into — what `open_change` sets and every plan write
is tagged with. It is deliberately in-memory and per-session: a fresh session
sees the open changes and re-selects rather than inheriting a stale pointer.

```rust
impl Engine {
    /// The project and change id this session is writing into, if any.
    pub fn open_change(&self) -> Option<(PathBuf, String)>;
    /// Set it directly — for an embedder that persists the pointer across a
    /// restart of its own process.
    pub fn set_open_change(&self, value: Option<(PathBuf, String)>);
}
```

`Engine` is `Clone` (clones share the session pointer), `Send` and `Sync`.

## Projects

Every tool takes an optional `project` (an absolute path) and defaults to the
process's working directory. An embedder that serves several projects from one
process should pass `project` on every call rather than relying on the default.

## The binary

`scryer-mcp` with no subcommand serves `ScryerServer` — the same handlers — over
stdio. `scryer_mcp::ScryerServer` is public so an embedder that wants the full
`rmcp::ServerHandler` (its own transport, its own `tools/list`) can serve it
directly instead of going through `Engine`; `Engine::server()` hands out the one
the engine already holds, session state and all.

## The typed path

The handler methods and their request types are public too, so an embedder that
knows at compile time which tool it wants can skip the name and the JSON:

```rust
use rmcp::handler::server::wrapper::Parameters;
use scryer_mcp::types::ReadModelRequest;

let engine = scryer_mcp::Engine::new();
let result = engine.server().read_model(Parameters(ReadModelRequest {
    project: Some("/abs/path".into()),
    node: None,
    layer: Default::default(),
}));
```

`Engine::call` is the path that cannot drift from the advertised catalogue;
this one is the path that cannot drift from the types. Both run the same
handler.
