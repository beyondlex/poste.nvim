//! ClickHouse introspection over the raw HTTP transport.
//!
//! Item shapes mirror the postgres driver. `Dialect` SQL templates use `{}`
//! placeholders inlined with backtick-escaped identifiers, like mssql.

use anyhow::Result;
use serde_json::{json, Value};

use super::{IntrospectParams, IntrospectType};
use crate::sql_ddl;
use crate::sql_dialect::{ClickHouseDialect, Dialect};
use crate::sql_executor::clickhouse::{
    clickhouse_post, connect_clickhouse, database, ClickHouseClient,
};

fn inline(sql: &str, args: &[&str]) -> String {
    let mut out = sql.to_string();
    for arg in args {
        let escaped = arg.replace('`', "``");
        out = out.replacen("{}", &escaped, 1);
    }
    out
}

async fn rows(client: &ClickHouseClient, sql: &str) -> Result<Vec<Vec<Value>>> {
    let ch = clickhouse_post(client, sql, 0).await?;
    Ok(ch.rows)
}

fn cell(row: &[Value], i: usize) -> &Value {
    row.get(i).unwrap_or(&Value::Null)
}

fn as_u64(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

pub(super) async fn introspect_clickhouse(params: &IntrospectParams) -> Result<Value> {
    let client = connect_clickhouse(&params.connection_url).await?;
    let dialect = ClickHouseDialect;

    let items: Vec<Value> = match params.introspect_type {
        IntrospectType::Databases => rows(&client, dialect.list_databases())
            .await?
            .into_iter()
            .map(|r| json!({ "name": cell(&r, 0) }))
            .collect(),
        IntrospectType::Schemas => Vec::new(),
        IntrospectType::Tables => {
            // ClickHouse has no schema level — the namespace is the database
            // from the connection URL (or the --database flag, which rewrites
            // the URL). Accept --schema as an override for symmetry.
            let db = params
                .schema
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(database(&client))
                .to_string();
            let sql = inline(dialect.list_tables(), &[&db]);
            rows(&client, &sql)
                .await?
                .into_iter()
                .map(
                    |r| json!({ "name": cell(&r, 0), "type": cell(&r, 1), "comment": Value::Null }),
                )
                .collect()
        }
        IntrospectType::Columns => {
            let db = database(&client).to_string();
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for columns introspection")
            })?;
            let sql = inline(dialect.list_columns(), &[&db, table]);
            rows(&client, &sql)
                .await?
                .into_iter()
                .map(|r| {
                    let col_type = cell(&r, 1).as_str().unwrap_or("");
                    json!({
                        "name": cell(&r, 0),
                        "type": cell(&r, 1),
                        "nullable": col_type.starts_with("Nullable("),
                        "default": cell(&r, 2),
                        "max_length": Value::Null,
                        "comment": Value::Null,
                        "fk_table": Value::Null,
                        "fk_column": Value::Null,
                    })
                })
                .collect()
        }
        IntrospectType::Indexes => {
            let db = database(&client).to_string();
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for indexes introspection")
            })?;
            let sql = inline(dialect.list_indexes(), &[&db, table]);
            rows(&client, &sql)
                .await?
                .into_iter()
                .map(|r| {
                    let expr = cell(&r, 1).as_str().unwrap_or("");
                    json!({
                        "name": cell(&r, 0),
                        "definition": format!("data skipping index ({}) on ({})", expr, cell(&r, 2).as_str().unwrap_or("")),
                        "unique": false,
                        "columns": vec![json!(expr)],
                    })
                })
                .collect()
        }
        IntrospectType::Ddl => {
            let db = (database(&client)).to_string();
            let table = params
                .table
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("table parameter required for ddl introspection"))?;
            build_create_table(&client, &db, table).await?
        }
        IntrospectType::DatabaseInfo => {
            let sql = "SELECT currentDatabase() AS name, \
                       (SELECT count() FROM system.tables WHERE database = currentDatabase()) AS table_count, \
                       version() AS encoding";
            rows(&client, sql)
                .await?
                .into_iter()
                .map(|r| {
                    json!({
                        "name": cell(&r, 0),
                        "total_size": "n/a",
                        "table_count": as_u64(&cell(&r, 1)),
                        "encoding": cell(&r, 2),
                    })
                })
                .collect()
        }
        IntrospectType::TableInfo => {
            let db = (database(&client)).to_string();
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for table_info introspection")
            })?;
            let sql = inline(
                "SELECT name, engine, total_rows, total_bytes \
                 FROM system.tables WHERE database = '{}' AND name = '{}'",
                &[&db, table],
            );
            rows(&client, &sql)
                .await?
                .into_iter()
                .map(|r| {
                    let bytes = as_u64(&cell(&r, 3));
                    json!({
                        "table_name": cell(&r, 0),
                        "schema_name": db,
                        "total_size": format!("{} KB", bytes / 1024),
                        "data_size": format!("{} KB", bytes / 1024),
                        "index_size": "0 KB",
                        "row_count_estimate": as_u64(&cell(&r, 2)),
                        "comment": Value::Null,
                    })
                })
                .collect()
        }
    };

    Ok(json!({
        "type": "introspect",
        "introspect_type": params.introspect_type.as_str(),
        "items": items,
        "schema": params.schema,
        "table": params.table,
        "dialect": "clickhouse",
    }))
}

async fn build_create_table(
    client: &ClickHouseClient,
    db: &str,
    table: &str,
) -> Result<Vec<Value>> {
    let dialect = ClickHouseDialect;

    let col_sql = inline(dialect.list_columns(), &[db, table]);
    let col_rows = rows(client, &col_sql).await?;

    let columns: Vec<sql_ddl::ColumnDef> = col_rows
        .iter()
        .map(|r| {
            let col_type = cell(&r, 1).as_str().unwrap_or_default().to_string();
            sql_ddl::ColumnDef {
                name: cell(&r, 0).as_str().unwrap_or_default().to_string(),
                col_type,
                nullable: cell(&r, 1).as_str().unwrap_or("").starts_with("Nullable("),
                default: cell(&r, 2).as_str().map(|s| s.to_string()),
                comment: None,
                extra: None,
            }
        })
        .collect();

    // ClickHouse has no PK constraints; ORDER BY is the table key. Use the
    // first column as a stand-in (the generator appends ENGINE + ORDER BY).
    let mut pk_cols: Vec<String> = Vec::new();
    if let Some(first) = col_rows.first() {
        if let Some(name) = first.get(0).and_then(|v| v.as_str()) {
            pk_cols.push(name.to_string());
        }
    }

    let schema_def = sql_ddl::TableSchema {
        name: format!("{}.{}", db, table),
        columns,
        primary_key: if pk_cols.is_empty() {
            None
        } else {
            Some(pk_cols)
        },
        comment: None,
    };

    if let Some(ddl_generator) = sql_ddl::ddl_for("clickhouse") {
        let ddl = ddl_generator.create_table(&schema_def);
        return Ok(vec![
            json!({"ddl": ddl, "type": "ddl", "table": table, "dialect": "clickhouse"}),
        ]);
    }

    Ok(vec![
        json!({"ddl": format!("-- Could not create DDL for table '{}'", table)}),
    ])
}
