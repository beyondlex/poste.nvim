//! SQL Server introspection via tiberius.
//!
//! Mirrors the postgres driver's item shapes. The `Dialect` SQL templates use
//! `{}` placeholders, which are inlined here with single-quote escaping
//! (tiberius has no sqlx-style binding; same approach as the mysql driver).

use anyhow::Result;
use serde_json::{json, Value};

use super::{IntrospectParams, IntrospectType};
use crate::sql_ddl;
use crate::sql_dialect::{Dialect, MssqlDialect};
use crate::sql_executor::mssql::{connect_mssql, mssql_query, MssqlClient};

/// Inline `{}` placeholders with single-quote-escaped string literals.
fn inline(sql: &str, args: &[&str]) -> String {
    let mut out = sql.to_string();
    for arg in args {
        let escaped = arg.replace('\'', "''");
        out = out.replacen("{}", &escaped, 1);
    }
    out
}

async fn rows(client: &mut MssqlClient, sql: &str) -> Result<Vec<Vec<Value>>> {
    let (_columns, json_rows) = mssql_query(client, sql, 0).await?;
    Ok(json_rows)
}

/// Lenient integer read: DMV aggregates can arrive as numeric (string in JSON).
fn cell(row: &[Value], i: usize) -> &Value {
    row.get(i).unwrap_or(&Value::Null)
}

