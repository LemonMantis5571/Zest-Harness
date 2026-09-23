//! OpenRouter's Decisions API, exposed as an agent tool for typed decisions.

use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::config::{Config, ProviderConfig};
use crate::provider::driver::{credentials_for, resolve};

use super::outcome::ToolOutcome;
use super::Tool;

const DECISIONS_URL: &str = "https://openrouter.ai/api/alpha/decisions";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_REVIEW_STATE_BYTES: usize = 80 * 1024;
// A displayed concern should be worth the user's attention. This is a
// conservative starting point, to be calibrated against labeled Zest work.
const DISPLAY_PROBABILITY: f64 = 0.9;

#[derive(Debug, Clone, Copy)]
pub enum JevReviewKind {
    Plan,
    Changes,
    Delegated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevCheck {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub choice: String,
    /// `clear`, `concern`, or `inconclusive`. This is a signal, not a finding.
    pub outcome: String,
    pub probability: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevReview {
    pub provider_id: String,
    pub model: String,
    pub configured_model: String,
    pub content_hash: String,
    pub checks: Vec<JevCheck>,
    pub usage: Option<Value>,
}

/// Optional second opinion after the full reviewer has inspected a delegated
/// diff. This is advisory; the ordinary reviewer owns acceptance.
pub async fn review_with_jev(config: &Config, state: Value) -> Result<Option<JevReview>, String> {
    check_with_jev(config, JevReviewKind::Delegated, state).await
}

pub fn configured_review_model(config: &Config) -> Option<&str> {
    config.providers.values().find_map(|provider| {
        if let ProviderConfig::OpenaiCompatible {
            decision_reviewer: true,
            decision_model: Some(model),
            ..
        } = provider
        {
            Some(model.as_str())
        } else {
            None
        }
    })
}

pub async fn check_with_jev(
    config: &Config,
    kind: JevReviewKind,
    state: Value,
) -> Result<Option<JevReview>, String> {
    let mut selected = None;
    for (id, provider) in &config.providers {
        let ProviderConfig::OpenaiCompatible {
            base_url,
            decision_model,
            decision_reviewer: true,
            ..
        } = provider
        else {
            continue;
        };
        if selected.is_some() {
            return Err("configure only one OpenRouter provider as the Jev reviewer".into());
        }
        let url = reqwest::Url::parse(base_url)
            .map_err(|_| format!("Jev reviewer `{id}` has an invalid endpoint"))?;
        if url.scheme() != "https" || url.host_str() != Some("openrouter.ai") {
            return Err(format!(
                "Jev reviewer `{id}` requires https://openrouter.ai"
            ));
        }
        let model = decision_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| format!("Jev reviewer `{id}` has no decision model"))?;
        selected = Some((id.as_str(), provider, model));
    }
    let Some((id, provider, model)) = selected else {
        return Ok(None);
    };
    let state_bytes = serde_json::to_vec(&state)
        .map_err(|error| format!("could not encode Jev review state: {error}"))?;
    let state_size = state_bytes.len();
    if state_size > MAX_REVIEW_STATE_BYTES {
        return Err(format!(
            "Jev review needs a smaller diff: {} bytes exceeds the {} byte review limit",
            state_size, MAX_REVIEW_STATE_BYTES
        ));
    }
    let key = resolve(credentials_for(id, provider))?
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| format!("OpenRouter API key for Jev reviewer `{id}` is missing"))?;
    let tool = OpenRouterDecisionTool::new(key, model, id, "jev_review")?;
    let outcome = tool
        .run(json!({
            "state": state,
            "questions": questions_for(kind)
        }))
        .await?;
    let response: Value = serde_json::from_str(&outcome.body)
        .map_err(|error| format!("could not parse Jev review response: {error}"))?;
    let checks = parse_review_answers(kind, &response)?;
    Ok(Some(JevReview {
        provider_id: id.to_string(),
        model: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_string(),
        configured_model: model.to_string(),
        content_hash: blake3::hash(&state_bytes).to_hex().to_string(),
        checks,
        usage: response.get("usage").cloned(),
    }))
}

