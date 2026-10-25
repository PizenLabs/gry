//! MCP server: tools, prompts and JSON-RPC transport.

pub mod handlers;
pub mod render;
pub mod server;
pub mod tools;

pub use server::Server;
pub use tools::ToolProfile;
