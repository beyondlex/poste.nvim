use anyhow::Result;
use clap::Parser;
use poste_exec::sql_exec_common::{
    clamp_rows, error_result_event, is_skippable, ok_result_event, progress_event,
    run_sqlx_statement, StmtOutcome, MYSQL_QUERY, POSTGRES_QUERY, SQLITE_QUERY,
};
use serde_json::json;
use sqlx::{Column as _, TypeInfo as _};
use std::time::Instant;

#[derive(Parser)]
pub struct ExecFileArgs {
    /// Path to .sql file
    pub file: String,
    /// Environment name
    #[arg(short, long, default_value = "dev")]
    pub env: String,
    /// Execution mode: "transaction" or "greedy"
    #[arg(short, long, default_value = "greedy")]
    pub mode: String,
    /// Per-statement timeout in seconds (0 = no timeout)
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,
    /// Max rows per SELECT result (0 = unlimited)
    #[arg(long, default_value_t = 1000)]
    pub max_rows: u64,
    /// Output as JSON (always true when called from Lua)
    #[arg(long)]
    pub json: bool,
    /// Connection URL (Lua-resolved, not a name from connections.json)
    #[arg(long)]
    pub connection: Option<String>,
    /// Override database name
    #[arg(long)]
    pub database: Option<String>,
}

pub async fn execute(args: ExecFileArgs) -> Result<()> {
    exec_file(&args, |line| println!("{}", line)).await
}

pub async fn exec_file<F>(args: &ExecFileArgs, mut emit: F) -> Result<()>
where
    F: FnMut(&str),
{
    let abs_path = std::path::Path::new(&args.file);
    let abs_path = if abs_path.is_absolute() {
        abs_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(abs_path)
    };
    let abs_path = std::fs::canonicalize(&abs_path)
        .map_err(|e| anyhow::anyhow!("File not found: {} ({})", abs_path.display(), e))?;

    let content = std::fs::read_to_string(&abs_path)?;
    let search_dir = abs_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    // Resolve connection URL: --connection arg (must be a URL, resolved by Lua)
    let mut connection_url = resolve_connection(args, &content, search_dir)?;

    // Apply --database override
    if let Some(ref db) = args.database {
        connection_url = poste_core::replace_database_in_url(&connection_url, db);
    }

    // Detect protocol from connection URL
    let protocol = poste_core::Protocol::from_sql_url(&connection_url).ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine protocol from connection URL: {}",
            poste_core::mask_url_password(&connection_url)
        )
    })?;

    // Extract database from connection URL for display
    let database = extract_database_from_url(&connection_url);

    // Parse SQL statements directly from file content (strip -- @... directives)
    let body = strip_sql_directives(&content);
    let all_statements = poste_core::sql_parser::split_statements(&body);

    if all_statements.is_empty() {
        anyhow::bail!("No SQL statements found in file");
    }

    // Filter out USE statements: they are silently skipped during execution
    // (the database is already set via --database or connection URL). Removing
    // them here keeps `total` and seq numbering consistent — no gaps, no stuck
    // progress bar. The predicate is the same one the exec loops skip on
    // (`is_skippable`), so the two can never disagree about what a USE is.
    let statements: Vec<String> = all_statements
        .iter()
        .filter(|s| {
            let t = s.trim();
            !t.is_empty() && !poste_core::sql_parser::is_use_statement(t)
        })
        .cloned()
        .collect();

    if statements.is_empty() {
        anyhow::bail!("No executable SQL statements found in file (only USE/empty statements)");
    }

    let total = statements.len() as u64;
    let max_rows = args.max_rows;

    let summary = run_statements(
        &protocol,
        &connection_url,
        &database,
        &statements,
        &args.mode,
        args.timeout,
        max_rows,
        total,
        &mut emit,
    )
    .await?;

    let summary_json = json!({
        "type": "summary",
        "total": summary.total,
        "succeeded": summary.succeeded,
        "failed": summary.failed,
        "total_rows": summary.total_rows,
        "total_affected": summary.total_affected,
        "total_time_ms": summary.total_time_ms,
        "connection": connection_url,
        "database": database,
        "dialect": summary.dialect,
        "mode": args.mode,
        "rolled_back": args.mode == "transaction" && summary.failed > 0,
    });
    emit(&summary_json.to_string());

    Ok(())
}

struct ExecSummary {
    total: u64,
    succeeded: u64,
    failed: u64,
    total_rows: u64,
    total_affected: u64,
    total_time_ms: u64,
    dialect: String,
}

