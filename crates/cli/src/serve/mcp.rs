use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use zest_coordinator::{
    CreateDelegationJobRequest, DelegationCoordinator, DelegationStatus,
    UpdateDelegationJobRequest, ALLOWED_ARTIFACTS, INBOUND_MCP_ORIGIN,
};
use zest_core::{LEGACY_MCP_PROTOCOL_VERSION, MODERN_MCP_PROTOCOL_VERSION};

use super::ServePolicy;

const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone)]
struct AppState {
    token: Arc<str>,
    coordinator: Arc<DelegationCoordinator>,
    root: PathBuf,
    policy: ServePolicy,
}

pub fn router(
    token: String,
    coordinator: Arc<DelegationCoordinator>,
    root: PathBuf,
    policy: ServePolicy,
) -> Router {
    let state = AppState {
        token: Arc::from(token),
        coordinator,
        root,
        policy,
    };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/mcp", post(mcp_post))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(json!({"ok": true})))
}

async fn mcp_post(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = authorize(&state, &headers) {
        return response;
    }
    if let Some(response) = check_origin(&headers) {
        return response;
    }
    if body.len() > MAX_REQUEST_BYTES {
        return json_error(
            None,
            -32600,
            "request too large",
            StatusCode::PAYLOAD_TOO_LARGE,
        );
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return json_error(None, -32700, "parse error", StatusCode::BAD_REQUEST),
    };
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = payload.get("id").cloned();
    let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
    if method.starts_with("notifications/") {
        if method == "notifications/initialized" {
            return StatusCode::ACCEPTED.into_response();
        }
        return StatusCode::ACCEPTED.into_response();
    }
    let result = match method {
        "server/discover" => Ok(json!({
            "resultType": "complete",
            "supportedVersions": [MODERN_MCP_PROTOCOL_VERSION],
            "capabilities": { "tools": {} }
        })),
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .filter(|version| {
                    *version == MODERN_MCP_PROTOCOL_VERSION
                        || *version == LEGACY_MCP_PROTOCOL_VERSION
                })
                .unwrap_or(MODERN_MCP_PROTOCOL_VERSION),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "zest-serve",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_defs(state.policy) })),
        "tools/call" => call_tool(&state, &params),
        _ => Err((
            -32601,
            format!("method `{method}` is not implemented"),
            StatusCode::OK,
        )),
    };
    match result {
        Ok(value) => json_result(id, value),
        Err((code, message, status)) => json_error(id, code, &message, status),
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let Some(header) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    };
    let Some(provided) = header.strip_prefix("Bearer ") else {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    };
    if provided != state.token.as_ref() {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    }
    None
}

fn check_origin(headers: &HeaderMap) -> Option<Response> {
    let origin = headers.get(axum::http::header::ORIGIN)?;
    let Ok(origin) = origin.to_str() else {
        return Some(StatusCode::FORBIDDEN.into_response());
    };
    if origin_allowed(origin) {
        None
    } else {
        Some(StatusCode::FORBIDDEN.into_response())
    }
}

