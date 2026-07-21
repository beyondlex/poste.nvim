use anyhow::Result;
use serde_json::{json, Value};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;

use super::{IntrospectParams, IntrospectType};
use crate::sql_connection::normalize_sqlite_connection;
use crate::sql_dialect::{Dialect, SqliteDialect};

pub(super) async fn introspect_sqlite(params: &IntrospectParams) -> Result<Value> {
    let conn_str = normalize_sqlite_connection(&params.connection_url)?;

    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&conn_str)
        .await?;

    let dialect = SqliteDialect;

    let items: Vec<Value> = match params.introspect_type {
        IntrospectType::Databases => {
            let sql = dialect.list_databases();
            let rows = sqlx::query(sql).fetch_all(&pool).await?;
            rows.iter()
                .map(|row| {
                    json!({
                        "name": row.get::<String, _>("name"),
                        "file": row.get::<Option<String>, _>("file"),
                    })
                })
                .collect()
        }
        IntrospectType::Schemas => Vec::new(),
        IntrospectType::Tables => {
            let sql = dialect.list_tables();
            let rows = sqlx::query(sql).fetch_all(&pool).await?;
            rows.iter()
                .map(|row| {
                    json!({
                        "name": row.get::<String, _>("name"),
                        "type": "BASE TABLE",
                    })
                })
                .collect()
        }
        IntrospectType::Columns => {
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for columns introspection")
            })?;
            let quoted = dialect.quote_identifier(table);
            let sql = dialect.list_columns().replace("{}", &quoted);
            let rows = sqlx::query(&sql).fetch_all(&pool).await?;
            let fk_pragma = format!(
                "SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list('{}')",
                table
            );
            let fk_rows = sqlx::query(&fk_pragma)
                .fetch_all(&pool)
                .await
                .unwrap_or_default();
            let fk_map: std::collections::HashMap<String, (String, String)> = fk_rows
                .iter()
                .map(|r| {
                    (
                        r.get::<String, _>("from"),
                        (r.get::<String, _>("table"), r.get::<String, _>("to")),
                    )
                })
                .collect();
            rows.iter()
                .map(|row| {
                    let col_name: String = row.get("name");
                    let (fk_table, fk_column) = fk_map.get(&col_name).cloned().unwrap_or_default();
                    json!({
                        "name": col_name,
                        "type": row.get::<String, _>("type"),
                        "nullable": row.get::<i64, _>("notnull") == 0,
                        "default": row.get::<Option<String>, _>("dflt_value"),
                        "pk": row.get::<i64, _>("pk") > 0,
                        "comment": null,
                        "fk_table": fk_table,
                        "fk_column": fk_column,
                    })
                })
                .collect()
        }
        IntrospectType::Indexes => {
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for indexes introspection")
            })?;
            let quoted = dialect.quote_identifier(table);
            let sql = dialect.list_indexes().replace("{}", &quoted);
            let rows = sqlx::query(&sql).fetch_all(&pool).await?;
            let mut items: Vec<Value> = Vec::new();
            for row in &rows {
                let name: String = row.get("name");
                let unique: bool = row.get::<i64, _>("unique") > 0;
                let mut columns: Vec<String> = Vec::new();
                if let Ok(info_rows) = sqlx::query("SELECT name FROM pragma_index_info(?)")
                    .bind(&name)
                    .fetch_all(&pool)
                    .await
                {
                    for info in &info_rows {
                        columns.push(info.get::<String, _>("name"));
                    }
                }
                items.push(json!({
                    "name": name,
                    "unique": unique,
                    "columns": columns,
                }));
            }
            items
        }
        IntrospectType::Ddl => {
            let table = params
                .table
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("table parameter required for ddl introspection"))?;
            build_create_table_from_introspect_sqlite(&pool, table).await?
        }
    };

    pool.close().await;

    Ok(json!({
        "type": "introspect",
        "introspect_type": params.introspect_type.as_str(),
        "items": items,
        "table": params.table,
        "dialect": "sqlite",
    }))
}

async fn build_create_table_from_introspect_sqlite(
    pool: &SqlitePool,
    table: &str,
) -> Result<Vec<serde_json::Value>> {
    use sqlx::Row;

    let sql = "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1";
    let rows = sqlx::query(sql).bind(table).fetch_all(pool).await?;

    if let Some(row) = rows.first() {
        let create_sql: Option<String> = row.get("sql");
        if let Some(ddl) = create_sql {
            return Ok(vec![
                json!({"ddl": ddl, "type": "ddl", "table": table, "dialect": "sqlite"}),
            ]);
        }
    }

    Ok(vec![
        json!({"ddl": format!("-- Table '{}' not found", table)}),
    ])
}
