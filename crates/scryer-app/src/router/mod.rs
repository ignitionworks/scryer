//! The network surface: one endpoint that takes a command by name, and one
//! long-lived stream that carries every event a host needs.

mod http;

pub use http::{router, ActorHeader};
