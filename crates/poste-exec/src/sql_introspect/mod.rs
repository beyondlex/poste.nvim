//! Database introspection queries.
//!
//! Provides structured introspection of database metadata: databases, schemas,
//! tables, columns, and indexes. Uses the `Dialect` trait from `sql_dialect.rs`
//! for SQL generation and handles per-dialect parameter binding differences.

mod mysql;
mod postgres;
mod sqlite;

use anyhow::Result;
use serde_json::Value;

/// The type of introspection query to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrospectType {
    Databases,
    Schemas,
    Tables,
    Columns,
    Indexes,
    Ddl,
}

impl IntrospectType {
    /// Parse an introspect type from a string.
    pub fn parse_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "databases" => Ok(Self::Databases),
            "schemas" => Ok(Self::Schemas),
            "tables" => Ok(Self::Tables),
            "columns" => Ok(Self::Columns),
            "indexes" => Ok(Self::Indexes),
            "ddl" => Ok(Self::Ddl),
            _ => anyhow::bail!(
                "Unknown introspect type: '{}'. Expected: databases, schemas, tables, columns, indexes, ddl",
                s
            ),
        }
    }

    /// Return the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Databases => "databases",
            Self::Schemas => "schemas",
            Self::Tables => "tables",
            Self::Columns => "columns",
            Self::Indexes => "indexes",
            Self::Ddl => "ddl",
        }
    }
}

/// Parameters for an introspection query.
pub struct IntrospectParams {
    pub connection_url: String,
    pub dialect_name: String,
    pub introspect_type: IntrospectType,
    pub schema: Option<String>,
    pub table: Option<String>,
}

