//! SQL Server (TDS) execution via `tiberius` — the first non-sqlx driver.
//!
//! `exec-file` and `session` in poste-cli reuse the connect/query helpers
//! here for their per-driver statement loops, mirroring the sqlx drivers.
//! Transactions work without a dedicated API: the server reports
//! BEGIN/COMMIT/ROLLBACK back as ENVCHANGE tokens and tiberius stores the
//! descriptor, so batches sent afterwards automatically join the transaction.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio_util::compat::TokioAsyncWriteCompatExt;

use super::value;
use super::StatementResult;
use crate::response::Response;
use poste_core::sql_parser;
use poste_core::Protocol;

pub type MssqlClient = tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>;

/// Parse an `mssql://user:pass@host:port/database` URL (the same shape the
/// Lua `resolve_connection_url` and Rust `to_url()` produce) into a tiberius
/// config.
pub fn mssql_url_to_config(url: &str) -> Result<tiberius::Config> {
    let rest = url
        .strip_prefix("mssql://")
        .ok_or_else(|| anyhow!("not an mssql:// URL: {}", url))?;

    let (auth, hostport_db) = match rest.rsplit_once('@') {
        Some((auth, rest)) => (Some(auth), rest),
        None => (None, rest),
    };

    let (hostport, database) = match hostport_db.split_once('/') {
        Some((hp, db)) => (hp, Some(db)),
        None => (hostport_db, None),
    };
    let database = database
        .map(|db| db.split('?').next().unwrap_or("").to_string())
        .unwrap_or_default();

    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(1433)),
        None => (hostport.to_string(), 1433),
    };

    let (user, pass) = match auth {
        Some(auth) => {
            let (u, p) = auth.split_once(':').unwrap_or((auth, ""));
            (
                percent_encoding::percent_decode_str(u)
                    .decode_utf8_lossy()
                    .to_string(),
                percent_encoding::percent_decode_str(p)
                    .decode_utf8_lossy()
                    .to_string(),
            )
        }
        None => ("sa".to_string(), String::new()),
    };

    let mut config = tiberius::Config::new();
    config.host(host);
    config.port(port);
    config.authentication(tiberius::AuthMethod::sql_server(user, pass));
    if !database.is_empty() {
        let db = percent_encoding::percent_decode_str(&database)
            .decode_utf8_lossy()
            .to_string();
        config.database(db);
    }
    // Dev containers present self-signed certs, and the sqlx drivers here run
    // plain TCP — trusting any cert is the matching posture for this tool.
    config.encryption(tiberius::EncryptionLevel::On);
    config.trust_cert();
    Ok(config)
}

pub async fn connect_mssql(url: &str) -> Result<MssqlClient> {
    let config = mssql_url_to_config(url)?;
    let addr = config.get_addr();
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    tcp.set_nodelay(true).ok();
    let client = tiberius::Client::connect(config, tcp.compat_write()).await?;
    Ok(client)
}

/// Statements that return rows on the TDS batch path; everything else is run
/// through the RPC execute path so DONE row counts are captured.
pub fn is_query_stmt(stmt: &str) -> bool {
    let upper = stmt.trim().to_uppercase();
    upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("VALUES")
        || upper.starts_with("EXEC")
}

/// Temp-table DDL (`CREATE TABLE #t`, `SELECT INTO #t`) must run on the batch
/// path: temp tables created inside the RPC prepare/execute scope are dropped
/// when that scope exits and never become visible to the session.
pub fn is_ddl_stmt(stmt: &str) -> bool {
    let upper = stmt.trim().to_uppercase();
    upper.starts_with("CREATE") || upper.starts_with("ALTER") || upper.starts_with("DROP")
}

/// Run one statement and collect (columns, rows). Columns come from the TDS
/// metadata token, so an empty result set still carries column definitions.
pub async fn mssql_query(
    client: &mut MssqlClient,
    sql: &str,
    timeout_secs: u64,
) -> Result<(Vec<Value>, Vec<Vec<Value>>)> {
    let work = async {
        let mut stream = client.simple_query(sql).await?;
        let columns: Vec<Value> = match stream.columns().await? {
            Some(cols) => cols.iter().map(mssql_column_json).collect(),
            None => Vec::new(),
        };
        let rows = stream.into_first_result().await?;
        let json_rows: Vec<Vec<Value>> = rows
            .iter()
            .map(|row| {
                (0..row.len())
                    .map(|i| mssql_value_to_json(row, i))
                    .collect()
            })
            .collect();
        Ok((columns, json_rows))
    };
    let outcome = if timeout_secs > 0 {
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), work).await {
            Ok(inner) => inner,
            Err(_) => return Err(anyhow!("Query timed out after {} seconds", timeout_secs)),
        }
    } else {
        work.await
    };
    outcome
}

