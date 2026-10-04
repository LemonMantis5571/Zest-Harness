//! Exercise the real JSONL binary against a local streaming provider fixture.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn provider_fixture(
    responses: Vec<(u16, String)>,
) -> (String, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let deadline = Instant::now() + Duration::from_secs(30);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "provider fixture was not called");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("provider fixture accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut length = None;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
            }
            let mut bytes = vec![0; length.expect("request content length")];
            reader.read_exact(&mut bytes).unwrap();
            requests.push(serde_json::from_slice(&bytes).unwrap());
            let content_type = if status == 200 {
                "text/event-stream"
            } else {
                "application/json"
            };
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(response.as_bytes()).unwrap();
        }
        requests
    });
    (url, server)
}

fn streamed(delta: Value, finish: &str) -> String {
    [
        format!("data: {}\n\n", json!({"model":"fixture-model", "choices":[{"delta":delta, "finish_reason":null}]})),
        format!("data: {}\n\n", json!({"model":"fixture-model", "choices":[{"delta":{}, "finish_reason":finish}]})),
        format!("data: {}\n\n", json!({"model":"fixture-model", "choices":[], "usage":{"prompt_tokens":40, "completion_tokens":5}})),
        "data: [DONE]\n\n".into(),
    ].concat()
}

fn write_config(root: &Path, url: &str) {
    std::fs::write(root.join("zest.toml"), format!(
        "[usage]\ntask_traces = true\n\n[providers.fixture]\nkind = \"openai_compatible\"\nbase_url = \"{url}\"\nmodel = \"fixture-model\"\n"
    )).unwrap();
    std::fs::write(root.join("README.md"), "PRIVATE_TOOL_RESULT\n").unwrap();
}

fn run_fixture(root: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_zest"))
        .args([
            "run",
            "--jsonl",
            "--provider",
            "fixture",
            "--usage-file",
            "usage.json",
            "--",
            "PRIVATE_PROMPT",
        ])
        .env_remove("ZEST_MODEL")
        .env_remove("ZEST_EFFORT")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(40);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "JSONL fixture timed out: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

fn events(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("invalid JSONL: {line}: {error}"))
        })
        .collect()
}

#[test]
fn jsonl_run_identity_covers_tool_rounds_denials_and_saved_usage() {
    let tools = streamed(
        json!({"tool_calls":[
            {"index":0,"id":"read-1","type":"function","function":{"name":"read_file","arguments":r#"{"path":"README.md"}"#}},
            {"index":1,"id":"write-1","type":"function","function":{"name":"write_file","arguments":r#"{"path":"denied.txt","content":"must not be written"}"#}}
        ]}),
        "tool_calls",
    );
    let answer = streamed(json!({"content":"The write was denied."}), "stop");
    let (url, server) = provider_fixture(vec![
        (200, tools.clone()),
        (200, answer.clone()),
        (200, tools),
        (200, answer),
    ]);
    let temp = tempfile::tempdir().unwrap();
    write_config(temp.path(), &url);
    let mut run_ids = Vec::new();
    for _ in 0..2 {
        let output = run_fixture(temp.path());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events = events(&output);
        assert_eq!(events.first().unwrap()["kind"], "session");
        assert_eq!(events.last().unwrap()["kind"], "done");
        let run_id = events[0]["run_id"].as_str().unwrap();
        assert!(run_id.starts_with("run-"));
        assert!(events.iter().all(|event| event["run_id"] == run_id));
        for kind in [
            "text",
            "tool_call_start",
            "tool_call_result",
            "approval_needed",
            "approval_decision",
        ] {
            assert!(
                events.iter().any(|event| event["kind"] == kind),
                "missing {kind}: {events:?}"
            );
        }
        assert!(events
            .iter()
            .any(|event| event["kind"] == "tool_call_result"
                && event["name"] == "read_file"
                && event["isError"] == false));
        assert!(events
            .iter()
            .any(|event| event["kind"] == "tool_call_result"
                && event["name"] == "write_file"
                && event["isError"] == true));
        assert!(!temp.path().join("denied.txt").exists());
        run_ids.push(run_id.to_string());
    }
    assert_ne!(run_ids[0], run_ids[1]);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "tool" && message["tool_call_id"] == "read-1"));
    let ledger = zest_core::Ledger::load_from(temp.path().join("usage.json"));
    assert_eq!(ledger.tasks().len(), 2);
    for (task, run_id) in ledger.tasks().iter().zip(&run_ids) {
        assert_eq!(task.run_id.as_ref(), Some(run_id));
        assert_eq!(task.status, "completed");
        assert_eq!(task.requests.len(), 2);
        assert_eq!(task.tools.len(), 2);
        assert!(task.requests.iter().all(|request| request.usage_available));
    }
    let trace = std::fs::read_to_string(temp.path().join("usage.json")).unwrap();
    assert!(!trace.contains("PRIVATE_PROMPT"));
    assert!(!trace.contains("PRIVATE_TOOL_RESULT"));
    assert!(!trace.contains(&temp.path().to_string_lossy().to_string()));
}

#[test]
fn jsonl_failure_keeps_run_identity_and_persists_the_failed_task() {
    let (url, server) = provider_fixture(vec![(
        400,
        json!({"error":{"message":"fixture request rejected", "type":"invalid_request_error"}})
            .to_string(),
    )]);
    let temp = tempfile::tempdir().unwrap();
    write_config(temp.path(), &url);
    let output = run_fixture(temp.path());
    assert!(!output.status.success());
    let events = events(&output);
    assert_eq!(events.first().unwrap()["kind"], "session");
    assert_eq!(events.last().unwrap()["kind"], "error");
    let run_id = events[0]["run_id"].as_str().unwrap();
    assert!(events.iter().all(|event| event["run_id"] == run_id));
    assert_eq!(server.join().unwrap().len(), 1);
    let ledger = zest_core::Ledger::load_from(temp.path().join("usage.json"));
    assert_eq!(ledger.tasks().len(), 1);
    assert_eq!(ledger.tasks()[0].run_id.as_deref(), Some(run_id));
    assert_eq!(ledger.tasks()[0].status, "failed");
    assert_eq!(ledger.tasks()[0].requests.len(), 1);
    assert!(!ledger.tasks()[0].requests[0].usage_available);
}
