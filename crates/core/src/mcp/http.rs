//! Streamable HTTP transport for a configured MCP `url`.

use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, ORIGIN};
use reqwest::{Client, StatusCode, Url};
use serde_json::{json, Value};

use super::{
    classify_discover, clip, with_request_meta, EraDecision, ProtocolEra, RequestError,
    CONNECT_TIMEOUT_SECS, DISCOVER_PROBE_TIMEOUT_SECS, LEGACY_PROTOCOL_VERSION, MAX_ERROR_CHARS,
    MAX_RESULT_BYTES, MODERN_PROTOCOL_VERSION,
};
use crate::config::McpServerConfig;
use crate::credentials;

pub struct HttpConnection {
    client: Client,
    url: Url,
    extra_headers: HeaderMap,
    next_id: u64,
    era: ProtocolEra,
    session_id: Option<String>,
}

pub async fn connect(id: &str, config: &McpServerConfig) -> Result<HttpConnection, String> {
    let raw = config
        .http_url()
        .ok_or_else(|| format!("{id} is missing a url"))?;
    let url = Url::parse(raw).map_err(|error| format!("{id} url is invalid: {error}"))?;
    let extra_headers = resolve_headers(config)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("could not build the MCP HTTP client: {error}"))?;
    let mut conn = HttpConnection {
        client,
        url,
        extra_headers,
        next_id: 1,
        era: ProtocolEra::Legacy,
        session_id: None,
    };
    match decide_era(&mut conn).await {
        EraDecision::Modern(version) => {
            conn.era = ProtocolEra::Modern { version };
            Ok(conn)
        }
        EraDecision::Fail(message) => Err(format!("{id}: {message}")),
        EraDecision::Legacy => {
            let handshake = tokio::time::timeout(
                Duration::from_secs(CONNECT_TIMEOUT_SECS),
                initialize(&mut conn),
            )
            .await;
            match handshake {
                Ok(Ok(())) => {
                    conn.era = ProtocolEra::Legacy;
                    Ok(conn)
                }
                Ok(Err(error)) => Err(error.message),
                Err(_) => Err(format!(
                    "{id} did not complete the MCP handshake within {CONNECT_TIMEOUT_SECS}s"
                )),
            }
        }
    }
}

async fn decide_era(conn: &mut HttpConnection) -> EraDecision {
    let probe = tokio::time::timeout(
        Duration::from_secs(DISCOVER_PROBE_TIMEOUT_SECS),
        discover(conn),
    )
    .await;
    match probe {
        Ok((status, outcome)) => classify_http_discover(status, outcome),
        Err(_) => EraDecision::Fail(format!(
            "did not answer server/discover within {DISCOVER_PROBE_TIMEOUT_SECS}s"
        )),
    }
}

fn classify_http_discover(status: u16, outcome: Result<Value, RequestError>) -> EraDecision {
    match outcome {
        Ok(result) => classify_discover(Ok(result)),
        Err(error) if error.is_modern_protocol_error() => classify_discover(Err(error)),
        Err(_) if status == StatusCode::BAD_REQUEST.as_u16() => EraDecision::Legacy,
        Err(error) => EraDecision::Fail(error.message),
    }
}

async fn discover(conn: &mut HttpConnection) -> (u16, Result<Value, RequestError>) {
    rpc(
        conn,
        "server/discover",
        json!({}),
        &[],
        Some(MODERN_PROTOCOL_VERSION),
    )
    .await
}

