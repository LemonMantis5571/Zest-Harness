//! Markdown -> flat document model. Hand-built because GPUI ships no Markdown and Zed's
//! `markdown` crate is GPL-3.0 and welded to Zed's `theme`/`language`/`ui` crates.
//! Every inline run of text is a `Unit`: the thing that is laid out, selected and copied.

use gpui::{Hsla, SharedString, rgb};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::OnceLock;
use std::time::Instant;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

#[derive(Clone, Debug)]
pub enum SpanKind {
    Bold,
    Italic,
    Strike,
    Code,
    Link(String),
    Color(Hsla),
}

#[derive(Clone)]
pub struct Unit {
    pub text: SharedString,
    pub spans: Vec<(Range<usize>, SpanKind)>,
    /// What copy puts after this unit.
    pub sep: &'static str,
    pub mono: bool,
}

#[derive(Clone)]
pub enum Block {
    Heading { level: u8, unit: usize },
    Para { unit: usize, quote: usize },
    Item { unit: usize, depth: usize, marker: String, quote: usize },
    /// `indent` is the list nesting the block sits in (0 = top level), so code under a list item stays under it.
    Code { lang: String, unit: usize, indent: usize },
    Table { rows: Vec<Vec<usize>>, indent: usize },
    Rule,
}

#[derive(Clone, Default)]
pub struct Doc {
    pub blocks: Vec<Block>,
    pub units: Vec<Unit>,
}

impl Doc {
    pub fn plain(text: &str) -> Doc {
        Doc {
            blocks: vec![Block::Para { unit: 0, quote: 0 }],
            units: vec![Unit { text: text.to_string().into(), spans: vec![], sep: "\n", mono: false }],
        }
    }
}

struct Cur {
    text: String,
    spans: Vec<(Range<usize>, SpanKind)>,
    open: Vec<(usize, SpanKind)>,
}

impl Cur {
    fn new() -> Self {
        Cur { text: String::new(), spans: vec![], open: vec![] }
    }
}

/// The part of a streaming reply made of complete top-level Markdown blocks, which is what the
/// "Stream complete blocks" option shows while a reply streams. A block is complete once a blank line is followed by a new block, its
/// code fence closes, or the text ends in a blank line outside a list or open fence.
pub fn complete_prefix(text: &str) -> &str {
    let mut boundary = 0;
    let mut fence: Option<(char, usize)> = None;
    let mut in_list = false;
    let mut blank_pending = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        if let Some((ch, len)) = fence {
            if fence_marker(content).is_some_and(|(c, l, rest)| c == ch && l >= len && rest.trim().is_empty()) {
                fence = None;
                in_list = false;
                blank_pending = false;
                // A closed fence ends its block only once the line is complete.
                if line.ends_with('\n') {
                    boundary = offset;
                }
            }
            continue;
        }
        if content.trim().is_empty() {
            blank_pending = true;
            continue;
        }
        let indent = content.len() - content.trim_start().len();
        if let Some((ch, len, _)) = fence_marker(content) {
            boundary = start;
            fence = Some((ch, len));
            in_list = false;
            blank_pending = false;
            continue;
        }
        let item = is_list_item(content.trim_start());
        if blank_pending {
            let continues = in_list && (indent >= 2 || item);
            if !continues {
                boundary = start;
                in_list = false;
            }
            blank_pending = false;
        }
        if item {
            in_list = true;
        }
    }
    let ends_blank = text.ends_with("\n\n") || text.ends_with("\r\n\r\n");
    if fence.is_none() && !in_list && ends_blank {
        return text;
    }
    &text[..boundary]
}

/// A fence line (``` or ~~~, at most three spaces of indent): its character, length and the
/// text after the run.
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = rest.chars().take_while(|c| *c == ch).count();
    (len >= 3).then(|| (ch, len, &rest[len..]))
}

fn is_list_item(line: &str) -> bool {
    let bullet = ["- ", "* ", "+ "].iter().any(|b| line.starts_with(b));
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    let ordered = digits > 0
        && digits <= 9
        && (line[digits..].starts_with(". ") || line[digits..].starts_with(") "));
    bullet || ordered
}

#[cfg(test)]
mod block_tests {
    use super::complete_prefix;

    #[::core::prelude::v1::test]
    fn holds_the_growing_paragraph() {
        assert_eq!(complete_prefix("First para.\n\nSecond is gro"), "First para.\n\n");
        assert_eq!(complete_prefix("Only one line so far"), "");
        assert_eq!(complete_prefix("Done.\n\n"), "Done.\n\n");
    }

    #[::core::prelude::v1::test]
    fn code_fences_appear_when_closed() {
        let open = "Intro.\n\n```rust\nfn main() {\n";
        assert_eq!(complete_prefix(open), "Intro.\n\n");
        let closed = "Intro.\n\n```rust\nfn main() {}\n```\nAfter";
        assert_eq!(complete_prefix(closed), "Intro.\n\n```rust\nfn main() {}\n```\n");
        // A shorter or different closing run does not close the fence.
        assert_eq!(complete_prefix("````\ncode\n```\nmore"), "");
        assert_eq!(complete_prefix("~~~\ncode\n```\n"), "");
    }

