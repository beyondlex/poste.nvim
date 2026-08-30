//! Redis command execution for the poste binary.
//!
//! Ported from the poste-data-modification branch (`execute_redis` /
//! `redis_value_to_json`, poste-exec/src/executor.rs), reshaped for the
//! `poste redis-exec` contract: Lua is the only parser (poste-redis.nvim
//! EXECUTION-PLAN.md, decision D1) — this module receives pre-tokenized
//! commands and reports one outcome per command; command errors never stop
//! the batch (greedy).

use anyhow::Result;
use serde_json::{json, Value};

/// Per-command outcomes for a batch.
pub struct CommandOutcome {
    /// Display form: tokens joined with single spaces.
    pub command: String,
    /// 1-based position in the batch.
    pub seq: usize,
    pub latency_ms: u128,
    /// Structured redis value (see `redis_value_to_json`).
    pub value: Value,
    /// Human-readable redis-cli style status line.
    pub status: String,
    /// `Some` when the command failed; the batch continues regardless.
    pub error: Option<String>,
}

/// Validate that a connection URL is a redis URL (Lua resolves
/// connections.toml — the binary never reads config files).
pub fn validate_connection_url(url: &str) -> Result<()> {
    if url.starts_with("redis://") || url.starts_with("rediss://") {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Not a redis connection URL (expected redis:// or rediss://): {}",
            url
        ))
    }
}

/// Execute pre-tokenized commands on one multiplexed connection.
/// A command error is recorded in its outcome; only transport-level
/// failures (connect, protocol) bail.
pub async fn execute_commands(
    connection_url: &str,
    commands: &[Vec<String>],
    max_items: usize,
    max_bytes: usize,
) -> Result<Vec<CommandOutcome>> {
    validate_connection_url(connection_url)?;
    let client = redis::Client::open(connection_url)?;
    let mut con = client.get_multiplexed_async_connection().await?;

    let mut outcomes = Vec::with_capacity(commands.len());
    for (i, tokens) in commands.iter().enumerate() {
        let started = std::time::Instant::now();
        let display = tokens.join(" ");
        if tokens.is_empty() {
            outcomes.push(CommandOutcome {
                command: display,
                seq: i + 1,
                latency_ms: 0,
                value: json!({"type": "nil", "value": null}),
                status: String::new(),
                error: Some("Empty command".into()),
            });
            continue;
        }

        let cmd_name = tokens[0].to_uppercase();
        let mut cmd = redis::cmd(&cmd_name);
        for arg in &tokens[1..] {
            cmd.arg(arg.as_str());
        }

        match cmd.query_async::<redis::Value>(&mut con).await {
            Ok(val) => {
                let value = redis_value_to_json(&val, &cmd_name, max_items, max_bytes);
                let status = status_text(&val);
                outcomes.push(CommandOutcome {
                    command: display,
                    seq: i + 1,
                    latency_ms: started.elapsed().as_millis(),
                    value,
                    status,
                    error: None,
                });
            }
            Err(e) => {
                outcomes.push(CommandOutcome {
                    command: display,
                    seq: i + 1,
                    latency_ms: started.elapsed().as_millis(),
                    value: json!({"type": "nil", "value": null}),
                    status: "error".into(),
                    error: Some(format!("{}", e)),
                });
            }
        }
    }
    Ok(outcomes)
}

fn redis_type_name(val: &redis::Value) -> &'static str {
    match val {
        redis::Value::Nil => "nil",
        redis::Value::Int(_) => "integer",
        redis::Value::Okay => "string",
        redis::Value::SimpleString(_) | redis::Value::BulkString(_) => "string",
        redis::Value::Array(_) => "list",
        redis::Value::Map(_) => "hash",
        _ => "unknown",
    }
}

/// Human-readable redis-cli style status line.
fn status_text(val: &redis::Value) -> String {
    match val {
        redis::Value::Okay => "OK".into(),
        redis::Value::Nil => "(nil)".into(),
        redis::Value::Int(n) => n.to_string(),
        redis::Value::Array(a) => format!("{} elements", a.len()),
        redis::Value::Map(m) => format!("{} entries", m.len()),
        // SimpleString/BulkString: show the payload itself (PING → PONG,
        // GET → the value), matching redis-cli's reply rendering
        redis::Value::SimpleString(s) => s.clone(),
        redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
        _ => redis_type_name(val).to_string(),
    }
}

fn key_string(val: &redis::Value) -> Option<String> {
    match val {
        redis::Value::BulkString(b) => Some(String::from_utf8_lossy(b).to_string()),
        redis::Value::SimpleString(s) => Some(s.clone()),
        _ => None,
    }
}

