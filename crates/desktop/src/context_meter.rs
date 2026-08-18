//! Context-window occupancy for the chat chrome.
//!
//! Honest labels: the provider's own count of the whole prompt when it reported
//! one, otherwise a char/4 estimate over system + conversation (no tool-schema
//! stringify). The arithmetic itself lives in [`zest_core::context_budget`],
//! because compaction has to reach the same answer.

use serde::Serialize;
use zest_core::context_budget::{
    auto_compaction_due, conversation_tokens, system_tokens, AUTO_COMPACT_THRESHOLD_PERCENT,
    MIN_COMPACTION_CONVERSATION_TOKENS,
};
use zest_core::{Agent, Usage};

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
    /// Fresh input on the last measured turn. Zero when `source` is `estimate`.
    pub input_tokens: u64,
    /// Prompt the provider served from its cache on the last measured turn.
    pub cache_read_tokens: u64,
    /// Prompt the provider wrote into its cache on the last measured turn.
    pub cache_write_tokens: u64,
    pub message_count: usize,
    pub checkpoint_count: usize,
    pub can_compact: bool,
    pub auto_compact_threshold_percent: u64,
    pub should_auto_compact: bool,
}

/// What one compaction did, alongside the resulting occupancy.
///
/// Separate from [`ContextUsageView`] rather than extra fields on it: the
/// `context_usage` command serves that type on every refresh, where these would
/// always be zero.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResultView {
    pub usage: ContextUsageView,
    /// True when shortening long tool results was enough and no model call was
    /// made — the conversation was kept rather than replaced by a summary.
    pub pruned_only: bool,
    pub results_pruned: usize,
}

/// Prefer the provider's count of the whole prompt; fall back to char/4.
///
/// `input_tokens` alone excludes everything the cache served. Since the 1h
/// tools+system prefix landed that is nearly the whole prompt, so reading it as
/// occupancy reported a nearly-full window as almost empty — the 80% threshold
/// fired late or never and a long session ended in a provider context-overflow
/// error instead of a compaction.
///
/// A pure function because [`estimate_context`] needs a live [`Agent`], and so
/// a provider, to be called at all; this is the part worth pinning.
fn used_tokens(last_usage: Option<&Usage>, estimate: u64) -> (u64, &'static str) {
    match last_usage.map(Usage::prompt_tokens) {
        Some(prompt) if prompt > 0 => (prompt, "last_turn"),
        _ => (estimate, "estimate"),
    }
}

pub fn estimate_context(agent: &Agent, checkpoint_count: usize) -> ContextUsageView {
    let window = agent.context_window();
    let system_tokens = system_tokens(agent.system.as_ref());
    let conversation_tokens = conversation_tokens(&agent.messages);

    let (used, source) = used_tokens(
        agent.last_usage.as_ref(),
        system_tokens + conversation_tokens,
    );
    let measured = agent.last_usage.as_ref().filter(|_| source == "last_turn");

    let remaining = window.saturating_sub(used);
    let percent_full = if window == 0 {
        0.0
    } else {
        ((used as f64) / (window as f64) * 100.0).min(100.0)
    };

    // Deliberately the *estimate*, never `used`: system prompt and tool schemas
    // can approach the threshold on their own, and compaction shrinks neither.
    // This floor is what stops a measured prompt from triggering a compaction
    // that could not have helped.
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
        input_tokens: measured.map_or(0, |u| u64::from(u.input_tokens)),
        cache_read_tokens: measured.map_or(0, |u| u64::from(u.cache_read_input_tokens)),
        cache_write_tokens: measured.map_or(0, |u| u64::from(u.cache_creation_input_tokens)),
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
    fn a_well_cached_prompt_is_measured_from_every_column() {
        let usage = Usage {
            input_tokens: 1_200,
            output_tokens: 800,
            cache_read_input_tokens: 96_000,
            cache_creation_input_tokens: 2_800,
        };
        // Reading `input_tokens` alone gave 1_200 here — 1% of a 120k window
        // that is in fact 83% full, so compaction never came due.
        assert_eq!(used_tokens(Some(&usage), 40_000), (100_000, "last_turn"));
        assert!(auto_compaction_due(100_000, 120_000, 40_000, 12));
        assert!(!auto_compaction_due(1_200, 120_000, 40_000, 12));
    }

    #[test]
    fn a_silent_provider_falls_back_to_the_estimate() {
        assert_eq!(
            used_tokens(Some(&Usage::default()), 9_000),
            (9_000, "estimate")
        );
        assert_eq!(used_tokens(None, 9_000), (9_000, "estimate"));
    }

    #[test]
    fn an_output_only_turn_is_not_a_measurement() {
        // Nothing was reported about the prompt, so the estimate still owns the
        // answer rather than a window that reads as empty.
        let usage = Usage {
            output_tokens: 4_096,
            ..Usage::default()
        };
        assert_eq!(used_tokens(Some(&usage), 9_000), (9_000, "estimate"));
    }
}
