use anyhow::Result;
use clap::Parser;
use serde_json::json;
use std::time::Instant;

type StatementResult = (
    Vec<serde_json::Value>,
    Vec<Vec<serde_json::Value>>,
    u64,
    u64,
    bool,
    bool,
);

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
    let protocol = if connection_url.starts_with("sqlite:") {
        poste_core::Protocol::Sqlite
    } else if connection_url.starts_with("mysql://") {
        poste_core::Protocol::Mysql
    } else if connection_url.starts_with("postgres://")
        || connection_url.starts_with("postgresql://")
    {
        poste_core::Protocol::Postgres
    } else {
        anyhow::bail!(
            "Cannot determine protocol from connection URL: {}",
            connection_url
        );
    };

    // Extract database from connection URL for display
    let database = extract_database_from_url(&connection_url);

    // Parse SQL statements directly from file content (strip -- @... directives)
    let body = strip_sql_directives(&content);
    let statements = poste_core::sql_parser::split_statements(&body);

    if statements.is_empty() {
        anyhow::bail!("No SQL statements found in file");
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
    // mysql://user:pass@host:3306/dbname → Some("dbname")
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
        _ => anyhow::bail!("Not a SQL protocol: {:?}", protocol),
    }

    let total_ms = total_start.elapsed().as_millis() as u64;
    let dialect = match protocol {
        poste_core::Protocol::Postgres => "postgres",
        poste_core::Protocol::Mysql => "mysql",
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
    _total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    use sqlx::{Column, Row, TypeInfo};

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

        if stmt_trimmed.is_empty() || stmt_trimmed.to_uppercase().starts_with("USE ") {
            continue;
        }

        // Emit progress
        let progress = json!({
            "type": "progress",
            "seq": seq,
            "total": total,
            "sql": stmt_trimmed,
        });
        emit(&progress.to_string());

        let stmt_start = Instant::now();
        let upper = stmt_trimmed.to_uppercase();

        let stmt_result: anyhow::Result<StatementResult> = async {
            if upper.starts_with("SELECT")
                || upper.starts_with("WITH")
                || upper.starts_with("EXPLAIN")
                || upper.starts_with("PRAGMA")
                || upper.starts_with("VALUES")
                || upper.contains("RETURNING")
            {
                let fetch = sqlx::query(stmt_trimmed).fetch_all(&mut *conn);
                let rows: Vec<sqlx::sqlite::SqliteRow> = if timeout_secs > 0 {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(timeout_secs),
                        fetch,
                    ).await {
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

                let columns: Vec<serde_json::Value> = rows
                    .first()
                    .map(|first_row| {
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
                    })
                    .unwrap_or_default();

                let json_rows: Vec<Vec<serde_json::Value>> = rows
                    .iter()
                    .take(display_rows)
                    .map(|row| {
                        (0..row.len())
                            .map(|i| {
                                sqlite_value_to_json(row, i, col_types.get(i).map_or("", |s| s))
                            })
                            .collect()
                    })
                    .collect();

                Ok((columns, json_rows, row_count, elapsed, truncated, false))
            } else {
                let exec = sqlx::query(stmt_trimmed).execute(&mut *conn);
                let _result = if timeout_secs > 0 {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(timeout_secs),
                        exec,
                    ).await {
                        Ok(result) => result?,
                        Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                    }
                } else {
                    exec.await?
                };
                let elapsed = stmt_start.elapsed().as_millis() as u64;
                Ok((Vec::new(), Vec::new(), 0u64, elapsed, false, true))
            }
        }
        .await;

        match stmt_result {
            Ok((columns, json_rows, row_count, elapsed, truncated, _is_dml)) => {
                *succeeded += 1;
                *total_rows += row_count;

                let result_obj = json!({
                    "type": "result",
                    "seq": seq,
                    "total": total,
                    "status": "ok",
                    "sql": stmt_trimmed,
                    "row_count": row_count,
                    "affected_rows": serde_json::Value::Null,
                    "execution_time_ms": elapsed,
                    "columns": columns,
                    "rows": json_rows,
                    "rows_truncated": truncated,
                });
                emit(&result_obj.to_string());
            }
            Err(e) => {
                *failed += 1;
                if in_transaction {
                    sqlx::query("ROLLBACK").execute(&mut *conn).await.ok();
                    in_transaction = false;
                }
                let result_obj = json!({
                    "type": "result",
                    "seq": seq,
                    "total": total,
                    "status": "error",
                    "sql": stmt_trimmed,
                    "error": format!("{}", e),
                    "execution_time_ms": 0,
                });
                emit(&result_obj.to_string());
                if mode == "transaction" {
                    break;
                }
            }
        }
    }

    if in_transaction {
        sqlx::query("COMMIT").execute(&mut *conn).await.ok();
    }

    drop(conn);
    pool.close().await;
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
    _total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    use sqlx::postgres::{PgPoolOptions, PgRow};
    use sqlx::{Column, Row, TypeInfo};

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

        if stmt_trimmed.is_empty() || stmt_trimmed.to_uppercase().starts_with("USE ") {
            continue;
        }

        let progress = json!({
            "type": "progress",
            "seq": seq,
            "total": total,
            "sql": stmt_trimmed,
        });
        emit(&progress.to_string());

        let stmt_start = Instant::now();
        let upper = stmt_trimmed.to_uppercase();

        let stmt_result: anyhow::Result<StatementResult> = async {
            if upper.starts_with("SELECT")
                || upper.starts_with("WITH")
                || upper.starts_with("EXPLAIN")
                || upper.starts_with("SHOW")
                || upper.starts_with("TABLE ")
                || upper.starts_with("VALUES")
                || upper.contains("RETURNING")
            {
                let fetch = sqlx::query(stmt_trimmed).fetch_all(&mut *conn);
                let rows: Vec<PgRow> = if timeout_secs > 0 {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(timeout_secs),
                        fetch,
                    ).await {
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

                let columns: Vec<serde_json::Value> = rows
                    .first()
                    .map(|first_row| {
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
                    })
                    .unwrap_or_default();

                let json_rows: Vec<Vec<serde_json::Value>> = rows
                    .iter()
                    .take(display_rows)
                    .map(|row| {
                        (0..row.len())
                            .map(|i| pg_value_to_json(row, i, col_types.get(i).map_or("", |s| s)))
                            .collect()
                    })
                    .collect();

                Ok((columns, json_rows, row_count, elapsed, truncated, false))
            } else {
                let exec = sqlx::query(stmt_trimmed).execute(&mut *conn);
                let _result = if timeout_secs > 0 {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(timeout_secs),
                        exec,
                    ).await {
                        Ok(result) => result?,
                        Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                    }
                } else {
                    exec.await?
                };
                let elapsed = stmt_start.elapsed().as_millis() as u64;
                Ok((Vec::new(), Vec::new(), 0u64, elapsed, false, true))
            }
        }
        .await;

        match stmt_result {
            Ok((columns, json_rows, row_count, elapsed, truncated, _is_dml)) => {
                *succeeded += 1;
                *total_rows += row_count;
                let result_obj = json!({
                    "type": "result", "seq": seq, "total": total, "status": "ok",
                    "sql": stmt_trimmed, "row_count": row_count,
                    "affected_rows": serde_json::Value::Null,
                    "execution_time_ms": elapsed, "columns": columns,
                    "rows": json_rows, "rows_truncated": truncated,
                });
                emit(&result_obj.to_string());
            }
            Err(e) => {
                *failed += 1;
                if in_transaction {
                    sqlx::query("ROLLBACK").execute(&mut *conn).await.ok();
                    in_transaction = false;
                }
                let result_obj = json!({
                    "type": "result", "seq": seq, "total": total, "status": "error",
                    "sql": stmt_trimmed, "error": format!("{}", e), "execution_time_ms": 0,
                });
                emit(&result_obj.to_string());
                if mode == "transaction" {
                    break;
                }
            }
        }
    }

    if in_transaction {
        sqlx::query("COMMIT").execute(&mut *conn).await.ok();
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
    _total_affected: &mut u64,
) -> Result<()>
where
    F: FnMut(&str),
{
    use sqlx::mysql::{MySqlPoolOptions, MySqlRow};
    use sqlx::{Column, Executor, Row, TypeInfo};

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

        if stmt_trimmed.is_empty() || stmt_trimmed.to_uppercase().starts_with("USE ") {
            continue;
        }

        let progress = json!({
            "type": "progress",
            "seq": seq,
            "total": total,
            "sql": stmt_trimmed,
        });
        emit(&progress.to_string());

        let stmt_start = Instant::now();
        let upper = stmt_trimmed.to_uppercase();

        let stmt_result: anyhow::Result<StatementResult> = async {
            if upper.starts_with("SELECT")
                || upper.starts_with("WITH")
                || upper.starts_with("EXPLAIN")
                || upper.starts_with("SHOW")
                || upper.starts_with("DESCRIBE")
                || upper.starts_with("DESC ")
                || upper.contains("RETURNING")
            {
                let fetch = sqlx::query(stmt_trimmed).fetch_all(&mut *conn);
                let rows: Vec<MySqlRow> = if timeout_secs > 0 {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(timeout_secs),
                        fetch,
                    ).await {
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

                let columns: Vec<serde_json::Value> = rows
                    .first()
                    .map(|first_row| {
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
                    })
                    .unwrap_or_default();

                let json_rows: Vec<Vec<serde_json::Value>> = rows
                    .iter()
                    .take(display_rows)
                    .map(|row| {
                        (0..row.len())
                            .map(|i| {
                                mysql_value_to_json(row, i, col_types.get(i).map_or("", |s| s))
                            })
                            .collect()
                    })
                    .collect();

                Ok((columns, json_rows, row_count, elapsed, truncated, false))
            } else {
                let exec = sqlx::query(stmt_trimmed).execute(&mut *conn);
                let _result = if timeout_secs > 0 {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(timeout_secs),
                        exec,
                    ).await {
                        Ok(result) => result?,
                        Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
                    }
                } else {
                    exec.await?
                };
                let elapsed = stmt_start.elapsed().as_millis() as u64;
                Ok((Vec::new(), Vec::new(), 0u64, elapsed, false, true))
            }
        }
        .await;

        match stmt_result {
            Ok((columns, json_rows, row_count, elapsed, truncated, _is_dml)) => {
                *succeeded += 1;
                *total_rows += row_count;
                let result_obj = json!({
                    "type": "result", "seq": seq, "total": total, "status": "ok",
                    "sql": stmt_trimmed, "row_count": row_count,
                    "affected_rows": serde_json::Value::Null,
                    "execution_time_ms": elapsed, "columns": columns,
                    "rows": json_rows, "rows_truncated": truncated,
                });
                emit(&result_obj.to_string());
            }
            Err(e) => {
                *failed += 1;
                if in_transaction {
                    conn.execute("ROLLBACK").await.ok();
                    in_transaction = false;
                }
                let result_obj = json!({
                    "type": "result", "seq": seq, "total": total, "status": "error",
                    "sql": stmt_trimmed, "error": format!("{}", e), "execution_time_ms": 0,
                });
                emit(&result_obj.to_string());
                if mode == "transaction" {
                    break;
                }
            }
        }
    }

    if in_transaction {
        if *failed == 0 {
            conn.execute("COMMIT").await.ok();
        }
        conn.execute("SET autocommit = 1").await.ok();
    }
    drop(conn);
    pool.close().await;
    Ok(())
}

