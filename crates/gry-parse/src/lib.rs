//! Parsing and extraction: tree-sitter grammars plus documentation, config and
//! infrastructure analyzers.

pub mod config;
pub mod docs;
pub mod extract;
pub mod infra;
pub mod lexical;
pub mod model;
pub mod registry;

pub use extract::{extract_file, extract_file_with_previous, sha256};
pub use model::*;
pub use registry::grammar;
