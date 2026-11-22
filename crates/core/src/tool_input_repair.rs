//! Tool-call arguments a model got almost right, fixed or sent back.
//!
//! Models sometimes send arguments as a JSON string instead of an object, put
//! a number or a nested object in quotes, or stop mid-object. Ending the turn
//! over that throws away the user's work for a mistake the model fixes on the
//! next call. So an unambiguous mistake is repaired against the tool's schema,
//! and anything else becomes an error result for that call alone: nothing runs,
//! and the model is told to call again.
//!
//! Providers keep an unparseable argument string as a JSON string in the
//! `tool_use` block's `input`; [`repair_tool_calls`] is the one place that
//! decides what it meant. Its output never leaves a non-object `input` behind,
//! because providers reject one when the history is sent back.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

/// Repair every `tool_use` block in `content` in place.
///
/// Returns the calls that could not be repaired, by tool-use id, with the
/// error result the model should get instead of a run. Their `input` is left
/// as an empty object so the assistant message can still be sent back.
pub fn repair_tool_calls(
    content: &mut [Value],
    schema_for: impl Fn(&str) -> Option<Value>,
) -> HashMap<String, String> {
    let mut invalid = HashMap::new();
    for block in content.iter_mut() {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input = block
            .get_mut("input")
            .map(Value::take)
            .unwrap_or(Value::Null);
        match repair_input(input, schema_for(&name).as_ref()) {
            Ok(repaired) => block["input"] = repaired,
            Err(reason) => {
                block["input"] = json!({});
                invalid.insert(
                    id,
                    format!(
                        "Invalid arguments for `{name}`: {reason}. Nothing ran. Call the \
                         tool again with its arguments as one JSON object that matches \
                         its schema."
                    ),
                );
            }
        }
    }
    invalid
}

/// The object `input` was meant to be, or why it cannot be known.
pub fn repair_input(input: Value, schema: Option<&Value>) -> Result<Value, String> {
    let object = match input {
        Value::Object(object) => object,
        // No arguments at all, for a tool that may take none.
        Value::Null => Map::new(),
        Value::String(text) => decode_object(&text)?,
        other => {
            return Err(format!(
                "they were {} instead of a JSON object",
                kind_of(&other)
            ))
        }
    };
    Ok(Value::Object(coerce_properties(object, schema)))
}

/// Arguments that arrived as text: JSON for an object, possibly encoded twice.
fn decode_object(text: &str) -> Result<Map<String, Value>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("they were empty".into());
    }
    let mut value: Value = serde_json::from_str(text).map_err(|error| {
        format!(
            "they were not valid JSON ({error}); received: {}",
            preview(text)
        )
    })?;
    if let Value::String(inner) = &value {
        value = serde_json::from_str(inner.trim())
            .map_err(|_| format!("they were a JSON string, not an object: {}", preview(text)))?;
    }
    match value {
        Value::Object(object) => Ok(object),
        other => Err(format!(
            "they were {} instead of a JSON object",
            kind_of(&other)
        )),
    }
}

/// Unquote a value whose schema names exactly one type and whose text is
/// unambiguously that type: `"5"` for an integer, `"true"` for a boolean,
/// `"[1,2]"` for an array. Anything else is left for the tool to judge.
fn coerce_properties(mut object: Map<String, Value>, schema: Option<&Value>) -> Map<String, Value> {
    let Some(properties) = schema
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)
    else {
        return object;
    };
    for (key, value) in object.iter_mut() {
        let Value::String(text) = value else { continue };
        let declared = properties
            .get(key)
            .and_then(|property| property.get("type"))
            .and_then(Value::as_str);
        let text = text.trim();
        let coerced = match declared {
            Some("integer") => text.parse::<i64>().ok().map(Value::from),
            Some("number") => text
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            Some("boolean") => match text {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            Some("object") => serde_json::from_str::<Value>(text)
                .ok()
                .filter(Value::is_object),
            Some("array") => serde_json::from_str::<Value>(text)
                .ok()
                .filter(Value::is_array),
            _ => None,
        };
        if let Some(coerced) = coerced {
            *value = coerced;
        }
    }
    object
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn preview(text: &str) -> String {
    const LIMIT: usize = 200;
    if text.chars().count() <= LIMIT {
        return text.to_string();
    }
    let cut: String = text.chars().take(LIMIT).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "offset": { "type": "integer" },
                "ratio": { "type": "number" },
                "recursive": { "type": "boolean" },
                "options": { "type": "object" },
                "paths": { "type": "array" },
            }
        })
    }

    #[test]
    fn stringified_json_becomes_the_object_it_encodes() {
        let repaired = repair_input(json!("{\"path\":\"a.rs\"}"), Some(&schema())).unwrap();
        assert_eq!(repaired, json!({ "path": "a.rs" }));
        let twice = repair_input(json!("\"{\\\"path\\\":\\\"a.rs\\\"}\""), None).unwrap();
        assert_eq!(twice, json!({ "path": "a.rs" }));
    }

    #[test]
    fn quoted_values_take_their_schema_type() {
        let repaired = repair_input(
            json!({
                "path": "12",
                "offset": " 40 ",
                "ratio": "0.5",
                "recursive": "true",
                "options": "{\"a\":1}",
                "paths": "[\"x\"]",
            }),
            Some(&schema()),
        )
        .unwrap();
        assert_eq!(
            repaired,
            json!({
                "path": "12",
                "offset": 40,
                "ratio": 0.5,
                "recursive": true,
                "options": { "a": 1 },
                "paths": ["x"],
            })
        );
    }

    #[test]
    fn ambiguous_values_are_left_for_the_tool() {
        let input = json!({ "offset": "forty", "recursive": "yes", "paths": "x" });
        assert_eq!(repair_input(input.clone(), Some(&schema())).unwrap(), input);
    }

    #[test]
    fn truncated_or_non_object_arguments_are_refused() {
        for input in [
            json!("{\"path\": "),
            json!(""),
            json!([1]),
            json!("[1]"),
            json!(7),
        ] {
            assert!(repair_input(input.clone(), None).is_err(), "{input}");
        }
        assert_eq!(repair_input(Value::Null, None).unwrap(), json!({}));
    }

    #[test]
    fn an_unrepairable_call_gets_an_error_and_an_empty_object() {
        let mut content = vec![
            json!({ "type": "text", "text": "Reading." }),
            json!({ "type": "tool_use", "id": "a", "name": "read", "input": "{\"path\": " }),
            json!({ "type": "tool_use", "id": "b", "name": "read", "input": "{\"offset\":\"3\"}" }),
        ];
        let invalid = repair_tool_calls(&mut content, |_| Some(schema()));
        assert_eq!(content[1]["input"], json!({}));
        assert_eq!(content[2]["input"], json!({ "offset": 3 }));
        assert_eq!(invalid.len(), 1);
        assert!(invalid["a"].contains("not valid JSON"), "{}", invalid["a"]);
        assert!(invalid["a"].contains("Call the tool again"));
    }
}
