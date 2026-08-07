use anyhow::Result;
use clap::Parser;
use rust_decimal::prelude::FromPrimitive;
use serde_json::{json, Value};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Parser)]
pub struct SessionArgs {
    #[arg(long)]
    pub connection: String,
    #[arg(long)]
    pub database: Option<String>,
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,
    #[arg(long, default_value_t = 1000)]
    pub max_rows: u64,
}

pub async fn execute(args: SessionArgs) -> Result<()> {
    let mut connection_url = args.connection;
    if let Some(ref db) = args.database {
        connection_url = poste_core::replace_database_in_url(&connection_url, db);
    }

    let protocol = if connection_url.starts_with("sqlite:") {
        poste_core::Protocol::Sqlite
    } else if connection_url.starts_with("mysql://") {
        poste_core::Protocol::Mysql
    } else if connection_url.starts_with("postgres://")
        || connection_url.starts_with("postgresql://")
    {
        poste_core::Protocol::Postgres
    } else {
        anyhow::bail!("Cannot determine protocol: {}", connection_url)
    };

    match protocol {
        poste_core::Protocol::Sqlite => {
            session_sqlite(&connection_url, args.timeout, args.max_rows).await
        }
        poste_core::Protocol::Postgres => {
            session_postgres(&connection_url, args.timeout, args.max_rows).await
        }
        poste_core::Protocol::Mysql => {
            session_mysql(&connection_url, args.timeout, args.max_rows).await
        }
        _ => anyhow::bail!("Not a SQL protocol"),
    }
}

async fn session_sqlite(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use sqlx::{Column, Row, TypeInfo};

    let conn_str = poste_exec::sql_connection::normalize_sqlite_connection(connection_url)?;
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&conn_str)
        .await
        .map_err(|e| anyhow::anyhow!("SQLite connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":format!("JSON parse error: {}", e)});
                stdout.write_all(format!("{}\n", serde_json::to_string(&err)?).as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };
        let seq = req.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
        let sql = req.get("sql").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if sql.is_empty() {
            continue;
        }

        let stmt_start = Instant::now();
        let upper = sql.to_uppercase();
        let result = if upper.starts_with("SELECT")
            || upper.starts_with("WITH")
            || upper.starts_with("EXPLAIN")
            || upper.starts_with("PRAGMA")
            || upper.starts_with("VALUES")
            || upper.contains("RETURNING")
        {
            let fetch = sqlx::query(&sql).fetch_all(&mut *conn);
            let rows: Vec<sqlx::sqlite::SqliteRow> = if timeout_secs > 0 {
                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fetch).await
                {
                    Ok(rows) => rows?,
                    Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                }
            } else {
                fetch.await?
            };
            let elapsed = stmt_start.elapsed().as_millis() as u64;
            let row_count = rows.len() as u64;
            let truncated = max_rows > 0 && row_count > max_rows;
            let display_rows = if max_rows > 0 {
                std::cmp::min(row_count, max_rows) as usize
            } else {
                row_count as usize
            };
            let col_types: Vec<String> = rows
                .first()
                .map(|first_row| {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| col.type_info().name().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let columns: Vec<Value> = rows
                .first()
                .map(|first_row| {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| {
                            json!({"name": col.name(), "type": col.type_info().name()})
                        })
                        .collect()
                })
                .unwrap_or_default();
            let json_rows: Vec<Vec<Value>> = rows
                .iter()
                .take(display_rows)
                .map(|row| {
                    (0..row.len())
                        .map(|i| sqlite_value_to_json(row, i, col_types.get(i).map_or("", |s| s)))
                        .collect()
                })
                .collect();
            json!({
                "type": "result", "seq": seq, "status": "ok",
                "sql": sql, "row_count": row_count,
                "affected_rows": null,
                "execution_time_ms": elapsed, "columns": columns,
                "rows": json_rows, "rows_truncated": truncated,
            })
        } else {
            let exec = sqlx::query(&sql).execute(&mut *conn);
            let result = if timeout_secs > 0 {
                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), exec).await
                {
                    Ok(result) => result?,
                    Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                }
            } else {
                exec.await?
            };
            let affected = result.rows_affected();
            let elapsed = stmt_start.elapsed().as_millis() as u64;
            json!({
                "type": "result", "seq": seq, "status": "ok",
                "sql": sql, "row_count": 0,
                "affected_rows": affected,
                "execution_time_ms": elapsed, "columns": [], "rows": [],
            })
        };
        stdout.write_all(format!("{}\n", serde_json::to_string(&result)?).as_bytes()).await?;
        stdout.flush().await?;
    }

    drop(conn);
    pool.close().await;
    Ok(())
}

