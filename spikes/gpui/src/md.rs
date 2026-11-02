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
    Code { lang: String, unit: usize },
    Table { rows: Vec<Vec<usize>> },
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
                        doc.blocks.push(Block::Code { lang, unit: doc.units.len() - 1 });
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
                        doc.blocks.push(Block::Table { rows });
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
    theme: Theme,
}

static HL: OnceLock<Hl> = OnceLock::new();

pub fn init_highlighter() -> std::time::Duration {
    let t = Instant::now();
    HL.get_or_init(|| {
        let ss = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();
        Hl { ss, theme: ts.themes["base16-ocean.dark"].clone() }
    });
    t.elapsed()
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, Vec<(Range<usize>, SpanKind)>>> = RefCell::new(HashMap::new());
}

pub fn highlight(lang: &str, code: &str) -> Vec<(Range<usize>, SpanKind)> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lang.hash(&mut h);
    code.hash(&mut h);
    let key = h.finish();
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let Some(hl) = HL.get() else { return vec![] };
    let lang = match lang {
        "ts" | "tsx" | "typescript" | "jsx" => "js",
        "sh" | "shell" | "zsh" | "powershell" | "ps1" => "bash",
        "rs" => "rust",
        "py" => "python",
        l => l,
    };
    let syntax = hl.ss.find_syntax_by_token(lang);
    let mut out = vec![];
    if let Some(syntax) = syntax {
        let mut hlines = HighlightLines::new(syntax, &hl.theme);
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

/// Which of Zest's shiki grammars the bundled syntect pack can colour.
pub fn language_coverage(tokens: &[&str]) -> Vec<(String, bool)> {
    let Some(hl) = HL.get() else { return vec![] };
    tokens.iter().map(|t| (t.to_string(), hl.ss.find_syntax_by_token(t).is_some())).collect()
}
