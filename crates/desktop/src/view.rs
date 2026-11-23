//! Selectable text element, cross-block selection state and the Markdown -> element renderer.

use crate::md::{Block, Doc, SpanKind, Unit};
use gpui::prelude::*;
use gpui::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

// Short names for the theme tokens the views use most.
pub use crate::theme::{MONO, border, link, sans};
pub fn bg() -> u32 { crate::theme::canvas() }
pub fn fg() -> u32 { crate::theme::foreground() }
pub fn muted() -> u32 { crate::theme::muted_foreground() }
pub fn panel() -> u32 { crate::theme::card() }
pub fn sel_bg() -> u32 { crate::theme::selection() }

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
                        hs.background_color = Some(rgb(crate::theme::secondary()).into());
                        hs.color = Some(rgb(crate::theme::foreground()).into());
                        mono = true;
                    }
                    SpanKind::Link(_) => {
                        hs.color = Some(rgb(link()).into());
                    }
                    SpanKind::Color(c) => hs.color = Some(*c),
                }
            }
        }
        if let Some(s) = &sel {
            if s.start <= a && s.end >= b {
                hs.background_color = Some(rgb(sel_bg()).into());
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
        el.border_l_2().border_color(rgb(border())).pl_3().text_color(rgb(muted()))
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
        // Markdown spacing: 4 px between list items, 12 px elsewhere.
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
                .child(div().w(px(20.)).flex_shrink_0().text_color(rgb(muted())).child(marker.clone()))
                .child(div().flex_1().min_w_0().child(unit_el(doc, *unit, msg, shared)))
                .into_any_element(),
            Block::Code { lang, unit, indent } => {
                let text = doc.units[*unit].text.to_string();
                div()
                    .when(*indent > 0, |d| d.ml(px(list_text_offset(*indent))))
                    .my_1()
                    .rounded(px(8.))
                    .bg(rgb(panel()))
                    .border_1()
                    .border_color(rgb(border()))
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
                            .border_color(rgb(border()))
                            .child(
                                div()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(rgb(border()))
                                    .bg(rgb(crate::theme::chip_bg()))
                                    .font_family(MONO)
                                    .text_size(px(11.))
                                    .line_height(px(12.))
                                    .text_color(rgb(muted()))
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
                .border_color(rgb(crate::theme::soft_border()))
                .bg(rgb(crate::theme::table_bg()))
                .overflow_hidden()
                .text_size(px(13.))
                .children(rows.iter().enumerate().map(|(ri, cells)| {
                    div()
                        .flex()
                        .flex_row()
                        .when(ri == 0, |d| {
                            d.bg(rgb(crate::theme::table_head()))
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(muted()))
                                .border_b_1()
                                .border_color(rgb(border()))
                        })
                        .when(ri > 1, |d| d.border_t_1().border_color(rgb(crate::theme::canvas_line())))
                        .children(cells.iter().enumerate().map(|(ci, u)| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_3()
                                .py(px(if ri == 0 { 8. } else { 10. }))
                                .when(ri > 0 && ci > 0, |d| d.text_color(rgb(muted())))
                                .child(unit_el(doc, *u, msg, shared))
                        }))
                }))
                .into_any_element(),
            Block::Rule => div().h(px(1.)).my_4().bg(rgb(border())).into_any_element(),
            Block::Image { src, alt, indent } => image_el(src, alt, msg, out.len())
                .map(|el| div().pl(px(if *indent > 0 { list_text_offset(*indent) } else { 0. })).child(el).into_any_element())
                .unwrap_or_else(|| div().text_color(rgb(muted())).child(format!("[image: {alt}]")).into_any_element()),
        };
        out.push(div().mt(px(gap)).child(el).into_any_element());
    }
    out
}

/// Where a Markdown image comes from, after deciding whether to show it.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageSrc {
    /// A local raster file (an absolute path or a `file://` URL).
    Local(std::path::PathBuf),
    /// A raster `data:` URL, decoded.
    Data(std::sync::Arc<Image>),
    /// An http(s) image. The transcript never fetches it; it shows a link to open it instead.
    Remote(String),
}

fn raster_format(ext: &str) -> Option<ImageFormat> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "bmp" => ImageFormat::Bmp,
        _ => return None,
    })
}

thread_local! {
    /// Decoded `data:` images by URL, so a message is not decoded again on every frame.
    static DATA_IMAGES: RefCell<HashMap<String, Option<std::sync::Arc<Image>>>> = RefCell::new(HashMap::new());
}

/// Decides how to show an image: local raster files and raster `data:` URLs inline, http(s)
/// as a link, anything else (relative paths, other schemes, SVG) not at all.
pub fn image_source(src: &str) -> Option<ImageSrc> {
    use base64::Engine as _;
    let src = src.trim();
    if let Some(rest) = src.strip_prefix("data:image/") {
        let (meta, payload) = rest.split_once(',')?;
        let (ext, encoding) = meta.split_once(';').unwrap_or((meta, ""));
        let format = raster_format(ext)?;
        if encoding != "base64" {
            return None;
        }
        return DATA_IMAGES.with(|cache| {
            cache
                .borrow_mut()
                .entry(src.to_string())
                .or_insert_with(|| {
                    base64::engine::general_purpose::STANDARD
                        .decode(payload.trim())
                        .ok()
                        .map(|bytes| std::sync::Arc::new(Image::from_bytes(format, bytes)))
                })
                .clone()
                .map(ImageSrc::Data)
        });
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        return Some(ImageSrc::Remote(src.to_string()));
    }
    let path = match url::Url::parse(src) {
        Ok(url) if url.scheme() == "file" => url.to_file_path().ok()?,
        _ => std::path::PathBuf::from(src),
    };
    let raster = path.extension().and_then(|e| e.to_str()).and_then(raster_format).is_some();
    (path.is_absolute() && raster).then_some(ImageSrc::Local(path))
}

