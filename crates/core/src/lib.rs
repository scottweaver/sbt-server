//! Core of sbt-server: everything `sbtd` and `sbtctl` share.
//!
//! `jsonrpc` speaks sbt's own JSON-RPC server protocol (LSP framing, sbt's
//! methods); `routing` maps a working directory to its build root; `paths`
//! locates the daemon's state directory; `protocol` is the agent-facing HTTP
//! contract. Binding constraints live in `.claude/rules/ARCHITECTURE.md`.

pub mod instance;
pub mod jsonrpc;
pub mod paths;
pub mod protocol;
pub mod registry;
pub mod routing;
