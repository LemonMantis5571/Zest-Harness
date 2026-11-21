//! Multi-line composer: soft wrap, grapheme movement, selection, clipboard (text + image),
//! IME through `EntityInputHandler`. Built on `StyledText` so wrapping and hit-testing come
//! from GPUI's own text layout instead of hand-rolled line breaking.

use crate::view::{FG, MONO, MUTED, SANS, SEL_BG};
use gpui::prelude::*;
use gpui::*;
use std::ops::Range;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

actions!(
    zest_spike,
    [
        Send, Newline, Backspace, Delete, Left, Right, Up, Down, SelectLeft, SelectRight, SelectUp,
        SelectDown, SelectAll, Home, End, CopySel, CutSel, PasteSel, Stop, ShowCharacterPalette,
        Quit,
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Send, Some("Composer")),
        KeyBinding::new("shift-enter", Newline, Some("Composer")),
        KeyBinding::new("backspace", Backspace, Some("Composer")),
        KeyBinding::new("delete", Delete, Some("Composer")),
        KeyBinding::new("left", Left, Some("Composer")),
        KeyBinding::new("right", Right, Some("Composer")),
        KeyBinding::new("up", Up, Some("Composer")),
        KeyBinding::new("down", Down, Some("Composer")),
        KeyBinding::new("shift-left", SelectLeft, Some("Composer")),
        KeyBinding::new("shift-right", SelectRight, Some("Composer")),
        KeyBinding::new("shift-up", SelectUp, Some("Composer")),
        KeyBinding::new("shift-down", SelectDown, Some("Composer")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Composer")),
        KeyBinding::new("home", Home, Some("Composer")),
        KeyBinding::new("end", End, Some("Composer")),
        KeyBinding::new("ctrl-c", CopySel, Some("Composer")),
        KeyBinding::new("ctrl-x", CutSel, Some("Composer")),
        KeyBinding::new("ctrl-v", PasteSel, Some("Composer")),
        KeyBinding::new("escape", Stop, Some("Composer")),
        KeyBinding::new("ctrl-c", CopySel, Some("Chat")),
        KeyBinding::new("ctrl-.", ShowCharacterPalette, Some("Composer")),
    ]);
}

fn ime_log(msg: String) {
    if std::env::var_os("ZEST_IME_TRACE").is_some() {
        println!("IME {msg}");
    }
}

pub enum ComposerEvent {
    Send { text: String, images: Vec<Arc<Image>> },
    Stop,
}

