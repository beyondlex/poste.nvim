//! Shared per-statement execution scaffolding for the five dialect paths
//! (`poste-cli` exec-file + session): statement classification, the timeout
//! wrapper, wire-event builders and row truncation.
//!
//! The per-dialect VALUE CONVERTERS — the documented "session is the live
//! converter, exec-file stays in sync" contract — live in `sql_values`.
//! What remains dialect-specific after both extractions is only connection
//! handling and transaction semantics.

use serde_json::{json, Value};
use std::future::Future;

// ---------------------------------------------------------------------------
// Statement classification
// ---------------------------------------------------------------------------

/// The query/DML split per dialect: a statement is fetched (rows expected)
/// when its blanked-literals form starts with one of `starts_with`, or
/// contains one of `contains` (the RETURNING rule: DML with RETURNING
/// yields rows on pg/sqlite). mssql keeps its own classifier in
/// `sql_executor::mssql::is_query_stmt`; ClickHouse decides from the
/// response shape.
pub struct QueryKinds {
    pub starts_with: &'static [&'static str],
    pub contains: &'static [&'static str],
}

pub const SQLITE_QUERY: QueryKinds = QueryKinds {
    starts_with: &["SELECT", "WITH", "EXPLAIN", "PRAGMA", "VALUES"],
    contains: &["RETURNING"],
};

pub const POSTGRES_QUERY: QueryKinds = QueryKinds {
    starts_with: &["SELECT", "WITH", "EXPLAIN", "SHOW", "TABLE ", "VALUES"],
    contains: &["RETURNING"],
};

pub const MYSQL_QUERY: QueryKinds = QueryKinds {
    starts_with: &["SELECT", "WITH", "EXPLAIN", "SHOW", "DESCRIBE", "DESC "],
    contains: &["RETURNING"],
};

/// Classify one statement against `kinds`. String literals are blanked and
/// comments removed first, so `'returning'` inside a value cannot flip DML
/// onto the fetch path, and a leading `/* hint */` cannot hide SELECT from
/// the prefix match (the comment view's doc has both regressions).
///
/// `contains` matches whole words only: `UPDATE orders SET returning_flag = 1`
/// is a DML statement, but a substring test read its column name as the
/// RETURNING clause and took the fetch branch — the statement still ran, yet
/// its result came back as "0 rows" instead of the affected count.
pub fn is_query_with(kinds: &QueryKinds, stmt: &str) -> bool {
    let upper = poste_core::sql_parser::blank_literals_and_comments(stmt)
        .trim_start()
        .to_uppercase();
    kinds.starts_with.iter().any(|p| upper.starts_with(p))
        || kinds.contains.iter().any(|w| {
            upper
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|token| token == *w)
        })
}

/// True for statements that must be skipped silently (empty, `USE` — the
/// database is fixed by the connection URL / `--database`). Trims its input,
/// so pre-trimmed and raw statement text behave the same.
pub fn is_skippable(stmt: &str) -> bool {
    let stmt = stmt.trim();
    stmt.is_empty() || poste_core::sql_parser::is_use_statement(stmt)
}

// ---------------------------------------------------------------------------
// Timed execution
// ---------------------------------------------------------------------------

/// `rows_affected` lives on each dialect's concrete QueryResult type in
/// sqlx 0.8 (the old `Done` trait is gone); this bridge lets the generic
/// sqlx statement runner stay dialect-agnostic.
pub trait RowsAffected {
    fn rows_affected(&self) -> u64;
}

impl RowsAffected for sqlx::sqlite::SqliteQueryResult {
    fn rows_affected(&self) -> u64 {
        Self::rows_affected(self)
    }
}
impl RowsAffected for sqlx::postgres::PgQueryResult {
    fn rows_affected(&self) -> u64 {
        Self::rows_affected(self)
    }
}
impl RowsAffected for sqlx::mysql::MySqlQueryResult {
    fn rows_affected(&self) -> u64 {
        Self::rows_affected(self)
    }
}

/// Run `fut` under the per-statement timeout. `timeout_secs == 0` means no
/// timeout. A timeout is a per-statement ERROR (an event for the session
/// loop, a failed statement for exec-file), never a process kill.
pub async fn timed<F, T>(timeout_secs: u64, fut: F) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    if timeout_secs == 0 {
        return fut.await;
    }
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!("Query timed out after {} seconds", timeout_secs),
    }
}

