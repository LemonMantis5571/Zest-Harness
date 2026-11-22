use std::env;
use std::io::{self, BufRead, Write};

fn send(line: &str) {
    println!("{line}");
    io::stdout().flush().expect("flush fixture response");
}

fn receive(reader: &mut impl BufRead) -> String {
    let mut line = String::new();
    assert!(reader.read_line(&mut line).expect("read fixture request") > 0);
    line
}

fn headless() {
    send(r#"{"type":"result","response":"worker ok"}"#);
}

fn delegation(prompt: &str) {
    if prompt.contains("independent Zest reviewer") {
        send(r#"{"type":"result","response":"{\"decision\":\"accepted\",\"summary\":\"fixture review passed\",\"findings\":[],\"checks\":[]}"}"#);
        return;
    }

    std::fs::write("delegated.txt", "fixture worker change\n").expect("write delegation fixture change");
    send(r#"{"type":"result","response":"{\"summary\":\"fixture worker changed delegated.txt\",\"changedFiles\":[\"delegated.txt\"],\"checksAttempted\":[],\"blockers\":[]}"}"#);
}

/// Like `delegation`, but the first `failures` reviewer runs answer prose
/// instead of a JSON verdict. `counter` is a file outside the worktrees that
/// counts reviewer runs across processes.
fn delegation_flaky_reviewer(counter: &str, failures: &str, prompt: &str) {
    if prompt.contains("independent Zest reviewer") {
        let runs = std::fs::read_to_string(counter)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        std::fs::write(counter, runs.to_string()).expect("count reviewer runs");
        if runs <= failures.parse::<u32>().expect("failure count") {
            send(r#"{"type":"result","response":"I looked at it and it seems fine overall."}"#);
            return;
        }
    }
    delegation(prompt);
}

/// A worker that adds a binary file, or with `large` a file over the
/// delegated-diff file limit.
fn delegation_file(kind: &str, prompt: &str) {
    if prompt.contains("independent Zest reviewer") {
        delegation(prompt);
        return;
    }
    match kind {
        "large" => std::fs::write("large.txt", vec![b'a'; 2_100_000]),
        _ => std::fs::write("image.bin", (0..=255u8).cycle().take(4096).collect::<Vec<u8>>()),
    }
    .expect("write fixture file");
    send(r#"{"type":"result","response":"{\"summary\":\"fixture worker added a file\",\"changedFiles\":[],\"checksAttempted\":[],\"blockers\":[]}"}"#);
}

/// A worker that rewrites line 7 of the project's `notes.txt`.
fn delegation_line7(prompt: &str) {
    if prompt.contains("independent Zest reviewer") {
        delegation(prompt);
        return;
    }
    let notes = std::fs::read_to_string("notes.txt").expect("read notes.txt");
    let mut lines: Vec<String> = notes.lines().map(str::to_string).collect();
    lines[6] = "worker line 7".into();
    std::fs::write("notes.txt", lines.join("
") + "
").expect("write notes.txt");
    send(r#"{"type":"result","response":"{\"summary\":\"fixture worker edited notes.txt\",\"changedFiles\":[\"notes.txt\"],\"checksAttempted\":[],\"blockers\":[]}"}"#);
}

/// A worker that starts a long-lived child of its own, then goes silent.
fn spawn_grandchild(pidfile: &str) {
    std::process::Command::new(env::current_exe().expect("fixture path"))
        .args(["sleeper", pidfile])
        .spawn()
        .expect("start the grandchild");
    std::thread::sleep(std::time::Duration::from_secs(300));
}

/// Record this process id, then sleep.
fn sleeper(pidfile: &str) {
    std::fs::write(pidfile, std::process::id().to_string()).expect("write pid file");
    std::thread::sleep(std::time::Duration::from_secs(300));
}

/// A worker that crashes with an error that mentions a connection.
fn delegation_crash() {
    send(r#"{"type":"result","is_error":true,"result":"Connection reset by peer; the service is unavailable"}"#);
    std::process::exit(3);
}

/// A worker that edits a file, then goes silent and never finishes.
fn delegation_hang() {
    std::fs::write("partial.txt", "unfinished work
").expect("write partial fixture change");
    std::thread::sleep(std::time::Duration::from_secs(300));
}

fn stream() {
    send(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}}"#);
    send(r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"tool-1","name":"Read"}}}"#);
    send(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1"}]}}"#);
    send(r#"{"type":"result","response":"hello"}"#);
}

fn fail_stdout() {
    send(r#"{"type":"result","is_error":true,"result":"You've hit your usage limit. Try again later."}"#);
    std::process::exit(1);
}

fn wait_for_eof() {
    send(r#"{"type":"result","response":"finished"}"#);
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
}

/// Mimic Claude `--input-format stream-json`: emit initialize, then wait for
/// an initialize ack and a JSON user message before producing a result.
fn stream_json_input() {
    send(r#"{"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}"#);
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut saw_user = false;
    let mut saw_init_ack = false;
    while !saw_user || !saw_init_ack {
        let line = receive(&mut reader);
        if line.contains("control_response") && line.contains("init-1") {
            saw_init_ack = true;
        }
        if line.contains("\"type\":\"user\"") || line.contains("\"type\": \"user\"") {
            saw_user = true;
        }
    }
    send(r#"{"type":"result","response":"got user"}"#);
}

/// Ask permission, wait for the host reply, then finish. Used to prove that
/// time spent in `respond()` is not counted against `timeout_secs`.
fn ask_then_result() {
    send(r#"{"type":"control_request","request_id":"tool-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hi"}}}"#);
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let line = receive(&mut reader);
    assert!(
        line.contains("control_response"),
        "expected a permission reply, got {line}"
    );
    send(r#"{"type":"result","response":"ran"}"#);
}

fn acp() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    let _ = receive(&mut reader);
    send(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#);

    let _ = receive(&mut reader);
    send(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"smoke-session"}}"#);

    let _ = receive(&mut reader);
    send(r#"{"jsonrpc":"2.0","id":10,"method":"fs/read_text_file","params":{"path":"input.txt"}}"#);
    assert!(receive(&mut reader).contains("source"));

    send(r#"{"jsonrpc":"2.0","id":11,"method":"fs/write_text_file","params":{"path":"output.txt","content":"created"}}"#);
    assert!(receive(&mut reader).contains(r#""id":11"#));

    send(r#"{"jsonrpc":"2.0","id":12,"method":"terminal/create","params":{"command":"rustc","args":["--version"],"cwd":".","outputByteLimit":4096}}"#);
    assert!(receive(&mut reader).contains("terminalId"));

    send(r#"{"jsonrpc":"2.0","id":13,"method":"terminal/wait_for_exit","params":{"terminalId":"zest-terminal-1"}}"#);
    assert!(receive(&mut reader).contains(r#""id":13"#));

    send(r#"{"jsonrpc":"2.0","id":14,"method":"terminal/output","params":{"terminalId":"zest-terminal-1"}}"#);
    assert!(receive(&mut reader).contains("rustc"));

    send(r#"{"jsonrpc":"2.0","id":4,"method":"session/request_permission","params":{"options":[{"optionId":"reject-once","kind":"reject_once"}]}}"#);
    assert!(receive(&mut reader).contains("cancelled"));

    send(r#"{"jsonrpc":"2.0","id":5,"method":"session/request_permission","params":{"options":[{"optionId":"allow-once","kind":"allow_once"}]}}"#);
    assert!(receive(&mut reader).contains("allow-once"));

    send(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"smoke-session","update":{"sessionUpdate":"usage_update","used":22,"size":1000,"cost":{"amount":0.02,"currency":"USD"}}}}"#);
    send(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"smoke-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"acp ok"}}}}"#);
    send(r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#);
}

fn main() {
    let mut args = env::args();
    match args.nth(1).as_deref() {
        Some("headless") => headless(),
        Some("stream") => stream(),
        Some("fail_stdout") => fail_stdout(),
        Some("wait_for_eof") => wait_for_eof(),
        Some("stream_json_input") => stream_json_input(),
        Some("ask_then_result") => ask_then_result(),
        Some("acp") => acp(),
        Some("delegation") => delegation(&args.next().unwrap_or_default()),
        Some("delegation_hang") => delegation_hang(),
        Some("delegation_crash") => delegation_crash(),
        Some("spawn_grandchild") => spawn_grandchild(&args.next().expect("pid file")),
        Some("sleeper") => sleeper(&args.next().expect("pid file")),
        Some("delegation_line7") => delegation_line7(&args.next().unwrap_or_default()),
        Some("delegation_file") => {
            let kind = args.next().expect("file kind");
            delegation_file(&kind, &args.next().unwrap_or_default())
        }
        Some("delegation_flaky_reviewer") => {
            let counter = args.next().expect("counter file");
            let failures = args.next().expect("failure count");
            delegation_flaky_reviewer(&counter, &failures, &args.next().unwrap_or_default())
        }
        other => panic!("unknown external-agent fixture mode: {other:?}"),
    }
}
