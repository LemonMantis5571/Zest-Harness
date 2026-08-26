//! Structured questions that pause the active turn until the user answers.
//!
//! The tool definition is provider-neutral. The agent loop owns the pause and
//! emits the question event; front-ends provide the answer through [`Questioner`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::approval::ToolRisk;
use super::outcome::ToolOutcome;
use super::prepared::PreparedToolCall;
use super::{Tool, ToolRegistry};

pub const ASK_USER_TOOL: &str = "ask_user";
const MAX_PROMPT_CHARS: usize = 2_000;
const MAX_CHOICE_CHARS: usize = 200;
const MAX_CHOICES: usize = 8;

/// A single question requested by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRequest {
    pub question_id: String,
    pub tool_call_id: String,
    pub prompt: String,
    pub choices: Vec<String>,
    pub multiple: bool,
    pub placeholder: Option<String>,
}

/// Front-end hook for an interactive `ask_user` call.
#[async_trait]
pub trait Questioner: Send + Sync {
    /// Reserve a wait slot before the question event is emitted so a fast
    /// answer cannot race registration.
    async fn prepare(&self, _question_id: &str) {}

    /// Wait for the user's answer. The returned string becomes the model-visible
    /// tool result, never an extra synthetic user message.
    async fn answer(&self, request: &QuestionRequest) -> Result<String, String>;
}

/// Safe default for headless callers that do not have an interactive surface.
pub struct DenyQuestioner;

#[async_trait]
impl Questioner for DenyQuestioner {
    async fn answer(&self, _request: &QuestionRequest) -> Result<String, String> {
        Err("interactive questions are unavailable in this interface".into())
    }
}

/// Validate and convert model input into the provider-independent request sent
/// to a front-end.
pub fn parse_question_input(
    input: &Value,
    question_id: impl Into<String>,
    tool_call_id: impl Into<String>,
) -> Result<QuestionRequest, String> {
    let prompt = input
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing required field `question`".to_string())?;
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "`question` is too long (maximum {MAX_PROMPT_CHARS} characters)"
        ));
    }

    let mut choices = Vec::new();
    if let Some(raw_choices) = input.get("choices") {
        let values = raw_choices
            .as_array()
            .ok_or_else(|| "`choices` must be an array of strings".to_string())?;
        if values.len() > MAX_CHOICES {
            return Err(format!(
                "`choices` may contain at most {MAX_CHOICES} options"
            ));
        }
        for value in values {
            let choice = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "every `choices` entry must be a non-empty string".to_string())?;
            if choice.chars().count() > MAX_CHOICE_CHARS {
                return Err(format!(
                    "choice labels may be at most {MAX_CHOICE_CHARS} characters"
                ));
            }
            if choices
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(choice))
            {
                return Err("`choices` may not contain duplicate options".into());
            }
            choices.push(choice.to_string());
        }
    }

    let placeholder = input
        .get("placeholder")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let multiple = input
        .get("multiple")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if multiple && choices.is_empty() {
        return Err("`multiple` requires at least one choice".into());
    }

    Ok(QuestionRequest {
        question_id: question_id.into(),
        tool_call_id: tool_call_id.into(),
        prompt: prompt.to_string(),
        choices,
        multiple,
        placeholder,
    })
}

/// Tool definition exposed to the model. Execution is intercepted by the
/// provider-independent agent loop so it can emit a UI event before waiting.
pub struct AskUser;

#[async_trait]
impl Tool for AskUser {
    fn name(&self) -> &str {
        ASK_USER_TOOL
    }

    fn description(&self) -> &str {
        "Ask the user for a decision or missing detail. Use this when the next step depends on the user's preference or clarification. Provide a short question and finite choices when appropriate; omit choices for a free-form answer. Ask one question at a time."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "A short, concrete question for the user."
                },
                "choices": {
                    "type": "array",
                    "description": "Optional answer choices. Use two to eight concise labels for a finite decision.",
                    "items": { "type": "string" },
                    "maxItems": MAX_CHOICES
                },
                "multiple": {
                    "type": "boolean",
                    "description": "Set true only when the user may select more than one choice."
                },
                "placeholder": {
                    "type": "string",
                    "description": "Optional hint shown in a free-form answer field."
                }
            },
            "required": ["question"],
            "additionalProperties": false
        })
    }

    fn prepare(&self, input: Value) -> Result<PreparedToolCall, String> {
        parse_question_input(&input, "question-pending", "tool-pending")?;
        Ok(PreparedToolCall::plain(
            ASK_USER_TOOL,
            ToolRisk::Read,
            input,
        ))
    }

    async fn run(&self, _input: Value) -> Result<ToolOutcome, String> {
        Err("ask_user must be handled by the interactive agent loop".into())
    }
}

pub fn register_question_tool(registry: &mut ToolRegistry) {
    registry.register(Arc::new(AskUser));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_choice_and_free_form_questions() {
        let choice = parse_question_input(
            &json!({
                "question": "Which layout should I use?",
                "choices": ["Compact", "Spacious"]
            }),
            "q1",
            "call1",
        )
        .unwrap();
        assert_eq!(choice.prompt, "Which layout should I use?");
        assert_eq!(choice.choices, ["Compact", "Spacious"]);
        assert!(!choice.multiple);

        let free_form = parse_question_input(
            &json!({ "question": "What should this section be called?" }),
            "q2",
            "call2",
        )
        .unwrap();
        assert!(free_form.choices.is_empty());
    }

    #[test]
    fn rejects_invalid_or_ambiguous_choices() {
        assert!(parse_question_input(
            &json!({ "question": "Pick one", "choices": ["A", "a"] }),
            "q",
            "call",
        )
        .is_err());
        assert!(parse_question_input(
            &json!({ "question": "Pick one", "choices": "A" }),
            "q",
            "call",
        )
        .is_err());
        assert!(parse_question_input(&json!({ "choices": ["A", "B"] }), "q", "call",).is_err());
        assert!(parse_question_input(
            &json!({ "question": "Describe it", "multiple": true }),
            "q",
            "call",
        )
        .is_err());
    }

    #[test]
    fn registers_as_a_read_only_tool() {
        let mut registry = ToolRegistry::new();
        register_question_tool(&mut registry);
        assert_eq!(registry.names(), vec![ASK_USER_TOOL]);
        assert_eq!(registry.risk(ASK_USER_TOOL), Some(ToolRisk::Read));
    }
}
