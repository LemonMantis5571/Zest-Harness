//! Bounding text to a byte ceiling without splitting a character.
//!
//! Every consumer here is protecting a *wire* cost — bytes sent to a model or
//! written to a durable record — so every budget in this module is in UTF-8
//! bytes. The char-budget clippers elsewhere in the crate answer a display-width
//! question instead and deliberately stay where they are; folding both into one
//! module would mean two incompatible APIs sharing a name.
//!
//! The one non-obvious rule is that a truncation notice must be paid for out of
//! the ceiling rather than added to it. A "bounded" result that comes back longer
//! than the ceiling is a limit that does not limit, and a bounded result longer
//! than its own input is a regression wearing a limit's clothes.

/// Largest char-boundary index at or below `index`, clamped to `text.len()`.
///
/// Wraps the std method and clamps explicitly, so the clamping contract belongs
/// to this module rather than to whatever the current standard library happens
/// to do past the end.
pub fn floor_boundary(text: &str, index: usize) -> usize {
    text.floor_char_boundary(index.min(text.len()))
}

/// Smallest char-boundary index at or above `index`, clamped to `text.len()`.
///
/// Private: only [`ends_within`] needs to round *up*, because only a tail start
/// does. Every head-only clipper in the crate rounds down.
fn ceil_boundary(text: &str, index: usize) -> usize {
    text.ceil_char_boundary(index.min(text.len()))
}