    #[::core::prelude::v1::test]
    fn lists_continue_across_blank_lines() {
        let list = "- one\n\n- two\n\n  more of two\n\n";
        assert_eq!(complete_prefix(list), "");
        let after = "- one\n- two\n\nNext para";
        assert_eq!(complete_prefix(after), "- one\n- two\n\n");
        assert_eq!(complete_prefix("1. a\n2. b\n\nText\n\n"), "1. a\n2. b\n\nText\n\n");
    }
}

pub fn parse(src: &str) -> Doc {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut doc = Doc::default();
    let mut cur: Option<Cur> = None;
    let mut lists: Vec<Option<u64>> = vec![];
    let mut marker: Option<String> = None;
    let mut quote = 0usize;
    let mut code: Option<(String, String)> = None;
    let mut heading: Option<u8> = None;
    let mut table: Option<Vec<Vec<usize>>> = None;
    let mut row: Vec<usize> = vec![];

    fn push_unit(doc: &mut Doc, c: Cur, sep: &'static str) -> usize {
        doc.units.push(Unit { text: c.text.into(), spans: c.spans, sep, mono: false });
        doc.units.len() - 1
    }

    // Flushes the inline run being built as a list item / paragraph.
    macro_rules! flush_text {
        () => {
            if let Some(c) = cur.take() {
                if !c.text.is_empty() {
                    if !lists.is_empty() {
                        let m = marker.take().unwrap_or_default();
                        let unit = push_unit(&mut doc, c, "\n");
                        doc.blocks.push(Block::Item { unit, depth: lists.len() - 1, marker: m, quote });
                    } else {
                        let unit = push_unit(&mut doc, c, "\n\n");
                        doc.blocks.push(Block::Para { unit, quote });
                    }
                }
            }
        };
    }

    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => cur = Some(Cur::new()),
                Tag::Heading { level, .. } => {
                    heading = Some(match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    });
                    cur = Some(Cur::new());
                }
                Tag::BlockQuote(_) => quote += 1,
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    flush_text!();
                    lists.push(start);
                }
                Tag::Item => {
                    marker = Some(match lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => "\u{2022}".to_string(),
                    });
                }
                Tag::Table(_) => table = Some(vec![]),
                Tag::TableHead | Tag::TableRow => row.clear(),
                Tag::TableCell => cur = Some(Cur::new()),
                Tag::Emphasis => open(&mut cur, SpanKind::Italic),
                Tag::Strong => open(&mut cur, SpanKind::Bold),
                Tag::Strikethrough => open(&mut cur, SpanKind::Strike),
                Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                    open(&mut cur, SpanKind::Link(dest_url.to_string()))
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => flush_text!(),
                TagEnd::Heading(_) => {
                    if let Some(c) = cur.take() {
                        let unit = push_unit(&mut doc, c, "\n\n");
                        doc.blocks.push(Block::Heading { level: heading.take().unwrap_or(1), unit });
                    }
                }
                TagEnd::BlockQuote(_) => quote = quote.saturating_sub(1),
                TagEnd::CodeBlock => {
                    if let Some((lang, mut text)) = code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        let spans = highlight(&lang, &text);
                        doc.units.push(Unit { text: text.into(), spans, sep: "\n\n", mono: true });
                        doc.blocks.push(Block::Code { lang, unit: doc.units.len() - 1, indent: lists.len() });
                    }
                }
                TagEnd::List(_) => {
                    flush_text!();
                    lists.pop();
                }
                TagEnd::Item => flush_text!(),
                TagEnd::TableCell => {
                    if let Some(c) = cur.take() {
                        row.push(push_unit(&mut doc, c, "\t"));
                    }
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some(last) = row.last() {
                        doc.units[*last].sep = "\n";
                    }
                    if let Some(t) = table.as_mut() {
                        t.push(std::mem::take(&mut row));
                    }
                }
                TagEnd::Table => {
                    if let Some(rows) = table.take() {
                        if let Some(last) = rows.last().and_then(|r| r.last()) {
                            doc.units[*last].sep = "\n\n";
                        }
                        doc.blocks.push(Block::Table { rows, indent: lists.len() });
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image => {
                    close(&mut cur)
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, buf)) = code.as_mut() {
                    buf.push_str(&t);
                } else {
                    cur.get_or_insert_with(Cur::new).text.push_str(&t);
                }
            }
            Event::Code(t) => {
                let c = cur.get_or_insert_with(Cur::new);
                let start = c.text.len();
                c.text.push_str(&t);
                c.spans.push((start..c.text.len(), SpanKind::Code));
            }
            Event::SoftBreak => cur.get_or_insert_with(Cur::new).text.push(' '),
            Event::HardBreak => cur.get_or_insert_with(Cur::new).text.push('\n'),
            Event::Html(t) | Event::InlineHtml(t) => cur.get_or_insert_with(Cur::new).text.push_str(&t),
            Event::TaskListMarker(done) => {
                cur.get_or_insert_with(Cur::new).text.push_str(if done { "\u{2611} " } else { "\u{2610} " })
            }
            Event::Rule => doc.blocks.push(Block::Rule),
            _ => {}
        }
    }
    flush_text!();
    doc
}

