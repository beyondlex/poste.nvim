use anyhow::Result;
use clap::Parser;
use serde_json::{json, Value};
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

        let outcome = execute_command_on(
            &mut con,
            &tokens,
            seq,
            args.max_items as usize,
            args.max_bytes as usize,
        )
        .await;

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
