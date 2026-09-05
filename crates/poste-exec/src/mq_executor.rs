//! AMQP (RabbitMQ) operation execution for the poste binary.
//!
//! `poste mq-exec` contract (poste-mq.nvim requirements §2.2, P1-1): Lua is
//! the only parser — this module receives pre-decoded operation objects
//! (never `.mq` file text) and reports one outcome per operation; operation
//! errors never stop the batch (greedy, redis-exec shape).
//!
//! ok semantics (poste-mq.nvim requirements §4.2): publish success requires
//! publisher-confirm evidence — unroutable messages come back via
//! mandatory + basic.return and mark the operation failed (routed=false).
//!
//! AMQP has no topology-enumeration method: `list` operations are rejected
//! here by design; the Lua router sends LIST to the management transport.

use anyhow::Result;
use lapin::options::{
    BasicAckOptions, BasicGetOptions, BasicNackOptions, BasicPublishOptions, ConfirmSelectOptions,
    ExchangeBindOptions, ExchangeDeclareOptions, ExchangeDeleteOptions, ExchangeUnbindOptions,
    QueueBindOptions, QueueDeclareOptions, QueueDeleteOptions, QueuePurgeOptions,
};
use lapin::ExchangeKind;
use lapin::types::{AMQPValue, FieldTable, ShortString};
use lapin::{
    message::Delivery, uri::AMQPUri, BasicProperties, Channel, Connection, ConnectionProperties,
};
use serde_json::{json, Value};

/// Per-operation outcomes for a batch (redis `CommandOutcome` shape).
pub struct MqOutcome {
    /// 1-based position in the batch.
    pub seq: usize,
    /// Canonical operation name (publish/consume/declare/bind/unbind/purge/delete).
    pub operation: String,
    pub latency_ms: u128,
    /// Structured metadata for the canonical response.
    pub value: Value,
    pub error: Option<String>,
}

pub fn validate_connection_url(url: &str) -> Result<()> {
    if url.starts_with("amqp://") || url.starts_with("amqps://") {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Not an amqp connection URL (expected amqp:// or amqps://): {}",
            url
        ))
    }
}

fn parse_uri(url: &str) -> Result<AMQPUri> {
    url.parse::<AMQPUri>()
        .map_err(|e| anyhow::anyhow!("Invalid amqp URL: {}", e))
}

// ── Property/argument conversions ───────────────────────────────────────

/// JSON headers/arguments → AMQP FieldTable. Field values map by JSON type;
/// objects recurse (nested FieldTable); arrays are skipped (Lua never sends
/// them — AMQP field arrays round-trip poorly through JSON).
pub fn json_to_field_table(v: &Value) -> FieldTable {
    let mut table = FieldTable::default();
    if let Value::Object(map) = v {
        for (k, val) in map {
            let amqp = match val {
                Value::Bool(b) => AMQPValue::Boolean(*b),
                Value::Number(n) => {
                    // AMQP field ints: i32 (LongInt) when it fits, i64 otherwise
                    match n.as_i64() {
                        Some(i) if i >= i32::MIN as i64 && i <= i32::MAX as i64 => {
                            AMQPValue::LongInt(i as i32)
                        }
                        Some(i) => AMQPValue::LongLongInt(i),
                        None => AMQPValue::Double(n.as_f64().unwrap_or(0.0)),
                    }
                }
                Value::String(s) => AMQPValue::LongString(s.clone().into_bytes().into()),
                Value::Object(_) => AMQPValue::FieldTable(json_to_field_table(val)),
                _ => continue,
            };
            table.insert(ShortString::from(k.as_str()), amqp);
        }
    }
    table
}

fn long_string_to_json(s: &lapin::types::LongString) -> Value {
    json!(String::from_utf8_lossy(s.as_bytes()).to_string())
}