/// Execute an introspection query and return structured JSON.
pub async fn introspect(params: &IntrospectParams) -> Result<Value> {
    match params.dialect_name.as_str() {
        "postgres" => postgres::introspect_postgres(params).await,
        "mysql" => mysql::introspect_mysql(params).await,
        "sqlite" => sqlite::introspect_sqlite(params).await,
        other => anyhow::bail!("Unknown dialect: {}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_introspect_type_from_str() {
        assert_eq!(
            IntrospectType::parse_str("databases").unwrap(),
            IntrospectType::Databases
        );
        assert_eq!(
            IntrospectType::parse_str("SCHEMAS").unwrap(),
            IntrospectType::Schemas
        );
        assert_eq!(
            IntrospectType::parse_str("Tables").unwrap(),
            IntrospectType::Tables
        );
        assert_eq!(
            IntrospectType::parse_str("columns").unwrap(),
            IntrospectType::Columns
        );
        assert_eq!(
            IntrospectType::parse_str("indexes").unwrap(),
            IntrospectType::Indexes
        );
        assert!(IntrospectType::parse_str("invalid").is_err());
        assert!(IntrospectType::parse_str("").is_err());
    }

    #[test]
    fn test_introspect_type_as_str() {
        assert_eq!(IntrospectType::Databases.as_str(), "databases");
        assert_eq!(IntrospectType::Schemas.as_str(), "schemas");
        assert_eq!(IntrospectType::Tables.as_str(), "tables");
        assert_eq!(IntrospectType::Columns.as_str(), "columns");
        assert_eq!(IntrospectType::Indexes.as_str(), "indexes");
        assert_eq!(IntrospectType::Ddl.as_str(), "ddl");
    }

    #[test]
    fn test_introspect_type_ddl_from_str() {
        assert_eq!(
            IntrospectType::parse_str("ddl").unwrap(),
            IntrospectType::Ddl
        );
        assert_eq!(
            IntrospectType::parse_str("DDL").unwrap(),
            IntrospectType::Ddl
        );
    }

    #[tokio::test]
    async fn test_introspect_sqlite_tables() {
        let db_path = "/tmp/poste_test_introspect_tables.db";
        let _ = std::fs::remove_file(db_path);

        std::process::Command::new("sqlite3")
            .args([
                db_path,
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT);",
            ])
            .output()
            .expect("sqlite3 should be available");

        std::process::Command::new("sqlite3")
            .args([
                db_path,
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT);",
            ])
            .output()
            .expect("sqlite3 should be available");

        let params = IntrospectParams {
            connection_url: format!("sqlite:{}", db_path),
            dialect_name: "sqlite".to_string(),
            introspect_type: IntrospectType::Tables,
            schema: None,
            table: None,
        };

        let result = introspect(&params).await.unwrap();
        assert_eq!(result["type"], "introspect");
        assert_eq!(result["introspect_type"], "tables");
        assert_eq!(result["dialect"], "sqlite");

        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "posts");
        assert_eq!(items[1]["name"], "users");

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_introspect_sqlite_columns() {
        let db_path = "/tmp/poste_test_introspect_cols.db";
        let _ = std::fs::remove_file(db_path);

        std::process::Command::new("sqlite3")
            .args([
                db_path,
                "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT DEFAULT 'x');",
            ])
            .output()
            .expect("sqlite3 should be available");

        let params = IntrospectParams {
            connection_url: format!("sqlite:{}", db_path),
            dialect_name: "sqlite".to_string(),
            introspect_type: IntrospectType::Columns,
            schema: None,
            table: Some("t".to_string()),
        };

        let result = introspect(&params).await.unwrap();
        assert_eq!(result["introspect_type"], "columns");

        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["name"], "id");
        assert_eq!(items[0]["pk"], true);
        assert_eq!(items[1]["name"], "name");
        assert_eq!(items[1]["nullable"], false);
        assert_eq!(items[2]["name"], "email");
        assert_eq!(items[2]["default"], "'x'");

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_introspect_sqlite_indexes() {
        let db_path = "/tmp/poste_test_introspect_idx.db";
        let _ = std::fs::remove_file(db_path);

        std::process::Command::new("sqlite3")
            .args([
                db_path,
                "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT); CREATE INDEX idx_name ON t(name);",
            ])
            .output()
            .expect("sqlite3 should be available");

        let params = IntrospectParams {
            connection_url: format!("sqlite:{}", db_path),
            dialect_name: "sqlite".to_string(),
            introspect_type: IntrospectType::Indexes,
            schema: None,
            table: Some("t".to_string()),
        };

        let result = introspect(&params).await.unwrap();
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "idx_name");

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_introspect_sqlite_databases() {
        let params = IntrospectParams {
            connection_url: "sqlite::memory:".to_string(),
            dialect_name: "sqlite".to_string(),
            introspect_type: IntrospectType::Databases,
            schema: None,
            table: None,
        };

        let result = introspect(&params).await.unwrap();
        let items = result["items"].as_array().unwrap();
        assert!(!items.is_empty());
        assert_eq!(items[0]["name"], "main");
    }

    #[tokio::test]
    async fn test_introspect_sqlite_schemas_empty() {
        let params = IntrospectParams {
            connection_url: "sqlite::memory:".to_string(),
            dialect_name: "sqlite".to_string(),
            introspect_type: IntrospectType::Schemas,
            schema: None,
            table: None,
        };

        let result = introspect(&params).await.unwrap();
        let items = result["items"].as_array().unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_introspect_unknown_dialect() {
        let params = IntrospectParams {
            connection_url: "fake://conn".to_string(),
            dialect_name: "oracle".to_string(),
            introspect_type: IntrospectType::Tables,
            schema: None,
            table: None,
        };

        let result = introspect(&params).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown dialect"));
    }

    #[tokio::test]
    async fn test_introspect_columns_without_table_errors() {
        let params = IntrospectParams {
            connection_url: "sqlite::memory:".to_string(),
            dialect_name: "sqlite".to_string(),
            introspect_type: IntrospectType::Columns,
            schema: None,
            table: None,
        };

        let result = introspect(&params).await;
        assert!(result.is_err());
    }
}
