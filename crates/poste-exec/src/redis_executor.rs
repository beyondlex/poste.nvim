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
            poste_core::mask_url_password(url)
        ))
    }
}

/// Execute one pre-tokenized command on an already-open connection, producing
/// its outcome. `seq` is caller-supplied (1-based within the batch/session).
/// Session mode (poste redis-session) reuses one connection across calls so
/// SELECT and other per-connection state persist between requests.
pub async fn execute_command_on(
    con: &mut redis::aio::MultiplexedConnection,
    tokens: &[String],
    seq: usize,
    max_items: usize,
    max_bytes: usize,
) -> CommandOutcome {
    let started = std::time::Instant::now();
    let display = tokens.join(" ");
    if tokens.is_empty() {
        return CommandOutcome {
            command: display,
            seq,
            latency_ms: 0,
            value: json!({"type": "nil", "value": null}),
            status: String::new(),
            error: Some("Empty command".into()),
        };
    }
    let cmd_name = tokens[0].to_uppercase();
    let mut cmd = redis::cmd(&cmd_name);
    for arg in &tokens[1..] {
        cmd.arg(arg.as_str());
    }
    match cmd.query_async::<redis::Value>(con).await {
        Ok(val) => {
            let value = redis_value_to_json(&val, &cmd_name, max_items, max_bytes);
            let status = status_text(&val, max_bytes);
            CommandOutcome {
                command: display,
                seq,
                latency_ms: started.elapsed().as_millis(),
                value,
                status,
                error: None,
            }
        }
        Err(e) => CommandOutcome {
            command: display,
            seq,
            latency_ms: started.elapsed().as_millis(),
            value: json!({"type": "nil", "value": null}),
            status: "error".into(),
            error: Some(format!("{}", e)),
        },
    }
}

/// Execute pre-tokenized commands on one multiplexed connection, invoking
/// `on_result` as each command completes. A command error is recorded in
/// its outcome; only transport-level failures (connect, protocol) bail.
pub async fn execute_commands_with<F>(
    connection_url: &str,
    commands: &[Vec<String>],
    max_items: usize,
    max_bytes: usize,
    mut on_result: F,
) -> Result<()>
where
    F: FnMut(&CommandOutcome),
{
    validate_connection_url(connection_url)?;
    let client = redis::Client::open(connection_url)?;
    let mut con = client.get_multiplexed_async_connection().await?;

    for (i, tokens) in commands.iter().enumerate() {
        let outcome = execute_command_on(&mut con, tokens, i + 1, max_items, max_bytes).await;
        on_result(&outcome);
    }
    Ok(())
}

/// Execute pre-tokenized commands, collecting all outcomes.
pub async fn execute_commands(
    connection_url: &str,
    commands: &[Vec<String>],
    max_items: usize,
    max_bytes: usize,
) -> Result<Vec<CommandOutcome>> {
    let mut outcomes = Vec::with_capacity(commands.len());
    execute_commands_with(connection_url, commands, max_items, max_bytes, |o| {
        outcomes.push(CommandOutcome {
            command: o.command.clone(),
            seq: o.seq,
            latency_ms: o.latency_ms,
            value: o.value.clone(),
            status: o.status.clone(),
            error: o.error.clone(),
        })
    })
    .await?;
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
        // RESP3-typed replies (`?protocol=resp3` in the connection URL)
        redis::Value::Set(_) => "set",
        redis::Value::Double(_) => "number",
        redis::Value::Boolean(_) => "boolean",
        redis::Value::BigNumber(_) => "integer",
        redis::Value::VerbatimString { .. } => "string",
        _ => "unknown",
    }
}