fn field_value_to_json(v: &AMQPValue) -> Value {
    match v {
        AMQPValue::Boolean(b) => json!(b),
        AMQPValue::ShortShortInt(i) => json!(i),
        AMQPValue::ShortShortUInt(u) => json!(u),
        AMQPValue::ShortInt(i) => json!(i),
        AMQPValue::ShortUInt(u) => json!(u),
        AMQPValue::LongInt(i) => json!(i),
        AMQPValue::LongUInt(u) => json!(u),
        AMQPValue::LongLongInt(i) => json!(i),
        AMQPValue::Float(f) => json!(f),
        AMQPValue::Double(d) => json!(d),
        AMQPValue::DecimalValue(d) => json!({"scale": d.scale, "value": d.value}),
        AMQPValue::ShortString(s) => json!(s.as_str()),
        AMQPValue::LongString(s) => long_string_to_json(s),
        AMQPValue::FieldArray(_) => json!(null),
        AMQPValue::ByteArray(b) => json!({"bytes": b.len(), "encoding": "base64"}),
        AMQPValue::Timestamp(t) => json!(t),
        AMQPValue::FieldTable(t) => field_table_to_json(t),
        AMQPValue::Void => json!(null),
    }
}

pub fn field_table_to_json(t: &FieldTable) -> Value {
    let mut out = serde_json::Map::new();
    for (k, v) in t.inner().iter() {
        out.insert(k.as_str().to_string(), field_value_to_json(v));
    }
    Value::Object(out)
}

/// JSON properties (snake_case, already mapped by the Lua dialect layer)
/// → lapin BasicProperties.
pub fn json_to_properties(p: &Value) -> BasicProperties {
    let mut props = BasicProperties::default();
    let get = |k: &str| p.get(k).and_then(|v| v.as_str()).map(String::from);
    if let Some(v) = get("content_type") {
        props = props.with_content_type(v.into());
    }
    if let Some(v) = get("content_encoding") {
        props = props.with_content_encoding(v.into());
    }
    if let Some(n) = p.get("delivery_mode").and_then(|v| v.as_u64()) {
        props = props.with_delivery_mode(n as u8);
    }
    if let Some(n) = p.get("priority").and_then(|v| v.as_u64()) {
        props = props.with_priority(n as u8);
    }
    if let Some(v) = get("correlation_id") {
        props = props.with_correlation_id(v.into());
    }
    if let Some(v) = get("reply_to") {
        props = props.with_reply_to(v.into());
    }
    if let Some(v) = get("expiration") {
        props = props.with_expiration(v.into());
    }
    if let Some(v) = get("message_id") {
        props = props.with_message_id(v.into());
    }
    if let Some(v) = get("type") {
        props = props.with_type(v.into());
    }
    if let Some(v) = get("user_id") {
        props = props.with_user_id(v.into());
    }
    if let Some(v) = get("app_id") {
        props = props.with_app_id(v.into());
    }
    if let Some(n) = p.get("timestamp").and_then(|v| v.as_u64()) {
        props = props.with_timestamp(n);
    }
    if let Some(headers) = p
        .get("headers")
        .filter(|h| h.as_object().map_or(false, |m| !m.is_empty()))
    {
        props = props.with_headers(json_to_field_table(headers));
    }
    props
}

/// Delivery properties + headers → canonical snake_case property map
/// (field names mirror the management transport so the two transports
/// produce equivalent canonical responses — P1-3).
pub fn delivery_properties_to_json(delivery: &Delivery) -> Value {
    let p = &delivery.properties;
    let mut out = serde_json::Map::new();
    let put_s = |out: &mut serde_json::Map<String, Value>, k: &str, v: &Option<ShortString>| {
        if let Some(s) = v {
            out.insert(k.to_string(), json!(s.as_str()));
        }
    };
    put_s(&mut out, "content_type", p.content_type());
    put_s(&mut out, "content_encoding", p.content_encoding());
    put_s(&mut out, "correlation_id", p.correlation_id());
    put_s(&mut out, "reply_to", p.reply_to());
    put_s(&mut out, "message_id", p.message_id());
    put_s(&mut out, "type", p.kind());
    put_s(&mut out, "user_id", p.user_id());
    put_s(&mut out, "app_id", p.app_id());
    if let Some(n) = p.delivery_mode() {
        out.insert("delivery_mode".into(), json!(n));
    }
    if let Some(n) = p.priority() {
        out.insert("priority".into(), json!(n));
    }
    if let Some(s) = p.expiration() {
        out.insert("expiration".into(), json!(s.as_str()));
    }
    if let Some(t) = p.timestamp() {
        out.insert("timestamp".into(), json!(t));
    }
    json!(out)
}