fn origin_allowed(origin: &str) -> bool {
    let origin = origin.trim();
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.rsplit_once('@').map(|(_, host)| host).unwrap_or(host);
    let host = host
        .rsplit_once(':')
        .filter(|(name, port)| !name.contains(']') && port.chars().all(|ch| ch.is_ascii_digit()))
        .map(|(name, _)| name)
        .unwrap_or(host);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

fn call_tool(state: &AppState, params: &Value) -> Result<Value, (i64, String, StatusCode)> {
    let name = params.get("name").and_then(Value::as_str).ok_or((
        -32602,
        "tools/call requires name".into(),
        StatusCode::OK,
    ))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let payload = match name {
        "delegation_targets" => {
            let targets = DelegationCoordinator::list_targets(&state.root).map_err(internal)?;
            json!({ "targets": targets })
        }
        "delegation_create" => {
            let mut request: CreateDelegationJobRequest = serde_json::from_value(arguments)
                .map_err(|error| {
                    (
                        -32602,
                        format!("invalid delegation_create arguments: {error}"),
                        StatusCode::OK,
                    )
                })?;
            if request
                .idempotency_key
                .as_deref()
                .map(str::trim)
                .is_none_or(|value| value.is_empty())
            {
                return Err((
                    -32602,
                    "delegation_create requires idempotencyKey".into(),
                    StatusCode::OK,
                ));
            }
            if request.origin_coordinator.is_none() {
                request.origin_coordinator = Some(INBOUND_MCP_ORIGIN.into());
            }
            let created = state
                .coordinator
                .create_job(&state.root, request)
                .map_err(tool_err)?;
            if state.policy.is_trusted() && created.status == DelegationStatus::AwaitingApproval {
                json!(state
                    .coordinator
                    .approve(&state.root, &created.job_id, None)
                    .map_err(tool_err)?)
            } else {
                json!(created)
            }
        }
        "delegation_list" => json!(zest_coordinator::list_views(&state.root).map_err(tool_err)?),
        "delegation_get" => {
            let job_id = required_str(&arguments, "jobId")?;
            json!(zest_coordinator::get_view(&state.root, &job_id).map_err(tool_err)?)
        }
        "delegation_artifact" => {
            let job_id = required_str(&arguments, "jobId")?;
            let name = required_str(&arguments, "name")?;
            if !ALLOWED_ARTIFACTS.contains(&name.as_str()) {
                return Err((
                    -32602,
                    format!("artifact `{name}` is not readable"),
                    StatusCode::OK,
                ));
            }
            let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
            json!(state
                .coordinator
                .artifact_page(&state.root, &job_id, &name, offset)
                .map_err(tool_err)?)
        }
        "delegation_update" => {
            let request: UpdateDelegationJobRequest =
                serde_json::from_value(arguments).map_err(|error| {
                    (
                        -32602,
                        format!("invalid delegation_update arguments: {error}"),
                        StatusCode::OK,
                    )
                })?;
            json!(state
                .coordinator
                .update_job(&state.root, request)
                .map_err(tool_err)?)
        }
        "delegation_approve" => {
            let job_id = required_str(&arguments, "jobId")?;
            let expected = optional_u64(&arguments, "expectedUpdatedAt");
            json!(state
                .coordinator
                .approve(&state.root, &job_id, expected)
                .map_err(tool_err)?)
        }
        "delegation_retry" => {
            let job_id = required_str(&arguments, "jobId")?;
            let expected = optional_u64(&arguments, "expectedUpdatedAt");
            json!(state
                .coordinator
                .retry(&state.root, &job_id, expected)
                .map_err(tool_err)?)
        }
        "delegation_cancel" => {
            let job_id = required_str(&arguments, "jobId")?;
            let expected = optional_u64(&arguments, "expectedUpdatedAt");
            json!(state
                .coordinator
                .cancel(&state.root, &job_id, expected)
                .map_err(tool_err)?)
        }
        "delegation_apply" => {
            let job_id = required_str(&arguments, "jobId")?;
            let expected = optional_u64(&arguments, "expectedUpdatedAt");
            json!(state
                .coordinator
                .apply(&state.root, &job_id, expected)
                .map_err(tool_err)?)
        }
        other => return Err((-32601, format!("unknown tool `{other}`"), StatusCode::OK)),
    };
    Ok(tool_content(payload))
}

fn tool_content(payload: Value) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    let text = if text.len() > MAX_RESPONSE_BYTES {
        format!("{}\n…truncated", &text[..MAX_RESPONSE_BYTES])
    } else {
        text
    };
    json!({
        "content": [{ "type": "text", "text": text }]
    })
}

