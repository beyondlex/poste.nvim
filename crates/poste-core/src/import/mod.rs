use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single .http file to be written to disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpFile {
    /// Relative path within the output directory, e.g. "pets/pets.http"
    pub path: String,
    /// Full .http file content
    pub content: String,
}

/// Result of a spec import conversion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportResult {
    pub files: Vec<HttpFile>,
    /// Environment variables extracted from the spec (for env.json)
    pub env_vars: HashMap<String, String>,
    pub warnings: Vec<String>,
}

/// Common trait for spec-to-.http converters.
pub trait SpecImporter {
    fn import(&self, spec_content: &str) -> Result<ImportResult>;
}

// Re-export sub-modules
pub mod openapi;
pub mod postman;
pub mod swagger;

#[cfg(test)]
mod tests;
