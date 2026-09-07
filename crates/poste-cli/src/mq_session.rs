use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use lapin::acker::Acker;
use lapin::options::{
    BasicAckOptions, BasicCancelOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Persistent AMQP session (poste-mq.nvim P1-5): keeps one connection open
/// across requests and hosts live push consumers for the interactive tail
/// (P2-4). Wire protocol (redis-session shape):
///   stdin:  {"seq":1,"operation":{...}}                          one-shot op
///           {"seq":2,"consumer":"tail","action":"start","queue":"q",
///            "ack":false}                                        start push consumer
///           {"seq":3,"consumer":"tail","action":"stop"}          stop consumer
///   stdout: {"type":"result","seq":..,...}   request/response
///           {"type":"message","consumer":..,"message":{..}}  push events
///           {"type":"session","status":"closed"}             on shutdown
/// Consumers default to requeue mode (non-destructive tail, requirements
/// §3.5); ack mode removes messages. Requeue-mode forwarders do NOT nack
/// per message (that redelivers the same message instantly forever); they
/// hold deliveries unacked under a prefetch cap and, on consumer stop or
/// session close, cancel the consumer then batch-nack the held deliveries
/// back to the queue.
#[derive(Parser)]
pub struct MqSessionArgs {
    /// AMQP connection URL (amqp:// or amqps://)
    #[arg(long)]
    pub connection: String,
}

/// Cap on unacked in-flight deliveries (channel QoS). Bounds the requeue-mode
/// watch buffer and gives ack-mode consumers backpressure against floods.
const WATCH_PREFETCH: u16 = 500;

/// Live consumer registry entry: forwarder task + stop signal. The stop
/// signal lets the task requeue outstanding deliveries before exiting,
/// instead of abort() leaving messages in unacked limbo.
struct ConsumerHandle {
    task: tokio::task::JoinHandle<()>,
    stop: oneshot::Sender<()>,
}

pub async fn execute(args: MqSessionArgs) -> Result<()> {
    use poste_exec::mq_executor::{
        delivery_to_message, execute_operation_on, validate_connection_url,
    };

    validate_connection_url(&args.connection)?;
    let connection =
        lapin::Connection::connect(&args.connection, lapin::ConnectionProperties::default())
            .await?;
    let channel = connection.create_channel().await?;

    // Single writer: request handlers and consumer forwarders push lines.
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let main_tx = tx.clone();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(ev) = rx.recv().await {
            let line = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
            if stdout
                .write_all(format!("{}\n", line).as_bytes())
                .await
                .is_err()
            {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    // consumer name → forwarder task handle + stop signal (abort on stop)
    let mut consumers: HashMap<String, ConsumerHandle> = HashMap::new();

    let mut stdout_lines = BufReader::new(tokio::io::stdin()).lines();
    let mut session_ok = true;

    while let Some(line) = stdout_lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = main_tx.send(json!({"type":"result","seq":0,"status":"error",
                        "error":format!("JSON parse error: {}", e)}));
                continue;
            }
        };
        let seq = req.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);

        // Consumer control?
        if let Some(consumer_name) = req
            .get("consumer")
            .and_then(|c| c.as_str())
            .map(String::from)
        {
            let action = req.get("action").and_then(|a| a.as_str()).unwrap_or("");
            match action {
                "start" => {
                    let queue = req
                        .get("queue")
                        .and_then(|q| q.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let ack = req.get("ack").and_then(|a| a.as_bool()).unwrap_or(false);
                    if consumers.contains_key(&consumer_name) {
                        let _ = main_tx.send(json!({"type":"result","seq":seq,"status":"error",
                                "error":format!("consumer already running: {}", consumer_name)}));
                        continue;
                    }
                    // Bounded in-flight deliveries (QoS). Watch mode holds
                    // unacked deliveries until stop, so this must be bounded
                    // or the session would buffer the whole queue.
                    if let Err(e) = channel
                        .basic_qos(WATCH_PREFETCH, BasicQosOptions::default())
                        .await
                    {
                        let _ = main_tx.send(json!({"type":"result","seq":seq,"status":"error",
                                "error":format!("qos failed: {}", e)}));
                        continue;
                    }
                    let consumer_tag = format!("poste-{}", consumer_name);
                    let consumer = channel
                        .basic_consume(
                            queue.as_str(),
                            &consumer_tag,
                            BasicConsumeOptions::default(),
                            Default::default(),
                        )
                        .await;
                    match consumer {
                        Ok(stream) => {
                            let tx = tx.clone();
                            let chan = channel.clone();
                            let name = consumer_name.clone();
                            let tag = consumer_tag.clone();
                            let (stop_tx, stop_rx) = oneshot::channel();
                            let handle = tokio::spawn(async move {
                                let mut stream = stream;
                                let mut stop_rx = stop_rx;
                                // Unacked deliveries held for batch-requeue on
                                // stop (non-destructive watch, no redelivery loop).
                                let mut pending: Vec<Acker> = Vec::new();
                                loop {
                                    let next = tokio::select! {
                                        delivery = stream.next() => delivery,
                                        _ = &mut stop_rx => None,
                                    };
                                    let Some(delivery) = next else {
                                        break;
                                    };
                                    match delivery {
                                        Ok(delivery) => {
                                            let message = delivery_to_message(&delivery, 0);
                                            if ack {
                                                let _ = delivery
                                                    .acker
                                                    .ack(BasicAckOptions::default())
                                                    .await;
                                            } else {
                                                pending.push(delivery.acker);
                                            }
                                            if tx
                                                .send(json!({
                                                    "type": "message",
                                                    "consumer": name,
                                                    "message": message,
                                                }))
                                                .is_err()
                                            {
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = tx.send(json!({
                                                "type": "error",
                                                "consumer": name,
                                                "error": e.to_string(),
                                            }));
                                            break;
                                        }
                                    }
                                }
                                // Stop/close: unregister the consumer FIRST (so
                                // requeued messages cannot bounce straight back
                                // into this consumer), then return the held
                                // deliveries to the queue. Order matters:
                                // basic_cancel alone does NOT requeue while the
                                // channel stays open.
                                let _ =
                                    chan.basic_cancel(&tag, BasicCancelOptions::default()).await;
                                for acker in pending {
                                    let _ = acker
                                        .nack(BasicNackOptions {
                                            requeue: true,
                                            ..Default::default()
                                        })
                                        .await;
                                }
                            });
                            consumers.insert(
                                consumer_name.clone(),
                                ConsumerHandle {
                                    task: handle,
                                    stop: stop_tx,
                                },
                            );
                            let _ = main_tx.send(json!({"type":"result","seq":seq,"status":"ok",
                                    "value":{"consumer":consumer_name,"queue":queue}}));
                        }
                        Err(e) => {
                            let _ = main_tx
                                .send(json!({"type":"result","seq":seq,"status":"error",
                                    "error":e.to_string()}));
                        }
                    }
                }
                "stop" => {
                    if let Some(handle) = consumers.remove(&consumer_name) {
                        // Signal the forwarder so it batch-requeues any held
                        // deliveries before unregistering, then wait for it.
                        let _ = handle.stop.send(());
                        let _ = handle.task.await;
                    }
                    let _ = main_tx.send(json!({"type":"result","seq":seq,"status":"ok",
                            "value":{"consumer":consumer_name,"stopped":true}}));
                }
                other => {
                    let _ = main_tx.send(json!({"type":"result","seq":seq,"status":"error",
                            "error":format!("unknown consumer action: {}", other)}));
                }
            }
            continue;
        }

        // One-shot operation (same shapes as mq-exec).
        let operation = req.get("operation").cloned().unwrap_or(Value::Null);
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            execute_operation_on(&channel, &operation, seq as usize),
        )
        .await;
        match outcome {
            Ok(outcome) => {
                let mut ev = json!({
                    "type": "result",
                    "seq": seq,
                    "operation": outcome.operation,
                    "status": if outcome.error.is_some() { "error" } else { "ok" },
                    "latency_ms": outcome.latency_ms,
                    "value": outcome.value,
                });
                if let Some(err) = &outcome.error {
                    ev["error"] = json!(err);
                }
                let _ = main_tx.send(ev);
            }
            Err(_) => {
                // Channel/connection wedged — end the session so the Lua
                // manager can restart it cleanly (session_conn pattern).
                session_ok = false;
                let _ = main_tx.send(json!({"type":"result","seq":seq,"status":"error",
                        "error":"session operation timed out (30s); session closing"}));
                break;
            }
        }
    }

    for (_, handle) in consumers.drain() {
        let _ = handle.stop.send(());
        let _ = handle.task.await;
    }
    let _ = connection.close(0, "").await;
    // Drop BOTH senders before joining the writer: main_tx still in scope
    // here would keep rx open forever and deadlock writer.await (the old
    // code relied on the Lua jobstart timeout kill to reap the process).
    drop(tx);
    drop(main_tx);
    let _ = writer.await;
    if !session_ok {
        std::process::exit(1);
    }
    Ok(())
}