fn questions_for(kind: JevReviewKind) -> Value {
    let mut questions = Map::new();
    for (id, _, instruction) in check_definitions(kind) {
        questions.insert(id.into(), json!({
            "type": "choice",
            "instructions": instruction,
            "criteria": {
                "clear": "The supplied evidence supports this check.",
                "concern": "The supplied evidence shows a concrete gap or conflict for this check.",
                "unclear": "The supplied evidence is insufficient to decide this check."
            }
        }));
    }
    Value::Object(questions)
}

fn parse_review_answers(kind: JevReviewKind, response: &Value) -> Result<Vec<JevCheck>, String> {
    let mut checks = Vec::new();
    for (id, label, _) in check_definitions(kind) {
        let answer = response
            .pointer(&format!("/answers/{id}"))
            .ok_or_else(|| format!("Jev omitted the `{id}` answer"))?;
        if answer.get("type").and_then(Value::as_str) != Some("choice") {
            return Err(format!("Jev `{id}` answer was not a Choice"));
        }
        let choice = answer
            .get("choice")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Jev `{id}` answer omitted its choice"))?;
        if !matches!(choice, "clear" | "concern" | "unclear") {
            return Err(format!("Jev `{id}` answer selected an unknown choice"));
        }
        let probability = answer
            .pointer(&format!("/probabilities/{choice}"))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or_else(|| format!("Jev `{id}` answer omitted a valid probability"))?;
        let outcome = if choice == "unclear" || probability < DISPLAY_PROBABILITY {
            "inconclusive"
        } else {
            choice
        };
        checks.push(JevCheck {
            id: id.into(),
            label: label.into(),
            choice: choice.into(),
            outcome: outcome.into(),
            probability,
        });
    }
    Ok(checks)
}

fn check_definitions(kind: JevReviewKind) -> [(&'static str, &'static str, &'static str); 3] {
    match kind {
        JevReviewKind::Plan => [
            ("coverage", "Request coverage", "Does the finished plan cover the user's requested deliverables? A concern means a requested deliverable is absent."),
            ("constraints", "Stated constraints", "Does the plan respect the user's explicit constraints? A concern means a proposed step conflicts with a stated constraint."),
            ("verification", "Verification path", "Does the plan say how its deliverables will be checked? A concern means meaningful verification is absent."),
        ],
        JevReviewKind::Changes | JevReviewKind::Delegated => [
            ("alignment", "Task alignment", "Does the supplied change evidence address the stated task? A concern means the change appears unrelated or misses the stated objective."),
            ("scope", "Change scope", "Does the supplied diff stay within the requested scope? A concern means it appears to add unrelated work or conflict with an explicit constraint."),
            ("completion", "Completion evidence", "Do the supplied checks and review evidence support the reported completion? A concern means they contradict a completion claim."),
        ],
    }
}

pub struct OpenRouterDecisionTool {
    http: reqwest::Client,
    api_key: String,
    model: String,
    name: String,
    description: String,
    endpoint: String,
}

impl OpenRouterDecisionTool {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        provider_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, String> {
        Self::with_endpoint(api_key, model, provider_id, name, DECISIONS_URL)
    }

    fn with_endpoint(
        api_key: impl Into<String>,
        model: impl Into<String>,
        provider_id: impl Into<String>,
        name: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Result<Self, String> {
        let model = model.into();
        if model.trim().is_empty() {
            return Err("OpenRouter decision model cannot be empty".into());
        }
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err("OpenRouter API key is missing".into());
        }
        let endpoint = endpoint.into();
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| format!("could not build OpenRouter Decisions client: {error}"))?;
        Ok(Self {
            http,
            api_key,
            model,
            name: name.into(),
            description: format!(
                "Ask Jev for typed decisions via the OpenRouter provider `{}`. Use for classification, yes/no judgments, or ordered scores. Jev returns answers and probabilities, not explanations. It only sees the `state` you provide; chat history is not sent automatically.",
                provider_id.into()
            ),
            endpoint,
        })
    }
}

