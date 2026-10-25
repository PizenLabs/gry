//! gry core: shared domain model, identities and low-level algorithms.
//!
//! This crate has no storage, transport or CLI dependencies. Everything here is
//! deterministic and testable in isolation.

pub mod config;
pub mod error;
pub mod fqn;
pub mod language;
pub mod minhash;
pub mod nomic;
pub mod profile;
pub mod tokenize;
pub mod types;

pub use config::{Config, RuntimeSettings};
pub use error::{GryError, GryResult};
pub use fqn::{
    fqn_compute, fqn_folder, fqn_module, fqn_module_dir, project_name_from_path,
    resolve_relative_import, sanitize_project_name, FILE_QN_NAME,
};
pub use language::{detect, disambiguate, LanguageDef, LanguageKind, LANGUAGES};
pub use types::{Edge, NewEdge, NewNode, Node, Project, Resolution};
