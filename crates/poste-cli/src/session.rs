use anyhow::Result;
use clap::Parser;
use poste_exec::sql_exec_common::{
    clamp_rows, error_result_event, ok_result_event, run_sqlx_statement, StmtOutcome, MYSQL_QUERY,
    POSTGRES_QUERY, SQLITE_QUERY,
};
use serde_json::{json, Value};
use sqlx::Column as _;
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
    } else if connection_url.starts_with("mssql://") {
        poste_core::Protocol::Mssql
    } else if connection_url.starts_with("clickhouse://") {
        poste_core::Protocol::ClickHouse
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
        poste_core::Protocol::Mssql => {
            session_mssql(&connection_url, args.timeout, args.max_rows).await
        }
        poste_core::Protocol::ClickHouse => {
            session_clickhouse(&connection_url, args.timeout, args.max_rows).await
        }
        _ => anyhow::bail!("Not a SQL protocol"),
    }
}

/// Write one wire event as an NDJSON line and flush, so the Lua side sees
/// each response the moment it is ready.
async fn emit(stdout: &mut tokio::io::Stdout, ev: &Value) -> Result<()> {
    stdout
        .write_all(format!("{}\n", serde_json::to_string(ev)?).as_bytes())
        .await?;
    stdout.flush().await?;
    Ok(())
}

/// Shared per-request tail for all five dialect loops: convert the outcome
/// (or the failure) into the session wire event. Session events carry no
/// `total` key; an error event reports the real elapsed time.
fn session_result_event(
    seq: u64,
    sql: &str,
    outcome: anyhow::Result<StmtOutcome>,
    started: &Instant,
) -> Value {
    match outcome {
        Ok(o) => ok_result_event(seq, None, sql, &o),
        Err(e) => error_result_event(seq, None, sql, &e, started.elapsed().as_millis() as u64),
    }
}

async fn session_sqlite(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use sqlx::TypeInfo;

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
        let (seq, sql) = match parse_request(&line) {
            Request::Run(seq, sql) => (seq, sql),
            Request::Blank => continue,
            Request::Malformed(msg) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":msg});
                emit(&mut stdout, &err).await?;
                continue;
            }
        };

        let started = Instant::now();
        let outcome = run_sqlx_statement(
            &mut *conn,
            &sql,
            &SQLITE_QUERY,
            timeout_secs,
            max_rows,
            started,
            poste_exec::sql_values::sqlite_value_to_json,
            |col| json!({ "name": col.name(), "type": col.type_info().name() }),
        )
        .await;
        let result = session_result_event(seq, &sql, outcome, &started);
        emit(&mut stdout, &result).await?;
    }

    drop(conn);
    pool.close().await;
    Ok(())
}

async fn session_postgres(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use sqlx::postgres::PgPoolOptions;
    use sqlx::TypeInfo;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("PostgreSQL connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let (seq, sql) = match parse_request(&line) {
            Request::Run(seq, sql) => (seq, sql),
            Request::Blank => continue,
            Request::Malformed(msg) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":msg});
                emit(&mut stdout, &err).await?;
                continue;
            }
        };

        let started = Instant::now();
        let outcome = run_sqlx_statement(
            &mut *conn,
            &sql,
            &POSTGRES_QUERY,
            timeout_secs,
            max_rows,
            started,
            poste_exec::sql_values::pg_value_to_json,
            |col| {
                json!({
                    "name": col.name(),
                    "type": col.type_info().name(),
                    "nullable": col.type_info().name() != "BOOL",
                })
            },
        )
        .await;
        let result = session_result_event(seq, &sql, outcome, &started);
        emit(&mut stdout, &result).await?;
    }

    drop(conn);
    pool.close().await;
    Ok(())
}

async fn session_mysql(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use sqlx::mysql::MySqlPoolOptions;
    use sqlx::TypeInfo;

    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("MySQL connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let (seq, sql) = match parse_request(&line) {
            Request::Run(seq, sql) => (seq, sql),
            Request::Blank => continue,
            Request::Malformed(msg) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":msg});
                emit(&mut stdout, &err).await?;
                continue;
            }
        };

        let started = Instant::now();
        let outcome = run_sqlx_statement(
            &mut *conn,
            &sql,
            &MYSQL_QUERY,
            timeout_secs,
            max_rows,
            started,
            poste_exec::sql_values::mysql_value_to_json,
            |col| json!({ "name": col.name(), "type": col.type_info().name() }),
        )
        .await;
        let result = session_result_event(seq, &sql, outcome, &started);
        emit(&mut stdout, &result).await?;
    }

    drop(conn);
    pool.close().await;
    Ok(())
}

