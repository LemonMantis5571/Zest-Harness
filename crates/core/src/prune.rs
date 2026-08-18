//! Model-free shrinking of over-long tool results in a wire history.
//!
//! Compaction only. An ordinary turn must never reach this: rewriting a tool
//! result the model has already seen diverges the cached prompt prefix from the
//! point of the rewrite onward, and compaction is the one moment when that prefix
//! is being discarded anyway. Buying context back on a normal turn would be
//! paying a full-price re-read to save bytes that were already cheap.
//!
//! The point is to try the free thing before the expensive one. Summarizing a
//! conversation costs a model call and replaces the history with a paraphrase;
//! trimming the middle out of three enormous tool results costs nothing and keeps
//! the conversation. Only the token meter can say whether the trim was enough —
//! char budgets are not token budgets, and this module deliberately reports what
//! it removed rather than deciding what that means.

use crate::anthropic::types::Message;

/// Combined text length, in Unicode code points, past which a tool result is
/// rewritten.
pub const PRUNE_THRESHOLD_CHARS: usize = 8_192;
/// Leading code points kept. The head carries the shape of the output: the
/// command, the first error, the header row.
const PRUNE_HEAD_CHARS: usize = 4_096;
/// Trailing code points kept. The tail carries the summary and the exit status.
const PRUNE_TAIL_CHARS: usize = 1_024;
/// Fixed replacement for the removed middle.
pub const PRUNE_MARKER: &str = "\n\n[... tool result middle pruned ...]\n\n";

/// Idempotence, enforced at compile time.
///
/// If a replacement could itself exceed the threshold, a second pass would prune
/// its own output and keep doing so forever. Keeping this a `const` assertion
/// rather than a runtime check is also why the budgets are consts rather than
/// configuration: a violation is a build error, not a startup error.
const _: () = assert!(
    PRUNE_HEAD_CHARS + PRUNE_MARKER.len() + PRUNE_TAIL_CHARS <= PRUNE_THRESHOLD_CHARS,
    "a pruned result must fit the threshold, or pruning never converges"
);

/// Trailing messages left verbatim.
///
/// The newest tool round is what the model is actively working from, and it is
/// the most damaging thing to cut. Four covers one assistant turn plus its
/// results with room to spare.
pub const KEEP_RECENT_MESSAGES: usize = 4;

/// What one pruning pass removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// Tool results rewritten.
    ///
    /// Zero means the history holds nothing left to shorten — the signal that
    /// pruning has nothing more to give and a summarizer is required. Callers rely
    /// on this to terminate: without it, a history that cannot be shrunk further
    /// would be "pruned" on every attempt and never summarized.
    pub replaced: usize,
    /// Code points removed across every rewritten body.
    pub chars_saved: u64,
}

impl PruneReport {
    /// char/4 over [`Self::chars_saved`].
    ///
    /// Under-counts tokens in code and JSON, which is the useful direction: every
    /// decision built on it then understates the saving and errs toward doing the
    /// more expensive, more correct thing.
    pub fn tokens_saved_estimate(&self) -> u64 {
        crate::context_budget::chars_to_tok(self.chars_saved)
    }
}

/// Rewrite over-long `tool_result` bodies to head + marker + tail, in place.
///
/// `keep_recent` trailing messages are left untouched. Same walk as
/// `redact_sensitive_staged`: user messages, `tool_result` blocks, overwrite
/// `content` and nothing else.
///
/// `tool_use_id` and `is_error` are deliberately preserved. That is not
/// cosmetic — the API validates that every `tool_use` has a matching
/// `tool_result`, so dropping or reordering one would make the whole request
/// invalid, and redaction finds sensitive bodies by that same id afterwards.
pub fn prune_tool_results(messages: &mut [Message], keep_recent: usize) -> PruneReport {
    let mut report = PruneReport::default();
    let prunable = messages.len().saturating_sub(keep_recent);

    for msg in messages.iter_mut().take(prunable) {
        if msg.role != "user" {
            continue;
        }
        for block in &mut msg.content {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            // Only a bare string body. Zest's own producer always writes one, and
            // a block-array body belongs to a path that reads it back
            // defensively — guessing at its shape here would paper over a real
            // mismatch instead of leaving it visible.
            let Some(body) = block.get("content").and_then(|c| c.as_str()) else {
                continue;
            };
            let Some(replacement) = prune_body(body) else {
                continue;
            };
            report.chars_saved +=
                (body.chars().count() as u64).saturating_sub(replacement.chars().count() as u64);
            report.replaced += 1;
            block["content"] = serde_json::Value::String(replacement);
        }
    }

    report
}

