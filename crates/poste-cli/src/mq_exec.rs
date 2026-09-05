use anyhow::Result;
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;

/// Execute pre-decoded AMQP operations read from stdin as a single JSON
/// line: {"connection": "amqp://...", "operations": [{...}, ...]}.
/// Lua is the only parser (poste-mq.nvim requirements §2.2) — this binary
/// never sees `.mq` file text. Emits one JSON event per line:
/// progress → result (per operation) → summary. Operation errors never stop
/// the batch (greedy); connection errors fail the process (stderr + exit 1).
///
/// LIST is intentionally unsupported here: AMQP has no topology-enumeration
/// method — the Lua router sends LIST to the management transport.
#[derive(Parser)]
pub struct MqExecArgs {}

#[derive(Deserialize)]
struct MqExecRequest {
    connection: String,
    operations: Vec<serde_json::Value>,
}

pub async fn execute(_args: MqExecArgs) -> Result<()> {
    let mut input = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut tokio::io::stdin(), &mut input).await?;
    let req: MqExecRequest = serde_json::from_str(input.trim())
        .map_err(|e| anyhow::anyhow!("Invalid stdin JSON: {}", e))?;

    let total = req.operations.len();
    let started = Instant::now();
    let mut failed = 0u64;

    poste_exec::mq_executor::execute_operations_with(&req.connection, &req.operations, |outcome| {
        println!(
            "{}",
            json!({
                "type": "progress",
                "index": outcome.seq,
                "total": total,
                "operation": outcome.operation,
            })
        );
        let mut ev = json!({
            "type": "result",
            "seq": outcome.seq,
            "operation": outcome.operation,
            "status": if outcome.error.is_some() { "error" } else { "ok" },
            "latency_ms": outcome.latency_ms,
            "value": outcome.value,
        });
        if let Some(err) = &outcome.error {
            failed += 1;
            ev["error"] = json!(err);
        }
        println!("{}", ev);
    })
    .await?;

    println!(
        "{}",
        json!({
            "type": "summary",
            "total": total,
            "failed": failed,
            "elapsed_ms": started.elapsed().as_millis(),
        })
    );
    Ok(())
}
