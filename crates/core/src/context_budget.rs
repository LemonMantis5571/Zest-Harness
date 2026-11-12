//! Context-window budget arithmetic, shared by the desktop meter and by
//! compaction.
//!
//! One home for these numbers because two callers need the same answer:
//! the chat chrome asks "how full is the window", and [`crate::Agent::
//! compact_context`] asks "would shortening tool results alone bring it back
//! under". Two copies of char/4 would drift, and the disagreement would show up
//! as a compaction that fires without relieving anything.
//!
//! Everything here is an *estimate* except a
//! [`crate::anthropic::types::Usage::prompt_tokens`] reading, which is what a
//! provider actually reported. char/4 stands in for a tokenizer, so it
//! under-counts code and JSON — decisions built on it should be arranged to err
//! toward doing the more expensive, more correct thing.
//!
//! System and conversation counts use different character/byte bases, and tool
//! schemas are estimated from their serialized definitions. The char/4
//! approximation still makes the result a floor rather than a bound.

use crate::anthropic::types::{Message, ToolDef};
use crate::provider::SystemPrompt;

/// Window occupancy at which the front-end starts compacting on its own.
pub const AUTO_COMPACT_THRESHOLD_PERCENT: u64 = 80;

/// Below this much estimated conversation there is nothing worth summarizing,
/// whatever the occupancy says.
///
/// This is the floor that keeps compaction honest on a small window: system
/// prompt and tool schemas can approach the threshold by themselves, and
/// compaction cannot shrink either one. Always measured against the
/// conversation estimate — never against a measured prompt total, which
/// includes the parts compaction cannot touch.
pub const MIN_COMPACTION_CONVERSATION_TOKENS: u64 = 4_000;

/// char/4, the standing stand-in for a tokenizer.
///
/// Non-empty input never estimates to zero: "some content" is a better answer
/// than "no content" for anything shorter than four characters.
pub fn chars_to_tok(chars: u64) -> u64 {
    if chars == 0 {
        0
    } else {
        (chars / 4).max(1)
    }
}

/// Estimated tokens held by the system prompt, cacheable and volatile halves
/// together.
pub fn system_tokens(system: Option<&SystemPrompt>) -> u64 {
    chars_to_tok(system.map_or(0, |prompt| prompt.char_len() as u64))
}

/// Estimated tokens held by a wire conversation.
///
/// Counts serialized block bytes, so structural JSON overhead is included and
/// image payloads are counted at their base64 length.
pub fn conversation_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|message| {
            chars_to_tok(
                message
                    .content
                    .iter()
                    .map(|block| block.to_string().len() as u64)
                    .sum(),
            )
        })
        .sum()
}

/// Estimated tokens held by the serialized tool definitions in the prompt.
///
/// Provider wrappers add a small amount of wire-format overhead, but counting
/// the definitions themselves avoids treating a large tool catalogue as free.
pub fn tool_schema_tokens(tools: &[ToolDef]) -> u64 {
    if tools.is_empty() {
        return 0;
    }
    serde_json::to_vec(tools)
        .map(|encoded| chars_to_tok(encoded.len() as u64))
        .unwrap_or(0)
}

/// The occupancy, in tokens, at which auto-compaction becomes due. Rounds up so
/// a threshold is reached rather than approached.
pub fn auto_compact_threshold(window: u64) -> u64 {
    window
        .saturating_mul(AUTO_COMPACT_THRESHOLD_PERCENT)
        .saturating_add(99)
        / 100
}

/// Whether the front-end should compact now.
///
/// `used` is the measured prompt total where one exists and the estimate
/// otherwise; `conversation_tokens` is always the estimate, because it answers
/// a different question — see [`MIN_COMPACTION_CONVERSATION_TOKENS`].
pub fn auto_compaction_due(
    used: u64,
    window: u64,
    conversation_tokens: u64,
    message_count: usize,
) -> bool {
    let can_compact =
        conversation_tokens > MIN_COMPACTION_CONVERSATION_TOKENS && message_count >= 4;
    can_compact && used >= auto_compact_threshold(window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_compaction_starts_at_threshold_and_requires_history() {
        assert!(!auto_compaction_due(102_399, 128_000, 4_001, 4));
        assert!(auto_compaction_due(102_400, 128_000, 4_001, 4));
        assert!(!auto_compaction_due(102_400, 128_000, 4_000, 4));
        assert!(!auto_compaction_due(102_400, 128_000, 4_001, 3));
    }

    #[test]
    fn a_short_string_still_estimates_as_content() {
        assert_eq!(chars_to_tok(0), 0);
        assert_eq!(chars_to_tok(1), 1);
        assert_eq!(chars_to_tok(3), 1);
        assert_eq!(chars_to_tok(8), 2);
    }

    #[test]
    fn a_missing_system_prompt_costs_nothing() {
        assert_eq!(system_tokens(None), 0);
    }

    #[test]
    fn both_halves_of_the_system_prompt_are_counted() {
        let prompt = SystemPrompt::new("a".repeat(400)).with_volatile("b".repeat(400));
        // 400 + 2 separator + 400, over four.
        assert_eq!(system_tokens(Some(&prompt)), 200);
    }

    #[test]
    fn conversation_tokens_sum_over_every_message() {
        let messages = vec![
            Message::user_text("hello"),
            Message::assistant(vec![json!({ "type": "text", "text": "hi" })]),
        ];
        let total = conversation_tokens(&messages);
        assert_eq!(
            total,
            conversation_tokens(&messages[..1]) + conversation_tokens(&messages[1..]),
            "the estimate is a per-message sum, so slicing must not change it"
        );
        assert!(total > 0, "{total}");
    }

    #[test]
    fn tool_schema_estimate_counts_serialized_definitions() {
        let tools = vec![ToolDef {
            name: "read_file".into(),
            description: "Read a file from the project".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            cache_control: None,
        }];
        let encoded_len = serde_json::to_vec(&tools).unwrap().len() as u64;

        assert_eq!(tool_schema_tokens(&tools), chars_to_tok(encoded_len));
        assert_eq!(tool_schema_tokens(&[]), 0);
    }

    #[test]
    fn an_empty_conversation_estimates_to_zero() {
        assert_eq!(conversation_tokens(&[]), 0);
    }

    #[test]
    fn an_unknown_window_reads_as_due() {
        // Recording the arithmetic rather than defending it: a zero window puts
        // the threshold at zero, so any occupancy clears it. Unreachable in
        // practice — `Agent::context_window` filters zero out and falls back to
        // the static table, which always answers non-zero.
        assert_eq!(auto_compact_threshold(0), 0);
        assert!(auto_compaction_due(0, 0, 4_001, 4));
    }
}