#[async_trait]
impl Tool for OpenRouterDecisionTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "state": {
                    "description": "Non-empty text, a non-empty JSON object, or an array of non-empty strings to evaluate.",
                    "anyOf": [
                        { "type": "string" },
                        { "type": "object" },
                        { "type": "array", "items": { "type": "string" } }
                    ]
                },
                "questions": {
                    "type": "object",
                    "description": "Map question ids to objects. Each object needs `type` and `instructions`. Choice needs `criteria` as an object of at least two option ids mapped to descriptions. Score needs `criteria` as an ordered array of at least two level descriptions. Noul may include true/false criteria.",
                    "minProperties": 1,
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string", "enum": ["choice", "score", "noul"] },
                            "instructions": { "type": "string" },
                            "criteria": { "description": "Choice: map option ids to descriptions. Score: ordered array of level descriptions. Noul: optional true/false descriptions." }
                        },
                        "required": ["type", "instructions"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["state", "questions"],
            "additionalProperties": false
        })
    }

    async fn run(&self, input: Value) -> Result<ToolOutcome, String> {
        let request = validate_input(input)?;
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&json!({
                "model": self.model,
                "state": request.state,
                "questions": request.questions,
            }))
            .send()
            .await
            .map_err(|error| format!("OpenRouter Decisions request failed: {error}"))?;

        let status = response.status();
        let body = read_limited_body(response).await?;
        let value: Value = serde_json::from_slice(&body).map_err(|error| {
            format!("OpenRouter Decisions returned invalid JSON (HTTP {status}): {error}")
        })?;
        if !status.is_success() {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("request was rejected");
            return Err(format!(
                "OpenRouter Decisions returned HTTP {status}: {}",
                truncate(message, 1200)
            ));
        }

        let answers = value
            .get("answers")
            .and_then(Value::as_object)
            .filter(|answers| !answers.is_empty())
            .ok_or_else(|| "OpenRouter Decisions returned no answers".to_string())?;
        for question_id in request.question_ids {
            if !answers.contains_key(&question_id) {
                return Err(format!(
                    "OpenRouter Decisions omitted the answer for `{question_id}`"
                ));
            }
        }

        let mut result = Map::new();
        result.insert("answers".into(), Value::Object(answers.clone()));
        for field in ["model", "usage", "id", "provider"] {
            if let Some(field_value) = value.get(field) {
                result.insert(field.into(), field_value.clone());
            }
        }
        let rendered = serde_json::to_string_pretty(&Value::Object(result))
            .map_err(|error| format!("could not format Jev decision result: {error}"))?;
        Ok(ToolOutcome::text(rendered))
    }
}

#[derive(Debug)]
struct DecisionRequest {
    state: Value,
    questions: Value,
    question_ids: Vec<String>,
}

fn validate_input(input: Value) -> Result<DecisionRequest, String> {
    let input = input
        .as_object()
        .ok_or_else(|| "decision input must be a JSON object".to_string())?;
    if input
        .keys()
        .any(|key| !matches!(key.as_str(), "state" | "questions"))
    {
        return Err("decision input only accepts `state` and `questions`".into());
    }

    let state = input
        .get("state")
        .filter(|state| valid_state(state))
        .cloned()
        .ok_or_else(|| {
            "`state` must be non-empty text, a JSON object, or an array of strings".to_string()
        })?;

    let questions = input
        .get("questions")
        .and_then(Value::as_object)
        .filter(|questions| !questions.is_empty())
        .ok_or_else(|| "`questions` must be a non-empty object".to_string())?;
    for (id, question) in questions {
        if id.trim().is_empty() || id.chars().count() > 64 {
            return Err("question ids must contain 1–64 characters".into());
        }
        validate_question(id, question)?;
    }

    Ok(DecisionRequest {
        state,
        questions: Value::Object(questions.clone()),
        question_ids: questions.keys().cloned().collect(),
    })
}

fn valid_state(state: &Value) -> bool {
    match state {
        Value::String(text) => !text.trim().is_empty(),
        Value::Object(object) => !object.is_empty(),
        Value::Array(items) => {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty()))
        }
        _ => false,
    }
}

