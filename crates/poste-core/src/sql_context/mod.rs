//! SQL context detection: tokenizer + cursor-based context analysis.
//!
//! Provides a position-aware SQL tokenizer and context detector for use
//! by the completion system. Unlike heuristic regex matching, this module
//! properly handles string/comment awareness, subquery nesting via paren
//! tracking, and schema-qualified identifiers.
//!
//! # Example
//!
//! ```rust
//! use poste_core::sql_context;
//!
//! let sql = "SELECT * FROM users WHERE users.";
//! // Cursor on the trailing dot (byte offset 31)
//! let result = sql_context::detect_context(sql, 31).unwrap();
//! assert_eq!(result.context_type, sql_context::ContextType::DotColumn {
//!     table: String::from("users"), schema: None
//! });
//! assert_eq!(result.tables[0].name, "users");
//! ```

mod functions;
mod tables;
mod tokenizer;

mod context;
mod detectors;
mod scanner;
mod scope;
mod statements;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use tokenizer::*;

pub use context::{detect_context, detect_context_with_dialect};
pub use statements::{find_all_statement_ranges, find_statement_span};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SqlDialect {
    Generic,
    Postgres,
    MySql,
    Sqlite,
}

impl SqlDialect {
    pub fn name(&self) -> &str {
        match self {
            Self::Generic => "generic",
            Self::Postgres => "postgres",
            Self::MySql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContextType {
    Keyword,
    Table,
    Column,
    DotColumn {
        table: String,
        schema: Option<String>,
    },
    SchemaTable {
        schema: String,
    },
    InsertColumn {
        table: String,
    },
    Connection,
    Database,
    DataType,
    String,
    Comment,
}

impl ContextType {
    pub fn name(&self) -> &str {
        match self {
            Self::Keyword => "keyword",
            Self::Table => "table",
            Self::Column => "column",
            Self::DotColumn { .. } => "dot_column",
            Self::SchemaTable { .. } => "schema_table",
            Self::InsertColumn { .. } => "insert_column",
            Self::Connection => "connection",
            Self::Database => "database",
            Self::DataType => "datatype",
            Self::String => "string",
            Self::Comment => "comment",
        }
    }

    pub fn data(&self) -> Option<String> {
        match self {
            Self::DotColumn { table, .. } => Some(table.clone()),
            Self::SchemaTable { schema } => Some(schema.clone()),
            Self::InsertColumn { table } => Some(table.clone()),
            Self::String | Self::Comment => None,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableRef {
    pub name: String,
    pub alias: Option<String>,
    pub schema: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextResult {
    pub context_type: ContextType,
    pub prefix: String,
    pub tables: Vec<TableRef>,
    pub functions: Vec<&'static str>,
    pub in_string: bool,
    pub in_comment: bool,
}