pub struct Composer {
    pub multiline_field: bool,
    pub placeholder: &'static str,
    pub has_files: bool,
    pub focus: FocusHandle,
    pub text: String,
    pub sel: Range<usize>,
    pub reversed: bool,
    pub marked: Option<Range<usize>>,
    layout: Option<TextLayout>,
    bounds: Option<Bounds<Pixels>>,
    dragging: bool,
    pub images: Vec<Arc<Image>>,
    /// Trace of what the platform asked of the input handler, for the IME report.
    pub trace: Vec<String>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            multiline_field: false,
            has_files: false,
            placeholder: "Message Zest…  (Enter sends, Shift+Enter newline)",
            focus: cx.focus_handle(),
            text: String::new(),
            sel: 0..0,
            reversed: false,
            marked: None,
            layout: None,
            bounds: None,
            dragging: false,
            images: vec![],
            trace: vec![],
        }
    }

    /// (visual lines, laid-out height in px) from the last paint.
    pub fn debug(&self) -> (usize, f32) {
        let lines = self
            .layout
            .as_ref()
            .map(|l| l.line_layouts().iter().map(|ll| 1 + ll.wrap_boundaries().len()).sum())
            .unwrap_or(0);
        (lines, self.bounds.map(|b| b.size.height.as_f32()).unwrap_or(0.))
    }

    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
    }

    pub fn cursor(&self) -> usize {
        if self.reversed { self.sel.start } else { self.sel.end }
    }

    fn move_to(&mut self, i: usize, window: &Window, cx: &mut Context<Self>) {
        self.sel = i..i;
        self.reversed = false;
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn select_to(&mut self, i: usize, window: &Window, cx: &mut Context<Self>) {
        if self.reversed { self.sel.start = i } else { self.sel.end = i }
        if self.sel.end < self.sel.start {
            self.reversed = !self.reversed;
            self.sel = self.sel.end..self.sel.start;
        }
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn prev(&self, i: usize) -> usize {
        self.text.grapheme_indices(true).rev().find_map(|(ix, _)| (ix < i).then_some(ix)).unwrap_or(0)
    }

    fn next(&self, i: usize) -> usize {
        self.text.grapheme_indices(true).find_map(|(ix, _)| (ix > i).then_some(ix)).unwrap_or(self.text.len())
    }

    /// One visual line up/down using the laid-out text.
    fn vertical(&self, i: usize, dir: f32) -> usize {
        let (Some(l), true) = (self.layout.as_ref(), self.layout.is_some()) else { return i };
        let Some(p) = l.position_for_index(i) else { return i };
        let target = point(p.x, p.y + l.line_height() * dir + l.line_height() * 0.5);
        match l.index_for_position(target) {
            Ok(ix) | Err(ix) => ix.min(self.text.len()),
        }
    }

    fn line_start(&self, i: usize) -> usize {
        self.text[..i].rfind('\n').map(|p| p + 1).unwrap_or(0)
    }
    fn line_end(&self, i: usize) -> usize {
        self.text[i..].find('\n').map(|p| i + p).unwrap_or(self.text.len())
    }

    fn insert(&mut self, new: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, new, window, cx);
    }

    fn do_send(&mut self, _: &Send, window: &mut Window, cx: &mut Context<Self>) {
        // IME consumes Enter while composing; this guards the synthetic-key path too.
        if self.marked.is_some() {
            return;
        }
        if self.multiline_field {
            self.insert("\n", window, cx);
            return;
        }
        let text = self.text.trim().to_string();
        if text.is_empty() && self.images.is_empty() && !self.has_files {
            return;
        }
        let images = std::mem::take(&mut self.images);
        cx.emit(ComposerEvent::Send { text, images });
        self.text.clear();
        self.sel = 0..0;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.insert("\n", window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let p = self.prev(self.cursor());
            self.select_to(p, window, cx);
        }
        self.insert("", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let n = self.next(self.cursor());
            self.select_to(n, window, cx);
        }
        self.insert("", window, cx);
    }

    fn left(&mut self, _: &Left, w: &mut Window, cx: &mut Context<Self>) {
        let t = if self.sel.is_empty() { self.prev(self.cursor()) } else { self.sel.start };
        self.move_to(t, w, cx);
    }
    fn right(&mut self, _: &Right, w: &mut Window, cx: &mut Context<Self>) {
        let t = if self.sel.is_empty() { self.next(self.sel.end) } else { self.sel.end };
        self.move_to(t, w, cx);
    }
    fn up(&mut self, _: &Up, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.vertical(self.cursor(), -1.);
        self.move_to(t, w, cx);
    }
    fn down(&mut self, _: &Down, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.vertical(self.cursor(), 1.);
        self.move_to(t, w, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.prev(self.cursor());
        self.select_to(t, w, cx);
    }
    fn select_right(&mut self, _: &SelectRight, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.next(self.cursor());
        self.select_to(t, w, cx);
    }
    fn select_up(&mut self, _: &SelectUp, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.vertical(self.cursor(), -1.);
        self.select_to(t, w, cx);
    }
    fn select_down(&mut self, _: &SelectDown, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.vertical(self.cursor(), 1.);
        self.select_to(t, w, cx);
    }
    fn select_all(&mut self, _: &SelectAll, w: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, w, cx);
        self.select_to(self.text.len(), w, cx);
    }
    fn home(&mut self, _: &Home, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.line_start(self.cursor());
        self.move_to(t, w, cx);
    }
    fn end(&mut self, _: &End, w: &mut Window, cx: &mut Context<Self>) {
        let t = self.line_end(self.cursor());
        self.move_to(t, w, cx);
    }

    fn copy(&mut self, _: &CopySel, _w: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            cx.propagate();
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.sel.clone()].to_string()));
    }
    fn cut(&mut self, _: &CutSel, w: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.sel.clone()].to_string()));
            self.insert("", w, cx);
        }
    }
    fn paste(&mut self, _: &PasteSel, w: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        let mut summary = vec![];
        for e in item.entries() {
            match e {
                ClipboardEntry::String(s) => {
                    summary.push(format!("text({}b)", s.text().len()));
                    let t = s.text().replace("\r\n", "\n").replace('\r', "\n");
                    self.insert(&t, w, cx);
                }
                ClipboardEntry::Image(img) => {
                    if self.multiline_field { continue; }
                    summary.push(format!("image({:?},{}b)", img.format, img.bytes().len()));
                    self.images.push(Arc::new(img.clone()));
                }
                ClipboardEntry::ExternalPaths(p) => {
                    summary.push(format!("paths({})", p.paths().len()));
                }
            }
        }
        self.trace.push(format!("paste: {}", summary.join(" + ")));
        cx.notify();
    }
    fn palette(&mut self, _: &ShowCharacterPalette, w: &mut Window, _cx: &mut Context<Self>) {
        w.show_character_palette();
    }
    fn stop(&mut self, _: &Stop, _w: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ComposerEvent::Stop);
    }

    fn index_at(&self, p: Point<Pixels>) -> usize {
        match &self.layout {
            Some(l) if !self.text.is_empty() => match l.index_for_position(p) {
                Ok(i) | Err(i) => i.min(self.text.len()),
            },
            _ => 0,
        }
    }

    // ---- UTF-16 <-> UTF-8 (the platform IME speaks UTF-16) ----
    fn off_from_u16(&self, off: usize) -> usize {
        let (mut u8o, mut u16c) = (0, 0);
        for ch in self.text.chars() {
            if u16c >= off {
                break;
            }
            u16c += ch.len_utf16();
            u8o += ch.len_utf8();
        }
        u8o
    }
    fn off_to_u16(&self, off: usize) -> usize {
        let (mut u16o, mut u8c) = (0, 0);
        for ch in self.text.chars() {
            if u8c >= off {
                break;
            }
            u8c += ch.len_utf8();
            u16o += ch.len_utf16();
        }
        u16o
    }
    fn range_to_u16(&self, r: &Range<usize>) -> Range<usize> {
        self.off_to_u16(r.start)..self.off_to_u16(r.end)
    }
    fn range_from_u16(&self, r: &Range<usize>) -> Range<usize> {
        self.off_from_u16(r.start)..self.off_from_u16(r.end)
    }
}