/// Run one non-query statement, returning the affected row count. DDL goes
/// through the batch path (see `is_ddl_stmt`) so temp tables survive.
pub async fn mssql_execute(client: &mut MssqlClient, sql: &str, timeout_secs: u64) -> Result<u64> {
    if is_ddl_stmt(sql) {
        mssql_batch(client, sql, timeout_secs).await?;
        return Ok(0);
    }
    // T-SQL requires MERGE to be terminated by a semicolon, but the statement
    // splitter strips trailing semicolons — put it back.
    let sql_owned;
    let sql = {
        let upper = sql.trim_start().to_uppercase();
        if upper.starts_with("MERGE") && !sql.trim_end().ends_with(';') {
            sql_owned = format!("{};", sql.trim_end());
            sql_owned.as_str()
        } else {
            sql
        }
    };
    let work = client.execute(sql, &[]);
    let outcome = if timeout_secs > 0 {
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), work).await {
            Ok(inner) => inner,
            Err(_) => return Err(anyhow!("Query timed out after {} seconds", timeout_secs)),
        }
    } else {
        work.await
    }?;
    Ok(outcome.total())
}

/// BEGIN/COMMIT/ROLLBACK must go through the batch path: the server reports
/// the (new/cleared) transaction descriptor back as an ENVCHANGE token.
/// `mssql_query` drains the stream to the end, so the stored descriptor
/// stays current.
pub async fn mssql_batch(client: &mut MssqlClient, sql: &str, timeout_secs: u64) -> Result<()> {
    mssql_query(client, sql, timeout_secs).await?;
    Ok(())
}

fn mssql_column_json(col: &tiberius::Column) -> Value {
    json!({ "name": col.name(), "type": mssql_type_label(&col.column_type()) })
}

fn mssql_type_label(ct: &tiberius::ColumnType) -> &'static str {
    use tiberius::ColumnType as CT;
    match ct {
        CT::Null => "null",
        CT::Bit | CT::Bitn => "bit",
        CT::Int1 => "tinyint",
        CT::Int2 => "smallint",
        CT::Int4 => "int",
        CT::Int8 | CT::Intn => "bigint",
        CT::Float4 => "real",
        CT::Float8 | CT::Floatn => "float",
        CT::Money | CT::Money4 => "money",
        CT::Datetime | CT::Datetime4 | CT::Datetimen => "datetime",
        CT::Datetime2 => "datetime2",
        CT::Daten => "date",
        CT::Timen => "time",
        CT::DatetimeOffsetn => "datetimeoffset",
        CT::Decimaln | CT::Numericn => "decimal",
        CT::Guid => "uniqueidentifier",
        CT::BigVarBin | CT::BigBinary => "varbinary",
        CT::BigVarChar | CT::BigChar => "varchar",
        CT::NVarchar | CT::NChar => "nvarchar",
        CT::Xml => "xml",
        CT::Text => "text",
        CT::NText => "ntext",
        CT::Image => "image",
        CT::Udt => "udt",
        CT::SSVariant => "sql_variant",
    }
}

