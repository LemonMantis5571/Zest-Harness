//! Context-window estimates for the chat chrome.
//!
//! Honest labels: last-turn `input_tokens` from the API when available; otherwise
//! a char/4 estimate over system + conversation (no tool-schema stringify).

use serde::Serialize;
use zest_core::Agent;

pub const AUTO_COMPACT_THRESHOLD_PERCENT: u64 = 80;
const MIN_COMPACTION_CONVERSATION_TOKENS: u64 = 4_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageView {
    pub used_tokens: u64,
    pub window_tokens: u64,
    pub remaining_tokens: u64,
    pub percent_full: f64,
    /// `last_turn` | `estimate`
    pub source: String,
    pub system_tokens: u64,
    pub conversation_tokens: u64,
    pub message_count: usize,
    pub checkpoint_count: usize,
    pub can_compact: bool,
    pub auto_compact_threshold_percent: u64,
    pub should_auto_compact: bool,
}

pub fn context_window_for_model(model: &str) -> u64 {
    zest_core::context_window_for_model(model)
}

fn chars_to_tok(chars: u64) -> u64 {
    if chars == 0 {
        0
    } else {
        (chars / 4).max(1)
    }
}

fn auto_compaction_due(
    used: u64,
    window: u64,
    conversation_tokens: u64,
    message_count: usize,
) -> bool {
    let can_compact =
        conversation_tokens > MIN_COMPACTION_CONVERSATION_TOKENS && message_count >= 4;
    let threshold = window
        .saturating_mul(AUTO_COMPACT_THRESHOLD_PERCENT)
        .saturating_add(99)
        / 100;
    can_compact && used >= threshold
}

pub fn estimate_context(agent: &Agent, checkpoint_count: usize) -> ContextUsageView {
    let window = agent
        .descriptor()
        .models
        .into_iter()
        .find(|model| model.id == agent.model)
        .map(|model| model.context_window)
        .filter(|window| *window > 0)
        .unwrap_or_else(|| context_window_for_model(&agent.model));

    let system_tokens = chars_to_tok(
        agent
            .system
            .as_ref()
            .map_or(0, |prompt| prompt.char_len() as u64),
    );
    let conversation_tokens: u64 = agent
        .messages
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
        .sum();

    let (used, source) = match &agent.last_usage {
        Some(u) if u.input_tokens > 0 => (u.input_tokens as u64, "last_turn"),
        _ => (system_tokens + conversation_tokens, "estimate"),
    };

    let remaining = window.saturating_sub(used);
    let percent_full = if window == 0 {
        0.0
    } else {
        ((used as f64) / (window as f64) * 100.0).min(100.0)
    };

    let can_compact =
        conversation_tokens > MIN_COMPACTION_CONVERSATION_TOKENS && agent.messages.len() >= 4;
    let should_auto_compact =
        auto_compaction_due(used, window, conversation_tokens, agent.messages.len());

    ContextUsageView {
        used_tokens: used,
        window_tokens: window,
        remaining_tokens: remaining,
        percent_full,
        source: source.into(),
        system_tokens,
        conversation_tokens,
        message_count: agent.messages.len(),
        checkpoint_count,
        can_compact,
        auto_compact_threshold_percent: AUTO_COMPACT_THRESHOLD_PERCENT,
        should_auto_compact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_compaction_starts_at_threshold_and_requires_history() {
        assert!(!auto_compaction_due(102_399, 128_000, 4_001, 4));
        assert!(auto_compaction_due(102_400, 128_000, 4_001, 4));
        assert!(!auto_compaction_due(102_400, 128_000, 4_000, 4));
        assert!(!auto_compaction_due(102_400, 128_000, 4_001, 3));
    }
}
