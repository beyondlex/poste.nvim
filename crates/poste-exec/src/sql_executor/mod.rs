//! SQL execution engine for PostgreSQL, MySQL, and SQLite.
//!
//! Uses sqlx for database connectivity and query execution.
//! Returns structured JSON responses compatible with the Lua-side
//! dataset renderer.

mod mysql;
mod postgres;
mod sqlite;
mod value;

use crate::response::Response;
use crate::sql_dialect;
use anyhow::Result;
use poste_core::sql_parser;
use poste_core::{Protocol, Request};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Execute a SQL request. Dispatches to the appropriate database driver
/// based on `request.protocol`.
pub async fn execute_sql(request: &Request) -> Result<Response> {
    let parsed = sql_parser::parse_sql_request(request)?;

    if parsed.statements.is_empty() {
        anyhow::bail!("No SQL statements found");
    }

    if parsed.statements.len() == 1 {
        if let Some(db_name) = sql_parser::detect_use_statement(&parsed.statements[0]) {
            let dialect = sql_dialect::dialect_for(&request.protocol)
                .map(|d| d.name().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let body = serde_json::to_string(&json!({
                "type": "use",
                "database_name": db_name,
                "is_use_statement": true,
                "connection": parsed.connection,
                "dialect": dialect,
            }))?;
            return Ok(make_response(
                &request.protocol,
                &parsed.connection,
                body,
                format!("Context → {}", db_name),
            ));
        }
    }

    match request.protocol {
        Protocol::Postgres => postgres::execute_postgres(&parsed).await,
        Protocol::Mysql => mysql::execute_mysql(&parsed).await,
        Protocol::Sqlite => sqlite::execute_sqlite(&parsed).await,
        _ => anyhow::bail!("Not a SQL protocol: {:?}", request.protocol),
    }
}

fn make_response(
    protocol: &Protocol,
    connection: &str,
    body: String,
    status_text: String,
) -> Response {
    let proto_name = match protocol {
        Protocol::Postgres => "postgres",
        Protocol::Mysql => "mysql",
        Protocol::Sqlite => "sqlite",
        _ => "sql",
    };
    let mut metadata = HashMap::new();
    metadata.insert("dialect".to_string(), proto_name.to_string());

    Response {
        protocol: proto_name.to_string(),
        status: 0,
        status_text,
        latency_ms: 0,
        url: connection.to_string(),
        content_type: "application/json".to_string(),
        headers: Vec::new(),
        body,
        cookies: Vec::new(),
        metadata,
    }
}

/// Result of executing a single SQL statement.
#[derive(Debug, Default)]
struct StatementResult {
    columns: Vec<Value>,
    rows: Vec<Vec<Value>>,
    row_count: usize,
    affected_rows: Option<u64>,
    execution_time_ms: u64,
    error: Option<String>,
    connection: Option<String>,
    translated_sql: Option<String>,
    original_sql: Option<String>,
}

fn build_response(
    protocol: &Protocol,
    connection: &str,
    database: &Option<String>,
    results: Vec<StatementResult>,
    total_ms: u64,
) -> Result<Response> {
    let has_error = results.iter().any(|r| r.error.is_some());
    // A statement is a "query" (SELECT/SHOW/WITH/etc) if affected_rows is None.
    // Mutations (INSERT/UPDATE/DELETE) set affected_rows; 0-row queries still lack it.
    // Exclude error results — they have affected_rows=None but are not queries.
    let is_query = results
        .iter()
        .any(|r| r.affected_rows.is_none() && r.error.is_none());
    let total_rows: usize = results.iter().map(|r| r.row_count).sum();
    let total_affected: u64 = results.iter().filter_map(|r| r.affected_rows).sum();

    let dialect = sql_dialect::dialect_for(protocol)
        .map(|d| d.name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let response_type = if is_query { "resultset" } else { "affected" };

    let json_results: Vec<Value> = results
        .iter()
        .map(|r| {
            let mut obj = json!({
                "columns": r.columns,
                "rows": r.rows,
                "row_count": r.row_count,
                "affected_rows": r.affected_rows,
                "execution_time_ms": r.execution_time_ms,
            });
            if let Some(ref err) = r.error {
                obj["error"] = json!(err);
            }
            if let Some(ref sql) = r.translated_sql {
                obj["translated_sql"] = json!(sql);
            }
            if let Some(ref sql) = r.original_sql {
                obj["original_sql"] = json!(sql);
            }
            if let Some(ref conn) = r.connection {
                obj["connection"] = json!(conn);
            }
            obj
        })
        .collect();

    let mut body_obj = json!({
        "type": response_type,
        "results": json_results,
        "total_results": json_results.len(),
        "total_rows": total_rows,
        "total_affected": total_affected,
        "total_execution_time_ms": total_ms,
        "connection": connection,
        "database": database.clone().unwrap_or_default(),
        "dialect": dialect,
    });
    if has_error {
        body_obj["has_error"] = json!(true);
    }

    let body = serde_json::to_string(&body_obj)?;

    let status_text = if is_query {
        format!(
            "{} row{} returned in {}ms",
            total_rows,
            if total_rows == 1 { "" } else { "s" },
            total_ms
        )
    } else if total_affected > 0 {
        format!(
            "{} row{} affected in {}ms",
            total_affected,
            if total_affected == 1 { "" } else { "s" },
            total_ms
        )
    } else {
        format!("Query OK in {}ms", total_ms)
    };

    Ok(make_response(protocol, connection, body, status_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_in_memory() {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool: sqlx::Pool<sqlx::Sqlite> = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("SELECT 1 as num")
            .fetch_all(&pool)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        pool.close().await;
    }

    #[tokio::test]
    async fn test_sqlite_file_connection() {
        use sqlx::sqlite::SqlitePoolOptions;
        let db_path = "/tmp/poste_test_exec.db";
        std::process::Command::new("sqlite3")
            .args([
                db_path,
                "CREATE TABLE IF NOT EXISTS t (x INT); INSERT OR REPLACE INTO t VALUES (42);",
            ])
            .output()
            .unwrap();

        let url = format!("sqlite:{}", db_path);
        let pool: sqlx::Pool<sqlx::Sqlite> = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(3))
            .connect(&url)
            .await
            .expect(&format!("Failed to connect to {}", url));

        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("SELECT * FROM t")
            .fetch_all(&pool)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        pool.close().await;
        std::fs::remove_file(db_path).ok();
    }

    #[test]
    fn test_build_response_zero_row_select_is_resultset() {
        let protocol = Protocol::Postgres;
        let results = vec![StatementResult {
            columns: vec![],
            rows: vec![],
            row_count: 0,
            affected_rows: None,
            execution_time_ms: 3,
            error: None,
            connection: None,
            translated_sql: None,
            original_sql: None,
        }];
        let resp = build_response(&protocol, "test", &None, results, 3).unwrap();
        let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            body["type"], "resultset",
            "0-row SELECT with affected_rows=None should be resultset"
        );
        assert!(
            resp.status_text.contains("0 rows returned"),
            "status_text should say '0 rows returned', got: {}",
            resp.status_text
        );
    }

    #[test]
    fn test_build_response_insert_is_affected() {
        let protocol = Protocol::Postgres;
        let results = vec![StatementResult {
            columns: vec![],
            rows: vec![],
            row_count: 0,
            affected_rows: Some(1),
            execution_time_ms: 5,
            error: None,
            connection: None,
            translated_sql: None,
            original_sql: None,
        }];
        let resp = build_response(&protocol, "test", &None, results, 5).unwrap();
        let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            body["type"], "affected",
            "INSERT with affected_rows=Some should be affected"
        );
    }
}
