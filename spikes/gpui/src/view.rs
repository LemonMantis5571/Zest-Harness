//! Selectable text element, cross-block selection state and the Markdown -> element renderer.

use crate::md::{Block, Doc, SpanKind, Unit};
use gpui::prelude::*;
use gpui::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

pub const BG: u32 = 0x15161a;
pub const FG: u32 = 0xd7dae0;
pub const MUTED: u32 = 0x8a8f9c;
pub const PANEL: u32 = 0x1d1f26;
pub const BORDER: u32 = 0x2c2f3a;
pub const LINK: u32 = 0x6cb0ff;
pub const SEL_BG: u32 = 0x2f5fa8;
pub const CODE_FG: u32 = 0xf0b072;
pub const SANS: &str = "Segoe UI";
pub const MONO: &str = "Consolas";

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
                    SpanKind::Bold => hs.font_weight = Some(FontWeight::BOLD),
                    SpanKind::Italic => hs.font_style = Some(FontStyle::Italic),
                    SpanKind::Strike => {
                        hs.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None })
                    }
                    SpanKind::Code => {
                        hs.background_color = Some(rgb(0x262932).into());
                        hs.color = Some(rgb(CODE_FG).into());
                        mono = true;
                    }
                    SpanKind::Link(_) => {
                        hs.color = Some(rgb(LINK).into());
                        hs.underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(rgb(LINK).into()),
                            wavy: false,
                        });
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
        el.border_l_2().border_color(rgb(0x3b4252)).pl_3().text_color(rgb(MUTED))
    } else {
        el
    }
}

pub fn render_blocks(doc: &Rc<Doc>, msg: usize, shared: &SharedRef) -> Vec<AnyElement> {
    let mut out = vec![];
    for block in &doc.blocks {
        let el: AnyElement = match block {
            Block::Heading { level, unit } => {
                let size = match level {
                    1 => 24.,
                    2 => 20.,
                    3 => 17.,
                    _ => 15.,
                };
                div()
                    .mt_2()
                    .text_size(px(size))
                    .font_weight(FontWeight::BOLD)
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
            Block::Code { lang, unit } => {
                let text = doc.units[*unit].text.to_string();
                div()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .px_3()
                            .py_1()
                            .bg(rgb(0x22252d))
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(if lang.is_empty() { "text".to_string() } else { lang.clone() })
                            .child(
                                div()
                                    .id(("copy-code", msg * 10_000 + *unit))
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(rgb(FG)))
                                    .child("Copy code")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id(("code-scroll", msg * 10_000 + *unit))
                            .overflow_x_scroll()
                            .px_3()
                            .py_2()
                            .font_family(MONO)
                            .text_size(px(13.))
                            .child(div().flex_none().whitespace_nowrap().child(unit_el(doc, *unit, msg, shared))),
                    )
                    .into_any_element()
            }
            Block::Table { rows } => div()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(rgb(BORDER))
                .overflow_hidden()
                .children(rows.iter().enumerate().map(|(ri, cells)| {
                    div()
                        .flex()
                        .flex_row()
                        .when(ri == 0, |d| d.bg(rgb(0x22252d)).font_weight(FontWeight::BOLD))
                        .when(ri > 0, |d| d.border_t_1().border_color(rgb(BORDER)))
                        .children(cells.iter().map(|u| {
                            div().flex_1().min_w_0().px_2().py_1().child(unit_el(doc, *u, msg, shared))
                        }))
                }))
                .into_any_element(),
            Block::Rule => div().h(px(1.)).my_2().bg(rgb(BORDER)).into_any_element(),
        };
        out.push(el);
    }
    out
}

pub fn render_user(doc: &Rc<Doc>, msg: usize, shared: &SharedRef, attachments: usize) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .px_4()
        .py_2()
        .child(
            div()
                .max_w(px(620.))
                .rounded_lg()
                .bg(rgb(0x2a3350))
                .px_3()
                .py_2()
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
        .px_4()
        .py_2()
        .child(
            div()
                .max_w(px(780.))
                .flex()
                .flex_col()
                .gap_2()
                .line_height(px(22.))
                .children(render_blocks(doc, msg, shared))
                .when(live, |d| d.child(div().text_color(rgb(MUTED)).child("\u{258D}"))),
        )
        .into_any_element()
}