fn validate_question(id: &str, question: &Value) -> Result<(), String> {
    let question = question
        .as_object()
        .ok_or_else(|| format!("question `{id}` must be an object"))?;
    let kind = question
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("question `{id}` needs a `type`"))?;
    let instructions = question
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("question `{id}` needs non-empty `instructions`"))?;
    if instructions.chars().count() > 4000 {
        return Err(format!(
            "question `{id}` instructions are too long (max 4000 characters)"
        ));
    }

    let valid = match kind {
        "choice" => {
            question.len() == 3
                && question
                    .get("criteria")
                    .and_then(Value::as_object)
                    .is_some_and(|criteria| {
                        criteria.len() >= 2
                            && criteria.iter().all(|(key, value)| {
                                !key.trim().is_empty()
                                    && value.as_str().is_some_and(|text| !text.trim().is_empty())
                            })
                    })
        }
        "score" => {
            question.len() == 3
                && question
                    .get("criteria")
                    .and_then(Value::as_array)
                    .is_some_and(|criteria| {
                        criteria.len() >= 2
                            && criteria.iter().all(|level| {
                                level.as_str().is_some_and(|text| !text.trim().is_empty())
                            })
                    })
        }
        "noul" => {
            question.len() == 2
                || (question.len() == 3
                    && question
                        .get("criteria")
                        .and_then(Value::as_object)
                        .is_some_and(|criteria| {
                            criteria.len() == 2
                                && ["true", "false"].iter().all(|key| {
                                    criteria
                                        .get(*key)
                                        .and_then(Value::as_str)
                                        .is_some_and(|text| !text.trim().is_empty())
                                })
                        }))
        }
        _ => false,
    };
    if !valid {
        return Err(format!(
            "question `{id}` must be a valid Choice, Score, or Noul question"
        ));
    }
    Ok(())
}

