use anyhow::Result;
use clap::Parser;
use serde::Serialize;
use std::io::Read;

use poste_core::sql_context::{self, ContextResult, SqlDialect};

#[derive(Parser)]
pub enum ContextAction {
    /// Detect SQL completion context at cursor position
    Detect {
        /// Byte offset of cursor within SQL text (0-based)
        offset: usize,
        /// Optional dialect for function filtering (generic, postgres, mysql, sqlite)
        #[arg(long, default_value = "generic")]
        dialect: String,
    },
    /// Find statement boundaries containing a cursor line
    Stmt {
        /// Cursor line number (0-based)
        cursor_line: usize,
    },
    /// Find ALL statement boundary line ranges in the given text
    StmtRanges,
    /// Persistent server mode: read line-delimited JSON requests from stdin
    Serve,
}

#[derive(Serialize)]
pub struct ContextDetectResponse {
    pub version: u32,
    pub ctx_type: String,
    pub ctx_data: Option<String>,
    pub ctx_schema: Option<String>,
    pub prefix: String,
    pub tables: Vec<TableRefInfo>,
    pub functions: Vec<&'static str>,
    pub in_string: bool,
    pub in_comment: bool,
}

#[derive(Serialize)]
pub struct TableRefInfo {
    pub name: String,
    pub alias: Option<String>,
    pub schema: Option<String>,
}

#[derive(Serialize)]
pub struct ContextStmtResponse {
    pub start_line: usize,
    pub end_line: usize,
}

pub(crate) fn make_detect_response(result: &ContextResult) -> ContextDetectResponse {
    let ctx_type = result.context_type.name().to_string();
    let ctx_data = result.context_type.data();
    let ctx_schema = match &result.context_type {
        sql_context::ContextType::DotColumn { schema, .. } => schema.clone(),
        sql_context::ContextType::SchemaTable { schema } => Some(schema.clone()),
        _ => None,
    };
    let tables: Vec<TableRefInfo> = result
        .tables
        .iter()
        .map(|t| TableRefInfo {
            name: t.name.clone(),
            alias: t.alias.clone(),
            schema: t.schema.clone(),
        })
        .collect();
    ContextDetectResponse {
        version: 1,
        ctx_type,
        ctx_data,
        ctx_schema,
        prefix: result.prefix.clone(),
        tables,
        functions: result.functions.clone(),
        in_string: result.in_string,
        in_comment: result.in_comment,
    }
}

pub fn execute(action: ContextAction) -> Result<()> {
    match action {
        ContextAction::Detect { offset, dialect } => {
            let mut sql = String::new();
            std::io::stdin().read_to_string(&mut sql)?;
            let dialect = match dialect.as_str() {
                "postgres" => SqlDialect::Postgres,
                "mysql" => SqlDialect::MySql,
                "sqlite" => SqlDialect::Sqlite,
                _ => SqlDialect::Generic,
            };
            let result = sql_context::detect_context_with_dialect(&sql, offset, dialect);
            let response = match result {
                Some(ctx) => make_detect_response(&ctx),
                None => ContextDetectResponse {
                    version: 1,
                    ctx_type: "keyword".into(),
                    ctx_data: None,
                    ctx_schema: None,
                    prefix: String::new(),
                    tables: vec![],
                    functions: vec![],
                    in_string: true,
                    in_comment: true,
                },
            };
            println!("{}", serde_json::to_string(&response)?);
        }
        ContextAction::Stmt { cursor_line } => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let lines: Vec<&str> = input.lines().collect();
            let span = sql_context::find_statement_span(&lines, cursor_line);
            let response = match span {
                Some((start, end)) => ContextStmtResponse {
                    start_line: start,
                    end_line: end,
                },
                None => ContextStmtResponse {
                    start_line: 0,
                    end_line: 0,
                },
            };
            println!("{}", serde_json::to_string(&response)?);
        }
        ContextAction::StmtRanges => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let lines: Vec<&str> = input.lines().collect();
            let ranges = sql_context::find_all_statement_ranges(&lines);
            println!("{}", serde_json::to_string(&ranges)?);
        }
        ContextAction::Serve => {
            crate::serve::handle_serve()?;
        }
    }

    Ok(())
}
