//! Store errors are the shared domain errors with a few storage helpers.

use gry_core::{GryError, GryResult};

pub type StoreResult<T> = GryResult<T>;

pub struct StoreError;

impl StoreError {
    pub fn not_found(project: &str) -> GryError {
        GryError::ProjectNotFound(project.to_string())
    }
}