async fn session_mssql(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use poste_exec::sql_executor::mssql;

    let mut client = mssql::connect_mssql(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("SQL Server connection failed: {}", e))?;
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let (seq, sql) = match parse_request(&line) {
            Request::Run(seq, sql) => (seq, sql),
            Request::Blank => continue,
            Request::Malformed(msg) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":msg});
                emit(&mut stdout, &err).await?;
                continue;
            }
        };

        let started = Instant::now();
        // Per-statement errors are reported as error results instead of
        // killing the session — a TDS session holds temp tables and
        // transactions, so surviving a bad statement matters more here.
        // mssql_query/mssql_execute own their timeouts internally.
        let outcome: anyhow::Result<StmtOutcome> = if mssql::is_query_stmt(&sql) {
            mssql::mssql_query(&mut client, &sql, timeout_secs)
                .await
                .map(|(columns, all_rows)| {
                    let row_count = all_rows.len() as u64;
                    let (rows, truncated) = clamp_rows(all_rows, max_rows);
                    StmtOutcome {
                        columns,
                        rows,
                        row_count,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        truncated,
                        is_dml: false,
                        affected: 0,
                    }
                })
        } else {
            mssql::mssql_execute(&mut client, &sql, timeout_secs)
                .await
                .map(|affected| StmtOutcome {
                    columns: vec![],
                    rows: vec![],
                    row_count: 0,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    truncated: false,
                    is_dml: true,
                    affected,
                })
        };
        let result = session_result_event(seq, &sql, outcome, &started);
        emit(&mut stdout, &result).await?;
    }

    Ok(())
}

async fn session_clickhouse(connection_url: &str, timeout_secs: u64, max_rows: u64) -> Result<()> {
    use poste_exec::sql_executor::clickhouse;

    let mut client = clickhouse::connect_clickhouse(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("ClickHouse connection failed: {}", e))?;
    // A session_id keeps temp tables and SET state alive across requests.
    client = clickhouse::with_session_id(client, uuid::Uuid::new_v4().to_string());

    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let (seq, sql) = match parse_request(&line) {
            Request::Run(seq, sql) => (seq, sql),
            Request::Blank => continue,
            Request::Malformed(msg) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":msg});
                emit(&mut stdout, &err).await?;
                continue;
            }
        };

        let started = Instant::now();
        // Query-vs-DML is decided by the response shape (columns = resultset).
        let outcome: anyhow::Result<StmtOutcome> =
            clickhouse::clickhouse_post(&client, &sql, timeout_secs)
                .await
                .map(|ch| {
                    let elapsed = started.elapsed().as_millis() as u64;
                    match ch.columns {
                        Some(columns) => {
                            let row_count = ch.rows.len() as u64;
                            let (rows, truncated) = clamp_rows(ch.rows, max_rows);
                            StmtOutcome {
                                columns,
                                rows,
                                row_count,
                                elapsed_ms: elapsed,
                                truncated,
                                is_dml: false,
                                affected: 0,
                            }
                        }
                        None => StmtOutcome {
                            columns: vec![],
                            rows: vec![],
                            row_count: 0,
                            elapsed_ms: elapsed,
                            truncated: false,
                            is_dml: true,
                            affected: ch.written_rows,
                        },
                    }
                });
        let result = session_result_event(seq, &sql, outcome, &started);
        emit(&mut stdout, &result).await?;
    }

    Ok(())
}

/// Outcome of parsing one stdin NDJSON request line.
enum Request {
    /// Serve this (seq, sql).
    Run(u64, String),
    /// Blank SQL: silently skipped, like the pre-consolidation loop.
    Blank,
    /// Malformed JSON: a seq-0 error event answers it and the loop continues.
    Malformed(String),
}

fn parse_request(line: &str) -> Request {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Request::Malformed(format!("JSON parse error: {}", e)),
    };
    let seq = req.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
    let sql = req
        .get("sql")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if sql.is_empty() {
        return Request::Blank;
    }
    Request::Run(seq, sql)
}