async fn read_limited_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err("OpenRouter Decisions response exceeded 1 MiB".into());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("could not read Jev response: {error}"))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("OpenRouter Decisions response exceeded 1 MiB".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut output: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        output.push_str("…");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn sample_input() -> Value {
        json!({
            "state": "The customer was billed twice.",
            "questions": {
                "team": {
                    "type": "choice",
                    "instructions": "Which team should handle this?",
                    "criteria": {
                        "billing": "Charges, invoices, and refunds",
                        "technical": "Bugs and outages"
                    }
                },
                "refund": {
                    "type": "noul",
                    "instructions": "Is the customer asking for a refund?"
                }
            }
        })
    }

    #[test]
    fn validates_all_three_question_primitives() {
        let mut request = sample_input();
        request["questions"]["severity"] = json!({
            "type": "score",
            "instructions": "How severe is the issue?",
            "criteria": ["Minor", "Blocking"]
        });
        assert!(validate_input(request).is_ok());

        let mut request = sample_input();
        request["questions"]["refund"]["criteria"] = json!({
            "true": "Customer requests a refund",
            "false": "No refund request"
        });
        assert!(validate_input(request).is_ok());
    }

    #[test]
    fn rejects_bad_state_and_incomplete_choice_criteria() {
        let mut request = sample_input();
        request["state"] = Value::Null;
        assert!(validate_input(request).unwrap_err().contains("`state`"));

        let mut request = sample_input();
        request["questions"]["team"]["criteria"] = json!({ "billing": "Billing" });
        assert!(validate_input(request)
            .unwrap_err()
            .contains("Choice, Score, or Noul"));
    }

    #[test]
    fn quick_checks_require_valid_answers_and_treat_weak_signals_as_inconclusive() {
        let response = json!({"answers": {
            "coverage": {"type":"choice", "choice":"clear", "probabilities":{"clear":0.94}},
            "constraints": {"type":"choice", "choice":"concern", "probabilities":{"concern":0.92}},
            "verification": {"type":"choice", "choice":"clear", "probabilities":{"clear":0.62}}
        }});
        let checks = parse_review_answers(JevReviewKind::Plan, &response).unwrap();
        assert_eq!(
            checks
                .iter()
                .map(|check| check.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["clear", "concern", "inconclusive"]
        );
        let mut malformed = response;
        malformed["answers"]["coverage"]["probabilities"] = json!({});
        assert!(parse_review_answers(JevReviewKind::Plan, &malformed).is_err());

        let change_response = json!({"answers": {
            "alignment": {"type":"choice", "choice":"clear", "probabilities":{"clear":0.95}},
            "scope": {"type":"choice", "choice":"concern", "probabilities":{"concern":0.93}},
            "completion": {"type":"choice", "choice":"unclear", "probabilities":{"unclear":0.97}}
        }});
        let change_checks = parse_review_answers(JevReviewKind::Changes, &change_response).unwrap();
        assert_eq!(
            change_checks
                .iter()
                .map(|check| check.id.as_str())
                .collect::<Vec<_>>(),
            vec!["alignment", "scope", "completion"]
        );
        assert_eq!(change_checks[1].outcome, "concern");
        assert_eq!(change_checks[2].outcome, "inconclusive");
    }

    #[tokio::test]
    async fn review_is_off_without_an_opted_in_provider() {
        let config = Config::env_fallback();
        assert!(review_with_jev(&config, json!({"objective": "test"}))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn review_rejects_ambiguous_provider_configuration_before_network_io() {
        let config = Config::parse(
            r#"
[providers.first]
kind = "openai_compatible"
base_url = "https://openrouter.ai/api/v1"
model = "openrouter/auto"
decision_model = "~typesafe/jev-latest"
decision_reviewer = true

[providers.second]
kind = "openai_compatible"
base_url = "https://openrouter.ai/api/v1"
model = "openrouter/auto"
decision_model = "~typesafe/jev-latest"
decision_reviewer = true
"#,
        )
        .unwrap();
        let error = review_with_jev(&config, json!({"objective": "test"}))
            .await
            .unwrap_err();
        assert!(error.contains("only one OpenRouter provider"));
    }

    #[tokio::test]
    async fn oversized_evidence_is_rejected_before_any_api_call() {
        let config = Config::parse(
            r#"
[providers.openrouter]
kind = "openai_compatible"
base_url = "https://openrouter.ai/api/v1"
model = "openrouter/auto"
decision_model = "~typesafe/jev-latest"
decision_reviewer = true
"#,
        )
        .unwrap();
        let error = check_with_jev(
            &config,
            JevReviewKind::Changes,
            json!({"diff": "x".repeat(MAX_REVIEW_STATE_BYTES)}),
        )
        .await
        .unwrap_err();
        assert!(error.contains("review limit"));
    }

    #[tokio::test]
    async fn posts_typed_questions_and_returns_answers_with_usage() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 2048];
            let (header_end, content_length) = loop {
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0, "client closed before sending the request");
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..index]);
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= index + 4 + length {
                        break (index, length);
                    }
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
            let body: Value =
                serde_json::from_slice(&bytes[header_end + 4..header_end + 4 + content_length])
                    .unwrap();
            request_tx.send((headers, body)).unwrap();
            let response = json!({
                "model": "typesafe/jev-1.13-20260917",
                "answers": {
                    "team": { "type": "choice", "choice": "billing", "confidence": 0.91 },
                    "refund": { "type": "noul", "noul": 0.88 }
                },
                "usage": { "input_tokens": 321, "output_tokens": 22, "cost": 0.00001 },
                "id": "gen-dec-test",
                "provider": "TypeSafe"
            })
            .to_string();
            let response_wire = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(), response
            );
            stream.write_all(response_wire.as_bytes()).await.unwrap();
        });

        let tool = OpenRouterDecisionTool::with_endpoint(
            "secret-test-key",
            "~typesafe/jev-latest",
            "openrouter",
            "jev_decide",
            format!("http://{address}/api/alpha/decisions"),
        )
        .unwrap();
        let outcome = tool.run(sample_input()).await.unwrap();
        let (headers, body) = request_rx.await.unwrap();
        assert!(headers.starts_with("post /api/alpha/decisions "));
        assert!(headers.contains("authorization: bearer secret-test-key"));
        assert_eq!(body["model"], "~typesafe/jev-latest");
        assert_eq!(body["questions"]["team"]["type"], "choice");
        assert_eq!(body["questions"]["refund"]["type"], "noul");

        let output: Value = serde_json::from_str(&outcome.body).unwrap();
        assert_eq!(output["answers"]["team"]["choice"], "billing");
        assert_eq!(output["usage"]["input_tokens"], 321);
        assert_eq!(output["id"], "gen-dec-test");
        server.await.unwrap();
    }
}
