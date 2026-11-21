//! Selectable text element, cross-block selection state and the Markdown -> element renderer.

use crate::md::{Block, Doc, SpanKind, Unit};
use gpui::prelude::*;
use gpui::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

// Short names for the theme tokens the views use most.
pub use crate::theme::{BORDER, LINK, MONO, SANS};
pub const BG: u32 = crate::theme::CANVAS;
pub const FG: u32 = crate::theme::FOREGROUND;
pub const MUTED: u32 = crate::theme::MUTED_FOREGROUND;
pub const PANEL: u32 = crate::theme::CARD;
/// Text selection: the primary accent dimmed onto the canvas.
pub const SEL_BG: u32 = 0x2b3060;
pub const CODE_FG: u32 = crate::theme::SECONDARY_FOREGROUND;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub msg: usize,
    pub unit: usize,
    pub off: usize,
}

pub struct Hit {
    pub layout: TextLayout,
    pub bounds: Bounds<Pixels>,
}

#[derive(Default)]
pub struct Shared {
    pub anchor: Option<Pos>,
    pub focus: Option<Pos>,
    pub dragging: bool,
    pub down_at: Option<Point<Pixels>>,
    /// Units painted this frame. Off-screen units are absent (the list is virtualized), so
    /// hit-testing uses this and copy uses the document model.
    pub hits: HashMap<(usize, usize), Hit>,
    /// Painted bounds of trace rows, by message index. `ListState::bounds_for_item` returns `None`
    /// for every item while the list follows the tail, so anything that must know where a row is
    /// at the tail has to record it at paint time.
    pub rows: HashMap<usize, Bounds<Pixels>>,
}

pub type SharedRef = Rc<RefCell<Shared>>;

impl Shared {
    pub fn ordered(&self) -> Option<(Pos, Pos)> {
        let (a, b) = (self.anchor?, self.focus?);
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    pub fn range_for(&self, msg: usize, unit: usize, len: usize) -> Option<Range<usize>> {
        let (a, b) = self.ordered()?;
        let k = (msg, unit);
        if k < (a.msg, a.unit) || k > (b.msg, b.unit) {
            return None;
        }
        let s = if k == (a.msg, a.unit) { a.off } else { 0 };
        let e = if k == (b.msg, b.unit) { b.off } else { len };
        (s < e).then_some(s..e.min(len))
    }

    fn pos_in(hit: &Hit, key: (usize, usize), p: Point<Pixels>) -> Pos {
        let off = if p.y > hit.bounds.bottom() {
            hit.layout.len()
        } else {
            match hit.layout.index_for_position(p) {
                Ok(i) | Err(i) => i,
            }
        };
        Pos { msg: key.0, unit: key.1, off }
    }

    /// Nearest painted unit to `p`, with the byte offset under it. Used while dragging.
    pub fn pos_at(&self, p: Point<Pixels>) -> Option<Pos> {
        let mut best: Option<(f32, (usize, usize), &Hit)> = None;
        for (k, h) in &self.hits {
            let dy = (h.bounds.top() - p.y).max(p.y - h.bounds.bottom()).max(px(0.));
            let dx = (h.bounds.left() - p.x).max(p.x - h.bounds.right()).max(px(0.));
            let d = dy.as_f32() * 4. + dx.as_f32();
            if best.as_ref().is_none_or(|b| d < b.0) {
                best = Some((d, *k, h));
            }
        }
        best.map(|(_, k, h)| Self::pos_in(h, k, p))
    }

    /// Only when the pointer is actually over text (link clicks).
    pub fn exact_pos_at(&self, p: Point<Pixels>) -> Option<Pos> {
        self.hits.iter().find_map(|(k, h)| {
            if !h.bounds.contains(&p) {
                return None;
            }
            h.layout.index_for_position(p).ok().map(|off| Pos { msg: k.0, unit: k.1, off })
        })
    }
}

/// Wraps `StyledText` so each paint records the layout for hit-testing.
pub struct SelText {
    pub inner: StyledText,
    pub key: (usize, usize),
    pub shared: SharedRef,
}

impl IntoElement for SelText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SelText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.inner.request_layout(id, inspector_id, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.prepaint(id, inspector_id, bounds, state, window, cx)
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        rl: &mut (),
        pp: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(id, inspector_id, bounds, rl, pp, window, cx);
        self.shared
            .borrow_mut()
            .hits
            .insert(self.key, Hit { layout: self.inner.layout().clone(), bounds });
    }
}

