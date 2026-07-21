use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};

use crate::context::{self, ContextDetectResponse, ContextStmtResponse};

#[derive(Deserialize)]
struct ServeRequest {
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Serialize)]
struct ServeResponse {
    id: u64,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct DetectParams {
    sql: String,
    offset: usize,
    #[serde(default = "default_dialect")]
    dialect: String,
}

fn default_dialect() -> String {
    "generic".to_string()
}

#[derive(Deserialize)]
struct StmtParams {
    sql: String,
    cursor_line: usize,
}

pub fn handle_serve() -> Result<()> {
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: ServeRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[poste serve] invalid request: {}", e);
                continue;
            }
        };

        let response = match request.method.as_str() {
            "detect" => match serde_json::from_value::<DetectParams>(request.params) {
                Ok(params) => {
                    let dialect = match params.dialect.as_str() {
                        "postgres" => poste_core::sql_context::SqlDialect::Postgres,
                        "mysql" => poste_core::sql_context::SqlDialect::MySql,
                        "sqlite" => poste_core::sql_context::SqlDialect::Sqlite,
                        _ => poste_core::sql_context::SqlDialect::Generic,
                    };
                    let result = poste_core::sql_context::detect_context_with_dialect(
                        &params.sql,
                        params.offset,
                        dialect,
                    );
                    let ctx_resp = match result {
                        Some(ctx) => context::make_detect_response(&ctx),
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
                    let val = serde_json::to_value(&ctx_resp).unwrap_or_default();
                    ServeResponse {
                        id: request.id,
                        ok: true,
                        result: Some(val),
                        error: None,
                    }
                }
                Err(e) => ServeResponse {
                    id: request.id,
                    ok: false,
                    result: None,
                    error: Some(format!("invalid detect params: {}", e)),
                },
            },
            "stmt" => match serde_json::from_value::<StmtParams>(request.params) {
                Ok(params) => {
                    let lines: Vec<&str> = params.sql.lines().collect();
                    let span =
                        poste_core::sql_context::find_statement_span(&lines, params.cursor_line);
                    let stmt_resp = match span {
                        Some((start, end)) => ContextStmtResponse {
                            start_line: start,
                            end_line: end,
                        },
                        None => ContextStmtResponse {
                            start_line: 0,
                            end_line: 0,
                        },
                    };
                    let val = serde_json::to_value(&stmt_resp).unwrap_or_default();
                    ServeResponse {
                        id: request.id,
                        ok: true,
                        result: Some(val),
                        error: None,
                    }
                }
                Err(e) => ServeResponse {
                    id: request.id,
                    ok: false,
                    result: None,
                    error: Some(format!("invalid stmt params: {}", e)),
                },
            },
            _ => ServeResponse {
                id: request.id,
                ok: false,
                result: None,
                error: Some(format!("unknown method: {}", request.method)),
            },
        };

        let json = serde_json::to_string(&response)?;
        writeln!(out, "{}", json)?;
        out.flush()?;
    }

    Ok(())
}