/// Cut `s` to at most `max_bytes`, never mid-character, and report whether
/// anything was dropped. `&s[..max_bytes]` panics when the cut lands inside a
/// multibyte sequence, and the payloads here are user data (a CJK or emoji
/// value whose `max_bytes`-th byte continues a sequence would otherwise crash
/// the whole redis-exec/session process).
fn cap_bytes(s: &str, max_bytes: usize) -> (&str, bool) {
    if s.len() <= max_bytes {
        return (s, false);
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

/// Human-readable redis-cli style status line.
///
/// Bounded by `max_bytes` like the value itself: the string arms quote the
/// payload, and this line ships on the same stdout the Lua side reads
/// line-by-line (and rides into its in-memory history) — an uncapped `GET` of
/// a 50 MB value would put all 50 MB there even though `value` was cut.
fn status_text(val: &redis::Value, max_bytes: usize) -> String {
    match val {
        redis::Value::Okay => "OK".into(),
        redis::Value::Nil => "(nil)".into(),
        redis::Value::Int(n) => n.to_string(),
        redis::Value::Array(a) => format!("{} elements", a.len()),
        redis::Value::Map(m) => format!("{} entries", m.len()),
        redis::Value::Set(s) => format!("{} elements", s.len()),
        redis::Value::Double(f) => f.to_string(),
        redis::Value::Boolean(b) => b.to_string(),
        redis::Value::BigNumber(n) => n.to_string(),
        // SimpleString/BulkString: show the payload itself (PING → PONG,
        // GET → the value), matching redis-cli's reply rendering
        redis::Value::SimpleString(s) => cap_bytes(s, max_bytes).0.to_string(),
        redis::Value::BulkString(b) => {
            let lossy = String::from_utf8_lossy(b);
            cap_bytes(&lossy, max_bytes).0.to_string()
        }
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

/// Pair-key display (`hash` field names, `zset`/stream members). Unlike
/// `key_string` it always produces text: a key that is not a plain string
/// (RESP3 maps can carry integer or nested keys) used to fail the extraction,
/// and the arm dropped the whole pair — `len` still counted it, so the panel
/// showed a collection with rows missing and no trace of why.
fn key_display(val: &redis::Value) -> String {
    match val {
        // A non-UTF-8 field name is still a field: `key_string` renders it
        // lossily (one replacement char per byte), so show hex instead.
        redis::Value::BulkString(b) if std::str::from_utf8(b).is_err() => {
            format!("hex:{}", hex_preview(b, 16))
        }
        // A typed key (RESP3 maps can carry integer keys) still names a row.
        redis::Value::Int(n) => n.to_string(),
        redis::Value::Double(f) => f.to_string(),
        redis::Value::Boolean(b) => b.to_string(),
        redis::Value::BigNumber(n) => n.to_string(),
        other => key_string(other).unwrap_or_else(|| format!("(redis {})", redis_type_name(other))),
    }
}

/// A child reply in a nested position (hash value column, list/set element):
/// the cell holds the child's scalar, or the child's whole shape when it has
/// no `value` key at all — a nested `hash` has `entries`, and flattening it to
/// JSON null hid a value that is very much there.
fn child_cell(val: &redis::Value, max_items: usize, max_bytes: usize) -> Value {
    let child = redis_value_to_json(val, "", max_items, max_bytes);
    match child.get("value") {
        Some(v) => v.clone(),
        None => child,
    }
}

/// Flat collection shape (`{"type": t, "value": [...], "len", "truncated"}`)
/// for item lists with no pair structure: list/set replies and RESP3 sets.
fn items_shape(arr: &[redis::Value], type_name: &str, max_items: usize, max_bytes: usize) -> Value {
    let len = arr.len();
    let truncated = len > max_items;
    let items: Vec<Value> = arr
        .iter()
        .take(max_items)
        .map(|v| child_cell(v, max_items, max_bytes))
        .collect();
    json!({"type": type_name, "value": items, "len": len, "truncated": truncated})
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
/// - strings carry `parsed` when the payload itself is valid JSON and fits
///   `max_bytes`, plus `len`/`truncated` beyond it
/// - non-UTF-8 payloads become `{"type":"binary","encoding":"hex",...}`, and
///   that shape carries a one-line `value` too, because nested cells take the
///   child's `value` (see `child_cell`)
/// - arrays infer list/set/hash/zset/stream from the command (HGETALL &
///   friends → `entries` pairs); `len`/`truncated` beyond `max_items`
/// - Redis 7 native maps use the same `entries` shape as inferred hashes
/// - RESP3 typed replies (`?protocol=resp3` in the URL) map onto the shapes
///   above: sets to `set`, doubles/booleans/big numbers to scalars, verbatim
///   strings to `string`, attributes to their payload
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
            // UTF-8 validity is the whole binary/text split: a NUL-containing
            // payload stays text (the Lua renderer escapes control characters
            // for the grid), while one stray 0xff byte makes the value binary.
            match std::str::from_utf8(b) {
                Ok(s) => {
                    let (shown, truncated) = cap_bytes(s, max_bytes);
                    let mut obj = json!({
                        "type": "string",
                        "value": shown,
                        "len": total,
                    });
                    if truncated {
                        obj["truncated"] = json!(true);
                    }
                    // Heuristic: surface parsed JSON alongside the raw string —
                    // only for a value that fit the cap. Re-emitting `parsed`
                    // for a truncated value writes the whole document back out
                    // anyway, which is exactly what `max_bytes` exists to bound
                    // (the Lua side reads line-delimited JSON and a 50 MB
                    // document starves the job's stdout).
                    if !truncated {
                        if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                            obj["parsed"] = parsed;
                        }
                    }
                    obj
                }
                Err(_) => {
                    // `value` is the same one-line summary the Lua text
                    // renderer builds: a hash field or list element holding a
                    // protobuf/msgpack blob is a nested cell, and those take
                    // the child's `value` — without it the row showed "(nil)",
                    // i.e. an apparently empty field that really has bytes.
                    let preview = hex_preview(b, 64);
                    json!({
                        "type": "binary",
                        "encoding": "hex",
                        "bytes": total,
                        "preview": preview,
                        "value": format!("(binary) {} bytes hex: {}", total, preview),
                    })
                }
            }
        }
        redis::Value::Array(arr) => {
            let len = arr.len();
            if arr.is_empty() {
                return json!({"type": "list", "value": [], "len": 0});
            }
            let truncated = len > max_items;
            let shown_len = len.min(max_items);

            // SCAN-family replies are [cursor, items[]] — surface the cursor
            // and recurse on the items so the Lua side can page through
            // HSCAN/SSCAN/ZSCAN (P1-3 large-collection navigation).
            let upper_cmd = cmd_name.to_uppercase();
            if upper_cmd == "SCAN"
                || upper_cmd == "HSCAN"
                || upper_cmd == "SSCAN"
                || upper_cmd == "ZSCAN"
            {
                let cursor = key_string(&arr[0]).unwrap_or_else(|| "0".into());
                let items = match arr.get(1) {
                    Some(redis::Value::Array(inner)) => inner.clone(),
                    _ => Vec::new(),
                };
                let mut out = redis_value_to_json(
                    &redis::Value::Array(items),
                    // HSCAN's items are flat field/value pairs → hash shape;
                    // ZSCAN's items are member/score pairs → zset shape;
                    // SSCAN's items are bare members → set shape;
                    // SCAN items are bare keys → list shape.
                    if upper_cmd == "HSCAN" {
                        "HGETALL"
                    } else if upper_cmd == "ZSCAN" {
                        "ZRANGE"
                    } else if upper_cmd == "SSCAN" {
                        "SMEMBERS"
                    } else {
                        "KEYS"
                    },
                    max_items,
                    max_bytes,
                );
                if let Some(obj) = out.as_object_mut() {
                    obj.insert("cursor".into(), json!(cursor));
                }
                return out;
            }

            let inferred_type = match upper_cmd.as_str() {
                "HGETALL" => "hash",
                // MGET/HMGET reply with one value per requested key/field — a
                // flat string array the even-count heuristic below would
                // misread as field-value pairs (MGET a b rendered as the hash
                // {a: <value of a>})
                "LRANGE" | "LINDEX" | "LPOP" | "RPOP" | "MGET" | "HMGET" => "list",
                "SMEMBERS" | "SINTER" | "SUNION" | "SDIFF" | "SRANDMEMBER" => "set",
                "ZRANGE" | "ZREVRANGE" | "ZRANGEBYSCORE" | "ZRANGEBYLEX" | "ZPOPMIN"
                | "ZPOPMAX" => "zset",
                "XRANGE" | "XREVRANGE" | "XREAD" | "XREADGROUP" => "stream",
                // KEYS always returns keys — never let the even-count
                // heuristic misread them as field-value pairs
                "KEYS" => "list",
                _ => {
                    // Heuristic: flat string arrays of even length look like
                    // field-value pairs
                    if len % 2 == 0
                        && arr.iter().all(|v| {
                            matches!(
                                v,
                                redis::Value::BulkString(_) | redis::Value::SimpleString(_)
                            )
                        })
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
                            let key = key_display(&chunk[0]);
                            let value = child_cell(&chunk[1], max_items, max_bytes);
                            entries.push(json!([key, value]));
                        }
                    }
                    json!({"type": "hash", "entries": entries, "len": len / 2, "truncated": truncated})
                }
                "zset" => {
                    let mut items = Vec::with_capacity(shown_len / 2 + 1);
                    for chunk in arr[..shown_len].chunks(2) {
                        if chunk.len() == 2 {
                            let member = key_display(&chunk[0]);
                            let score = match &chunk[1] {
                                redis::Value::BulkString(b) => {
                                    String::from_utf8_lossy(b).parse::<f64>().unwrap_or(0.0)
                                }
                                // RESP3 (`?protocol=resp3`) scores arrive typed
                                redis::Value::Double(f) => *f,
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
                                let id = key_display(&pair[0]);
                                let fields = match &pair[1] {
                                    redis::Value::Array(f) => {
                                        let mut out = Vec::new();
                                        for chunk in f.chunks(2) {
                                            if chunk.len() == 2 {
                                                out.push(json!([
                                                    key_display(&chunk[0]),
                                                    key_display(&chunk[1]),
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
                other => items_shape(arr, other, max_items, max_bytes),
            }
        }
        redis::Value::Map(m) => {
            let len = m.len();
            let truncated = len > max_items;
            let mut entries = Vec::with_capacity(len.min(max_items));
            for (k, v) in m.iter().take(max_items) {
                let key = key_display(k);
                let value = child_cell(v, max_items, max_bytes);
                entries.push(json!([key, value]));
            }
            json!({"type": "hash", "entries": entries, "len": len, "truncated": truncated})
        }
        redis::Value::Set(items) => {
            // SMEMBERS/ZRANGE under RESP3 (`?protocol=resp3` in the URL) arrive
            // typed as a set. Left to the array heuristic they would read as a
            // list — or, when the member count is even, as a hash of "pairs".
            items_shape(items, "set", max_items, max_bytes)
        }
        // RESP3 scalars: without these arms every one of them fell into the
        // debug-format `unknown` shape, so the panel printed `Double(1.5)`
        // where the user asked for a number.
        redis::Value::Double(f) => json!({"type": "number", "value": f}),
        redis::Value::Boolean(b) => json!({"type": "boolean", "value": b}),
        // Big numbers keep their digits as text: as an f64 they would silently
        // round (same rule the SQL side applies to wide integers).
        redis::Value::BigNumber(n) => json!({"type": "integer", "value": n.to_string()}),
        // INFO and friends reply verbatim under RESP3: `format` is metadata,
        // the text is the value.
        redis::Value::VerbatimString { text, .. } => json!({
            "type": "string",
            "value": text,
            "len": text.len(),
        }),
        // Attributes are side metadata; the payload is what was asked for.
        redis::Value::Attribute { data, .. } => {
            redis_value_to_json(data, cmd_name, max_items, max_bytes)
        }
        // `ServerError` (an error typed *inside* a RESP3 collection) and `Push`
        // get no shape of their own: the debug-format arm below keeps their
        // text on screen, which is all a reply like that asks for.
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
    fn rejection_message_hides_the_password() {
        // The realistic leak: a SQL entry pasted into a redis slot. The
        // message is shown in the panel's error block, so the credential next
        // to the host must not come along with it.
        let err = validate_connection_url("postgres://alice:s3cret@db.example.com:5432/myapp")
            .unwrap_err()
            .to_string();
        assert!(!err.contains("s3cret"), "password leaked into {err}");
        assert!(
            err.contains("postgres://alice:****@db.example.com:5432/myapp"),
            "{err}"
        );
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
        let out = redis_value_to_json(
            &redis::Value::SimpleString("PONG".into()),
            "PING",
            100,
            1024,
        );
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
    fn truncated_json_loses_the_parsed_field() {
        // The cap is what keeps a huge value off stdout; re-serialising the
        // whole document through `parsed` silently removed it again.
        let raw = format!(r#"{{"a":"{}"}}"#, "x".repeat(400)).into_bytes();
        let out = redis_value_to_json(&redis::Value::BulkString(raw), "GET", 100, 64);
        assert_eq!(out["truncated"], json!(true));
        assert_eq!(out["len"], 408);
        assert!(
            out.get("parsed").is_none(),
            "truncated reply must not carry the full document: {out}"
        );
        assert_eq!(out["value"].as_str().unwrap().len(), 64);
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
    fn status_line_is_capped_like_the_value() {
        // `status` shares stdout with the capped `value` and is stored in the
        // Lua history; quoting the reply in full meant a single GET could emit
        // its whole payload no matter what --max-bytes said.
        let raw = "y".repeat(5000).into_bytes();
        let val = redis::Value::BulkString(raw);
        assert_eq!(status_text(&val, 64).len(), 64);
        assert_eq!(
            redis_value_to_json(&val, "GET", 100, 64)["value"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        // short replies still quote the payload (redis-cli style)
        assert_eq!(
            status_text(&redis::Value::BulkString(b"PONG".to_vec()), usize::MAX / 2),
            "PONG"
        );
        // and the cut stays on a character boundary: 30 CJK chars = 90 bytes,
        // a cap of 64 lands inside the 22nd char and floors to 63 bytes
        let cjk = redis::Value::BulkString("漢".repeat(30).into_bytes());
        assert_eq!(status_text(&cjk, 64).len(), 63);
    }

    #[test]
    fn bulk_string_truncation_respects_char_boundaries() {
        // regression: the cut used to be `&s[..max_bytes]`, which panics when
        // max_bytes lands inside a multibyte character — a CJK value whose
        // 64th byte continues a sequence would crash the whole
        // redis-exec/session process. 22 CJK chars = 66 bytes; the cut at 64
        // lands on the 2nd byte of the 22nd char → floors to 63 (21 chars).
        let raw = "漢".repeat(22).into_bytes(); // 66 bytes
        let out = redis_value_to_json(&redis::Value::BulkString(raw.clone()), "GET", 100, 64);
        assert_eq!(out["truncated"], json!(true));
        assert_eq!(out["len"], 66);
        let shown = out["value"].as_str().unwrap();
        assert_eq!(shown.len(), 63);
        assert!(shown.chars().count() == 21);
        // pure multibyte value, cut one byte into a char: floors to a boundary
        let out2 = redis_value_to_json(&redis::Value::BulkString("漢漢漢".into()), "GET", 100, 7);
        assert_eq!(out2["value"].as_str().unwrap().len(), 6);
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
        // Nested cells read `value`, so the shape has to carry one
        assert_eq!(out["value"], "(binary) 4 bytes hex: 48 65 ff 00");
    }

    #[test]
    fn binary_field_value_is_visible_in_a_hash_row() {
        // HGETALL of a msgpack/protobuf field: flattening took the child's
        // `value`, which the binary shape did not have, so the panel printed
        // "(nil)" for a field holding 4 bytes.
        let arr = vec![
            redis::Value::BulkString(b"blob".to_vec()),
            redis::Value::BulkString(vec![0x48, 0x65, 0xff, 0x00]),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "HGETALL", 100, 1024);
        assert_eq!(out["entries"][0][0], "blob");
        assert_eq!(out["entries"][0][1], "(binary) 4 bytes hex: 48 65 ff 00");
    }

    #[test]
    fn binary_element_is_visible_in_a_list() {
        // 0xff is what makes this binary: UTF-8 validity is the test, so a
        // NUL-only payload would stay a (control-char) string on purpose
        let arr = vec![
            redis::Value::BulkString(b"ok".to_vec()),
            redis::Value::BulkString(vec![0xff, 0x00]),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "LRANGE", 100, 1024);
        assert_eq!(out["type"], "list");
        assert_eq!(out["value"], json!(["ok", "(binary) 2 bytes hex: ff 00"]));
    }

    #[test]
    fn nested_collection_child_keeps_its_shape() {
        // A RESP3 map whose value is itself a map: `entries` cells take the
        // child's `value`, which a list has — but a nested *hash* has only
        // `entries`, and flattening it produced a null row.
        let inner = redis::Value::Map(vec![(
            redis::Value::BulkString(b"f".to_vec()),
            redis::Value::BulkString(b"v".to_vec()),
        )]);
        let outer = redis::Value::Map(vec![(redis::Value::BulkString(b"nested".to_vec()), inner)]);
        let out = redis_value_to_json(&outer, "XREAD", 100, 1024);
        assert_eq!(out["entries"][0][0], "nested");
        assert_eq!(out["entries"][0][1]["type"], "hash");
        assert_eq!(out["entries"][0][1]["entries"], json!([["f", "v"]]));
    }

    #[test]
    fn non_string_map_key_still_names_its_row() {
        let outer = redis::Value::Map(vec![(
            redis::Value::Int(5),
            redis::Value::BulkString(b"v".to_vec()),
        )]);
        let out = redis_value_to_json(&outer, "COMMAND", 100, 1024);
        assert_eq!(out["entries"], json!([["5", "v"]]));
        assert_eq!(out["len"], 1);
    }

    #[test]
    fn json_heuristic_respects_the_byte_cap() {
        // `parsed` used to parse the *untruncated* string, so a 200 MB JSON
        // blob arrived capped at 64 KB plus its full parsed tree
        let raw = format!(r#"{{"a": "{}"}}"#, "x".repeat(200)).into_bytes();
        let out = redis_value_to_json(&redis::Value::BulkString(raw), "GET", 100, 64);
        assert_eq!(out["truncated"], json!(true));
        assert!(
            out.get("parsed").is_none(),
            "parsed must not bypass max_bytes"
        );
        // the same document within the cap still surfaces its parsed form
        let small = br#"{"a": 1}"#.to_vec();
        let out = redis_value_to_json(&redis::Value::BulkString(small), "GET", 100, 64);
        assert_eq!(out["parsed"], json!({"a": 1}));
    }

    #[test]
    fn resp3_set_stays_a_set() {
        // Two members is an even count of flat strings — the array heuristic
        // would have read this SMEMBERS reply as a hash of "pairs"
        let set = redis::Value::Set(vec![
            redis::Value::BulkString(b"alice".to_vec()),
            redis::Value::BulkString(b"bob".to_vec()),
        ]);
        let out = redis_value_to_json(&set, "SMEMBERS", 100, 1024);
        assert_eq!(out["type"], "set");
        assert_eq!(out["value"], json!(["alice", "bob"]));
        assert_eq!(out["len"], 2);
    }

    #[test]
    fn resp3_scalars_get_real_shapes() {
        let out = redis_value_to_json(&redis::Value::Double(1.5), "GET", 100, 1024);
        assert_eq!(out, json!({"type": "number", "value": 1.5}));
        let out = redis_value_to_json(&redis::Value::Boolean(true), "COPY", 100, 1024);
        assert_eq!(out, json!({"type": "boolean", "value": true}));
        let out = redis_value_to_json(
            &redis::Value::VerbatimString {
                format: redis::VerbatimFormat::Text,
                text: "redis_version:7.4.0".into(),
            },
            "INFO",
            100,
            1024,
        );
        assert_eq!(out["type"], "string");
        assert_eq!(out["value"], "redis_version:7.4.0");
        // no debug-format "Double(1.5)" anywhere
        assert_eq!(redis_type_name(&redis::Value::Double(1.5)), "number");
        assert_eq!(status_text(&redis::Value::Double(1.5), usize::MAX), "1.5");
    }

    #[test]
    fn resp3_double_score_reads_as_a_number() {
        let arr = vec![
            redis::Value::BulkString(b"alice".to_vec()),
            redis::Value::Double(99.5),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "ZRANGE", 100, 1024);
        assert_eq!(out["value"][0]["score"], json!(99.5));
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
        assert_eq!(out["entries"], json!([["name", "Alice"], ["age", "30"]]));
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
    fn mget_stays_list_despite_even_string_count() {
        // MGET a b replies [1, 2] — flat strings, even count: the heuristic
        // must not read it as the hash {a: 1}; the values would render as
        // "keys" in the panel grid
        let arr = vec![
            redis::Value::BulkString(b"1".to_vec()),
            redis::Value::BulkString(b"2".to_vec()),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "MGET", 100, 1024);
        assert_eq!(out["type"], "list");
        assert_eq!(out["value"], json!(["1", "2"]));

        let arr2 = vec![
            redis::Value::BulkString(b"v1".to_vec()),
            redis::Value::BulkString(b"v2".to_vec()),
        ];
        let out2 = redis_value_to_json(&redis::Value::Array(arr2), "HMGET", 100, 1024);
        assert_eq!(out2["type"], "list");
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
        let m = vec![(
            redis::Value::BulkString(b"k".to_vec()),
            redis::Value::BulkString(b"v".to_vec()),
        )];
        let out = redis_value_to_json(&redis::Value::Map(m.clone()), "XREAD", 100, 1024);
        assert_eq!(out["type"], "hash");
        assert_eq!(out["entries"], json!([["k", "v"]]));
    }

    #[test]
    fn hscan_surfaces_cursor_and_parses_items() {
        let arr = vec![
            redis::Value::BulkString(b"17".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"name".to_vec()),
                redis::Value::BulkString(b"Alice".to_vec()),
                redis::Value::BulkString(b"age".to_vec()),
                redis::Value::BulkString(b"30".to_vec()),
            ]),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "HSCAN", 100, 1024);
        assert_eq!(out["cursor"], "17");
        assert_eq!(out["type"], "hash");
        assert_eq!(out["entries"], json!([["name", "Alice"], ["age", "30"]]));
        assert_eq!(out["len"], 2);
    }

    #[test]
    fn scan_surfaces_cursor_as_list() {
        let arr = vec![
            redis::Value::BulkString(b"0".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"user:1".to_vec()),
                redis::Value::BulkString(b"user:2".to_vec()),
            ]),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "SCAN", 100, 1024);
        assert_eq!(out["cursor"], "0");
        assert_eq!(out["type"], "list");
        assert_eq!(out["value"], json!(["user:1", "user:2"]));
    }

    #[test]
    fn hscan_empty_items_keeps_cursor() {
        let arr = vec![
            redis::Value::BulkString(b"0".to_vec()),
            redis::Value::Array(vec![]),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "HSCAN", 100, 1024);
        assert_eq!(out["cursor"], "0");
        assert_eq!(out["len"], 0);
    }

    #[test]
    fn zscan_surfaces_cursor_and_parses_zset() {
        let arr = vec![
            redis::Value::BulkString(b"0".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"alice".to_vec()),
                redis::Value::BulkString(b"99".to_vec()),
                redis::Value::BulkString(b"bob".to_vec()),
                redis::Value::Int(87),
            ]),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "ZSCAN", 100, 1024);
        assert_eq!(out["cursor"], "0");
        assert_eq!(out["type"], "zset");
        assert_eq!(out["len"], 2);
        assert_eq!(
            out["value"],
            json!([{"member": "alice", "score": 99.0}, {"member": "bob", "score": 87.0}])
        );
    }

    #[test]
    fn sscan_surfaces_cursor_and_parses_set() {
        let arr = vec![
            redis::Value::BulkString(b"0".to_vec()),
            redis::Value::Array(vec![
                redis::Value::BulkString(b"vim".to_vec()),
                redis::Value::BulkString(b"lua".to_vec()),
                redis::Value::BulkString(b"redis".to_vec()),
            ]),
        ];
        let out = redis_value_to_json(&redis::Value::Array(arr), "SSCAN", 100, 1024);
        assert_eq!(out["cursor"], "0");
        assert_eq!(out["type"], "set");
        assert_eq!(out["len"], 3);
        assert_eq!(out["value"], json!(["vim", "lua", "redis"]));
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
                vec![
                    "SET".into(),
                    "poste:test".into(),
                    "1".into(),
                    "EX".into(),
                    "60".into(),
                ],
                vec!["GET".into(), "poste:test".into()],
                vec![
                    "HSET".into(),
                    "poste:h".into(),
                    "a".into(),
                    "1".into(),
                    "b".into(),
                    "2".into(),
                ],
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