fn as_int(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

pub(super) async fn introspect_mssql(params: &IntrospectParams) -> Result<Value> {
    let mut client = connect_mssql(&params.connection_url).await?;
    let dialect = MssqlDialect;

    let items: Vec<Value> = match params.introspect_type {
        IntrospectType::Databases => rows(&mut client, dialect.list_databases())
            .await?
            .into_iter()
            .map(|r| json!({ "name": cell(&r, 0) }))
            .collect(),
        IntrospectType::Schemas => {
            let sql = dialect.list_schemas().unwrap();
            rows(&mut client, sql)
                .await?
                .into_iter()
                .map(|r| json!({ "name": cell(&r, 0) }))
                .collect()
        }
        IntrospectType::Tables => {
            let schema = params.schema.as_deref().unwrap_or("dbo");
            let sql = inline(dialect.list_tables(), &[schema]);
            // Extended-property comments need per-table lookups; skip for now.
            rows(&mut client, &sql)
                .await?
                .into_iter()
                .map(
                    |r| json!({ "name": cell(&r, 0), "type": cell(&r, 1), "comment": Value::Null }),
                )
                .collect()
        }
        IntrospectType::Columns => {
            let schema = params.schema.as_deref().unwrap_or("dbo");
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for columns introspection")
            })?;
            let fk_sql = inline(
                "SELECT c.name AS column_name, rt.name AS fk_table, rc.name AS fk_column \
                 FROM sys.foreign_keys fk \
                 JOIN sys.foreign_key_columns fkc ON fk.object_id = fkc.constraint_object_id \
                 JOIN sys.columns c ON fkc.parent_object_id = c.object_id AND fkc.parent_column_id = c.column_id \
                 JOIN sys.tables rt ON fk.referenced_object_id = rt.object_id \
                 JOIN sys.columns rc ON fkc.referenced_object_id = rc.object_id AND fkc.referenced_column_id = rc.column_id \
                 JOIN sys.tables t ON fk.parent_object_id = t.object_id \
                 JOIN sys.schemas s ON t.schema_id = s.schema_id \
                 WHERE s.name = '{}' AND t.name = '{}'",
                &[schema, table],
            );
            let fk_rows = rows(&mut client, &fk_sql).await.unwrap_or_default();
            let fk_map: std::collections::HashMap<String, (String, String)> = fk_rows
                .iter()
                .map(|r| {
                    (
                        cell(&r, 0).as_str().unwrap_or_default().to_string(),
                        (
                            cell(&r, 1).as_str().unwrap_or_default().to_string(),
                            cell(&r, 2).as_str().unwrap_or_default().to_string(),
                        ),
                    )
                })
                .collect();
            let sql = inline(dialect.list_columns(), &[schema, table]);
            rows(&mut client, &sql)
                .await?
                .into_iter()
                .map(|r| {
                    let col_name = cell(&r, 0).as_str().unwrap_or_default().to_string();
                    let fk = fk_map.get(&col_name);
                    json!({
                        "name": col_name,
                        "type": cell(&r, 1),
                        "nullable": cell(&r, 2).as_str() == Some("YES"),
                        "default": cell(&r, 3),
                        "max_length": as_int(&cell(&r, 4)),
                        "comment": Value::Null,
                        "fk_table": fk.map(|f| f.0.as_str()),
                        "fk_column": fk.map(|f| f.1.as_str()),
                    })
                })
                .collect()
        }
        IntrospectType::Indexes => {
            let schema = params.schema.as_deref().unwrap_or("dbo");
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for indexes introspection")
            })?;
            let sql = inline(dialect.list_indexes(), &[schema, table]);
            let raw = rows(&mut client, &sql).await?;
            // The index listing is one row per (index, column); group columns.
            let mut order: Vec<String> = Vec::new();
            let mut grouped: std::collections::HashMap<String, Vec<Value>> =
                std::collections::HashMap::new();
            let mut meta: std::collections::HashMap<String, (bool, bool)> =
                std::collections::HashMap::new();
            for r in raw {
                let name = cell(&r, 0).as_str().unwrap_or_default().to_string();
                let unique = cell(&r, 1).as_bool().unwrap_or(false);
                let is_pk = cell(&r, 2).as_bool().unwrap_or(false);
                let column = cell(&r, 3).as_str().unwrap_or_default().to_string();
                if !meta.contains_key(&name) {
                    order.push(name.clone());
                    meta.insert(name.clone(), (unique, is_pk));
                }
                grouped.entry(name.clone()).or_default().push(json!(column));
            }
            let q_table = dialect.quote_identifier(table);
            order
                .into_iter()
                .map(|name| {
                    let (unique, is_pk) = meta[&name].clone();
                    let cols = grouped[&name]
                        .iter()
                        .map(|c| c.as_str().unwrap_or_default().to_string())
                        .collect::<Vec<_>>();
                    let definition = if is_pk {
                        format!("PRIMARY KEY constraint ({})", cols.join(", "))
                    } else {
                        format!(
                            "CREATE {} INDEX {} ON {} ({})",
                            if unique { "UNIQUE" } else { "" },
                            dialect.quote_identifier(&name),
                            q_table,
                            cols.join(", ")
                        )
                    };
                    json!({
                        "name": name,
                        "definition": definition,
                        "unique": unique,
                        "columns": grouped[&name].clone(),
                    })
                })
                .collect()
        }
        IntrospectType::Ddl => {
            let schema = params.schema.as_deref().unwrap_or("dbo");
            let table = params
                .table
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("table parameter required for ddl introspection"))?;
            build_create_table(&mut client, schema, table).await?
        }
        IntrospectType::DatabaseInfo => {
            let sql = "\
                SELECT DB_NAME() AS name, \
                       CAST(SUM(mf.size) * 8 / 1024 AS VARCHAR(20)) + ' MB' AS total_size, \
                       (SELECT COUNT(*) FROM information_schema.tables \
                        WHERE TABLE_SCHEMA NOT IN ('sys', 'guest', 'INFORMATION_SCHEMA')) AS table_count, \
                       CONVERT(VARCHAR(128), SERVERPROPERTY('Collation')) AS encoding \
                FROM sys.master_files mf \
                WHERE mf.database_id = DB_ID() AND mf.state_desc = 'ONLINE'";
            rows(&mut client, sql)
                .await?
                .into_iter()
                .map(|r| {
                    json!({
                        "name": cell(&r, 0),
                        "total_size": cell(&r, 1),
                        "table_count": as_int(&cell(&r, 2)),
                        "encoding": cell(&r, 3),
                    })
                })
                .collect()
        }
        IntrospectType::TableInfo => {
            let schema = params.schema.as_deref().unwrap_or("dbo");
            let table = params.table.as_deref().ok_or_else(|| {
                anyhow::anyhow!("table parameter required for table_info introspection")
            })?;
            let sql = inline(
                "SELECT t.name AS table_name, s.name AS schema_name, \
                        ISNULL(SUM(CASE WHEN ps.index_id IN (0, 1) THEN ps.row_count ELSE 0 END), 0) AS row_count, \
                        ISNULL(SUM(CASE WHEN ps.index_id IN (0, 1) THEN \
                            ps.in_row_data_page_count + ps.lob_used_page_count \
                            + ps.row_overflow_used_page_count ELSE 0 END), 0) * 8 AS data_kb, \
                        ISNULL(SUM(CASE WHEN ps.index_id > 1 THEN ps.used_page_count \
                            ELSE 0 END), 0) * 8 AS index_kb \
                 FROM sys.tables t \
                 JOIN sys.schemas s ON t.schema_id = s.schema_id \
                 LEFT JOIN sys.dm_db_partition_stats ps \
                   ON ps.object_id = t.object_id AND ps.index_id IN (0, 1, 2) \
                 WHERE s.name = '{}' AND t.name = '{}' \
                 GROUP BY t.name, s.name",
                &[schema, table],
            );
            rows(&mut client, &sql)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| {
                    let data_kb = as_int(&cell(&r, 3));
                    let index_kb = as_int(&cell(&r, 4));
                    json!({
                        "table_name": cell(&r, 0),
                        "schema_name": cell(&r, 1),
                        "total_size": format!("{} KB", data_kb + index_kb),
                        "data_size": format!("{} KB", data_kb),
                        "index_size": format!("{} KB", index_kb),
                        "row_count_estimate": as_int(&cell(&r, 2)),
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
        "dialect": "mssql",
    }))
}