fn tool_defs(policy: ServePolicy) -> Vec<Value> {
    let create_description = if policy.is_trusted() {
        "Create a feature card. This daemon is trusted: the worker starts immediately, and a passing review is applied without calling delegation_approve or delegation_apply."
    } else {
        "Create a feature card. Stays awaiting_approval until delegation_approve."
    };
    vec![
        tool("delegation_targets", "List worker and reviewer targets for this project.", json!({"type":"object"})),
        tool(
            "delegation_create",
            create_description,
            json!({
                "type": "object",
                "required": ["idempotencyKey", "parentThreadId", "title", "objective", "lane", "scope", "worker"],
                "properties": {
                    "idempotencyKey": {"type": "string"},
                    "parentThreadId": {"type": "string"},
                    "title": {"type": "string"},
                    "objective": {"type": "string"},
                    "lane": {"type": "string"},
                    "scope": {"type": "array", "items": {"type": "string"}},
                    "context": {"type": "array", "items": {"type": "string"}},
                    "dependsOn": {"type": "array", "items": {"type": "string"}},
                    "acceptanceChecks": {"type": "array", "items": {"type": "string"}},
                    "worker": {"type": "object"},
                    "reviewer": {"type": "object"},
                    "chatId": {"type": "string"},
                    "originCoordinator": {"type": "string"}
                }
            }),
        ),
        tool(
            "delegation_list",
            "List feature cards for this project.",
            json!({"type":"object"}),
        ),
        tool(
            "delegation_get",
            "Get one feature card by jobId.",
            object_with(&["jobId"]),
        ),
        tool(
            "delegation_artifact",
            "Read a paged worker.diff, worker-result.json, or review-result.json artifact.",
            json!({
                "type": "object",
                "required": ["jobId", "name"],
                "properties": {
                    "jobId": {"type": "string"},
                    "name": {"type": "string", "enum": ["worker.diff", "worker-result.json", "review-result.json"]},
                    "offset": {"type": "integer", "minimum": 0}
                }
            }),
        ),
        tool(
            "delegation_update",
            "Edit a card that is still awaiting approval.",
            json!({
                "type": "object",
                "required": ["jobId"],
                "properties": {
                    "jobId": {"type": "string"},
                    "expectedUpdatedAt": {"type": "integer"},
                    "title": {"type": "string"},
                    "objective": {"type": "string"},
                    "scope": {"type": "array", "items": {"type": "string"}},
                    "context": {"type": "array", "items": {"type": "string"}},
                    "acceptanceChecks": {"type": "array", "items": {"type": "string"}},
                    "worker": {"type": "object"},
                    "reviewer": {"type": "object"}
                }
            }),
        ),
        tool(
            "delegation_approve",
            "Record human approval, pin fingerprints, and enqueue the worker. Does not apply a patch.",
            object_with(&["jobId"]),
        ),
        tool(
            "delegation_retry",
            "Return a blocked or failed card to awaiting_approval. A new approval is required.",
            object_with(&["jobId"]),
        ),
        tool(
            "delegation_cancel",
            "Cancel a card that is not already terminal.",
            object_with(&["jobId"]),
        ),
        tool(
            "delegation_apply",
            "Apply a ready_to_apply worker.diff after scope validation and git apply --check.",
            object_with(&["jobId"]),
        ),
    ]
}

fn object_with(required: &[&str]) -> Value {
    let mut properties = serde_json::Map::new();
    for name in required {
        properties.insert((*name).into(), json!({"type": "string"}));
    }
    properties.insert("expectedUpdatedAt".into(), json!({"type": "integer"}));
    json!({
        "type": "object",
        "required": required,
        "properties": properties
    })
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn required_str(arguments: &Value, name: &str) -> Result<String, (i64, String, StatusCode)> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or((-32602, format!("missing `{name}`"), StatusCode::OK))
}

fn optional_u64(arguments: &Value, name: &str) -> Option<u64> {
    arguments.get(name).and_then(Value::as_u64)
}

fn tool_err(error: String) -> (i64, String, StatusCode) {
    let code = if error.contains("changed; expected updatedAt") {
        -32009
    } else {
        -32000
    };
    (code, error, StatusCode::OK)
}

fn internal(error: String) -> (i64, String, StatusCode) {
    (-32603, error, StatusCode::OK)
}

fn json_result(id: Option<Value>, result: Value) -> Response {
    let mut body = json!({
        "jsonrpc": "2.0",
        "result": result
    });
    if let Some(id) = id {
        body["id"] = id;
    }
    json_response(StatusCode::OK, body)
}

fn json_error(id: Option<Value>, code: i64, message: &str, status: StatusCode) -> Response {
    let mut body = json!({
        "jsonrpc": "2.0",
        "error": { "code": code, "message": message }
    });
    if let Some(id) = id {
        body["id"] = id;
    }
    json_response(status, body)
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let mut response = Response::new(axum::body::Body::from(bytes));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}