fn styled(unit: &Unit, sel: Option<Range<usize>>) -> StyledText {
    let len = unit.text.len();
    let mut cuts = vec![0, len];
    for (r, _) in &unit.spans {
        cuts.push(r.start.min(len));
        cuts.push(r.end.min(len));
    }
    if let Some(s) = &sel {
        cuts.push(s.start);
        cuts.push(s.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut highlights = vec![];
    let mut mono_ranges = vec![];
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        let mut hs = HighlightStyle::default();
        let mut touched = false;
        let mut mono = false;
        for (r, k) in &unit.spans {
            if r.start <= a && r.end >= b {
                touched = true;
                match k {
                    SpanKind::Bold => hs.font_weight = Some(FontWeight::SEMIBOLD),
                    SpanKind::Italic => hs.font_style = Some(FontStyle::Italic),
                    SpanKind::Strike => {
                        hs.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None })
                    }
                    SpanKind::Code => {
                        hs.background_color = Some(rgb(crate::theme::SECONDARY).into());
                        hs.color = Some(rgb(crate::theme::FOREGROUND).into());
                        mono = true;
                    }
                    SpanKind::Link(_) => {
                        hs.color = Some(rgb(LINK).into());
                    }
                    SpanKind::Color(c) => hs.color = Some(*c),
                }
            }
        }
        if let Some(s) = &sel {
            if s.start <= a && s.end >= b {
                hs.background_color = Some(rgb(SEL_BG).into());
                touched = true;
            }
        }
        if touched {
            highlights.push((a..b, hs));
        }
        if mono && !unit.mono {
            mono_ranges.push((a..b, SharedString::from(MONO)));
        }
    }
    let mut st = StyledText::new(unit.text.clone()).with_highlights(highlights);
    if !mono_ranges.is_empty() {
        st = st.with_font_family_overrides(mono_ranges);
    }
    st
}

fn unit_el(doc: &Doc, unit: usize, msg: usize, shared: &SharedRef) -> SelText {
    let u = &doc.units[unit];
    let sel = shared.borrow().range_for(msg, unit, u.text.len());
    SelText { inner: styled(u, sel), key: (msg, unit), shared: shared.clone() }
}

fn quoted(el: Div, quote: usize) -> Div {
    if quote > 0 {
        el.border_l_2().border_color(rgb(BORDER)).pl_3().text_color(rgb(MUTED))
    } else {
        el
    }
}

/// Left edge of a list item's text: its nesting indent, the 20 px marker column and the 8 px gap.
fn list_text_offset(indent: usize) -> f32 {
    18. * indent.saturating_sub(1) as f32 + 28.
}

