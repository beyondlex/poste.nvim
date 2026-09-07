// Process-level integration test for `poste mq-session` push consumers.
// Only runs when MQ_TEST_URL is provided (CI service / local docker).
//
// Regression: requeue-mode (non-destructive watch) consumers must NOT
// redelivery-loop. The old forwarder nacked every delivery with
// requeue=true immediately, so RabbitMQ redelivered the same message the
// instant the consumer freed a prefetch slot — the tail showed the same
// payload thousands of times. Fix: hold deliveries unacked and batch-requeue
// on consumer stop. This test proves exactly M deliveries for M published
// messages while running (no storm), and that stop requeues them.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn write_req(stdin: &mut std::process::ChildStdin, req: &serde_json::Value) {
    stdin
        .write_all(format!("{}\n", req).as_bytes())
        .expect("write request");
    stdin.flush().unwrap();
}

fn next_event(rx: &Receiver<String>, timeout: Duration) -> serde_json::Value {
    match rx.recv_timeout(timeout) {
        Ok(line) => serde_json::from_str(line.trim()).expect("each session line is JSON"),
        Err(_) => panic!("timeout waiting for session event"),
    }
}

/// Read events until the result for `seq` arrives (skipping push messages).
fn wait_result(rx: &Receiver<String>, seq: u64) -> serde_json::Value {
    loop {
        let ev = next_event(rx, Duration::from_secs(10));
        if ev["type"] == "message" {
            continue;
        }
        assert_eq!(ev["type"], "result", "expected result, got: {ev}");
        assert_eq!(ev["seq"].as_u64(), Some(seq), "wrong seq, got: {ev}");
        assert!(ev["status"] == "ok", "request {seq} failed: {ev}");
        return ev;
    }
}

#[test]
fn mq_session_requeue_consumer_delivers_each_message_once() {
    let Ok(url) = std::env::var("MQ_TEST_URL") else {
        return;
    };
    let bin = env!("CARGO_BIN_EXE_poste");
    let queue = format!("poste.mqsession.storm.{}", std::process::id());

    let mut child = Command::new(bin)
        .args(["mq-session", "--connection", &url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn poste mq-session");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        let stderr_text = stderr_text.clone();
        let err_reader = BufReader::new(child.stderr.take().unwrap());
        std::thread::spawn(move || {
            for line in err_reader.lines() {
                match line {
                    Ok(l) => {
                        stderr_text.lock().unwrap().push_str(&l);
                        stderr_text.lock().unwrap().push('\n');
                    }
                    Err(_) => break,
                }
            }
        });
    }
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut seq = 0u64;

    // Declare + publish exactly 3 messages.
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "operation": {"op": "declare", "kind": "queue",
            "name": queue, "durable": false}}),
    );
    assert_eq!(wait_result(&rx, seq)["value"]["kind"], "queue");

    for n in 0..3 {
        seq += 1;
        write_req(
            &mut stdin,
            &serde_json::json!({"seq": seq, "operation": {"op": "publish", "queue": queue,
                "payload": format!("{{\"n\":{}}}", n), "properties": {},
                "headers": {}, "mandatory": true, "confirm": true}}),
        );
        let ev = wait_result(&rx, seq);
        assert_eq!(ev["value"]["routed"], true);
    }

    // Start a requeue-mode (non-destructive) consumer.
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "consumer": "tail", "action": "start",
            "queue": queue, "ack": false}),
    );
    wait_result(&rx, seq);

    // Expect exactly 3 deliveries — the pre-fix forwarder would deliver the
    // same messages thousands of times here.
    let mut seen = 0;
    for _ in 0..3 {
        let ev = next_event(&rx, Duration::from_secs(10));
        assert_eq!(ev["type"], "message", "expected push message, got: {ev}");
        assert_eq!(ev["consumer"], "tail");
        assert_eq!(ev["message"]["routing_key"], queue);
        seen += 1;
    }
    assert_eq!(seen, 3);

    // No storm: a requeued redelivery would arrive immediately; nothing may.
    let quiet = Duration::from_millis(800);
    assert!(
        rx.recv_timeout(quiet).is_err(),
        "requeue-mode consumer redelivered a message it already showed"
    );

    // Stop: the forwarder batch-requeues held deliveries before replying.
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "consumer": "tail", "action": "stop"}),
    );
    wait_result(&rx, seq);
    assert!(rx.recv_timeout(quiet).is_err(), "stray message after stop");

    // Non-destructive preserved depth: all 3 back in the queue, redelivered.
    // Poll: requeue-on-cancel settles in the broker, so retry briefly.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        seq += 1;
        write_req(
            &mut stdin,
            &serde_json::json!({"seq": seq, "operation": {"op": "consume", "queue": queue,
                "count": 10, "ack": false}}),
        );
        let ev = wait_result(&rx, seq);
        let msgs = ev["value"]["messages"].as_array().unwrap().clone();
        if msgs.len() == 3 || std::time::Instant::now() > deadline {
            assert_eq!(msgs.len(), 3, "requeue lost messages");
            for msg in &msgs {
                assert_eq!(msg["redelivered"], true, "requeued message not flagged");
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // Cleanup.
    seq += 1;
    write_req(
        &mut stdin,
        &serde_json::json!({"seq": seq, "operation": {"op": "delete", "kind": "queue",
            "name": queue}}),
    );
    wait_result(&rx, seq);

    drop(stdin);
    // Drain remaining events until the process exits (stdout EOF).
    while rx.recv().is_ok() {}
    let status = child.wait().unwrap();
    let err = stderr_text.lock().unwrap().clone();
    assert!(status.success(), "stderr: {err}");
}
