//! The one command the service names but will not run.
//!
//! Every other command on the surface does its work here. `open_in_editor`
//! cannot: a service has no window, and launching an editor on the machine it
//! runs on would open a file in front of whoever is sitting at that machine —
//! not the caller. A host renders its own file view and links there, which is
//! what upstream's frontend gets when it mounts against a service.

use crate::error::{CommandError, CommandResult};

pub fn open_in_editor(command: &str) -> CommandResult<serde_json::Value> {
    Err(CommandError::NotImplemented {
        command: command.to_string(),
        message: "a service never launches an editor on the machine it runs on; \
                  a host renders its own file view"
            .to_string(),
    })
}