fn resolve_connection(
    args: &ExecFileArgs,
    content: &str,
    search_dir: &std::path::Path,
) -> Result<String> {
    let conn = if let Some(ref conn) = args.connection {
        conn.clone()
    } else if let Some(conn) = extract_connection_directive(content) {
        conn
    } else {
        anyhow::bail!(
            "No connection specified. Use --connection <url> or add -- @connection <url> to the SQL file."
        )
    };

    if crate::util::is_connection_url(&conn) {
        return Ok(conn);
    }

    let store = poste_exec::sql_connection::ConnectionStore::load(search_dir)?;
    let env_vars = crate::util::load_env_vars(search_dir, &args.env);
    store.resolve(&conn, &env_vars)
}

fn extract_connection_directive(content: &str) -> Option<String> {
    let re = regex::Regex::new(r"--\s*@connection\s+(.+)").ok()?;
    for line in content.lines() {
        if let Some(caps) = re.captures(line) {
            let val = caps[1].trim().to_string();
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    None
}

fn extract_database_from_url(url: &str) -> Option<String> {
    // postgres://user:pass@host:5432/dbname → Some("dbname")
    // mysql://user:pass@host:3306/dbname?sslmode=require → Some("dbname")
    // sqlite::memory: → None
    // sqlite:/path/to/db.sqlite → extract filename without extension
    if let Some(rest) = url.strip_prefix("sqlite:") {
        let rest = rest.trim_start_matches('/');
        if rest == ":memory:" || rest.is_empty() {
            return None;
        }
        let path = std::path::Path::new(rest);
        return path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
    }
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        if let Some(last_slash) = after_scheme.rfind('/') {
            let db = after_scheme[last_slash + 1..].to_string();
            // Drop a query string / fragment: ?sslmode=require is connection
            // config, not part of the name (replace_database_in_url makes
            // the same split when --database swaps the path segment).
            let db = match db.find(['?', '#']) {
                Some(i) => db[..i].to_string(),
                None => db,
            };
            if !db.is_empty() {
                return Some(db);
            }
        }
    }
    None
}

fn strip_sql_directives(content: &str) -> String {
    let directive_re = regex::Regex::new(r"^\s*--\s*@\w+").unwrap();
    content
        .lines()
        .filter(|line| !directive_re.is_match(line))
        .filter(|line| line.trim() != "###")
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(clippy::too_many_arguments)]
async fn run_statements<F>(
    protocol: &poste_core::Protocol,
    connection_url: &str,
    _database: &Option<String>,
    statements: &[String],
    mode: &str,
    timeout_secs: u64,
    max_rows: u64,
    total: u64,
    emit: &mut F,
) -> Result<ExecSummary>
where
    F: FnMut(&str),
{
    let total_start = Instant::now();
    let mut succeeded = 0u64;
    let mut failed = 0u64;
    let mut total_rows = 0u64;
    let mut total_affected = 0u64;

    match protocol {
        poste_core::Protocol::Sqlite => {
            exec_sqlite(
                connection_url,
                statements,
                mode,
                timeout_secs,
                max_rows,
                total,
                emit,
                &mut succeeded,
                &mut failed,
                &mut total_rows,
                &mut total_affected,
            )
            .await?;
        }
        poste_core::Protocol::Postgres => {
            exec_postgres(
                connection_url,
                statements,
                mode,
                timeout_secs,
                max_rows,
                total,
                emit,
                &mut succeeded,
                &mut failed,
                &mut total_rows,
                &mut total_affected,
            )
            .await?;
        }
        poste_core::Protocol::Mysql => {
            exec_mysql(
                connection_url,
                statements,
                mode,
                timeout_secs,
                max_rows,
                total,
                emit,
                &mut succeeded,
                &mut failed,
                &mut total_rows,
                &mut total_affected,
            )
            .await?;
        }
        poste_core::Protocol::Mssql => {
            exec_mssql(
                connection_url,
                statements,
                mode,
                timeout_secs,
                max_rows,
                total,
                emit,
                &mut succeeded,
                &mut failed,
                &mut total_rows,
                &mut total_affected,
            )
            .await?;
        }
        poste_core::Protocol::ClickHouse => {
            exec_clickhouse(
                connection_url,
                statements,
                mode,
                timeout_secs,
                max_rows,
                total,
                emit,
                &mut succeeded,
                &mut failed,
                &mut total_rows,
                &mut total_affected,
            )
            .await?;
        }
        _ => anyhow::bail!("Not a SQL protocol: {:?}", protocol),
    }

    let total_ms = total_start.elapsed().as_millis() as u64;
    let dialect = match protocol {
        poste_core::Protocol::Postgres => "postgres",
        poste_core::Protocol::Mysql => "mysql",
        poste_core::Protocol::Mssql => "mssql",
        poste_core::Protocol::ClickHouse => "clickhouse",
        poste_core::Protocol::Sqlite => "sqlite",
        _ => "unknown",
    };

    Ok(ExecSummary {
        total,
        succeeded,
        failed,
        total_rows,
        total_affected,
        total_time_ms: total_ms,
        dialect: dialect.to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
/// The success arm shared by every dialect: bump counters and emit the wire
/// `result` event (the exec-file stream carries `total`).
fn record_ok<F>(
    emit: &mut F,
    seq: u64,
    total: u64,
    sql: &str,
    outcome: &StmtOutcome,
    succeeded: &mut u64,
    total_rows: &mut u64,
    total_affected: &mut u64,
) where
    F: FnMut(&str),
{
    *succeeded += 1;
    *total_rows += outcome.row_count;
    if outcome.is_dml {
        *total_affected += outcome.affected;
    }
    emit(&ok_result_event(seq, Some(total), sql, outcome).to_string());
}

/// The failure arm: bump the counter and emit the error event. The
/// execution_time_ms of an exec-file error event stays 0 (pre-consolidation
/// shape); rollback/break handling is dialect-specific and stays inline.
fn record_err<F>(
    emit: &mut F,
    seq: u64,
    total: u64,
    sql: &str,
    err: &anyhow::Error,
    failed: &mut u64,
) where
    F: FnMut(&str),
{
    *failed += 1;
    emit(&error_result_event(seq, Some(total), sql, err, 0).to_string());
}

/// Close a `--mode transaction` batch. Every dialect used to end with
/// `COMMIT … .ok()`, which threw away the one error that matters most: a
/// server-side COMMIT failure (deferred constraint, read-only replica, disk
/// full, connection dropped mid-batch) rolls the whole batch back while the
/// stream still reads "N statements succeeded" and the summary says no
/// failures. Reported as a statement-shaped error event so the Lua side
/// counts it (`status ~= "ok"` → `has_error`) and shows the reason; the
/// summary's `rolled_back` follows from `failed > 0`, as it already does for
/// a mid-batch abort.
fn record_commit<F>(emit: &mut F, total: u64, committed: anyhow::Result<()>, failed: &mut u64)
where
    F: FnMut(&str),
{
    if let Err(e) = committed {
        record_err(emit, total + 1, total, "COMMIT", &e, failed);
    }
}

#[allow(clippy::too_many_arguments)]
async fn exec_sqlite<F>(
    connection_url: &str,
    statements: &[String],
    mode: &str,
    timeout_secs: u64,
    max_rows: u64,
    total: u64,
    emit: &mut F,
    succeeded: &mut u64,
    failed: &mut u64,
    total_rows: &mut u64,
    total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    let conn_str = poste_exec::sql_connection::normalize_sqlite_connection(connection_url)?;
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&conn_str)
        .await
        .map_err(|e| anyhow::anyhow!("SQLite connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;

    let mut in_transaction = false;
    if mode == "transaction" {
        sqlx::query("BEGIN").execute(&mut *conn).await?;
        in_transaction = true;
    }

    for (seq, stmt) in statements.iter().enumerate() {
        let seq = seq as u64 + 1;
        let stmt_trimmed = stmt.trim();
        if is_skippable(stmt_trimmed) {
            continue;
        }
        emit(&progress_event(seq, total, stmt_trimmed).to_string());

        let started = Instant::now();
        let outcome = run_sqlx_statement(
            &mut *conn,
            stmt_trimmed,
            &SQLITE_QUERY,
            timeout_secs,
            max_rows,
            started,
            poste_exec::sql_values::sqlite_value_to_json,
            |col| json!({ "name": col.name(), "type": col.type_info().name() }),
        )
        .await;

        match outcome {
            Ok(outcome) => {
                record_ok(
                    emit,
                    seq,
                    total,
                    stmt_trimmed,
                    &outcome,
                    succeeded,
                    total_rows,
                    total_affected,
                );
            }
            Err(e) => {
                record_err(emit, seq, total, stmt_trimmed, &e, failed);
                if in_transaction {
                    sqlx::query("ROLLBACK").execute(&mut *conn).await.ok();
                    in_transaction = false;
                }
                if mode == "transaction" {
                    break;
                }
            }
        }
    }

    if in_transaction {
        record_commit(
            emit,
            total,
            sqlx::query("COMMIT")
                .execute(&mut *conn)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from),
            failed,
        );
    }

    drop(conn);
    pool.close().await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn exec_mssql<F>(
    connection_url: &str,
    statements: &[String],
    mode: &str,
    timeout_secs: u64,
    max_rows: u64,
    total: u64,
    emit: &mut F,
    succeeded: &mut u64,
    failed: &mut u64,
    total_rows: &mut u64,
    total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    use poste_exec::sql_executor::mssql;

    let mut client = mssql::connect_mssql(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("SQL Server connection failed: {}", e))?;

    let mut in_transaction = false;
    if mode == "transaction" {
        mssql::mssql_batch(&mut client, "BEGIN TRANSACTION", timeout_secs).await?;
        in_transaction = true;
    }

    for (seq, stmt) in statements.iter().enumerate() {
        let seq = seq as u64 + 1;
        let stmt_trimmed = stmt.trim();
        if is_skippable(stmt_trimmed) {
            continue;
        }
        emit(&progress_event(seq, total, stmt_trimmed).to_string());

        let started = Instant::now();
        // mssql_query/mssql_execute own their timeouts internally.
        let outcome: anyhow::Result<StmtOutcome> = if mssql::is_query_stmt(stmt_trimmed) {
            mssql::mssql_query(&mut client, stmt_trimmed, timeout_secs)
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
            mssql::mssql_execute(&mut client, stmt_trimmed, timeout_secs)
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

        match outcome {
            Ok(outcome) => {
                record_ok(
                    emit,
                    seq,
                    total,
                    stmt_trimmed,
                    &outcome,
                    succeeded,
                    total_rows,
                    total_affected,
                );
            }
            Err(e) => {
                record_err(emit, seq, total, stmt_trimmed, &e, failed);
                if in_transaction {
                    mssql::mssql_batch(&mut client, "ROLLBACK", timeout_secs)
                        .await
                        .ok();
                    in_transaction = false;
                }
                if mode == "transaction" {
                    break;
                }
            }
        }
    }

    if in_transaction {
        record_commit(
            emit,
            total,
            mssql::mssql_batch(&mut client, "COMMIT", timeout_secs).await,
            failed,
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn exec_clickhouse<F>(
    connection_url: &str,
    statements: &[String],
    mode: &str,
    timeout_secs: u64,
    max_rows: u64,
    total: u64,
    emit: &mut F,
    succeeded: &mut u64,
    failed: &mut u64,
    total_rows: &mut u64,
    total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    use poste_exec::sql_executor::clickhouse;

    if mode == "transaction" {
        anyhow::bail!("ClickHouse has no transactions; use --mode greedy");
    }

    // One session_id per file keeps temp tables alive across statements.
    let client = clickhouse::with_session_id(
        clickhouse::connect_clickhouse(connection_url)
            .await
            .map_err(|e| anyhow::anyhow!("ClickHouse connection failed: {}", e))?,
        sqlx::types::Uuid::new_v4().to_string(),
    );

    for (seq, stmt) in statements.iter().enumerate() {
        let seq = seq as u64 + 1;
        let stmt_trimmed = stmt.trim();
        if is_skippable(stmt_trimmed) {
            continue;
        }
        emit(&progress_event(seq, total, stmt_trimmed).to_string());

        let started = Instant::now();
        // clickhouse_post owns its timeout; query-vs-DML is decided by the
        // response shape (columns present = resultset).
        let outcome: anyhow::Result<StmtOutcome> =
            clickhouse::clickhouse_post(&client, stmt_trimmed, timeout_secs)
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

        match outcome {
            Ok(outcome) => {
                record_ok(
                    emit,
                    seq,
                    total,
                    stmt_trimmed,
                    &outcome,
                    succeeded,
                    total_rows,
                    total_affected,
                );
            }
            Err(e) => {
                record_err(emit, seq, total, stmt_trimmed, &e, failed);
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn exec_postgres<F>(
    connection_url: &str,
    statements: &[String],
    mode: &str,
    timeout_secs: u64,
    max_rows: u64,
    total: u64,
    emit: &mut F,
    succeeded: &mut u64,
    failed: &mut u64,
    total_rows: &mut u64,
    total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    use sqlx::postgres::PgPoolOptions;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("PostgreSQL connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;

    let mut in_transaction = false;
    if mode == "transaction" {
        sqlx::query("BEGIN").execute(&mut *conn).await?;
        in_transaction = true;
    }

    for (seq, stmt) in statements.iter().enumerate() {
        let seq = seq as u64 + 1;
        let stmt_trimmed = stmt.trim();
        if is_skippable(stmt_trimmed) {
            continue;
        }
        emit(&progress_event(seq, total, stmt_trimmed).to_string());

        let started = Instant::now();
        let outcome = run_sqlx_statement(
            &mut *conn,
            stmt_trimmed,
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

        match outcome {
            Ok(outcome) => {
                record_ok(
                    emit,
                    seq,
                    total,
                    stmt_trimmed,
                    &outcome,
                    succeeded,
                    total_rows,
                    total_affected,
                );
            }
            Err(e) => {
                record_err(emit, seq, total, stmt_trimmed, &e, failed);
                if in_transaction {
                    sqlx::query("ROLLBACK").execute(&mut *conn).await.ok();
                    in_transaction = false;
                }
                if mode == "transaction" {
                    break;
                }
            }
        }
    }

    if in_transaction {
        record_commit(
            emit,
            total,
            sqlx::query("COMMIT")
                .execute(&mut *conn)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from),
            failed,
        );
    }
    drop(conn);
    pool.close().await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn exec_mysql<F>(
    connection_url: &str,
    statements: &[String],
    mode: &str,
    timeout_secs: u64,
    max_rows: u64,
    total: u64,
    emit: &mut F,
    succeeded: &mut u64,
    failed: &mut u64,
    total_rows: &mut u64,
    total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    use sqlx::mysql::MySqlPoolOptions;
    use sqlx::Executor;

    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(connection_url)
        .await
        .map_err(|e| anyhow::anyhow!("MySQL connection failed: {}", e))?;
    let mut conn = pool.acquire().await?;

    let mut in_transaction = false;
    if mode == "transaction" {
        conn.execute("SET autocommit = 0")
            .await
            .map_err(|e| anyhow::anyhow!("Failed to disable autocommit: {}", e))?;
        in_transaction = true;
    }

    for (seq, stmt) in statements.iter().enumerate() {
        let seq = seq as u64 + 1;
        let stmt_trimmed = stmt.trim();
        if is_skippable(stmt_trimmed) {
            continue;
        }
        emit(&progress_event(seq, total, stmt_trimmed).to_string());

        let started = Instant::now();
        let outcome = run_sqlx_statement(
            &mut *conn,
            stmt_trimmed,
            &MYSQL_QUERY,
            timeout_secs,
            max_rows,
            started,
            poste_exec::sql_values::mysql_value_to_json,
            |col| json!({ "name": col.name(), "type": col.type_info().name() }),
        )
        .await;

        match outcome {
            Ok(outcome) => {
                record_ok(
                    emit,
                    seq,
                    total,
                    stmt_trimmed,
                    &outcome,
                    succeeded,
                    total_rows,
                    total_affected,
                );
            }
            Err(e) => {
                record_err(emit, seq, total, stmt_trimmed, &e, failed);
                if in_transaction {
                    conn.execute("ROLLBACK").await.ok();
                    in_transaction = false;
                }
                if mode == "transaction" {
                    break;
                }
            }
        }
    }

    if in_transaction {
        // mysql COMMITs only a clean run; the failing statement above
        // already rolled back, so re-COMMITting would be a no-op at best.
        if *failed == 0 {
            record_commit(
                emit,
                total,
                conn.execute("COMMIT")
                    .await
                    .map(|_| ())
                    .map_err(anyhow::Error::from),
                failed,
            );
        }
        conn.execute("SET autocommit = 1").await.ok();
    }
    drop(conn);
    pool.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A COMMIT the server rejected must reach the stream as an error event:
    /// the batch is rolled back even though every statement above it
    /// succeeded, and `failed > 0` is also what flips the summary's
    /// `rolled_back` flag.
    #[test]
    fn commit_failure_is_reported() {
        let mut events = Vec::new();
        let mut failed = 0u64;
        record_commit(
            &mut |line| events.push(line.to_string()),
            3,
            Err(anyhow::anyhow!("FOREIGN KEY constraint failed")),
            &mut failed,
        );
        assert_eq!(failed, 1);
        assert_eq!(events.len(), 1);
        let ev: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
        assert_eq!(ev["type"], "result");
        assert_eq!(ev["status"], "error");
        assert_eq!(ev["sql"], "COMMIT");
        assert_eq!(ev["seq"], 4, "one past the last statement's seq");
        assert_eq!(ev["total"], 3);
        assert_eq!(ev["error"], "FOREIGN KEY constraint failed");
    }

    /// A clean COMMIT emits nothing — otherwise every transaction-mode run
    /// would gain a phantom failed statement.
    #[test]
    fn clean_commit_emits_nothing() {
        let mut events = Vec::new();
        let mut failed = 0u64;
        record_commit(
            &mut |line| events.push(line.to_string()),
            3,
            Ok(()),
            &mut failed,
        );
        assert!(events.is_empty());
        assert_eq!(failed, 0);
    }

    #[test]
    fn test_extract_database_from_url() {
        assert_eq!(
            extract_database_from_url("postgres://u:p@host:5432/dbname"),
            Some("dbname".to_string())
        );
        // query strings and fragments are connection config, not the name
        assert_eq!(
            extract_database_from_url("postgres://u:p@host:5432/dbname?sslmode=require"),
            Some("dbname".to_string())
        );
        assert_eq!(
            extract_database_from_url("mysql://host/dbname#frag"),
            Some("dbname".to_string())
        );
        assert_eq!(
            extract_database_from_url("postgres://host:5432"),
            None,
            "no path db"
        );
        assert_eq!(extract_database_from_url("sqlite::memory:"), None);
        assert_eq!(
            extract_database_from_url("sqlite:/path/to/db.sqlite"),
            Some("db".to_string())
        );
    }

    fn collect_events(args: &ExecFileArgs) -> Vec<serde_json::Value> {
        let events = std::sync::Mutex::new(Vec::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            exec_file(args, |line| {
                let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
                events.lock().unwrap().push(parsed);
            })
            .await
        })
        .unwrap();
        let guard = events.lock().unwrap();
        guard.clone()
    }

    #[test]
    fn test_sqlite_create_insert_select() {
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("test.sql");

        let sql_content = r#"-- @connection test_conn
CREATE TABLE t (x INT);
INSERT INTO t VALUES (1);
INSERT INTO t VALUES (2);
INSERT INTO t VALUES (3);
SELECT * FROM t ORDER BY x;
"#;
        std::fs::write(&sql_path, sql_content).unwrap();

        // Write connections.json with SQLite :memory: connection
        let conn_json = serde_json::json!({
            "test_conn": {
                "dialect": "sqlite",
                "database": ":memory:"
            }
        });
        std::fs::write(
            dir.path().join("connections.json"),
            serde_json::to_string_pretty(&conn_json).unwrap(),
        )
        .unwrap();

        let args = ExecFileArgs {
            file: sql_path.to_string_lossy().to_string(),
            env: "dev".to_string(),
            mode: "greedy".to_string(),
            timeout: 10,
            max_rows: 1000,
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let events = collect_events(&args);

        // Should have: 4 progress + 4 result + 1 summary = 9 events
        // Actually, the progress events are emitted before each result
        // But we need to check: USE statements are skipped
        // No USE statements in this file, so 4 statements → 4 progress + 4 result + 1 summary = 9
        assert!(
            events.len() >= 9,
            "Expected at least 9 events, got {}",
            events.len()
        );

        // Check summary
        let summary = &events[events.len() - 1];
        assert_eq!(summary["type"], "summary");
        assert_eq!(summary["total"], 5);
        assert_eq!(summary["succeeded"], 5);
        assert_eq!(summary["failed"], 0);
        assert_eq!(summary["total_rows"], 3); // SELECT returns 3 rows
        assert_eq!(summary["dialect"], "sqlite");
        assert_eq!(summary["mode"], "greedy");

        // Check progress events
        let progress_events: Vec<&serde_json::Value> =
            events.iter().filter(|e| e["type"] == "progress").collect();
        assert_eq!(progress_events.len(), 5);
        assert_eq!(progress_events[0]["seq"], 1);
        assert_eq!(progress_events[0]["sql"], "CREATE TABLE t (x INT)");
        assert_eq!(progress_events[4]["seq"], 5);

        // Check result events
        let result_events: Vec<&serde_json::Value> =
            events.iter().filter(|e| e["type"] == "result").collect();
        assert_eq!(result_events.len(), 5);
        assert_eq!(result_events[0]["status"], "ok");
        assert_eq!(result_events[4]["status"], "ok");
        assert_eq!(result_events[4]["row_count"], 3);
        assert_eq!(result_events[4]["rows"][0][0], 1);
        assert_eq!(result_events[4]["rows"][1][0], 2);
        assert_eq!(result_events[4]["rows"][2][0], 3);
    }

    #[test]
    fn test_sqlite_max_rows_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("test.sql");

        let sql_content = r#"-- @connection test_conn
CREATE TABLE t (x INT);
INSERT INTO t VALUES (1);
INSERT INTO t VALUES (2);
INSERT INTO t VALUES (3);
INSERT INTO t VALUES (4);
INSERT INTO t VALUES (5);
SELECT * FROM t ORDER BY x;
"#;
        std::fs::write(&sql_path, sql_content).unwrap();

        let conn_json = serde_json::json!({
            "test_conn": {
                "dialect": "sqlite",
                "database": ":memory:"
            }
        });
        std::fs::write(
            dir.path().join("connections.json"),
            serde_json::to_string_pretty(&conn_json).unwrap(),
        )
        .unwrap();

        let args = ExecFileArgs {
            file: sql_path.to_string_lossy().to_string(),
            env: "dev".to_string(),
            mode: "greedy".to_string(),
            timeout: 10,
            max_rows: 3, // Only return 3 rows max
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let events = collect_events(&args);
        let result_events: Vec<&serde_json::Value> =
            events.iter().filter(|e| e["type"] == "result").collect();
        let select_result = result_events.last().unwrap();

        // Should have 5 rows total but only 3 returned
        assert_eq!(select_result["row_count"], 5);
        assert_eq!(select_result["rows"].as_array().unwrap().len(), 3);
        assert_eq!(select_result["rows_truncated"], true);
    }

    #[test]
    fn test_sqlite_error_in_greedy_mode() {
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("test.sql");

        let sql_content = r#"-- @connection test_conn
CREATE TABLE t (x INT);
INSERT INTO t VALUES (1);
SELECT * FROM t;
SELECT * FROM nonexistent;
SELECT 1;
"#;
        std::fs::write(&sql_path, sql_content).unwrap();

        let conn_json = serde_json::json!({
            "test_conn": {
                "dialect": "sqlite",
                "database": ":memory:"
            }
        });
        std::fs::write(
            dir.path().join("connections.json"),
            serde_json::to_string_pretty(&conn_json).unwrap(),
        )
        .unwrap();

        let args = ExecFileArgs {
            file: sql_path.to_string_lossy().to_string(),
            env: "dev".to_string(),
            mode: "greedy".to_string(),
            timeout: 10,
            max_rows: 1000,
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let events = collect_events(&args);
        let summary = &events[events.len() - 1];

        assert_eq!(summary["succeeded"], 4);
        assert_eq!(summary["failed"], 1);
        assert_eq!(summary["total"], 5);

        // The error result should have "error" field
        let result_events: Vec<&serde_json::Value> =
            events.iter().filter(|e| e["type"] == "result").collect();
        let error_result = result_events
            .iter()
            .find(|e| e["status"] == "error")
            .unwrap();
        assert!(error_result["error"]
            .as_str()
            .unwrap()
            .contains("no such table"));
    }

    #[test]
    fn test_sqlite_transaction_mode_rollback_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("test.sql");

        let sql_content = r#"-- @connection test_conn
CREATE TABLE t (x INT);
INSERT INTO t VALUES (1);
SELECT * FROM nonexistent;
SELECT 1;
"#;
        std::fs::write(&sql_path, sql_content).unwrap();

        let conn_json = serde_json::json!({
            "test_conn": {
                "dialect": "sqlite",
                "database": ":memory:"
            }
        });
        std::fs::write(
            dir.path().join("connections.json"),
            serde_json::to_string_pretty(&conn_json).unwrap(),
        )
        .unwrap();

        let args = ExecFileArgs {
            file: sql_path.to_string_lossy().to_string(),
            env: "dev".to_string(),
            mode: "transaction".to_string(),
            timeout: 10,
            max_rows: 1000,
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let events = collect_events(&args);
        let summary = &events[events.len() - 1];

        // In transaction mode: first 2 succeed, 3rd fails, 4th never runs
        assert_eq!(summary["succeeded"], 2);
        assert_eq!(summary["failed"], 1);
        assert_eq!(summary["total"], 4);

        // Only 3 results should exist (4th never executed)
        let result_events: Vec<&serde_json::Value> =
            events.iter().filter(|e| e["type"] == "result").collect();
        assert_eq!(result_events.len(), 3);
    }

    #[test]
    fn test_sqlite_returning_word_in_literal_stays_dml() {
        // 'returning' inside a string literal must not flip the INSERT/UPDATE
        // onto the fetch path — the results must still carry affected_rows.
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("test.sql");

        let sql_content = r#"-- @connection test_conn
CREATE TABLE t (msg TEXT);
INSERT INTO t VALUES ('returning merchandise');
UPDATE t SET msg = 'returning' WHERE msg = 'returning merchandise';
SELECT * FROM t;
"#;
        std::fs::write(&sql_path, sql_content).unwrap();

        let conn_json = serde_json::json!({
            "test_conn": {
                "dialect": "sqlite",
                "database": ":memory:"
            }
        });
        std::fs::write(
            dir.path().join("connections.json"),
            serde_json::to_string_pretty(&conn_json).unwrap(),
        )
        .unwrap();

        let args = ExecFileArgs {
            file: sql_path.to_string_lossy().to_string(),
            env: "dev".to_string(),
            mode: "greedy".to_string(),
            timeout: 10,
            max_rows: 1000,
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let events = collect_events(&args);
        let result_events: Vec<&serde_json::Value> =
            events.iter().filter(|e| e["type"] == "result").collect();

        let insert = result_events[1].as_object().unwrap();
        assert_eq!(
            insert["affected_rows"], 1,
            "INSERT must report affected_rows"
        );
        let update = result_events[2].as_object().unwrap();
        assert_eq!(
            update["affected_rows"], 1,
            "UPDATE must report affected_rows"
        );
    }

    #[test]
    fn test_sqlite_scalar_looking_text_stays_string() {
        // TEXT values that merely look like scalar JSON must come back as
        // strings, not be coerced into numbers/bools/null.
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("test.sql");

        let sql_content = r#"-- @connection test_conn
CREATE TABLE t (v TEXT);
INSERT INTO t VALUES ('123'), ('null'), ('true'), ('[1,2]'), ('{"a":1}');
SELECT v FROM t ORDER BY rowid;
"#;
        std::fs::write(&sql_path, sql_content).unwrap();

        let conn_json = serde_json::json!({
            "test_conn": {
                "dialect": "sqlite",
                "database": ":memory:"
            }
        });
        std::fs::write(
            dir.path().join("connections.json"),
            serde_json::to_string_pretty(&conn_json).unwrap(),
        )
        .unwrap();

        let args = ExecFileArgs {
            file: sql_path.to_string_lossy().to_string(),
            env: "dev".to_string(),
            mode: "greedy".to_string(),
            timeout: 10,
            max_rows: 1000,
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let events = collect_events(&args);
        let result_events: Vec<&serde_json::Value> =
            events.iter().filter(|e| e["type"] == "result").collect();
        let select = result_events.last().unwrap();
        let rows = select["rows"].as_array().unwrap();

        assert_eq!(rows[0][0], "123", "text '123' must stay a string");
        assert_eq!(rows[1][0], "null", "text 'null' must stay a string");
        assert_eq!(rows[2][0], "true", "text 'true' must stay a string");
        // Structural JSON (objects/arrays) still parses — useful for
        // json_object()/json_array() results.
        assert_eq!(
            rows[3][0],
            serde_json::json!([1, 2]),
            "array-shaped text parses as JSON"
        );
        assert_eq!(
            rows[4][0],
            serde_json::json!({"a": 1}),
            "object-shaped text parses as JSON"
        );
    }

    #[test]
    fn test_no_sql_file_error() {
        let args = ExecFileArgs {
            file: "/nonexistent/path/file.sql".to_string(),
            env: "dev".to_string(),
            mode: "greedy".to_string(),
            timeout: 30,
            max_rows: 1000,
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async { exec_file(&args, |_| {}).await });
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("File not found"));
    }

    #[test]
    fn test_sqlite_empty_file_error() {
        let dir = tempfile::tempdir().unwrap();
        let sql_path = dir.path().join("empty.sql");
        std::fs::write(&sql_path, "-- @connection test_conn\n").unwrap();

        let conn_json = serde_json::json!({
            "test_conn": {
                "dialect": "sqlite",
                "database": ":memory:"
            }
        });
        std::fs::write(
            dir.path().join("connections.json"),
            serde_json::to_string_pretty(&conn_json).unwrap(),
        )
        .unwrap();

        let args = ExecFileArgs {
            file: sql_path.to_string_lossy().to_string(),
            env: "dev".to_string(),
            mode: "greedy".to_string(),
            timeout: 10,
            max_rows: 1000,
            json: true,
            database: None,
            connection: Some("test_conn".to_string()),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async { exec_file(&args, |_| {}).await });
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("No SQL statements") || err.contains("No statements"),
            "Error: {}",
            err
        );
    }
}
