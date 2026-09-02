use anyhow::Result;
use clap::Parser;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Persistent redis session: keeps one connection open across requests so
/// SELECT and other per-connection state survive between `<CR>` executions.
///
/// Wire protocol (poste-redis EXECUTION-PLAN §3.3):
///   stdin:  one JSON line per request  {"seq": 1, "command": ["GET", "k"]}
///   stdout: one JSON line per response {"type":"result","seq":1,...}
/// The stdin loop mirrors poste-cli `session.rs` (SQL sessions); the binary
/// never parses `.redis` file text (D1 — Lua is the only parser).
#[derive(Parser)]
pub struct RedisSessionArgs {
    /// Redis connection URL (redis:// or rediss://)
    #[arg(long)]
    pub connection: String,
    /// Max elements emitted per array/map reply (extra marked truncated)
    #[arg(long, default_value_t = 1000)]
    pub max_items: u64,
    /// Max bytes per string reply (extra marked truncated)
    #[arg(long, default_value_t = 65536)]
    pub max_bytes: u64,
}

/// Emit one NDJSON event line on stdout.
async fn emit(stdout: &mut tokio::io::Stdout, ev: Value) -> Result<()> {
    stdout
        .write_all(format!("{}\n", serde_json::to_string(&ev)?).as_bytes())
        .await?;
    stdout.flush().await?;
    Ok(())
}

pub async fn execute(args: RedisSessionArgs) -> Result<()> {
    use poste_exec::redis_executor::execute_command_on;

    poste_exec::redis_executor::validate_connection_url(&args.connection)?;

    let client = redis::Client::open(args.connection.as_str())?;
    let mut con = client.get_multiplexed_async_connection().await?;
    // The db the shared connection is currently SELECTed to (Lua steers it
    // with fire-and-forget SELECTs); restored after a reconnect.
    let mut current_db: Option<i64> = None;

    // Per-command bound: when the TCP connection dies mid-stream (docker's
    // userland proxy drops connections under burst load, network blips), a
    // multiplexed-command future may never resolve and the session would
    // hang forever — no response, no error, every later request queued
    // behind it. Bound each attempt; on timeout rebuild the connection,
    // restore SELECT state, and retry the command once.
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
    const RECONNECT_TIMEOUT: Duration = Duration::from_secs(10);

    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"type":"result","seq":0,"status":"error","error":format!("JSON parse error: {}", e)});
                emit(&mut stdout, err).await?;
                continue;
            }
        };
        let seq = req.get("seq").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let tokens: Vec<String> = req
            .get("command")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if tokens.is_empty() {
            continue;
        }

        let max_items = args.max_items as usize;
        let max_bytes = args.max_bytes as usize;
        let mut attempt = tokio::time::timeout(
            COMMAND_TIMEOUT,
            execute_command_on(&mut con, &tokens, seq, max_items, max_bytes),
        )
        .await;
        // A dead connection surfaces either as a hang (timeout) or as an
        // immediate connection-class error from the multiplexed driver.
        // Either way: rebuild the connection, restore the tracked SELECT
        // state, and retry the command once.
        let needs_rescue = match &attempt {
            Err(_) => true,
            Ok(outcome) => match &outcome.error {
                Some(err) => {
                    let e = err.to_lowercase();
                    e.contains("connection") || e.contains("broken") || e.contains("dropped")
                }
                None => false,
            },
        };
        if needs_rescue {
            let rebuilt = tokio::time::timeout(
                RECONNECT_TIMEOUT,
                client.get_multiplexed_async_connection(),
            )
            .await;
            match rebuilt {
                Ok(Ok(fresh)) => {
                    con = fresh;
                    if let Some(db) = current_db.filter(|d| *d != 0) {
                        let _ = redis::cmd("SELECT")
                            .arg(db.to_string())
                            .query_async::<String>(&mut con)
                            .await;
                    }
                    attempt = tokio::time::timeout(
                        COMMAND_TIMEOUT,
                        execute_command_on(&mut con, &tokens, seq, max_items, max_bytes),
                    )
                    .await;
                }
                _ => {} // reconnect failed: fall through to the error outcome
            }
        }
        let mut outcome = match attempt {
            Ok(outcome) => outcome,
            Err(_) => poste_exec::redis_executor::CommandOutcome {
                command: tokens.join(" "),
                seq,
                latency_ms: 0,
                value: json!({"type": "nil", "value": null}),
                status: "error".into(),
                error: Some("redis connection lost (command timed out)".into()),
            },
        };
        if outcome.error.is_none()
            && tokens.first().map(|t| t.to_uppercase() == "SELECT").unwrap_or(false)
        {
            if let Some(db) = tokens.get(1).and_then(|t| t.parse::<i64>().ok()) {
                current_db = Some(db);
            }
        }

        let mut ev = json!({
            "type": "result",
            "seq": outcome.seq,
            "command": outcome.command,
            "status": outcome.status,
            "latency_ms": outcome.latency_ms,
            "value": outcome.value,
        });
        if let Some(err) = &outcome.error {
            ev["error"] = json!(err);
            ev["status"] = json!("error");
        }
        emit(&mut stdout, ev).await?;
    }

    Ok(())
}
