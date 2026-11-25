//! Semantic code search, as an MCP stdio server for one workspace root or one search per process.

pub mod search;
pub mod server;

pub use server::SemanticServer;