// ── Per-operation execution ─────────────────────────────────────────────

async fn op_publish(channel: &Channel, op: &Value) -> Result<Value> {
    let (exchange, routing_key) = match op.get("queue").and_then(|q| q.as_str()) {
        Some(queue) => (String::new(), queue.to_string()), // default exchange
        None => (
            op.get("exchange")
                .and_then(|e| e.as_str())
                .unwrap_or_default()
                .to_string(),
            op.get("routing_key")
                .and_then(|r| r.as_str())
                .unwrap_or_default()
                .to_string(),
        ),
    };
    let payload = op
        .get("payload")
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .as_bytes()
        .to_vec();
    let props = json_to_properties(op.get("properties").unwrap_or(&Value::Null));
    let mut options = BasicPublishOptions::default();
    options.mandatory = op.get("mandatory").and_then(|m| m.as_bool()).unwrap_or(true);

    // Publisher confirms: unroutable messages (mandatory + nothing bound)
    // come back via basic.return and MUST mark the operation failed
    // (requirements §4.2 — never trust the absence of an error).
    let confirm_mode = op.get("confirm").and_then(|c| c.as_bool()).unwrap_or(true);
    if confirm_mode {
        channel
            .confirm_select(ConfirmSelectOptions::default())
            .await?;
    }
    channel
        .basic_publish(
            exchange.as_str(),
            routing_key.as_str(),
            options,
            &payload,
            props,
        )
        .await?;
    let mut routed = true;
    if confirm_mode {
        let returns = channel.wait_for_confirms().await?;
        if !returns.is_empty() {
            routed = false;
        }
    }
    if !routed {
        return Err(anyhow::anyhow!(
            "message not routed (unroutable: no queue bound or routing key mismatch)"
        ));
    }
    Ok(json!({"routed": true, "confirmed": confirm_mode}))
}

/// One consumed message in canonical shape (management transport field names,
/// so the two transports produce equivalent canonical responses — P1-3).
pub fn delivery_to_message(delivery: &Delivery, queue_depth: u32) -> Value {
    let payload = String::from_utf8_lossy(&delivery.data).to_string();
    let mut message = json!({
        "exchange": delivery.exchange.as_str(),
        "routing_key": delivery.routing_key.as_str(),
        "redelivered": delivery.redelivered,
        "properties": delivery_properties_to_json(delivery),
        "payload": payload,
        "payload_encoding": "string",
        "queue_depth": queue_depth,
    });
    if let Ok(parsed) = serde_json::from_str::<Value>(&message["payload"].as_str().unwrap_or("")) {
        message["payload_json"] = parsed;
    }
    if let Some(t) = delivery.properties.timestamp() {
        message["ts"] = json!(t);
    }
    message
}

async fn op_consume(channel: &Channel, op: &Value) -> Result<Value> {
    let queue = op
        .get("queue")
        .and_then(|q| q.as_str())
        .ok_or_else(|| anyhow::anyhow!("consume requires queue"))?;
    let count = op.get("count").and_then(|c| c.as_u64()).unwrap_or(10);
    let ack = op.get("ack").and_then(|a| a.as_bool()).unwrap_or(false);

    let mut messages = Vec::new();
    let mut depth = None;
    // Buffer deliveries and settle them AFTER the loop: acking/nacking each
    // message immediately would hand the SAME message back on the next
    // basic_get. Deferring the requeue drains up to min(count, depth)
    // distinct messages first — matching the management API's get semantics.
    let mut pending = Vec::new();
    for _ in 0..count {
        let result = channel.basic_get(queue, BasicGetOptions::default()).await?;
        match result {
            Some(get) => {
                depth = Some(get.message_count);
                messages.push(delivery_to_message(&get.delivery, get.message_count));
                pending.push(get);
            }
            None => break, // empty queue is a legal outcome
        }
    }
    for get in pending {
        if ack {
            get.delivery.acker.ack(BasicAckOptions::default()).await?;
        } else {
            // Peek: requeue so the queue depth is unchanged
            // (non-destructive default, requirements §3.5).
            get.delivery
                .acker
                .nack(BasicNackOptions { requeue: true, ..Default::default() })
                .await?;
        }
    }
    Ok(json!({
        "message_count": messages.len(),
        "queue_depth": depth.unwrap_or(0),
        "messages": messages,
    }))
}