fn hex_preview(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert a redis reply into the structured JSON shape consumed by the
/// Lua side:
/// - strings carry `parsed` when the payload itself is valid JSON, plus
///   `len`/`truncated` beyond `max_bytes`
/// - non-UTF-8 payloads become `{"type":"binary","encoding":"hex",...}`
/// - arrays infer list/set/hash/zset/stream from the command (HGETALL &
///   friends → `entries` pairs); `len`/`truncated` beyond `max_items`
/// - Redis 7 native maps use the same `entries` shape as inferred hashes
pub fn redis_value_to_json(
    val: &redis::Value,
    cmd_name: &str,
    max_items: usize,
    max_bytes: usize,
) -> Value {
    match val {
        redis::Value::Nil => json!({"type": "nil", "value": null}),
        redis::Value::Int(n) => json!({"type": "integer", "value": n}),
        redis::Value::Okay => json!({"type": "string", "value": "OK"}),
        redis::Value::SimpleString(s) => json!({"type": "string", "value": s}),
        redis::Value::BulkString(b) => {
            let total = b.len();
            match std::str::from_utf8(b) {
                Ok(s) => {
                    let truncated = total > max_bytes;
                    let shown = if truncated { &s[..max_bytes] } else { s };
                    let mut obj = json!({
                        "type": "string",
                        "value": shown,
                        "len": total,
                    });
                    if truncated {
                        obj["truncated"] = json!(true);
                    }
                    // Heuristic: surface parsed JSON alongside the raw string
                    if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                        obj["parsed"] = parsed;
                    }
                    obj
                }
                Err(_) => json!({
                    "type": "binary",
                    "encoding": "hex",
                    "bytes": total,
                    "preview": hex_preview(b, 64),
                }),
            }
        }
        redis::Value::Array(arr) => {
            let len = arr.len();
            if arr.is_empty() {
                return json!({"type": "list", "value": [], "len": 0});
            }
            let truncated = len > max_items;
            let shown_len = len.min(max_items);

            let inferred_type = match cmd_name.to_uppercase().as_str() {
                "HGETALL" | "HSCAN" => "hash",
                "LRANGE" | "LINDEX" | "LPOP" | "RPOP" => "list",
                "SMEMBERS" | "SINTER" | "SUNION" | "SDIFF" | "SRANDMEMBER" => "set",
                "ZRANGE" | "ZRANGEBYSCORE" | "ZRANGEBYLEX" | "ZPOPMIN" | "ZPOPMAX" => "zset",
                "XRANGE" | "XREVRANGE" | "XREAD" | "XREADGROUP" => "stream",
                // KEYS always returns keys — never let the even-count
                // heuristic misread them as field-value pairs
                "KEYS" => "list",
                _ => {
                    // Heuristic: flat string arrays of even length look like
                    // field-value pairs
                    if len % 2 == 0
                        && arr
                            .iter()
                            .all(|v| matches!(v, redis::Value::BulkString(_) | redis::Value::SimpleString(_)))
                    {
                        "hash"
                    } else {
                        "list"
                    }
                }
            };

            match inferred_type {
                "hash" => {
                    let mut entries = Vec::with_capacity(shown_len / 2 + 1);
                    for chunk in arr[..shown_len].chunks(2) {
                        if chunk.len() == 2 {
                            if let Some(key) = key_string(&chunk[0]) {
                                let value = redis_value_to_json(&chunk[1], "", max_items, max_bytes);
                                entries.push(json!([key, value["value"].clone()]));
                            }
                        }
                    }
                    json!({"type": "hash", "entries": entries, "len": len / 2, "truncated": truncated})
                }
                "zset" => {
                    let mut items = Vec::with_capacity(shown_len / 2 + 1);
                    for chunk in arr[..shown_len].chunks(2) {
                        if chunk.len() == 2 {
                            let member = key_string(&chunk[0]).unwrap_or_default();
                            let score = match &chunk[1] {
                                redis::Value::BulkString(b) => {
                                    String::from_utf8_lossy(b).parse::<f64>().unwrap_or(0.0)
                                }
                                redis::Value::Int(n) => *n as f64,
                                _ => 0.0,
                            };
                            items.push(json!({"member": member, "score": score}));
                        }
                    }
                    json!({"type": "zset", "value": items, "len": len / 2, "truncated": truncated})
                }
                "stream" => {
                    // XRANGE returns [[id, [field, value, ...]], ...]
                    let mut items = Vec::with_capacity(shown_len);
                    for entry in arr.iter().take(shown_len) {
                        if let redis::Value::Array(pair) = entry {
                            if pair.len() == 2 {
                                let id = key_string(&pair[0]).unwrap_or_default();
                                let fields = match &pair[1] {
                                    redis::Value::Array(f) => {
                                        let mut out = Vec::new();
                                        for chunk in f.chunks(2) {
                                            if chunk.len() == 2 {
                                                out.push(json!([
                                                    key_string(&chunk[0]).unwrap_or_default(),
                                                    key_string(&chunk[1]).unwrap_or_default(),
                                                ]));
                                            }
                                        }
                                        out
                                    }
                                    _ => Vec::new(),
                                };
                                items.push(json!({"id": id, "fields": fields}));
                            }
                        }
                    }
                    json!({"type": "stream", "value": items, "len": len, "truncated": truncated})
                }
                other => {
                    let items: Vec<Value> = arr
                        .iter()
                        .take(shown_len)
                        .map(|v| redis_value_to_json(v, "", max_items, max_bytes)["value"].clone())
                        .collect();
                    json!({"type": other, "value": items, "len": len, "truncated": truncated})
                }
            }
        }
        redis::Value::Map(m) => {
            let len = m.len();
            let truncated = len > max_items;
            let mut entries = Vec::with_capacity(len.min(max_items));
            for (k, v) in m.iter().take(max_items) {
                if let Some(key) = key_string(k) {
                    let value = redis_value_to_json(v, "", max_items, max_bytes);
                    entries.push(json!([key, value["value"].clone()]));
                }
            }
            json!({"type": "hash", "entries": entries, "len": len, "truncated": truncated})
        }
        _ => json!({"type": "unknown", "value": format!("{:?}", val)}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_connection_urls() {
        assert!(validate_connection_url("redis://127.0.0.1:6379/0").is_ok());
        assert!(validate_connection_url("rediss://host:6379").is_ok());
        assert!(validate_connection_url("redis://:pass@host:6379/2").is_ok());
        assert!(validate_connection_url("postgres://host/db").is_err());
        assert!(validate_connection_url("127.0.0.1:6379").is_err());
    }

    #[test]
    fn scalar_values() {
        assert_eq!(
            redis_value_to_json(&redis::Value::Nil, "GET", 100, 1024),
            json!({"type": "nil", "value": null})
        );
        assert_eq!(
            redis_value_to_json(&redis::Value::Int(42), "INCR", 100, 1024),
            json!({"type": "integer", "value": 42})
        );
        assert_eq!(
            redis_value_to_json(&redis::Value::Okay, "SET", 100, 1024),
            json!({"type": "string", "value": "OK"})
        );
        let out = redis_value_to_json(&redis::Value::SimpleString("PONG".into()), "PING", 100, 1024);
        assert_eq!(out["value"], json!("PONG"));
    }

    #[test]
    fn bulk_string_with_json_heuristic() {
        let raw = br#"{"a": 1}"#;
        let out = redis_value_to_json(&redis::Value::BulkString(raw.to_vec()), "GET", 100, 1024);
        assert_eq!(out["type"], "string");
        assert_eq!(out["parsed"], json!({"a": 1}));
        assert_eq!(out["len"], 8);
        assert!(out.get("truncated").is_none());
    }

    #[test]
    fn bulk_string_truncation() {
        let raw = "x".repeat(200).into_bytes();
        let out = redis_value_to_json(&redis::Value::BulkString(raw), "GET", 100, 64);
        assert_eq!(out["truncated"], json!(true));
        assert_eq!(out["len"], 200);
        assert_eq!(out["value"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn non_utf8_becomes_binary() {
        let out = redis_value_to_json(
            &redis::Value::BulkString(vec![0x48, 0x65, 0xff, 0x00]),
            "GET",
            100,
            1024,
        );
        assert_eq!(out["type"], "binary");
        assert_eq!(out["encoding"], "hex");
        assert_eq!(out["bytes"], 4);
        assert_eq!(out["preview"], "48 65 ff 00");
    }

    #[test]
    fn array_of_scalars_is_list() {
        let arr = vec![
            redis::Value::BulkString(b"a".to_vec()),
            redis::Value::BulkString(b"b".to_vec()),
            redis::Value::Int(3),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "KEYS", 100, 1024);
        assert_eq!(out["type"], "list");
        assert_eq!(out["len"], 3);
        assert_eq!(out["value"], json!(["a", "b", 3]));
    }

    #[test]
    fn empty_array_is_empty_list() {
        let out = redis_value_to_json(&redis::Value::Array(vec![]), "KEYS", 100, 1024);
        assert_eq!(out, json!({"type": "list", "value": [], "len": 0}));
    }

    #[test]
    fn hgetall_becomes_hash_entries() {
        let arr = vec![
            redis::Value::BulkString(b"name".to_vec()),
            redis::Value::BulkString(b"Alice".to_vec()),
            redis::Value::BulkString(b"age".to_vec()),
            redis::Value::BulkString(b"30".to_vec()),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "HGETALL", 100, 1024);
        assert_eq!(out["type"], "hash");
        assert_eq!(out["len"], 2);
        assert_eq!(
            out["entries"],
            json!([["name", "Alice"], ["age", "30"]])
        );
    }

    #[test]
    fn zrange_withscores_becomes_zset() {
        let arr = vec![
            redis::Value::BulkString(b"alice".to_vec()),
            redis::Value::BulkString(b"99".to_vec()),
            redis::Value::BulkString(b"bob".to_vec()),
            redis::Value::Int(87),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "ZRANGE", 100, 1024);
        assert_eq!(out["type"], "zset");
        assert_eq!(out["len"], 2);
        assert_eq!(
            out["value"],
            json!([{"member": "alice", "score": 99.0}, {"member": "bob", "score": 87.0}])
        );
    }

    #[test]
    fn xrange_becomes_stream_entries() {
        let entry = redis::Value::Array(vec![
            redis::Value::BulkString(b"1690000000000-0".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"to".to_vec()),
                redis::Value::BulkString(b"a@b.c".to_vec()),
            ]),
        ]);
        let out = redis_value_to_json(&redis::Value::Array(vec![entry]), "XRANGE", 100, 1024);
        assert_eq!(out["type"], "stream");
        assert_eq!(out["value"][0]["id"], "1690000000000-0");
        assert_eq!(out["value"][0]["fields"], json!([["to", "a@b.c"]]));
    }

    #[test]
    fn array_truncation_marks_and_slices() {
        let arr: Vec<redis::Value> = (0..10)
            .map(|i| redis::Value::BulkString(i.to_string().into_bytes()))
            .collect();
        let out = redis_value_to_json(&redis::Value::Array(arr), "KEYS", 5, 1024);
        assert_eq!(out["len"], 10);
        assert_eq!(out["truncated"], json!(true));
        assert_eq!(out["value"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn even_flat_string_array_infers_hash() {
        let arr = vec![
            redis::Value::BulkString(b"a".to_vec()),
            redis::Value::BulkString(b"1".to_vec()),
            redis::Value::BulkString(b"b".to_vec()),
            redis::Value::BulkString(b"2".to_vec()),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "CONFIG", 100, 1024);
        assert_eq!(out["type"], "hash");
        assert_eq!(out["entries"], json!([["a", "1"], ["b", "2"]]));
    }

    #[test]
    fn native_map_becomes_hash_entries() {
        let m = vec![
            (
                redis::Value::BulkString(b"k".to_vec()),
                redis::Value::BulkString(b"v".to_vec()),
            ),
        ];
        let out = redis_value_to_json(&redis::Value::Map(m.clone()), "XREAD", 100, 1024);
        assert_eq!(out["type"], "hash");
        assert_eq!(out["entries"], json!([["k", "v"]]));
    }

    #[tokio::test]
    async fn integration_against_live_redis() {
        // Only runs when REDIS_TEST_URL is provided (CI service / local docker)
        let Ok(url) = std::env::var("REDIS_TEST_URL") else {
            return;
        };
        let outcomes = execute_commands(
            &url,
            &[
                vec!["PING".into()],
                vec!["SET".into(), "poste:test".into(), "1".into(), "EX".into(), "60".into()],
                vec!["GET".into(), "poste:test".into()],
                vec!["HSET".into(), "poste:h".into(), "a".into(), "1".into(), "b".into(), "2".into()],
                vec!["HGETALL".into(), "poste:h".into()],
                vec!["NOSUCHCMD".into(), "x".into()],
            ],
            100,
            1024,
        )
        .await
        .expect("batch should not bail on command errors");

        assert_eq!(outcomes.len(), 6);
        assert!(outcomes[0].error.is_none());
        assert_eq!(outcomes[0].status, "PONG");
        assert!(outcomes[3].error.is_none());
        assert_eq!(outcomes[4].value["type"], "hash");
        assert!(outcomes[5].error.is_some()); // NOSUCHCMD recorded, batch continued
    }
}