/// Keep both ends of `text` inside a hard ceiling of `limit` bytes, with
/// `marker(omitted)` describing the gap and *paid for out of that ceiling*.
///
/// Returns `text` unchanged when it already fits. Returns `None` when no
/// replacement can honor the ceiling — the marker alone does not fit, or the
/// caller's marker grew — and the caller must then keep the original: bounding
/// is not worth breaking the guarantee for.
///
/// `marker` must not grow as `omitted` shrinks. That holds for any decimal
/// rendering of the count and is what makes a single sizing pass sufficient. A
/// marker that violates it is caught rather than trusted, at the cost of one
/// length comparison.
///
/// # Guarantees
///
/// For any returned `Some(out)`: `out.len() <= limit`, `out.len() < text.len()`
/// whenever anything was omitted, and `out` never splits a `char`.
pub fn ends_within(text: &str, limit: usize, marker: impl Fn(usize) -> String) -> Option<String> {
    if text.len() <= limit {
        return Some(text.to_string());
    }

    // The longest marker this call can produce, since `omitted` only ever
    // shrinks from here.
    let reserve = marker(text.len()).len();
    let budget = limit.checked_sub(reserve)?;

    // A zero budget is not a failure: the marker alone still fits the ceiling
    // and still tells the model what happened, so it is emitted on its own.
    let head_end = floor_boundary(text, budget / 2);
    let tail_start = ceil_boundary(text, text.len() - (budget - budget / 2));
    let omitted = tail_start.saturating_sub(head_end);

    let mut out = String::with_capacity(limit);
    out.push_str(&text[..head_end]);
    out.push_str(&marker(omitted));
    out.push_str(&text[tail_start..]);

    // Cheap insurance against a non-monotonic marker. Returning the original is
    // always safe; returning something over the ceiling never is.
    (out.len() <= limit).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(omitted: usize) -> String {
        format!("[{omitted} omitted]")
    }

    #[test]
    fn text_within_the_limit_is_returned_whole() {
        assert_eq!(ends_within("hello", 64, marker).unwrap(), "hello");
        // Exactly at the limit is within it.
        assert_eq!(ends_within("hello", 5, marker).unwrap(), "hello");
    }

    #[test]
    fn a_clipped_result_never_exceeds_the_limit() {
        let text = "x".repeat(10_000);
        for limit in [20, 33, 64, 100, 999, 4_096] {
            let out = ends_within(&text, limit, marker).unwrap();
            assert!(out.len() <= limit, "limit {limit} produced {}", out.len());
        }
    }

    #[test]
    fn the_marker_is_paid_for_out_of_the_limit() {
        let text = "x".repeat(1_000);
        // Room for the marker plus a little payload, and no more.
        let limit = marker(1_000).len() + 10;
        let out = ends_within(&text, limit, marker).unwrap();
        assert!(out.len() <= limit, "{}", out.len());
        assert!(
            out.starts_with("xxx") && out.ends_with("xxx"),
            "some payload from both ends must survive: {out}"
        );
    }

    #[test]
    fn both_ends_survive_a_clip() {
        let text = format!("HEAD{}TAIL", "x".repeat(5_000));
        let out = ends_within(&text, 200, marker).unwrap();
        assert!(out.starts_with("HEAD"), "{out}");
        assert!(out.ends_with("TAIL"), "{out}");
        assert!(out.contains("omitted"), "{out}");
    }

    #[test]
    fn a_replacement_is_always_smaller_than_the_original() {
        let text = "x".repeat(10_000);
        let out = ends_within(&text, 4_096, marker).unwrap();
        assert!(out.len() < text.len());
    }

    #[test]
    fn a_marker_that_cannot_fit_returns_none() {
        let text = "x".repeat(1_000);
        // The marker alone is longer than the whole ceiling.
        assert!(ends_within(&text, 4, marker).is_none());
        assert!(ends_within(&text, 0, marker).is_none());
    }

    #[test]
    fn a_budget_of_exactly_the_marker_emits_the_marker_alone() {
        let text = "x".repeat(1_000);
        let limit = marker(1_000).len();
        let out = ends_within(&text, limit, marker).unwrap();
        assert_eq!(out, marker(1_000));
        assert!(out.len() <= limit);
    }

    #[test]
    fn a_non_monotonic_marker_is_refused_rather_than_trusted() {
        let text = "x".repeat(1_000);
        // Shorter count, longer marker — the opposite of the documented rule.
        let bad = |omitted: usize| "!".repeat(2_000 - omitted);
        assert!(ends_within(&text, 500, bad).is_none());
    }

    #[test]
    fn clipping_never_splits_a_codepoint() {
        // Two bytes per char, so every odd split lands mid-character.
        let text = "é".repeat(5_000);
        for limit in [21, 22, 23, 64, 65, 501] {
            let out = ends_within(&text, limit, marker).unwrap();
            assert!(out.len() <= limit);
            assert!(
                std::str::from_utf8(out.as_bytes()).is_ok(),
                "limit {limit} split a codepoint"
            );
        }
    }

    #[test]
    fn an_astral_codepoint_is_never_split() {
        let text = "🙂".repeat(5_000); // four bytes each
        let out = ends_within(&text, 64, marker).unwrap();
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        assert!(out.starts_with('🙂') || out.starts_with('['), "{out}");
    }

    #[test]
    fn a_boundary_index_past_the_end_is_clamped() {
        let text = "hé";
        assert_eq!(text.len(), 3);
        assert_eq!(floor_boundary(text, 99), 3);
        assert_eq!(ceil_boundary(text, 99), 3);
    }

    #[test]
    fn a_boundary_inside_a_codepoint_moves_the_expected_way() {
        let text = "héllo"; // 'é' occupies bytes 1..3
        assert_eq!(floor_boundary(text, 2), 1);
        assert_eq!(ceil_boundary(text, 2), 3);
        // An index already on a boundary does not move.
        assert_eq!(floor_boundary(text, 3), 3);
        assert_eq!(ceil_boundary(text, 3), 3);
    }

    #[test]
    fn an_empty_string_is_bounded_by_any_limit() {
        assert_eq!(ends_within("", 0, marker).unwrap(), "");
        assert_eq!(floor_boundary("", 5), 0);
        assert_eq!(ceil_boundary("", 5), 0);
    }
}
