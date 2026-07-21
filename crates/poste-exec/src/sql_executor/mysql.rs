use std::time::Instant;

use poste_core::sql_parser;
use poste_core::{replace_database_in_url, Protocol};
use serde_json::{json, Value};

use super::value;
use super::{build_response, StatementResult};
use crate::response::Response;

pub(super) async fn execute_mysql(parsed: &sql_parser::SqlParseResult) -> anyhow::Result<Response> {
    use sqlx::mysql::{MySqlPoolOptions, MySqlRow};
    use sqlx::{Column, Executor, Row, TypeInfo};

    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&parsed.connection)
        .await?;
    let mut conn = pool.acquire().await?;
    let mut current_url = parsed.connection.clone();

    let mut results = Vec::new();
    let total_start = Instant::now();

    for stmt in &parsed.statements {
        if let Some(db_name) = sql_parser::detect_use_statement(stmt) {
            let safe_db = db_name.replace('`', "``");
            conn.execute(format!("USE `{}`", safe_db).as_str())
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to switch to database '{}': {}", db_name, e)
                })?;
            current_url = replace_database_in_url(&current_url, &db_name);
            continue;
        }

        let stmt_conn = current_url.clone();
        let stmt_result: anyhow::Result<StatementResult> = async {
            let stmt_start = Instant::now();
            let upper = stmt.trim().to_uppercase();

            if upper.starts_with("SELECT")
                || upper.starts_with("WITH")
                || upper.starts_with("EXPLAIN")
                || upper.starts_with("SHOW")
                || upper.starts_with("DESCRIBE")
                || upper.starts_with("DESC ")
                || upper.contains("RETURNING")
            {
                let rows: Vec<MySqlRow> = sqlx::query(stmt).fetch_all(&mut *conn).await?;
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
                            .map(|i| mysql_value_to_json(row, i))
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
                    connection: Some(stmt_conn.clone()),
                    translated_sql: None,
                    original_sql: None,
                })
            } else {
                let result = sqlx::query(stmt).execute(&mut *conn).await?;
                let elapsed = stmt_start.elapsed().as_millis() as u64;

                Ok(StatementResult {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    row_count: 0,
                    affected_rows: Some(result.rows_affected()),
                    execution_time_ms: elapsed,
                    error: None,
                    connection: Some(stmt_conn.clone()),
                    translated_sql: None,
                    original_sql: None,
                })
            }
        }
        .await;

        match stmt_result {
            Ok(mut sr) => {
                sr.connection = Some(current_url.clone());
                results.push(sr);
            }
            Err(e) => {
                results.push(StatementResult {
                    error: Some(format!("{}", e)),
                    connection: Some(current_url.clone()),
                    ..Default::default()
                });
            }
        }
    }

    let total_ms = total_start.elapsed().as_millis() as u64;
    build_response(
        &Protocol::Mysql,
        &parsed.connection,
        &parsed.database,
        results,
        total_ms,
    )
}

fn mysql_value_to_json(row: &sqlx::mysql::MySqlRow, idx: usize) -> Value {
    use sqlx::{Column, Row, TypeInfo, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    let type_name = row.column(idx).type_info().name();

    match type_name {
        "BOOLEAN" => value::opt_json(row.try_get::<Option<bool>, _>(idx).ok().flatten()),
        "TINYINT" => value::opt_json(
            row.try_get::<Option<i8>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "TINYINT UNSIGNED" => value::opt_json(
            row.try_get::<Option<u8>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "SMALLINT" => value::opt_json(
            row.try_get::<Option<i16>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "SMALLINT UNSIGNED" => value::opt_json(
            row.try_get::<Option<u16>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "MEDIUMINT" | "MEDIUMINT UNSIGNED" | "INT" => value::opt_json(
            row.try_get::<Option<i32>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "INT UNSIGNED" => value::opt_json(
            row.try_get::<Option<u32>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "BIGINT" => value::opt_json(row.try_get::<Option<i64>, _>(idx).ok().flatten()),
        "BIGINT UNSIGNED" => {
            if let Ok(Some(v)) = row.try_get::<Option<u64>, _>(idx) {
                json!(v.to_string())
            } else {
                Value::Null
            }
        }
        "FLOAT" => value::opt_json(row.try_get::<Option<f32>, _>(idx).ok().flatten()),
        "DOUBLE" => value::opt_json(row.try_get::<Option<f64>, _>(idx).ok().flatten()),
        "DECIMAL" => {
            let val: Option<rust_decimal::Decimal> = row.try_get::<_, _>(idx).ok().flatten();
            val.map(|v: rust_decimal::Decimal| {
                v.to_string()
                    .parse::<f64>()
                    .map(|n| json!(n))
                    .unwrap_or(json!(v.to_string()))
            })
            .unwrap_or(Value::Null)
        }
        "DATE" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDate>, _>(idx) {
                json!(v.format("%Y-%m-%d").to_string())
            } else if let Ok(Some(v)) =
                row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx)
            {
                json!(v.format("%Y-%m-%d").to_string())
            } else {
                Value::Null
            }
        }
        "DATETIME" => value::opt_json(
            row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v.format("%Y-%m-%d %H:%M:%S%.3f").to_string()),
        ),
        "TIMESTAMP" => value::opt_json(
            row.try_get::<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v.format("%Y-%m-%d %H:%M:%S%.3f").to_string()),
        ),
        "TIME" => value::opt_json(
            row.try_get::<Option<sqlx::types::chrono::NaiveTime>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v.format("%H:%M:%S%.3f").to_string()),
        ),
        "JSON" => {
            if let Ok(Some(json_val)) = row.try_get::<Option<sqlx::types::Json<Value>>, _>(idx) {
                json_val.0
            } else if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
                serde_json::from_str(&s).unwrap_or(json!(s))
            } else if let Ok(Some(b)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                let s = String::from_utf8_lossy(&b);
                serde_json::from_str(&s).unwrap_or(json!(s.to_string()))
            } else {
                Value::Null
            }
        }
        "VARCHAR" | "VAR_STRING" | "STRING" | "CHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT"
        | "LONGTEXT" | "ENUM" | "SET" => {
            value::opt_json(row.try_get::<Option<String>, _>(idx).ok().flatten())
        }
        _ => value::string_fallback(
            row.try_get::<Option<String>, _>(idx).ok().flatten(),
            row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
        ),
    }
}
