//! Find `edit_file`'s `old_string` in a real file and splice the replacement in.
//!
//! Models quote text from `read_file`, which hides carriage returns, and they
//! retype punctuation from memory. A byte-exact search fails on both, and the
//! model then retries the same quote in a loop. So matching runs on a view of
//! the file with CRLF folded to LF. Looser views are tried only after that
//! fails, and only accepted when they still point at exactly one place. Every
//! match maps back to a raw byte range, so all bytes outside an edit stay as
//! they were, and the replacement takes the line endings of the text it
//! replaces.

use std::ops::Range;

use similar::TextDiff;

/// Which comparison found the match. Each one also ignores everything the
/// ones before it ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchKind {
    /// Same text, ignoring only CRLF versus LF.
    Exact,
    /// Also ignores curly quotes, Unicode dashes and non-breaking spaces.
    Punctuation,
    /// Also ignores whitespace at the end of each line.
    TrailingWhitespace,
}

impl MatchKind {
    /// What this comparison ignored, for messages. Empty for [`Exact`](Self::Exact).
    pub(crate) fn ignored(self) -> &'static str {
        match self {
            Self::Exact => "",
            Self::Punctuation => "curly quotes, dashes and non-breaking spaces",
            Self::TrailingWhitespace => {
                "curly quotes, dashes, non-breaking spaces and trailing whitespace"
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Located {
    /// Raw byte ranges in the file, sorted and not overlapping.
    Found {
        ranges: Vec<Range<usize>>,
        kind: MatchKind,
    },
    /// More than one place matched where exactly one was required.
    Ambiguous {
        count: usize,
        kind: MatchKind,
    },
    Missing,
}

/// Find `needle` in `raw`. An exact match may hit several places when
/// `replace_all` is set; a fallback match must always hit exactly one.
pub(crate) fn locate(raw: &str, needle: &str, replace_all: bool) -> Located {
    let exact = Folded::new(raw, MatchKind::Exact);
    let exact_needle = fold_text(needle, MatchKind::Exact);
    if !exact_needle.is_empty() {
        let starts: Vec<usize> = exact
            .text
            .match_indices(exact_needle.as_str())
            .map(|(at, _)| at)
            .collect();
        match starts.len() {
            0 => {}
            n if n > 1 && !replace_all => {
                return Located::Ambiguous {
                    count: n,
                    kind: MatchKind::Exact,
                }
            }
            _ => {
                return Located::Found {
                    ranges: starts
                        .iter()
                        .map(|&at| exact.source_range(at, exact_needle.len()))
                        .collect(),
                    kind: MatchKind::Exact,
                }
            }
        }
    }

    for kind in [MatchKind::Punctuation, MatchKind::TrailingWhitespace] {
        let folded_needle = fold_text(needle, kind);
        if folded_needle.is_empty() {
            continue;
        }
        let folded = Folded::new(raw, kind);
        let Some(first) = folded.text.find(folded_needle.as_str()) else {
            continue;
        };
        // Overlapping hits count too: a loose match has to be unmistakable.
        let next = first
            + folded.text[first..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
        if folded.text[next..].contains(folded_needle.as_str()) {
            return Located::Ambiguous {
                count: folded.text.matches(folded_needle.as_str()).count().max(2),
                kind,
            };
        }
        return Located::Found {
            ranges: vec![folded.source_range(first, folded_needle.len())],
            kind,
        };
    }
    Located::Missing
}

/// Replace each range of `raw` with `replacement`, written in the line ending
/// of the text it replaces (see [`line_ending_for`]).
pub(crate) fn splice(raw: &str, ranges: &[Range<usize>], replacement: &str) -> String {
    let lf = replacement.replace("\r\n", "\n");
    let crlf = lf.replace('\n', "\r\n");
    let mut out = String::with_capacity(raw.len() + ranges.len() * crlf.len());
    let mut cursor = 0;
    for range in ranges {
        out.push_str(&raw[cursor..range.start]);
        out.push_str(if line_ending_for(raw, range) == "\r\n" {
            &crlf
        } else {
            &lf
        });
        cursor = range.end;
    }
    out.push_str(&raw[cursor..]);
    out
}

/// The line ending a replacement for `range` should use: whichever ending is
/// more common inside the range (CRLF on a tie), else the ending of the line
/// the range sits on, else the line before it, else LF.
fn line_ending_for(raw: &str, range: &Range<usize>) -> &'static str {
    let inside = &raw[range.clone()];
    let newlines = inside.matches('\n').count();
    if newlines > 0 {
        let crlf = inside.matches("\r\n").count();
        return if crlf * 2 >= newlines { "\r\n" } else { "\n" };
    }
    let ending_at = |newline: usize| {
        if newline > 0 && raw.as_bytes()[newline - 1] == b'\r' {
            "\r\n"
        } else {
            "\n"
        }
    };
    if let Some(offset) = raw[range.end..].find('\n') {
        return ending_at(range.end + offset);
    }
    raw[..range.start].rfind('\n').map_or("\n", ending_at)
}

/// Read_file-style numbered lines around the place in `raw` that looks most
/// like `needle`, or `None` when nothing is close enough to help.
pub(crate) fn closest_lines(raw: &str, needle: &str) -> Option<String> {
    const MIN_SCORE: f32 = 0.6;
    const SCORE_CHARS: usize = 400;
    const SHOW_LINE_CHARS: usize = 240;
    const MAX_SHOWN: usize = 24;

    let needle = needle.replace("\r\n", "\n");
    let needle_lines: Vec<&str> = needle.lines().collect();
    // Anchor on the most distinctive (longest) line of the quote.
    let (anchor_at, anchor) = needle_lines
        .iter()
        .map(|line| fold_text(line.trim(), MatchKind::Punctuation))
        .enumerate()
        .max_by_key(|(_, line)| line.len())?;
    if anchor.is_empty() {
        return None;
    }
    let anchor = clip(&anchor, SCORE_CHARS).to_string();

    let lines: Vec<&str> = raw
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let mut best: Option<(usize, f32)> = None;
    for (index, line) in lines.iter().enumerate() {
        let candidate = fold_text(line.trim(), MatchKind::Punctuation);
        let candidate = clip(&candidate, SCORE_CHARS);
        if candidate.is_empty() {
            continue;
        }
        // The diff ratio can never beat 2·min/(a+b); skip lines that cannot win.
        let (a, b) = (candidate.chars().count(), anchor.chars().count());
        let ceiling = 2.0 * a.min(b) as f32 / (a + b) as f32;
        let floor = best.map_or(MIN_SCORE, |(_, score)| score.max(MIN_SCORE));
        if ceiling < floor {
            continue;
        }
        let score = TextDiff::from_chars(candidate, anchor.as_str()).ratio();
        if score >= floor && best.is_none_or(|(_, top)| score > top) {
            best = Some((index, score));
        }
    }
    let (hit, _) = best?;

    let first = hit.saturating_sub(anchor_at).saturating_sub(1);
    let last = (first + needle_lines.len() + 2)
        .min(lines.len())
        .min(first + MAX_SHOWN);
    let mut out = String::new();
    for (offset, line) in lines[first..last].iter().enumerate() {
        let shown = clip(line, SHOW_LINE_CHARS);
        let ellipsis = if shown.len() < line.len() { "…" } else { "" };
        out.push_str(&format!("{:>6}\t{shown}{ellipsis}\n", first + offset + 1));
    }
    Some(out)
}

fn clip(text: &str, chars: usize) -> &str {
    text.char_indices()
        .nth(chars)
        .map_or(text, |(end, _)| &text[..end])
}

/// `raw` folded for one [`MatchKind`], remembering which raw bytes produced
/// each folded byte so a match can be mapped back.
struct Folded {
    text: String,
    /// For each byte of `text`, the raw byte range of the unit that produced it.
    starts: Vec<u32>,
    ends: Vec<u32>,
}

impl Folded {
    fn new(raw: &str, kind: MatchKind) -> Self {
        let mut folded = Self {
            text: String::with_capacity(raw.len()),
            starts: Vec::with_capacity(raw.len()),
            ends: Vec::with_capacity(raw.len()),
        };
        fold(raw, kind, |c, range| {
            let before = folded.text.len();
            folded.text.push(c);
            for _ in before..folded.text.len() {
                folded.starts.push(range.start as u32);
                folded.ends.push(range.end as u32);
            }
        });
        folded
    }

    fn source_range(&self, at: usize, len: usize) -> Range<usize> {
        self.starts[at] as usize..self.ends[at + len - 1] as usize
    }
}

fn fold_text(text: &str, kind: MatchKind) -> String {
    let mut out = String::with_capacity(text.len());
    fold(text, kind, |c, _| out.push(c));
    out
}

/// Walk `raw`, emitting each folded char with the raw byte range it stands
/// for. CRLF becomes one `\n` covering both bytes, so a match can never split
/// a line ending. Trailing whitespace is dropped without a trace, leaving it
/// outside any range that ends at a line's last visible char.
fn fold(raw: &str, kind: MatchKind, mut emit: impl FnMut(char, Range<usize>)) {
    let strip_trailing = kind == MatchKind::TrailingWhitespace;
    let mut pending: Vec<(char, Range<usize>)> = Vec::new();
    let mut chars = raw.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let (c, range) = if c == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
            chars.next();
            ('\n', at..at + 2)
        } else {
            (c, at..at + c.len_utf8())
        };
        let c = if kind == MatchKind::Exact {
            c
        } else {
            fold_punctuation(c)
        };
        if strip_trailing && c != '\n' && c != '\r' && c.is_whitespace() {
            pending.push((c, range));
            continue;
        }
        if c != '\n' {
            for (space, range) in pending.drain(..) {
                emit(space, range);
            }
        }
        pending.clear();
        emit(c, range);
    }
}

fn fold_punctuation(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(raw: &str, needle: &str) -> (Vec<Range<usize>>, MatchKind) {
        match locate(raw, needle, false) {
            Located::Found { ranges, kind } => (ranges, kind),
            other => panic!("expected a match, got {other:?}"),
        }
    }

    #[test]
    fn a_multi_line_quote_matches_crlf_text_and_keeps_its_endings() {
        let raw = "a\r\nb\r\nc\r\n";
        let (ranges, kind) = found(raw, "a\nb\n");
        assert_eq!(kind, MatchKind::Exact);
        assert_eq!(ranges, vec![0..6]);
        assert_eq!(splice(raw, &ranges, "x\ny\n"), "x\r\ny\r\nc\r\n");
    }

    #[test]
    fn a_match_never_splits_a_crlf() {
        let raw = "a\r\nb\r\n";
        let (ranges, _) = found(raw, "\nb");
        assert_eq!(ranges, vec![1..4]);
        assert_eq!(splice(raw, &ranges, "\nc"), "a\r\nc\r\n");
    }

    #[test]
    fn a_single_line_replacement_takes_the_ending_of_its_line() {
        assert_eq!(line_ending_for("x\r\nab\ny", &(3..5)), "\n");
        assert_eq!(line_ending_for("x\nab\r\ny", &(2..4)), "\r\n");
        assert_eq!(line_ending_for("x\r\nab", &(3..5)), "\r\n");
        assert_eq!(line_ending_for("ab", &(0..2)), "\n");
    }

    #[test]
    fn trailing_whitespace_fold_keeps_the_last_lines_spaces_outside_the_range() {
        let raw = "fn a() {   \n    x();  \n}\n";
        let (ranges, kind) = found(raw, "fn a() {\n    x();");
        assert_eq!(kind, MatchKind::TrailingWhitespace);
        assert_eq!(&raw[ranges[0].clone()], "fn a() {   \n    x();");
    }

    #[test]
    fn an_empty_folded_quote_never_matches() {
        // Only trailing spaces: nothing is left to anchor a loose match on.
        assert_eq!(locate("a \nb", "  ", false), Located::Missing);
    }

    #[test]
    fn overlapping_loose_matches_are_ambiguous() {
        assert!(matches!(
            locate("a\u{2014}a\u{2014}a", "a-a", false),
            Located::Ambiguous {
                kind: MatchKind::Punctuation,
                ..
            }
        ));
    }

    #[test]
    fn closest_lines_points_at_the_nearest_text() {
        let raw = "one\ntwo\nfn compute_total(items: &[Item]) -> u32 {\nthree\n";
        let hint = closest_lines(raw, "fn compute_totals(items: &[Items]) -> u64 {").unwrap();
        assert!(hint.contains("     3\tfn compute_total("), "{hint}");
        assert!(closest_lines(raw, "completely unrelated words here").is_none());
    }
}