pub fn render_blocks(doc: &Rc<Doc>, msg: usize, shared: &SharedRef) -> Vec<AnyElement> {
    let mut out = vec![];
    let mut previous_item = false;
    for block in &doc.blocks {
        // Markdown spacing from the React renderer: 4 px between list items, 12 px elsewhere.
        let item = matches!(block, Block::Item { .. });
        let gap = if out.is_empty() { 0. } else if item && previous_item { 4. } else { 12. };
        previous_item = item;
        let el: AnyElement = match block {
            Block::Heading { level, unit } => {
                let size = match level {
                    1 => 16.,
                    2 => 15.,
                    _ => 14.,
                };
                div()
                    .mt_2()
                    .text_size(px(size))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(unit_el(doc, *unit, msg, shared))
                    .into_any_element()
            }
            Block::Para { unit, quote } => {
                quoted(div(), *quote).child(unit_el(doc, *unit, msg, shared)).into_any_element()
            }
            Block::Item { unit, depth, marker, quote } => quoted(div(), *quote)
                .flex()
                .flex_row()
                .gap_2()
                .pl(px(18. * *depth as f32))
                .child(div().w(px(20.)).flex_shrink_0().text_color(rgb(MUTED)).child(marker.clone()))
                .child(div().flex_1().min_w_0().child(unit_el(doc, *unit, msg, shared)))
                .into_any_element(),
            Block::Code { lang, unit, indent } => {
                let text = doc.units[*unit].text.to_string();
                div()
                    .when(*indent > 0, |d| d.ml(px(list_text_offset(*indent))))
                    .my_1()
                    .rounded(px(8.))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .overflow_hidden()
                    .child(
                        div()
                            .min_h(px(36.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(0x18191a))
                                    .font_family(MONO)
                                    .text_size(px(11.))
                                    .line_height(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(if lang.is_empty() { "TEXT".to_string() } else { lang.to_uppercase() }),
                            )
                            .child(div().flex_1())
                            .child(crate::ui::icon_button(
                                ("copy-code", msg * 10_000 + *unit),
                                crate::theme::Icon::Copy,
                                "Copy code",
                                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())),
                            )),
                    )
                    .child(
                        div()
                            .id(("code-scroll", msg * 10_000 + *unit))
                            .overflow_x_scroll()
                            .p_3()
                            .font_family(MONO)
                            .text_size(px(13.))
                            .line_height(px(24.))
                            .child(div().flex_none().whitespace_nowrap().child(unit_el(doc, *unit, msg, shared))),
                    )
                    .into_any_element()
            }
            Block::Table { rows, indent } => div()
                .when(*indent > 0, |d| d.ml(px(list_text_offset(*indent))))
                .flex()
                .flex_col()
                .rounded(px(12.))
                .border_1()
                .border_color(rgb(0x25262b))
                .bg(rgb(0x111213))
                .overflow_hidden()
                .text_size(px(13.))
                .children(rows.iter().enumerate().map(|(ri, cells)| {
                    div()
                        .flex()
                        .flex_row()
                        .when(ri == 0, |d| {
                            d.bg(rgb(0x18191b))
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(MUTED))
                                .border_b_1()
                                .border_color(rgb(BORDER))
                        })
                        .when(ri > 1, |d| d.border_t_1().border_color(rgb(0x1f2125)))
                        .children(cells.iter().enumerate().map(|(ci, u)| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_3()
                                .py(px(if ri == 0 { 8. } else { 10. }))
                                .when(ri > 0 && ci > 0, |d| d.text_color(rgb(MUTED)))
                                .child(unit_el(doc, *u, msg, shared))
                        }))
                }))
                .into_any_element(),
            Block::Rule => div().h(px(1.)).my_4().bg(rgb(BORDER)).into_any_element(),
        };
        out.push(div().mt(px(gap)).child(el).into_any_element());
    }
    out
}

pub fn render_user(doc: &Rc<Doc>, msg: usize, shared: &SharedRef, attachments: usize) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(6.))
        .child(
            div()
                .max_w(relative(0.85))
                .rounded(px(12.))
                .bg(rgb(crate::theme::USER_BUBBLE))
                .px_3()
                .py_2()
                .text_size(px(13.5))
                .line_height(px(22.))
                .child(unit_el(doc, 0, msg, shared)),
        )
        .when(attachments > 0, |d| {
            d.child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(format!("{attachments} image attachment(s)")))
        })
        .into_any_element()
}

pub fn render_assistant(doc: &Rc<Doc>, msg: usize, shared: &SharedRef, live: bool) -> AnyElement {
    div()
        .w_full()
        .child(
            div()
                .flex()
                .flex_col()
                .text_size(px(15.))
                .line_height(px(25.))
                .children(render_blocks(doc, msg, shared))
                .when(live, |d| d.child(div().w(px(6.)).h(px(16.)).bg(crate::theme::alpha(FG, 0.7)))),
        )
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
}

#[derive(Clone, Debug)]
pub enum TraceKind {
    Recorded { label: String, status: String },
    Thinking { live: bool, secs: f32 },
    Tool { name: String, id: String, status: ToolStatus, secs: f32 },
}

/// One collapsible row of an agent turn (reasoning or a tool call). Its body is an ordinary `Doc`, so
/// selection, copy and highlighting work in it with no extra code.
#[derive(Clone, Debug)]
pub struct Trace {
    pub kind: TraceKind,
    /// One-line description shown in the header (first line of the tool result).
    pub title: String,
    pub open: bool,
}

