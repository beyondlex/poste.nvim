use std::time::Instant;

use poste_core::sql_parser;
use poste_core::Protocol;
use serde_json::{json, Value};

use super::value;
use super::{build_response, StatementResult};
use crate::response::Response;

pub(super) fn translate_pg_mysql_compat(stmt: &str) -> Option<(String, String)> {
    let upper = stmt.trim().to_uppercase();
    let trimmed = stmt.trim();

    if upper == "SHOW TABLES" || upper == "SHOW TABLES;" {
        let sql = "\
            SELECT table_name AS \"Table\", table_type AS \"Type\" \
            FROM information_schema.tables \
            WHERE table_schema = 'public' \
            ORDER BY table_name"
            .to_string();
        return Some((sql, trimmed.to_string()));
    }

    if upper.starts_with("DESC ") || upper.starts_with("DESCRIBE ") {
        let (_, rest) = trimmed.split_once(char::is_whitespace)?;
        let table_name = rest
            .trim_end_matches(';')
            .trim_end()
            .trim_start_matches('"')
            .trim_end_matches('"');
        if table_name.is_empty() {
            return None;
        }
        let (schema, table) = if let Some(dot) = table_name.rfind('.') {
            let s = table_name[..dot].trim_matches('"');
            let t = table_name[dot + 1..].trim_matches('"');
            (s.to_string(), t.to_string())
        } else {
            ("public".to_string(), table_name.to_string())
        };
        let schema_escaped = schema.replace('\'', "''");
        let table_escaped = table.replace('\'', "''");
        let sql_inlined = format!(
            "SELECT c.column_name AS \"Column\", c.data_type AS \"Type\", \
             c.is_nullable AS \"Nullable\", c.column_default AS \"Default\", \
             CASE WHEN pk.column_name IS NOT NULL THEN 'PRI' ELSE '' END AS \"Key\" \
             FROM information_schema.columns c \
             LEFT JOIN ( \
               SELECT kcu.column_name \
               FROM information_schema.table_constraints tc \
               JOIN information_schema.key_column_usage kcu \
                 ON tc.constraint_name = kcu.constraint_name \
               WHERE tc.table_schema = '{schema_escaped}' AND tc.table_name = '{table_escaped}' \
                 AND tc.constraint_type = 'PRIMARY KEY' \
             ) pk ON c.column_name = pk.column_name \
             WHERE c.table_schema = '{schema_escaped}' AND c.table_name = '{table_escaped}' \
             ORDER BY c.ordinal_position"
        );
        return Some((sql_inlined, trimmed.to_string()));
    }

    None
}

