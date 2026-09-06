//! ClickHouse execution over raw HTTP — the first HTTP transport in
//! poste-exec.
//!
//! The official `clickhouse` crate is SELECT-only: it appends ` FORMAT x`
//! to the SQL text and forces `readonly=1`, so DDL/DML/temp tables/`SET`
//! cannot run through it. Instead we POST the statement directly and ask
//! for the `JSON` output format — its `meta` carries column names and
//! types, and DDL responses come back as plain `OK`/empty, so one code
//! path handles both result sets and side-effect statements.
//!
//! Sessions keep temp tables alive via the `session_id` query parameter.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::StatementResult;
use crate::response::Response;
use poste_core::sql_parser;
use poste_core::Protocol;

pub struct ClickHouseClient {
    http: reqwest::Client,
    base_url: String,
    user: String,
    password: String,
    database: String,
    session_id: Option<String>,
}

/// Outcome of one statement POST.
pub struct ChResponse {
    /// None when the server answered `OK`/empty (DDL/DML, no result set).
    pub columns: Option<Vec<Value>>,
    pub rows: Vec<Vec<Value>>,
    /// `written_rows` from the `x-clickhouse-summary` response header.
    pub written_rows: u64,
}

/// Parse `clickhouse://user:pass@host:8123/database`. The scheme implies
/// http; https servers are out of scope for now (dev-tool posture).
pub fn clickhouse_url_to_config(url: &str) -> Result<(String, String, String, String)> {
    let rest = url
        .strip_prefix("clickhouse://")
        .ok_or_else(|| anyhow!("not a clickhouse:// URL: {}", url))?;

    let (auth, hostport_db) = match rest.rsplit_once('@') {
        Some((auth, rest)) => (Some(auth), rest),
        None => (None, rest),
    };
    let (hostport, database) = match hostport_db.split_once('/') {
        Some((hp, db)) => (hp, db.split('?').next().unwrap_or("")),
        None => (hostport_db, ""),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(8123)),
        None => (hostport.to_string(), 8123),
    };

    let (user, password) = match auth {
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
        None => ("default".to_string(), String::new()),
    };
    let database = percent_encoding::percent_decode_str(database)
        .decode_utf8_lossy()
        .to_string();

    Ok((
        format!("http://{}:{}", host, port),
        user,
        password,
        database,
    ))
}

pub async fn connect_clickhouse(url: &str) -> Result<ClickHouseClient> {
    let (base_url, user, password, database) = clickhouse_url_to_config(url)?;
    let http = reqwest::Client::builder().user_agent("poste").build()?;
    Ok(ClickHouseClient {
        http,
        base_url,
        user,
        password,
        database,
        session_id: None,
    })
}

/// Sessions keep temp tables and `SET` state alive via `session_id`.
pub fn with_session_id(mut client: ClickHouseClient, id: impl Into<String>) -> ClickHouseClient {
    client.session_id = Some(id.into());
    client
}

/// The database the client is bound to (namespace for introspection).
pub fn database(client: &ClickHouseClient) -> &str {
    &client.database
}

/// POST one statement. Non-2xx responses become errors carrying the
/// server message; resultset responses parse `meta`/`data` from the JSON
/// output format.
pub async fn clickhouse_post(
    client: &ClickHouseClient,
    sql: &str,
    timeout_secs: u64,
) -> Result<ChResponse> {
    let mut params: Vec<(&str, &str)> = vec![("default_format", "JSON")];
    if !client.database.is_empty() {
        params.push(("database", client.database.as_str()));
    }
    if let Some(sid) = &client.session_id {
        params.push(("session_id", sid));
    }

    let mut req = client
        .http
        .post(format!("{}/", client.base_url))
        .basic_auth(&client.user, Some(&client.password))
        .query(&params)
        .body(sql.to_string());
    if timeout_secs > 0 {
        req = req.timeout(std::time::Duration::from_secs(timeout_secs));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("ClickHouse request failed: {}", e))?;
    let status = resp.status();
    let summary_header = resp.headers().get("x-clickhouse-summary").cloned();
    let text = resp.text().await?;

    if !status.is_success() {
        let first = text.lines().next().unwrap_or("").to_string();
        return Err(anyhow!("ClickHouse error ({}): {}", status, first));
    }

    let mut written_rows = 0u64;
    if let Some(header) = summary_header {
        if let Ok(v) = header.to_str() {
            if let Ok(summary) = serde_json::from_str::<Value>(v) {
                written_rows = summary
                    .get("written_rows")
                    .and_then(|r| r.as_u64())
                    .unwrap_or(0);
            }
        }
    }

    // DDL/DML → "OK" or empty body.
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "OK" {
        return Ok(ChResponse {
            columns: None,
            rows: Vec::new(),
            written_rows,
        });
    }

    // Resultset → JSON format: {"meta":[{name,type}],"data":[{col:val}]}.
    if let Ok(v) = serde_json::from_str::<Value>(&text) {
        if let (Some(meta), Some(data)) = (v.get("meta"), v.get("data")) {
            let columns: Vec<Value> = meta
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|m| json!({ "name": m["name"], "type": m["type"] }))
                        .collect()
                })
                .unwrap_or_default();
            let names: Vec<&str> = meta
                .as_array()
                .map(|arr| arr.iter().filter_map(|m| m["name"].as_str()).collect())
                .unwrap_or_default();
            let rows: Vec<Vec<Value>> = data
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|row_obj| {
                            names
                                .iter()
                                .map(|name| row_obj.get(*name).cloned().unwrap_or(Value::Null))
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default();
            return Ok(ChResponse {
                columns: Some(columns),
                rows,
                written_rows,
            });
        }
    }

    // Fallback: a user-supplied `FORMAT JSONEachRow` yields one JSON object
    // per line — parse it as rows with columns from the first line's keys.
    let mut rows: Vec<Vec<Value>> = Vec::new();
    let mut columns: Option<Vec<Value>> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(obj) = serde_json::from_str::<Value>(line) {
            if let Some(names) = obj
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
            {
                if columns.is_none() {
                    columns = Some(
                        names
                            .iter()
                            .map(|n| json!({ "name": n, "type": "string" }))
                            .collect(),
                    );
                }
                let names_ref = &names;
                rows.push(
                    names_ref
                        .iter()
                        .map(|n| obj.get(n).cloned().unwrap_or(Value::Null))
                        .collect(),
                );
            }
        }
    }
    Ok(ChResponse {
        columns,
        rows,
        written_rows,
    })
}