async fn session_postgres(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use sqlx::postgres::{PgPoolOptions, PgRow};
    use sqlx::{Column, Row, TypeInfo};

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("PostgreSQL connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":format!("JSON parse error: {}", e)});
                stdout.write_all(format!("{}\n", serde_json::to_string(&err)?).as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };
        let seq = req.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
        let sql = req.get("sql").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if sql.is_empty() {
            continue;
        }

        let stmt_start = Instant::now();
        let upper = sql.to_uppercase();
        let result = if upper.starts_with("SELECT")
            || upper.starts_with("WITH")
            || upper.starts_with("EXPLAIN")
            || upper.starts_with("SHOW")
            || upper.starts_with("TABLE ")
            || upper.starts_with("VALUES")
            || upper.contains("RETURNING")
        {
            let fetch = sqlx::query(&sql).fetch_all(&mut *conn);
            let rows: Vec<PgRow> = if timeout_secs > 0 {
                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fetch).await
                {
                    Ok(rows) => rows?,
                    Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                }
            } else {
                fetch.await?
            };
            let elapsed = stmt_start.elapsed().as_millis() as u64;
            let row_count = rows.len() as u64;
            let truncated = max_rows > 0 && row_count > max_rows;
            let display_rows = if max_rows > 0 {
                std::cmp::min(row_count, max_rows) as usize
            } else {
                row_count as usize
            };
            let col_types: Vec<String> = rows
                .first()
                .map(|first_row| {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| col.type_info().name().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let columns: Vec<Value> = rows
                .first()
                .map(|first_row| {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| {
                            json!({"name": col.name(), "type": col.type_info().name()})
                        })
                        .collect()
                })
                .unwrap_or_default();
            let json_rows: Vec<Vec<Value>> = rows
                .iter()
                .take(display_rows)
                .map(|row| {
                    (0..row.len())
                        .map(|i| pg_value_to_json(row, i, col_types.get(i).map_or("", |s| s)))
                        .collect()
                })
                .collect();
            json!({
                "type": "result", "seq": seq, "status": "ok",
                "sql": sql, "row_count": row_count,
                "affected_rows": null,
                "execution_time_ms": elapsed, "columns": columns,
                "rows": json_rows, "rows_truncated": truncated,
            })
        } else {
            let exec = sqlx::query(&sql).execute(&mut *conn);
            let result = if timeout_secs > 0 {
                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), exec).await
                {
                    Ok(result) => result?,
                    Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                }
            } else {
                exec.await?
            };
            let affected = result.rows_affected();
            let elapsed = stmt_start.elapsed().as_millis() as u64;
            json!({
                "type": "result", "seq": seq, "status": "ok",
                "sql": sql, "row_count": 0,
                "affected_rows": affected,
                "execution_time_ms": elapsed, "columns": [], "rows": [],
            })
        };
        stdout.write_all(format!("{}\n", serde_json::to_string(&result)?).as_bytes()).await?;
        stdout.flush().await?;
    }

    drop(conn);
    pool.close().await;
    Ok(())
}

