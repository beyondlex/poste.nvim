//! SQL execution drivers shared by the CLI's exec-file / session loops.
//!
//! The per-dialect `execute_*` orchestrators the HTTP transport used went
//! with it: the CLI splits statements itself (sql_parser::split_statements_with)
//! and drives each dialect's connect/query helpers directly. mssql and
//! clickhouse stay here because their connect/query helpers are that shared
//! surface (sqlx's postgres/mysql/sqlite counterparts live in sql_exec_common).

mod value;

pub mod clickhouse;
pub mod mssql;

#[cfg(test)]
mod tests {
    // Driver smoke tests: prove the sqlx/tiberius stacks build and answer on
    // the spot (no poste code between them and the driver).
    #[tokio::test]
    async fn test_sqlite_in_memory() {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool: sqlx::Pool<sqlx::Sqlite> = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query("SELECT 1 as num")
            .fetch_all(&pool)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        pool.close().await;
    }
}
