use std::time::Instant;

use poste_core::sql_parser;
use poste_core::Protocol;
use serde_json::{json, Value};

use super::{build_response, StatementResult};
use crate::response::Response;
use crate::sql_connection;

pub(super) async fn execute_sqlite(
    parsed: &sql_parser::SqlParseResult,
) -> anyhow::Result<Response> {
    use sqlx::sqlite::{SqlitePoolOptions, SqliteRow};
    use sqlx::{Column, Row, TypeInfo};

    let conn_str = sql_connection::normalize_sqlite_connection(&parsed.connection)?;

    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&conn_str)
        .await
        .map_err(|e| anyhow::anyhow!("SQLite connection failed for '{}': {}", conn_str, e))?;

    let mut results = Vec::new();
    let total_start = Instant::now();

    for stmt in &parsed.statements {
        if sql_parser::detect_use_statement(stmt).is_some() {
            continue;
        }

        let stmt_result: anyhow::Result<StatementResult> = async {
            let stmt_start = Instant::now();
            let upper = stmt.trim().to_uppercase();

            if upper.starts_with("SELECT")
                || upper.starts_with("WITH")
                || upper.starts_with("EXPLAIN")
                || upper.starts_with("PRAGMA")
                || upper.starts_with("VALUES")
                || upper.contains("RETURNING")
            {
                let rows: Vec<SqliteRow> = sqlx::query(stmt).fetch_all(&pool).await?;
                let elapsed = stmt_start.elapsed().as_millis() as u64;

                let columns: Vec<Value> = if let Some(first_row) = rows.first() {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| {
                            json!({
                                "name": col.name(),
                                "type": col.type_info().name(),
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let json_rows: Vec<Vec<Value>> = rows
                    .iter()
                    .map(|row| {
                        (0..row.len())
                            .map(|i| sqlite_value_to_json(row, i))
                            .collect()
                    })
                    .collect();
                let row_count = json_rows.len();

                Ok(StatementResult {
                    columns,
                    rows: json_rows,
                    row_count,
                    affected_rows: None,
                    execution_time_ms: elapsed,
                    error: None,
                    connection: None,
                    translated_sql: None,
                    original_sql: None,
                })
            } else {
                let result = sqlx::query(stmt).execute(&pool).await?;
                let elapsed = stmt_start.elapsed().as_millis() as u64;

                Ok(StatementResult {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    row_count: 0,
                    affected_rows: Some(result.rows_affected()),
                    execution_time_ms: elapsed,
                    error: None,
                    connection: None,
                    translated_sql: None,
                    original_sql: None,
                })
            }
        }
        .await;

        match stmt_result {
            Ok(sr) => results.push(sr),
            Err(e) => {
                results.push(StatementResult {
                    error: Some(format!("{}", e)),
                    ..Default::default()
                });
            }
        }
    }

    pool.close().await;
    let total_ms = total_start.elapsed().as_millis() as u64;
    build_response(
        &Protocol::Sqlite,
        &parsed.connection,
        &parsed.database,
        results,
        total_ms,
    )
}

fn sqlite_value_to_json(row: &sqlx::sqlite::SqliteRow, idx: usize) -> Value {
    use sqlx::{Row, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        return json!(v);
    }

    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        return json!(v);
    }

    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&v) {
            return parsed;
        }
        return json!(v);
    }

    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        return json!(v);
    }

    Value::Null
}