async fn session_mysql(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use sqlx::mysql::{MySqlPoolOptions, MySqlRow};
    use sqlx::{Column, Row, TypeInfo};

    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("MySQL connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":format!("JSON parse error: {}", e)});
                stdout.write_all(format!("{}\n", serde_json::to_string(&err)?).as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };
        let seq = req.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
        let sql = req.get("sql").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if sql.is_empty() {
            continue;
        }

        let stmt_start = Instant::now();
        let upper = sql.to_uppercase();
        let result = if upper.starts_with("SELECT")
            || upper.starts_with("WITH")
            || upper.starts_with("EXPLAIN")
            || upper.starts_with("SHOW")
            || upper.starts_with("DESCRIBE")
            || upper.starts_with("DESC ")
            || upper.contains("RETURNING")
        {
            let fetch = sqlx::query(&sql).fetch_all(&mut *conn);
            let rows: Vec<MySqlRow> = if timeout_secs > 0 {
                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fetch).await
                {
                    Ok(rows) => rows?,
                    Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                }
            } else {
                fetch.await?
            };
            let elapsed = stmt_start.elapsed().as_millis() as u64;
            let row_count = rows.len() as u64;
            let truncated = max_rows > 0 && row_count > max_rows;
            let display_rows = if max_rows > 0 {
                std::cmp::min(row_count, max_rows) as usize
            } else {
                row_count as usize
            };
            let col_types: Vec<String> = rows
                .first()
                .map(|first_row| {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| col.type_info().name().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let columns: Vec<Value> = rows
                .first()
                .map(|first_row| {
                    first_row
                        .columns()
                        .iter()
                        .map(|col| {
                            json!({"name": col.name(), "type": col.type_info().name()})
                        })
                        .collect()
                })
                .unwrap_or_default();
            let json_rows: Vec<Vec<Value>> = rows
                .iter()
                .take(display_rows)
                .map(|row| {
                    (0..row.len())
                        .map(|i| mysql_value_to_json(row, i, col_types.get(i).map_or("", |s| s)))
                        .collect()
                })
                .collect();
            json!({
                "type": "result", "seq": seq, "status": "ok",
                "sql": sql, "row_count": row_count,
                "affected_rows": null,
                "execution_time_ms": elapsed, "columns": columns,
                "rows": json_rows, "rows_truncated": truncated,
            })
        } else {
            let exec = sqlx::query(&sql).execute(&mut *conn);
            let result = if timeout_secs > 0 {
                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), exec).await
                {
                    Ok(result) => result?,
                    Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                }
            } else {
                exec.await?
            };
            let affected = result.rows_affected();
            let elapsed = stmt_start.elapsed().as_millis() as u64;
            json!({
                "type": "result", "seq": seq, "status": "ok",
                "sql": sql, "row_count": 0,
                "affected_rows": affected,
                "execution_time_ms": elapsed, "columns": [], "rows": [],
            })
        };
        stdout.write_all(format!("{}\n", serde_json::to_string(&result)?).as_bytes()).await?;
        stdout.flush().await?;
    }

    drop(conn);
    pool.close().await;
    Ok(())
}

