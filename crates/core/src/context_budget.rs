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
//! Two known inconsistencies, preserved verbatim from the desktop meter this
//! moved out of so that a regression stays bisectable: system length is counted
//! in *characters* while conversation length is counted in serialized-JSON
//! *bytes*, and tool schemas are not counted at all. Both make the estimate a
//! floor rather than a bound.

use crate::anthropic::types::Message;
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