// ---------------------------------------------------------------------------
// Row blocks and truncation
// ---------------------------------------------------------------------------

/// A fetched resultset, already converted to wire-shaped JSON.
pub struct RowBlock {
    pub columns: Vec<Value>,
    /// All converted rows, BEFORE max_rows truncation.
    pub rows: Vec<Vec<Value>>,
}

/// The per-statement outcome shared by every dialect path — the wire
/// `result` event fields, computed identically everywhere.
pub struct StmtOutcome {
    pub columns: Vec<Value>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: u64,
    pub elapsed_ms: u64,
    pub truncated: bool,
    pub is_dml: bool,
    pub affected: u64,
}

/// Apply the `--max-rows` cap to a converted row list.
/// Returns (display rows, truncated?).
pub fn clamp_rows(rows: Vec<Vec<Value>>, max_rows: u64) -> (Vec<Vec<Value>>, bool) {
    let total = rows.len();
    let truncated = max_rows > 0 && total as u64 > max_rows;
    let take = if max_rows > 0 {
        (total as u64).min(max_rows) as usize
    } else {
        total
    };
    (rows.into_iter().take(take).collect(), truncated)
}

/// Assemble a `RowBlock` from fetched sqlx rows: column metadata from the
/// first row, per-cell conversion via `value_fn`, column JSON via
/// `col_json_fn` (postgres decorates `nullable`, sqlite/mysql do not —
/// kept as a parameter so the wire shape stays byte-identical). Rows are
/// returned UNTRUNCATED; apply `clamp_rows` so `row_count` can stay full.
pub fn sqlx_rows_to_block<R, FV, FC>(rows: &[R], value_fn: FV, col_json_fn: FC) -> RowBlock
where
    R: sqlx::Row,
    FV: Fn(&R, usize, &str) -> Value,
    FC: Fn(&<<R as sqlx::Row>::Database as sqlx::Database>::Column) -> Value,
{
    use sqlx::{Column, TypeInfo};

    let col_types: Vec<String> = rows
        .first()
        .map(|first| {
            first
                .columns()
                .iter()
                .map(|col| col.type_info().name().to_string())
                .collect()
        })
        .unwrap_or_default();

    let columns: Vec<Value> = rows
        .first()
        .map(|first| first.columns().iter().map(col_json_fn).collect())
        .unwrap_or_default();

    let all_rows: Vec<Vec<Value>> = rows
        .iter()
        .map(|row| {
            (0..row.len())
                .map(|i| value_fn(row, i, col_types.get(i).map_or("", |s| s)))
                .collect()
        })
        .collect();

    RowBlock {
        columns,
        rows: all_rows,
    }
}

/// Run one statement against a sqlx connection: classify, fetch+convert or
/// execute (both under the per-statement timeout), and shape the outcome.
/// `total` semantics differ between exec-file (present) and session
/// (absent), so both event builders take `Option<u64>`.
#[allow(clippy::too_many_arguments)]
pub async fn run_sqlx_statement<'e, DB, E, FV, FC>(
    executor: E,
    stmt: &str,
    kinds: &QueryKinds,
    timeout_secs: u64,
    max_rows: u64,
    started: std::time::Instant,
    value_fn: FV,
    col_fn: FC,
) -> anyhow::Result<StmtOutcome>
where
    DB: sqlx::Database,
    for<'q> <DB as sqlx::Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    E: sqlx::Executor<'e, Database = DB>,
    DB::QueryResult: RowsAffected,
    FV: Fn(&DB::Row, usize, &str) -> Value,
    FC: Fn(&DB::Column) -> Value,
{
    if is_query_with(kinds, stmt) {
        let rows: Vec<DB::Row> = timed(timeout_secs, async move {
            let rows = sqlx::query(stmt).fetch_all(executor).await?;
            Ok(rows)
        })
        .await?;
        let row_count = rows.len() as u64;
        let block = sqlx_rows_to_block(&rows, value_fn, col_fn);
        let (display, truncated) = clamp_rows(block.rows, max_rows);
        Ok(StmtOutcome {
            columns: block.columns,
            rows: display,
            row_count,
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated,
            is_dml: false,
            affected: 0,
        })
    } else {
        let result = timed(timeout_secs, async move {
            let r = sqlx::query(stmt).execute(executor).await?;
            Ok(r)
        })
        .await?;
        let affected = RowsAffected::rows_affected(&result);
        Ok(StmtOutcome {
            columns: vec![],
            rows: vec![],
            row_count: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated: false,
            is_dml: true,
            affected,
        })
    }
}