pub fn render_trace(
    msg: usize,
    tr: &Trace,
    doc: &Rc<Doc>,
    shared: &SharedRef,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    use crate::theme::{self, Icon};
    let (label, badge, badge_color, glyph) = match &tr.kind {
        TraceKind::Recorded { label, status } => {
            let failed = matches!(status.as_str(), "error" | "failed");
            (
                label.clone(),
                status.clone(),
                if failed { theme::DESTRUCTIVE } else { MUTED },
                if failed { Icon::X } else { trace_icon(label) },
            )
        }
        TraceKind::Thinking { live: true, .. } => ("Thinking".to_string(), String::new(), MUTED, Icon::Brain),
        TraceKind::Thinking { secs, .. } => ("Thought".to_string(), format!("for {secs:.0}s"), MUTED, Icon::Brain),
        TraceKind::Tool { name, status, secs, .. } => (
            name.clone(),
            match status {
                ToolStatus::Running => "running\u{2026}".to_string(),
                ToolStatus::Ok | ToolStatus::Error => format!("{secs:.1}s"),
            },
            match status {
                ToolStatus::Error => theme::DESTRUCTIVE,
                _ => MUTED,
            },
            match status {
                ToolStatus::Error => Icon::X,
                _ => trace_icon(name),
            },
        ),
    };
    let failed = badge_color == theme::DESTRUCTIVE;
    let has_body = !doc.units.is_empty();
    let sh = shared.clone();
    let group = SharedString::from(format!("trace-{msg}"));
    div()
        .w_full()
        .relative()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    sh.borrow_mut().rows.insert(msg, bounds);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(
            div()
                .id(("trace", msg))
                .group(group.clone())
                .role(Role::Button).aria_label(format!("{label} {}", tr.title)).aria_expanded(tr.open).tab_index(0)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .text_size(px(14.))
                .text_color(rgb(if failed { theme::DESTRUCTIVE } else { MUTED }))
                .hover(|s| s.text_color(rgb(FG)))
                .child(
                    theme::icon(glyph, 16.)
                        .text_color(rgb(if failed { theme::DESTRUCTIVE } else { MUTED }))
                        .group_hover(group.clone(), |s| s.text_color(rgb(FG))),
                )
                .child(div().flex_none().child(label))
                .child(div().min_w_0().truncate().font_family(MONO).text_size(px(13.)).child(tr.title.clone()))
                .child(div().flex_none().text_size(px(12.)).text_color(rgb(badge_color)).child(badge))
                .when(has_body, |d| {
                    d.child(
                        theme::icon(if tr.open { Icon::ChevronDown } else { Icon::ChevronRight }, 16.)
                            .group_hover(group, |s| s.text_color(rgb(FG))),
                    )
                })
                .on_click(on_toggle),
        )
        .when(tr.open && has_body, |d| {
            d.child(
                div()
                    .ml(px(7.))
                    .mt_2()
                    .mb_1()
                    .pl_4()
                    .pr_1()
                    .border_l_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .line_height(px(21.))
                    .children(render_blocks(doc, msg, shared)),
            )
        })
        .into_any_element()
}

/// The React UI's icon for a tool or recorded step.
fn trace_icon(name: &str) -> crate::theme::Icon {
    use crate::theme::Icon;
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "bash" | "shell" | "run_command" | "command" => Icon::Terminal,
        "edit_file" | "write_file" | "edit" | "write" | "apply_patch" => Icon::Pencil,
        "glob" | "grep" | "search" => Icon::Search,
        "read_file" | "read" => Icon::FileText,
        "list_dir" | "ls" => Icon::Files,
        "web_search" | "web_fetch" | "browser" => Icon::Globe,
        "delegate_external" | "delegate" => Icon::Sparkles,
        "thought" | "thinking" => Icon::Brain,
        "provider" => Icon::Activity,
        _ if name.starts_with("mcp") => Icon::Zap,
        _ if name.starts_with("browser") => Icon::Globe,
        _ => Icon::Wrench,
    }
}