fn field_arguments(op: &Value) -> FieldTable {
    match op.get("arguments") {
        Some(v) if v.is_object() => json_to_field_table(v),
        _ => FieldTable::default(),
    }
}

async fn op_declare(channel: &Channel, op: &Value) -> Result<Value> {
    let kind = op
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or_else(|| anyhow::anyhow!("declare requires kind"))?;
    let name = op
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("declare requires name"))?
        .to_string();
    let durable = op.get("durable").and_then(|d| d.as_bool()).unwrap_or(true);
    let auto_delete = op
        .get("auto_delete")
        .and_then(|d| d.as_bool())
        .unwrap_or(false);
    let args = field_arguments(op);
    match kind {
        "exchange" => {
            let exchange_type = match op.get("exchange_type").and_then(|t| t.as_str()) {
                Some("fanout") => ExchangeKind::Fanout,
                Some("direct") => ExchangeKind::Direct,
                Some("headers") => ExchangeKind::Headers,
                _ => ExchangeKind::Topic,
            };
            channel
                .exchange_declare(
                    name.as_str(),
                    exchange_type,
                    ExchangeDeclareOptions { durable, auto_delete, ..Default::default() },
                    args,
                )
                .await?;
            Ok(json!({"kind": "exchange", "name": name}))
        }
        _ => {
            let ok = channel
                .queue_declare(
                    name.as_str(),
                    QueueDeclareOptions { durable, auto_delete, ..Default::default() },
                    args,
                )
                .await?;
            Ok(json!({
                "kind": "queue",
                "name": name,
                "message_count": ok.message_count(),
                "consumer_count": ok.consumer_count(),
            }))
        }
    }
}

async fn op_bind(channel: &Channel, op: &Value, unbind: bool) -> Result<Value> {
    let source = op
        .get("source")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("bind requires source (exchange)"))?
        .to_string();
    let destination = op
        .get("destination")
        .and_then(|d| d.as_str())
        .ok_or_else(|| anyhow::anyhow!("bind requires destination"))?
        .to_string();
    let routing_key = op
        .get("routing_key")
        .and_then(|r| r.as_str())
        .unwrap_or_default()
        .to_string();
    let is_exchange_dest =
        op.get("destination_kind").and_then(|k| k.as_str()) == Some("exchange");
    if unbind {
        if is_exchange_dest {
            channel
                .exchange_unbind(
                    destination.as_str(),
                    source.as_str(),
                    routing_key.as_str(),
                    ExchangeUnbindOptions::default(),
                    FieldTable::default(),
                )
                .await?;
        } else {
            channel
                .queue_unbind(
                    destination.as_str(),
                    source.as_str(),
                    routing_key.as_str(),
                    FieldTable::default(),
                )
                .await?;
        }
    } else {
        let args = field_arguments(op);
        if is_exchange_dest {
            channel
                .exchange_bind(
                    destination.as_str(),
                    source.as_str(),
                    routing_key.as_str(),
                    ExchangeBindOptions::default(),
                    args,
                )
                .await?;
        } else {
            channel
                .queue_bind(
                    destination.as_str(),
                    source.as_str(),
                    routing_key.as_str(),
                    QueueBindOptions::default(),
                    args,
                )
                .await?;
        }
    }
    Ok(json!({}))
}

async fn op_purge(channel: &Channel, op: &Value) -> Result<Value> {
    let queue = op
        .get("queue")
        .and_then(|q| q.as_str())
        .ok_or_else(|| anyhow::anyhow!("purge requires queue"))?;
    let purged = channel
        .queue_purge(queue, QueuePurgeOptions::default())
        .await?;
    Ok(json!({"purged": purged}))
}