fn open(cur: &mut Option<Cur>, kind: SpanKind) {
    let c = cur.get_or_insert_with(Cur::new);
    c.open.push((c.text.len(), kind));
}

fn close(cur: &mut Option<Cur>) {
    if let Some(c) = cur.as_mut() {
        if let Some((start, kind)) = c.open.pop() {
            if start < c.text.len() {
                c.spans.push((start..c.text.len(), kind));
            }
        }
    }
}

// ---- syntax highlighting (syntect, pure-Rust regex backend) ----

struct Hl {
    ss: SyntaxSet,
    dark: Theme,
    /// For light palettes such as Oceanic.
    light: Theme,
}

static HL: OnceLock<Hl> = OnceLock::new();

pub fn init_highlighter() -> std::time::Duration {
    let t = Instant::now();
    HL.get_or_init(|| {
        // `--features grammars` swaps in the bat/two-face pack (TypeScript, TSX, TOML, Dockerfile, Kotlin, ...).
        #[cfg(feature = "grammars")]
        let ss = two_face::syntax::extra_newlines();
        #[cfg(not(feature = "grammars"))]
        let ss = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();
        Hl { ss, dark: ts.themes["base16-ocean.dark"].clone(), light: ts.themes["InspiredGitHub"].clone() }
    });
    t.elapsed()
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, Vec<(Range<usize>, SpanKind)>>> = RefCell::new(HashMap::new());
}

pub fn highlight(lang: &str, code: &str) -> Vec<(Range<usize>, SpanKind)> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let dark = crate::theme::palette().dark;
    lang.hash(&mut h);
    code.hash(&mut h);
    dark.hash(&mut h);
    let key = h.finish();
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let Some(hl) = HL.get() else { return vec![] };
    let syntax = find_syntax(&hl.ss, lang).map(|(s, _)| s);
    let mut out = vec![];
    if let Some(syntax) = syntax {
        let mut hlines = HighlightLines::new(syntax, if dark { &hl.dark } else { &hl.light });
        let mut off = 0;
        for line in LinesWithEndings::from(code) {
            if let Ok(ranges) = hlines.highlight_line(line, &hl.ss) {
                for (style, piece) in ranges {
                    let end = off + piece.len();
                    let c = style.foreground;
                    out.push((off..end, SpanKind::Color(rgb(u32::from_be_bytes([0, c.r, c.g, c.b])).into())));
                    off = end;
                }
            } else {
                off += line.len();
            }
        }
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 4096 {
            c.clear();
        }
        c.insert(key, out.clone());
    });
    out
}

/// Copy text for the selection `a..=b` (positions are (unit, byte offset) within one doc).
pub fn unit_slice(u: &Unit, range: Range<usize>) -> &str {
    let len = u.text.len();
    let (s, e) = (range.start.min(len), range.end.min(len));
    u.text.get(s..e).unwrap_or("")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coverage {
    /// The pack has a grammar for the language itself.
    Native,
    /// Coloured with a neighbouring grammar (TypeScript as JavaScript, PowerShell as Bash).
    Approx,
    None,
}

/// Resolves a fence label to a syntect grammar. The first lookup uses the label as the pack knows it
/// (Zest's shiki names such as `csharp` and `docker` map to the tokens `cs` and `dockerfile`).
fn find_syntax<'a>(ss: &'a SyntaxSet, lang: &str) -> Option<(&'a syntect::parsing::SyntaxReference, Coverage)> {
    let token = match lang {
        "csharp" => "cs",
        "docker" => "dockerfile",
        "kotlin" => "kt",
        "typescript" => "ts",
        "sh" | "shell" | "zsh" => "bash",
        "ps1" => "powershell",
        "rs" => "rust",
        "py" => "python",
        l => l,
    };
    if let Some(s) = ss.find_syntax_by_token(token) {
        return Some((s, Coverage::Native));
    }
    let near = match lang {
        "ts" | "tsx" | "typescript" | "jsx" => "js",
        "powershell" | "ps1" => "bash",
        _ => return None,
    };
    ss.find_syntax_by_token(near).map(|s| (s, Coverage::Approx))
}

/// Which of Zest's shiki grammars the loaded syntect pack can colour, natively or approximately.
pub fn language_coverage(tokens: &[&str]) -> Vec<(String, Coverage)> {
    let Some(hl) = HL.get() else { return vec![] };
    tokens
        .iter()
        .map(|t| (t.to_string(), find_syntax(&hl.ss, t).map(|(_, c)| c).unwrap_or(Coverage::None)))
        .collect()
}

/// Number of grammars in the loaded pack.
pub fn grammar_count() -> usize {
    HL.get().map(|h| h.ss.syntaxes().len()).unwrap_or(0)
}