fn sqlite_value_to_json(
    row: &sqlx::sqlite::SqliteRow,
    idx: usize,
    _col_type: &str,
) -> serde_json::Value {
    use sqlx::{Row, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return serde_json::Value::Null;
        }
    }

    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        return json!(v);
    }

    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        return json!(v);
    }

    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&v) {
            return parsed;
        }
        return json!(v);
    }

    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        return json!(v);
    }

    serde_json::Value::Null
}

fn pg_value_to_json(row: &sqlx::postgres::PgRow, idx: usize, col_type: &str) -> serde_json::Value {
    use sqlx::{Row, ValueRef};
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return serde_json::Value::Null;
        }
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
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&v) {
            return parsed;
        }
        let upper = col_type.to_uppercase();
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
    serde_json::Value::Null
}

fn mysql_value_to_json(
    row: &sqlx::mysql::MySqlRow,
    idx: usize,
    col_type: &str,
) -> serde_json::Value {
    use sqlx::{Row, ValueRef};
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return serde_json::Value::Null;
        }
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
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&v) {
            return parsed;
        }
        let upper = col_type.to_uppercase();
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
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&s) {
            return parsed;
        }
        return json!(s.to_string());
    }
    serde_json::Value::Null
}

#[cfg(test)]
mod tests {
    use super::*;

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