async fn build_create_table(
    client: &mut MssqlClient,
    schema: &str,
    table: &str,
) -> Result<Vec<Value>> {
    let dialect = MssqlDialect;

    let col_sql = inline(dialect.list_columns(), &[schema, table]);
    let col_rows = rows(client, &col_sql).await?;

    let columns: Vec<sql_ddl::ColumnDef> = col_rows
        .iter()
        .map(|r| sql_ddl::ColumnDef {
            name: cell(&r, 0).as_str().unwrap_or_default().to_string(),
            col_type: cell(&r, 1).as_str().unwrap_or_default().to_string(),
            nullable: cell(&r, 2).as_str() == Some("YES"),
            default: cell(&r, 3).as_str().map(|s| s.to_string()),
            comment: None,
            extra: None,
        })
        .collect();

    let pk_sql = inline(
        "SELECT kc.COLUMN_NAME AS column_name \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kc \
           ON kc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME \
          AND kc.TABLE_SCHEMA = tc.TABLE_SCHEMA \
         WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY' \
           AND tc.TABLE_SCHEMA = '{}' AND tc.TABLE_NAME = '{}' \
         ORDER BY kc.ORDINAL_POSITION",
        &[schema, table],
    );
    let mut pk_cols: Vec<String> = rows(client, &pk_sql)
        .await?
        .iter()
        .map(|r| cell(&r, 0).as_str().unwrap_or_default().to_string())
        .collect();
    pk_cols.retain(|c| !c.is_empty());

    let schema_def = sql_ddl::TableSchema {
        name: table.to_string(),
        columns,
        primary_key: if pk_cols.is_empty() {
            None
        } else {
            Some(pk_cols)
        },
        comment: None,
    };

    if let Some(ddl_generator) = sql_ddl::ddl_for("mssql") {
        let ddl = ddl_generator.create_table(&schema_def);
        return Ok(vec![
            json!({"ddl": ddl, "type": "ddl", "table": table, "dialect": "mssql"}),
        ]);
    }

    Ok(vec![
        json!({"ddl": format!("-- Could not create DDL for table '{}'", table)}),
    ])
}