fn image_el(src: &str, alt: &str, msg: usize, index: usize) -> Option<AnyElement> {
    let id = SharedString::from(format!("image-{msg}-{index}"));
    let label = if alt.trim().is_empty() { "Image".to_string() } else { alt.trim().to_string() };
    let shown = |source: ImageSource| {
        div()
            .id(id.clone())
            .role(accesskit::Role::Image)
            .aria_label(SharedString::from(label.clone()))
            .max_w(px(560.))
            .child(img(source).max_w_full().max_h(px(420.)).object_fit(ObjectFit::Contain).rounded(px(8.)))
            .into_any_element()
    };
    Some(match image_source(src)? {
        ImageSrc::Local(path) => shown(path.into()),
        ImageSrc::Data(image) => shown(ImageSource::Image(image)),
        ImageSrc::Remote(url) => {
            let host = url::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
            div()
                .id(id)
                .role(accesskit::Role::Link)
                .aria_label(SharedString::from(format!("Open image {label} from {host}")))
                .cursor_pointer()
                .text_color(rgb(crate::theme::link()))
                .child(format!("View image: {label} ({host})"))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .into_any_element()
        }
    })
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
                .bg(rgb(crate::theme::user_bubble()))
                .px_3()
                .py_2()
                .text_size(px(13.5))
                .line_height(px(22.))
                .child(unit_el(doc, 0, msg, shared)),
        )
        .when(attachments > 0, |d| {
            d.child(div().text_size(px(12.)).text_color(rgb(muted())).child(format!("{attachments} image attachment(s)")))
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
                .when(live, |d| d.child(div().w(px(6.)).h(px(16.)).bg(crate::theme::alpha(fg(), 0.7)))),
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
                if failed { theme::destructive() } else { muted() },
                if failed { Icon::X } else { trace_icon(label) },
            )
        }
        TraceKind::Thinking { live: true, .. } => ("Thinking".to_string(), String::new(), muted(), Icon::Brain),
        TraceKind::Thinking { secs, .. } => ("Thought".to_string(), format!("for {secs:.0}s"), muted(), Icon::Brain),
        TraceKind::Tool { name, status, secs, .. } => (
            name.clone(),
            match status {
                ToolStatus::Running => "running\u{2026}".to_string(),
                ToolStatus::Ok | ToolStatus::Error => format!("{secs:.1}s"),
            },
            match status {
                ToolStatus::Error => theme::destructive(),
                _ => muted(),
            },
            match status {
                ToolStatus::Error => Icon::X,
                _ => trace_icon(name),
            },
        ),
    };
    let failed = badge_color == theme::destructive();
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
                .text_color(rgb(if failed { theme::destructive() } else { muted() }))
                .hover(|s| s.text_color(rgb(fg())))
                .child(
                    theme::icon(glyph, 16.)
                        .text_color(rgb(if failed { theme::destructive() } else { muted() }))
                        .group_hover(group.clone(), |s| s.text_color(rgb(fg()))),
                )
                .child(div().flex_none().child(label))
                .child(div().min_w_0().truncate().font_family(MONO).text_size(px(13.)).child(tr.title.clone()))
                .child(div().flex_none().text_size(px(12.)).text_color(rgb(badge_color)).child(badge))
                .when(has_body, |d| {
                    d.child(
                        theme::icon(if tr.open { Icon::ChevronDown } else { Icon::ChevronRight }, 16.)
                            .group_hover(group, |s| s.text_color(rgb(fg()))),
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
                    .border_color(rgb(border()))
                    .flex()
                    .flex_col()
                    .text_size(px(13.))
                    .text_color(rgb(muted()))
                    .line_height(px(21.))
                    .children(render_blocks(doc, msg, shared)),
            )
        })
        .into_any_element()
}

/// The icon for a tool or recorded step.
pub(crate) fn trace_icon(name: &str) -> crate::theme::Icon {
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

#[cfg(test)]
mod image_source_tests {
    use super::{image_source, ImageSrc};

    #[::core::prelude::v1::test]
    fn local_raster_files_and_data_urls_show_and_the_rest_do_not() {
        let local = if cfg!(windows) { r"C:\out\chart.png" } else { "/out/chart.png" };
        assert!(matches!(image_source(local), Some(ImageSrc::Local(_))));
        let file_url = if cfg!(windows) { "file:///C:/out/chart.jpg" } else { "file:///out/chart.jpg" };
        assert!(matches!(image_source(file_url), Some(ImageSrc::Local(_))));
        // A 1x1 transparent PNG.
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        assert!(matches!(image_source(png), Some(ImageSrc::Data(_))));
        assert_eq!(image_source("https://x.test/a.png"), Some(ImageSrc::Remote("https://x.test/a.png".into())));
        assert_eq!(image_source("relative/chart.png"), None, "relative paths have no base");
        assert_eq!(image_source(if cfg!(windows) { r"C:\notes.txt" } else { "/notes.txt" }), None);
        assert_eq!(image_source("data:image/svg+xml;base64,PHN2Zz4="), None, "SVG is not a raster format");
        assert_eq!(image_source("data:image/png,raw"), None, "only base64 data URLs");
        assert_eq!(image_source("javascript:alert(1)"), None);
    }
}