// ---------------------------------------------------------------------------
// Wire event builders
// ---------------------------------------------------------------------------

/// `total` is `Some` on the exec-file stream; the session stream has no
/// total concept and omits the key entirely (byte-stable with the
/// pre-consolidation shapes on both transports).
fn set_total(ev: &mut Value, total: Option<u64>) {
    if let Some(total) = total {
        ev["total"] = json!(total);
    }
}

pub fn progress_event(seq: u64, total: u64, sql: &str) -> Value {
    json!({ "type": "progress", "seq": seq, "total": total, "sql": sql })
}

pub fn ok_result_event(seq: u64, total: Option<u64>, sql: &str, outcome: &StmtOutcome) -> Value {
    let mut ev = json!({
        "type": "result",
        "seq": seq,
        "status": "ok",
        "sql": sql,
        "row_count": outcome.row_count,
        "affected_rows": if outcome.is_dml { json!(outcome.affected) } else { Value::Null },
        "execution_time_ms": outcome.elapsed_ms,
        "columns": outcome.columns,
        "rows": outcome.rows,
        "rows_truncated": outcome.truncated,
    });
    set_total(&mut ev, total);
    ev
}

pub fn error_result_event(
    seq: u64,
    total: Option<u64>,
    sql: &str,
    err: &anyhow::Error,
    elapsed_ms: u64,
) -> Value {
    let mut ev = json!({
        "type": "result",
        "seq": seq,
        "status": "error",
        "sql": sql,
        "error": format!("{}", err),
        "execution_time_ms": elapsed_ms,
    });
    set_total(&mut ev, total);
    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_matches_the_pre_extraction_prefix_sets() {
        assert!(is_query_with(&SQLITE_QUERY, "SELECT 1"));
        assert!(is_query_with(
            &SQLITE_QUERY,
            "with x as (select 1) select * from x"
        ));
        assert!(is_query_with(&SQLITE_QUERY, "PRAGMA table_info(t)"));
        assert!(is_query_with(&POSTGRES_QUERY, "SELECT 1"));
        // SHOW/TABLE are postgres-only
        assert!(!is_query_with(&SQLITE_QUERY, "SHOW TABLES"));
        assert!(is_query_with(&POSTGRES_QUERY, "SHOW search_path"));
        assert!(is_query_with(&POSTGRES_QUERY, "TABLE users"));
        // DESCRIBE/DESC are mysql-only
        assert!(!is_query_with(&POSTGRES_QUERY, "DESCRIBE users"));
        assert!(is_query_with(&MYSQL_QUERY, "DESC users"));
        assert!(is_query_with(&MYSQL_QUERY, "DESCRIBE users"));
        // RETURNING anywhere (literals blanked first)
        assert!(is_query_with(
            &SQLITE_QUERY,
            "INSERT INTO t (a) VALUES (1) RETURNING id"
        ));
        assert!(!is_query_with(
            &SQLITE_QUERY,
            "UPDATE t SET note = 'returning item' WHERE id = 1"
        ));
        assert!(!is_query_with(&SQLITE_QUERY, "DELETE FROM t"));
    }

    #[test]
    fn skippable_statements() {
        assert!(is_skippable(""));
        assert!(is_skippable("   "));
        assert!(is_skippable("USE mydb"));
        assert!(is_skippable("use mydb;"));
        assert!(is_skippable("USE\tmydb"), "any whitespace separates USE");
        assert!(!is_skippable("USELESS"));
        assert!(!is_skippable("SELECT * FROM use_table"));
    }

    #[test]
    fn classification_sees_through_leading_comments() {
        // A hint comment hid SELECT from the prefix match: the statement ran
        // on the execute path and its rows were lost.
        assert!(is_query_with(&POSTGRES_QUERY, "/*+ SeqScan(t) */ SELECT 1"));
        assert!(is_query_with(&POSTGRES_QUERY, "-- header\nSELECT 1"));
        assert!(is_query_with(
            &POSTGRES_QUERY,
            "/* intro */ WITH x AS (SELECT 1) SELECT * FROM x"
        ));
        // "returning" inside a comment must not flip DML onto the fetch path.
        assert!(!is_query_with(
            &POSTGRES_QUERY,
            "UPDATE t SET a = 1 /* returning to baseline */ WHERE id = 1"
        ));
        // A real clause after a comment is still classified.
        assert!(is_query_with(
            &POSTGRES_QUERY,
            "UPDATE t SET a = 1 /* note */ RETURNING id"
        ));
    }

    #[test]
    fn returning_matches_as_a_word() {
        // A column whose name embeds the keyword is still plain DML: reading
        // it as a RETURNING clause sent the statement down the fetch branch,
        // which ran it but reported "0 rows" with no affected count.
        assert!(!is_query_with(
            &POSTGRES_QUERY,
            "UPDATE orders SET returning_flag = 1 WHERE id = 2"
        ));
        assert!(!is_query_with(
            &MYSQL_QUERY,
            "DELETE FROM t WHERE returning_count > 0"
        ));
        assert!(!is_query_with(
            &SQLITE_QUERY,
            "INSERT INTO returning_log (a) VALUES (1)"
        ));
        // the real clause, in every spacing it arrives in
        assert!(is_query_with(
            &POSTGRES_QUERY,
            "UPDATE orders SET returning_flag = 1 RETURNING returning_flag"
        ));
        assert!(is_query_with(&SQLITE_QUERY, "DELETE FROM t RETURNING *"));
        assert!(is_query_with(
            &POSTGRES_QUERY,
            "INSERT INTO t (a) VALUES (1) RETURNING id;"
        ));
    }

    #[test]
    fn clamp_rows_caps_and_flags() {
        let rows: Vec<Vec<Value>> = (1..=8).map(|n| vec![json!(n)]).collect();
        let (display, truncated) = clamp_rows(rows.clone(), 3);
        assert_eq!(display.len(), 3);
        assert!(truncated);

        let (display, truncated) = clamp_rows(rows.clone(), 0);
        assert_eq!(display.len(), 8);
        assert!(!truncated, "0 = unlimited");

        let (display, truncated) = clamp_rows(rows, 8);
        assert_eq!(display.len(), 8);
        assert!(!truncated, "exactly at the cap is not truncated");
    }

    #[test]
    fn event_shapes_are_stable() {
        let p = progress_event(2, 8, "SELECT 1");
        assert_eq!(p["type"], "progress");
        assert_eq!(p["seq"], 2);
        assert_eq!(p["total"], 8);

        let outcome = StmtOutcome {
            columns: vec![json!({"name": "id"})],
            rows: vec![vec![json!(1)]],
            row_count: 1,
            elapsed_ms: 3,
            truncated: false,
            is_dml: false,
            affected: 0,
        };
        let ok = ok_result_event(3, Some(8), "SELECT 1", &outcome);
        assert_eq!(ok["status"], "ok");
        assert_eq!(
            ok["affected_rows"],
            Value::Null,
            "queries carry null affected"
        );
        assert_eq!(ok["row_count"], 1);

        let dml = StmtOutcome {
            columns: vec![],
            rows: vec![],
            row_count: 0,
            elapsed_ms: 1,
            truncated: false,
            is_dml: true,
            affected: 2,
        };
        let ok = ok_result_event(4, None, "INSERT ..", &dml);
        assert_eq!(ok["affected_rows"], 2, "DML carries the affected count");

        let err = error_result_event(5, Some(8), "boom", &anyhow::anyhow!("no such table"), 0);
        assert_eq!(err["status"], "error");
        assert_eq!(err["error"], "no such table");
        assert_eq!(err["total"], 8);

        // session shape: no `total` key at all
        let sess = error_result_event(5, None, "boom", &anyhow::anyhow!("x"), 7);
        assert!(sess.get("total").is_none());
        assert_eq!(sess["execution_time_ms"], 7);
    }
}
