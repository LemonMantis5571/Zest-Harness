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

fn stream() {
    send(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}}"#);
    send(r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"tool-1","name":"Read"}}}"#);
    send(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1"}]}}"#);
    send(r#"{"type":"result","response":"hello"}"#);
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
        Some("wait_for_eof") => wait_for_eof(),
        Some("stream_json_input") => stream_json_input(),
        Some("acp") => acp(),
        Some("delegation") => delegation(&args.next().unwrap_or_default()),
        other => panic!("unknown external-agent fixture mode: {other:?}"),
    }
}