pub(super) async fn execute_postgres(
    parsed: &sql_parser::SqlParseResult,
) -> anyhow::Result<Response> {
    use sqlx::postgres::{PgPoolOptions, PgRow};
    use sqlx::{Column, Row, TypeInfo};

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&parsed.connection)
        .await?;

    let mut results = Vec::new();
    let total_start = Instant::now();

    for stmt in &parsed.statements {
        if sql_parser::detect_use_statement(stmt).is_some() {
            continue;
        }

        let stmt_result: anyhow::Result<StatementResult> = async {
            let stmt_start = Instant::now();
            let (exec_stmt, translated_sql, original_sql) = match translate_pg_mysql_compat(stmt) {
                Some((translated, original)) => {
                    (translated.clone(), Some(translated), Some(original))
                }
                None => (stmt.clone(), None, None),
            };
            let upper = exec_stmt.trim().to_uppercase();

            if upper.starts_with("SELECT")
                || upper.starts_with("WITH")
                || upper.starts_with("EXPLAIN")
                || upper.starts_with("SHOW")
                || upper.starts_with("TABLE ")
                || upper.contains("RETURNING")
            {
                let rows: Vec<PgRow> = sqlx::query(&exec_stmt).fetch_all(&pool).await?;
                let elapsed = stmt_start.elapsed().as_millis() as u64;

                let columns: Vec<Value> = if let Some(first_row) = rows.first() {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| {
                            json!({
                                "name": col.name(),
                                "type": col.type_info().name(),
                                "nullable": col.type_info().name() != "BOOL",
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let json_rows: Vec<Vec<Value>> = rows
                    .iter()
                    .map(|row| (0..row.len()).map(|i| pg_value_to_json(row, i)).collect())
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
                    translated_sql,
                    original_sql,
                })
            } else {
                let result = sqlx::query(&exec_stmt).execute(&pool).await?;
                let elapsed = stmt_start.elapsed().as_millis() as u64;

                Ok(StatementResult {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    row_count: 0,
                    affected_rows: Some(result.rows_affected()),
                    execution_time_ms: elapsed,
                    error: None,
                    connection: None,
                    translated_sql,
                    original_sql,
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
        &Protocol::Postgres,
        &parsed.connection,
        &parsed.database,
        results,
        total_ms,
    )
}

fn pg_value_to_json(row: &sqlx::postgres::PgRow, idx: usize) -> Value {
    use sqlx::{Column, Row, TypeInfo, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    let type_name = row.column(idx).type_info().name();

    match type_name {
        "BOOL" => value::opt_json(row.try_get::<Option<bool>, _>(idx).ok().flatten()),
        "INT2" => value::opt_json(
            row.try_get::<Option<i16>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "INT4" => value::opt_json(
            row.try_get::<Option<i32>, _>(idx)
                .ok()
                .flatten()
                .map(|v| v as i64),
        ),
        "INT8" => value::opt_json(row.try_get::<Option<i64>, _>(idx).ok().flatten()),
        "FLOAT4" => value::opt_json(row.try_get::<Option<f32>, _>(idx).ok().flatten()),
        "FLOAT8" => value::opt_json(row.try_get::<Option<f64>, _>(idx).ok().flatten()),
        "NUMERIC" => {
            let val: Option<rust_decimal::Decimal> = row.try_get::<_, _>(idx).ok().flatten();
            val.map(|v: rust_decimal::Decimal| {
                v.to_string()
                    .parse::<f64>()
                    .map(|n| json!(n))
                    .unwrap_or(json!(v.to_string()))
            })
            .unwrap_or(Value::Null)
        }
        "DATE" => value::date_fallback(
            row.try_get::<Option<sqlx::types::chrono::NaiveDate>, _>(idx)
                .ok()
                .flatten(),
            row.try_get::<Option<String>, _>(idx).ok().flatten(),
            row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
        ),
        "TIMESTAMP" => value::datetime_fallback(
            row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx)
                .ok()
                .flatten(),
            row.try_get::<Option<String>, _>(idx).ok().flatten(),
            row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
        ),
        "TIMESTAMPTZ" => value::timestamptz_fallback(
            row.try_get::<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>, _>(idx)
                .ok()
                .flatten(),
            row.try_get::<Option<String>, _>(idx).ok().flatten(),
            row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
        ),
        "TIME" => value::time_fallback(
            row.try_get::<Option<sqlx::types::chrono::NaiveTime>, _>(idx)
                .ok()
                .flatten(),
            row.try_get::<Option<String>, _>(idx).ok().flatten(),
            row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
        ),
        "UUID" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::uuid::Uuid>, _>(idx) {
                json!(v.to_string())
            } else {
                value::string_fallback(
                    row.try_get::<Option<String>, _>(idx).ok().flatten(),
                    row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
                )
            }
        }
        "INET" | "CIDR" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::ipnetwork::IpNetwork>, _>(idx) {
                json!(v.to_string())
            } else {
                value::string_fallback(
                    row.try_get::<Option<String>, _>(idx).ok().flatten(),
                    row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
                )
            }
        }
        "JSON" | "JSONB" => {
            if let Ok(Some(json_val)) = row.try_get::<Option<sqlx::types::Json<Value>>, _>(idx) {
                json_val.0
            } else if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
                serde_json::from_str(&s).unwrap_or(json!(s))
            } else {
                Value::Null
            }
        }
        _ => value::string_fallback(
            row.try_get::<Option<String>, _>(idx).ok().flatten(),
            row.try_get::<Option<Vec<u8>>, _>(idx).ok().flatten(),
        ),
    }
}