async fn initialize(conn: &mut HttpConnection) -> Result<(), RequestError> {
    let (_, result) = rpc(
        conn,
        "initialize",
        json!({
            "protocolVersion": LEGACY_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "zest", "version": env!("CARGO_PKG_VERSION") },
        }),
        &[],
        None,
    )
    .await;
    result?;
    let (status, result) = rpc(conn, "notifications/initialized", json!({}), &[], None).await;
    match result {
        Ok(_) => Ok(()),
        Err(error)
            if status == StatusCode::ACCEPTED.as_u16() || error.message.contains("no JSON-RPC") =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub async fn request(
    conn: &mut HttpConnection,
    method: &str,
    params: Value,
    extra_headers: &[(String, String)],
) -> Result<Value, RequestError> {
    let version = match &conn.era {
        ProtocolEra::Modern { version } => Some(version.clone()),
        ProtocolEra::Legacy => None,
    };
    let (_, result) = rpc(conn, method, params, extra_headers, version.as_deref()).await;
    result
}

async fn rpc(
    conn: &mut HttpConnection,
    method: &str,
    params: Value,
    extra_headers: &[(String, String)],
    modern_version: Option<&str>,
) -> (u16, Result<Value, RequestError>) {
    let id = conn.next_id;
    conn.next_id += 1;
    let notification = method.starts_with("notifications/");
    let params = match modern_version {
        Some(version) => with_request_meta(params, version),
        None => params,
    };
    let mut body = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    if !notification {
        body["id"] = json!(id);
    }

    let protocol_version = modern_version.unwrap_or(LEGACY_PROTOCOL_VERSION);
    let mut headers = conn.extra_headers.clone();
    insert_header(&mut headers, ACCEPT, "application/json, text/event-stream");
    insert_header(&mut headers, CONTENT_TYPE, "application/json");
    insert_header(&mut headers, ORIGIN, &origin_for(&conn.url));
    insert_ascii_header(&mut headers, "MCP-Protocol-Version", protocol_version);
    insert_ascii_header(&mut headers, "Mcp-Method", method);
    if method == "tools/call" {
        if let Some(name) = params.get("name").and_then(Value::as_str) {
            insert_ascii_header(&mut headers, "Mcp-Name", &encode_header_value(name));
        }
    }
    if let Some(session) = &conn.session_id {
        insert_ascii_header(&mut headers, "Mcp-Session-Id", session);
    }
    for (name, value) in extra_headers {
        match (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            (Ok(name), Ok(value)) => {
                headers.insert(name, value);
            }
            _ => {
                return (
                    0,
                    Err(RequestError::fatal(format!(
                        "could not send MCP header {name}"
                    ))),
                );
            }
        }
    }

    let sent = conn
        .client
        .post(conn.url.clone())
        .headers(headers)
        .json(&body)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            return (
                0,
                Err(RequestError::transport(format!(
                    "MCP HTTP request failed: {error}"
                ))),
            );
        }
    };
    let status = response.status().as_u16();
    if let Some(session) = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        conn.session_id = Some(session.to_string());
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = match read_body_limited(response).await {
        Ok(bytes) => bytes,
        Err(error) => return (status, Err(error)),
    };

    if notification && (status == StatusCode::ACCEPTED.as_u16() || bytes.is_empty()) {
        return (status, Ok(Value::Null));
    }

    let parsed = if content_type.contains("text/event-stream") {
        take_sse_result(&bytes, id)
    } else {
        take_json_result(&bytes, id)
    };
    (status, map_http_status(status, parsed, method))
}

fn map_http_status(
    status: u16,
    parsed: Result<Value, RequestError>,
    method: &str,
) -> Result<Value, RequestError> {
    if (200..300).contains(&status) {
        return parsed;
    }
    // 400 and modern protocol errors stay typed so era detection can read them.
    // Every other non-2xx is a failed HTTP call, even if the body looks like
    // a JSON-RPC result.
    match parsed {
        Err(error) if error.is_modern_protocol_error() => Err(error),
        Err(error) if status == StatusCode::BAD_REQUEST.as_u16() => Err(error),
        _ => Err(RequestError::fatal(format!(
            "MCP HTTP {status} on {method}"
        ))),
    }
}