impl EntityInputHandler for Composer {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_from_u16(&range);
        actual.replace(self.range_to_u16(&r));
        Some(self.text[r].to_string())
    }

    fn selected_text_range(&mut self, _ignore: bool, _w: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_u16(&self.sel), reversed: self.reversed })
    }

    fn marked_text_range(&self, _w: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_u16(r))
    }

    fn unmark_text(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        ime_log("unmark_text".into());
        self.trace.push("unmark".into());
        self.marked = None;
        w.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, new: &str, w: &mut Window, cx: &mut Context<Self>) {
        let r = range
            .as_ref()
            .map(|r| self.range_from_u16(r))
            .or(self.marked.clone())
            .unwrap_or(self.sel.clone());
        ime_log(format!("replace_text_in_range range16={range:?} new={new:?} marked={:?}", self.marked));
        if !new.is_empty() && new.chars().any(|c| !c.is_ascii() || c.is_control()) {
            self.trace.push(format!("commit {new:?} over {r:?}"));
        }
        self.text.replace_range(r.clone(), new);
        let end = r.start + new.len();
        self.sel = end..end;
        self.reversed = false;
        self.marked = None;
        w.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        new_sel: Option<Range<usize>>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = range
            .as_ref()
            .map(|r| self.range_from_u16(r))
            .or(self.marked.clone())
            .unwrap_or(self.sel.clone());
        ime_log(format!("replace_and_mark_text_in_range range16={range:?} new={new:?} new_sel={new_sel:?}"));
        self.trace.push(format!("compose {new:?} sel={new_sel:?} over {r:?}"));
        self.text.replace_range(r.clone(), new);
        self.marked = (!new.is_empty()).then(|| r.start..r.start + new.len());
        let start16 = self.off_to_u16(r.start);
        let len16 = new.encode_utf16().count();
        self.sel = new_sel
            .map(|s| self.range_from_u16(&(start16 + s.start.min(len16)..start16 + s.end.min(len16))))
            .unwrap_or(r.start + new.len()..r.start + new.len());
        self.reversed = false;
        w.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let l = self.layout.as_ref()?;
        let r = self.range_from_u16(&range);
        ime_log(format!("bounds_for_range range16={range:?}"));
        let a = l.position_for_index(r.start)?;
        let b = l.position_for_index(r.end).unwrap_or(a);
        Some(Bounds::from_corners(a, point(b.x.max(a.x + px(2.)), b.y + l.line_height())))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        let b = self.bounds?;
        b.contains(&p).then(|| self.off_to_u16(self.index_at(p)))
    }
}

struct ComposerText {
    input: Entity<Composer>,
    inner: StyledText,
    caret_blink_on: bool,
}

