// Process-level integration test for `poste mq-exec`.
// Only runs when MQ_TEST_URL is provided (CI service / local docker).

use std::io::Write;
use std::process::{Command, Stdio};

fn run_mq_exec(operations: serde_json::Value) -> Vec<serde_json::Value> {
    let Ok(url) = std::env::var("MQ_TEST_URL") else {
        return Vec::new();
    };
    let bin = env!("CARGO_BIN_EXE_poste");

    let mut child = Command::new(bin)
        .args(["mq-exec"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn poste");

    let payload = serde_json::json!({
        "connection": url,
        "operations": operations,
    });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();

    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is JSON"))
        .collect()
}

fn results(lines: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    lines.iter().filter(|l| l["type"] == "result").collect()
}

#[test]
fn mq_exec_publish_consume_peek_and_unroutable() {
    let Ok(_) = std::env::var("MQ_TEST_URL") else {
        return;
    };
    let queue = "poste.mqexec.test";

    // declare + publish + peek + unroutable + cleanup
    let lines = run_mq_exec(serde_json::json!([
        {"op": "declare", "kind": "queue", "name": queue, "durable": false},
        {"op": "publish", "queue": queue, "payload": "{\"n\":1}",
         "properties": {"content_type": "application/json", "correlation_id": "c1"}},
        {"op": "consume", "queue": queue, "count": 5, "ack": false},
        {"op": "publish", "queue": "poste.no.such.queue.e2e", "payload": "x"},
        {"op": "consume", "queue": queue, "count": 5, "ack": true},
        {"op": "delete", "kind": "queue", "name": queue},
    ]));
    if lines.is_empty() {
        return; // gate unset
    }

    let results = results(&lines);
    assert_eq!(results.len(), 6);
    // 6 × (progress + result) + 1 summary
    assert_eq!(lines.len(), 13);

    // declare ok, reports depth
    assert_eq!(results[0]["status"], "ok");
    assert_eq!(results[0]["value"]["kind"], "queue");

    // publish with confirm evidence
    assert_eq!(results[1]["status"], "ok");
    assert_eq!(results[1]["value"]["routed"], true);
    assert_eq!(results[1]["value"]["confirmed"], true);

    // peek: exactly one distinct message, requeued afterwards
    assert_eq!(results[2]["status"], "ok");
    assert_eq!(results[2]["value"]["message_count"], 1);
    let message = &results[2]["value"]["messages"][0];
    assert_eq!(message["payload_json"]["n"], 1);
    assert_eq!(message["properties"]["correlation_id"], "c1");

    // unroutable publish FAILS (§4.2): HTTP-less protocol, evidence = basic.return
    assert_eq!(results[3]["status"], "error");
    assert!(results[3]["error"]
        .as_str()
        .unwrap()
        .contains("message not routed"));

    // ack consume removed the requeued message
    assert_eq!(results[4]["status"], "ok");
    assert_eq!(results[4]["value"]["message_count"], 1);

    // summary counts the one failure
    let summary = lines.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["failed"], 1);
    assert_eq!(summary["total"], 6);
}

#[test]
fn mq_exec_rejects_non_amqp_urls_and_list() {
    let Ok(_) = std::env::var("MQ_TEST_URL") else {
        return;
    };
    let bin = env!("CARGO_BIN_EXE_poste");

    // non-amqp URL fails the process with a clear message
    let mut child = Command::new(bin)
        .args(["mq-exec"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::json!({"connection": "redis://h", "operations": []})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("amqp"));

    // list has no AMQP method: greedy error event, batch continues
    let lines = run_mq_exec(serde_json::json!([
        {"op": "list", "noun": "queues"},
        {"op": "declare", "kind": "queue", "name": "poste.mqexec.list", "durable": false},
        {"op": "delete", "kind": "queue", "name": "poste.mqexec.list"},
    ]));
    if lines.is_empty() {
        return;
    }
    let results = results(&lines);
    assert_eq!(results[0]["status"], "error");
    assert!(results[0]["error"].as_str().unwrap().contains("list"));
    assert_eq!(results[1]["status"], "ok");
}