pub fn is_query_stmt(stmt: &str) -> bool {
    let upper = stmt.trim().to_uppercase();
    upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("SHOW")
        || upper.starts_with("DESCRIBE")
        || upper.starts_with("DESC ")
        || upper.starts_with("EXISTS")
        || upper.starts_with("EXPLAIN")
        || upper.starts_with("VALUES")
}

pub(super) async fn execute_clickhouse(
    parsed: &sql_parser::SqlParseResult,
    timeout_secs: u64,
) -> Result<Response> {
    // One session_id per run keeps temp tables alive across statements
    // (HTTP is otherwise stateless), matching the TCP drivers' behavior.
    let client = with_session_id(
        connect_clickhouse(&parsed.connection).await?,
        sqlx::types::Uuid::new_v4().to_string(),
    );

    let mut results = Vec::new();
    let total_start = std::time::Instant::now();

    for stmt in &parsed.statements {
        if sql_parser::detect_use_statement(stmt).is_some() {
            continue;
        }

        let stmt_result: anyhow::Result<StatementResult> = async {
            let stmt_start = std::time::Instant::now();
            let ch = clickhouse_post(&client, stmt.trim(), timeout_secs).await?;
            let elapsed = stmt_start.elapsed().as_millis() as u64;
            match ch.columns {
                Some(columns) => {
                    let row_count = ch.rows.len();
                    Ok(StatementResult {
                        columns,
                        rows: ch.rows,
                        row_count,
                        affected_rows: None,
                        execution_time_ms: elapsed,
                        error: None,
                        connection: None,
                        translated_sql: None,
                        original_sql: None,
                    })
                }
                None => Ok(StatementResult {
                    affected_rows: Some(ch.written_rows),
                    execution_time_ms: elapsed,
                    ..Default::default()
                }),
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
        &Protocol::ClickHouse,
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
        let (base, user, pass, db) =
            clickhouse_url_to_config("clickhouse://default@localhost:18123/playground").unwrap();
        assert_eq!(base, "http://localhost:18123");
        assert_eq!(user, "default");
        assert_eq!(pass, "");
        assert_eq!(db, "playground");

        let (base, user, pass, db) =
            clickhouse_url_to_config("clickhouse://alice:p%40ss@ch.example.com").unwrap();
        assert_eq!(base, "http://ch.example.com:8123");
        assert_eq!(user, "alice");
        assert_eq!(pass, "p@ss");
        assert_eq!(db, "");
    }

    #[test]
    fn test_is_query_stmt() {
        assert!(is_query_stmt("SELECT * FROM t"));
        assert!(is_query_stmt("SHOW TABLES"));
        assert!(is_query_stmt("DESCRIBE TABLE t"));
        assert!(is_query_stmt("EXISTS TABLE t"));
        assert!(!is_query_stmt("INSERT INTO t VALUES (1)"));
        assert!(!is_query_stmt("CREATE TABLE t (a Int32) ENGINE = Memory"));
        assert!(!is_query_stmt("ALTER TABLE t DELETE WHERE a = 1"));
    }
}
