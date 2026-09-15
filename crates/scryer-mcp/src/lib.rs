//! Scryer's MCP tool handlers, as a library.
//!
//! The `scryer-mcp` binary is a thin stdio server over this crate: it serves
//! [`ScryerServer`], whose `call_tool` dispatches through [`Engine::call`]. An
//! embedder that wants the tools in-process — an editor, an agent host, a
//! server that fronts several engines — calls the same entry point and gets
//! the same answer, with no child process and no JSON-RPC in between.
//!
//! See `EMBEDDING.md` beside this crate for the embedder's guide.
//!
//! ```no_run
//! let engine = scryer_mcp::Engine::new();
//! let outcome = engine.call("get_health", serde_json::json!({"project": "/tmp/p"}))?;
//! println!("{}", outcome.text);
//! # Ok::<(), scryer_mcp::ToolError>(())
//! ```
//!
//! The handler methods and their request types are public too, for an embedder
//! that knows at compile time which tool it wants:
//!
//! ```no_run
//! use rmcp::handler::server::wrapper::Parameters;
//! use scryer_mcp::types::ReadModelRequest;
//!
//! let engine = scryer_mcp::Engine::new();
//! let result = engine.server().read_model(Parameters(ReadModelRequest {
//!     project: Some("/tmp/p".into()),
//!     node: None,
//!     layer: Default::default(),
//! }));
//! ```

pub mod cli;
mod engine;
mod helpers;
mod hook_client;
pub mod init;
mod instructions;
mod server;
mod tools;
pub mod types;

// Validation lives in scryer-core so the deterministic extractor and any
// orchestrator share one definition of "valid". Re-exported here as
// `crate::validate` so the tool handlers' `use crate::validate;` stays put.
pub use scryer_core::validate;

pub use engine::{catalogue, preamble, tool, Engine, ToolEffect, ToolError, ToolOutcome, ToolSpec};
pub use hook_client::run_hook_client;
pub use instructions::INSTRUCTIONS;
pub use server::ScryerServer;