impl IntoElement for ComposerText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ComposerText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(&mut self, id: Option<&GlobalElementId>, ii: Option<&InspectorElementId>, w: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        self.inner.request_layout(id, ii, w, cx)
    }
    fn prepaint(&mut self, id: Option<&GlobalElementId>, ii: Option<&InspectorElementId>, b: Bounds<Pixels>, st: &mut (), w: &mut Window, cx: &mut App) {
        self.inner.prepaint(id, ii, b, st, w, cx)
    }
    fn paint(&mut self, id: Option<&GlobalElementId>, ii: Option<&InspectorElementId>, bounds: Bounds<Pixels>, rl: &mut (), pp: &mut (), w: &mut Window, cx: &mut App) {
        let focus = self.input.read(cx).focus.clone();
        w.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        self.inner.paint(id, ii, bounds, rl, pp, w, cx);
        let layout = self.inner.layout().clone();
        let (cursor, collapsed) = {
            let c = self.input.read(cx);
            (c.cursor(), c.sel.is_empty())
        };
        if focus.is_focused(w) && collapsed && self.caret_blink_on {
            let origin = layout.position_for_index(cursor).unwrap_or(bounds.origin);
            let lh = layout.line_height();
            w.paint_quad(fill(Bounds::new(origin, size(px(1.5), lh)), rgb(FG)));
            w.request_autoscroll(Bounds::new(origin, size(px(1.5), lh)));
        }
        let input = self.input.clone();
        input.update(cx, |c, _| {
            c.layout = Some(layout);
            c.bounds = Some(bounds);
        });
        // Drag-select keeps working when the pointer leaves the box.
        let input = self.input.clone();
        w.on_mouse_event(move |e: &MouseMoveEvent, phase, _w, cx| {
            if phase == DispatchPhase::Bubble && e.dragging() {
                input.update(cx, |c, cx| {
                    if c.dragging {
                        let i = c.index_at(e.position);
                        c.select_to(i, _w, cx);
                    }
                });
            }
        });
        let input = self.input.clone();
        w.on_mouse_event(move |_: &MouseUpEvent, _phase, _w, cx| {
            input.update(cx, |c, _| c.dragging = false);
        });
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.text.is_empty();
        let mut runs: Vec<(Range<usize>, HighlightStyle)> = vec![];
        let len = self.text.len();
        let mut cuts = vec![0, len];
        if !self.sel.is_empty() {
            cuts.extend([self.sel.start, self.sel.end]);
        }
        if let Some(m) = &self.marked {
            cuts.extend([m.start, m.end]);
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a >= b {
                continue;
            }
            let mut hs = HighlightStyle::default();
            let mut touched = false;
            if !self.sel.is_empty() && self.sel.start <= a && self.sel.end >= b {
                hs.background_color = Some(rgb(SEL_BG).into());
                touched = true;
            }
            if self.marked.as_ref().is_some_and(|m| m.start <= a && m.end >= b) {
                hs.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(rgb(FG).into()), wavy: false });
                touched = true;
            }
            if touched {
                runs.push((a..b, hs));
            }
        }
        let styled = StyledText::new(if empty { SharedString::from(" ") } else { SharedString::from(self.text.clone()) })
            .with_highlights(runs);
        let focused = self.focus.is_focused(window);
        let lh = px(22.);
        let this = cx.entity();
        let thumbs = self.images.clone();

        div()
            .id("composer")
            .role(Role::MultilineTextInput)
            .aria_label("Message")
            .aria_value(SharedString::from(self.text.clone()))
            .key_context("Composer")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::do_send))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::palette))
            .on_action(cx.listener(Self::stop))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(rgb(0x1d1f26))
            .border_1()
            .border_color(rgb(if focused { 0x4a78c2 } else { 0x2c2f3a }))
            .when(!thumbs.is_empty(), |d| {
                d.child(div().flex().flex_row().gap_2().children(thumbs.into_iter().map(|im| {
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(img(im.clone()).size(px(56.)).rounded_md().object_fit(ObjectFit::Cover))
                        .child(format!("{:?} {}KB", im.format, im.bytes().len() / 1024))
                })))
            })
            .child(
                div()
                    .id("composer-scroll")
                    .relative()
                    .min_h(lh)
                    .max_h(lh * 8.)
                    .overflow_y_scroll()
                    .font_family(SANS)
                    .text_size(px(14.))
                    .line_height(lh)
                    .text_color(rgb(FG))
                    .on_mouse_down(MouseButton::Left, {
                        let this = this.clone();
                        move |e: &MouseDownEvent, w, cx| {
                            this.update(cx, |c, cx| {
                                w.focus(&c.focus, cx);
                                c.dragging = true;
                                let i = c.index_at(e.position);
                                if e.modifiers.shift { c.select_to(i, w, cx) } else { c.move_to(i, w, cx) }
                            });
                        }
                    })
                    .child(ComposerText { input: this, inner: styled, caret_blink_on: true })
                    .when(empty, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .text_color(rgb(MUTED))
                                .child(self.placeholder),
                        )
                    }),
            )
    }
}

impl Focusable for Composer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[allow(dead_code)]
const _: &str = MONO;