pub fn mssql_value_to_json(row: &tiberius::Row, idx: usize) -> Value {
    use tiberius::ColumnType as CT;
    let ct = row.columns().get(idx).map(|c| c.column_type());
    match ct {
        Some(CT::Bit | CT::Bitn) => value::opt_json(row.try_get::<bool, _>(idx).ok().flatten()),
        Some(CT::Int1) => {
            value::opt_json(row.try_get::<u8, _>(idx).ok().flatten().map(|v| v as i64))
        }
        Some(CT::Int2) => {
            value::opt_json(row.try_get::<i16, _>(idx).ok().flatten().map(|v| v as i64))
        }
        Some(CT::Int4) => {
            value::opt_json(row.try_get::<i32, _>(idx).ok().flatten().map(|v| v as i64))
        }
        Some(CT::Int8 | CT::Intn) => value::opt_int_json(row.try_get::<i64, _>(idx).ok().flatten()),
        Some(CT::Float4) => value::opt_json(row.try_get::<f32, _>(idx).ok().flatten()),
        Some(CT::Float8 | CT::Floatn) => value::opt_json(row.try_get::<f64, _>(idx).ok().flatten()),
        Some(CT::Decimaln | CT::Numericn) => {
            let v: Option<rust_decimal::Decimal> = row.try_get(idx).ok().flatten();
            v.map(value::decimal_json).unwrap_or(Value::Null)
        }
        Some(CT::Guid) => {
            let v: Option<sqlx::types::Uuid> = row.try_get(idx).ok().flatten();
            v.map(|u| json!(u.to_string())).unwrap_or(Value::Null)
        }
        Some(CT::Daten) => value::date_fallback(
            row.try_get::<chrono::NaiveDate, _>(idx).ok().flatten(),
            row.try_get::<&str, _>(idx)
                .ok()
                .flatten()
                .map(|s| s.to_string()),
            row.try_get::<&[u8], _>(idx)
                .ok()
                .flatten()
                .map(|b| b.to_vec()),
        ),
        Some(CT::Timen) => value::time_fallback(
            row.try_get::<chrono::NaiveTime, _>(idx).ok().flatten(),
            row.try_get::<&str, _>(idx)
                .ok()
                .flatten()
                .map(|s| s.to_string()),
            row.try_get::<&[u8], _>(idx)
                .ok()
                .flatten()
                .map(|b| b.to_vec()),
        ),
        Some(CT::Datetime | CT::Datetime4 | CT::Datetimen | CT::Datetime2) => {
            value::datetime_fallback(
                row.try_get::<chrono::NaiveDateTime, _>(idx).ok().flatten(),
                row.try_get::<&str, _>(idx)
                    .ok()
                    .flatten()
                    .map(|s| s.to_string()),
                row.try_get::<&[u8], _>(idx)
                    .ok()
                    .flatten()
                    .map(|b| b.to_vec()),
            )
        }
        Some(CT::DatetimeOffsetn) => {
            let v: Option<chrono::DateTime<chrono::FixedOffset>> = row.try_get(idx).ok().flatten();
            v.map(|dt| {
                json!(dt
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
                    .to_string())
            })
            .unwrap_or(Value::Null)
        }
        _ => value::string_fallback(
            row.try_get::<&str, _>(idx)
                .ok()
                .flatten()
                .map(|s| s.to_string()),
            row.try_get::<&[u8], _>(idx)
                .ok()
                .flatten()
                .map(|b| b.to_vec()),
        ),
    }
}

pub(super) async fn execute_mssql(
    parsed: &sql_parser::SqlParseResult,
    timeout_secs: u64,
) -> Result<Response> {
    let mut client = connect_mssql(&parsed.connection).await?;

    let mut results = Vec::new();
    let total_start = std::time::Instant::now();

    for stmt in &parsed.statements {
        if sql_parser::detect_use_statement(stmt).is_some() {
            continue;
        }

        let stmt_result: anyhow::Result<StatementResult> = async {
            let stmt_start = std::time::Instant::now();
            let trimmed = stmt.trim();
            if is_query_stmt(trimmed) {
                let (columns, json_rows) = mssql_query(&mut client, trimmed, timeout_secs).await?;
                let elapsed = stmt_start.elapsed().as_millis() as u64;
                Ok(StatementResult {
                    row_count: json_rows.len(),
                    columns,
                    rows: json_rows,
                    affected_rows: None,
                    execution_time_ms: elapsed,
                    error: None,
                    connection: None,
                    translated_sql: None,
                    original_sql: None,
                })
            } else {
                let affected = mssql_execute(&mut client, trimmed, timeout_secs).await?;
                let elapsed = stmt_start.elapsed().as_millis() as u64;
                Ok(StatementResult {
                    affected_rows: Some(affected),
                    execution_time_ms: elapsed,
                    ..Default::default()
                })
            }
        }
        .await;

        match stmt_result {
            Ok(sr) => results.push(sr),
            Err(e) => results.push(StatementResult {
                error: Some(format!("{}", e)),
                ..Default::default()
            }),
        }
    }

    let total_ms = total_start.elapsed().as_millis() as u64;
    super::build_response(
        &Protocol::Mssql,
        &parsed.connection,
        &parsed.database,
        results,
        total_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_to_config() {
        let config = mssql_url_to_config("mssql://sa:P%40ss@localhost:11433/master").unwrap();
        assert_eq!(config.get_addr(), "localhost:11433");

        let config = mssql_url_to_config("mssql://localhost").unwrap();
        assert_eq!(config.get_addr(), "localhost:1433");

        let config = mssql_url_to_config("mssql://alice@db.example.com/blog").unwrap();
        assert_eq!(config.get_addr(), "db.example.com:1433");
    }

    #[test]
    fn test_is_query_stmt() {
        assert!(is_query_stmt("SELECT * FROM users"));
        assert!(is_query_stmt("  with cte as (select 1) select * from cte"));
        assert!(is_query_stmt("VALUES (1)"));
        assert!(is_query_stmt("EXEC dbo.some_proc"));
        assert!(!is_query_stmt("INSERT INTO users VALUES (1)"));
        assert!(!is_query_stmt("UPDATE users SET a = 1"));
        assert!(!is_query_stmt("CREATE TABLE t (a INT)"));
        assert!(!is_query_stmt("BEGIN TRANSACTION"));
    }
}