fn sqlite_value_to_json(row: &sqlx::sqlite::SqliteRow, idx: usize, _col_type: &str) -> Value {
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

fn pg_value_to_json(row: &sqlx::postgres::PgRow, idx: usize, col_type: &str) -> Value {
    use sqlx::{Row, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    let upper = col_type.to_uppercase();
    match upper.as_str() {
        "NUMERIC" => {
            if let Ok(Some(v)) = row.try_get::<Option<rust_decimal::Decimal>, _>(idx) {
                return match v.to_string().parse::<f64>() {
                    Ok(n) if rust_decimal::Decimal::from_f64(n) == Some(v) => json!(n),
                    _ => json!(v.to_string()),
                };
            }
            return Value::Null;
        }
        "DATE" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDate>, _>(idx) {
                return json!(v.format("%Y-%m-%d").to_string());
            }
        }
        "TIMESTAMP" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
                return json!(v.format("%Y-%m-%d %H:%M:%S%.3f").to_string());
            }
        }
        "TIMESTAMPTZ" | "TIMESTAMP WITH TIME ZONE" => {
            if let Ok(Some(v)) = row
                .try_get::<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>, _>(idx)
            {
                let local = v.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string());
            }
        }
        "TIME" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveTime>, _>(idx) {
                return json!(v.format("%H:%M:%S%.3f").to_string());
            }
        }
        "UUID" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::uuid::Uuid>, _>(idx) {
                return json!(v.to_string());
            }
        }
        "INET" | "CIDR" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::ipnetwork::IpNetwork>, _>(idx) {
                return json!(v.to_string());
            }
        }
        "JSON" | "JSONB" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::Json<Value>>, _>(idx) {
                return v.0;
            }
            if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
                return serde_json::from_str(&s).unwrap_or(json!(s));
            }
            return Value::Null;
        }
        _ => {}
    }

    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        if upper == "INT8" || upper == "BIGINT" {
            let max_safe: i64 = 9_007_199_254_740_992;
            if v > -max_safe && v < max_safe {
                return json!(v);
            } else {
                return json!(v.to_string());
            }
        }
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&v) {
            return parsed;
        }
        if upper == "TIMESTAMPTZ" || upper == "TIMESTAMP WITH TIME ZONE" {
            if let Ok(dt) = v.parse::<chrono::DateTime<chrono::Utc>>() {
                let local = dt.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
            }
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&v, "%Y-%m-%d %H:%M:%S%.f") {
                let utc = dt.and_utc();
                let local = utc.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
            }
        }
        return json!(v);
    }
    Value::Null
}

fn mysql_value_to_json(row: &sqlx::mysql::MySqlRow, idx: usize, col_type: &str) -> Value {
    use sqlx::{Row, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    let upper = col_type.to_uppercase();
    match upper.as_str() {
        "DECIMAL" | "DEC" | "NUMERIC" | "FIXED" => {
            if let Ok(Some(v)) = row.try_get::<Option<rust_decimal::Decimal>, _>(idx) {
                return match v.to_string().parse::<f64>() {
                    Ok(n) if rust_decimal::Decimal::from_f64(n) == Some(v) => json!(n),
                    _ => json!(v.to_string()),
                };
            }
            return Value::Null;
        }
        "DATE" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDate>, _>(idx) {
                return json!(v.format("%Y-%m-%d").to_string());
            }
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
                return json!(v.format("%Y-%m-%d").to_string());
            }
        }
        "DATETIME" | "DATETIME2" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
                return json!(v.format("%Y-%m-%d %H:%M:%S%.3f").to_string());
            }
        }
        "TIMESTAMP" => {
            if let Ok(Some(v)) = row
                .try_get::<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>, _>(idx)
            {
                let local = v.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string());
            }
        }
        "TIME" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveTime>, _>(idx) {
                return json!(v.format("%H:%M:%S%.3f").to_string());
            }
        }
        "JSON" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::Json<Value>>, _>(idx) {
                return v.0;
            }
            if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
                return serde_json::from_str(&s).unwrap_or(json!(s));
            }
            if let Ok(Some(b)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                let s = String::from_utf8_lossy(&b);
                return serde_json::from_str(&s).unwrap_or(json!(s.to_string()));
            }
            return Value::Null;
        }
        "BIGINT" => {
            if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
                let max_safe: i64 = 9_007_199_254_740_992;
                if v > -max_safe && v < max_safe {
                    return json!(v);
                } else {
                    return json!(v.to_string());
                }
            }
        }
        "BIGINT UNSIGNED" => {
            if let Ok(Some(v)) = row.try_get::<Option<u64>, _>(idx) {
                return json!(v.to_string());
            }
        }
        _ => {}
    }

    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&v) {
            return parsed;
        }
        if upper == "TIMESTAMP" || upper == "TIMESTAMP WITHOUT TIME ZONE" {
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&v, "%Y-%m-%d %H:%M:%S%.f") {
                let utc = dt.and_utc();
                let local = utc.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
            }
        }
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        let s = String::from_utf8_lossy(&v);
        if let Ok(parsed) = serde_json::from_str::<Value>(&s) {
            return parsed;
        }
        return json!(s.to_string());
    }
    Value::Null
}