async fn op_delete(channel: &Channel, op: &Value) -> Result<Value> {
    let kind = op.get("kind").and_then(|k| k.as_str()).unwrap_or("queue");
    let name = op
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("delete requires name"))?;
    if kind == "exchange" {
        channel
            .exchange_delete(name, ExchangeDeleteOptions::default())
            .await?;
        Ok(json!({}))
    } else {
        let deleted = channel
            .queue_delete(name, QueueDeleteOptions::default())
            .await?;
        Ok(json!({"deleted_messages": deleted}))
    }
}

pub async fn execute_operation_on(
    channel: &Channel,
    op: &Value,
    seq: usize,
) -> MqOutcome {
    let started = std::time::Instant::now();
    let operation = op
        .get("op")
        .and_then(|o| o.as_str())
        .unwrap_or("unknown")
        .to_string();
    let result: Result<Value> = match operation.as_str() {
        "publish" => op_publish(channel, op).await,
        "consume" => op_consume(channel, op).await,
        "declare" => op_declare(channel, op).await,
        "bind" => op_bind(channel, op, false).await,
        "unbind" => op_bind(channel, op, true).await,
        "purge" => op_purge(channel, op).await,
        "delete" => op_delete(channel, op).await,
        other => Err(anyhow::anyhow!("unsupported operation: {}", other)),
    };
    let value = result.as_ref().ok().cloned().unwrap_or(Value::Null);
    MqOutcome {
        seq,
        operation,
        latency_ms: started.elapsed().as_millis(),
        value,
        error: result.err().map(|e| e.to_string()),
    }
}

/// Execute a batch of pre-decoded operations over one connection, invoking
/// `on_outcome` per operation (greedy: errors never stop the batch).
pub async fn execute_operations_with<F>(
    url: &str,
    operations: &[Value],
    mut on_outcome: F,
) -> Result<()>
where
    F: FnMut(MqOutcome),
{
    validate_connection_url(url)?;
    // lapin parses the URI itself (Connect impl for &str).
    let connection = Connection::connect(url, ConnectionProperties::default()).await?;
    let channel = connection.create_channel().await?;

    for (i, op) in operations.iter().enumerate() {
        let outcome = execute_operation_on(&channel, op, i + 1).await;
        on_outcome(outcome);
    }
    connection.close(0, "").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_amqp_urls() {
        assert!(validate_connection_url("amqp://h:5672/%2F").is_ok());
        assert!(validate_connection_url("amqps://h").is_ok());
        assert!(validate_connection_url("redis://h").is_err());
        assert!(validate_connection_url("http://h").is_err());
    }

    #[test]
    fn parses_uri_with_vhost() {
        let uri = parse_uri("amqp://bob:s3cret@localhost:5672/%2F").unwrap();
        assert_eq!(uri.vhost, "/");
        assert_eq!(uri.authority.userinfo.username, "bob");
        let uri2 = parse_uri("amqp://localhost:5672/myvh").unwrap();
        assert_eq!(uri2.vhost, "myvh");
    }

    #[test]
    fn json_properties_map_to_amqp_and_back() {
        let props = json!({
            "content_type": "application/json",
            "delivery_mode": 2,
            "correlation_id": "c1",
            "headers": {"X-Trace": "t1", "ttl": 30, "dry": true},
        });
        let amqp_props = json_to_properties(&props);
        assert_eq!(
            amqp_props.content_type(),
            &Some(ShortString::from("application/json"))
        );
        assert_eq!(amqp_props.delivery_mode(), &Some(2));
        assert_eq!(
            amqp_props.correlation_id(),
            &Some(ShortString::from("c1"))
        );
        let header_table = amqp_props.headers().as_ref().unwrap();
        let back = field_table_to_json(header_table);
        assert_eq!(back["X-Trace"], json!("t1"));
        assert_eq!(back["ttl"], json!(30));
        assert_eq!(back["dry"], json!(true));
    }

    #[test]
    fn empty_arguments_render_as_empty_table() {
        let args = field_arguments(&json!({"op": "declare"}));
        assert!(args.inner().is_empty());
    }
}