/// Read the response while it is still on the wire. `Content-Length` over the
/// cap is refused before any bytes are copied; chunked bodies stop at the same
/// bound so a remote endpoint cannot fill memory.
async fn read_body_limited(response: reqwest::Response) -> Result<Vec<u8>, RequestError> {
    if let Some(len) = response.content_length() {
        if len > MAX_RESULT_BYTES as u64 {
            return Err(RequestError::fatal(format!(
                "MCP HTTP body is {len} bytes; Zest reads at most {MAX_RESULT_BYTES}"
            )));
        }
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| RequestError::transport(format!("MCP HTTP body failed: {error}")))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESULT_BYTES {
            return Err(RequestError::fatal(format!(
                "MCP HTTP body exceeded {MAX_RESULT_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn take_json_result(bytes: &[u8], expected_id: u64) -> Result<Value, RequestError> {
    if bytes.is_empty() {
        return Err(RequestError::fatal("MCP HTTP response was empty"));
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        RequestError::fatal(format!(
            "MCP HTTP response was not JSON: {error}: {}",
            clip(&String::from_utf8_lossy(bytes), 200)
        ))
    })?;
    take_rpc_result(value, expected_id)
}

fn take_sse_result(bytes: &[u8], expected_id: u64) -> Result<Value, RequestError> {
    let text = String::from_utf8_lossy(bytes);
    let mut last_error = RequestError::fatal("MCP SSE stream ended without a JSON-RPC response");
    for event in sse_json_events(&text) {
        match take_rpc_result(event, expected_id) {
            Ok(value) => return Ok(value),
            Err(error) if error.rpc_code.is_some() => return Err(error),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn sse_json_events(body: &str) -> Vec<Value> {
    let mut events = Vec::new();
    let mut data_lines = Vec::new();
    let flush = |data_lines: &mut Vec<String>, events: &mut Vec<Value>| {
        if data_lines.is_empty() {
            return;
        }
        let data = data_lines.join("\n");
        data_lines.clear();
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            events.push(value);
        }
    };
    for line in body.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            flush(&mut data_lines, &mut events);
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
    }
    flush(&mut data_lines, &mut events);
    events
}

fn take_rpc_result(value: Value, expected_id: u64) -> Result<Value, RequestError> {
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        return Err(RequestError::fatal(format!(
            "MCP HTTP stream sent {method} instead of a result"
        )));
    }
    if let Some(id) = value.get("id") {
        if id != &json!(expected_id) && id != &json!(expected_id.to_string()) {
            return Err(RequestError::fatal("MCP HTTP response id did not match"));
        }
    }
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the MCP server rejected the request");
        return Err(RequestError::rpc(
            error.get("code").and_then(Value::as_i64).unwrap_or(0),
            clip(message, MAX_ERROR_CHARS),
            error.get("data").cloned(),
        ));
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

fn resolve_headers(config: &McpServerConfig) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (name, env_name) in &config.headers {
        let value = std::env::var(env_name)
            .map_err(|_| format!("MCP header {name} reads {env_name}, which is not set"))?;
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("MCP header name {name} is invalid"))?;
        let header_value = HeaderValue::from_str(&value)
            .map_err(|_| format!("MCP header {name} from {env_name} is not a valid HTTP value"))?;
        headers.insert(header_name, header_value);
    }
    for (name, account) in &config.header_credentials {
        let value = credentials::get(account)
            .map_err(|error| format!("MCP header {name} credential store unavailable: {error}"))?
            .ok_or_else(|| format!("MCP header {name} has no saved credential"))?;
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("MCP header name {name} is invalid"))?;
        let header_value = HeaderValue::from_str(&value)
            .map_err(|_| format!("MCP header {name} saved credential is not a valid HTTP value"))?;
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}

fn origin_for(url: &Url) -> String {
    let host = url.host_str().unwrap_or("localhost");
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

fn insert_header(headers: &mut HeaderMap, name: reqwest::header::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

fn insert_ascii_header(headers: &mut HeaderMap, name: &str, value: &str) {
    if let (Ok(name), Ok(value)) = (
        HeaderName::from_bytes(name.as_bytes()),
        HeaderValue::from_str(value),
    ) {
        headers.insert(name, value);
    }
}

/// Encode a header value the way 2026-07-28 requires.
pub fn encode_header_value(value: &str) -> String {
    let needs_encode = value.starts_with("=?base64?") && value.ends_with("?=")
        || value.starts_with(' ')
        || value.ends_with(' ')
        || value.contains('\t')
        || !value
            .chars()
            .all(|c| matches!(c as u32, 0x20 | 0x09 | 0x21..=0x7E));
    if needs_encode {
        format!(
            "=?base64?{}?=",
            base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
        )
    } else {
        value.to_string()
    }
}

/// `x-mcp-header` annotations that Streamable HTTP must mirror.
pub fn param_headers(schema: &Value, arguments: &Value) -> Result<Vec<(String, String)>, String> {
    let mut headers = Vec::new();
    for annotation in header_annotations(schema)? {
        let Some(value) = value_at(arguments, &annotation.path) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let encoded = match annotation.kind.as_str() {
            "string" => value
                .as_str()
                .ok_or_else(|| format!("MCP header {} needs a string", annotation.header))?
                .to_string(),
            "integer" => {
                let number = value
                    .as_i64()
                    .ok_or_else(|| format!("MCP header {} needs an integer", annotation.header))?;
                if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&number) {
                    return Err(format!(
                        "MCP header {} is outside the integer range HTTP can carry",
                        annotation.header
                    ));
                }
                number.to_string()
            }
            "boolean" => value
                .as_bool()
                .ok_or_else(|| format!("MCP header {} needs a boolean", annotation.header))?
                .to_string(),
            other => {
                return Err(format!(
                    "MCP header {} cannot use type {other}",
                    annotation.header
                ));
            }
        };
        headers.push((
            format!("Mcp-Param-{}", annotation.header),
            encode_header_value(&encoded),
        ));
    }
    Ok(headers)
}

pub fn http_tool_definition_ok(schema: &Value) -> Result<(), String> {
    header_annotations(schema).map(|_| ())
}

struct HeaderAnnotation {
    path: Vec<String>,
    header: String,
    kind: String,
}

fn header_annotations(schema: &Value) -> Result<Vec<HeaderAnnotation>, String> {
    let mut found = Vec::new();
    walk_properties(schema, Vec::new(), &mut found)?;
    let mut seen = Vec::new();
    for item in &found {
        let lower = item.header.to_ascii_lowercase();
        if seen.iter().any(|existing: &String| existing == &lower) {
            return Err(format!("duplicate x-mcp-header `{}`", item.header));
        }
        seen.push(lower);
    }
    Ok(found)
}

fn walk_properties(
    schema: &Value,
    path: Vec<String>,
    out: &mut Vec<HeaderAnnotation>,
) -> Result<(), String> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(());
    };
    for (name, property) in properties {
        let mut next = path.clone();
        next.push(name.clone());
        if let Some(header) = property.get("x-mcp-header").and_then(Value::as_str) {
            if header.is_empty() || !is_tchar(header) {
                return Err(format!("invalid x-mcp-header `{header}`"));
            }
            let kind = property.get("type").and_then(Value::as_str).unwrap_or("");
            if !matches!(kind, "string" | "integer" | "boolean") {
                return Err(format!(
                    "x-mcp-header `{header}` must be string, integer, or boolean"
                ));
            }
            out.push(HeaderAnnotation {
                path: next.clone(),
                header: header.to_string(),
                kind: kind.to_string(),
            });
        }
        walk_properties(property, next, out)?;
    }
    Ok(())
}

fn is_tchar(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn value_at<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = root;
    for key in path {
        current = current.get(key)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn plain_ascii_stays_unencoded() {
        assert_eq!(encode_header_value("us-west1"), "us-west1");
    }

    #[test]
    fn non_ascii_uses_the_base64_sentinel() {
        assert_eq!(
            encode_header_value("Hello, 世界"),
            "=?base64?SGVsbG8sIOS4lueVjA==?="
        );
    }

    #[test]
    fn a_literal_sentinel_is_encoded_again() {
        assert_eq!(
            encode_header_value("=?base64?literal?="),
            "=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?="
        );
    }

    #[test]
    fn sse_events_yield_the_json_rpc_result() {
        let body =
            ": keep-alive\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let events = sse_json_events(body);
        assert_eq!(events[0]["result"]["ok"], true);
        assert_eq!(
            take_rpc_result(events[0].clone(), 1).unwrap(),
            json!({ "ok": true })
        );
    }

    #[test]
    fn param_headers_follow_the_schema_path() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": { "type": "string", "x-mcp-header": "Region" },
                "query": { "type": "string" }
            }
        });
        let headers = param_headers(
            &schema,
            &json!({ "region": "us-west1", "query": "select 1" }),
        )
        .unwrap();
        assert_eq!(
            headers,
            vec![("Mcp-Param-Region".into(), "us-west1".into())]
        );
    }

    #[test]
    fn a_number_type_header_is_rejected() {
        let schema = json!({
            "type": "object",
            "properties": {
                "n": { "type": "number", "x-mcp-header": "N" }
            }
        });
        assert!(http_tool_definition_ok(&schema).is_err());
    }

    #[test]
    fn http_400_without_a_modern_error_is_legacy() {
        assert_eq!(
            classify_http_discover(400, Err(RequestError::fatal("nope"))),
            EraDecision::Legacy
        );
    }

    #[test]
    fn http_404_is_not_silently_legacy() {
        match classify_http_discover(404, Err(RequestError::fatal("missing"))) {
            EraDecision::Fail(message) => assert!(message.contains("missing"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_json_rpc_result_on_401_is_still_an_http_failure() {
        let error = map_http_status(401, Ok(json!({ "ok": true })), "tools/list").unwrap_err();
        assert!(error.message.contains("401"), "{}", error.message);
    }

    #[test]
    fn a_json_rpc_result_on_500_is_still_an_http_failure() {
        let error = map_http_status(500, Ok(json!({ "ok": true })), "tools/call").unwrap_err();
        assert!(error.message.contains("500"), "{}", error.message);
    }

    #[test]
    fn a_2xx_json_rpc_result_is_accepted() {
        assert_eq!(
            map_http_status(200, Ok(json!({ "ok": true })), "tools/list").unwrap(),
            json!({ "ok": true })
        );
    }

    #[tokio::test]
    async fn a_modern_http_server_lists_and_calls_without_initialize() {
        let state = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = state.clone();
        let (url, task) = spawn_http_fixture(move |req| {
            seen.lock().unwrap().push(req.method.clone());
            if req.method == "initialize" {
                return http_response(500, "application/json", "initialize must not run");
            }
            if req.method == "server/discover" {
                return json_rpc(
                    req.id,
                    json!({
                        "resultType": "complete",
                        "supportedVersions": ["2026-07-28"],
                        "capabilities": { "tools": {} }
                    }),
                );
            }
            if req.meta_version.as_deref() != Some("2026-07-28") {
                return json_rpc_error(req.id, -32602, "missing modern _meta");
            }
            if req.method == "tools/list" {
                return json_rpc(
                    req.id,
                    json!({
                        "resultType": "complete",
                        "tools": [{
                            "name": "echo",
                            "description": "modern-http",
                            "inputSchema": { "type": "object" }
                        }]
                    }),
                );
            }
            if req.method == "tools/call" {
                return sse_rpc(
                    req.id,
                    json!({ "content": [{ "type": "text", "text": "http success" }] }),
                );
            }
            json_rpc_error(req.id, -32601, "unknown")
        })
        .await;

        let server = crate::mcp::McpServer::new(
            "remote",
            McpServerConfig {
                command: String::new(),
                args: Vec::new(),
                url: Some(url),
                headers: Default::default(),
                header_credentials: Default::default(),
                env_vars: Vec::new(),
                enabled: true,
                timeout_secs: 5,
            },
            ".",
        );
        let tools = server.list_tools().await.unwrap();
        assert_eq!(tools[0].name, "echo");
        assert_eq!(
            server
                .call_tool("echo", json!({}), &json!({}))
                .await
                .unwrap(),
            "http success"
        );
        assert!(!state.lock().unwrap().iter().any(|m| m == "initialize"));
        task.abort();
    }

    #[tokio::test]
    async fn an_oversized_http_body_is_refused_before_parse() {
        let (url, task) = spawn_http_fixture(|req| {
            if req.method == "server/discover" {
                return json_rpc(
                    req.id,
                    json!({
                        "resultType": "complete",
                        "supportedVersions": ["2026-07-28"],
                        "capabilities": { "tools": {} }
                    }),
                );
            }
            http_response(200, "application/json", &"x".repeat(MAX_RESULT_BYTES + 1))
        })
        .await;

        let server = crate::mcp::McpServer::new("remote", http_config(url), ".");
        let error = server.list_tools().await.expect_err("body over the cap");
        assert!(error.contains(&MAX_RESULT_BYTES.to_string()), "{error}");
        task.abort();
    }

    #[tokio::test]
    async fn a_401_with_a_json_rpc_result_is_rejected() {
        let (url, task) = spawn_http_fixture(|req| {
            if req.method == "server/discover" {
                return json_rpc(
                    req.id,
                    json!({
                        "resultType": "complete",
                        "supportedVersions": ["2026-07-28"],
                        "capabilities": { "tools": {} }
                    }),
                );
            }
            let (_, content_type, body) = json_rpc(req.id, json!({ "tools": [] }));
            (401, content_type, body)
        })
        .await;

        let server = crate::mcp::McpServer::new("remote", http_config(url), ".");
        let error = server.list_tools().await.expect_err("401 is not success");
        assert!(error.contains("401"), "{error}");
        task.abort();
    }

    fn http_config(url: String) -> McpServerConfig {
        McpServerConfig {
            command: String::new(),
            args: Vec::new(),
            url: Some(url),
            headers: Default::default(),
            header_credentials: Default::default(),
            env_vars: Vec::new(),
            enabled: true,
            timeout_secs: 5,
        }
    }

    #[test]
    fn a_missing_saved_header_credential_is_reported_without_its_account() {
        let mut config = http_config("https://example.com/mcp".into());
        config.header_credentials.insert(
            "Authorization".into(),
            "mcp-header:missing-test-account".into(),
        );
        let error = resolve_headers(&config).expect_err("missing credentials must fail closed");
        assert!(error.contains("Authorization"), "{error}");
        assert!(!error.contains("missing-test-account"), "{error}");
    }

    struct FixtureRequest {
        method: String,
        id: Value,
        meta_version: Option<String>,
    }

    fn json_rpc(id: Value, result: Value) -> (u16, String, String) {
        (
            200,
            "application/json".into(),
            json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        )
    }

    fn json_rpc_error(id: Value, code: i64, message: &str) -> (u16, String, String) {
        (
            400,
            "application/json".into(),
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message }
            })
            .to_string(),
        )
    }

    fn sse_rpc(id: Value, result: Value) -> (u16, String, String) {
        let payload = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        (
            200,
            "text/event-stream".into(),
            format!("data: {payload}\n\n"),
        )
    }

    fn http_response(status: u16, content_type: &str, body: &str) -> (u16, String, String) {
        (status, content_type.into(), body.into())
    }

    async fn spawn_http_fixture<F>(handler: F) -> (String, tokio::task::JoinHandle<()>)
    where
        F: Fn(FixtureRequest) -> (u16, String, String) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let Some(req) = read_fixture_request(&mut stream).await else {
                        return;
                    };
                    let (status, content_type, body) = handler(req);
                    let response = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}/mcp"), task)
    }

    async fn read_fixture_request(stream: &mut tokio::net::TcpStream) -> Option<FixtureRequest> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                if let Some(len) = content_length(&buf) {
                    let split = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
                    if buf.len() >= split + 4 + len {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
        let value: Value = serde_json::from_str(body.trim_end_matches('\0')).ok()?;
        Some(FixtureRequest {
            method: value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            id: value.get("id").cloned().unwrap_or(Value::Null),
            meta_version: value
                .get("params")
                .and_then(|params| params.get("_meta"))
                .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    fn content_length(buf: &[u8]) -> Option<usize> {
        let text = String::from_utf8_lossy(buf);
        for line in text.split("\r\n") {
            if let Some(rest) = line
                .split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim())
            {
                return rest.parse().ok();
            }
        }
        None
    }
}
