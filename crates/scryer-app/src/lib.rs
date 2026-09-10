//! Scryer's command surface as plain functions over `scryer-core`,
//! `scryer-extract` and `scryer-acp`, with an HTTP + SSE router and a headless
//! serve binary around them.
//!
//! The desktop shell (`src-tauri/`) reaches the same core through Tauri
//! commands, in a process that owns a window, one project and one user. This
//! crate is the same surface with none of that: many projects, many hosts, no
//! window. It is ADDITIVE — the shell's command bodies are left exactly as
//! upstream ships them, and where a piece exists only there it is copied here
//! with a comment naming the source file, so each rebase diffs the two.
//!
//! What the service does NOT do: it knows nothing about people, sessions, hubs
//! or teams. An actor is an opaque string it records and never interprets;
//! identity, authorisation and audience belong to whatever host mounts it.

pub mod commands;
pub mod error;
pub mod events;
pub mod hooks;
pub mod router;
pub mod state;

pub use error::{CommandError, CommandResult};
pub use events::{Broadcast, Event, EventSink, NullSink, Touch};
pub use state::{revision, AppState, Project};