/// The single-body rule. `None` when `body` already fits.
///
/// Slices by `char`, never by byte: byte indexing a `str` mid-character panics
/// rather than truncating, and one non-BMP scalar is a single `char` here, so an
/// emoji is atomic instead of splittable.
fn prune_body(body: &str) -> Option<String> {
    let total = body.chars().count();
    if total <= PRUNE_THRESHOLD_CHARS {
        return None;
    }
    let head: String = body.chars().take(PRUNE_HEAD_CHARS).collect();
    let tail: String = body
        .chars()
        .skip(total.saturating_sub(PRUNE_TAIL_CHARS))
        .collect();
    Some(format!("{head}{PRUNE_MARKER}{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn result_message(id: &str, body: &str) -> Message {
        Message::user_blocks(vec![crate::anthropic::types::tool_result(id, body, false)])
    }

    /// Enough trailing filler that the message under test is prunable.
    fn padded(messages: Vec<Message>) -> Vec<Message> {
        let mut out = messages;
        for _ in 0..KEEP_RECENT_MESSAGES {
            out.push(Message::user_text("padding"));
        }
        out
    }

    fn body_of(message: &Message) -> &str {
        message.content[0]
            .get("content")
            .and_then(|c| c.as_str())
            .expect("a string body")
    }

    #[test]
    fn an_over_long_result_becomes_head_marker_tail() {
        let body = format!("HEAD{}TAIL", "x".repeat(50_000));
        let mut messages = padded(vec![result_message("call-1", &body)]);
        let report = prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES);

        assert_eq!(report.replaced, 1);
        let pruned = body_of(&messages[0]);
        assert!(pruned.starts_with("HEAD"), "{}", &pruned[..40]);
        assert!(pruned.ends_with("TAIL"), "lost the tail");
        assert!(pruned.contains(PRUNE_MARKER));
        assert_eq!(
            pruned.chars().count(),
            PRUNE_HEAD_CHARS + PRUNE_MARKER.chars().count() + PRUNE_TAIL_CHARS
        );
        assert!(report.chars_saved > 0);
    }

    #[test]
    fn pruning_twice_emits_no_second_replacement() {
        let mut messages = padded(vec![result_message("call-1", &"x".repeat(50_000))]);
        assert_eq!(
            prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES).replaced,
            1
        );

        let after_first = serde_json::to_string(&messages).unwrap();
        let second = prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES);
        assert_eq!(
            second.replaced, 0,
            "pruning must converge, or compaction would never summarize"
        );
        assert_eq!(
            serde_json::to_string(&messages).unwrap(),
            after_first,
            "a second pass must not touch a byte"
        );
    }

    #[test]
    fn a_replacement_is_strictly_smaller_than_what_triggered_it() {
        let body = "x".repeat(PRUNE_THRESHOLD_CHARS + 1);
        let replacement = prune_body(&body).expect("over threshold");
        assert!(replacement.chars().count() < body.chars().count());
        assert!(replacement.chars().count() <= PRUNE_THRESHOLD_CHARS);
    }

    #[test]
    fn a_result_at_the_threshold_is_left_alone() {
        assert!(prune_body(&"x".repeat(PRUNE_THRESHOLD_CHARS)).is_none());
        assert!(prune_body(&"x".repeat(PRUNE_THRESHOLD_CHARS + 1)).is_some());
    }

    #[test]
    fn slicing_counts_code_points_not_bytes() {
        // Four bytes each: a byte-indexed slice would land mid-character.
        let body = "🙂".repeat(PRUNE_THRESHOLD_CHARS + 100);
        let replacement = prune_body(&body).expect("over threshold");
        assert_eq!(
            replacement.chars().count(),
            PRUNE_HEAD_CHARS + PRUNE_MARKER.chars().count() + PRUNE_TAIL_CHARS
        );
        assert!(std::str::from_utf8(replacement.as_bytes()).is_ok());
        assert!(replacement.starts_with('🙂') && replacement.ends_with('🙂'));
    }

    #[test]
    fn a_result_whose_body_is_not_a_string_is_left_alone() {
        let mut messages = padded(vec![Message::user_blocks(vec![json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "content": [{ "type": "text", "text": "x".repeat(50_000) }],
            "is_error": false,
        })])]);
        let report = prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES);
        assert_eq!(report.replaced, 0);
        assert!(messages[0].content[0]["content"].is_array());
    }

    #[test]
    fn blocks_that_are_not_tool_results_keep_their_positions() {
        let mut messages = padded(vec![Message::user_blocks(vec![
            json!({ "type": "image", "source": { "data": "AAAA" } }),
            crate::anthropic::types::tool_result("call-1", &"x".repeat(50_000), false),
            json!({ "type": "text", "text": "trailing note" }),
        ])]);
        let report = prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES);

        assert_eq!(report.replaced, 1);
        let blocks = &messages[0].content;
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[1]["type"], "tool_result");
        assert_eq!(blocks[2]["text"], "trailing note");
    }

    #[test]
    fn the_tool_use_pairing_survives() {
        let mut messages = padded(vec![result_message("call-1", &"x".repeat(50_000))]);
        prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES);
        let block = &messages[0].content[0];
        assert_eq!(block["tool_use_id"], "call-1");
        assert_eq!(block["is_error"], false);
        assert_eq!(block["type"], "tool_result");
    }

    #[test]
    fn the_most_recent_messages_are_never_pruned() {
        let big = "x".repeat(50_000);
        let mut messages = vec![
            result_message("old", &big),
            result_message("recent-1", &big),
            result_message("recent-2", &big),
            result_message("recent-3", &big),
            result_message("recent-4", &big),
        ];
        let report = prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES);

        assert_eq!(report.replaced, 1, "only the message past the window");
        assert!(body_of(&messages[0]).contains(PRUNE_MARKER));
        for recent in &messages[1..] {
            assert_eq!(body_of(recent), big, "the live tool round was rewritten");
        }
    }

    #[test]
    fn a_history_shorter_than_the_window_is_untouched() {
        let mut messages = vec![result_message("a", &"x".repeat(50_000))];
        assert_eq!(
            prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES).replaced,
            0
        );
    }

    #[test]
    fn an_assistant_message_is_never_rewritten() {
        let mut messages = padded(vec![Message::assistant(vec![json!({
            "type": "text",
            "text": "x".repeat(50_000),
        })])]);
        assert_eq!(
            prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES).replaced,
            0
        );
        assert_eq!(
            messages[0].content[0]["text"].as_str().unwrap().len(),
            50_000
        );
    }

    #[test]
    fn every_over_long_result_in_one_message_is_pruned() {
        // A parallel tool round puts every result in a single user message.
        let big = "x".repeat(50_000);
        let mut messages = padded(vec![Message::user_blocks(vec![
            crate::anthropic::types::tool_result("call-1", &big, false),
            crate::anthropic::types::tool_result("call-2", "short", false),
            crate::anthropic::types::tool_result("call-3", &big, true),
        ])]);
        let report = prune_tool_results(&mut messages, KEEP_RECENT_MESSAGES);

        assert_eq!(report.replaced, 2);
        assert_eq!(messages[0].content[1]["content"], "short");
        // An error result is still a result; its `is_error` flag is preserved.
        assert_eq!(messages[0].content[2]["is_error"], true);
    }

    #[test]
    fn an_empty_history_reports_nothing() {
        let report = prune_tool_results(&mut [], KEEP_RECENT_MESSAGES);
        assert_eq!(report, PruneReport::default());
        assert_eq!(report.chars_saved, 0);
        assert_eq!(report.tokens_saved_estimate(), 0);
    }
}
