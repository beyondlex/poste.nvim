//! Redis protocol execution.
//!
//! Handles Redis command execution via the `redis` crate, converting
//! responses to structured JSON for Lua-side rendering.

use crate::response::Response;
use anyhow::Result;
use poste_core::Request;
use std::collections::HashMap;

impl crate::executor::Executor {
    /// Execute a Redis command.
    pub async fn execute_redis(request: &Request) -> Result<Response> {
        let connection = if request.connection.is_empty() {
            anyhow::bail!("Redis request missing @connection directive");
        } else {
            &request.connection
        };

        let client = redis::Client::open(connection.as_str())?;
        let mut con = client.get_multiplexed_async_connection().await?;

        // Parse the command from body: concatenate all non-empty, non-comment lines
        let cmd_lines: Vec<&str> = request
            .body_str()
            .lines()
            .map(|l| l.trim())
            .filter(|l| {
                !l.is_empty() && !l.starts_with('#') && !l.starts_with("--") && !l.starts_with('>')
            })
            .collect();

        if cmd_lines.is_empty() {
            anyhow::bail!("Empty Redis command");
        }

        let cmd_line = cmd_lines.join(" ");
        let tokens = crate::executor::parse_shell_args(&cmd_line)?;
        if tokens.is_empty() {
            anyhow::bail!("Empty Redis command");
        }

        let cmd_name = tokens[0].to_uppercase();
        let args: Vec<&str> = tokens[1..].iter().map(|s| s.as_str()).collect();

        let mut cmd = redis::cmd(&cmd_name);
        for arg in &args {
            cmd.arg(*arg);
        }

        let result: redis::Value = cmd.query_async(&mut con).await?;

        // Convert to structured JSON for Lua-side rendering
        let structured = redis_value_to_json(&result, &cmd_name);
        let body = serde_json::to_string(&structured)?;

        let type_name = structured
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let status_text = match &result {
            redis::Value::Okay => "OK".to_string(),
            redis::Value::Nil => "(nil)".to_string(),
            redis::Value::Int(n) => format!("{}", n),
            redis::Value::Array(a) => format!("{} elements", a.len()),
            _ => type_name.to_string(),
        };

        let mut metadata = HashMap::new();
        metadata.insert("command".to_string(), cmd_line.to_string());
        metadata.insert("type".to_string(), type_name.to_string());

        Ok(Response {
            protocol: "redis".to_string(),
            status: 0,
            status_text,
            latency_ms: 0,
            url: connection.clone(),
            content_type: "text/plain".to_string(),
            headers: Vec::new(),
            body,
            cookies: Vec::new(),
            metadata,
        })
    }
}

/// Convert Redis Value to structured JSON for Lua-side rendering.
/// Uses command inference and heuristics to detect semantic types.
fn redis_value_to_json(val: &redis::Value, cmd_name: &str) -> serde_json::Value {
    use serde_json::{json, Value};

    match val {
        redis::Value::Nil => json!({
            "type": "nil",
            "value": null
        }),
        redis::Value::Int(n) => json!({
            "type": "integer",
            "value": n
        }),
        redis::Value::Okay => json!({
            "type": "string",
            "value": "OK"
        }),
        redis::Value::SimpleString(s) => json!({
            "type": "string",
            "value": s
        }),
        redis::Value::BulkString(b) => {
            match std::str::from_utf8(b) {
                Ok(s) => {
                    // Heuristic: try JSON parse
                    if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                        json!({
                            "type": "string",
                            "value": s,
                            "parsed": parsed
                        })
                    } else {
                        json!({
                            "type": "string",
                            "value": s
                        })
                    }
                }
                Err(_) => json!({
                    "type": "binary",
                    "bytes": b.len()
                }),
            }
        }
        redis::Value::Array(arr) => {
            if arr.is_empty() {
                return json!({
                    "type": "list",
                    "value": []
                });
            }

            // Command inference
            let inferred_type = match cmd_name.to_uppercase().as_str() {
                "HGETALL" | "HSCAN" => "hash",
                "LRANGE" | "LINDEX" | "LPOP" | "RPOP" => "list",
                "SMEMBERS" | "SINTER" | "SUNION" | "SDIFF" | "SRANDMEMBER" => "set",
                "ZRANGE" | "ZRANGEBYSCORE" | "ZRANGEBYLEX" | "ZPOPMIN" | "ZPOPMAX" => "zset",
                "XRANGE" | "XREVRANGE" | "XREAD" | "XREADGROUP" => "stream",
                _ => {
                    // Heuristic: check if array looks like key-value pairs (even length, alternating types)
                    if arr.len() % 2 == 0 && !arr.is_empty() {
                        let all_strings = arr.iter().all(|v| {
                            matches!(
                                v,
                                redis::Value::BulkString(_) | redis::Value::SimpleString(_)
                            )
                        });
                        if all_strings {
                            "hash"
                        } else {
                            "list"
                        }
                    } else {
                        "list"
                    }
                }
            };

            match inferred_type {
                "hash" => {
                    // Convert to object
                    let mut map = serde_json::Map::new();
                    for chunk in arr.chunks(2) {
                        if chunk.len() == 2 {
                            let key = match &chunk[0] {
                                redis::Value::BulkString(b) => {
                                    String::from_utf8_lossy(b).to_string()
                                }
                                redis::Value::SimpleString(s) => s.clone(),
                                _ => continue,
                            };
                            let val = redis_value_to_json(&chunk[1], "");
                            map.insert(key, val["value"].clone());
                        }
                    }
                    json!({
                        "type": "hash",
                        "value": Value::Object(map)
                    })
                }
                "zset" => {
                    // Convert to array of {score, member}
                    let mut items = Vec::new();
                    for chunk in arr.chunks(2) {
                        if chunk.len() == 2 {
                            let member = match &chunk[0] {
                                redis::Value::BulkString(b) => {
                                    String::from_utf8_lossy(b).to_string()
                                }
                                redis::Value::SimpleString(s) => s.clone(),
                                _ => continue,
                            };
                            let score = match &chunk[1] {
                                redis::Value::BulkString(b) => {
                                    String::from_utf8_lossy(b).parse::<f64>().unwrap_or(0.0)
                                }
                                redis::Value::Int(n) => *n as f64,
                                _ => 0.0,
                            };
                            items.push(json!({
                                "member": member,
                                "score": score
                            }));
                        }
                    }
                    json!({
                        "type": "zset",
                        "value": items
                    })
                }
                _ => {
                    // list or set
                    let items: Vec<Value> = arr
                        .iter()
                        .map(|v| redis_value_to_json(v, "")["value"].clone())
                        .collect();
                    json!({
                        "type": inferred_type,
                        "value": items
                    })
                }
            }
        }
        redis::Value::Map(m) => {
            // Redis 7+ native map type
            let mut map = serde_json::Map::new();
            for (k, v) in m {
                let key = match k {
                    redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
                    redis::Value::SimpleString(s) => s.clone(),
                    _ => continue,
                };
                let val = redis_value_to_json(v, "");
                map.insert(key, val["value"].clone());
            }
            json!({
                "type": "hash",
                "value": Value::Object(map)
            })
        }
        _ => json!({
            "type": "unknown",
            "value": format!("{:?}", val)
        }),
    }
}