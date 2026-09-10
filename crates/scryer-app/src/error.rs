//! What a command can refuse with.
//!
//! The desktop's commands all return `Result<T, String>`, because a Tauri
//! `invoke` failure only ever reaches a `catch` in the webview. A host over
//! HTTP needs to tell a stale write apart from a missing command apart from a
//! genuine failure, and to act on each differently — so the refusals that a
//! caller can DO something about get their own shapes, and everything else
//! stays the string the desktop would have returned.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CommandError {
    /// No command by that name. Carries the names that do exist, so a caller
    /// with a stale client can see what it should have asked for.
    #[serde(rename_all = "camelCase")]
    UnknownCommand {
        command: String,
        available: Vec<&'static str>,
    },
    /// The arguments did not have the shape the command needs.
    #[serde(rename_all = "camelCase")]
    BadArguments { command: String, message: String },
    /// A plan write named a base revision that is no longer current: somebody
    /// wrote in between. Carries the CURRENT revision so the caller can reload
    /// the plan, re-apply its edit, and write again.
    #[serde(rename_all = "camelCase")]
    StaleRevision { current: String },
    /// The command exists but this build does not serve it yet.
    #[serde(rename_all = "camelCase")]
    NotImplemented { command: String, message: String },
    /// Everything else — the string the desktop command would have returned.
    #[serde(rename_all = "camelCase")]
    Failed { message: String },
}

impl CommandError {
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
        }
    }

    /// The HTTP status a router should answer with. `409 Conflict` for a stale
    /// revision is the one a client is expected to handle by reloading.
    pub fn status(&self) -> u16 {
        match self {
            Self::UnknownCommand { .. } => 404,
            Self::BadArguments { .. } => 400,
            Self::StaleRevision { .. } => 409,
            Self::NotImplemented { .. } => 501,
            Self::Failed { .. } => 500,
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand { command, available } => write!(
                f,
                "no command '{command}'; the service serves: {}",
                available.join(", ")
            ),
            Self::BadArguments { command, message } => write!(f, "{command}: {message}"),
            Self::StaleRevision { current } => write!(
                f,
                "the plan moved on: reload at revision {current}, re-apply, and write again"
            ),
            Self::NotImplemented { command, message } => write!(f, "{command}: {message}"),
            Self::Failed { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::Failed { message }
    }
}

pub type CommandResult<T> = Result<T, CommandError>;
