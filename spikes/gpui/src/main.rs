mod bridge;
mod composer;
mod md;
mod seed;
mod view;
mod wv;

use bridge::{Bridge, Cmd, EvKind};
use composer::{Composer, ComposerEvent, CopySel};
use futures::StreamExt;
use gpui::prelude::*;
use gpui::*;
use md::Doc;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use view::*;

#[derive(Default, Clone)]
struct Args {
    bench: bool,
    scroll: bool,
    selftest: bool,
    idle: bool,
    topmost: bool,
    webview: bool,
    a11y: bool,
    focus: bool,
    langs: bool,
    /// Agent-shaped history and turns: reasoning rows and tool rows between the messages.
    agent: bool,
    /// Start with every reasoning and tool row expanded (worst-case density for the scroll bench).
    open: bool,
    show: String,
    rate: u32,
    secs: u32,
    history: usize,
}

fn parse_args() -> Args {
    let mut a = Args { rate: 75, secs: 30, history: 500, ..Default::default() };
    for s in std::env::args().skip(1) {
        match s.as_str() {
            "--bench" => a.bench = true,
            "--scroll" => a.scroll = true,
            "--selftest" => a.selftest = true,
            "--idle" => a.idle = true,
            "--topmost" => a.topmost = true,
            "--webview" => a.webview = true,
            "--a11y" => a.a11y = true,
            "--focus" => a.focus = true,
            "--langs" => a.langs = true,
            "--agent" => a.agent = true,
            "--open" => a.open = true,
            _ => {
                if let Some(v) = s.strip_prefix("--show=") {
                    a.show = v.to_string();
                    a.idle = true;
                } else if let Some(v) = s.strip_prefix("--rate=") {
                    a.rate = v.parse().unwrap_or(75)
                } else if let Some(v) = s.strip_prefix("--secs=") {
                    a.secs = v.parse().unwrap_or(30)
                } else if let Some(v) = s.strip_prefix("--history=") {
                    a.history = v.parse().unwrap_or(500)
                }
            }
        }
    }
    a
}

struct Msg {
    user: bool,
    doc: Rc<Doc>,
    attachments: usize,
    /// Set for reasoning and tool rows; `doc` is then the row's body.
    trace: Option<Trace>,
}

impl Msg {
    fn user(text: &str, attachments: usize) -> Self {
        Msg { user: true, doc: Rc::new(Doc::plain(text)), attachments, trace: None }
    }
    fn assistant(doc: Doc) -> Self {
        Msg { user: false, doc: Rc::new(doc), attachments: 0, trace: None }
    }
    fn trace(trace: Trace, doc: Doc) -> Self {
        Msg { user: false, doc: Rc::new(doc), attachments: 0, trace: Some(trace) }
    }
}

/// Body of a tool row: the diff when there is one, else the result text, as a fenced block.
fn tool_body(summary: &str, diff: Option<&str>) -> Doc {
    let (lang, body) = match diff {
        Some(d) => ("diff", d),
        None => ("text", summary),
    };
    md::parse(&format!("```{lang}\n{body}\n```"))
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

#[derive(Default)]
struct Stats {
    parse_us: Vec<u64>,
    latency_us: Vec<u64>,
    tokens: usize,
    stream_started: Option<Instant>,
    snapshot0: Option<gpui::FrameDurationSnapshot>,
}

struct Root {
    msgs: Vec<Msg>,
    list: ListState,
    shared: SharedRef,
    composer: Entity<Composer>,
    focus: FocusHandle,
    live: Option<usize>,
    live_text: String,
    /// True from a turn's Start to its End, including before the first answer token.
    turn_active: bool,
    turns_done: usize,
    /// The reasoning row of the running turn: (row index, text so far, started).
    think: Option<(usize, String, Instant)>,
    /// Running tool rows of the turn: (call id, row index, started).
    tools: Vec<(String, usize, Instant)>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    stats: Stats,
    args: Args,
    window: AnyWindowHandle,
    last_link: Option<String>,
}

fn pct(v: &mut Vec<u64>, p: f64) -> f64 {
    if v.is_empty() {
        return 0.;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p) as usize] as f64
}

impl Root {
    fn new(args: Args, bridge: Bridge, window: AnyWindowHandle, cx: &mut Context<Self>) -> Self {
        let t = Instant::now();
        let mut msgs = Vec::with_capacity(args.history);
        for i in 0..args.history {
            if args.agent {
                // One agent turn is six rows: user, reasoning, three tool calls, answer.
                msgs.push(match i % 6 {
                    0 => Msg::user(&seed::user_text(i), 0),
                    1 => Msg::trace(
                        Trace { kind: TraceKind::Thinking { live: false, secs: 2. + (i % 7) as f32 }, title: String::new(), open: false },
                        Doc::plain(&seed::think_text(i)),
                    ),
                    2..=4 => {
                        let t = seed::tool_seed(i);
                        let status = if t.is_error { ToolStatus::Error } else { ToolStatus::Ok };
                        Msg::trace(
                            Trace {
                                kind: TraceKind::Tool { name: t.name.to_string(), id: format!("h{i}"), status, secs: t.delay_ms as f32 / 40. },
                                title: first_line(&t.summary),
                                open: false,
                            },
                            tool_body(&t.summary, t.diff.as_deref()),
                        )
                    }
                    _ => Msg::assistant(md::parse(&seed::assistant_text(i))),
                });
            } else if i % 2 == 0 {
                msgs.push(Msg::user(&seed::user_text(i), 0));
            } else {
                msgs.push(Msg::assistant(md::parse(&seed::assistant_text(i))));
            }
        }
        if args.open {
            for t in msgs.iter_mut().filter_map(|m| m.trace.as_mut()) {
                t.open = true;
            }
        }
        println!("seed: {} messages parsed+highlighted in {:?}", msgs.len(), t.elapsed());

        let list = ListState::new(msgs.len(), ListAlignment::Bottom, px(800.));
        list.set_follow_mode(FollowMode::Tail);
        let composer = cx.new(Composer::new);
        cx.subscribe(&composer, |this, _, ev: &ComposerEvent, cx| match ev {
            ComposerEvent::Send { text, images } => this.send(text.clone(), *images, cx),
            ComposerEvent::Stop => {
                let _ = this.cmd_tx.send(Cmd::Cancel);
            }
        })
        .detach();

        // Event pump: the receiver's waker is GPUI's foreground executor.
        let mut rx = bridge.rx.expect("rx");
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(e) = rx.try_recv() {
                    batch.push(e);
                }
                if this.update(cx, |root, cx| root.apply(batch, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            msgs,
            list,
            shared: Rc::new(RefCell::new(Shared::default())),
            composer,
            focus: cx.focus_handle(),
            live: None,
            live_text: String::new(),
            turn_active: false,
            turns_done: 0,
            think: None,
            tools: vec![],
            cmd_tx: bridge.tx,
            stats: Stats::default(),
            args,
            window,
            last_link: None,
        }
    }

    fn push(&mut self, m: Msg) -> usize {
        let ix = self.msgs.len();
        self.msgs.push(m);
        self.list.splice(ix..ix, 1);
        ix
    }

    fn send(&mut self, text: String, images: usize, cx: &mut Context<Self>) {
        self.push(Msg::user(&text, images));
        let _ = self.cmd_tx.send(Cmd::Reply { prompt: text, images, agent: self.args.agent });
        self.list.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    fn apply(&mut self, batch: Vec<bridge::Ev>, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut dirty = false;
        for ev in batch {
            self.stats.latency_us.push(now.duration_since(ev.at).as_micros() as u64);
            match ev.kind {
                EvKind::Start => {
                    // The answer row is pushed on the first text token, after any reasoning and tool rows.
                    self.turn_active = true;
                    self.live = None;
                    self.think = None;
                    self.tools.clear();
                    self.live_text.clear();
                    self.stats.stream_started = Some(Instant::now());
                    self.stats.tokens = 0;
                    self.stats.snapshot0 = self
                        .window
                        .update(cx, |_, w, _| w.frame_duration_snapshot())
                        .ok();
                }
                EvKind::Thinking(t) => {
                    if self.think.is_none() {
                        let row = Trace { kind: TraceKind::Thinking { live: true, secs: 0. }, title: String::new(), open: false };
                        let ix = self.push(Msg::trace(row, Doc::default()));
                        self.think = Some((ix, String::new(), Instant::now()));
                    }
                    if let Some((ix, buf, _)) = self.think.as_mut() {
                        buf.push_str(&t);
                        // A collapsed row shows no body, so only an open one needs rebuilding per token.
                        if self.msgs[*ix].trace.as_ref().is_some_and(|tr| tr.open) {
                            self.msgs[*ix].doc = Rc::new(Doc::plain(buf));
                            self.list.remeasure_items(*ix..*ix + 1);
                        }
                    }
                    dirty = true;
                }
                EvKind::ToolStart { id, name } => {
                    self.finish_think();
                    let row = Trace {
                        kind: TraceKind::Tool { name, id: id.clone(), status: ToolStatus::Running, secs: 0. },
                        title: String::new(),
                        open: false,
                    };
                    let ix = self.push(Msg::trace(row, Doc::default()));
                    self.tools.push((id, ix, Instant::now()));
                    dirty = true;
                }
                EvKind::ToolResult { id, summary, diff, is_error } => {
                    if let Some(pos) = self.tools.iter().position(|t| t.0 == id) {
                        let (_, ix, started) = self.tools.remove(pos);
                        let m = &mut self.msgs[ix];
                        m.doc = Rc::new(tool_body(&summary, diff.as_deref()));
                        if let Some(Trace { kind: TraceKind::Tool { status, secs, .. }, title, .. }) = m.trace.as_mut() {
                            *status = if is_error { ToolStatus::Error } else { ToolStatus::Ok };
                            *secs = started.elapsed().as_secs_f32();
                            *title = first_line(&summary);
                        }
                        self.list.remeasure_items(ix..ix + 1);
                    }
                    dirty = true;
                }
                EvKind::Text(t) => {
                    self.finish_think();
                    if self.live.is_none() {
                        let ix = self.push(Msg::assistant(Doc::default()));
                        self.live = Some(ix);
                    }
                    self.live_text.push_str(&t);
                    self.stats.tokens += 1;
                    dirty = true;
                }
                EvKind::End { tokens, cancelled } => {
                    dirty = true;
                    self.finish_think();
                    self.turn_active = false;
                    self.turns_done += 1;
                    if let Some(ix) = self.live.take() {
                        self.reparse(ix);
                    }
                    println!("turn end: {tokens} tokens cancelled={cancelled}");
                    if self.args.bench {
                        self.report_and_quit(cx);
                    }
                }
            }
        }
        if dirty {
            if let Some(ix) = self.live {
                self.reparse(ix);
            }
            cx.notify();
        }
    }

    /// Closes the running reasoning row: final text and duration, header relabelled "Thought".
    fn finish_think(&mut self) {
        if let Some((ix, buf, started)) = self.think.take() {
            let secs = started.elapsed().as_secs_f32();
            let m = &mut self.msgs[ix];
            m.doc = Rc::new(Doc::plain(&buf));
            if let Some(tr) = m.trace.as_mut() {
                tr.kind = TraceKind::Thinking { live: false, secs };
            }
            self.list.remeasure_items(ix..ix + 1);
        }
    }

    fn toggle_trace(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(tr) = self.msgs.get_mut(ix).and_then(|m| m.trace.as_mut()) {
            tr.open = !tr.open;
            self.list.remeasure_items(ix..ix + 1);
            cx.notify();
        }
    }

    fn reparse(&mut self, ix: usize) {
        let t = Instant::now();
        let doc = md::parse(&self.live_text);
        self.stats.parse_us.push(t.elapsed().as_micros() as u64);
        self.msgs[ix].doc = Rc::new(doc);
        self.list.remeasure_items(ix..ix + 1);
    }

    fn report_and_quit(&mut self, cx: &mut Context<Self>) {
        let secs = self.stats.stream_started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.);
        let snap = self.window.update(cx, |_, w, _| w.frame_duration_snapshot()).ok();
        let mut out = serde_json::Map::new();
        out.insert("history".into(), self.args.history.into());
        out.insert("rate_target".into(), self.args.rate.into());
        out.insert("tokens".into(), self.stats.tokens.into());
        out.insert("stream_secs".into(), secs.into());
        out.insert("tok_per_s".into(), (self.stats.tokens as f64 / secs.max(0.001)).into());
        out.insert("live_chars".into(), self.live_text.len().into());
        let ms = |ns: f64| (ns / 1.0e4).round() / 100.;
        if let Some(mut s) = snap {
            if let Some(s0) = &self.stats.snapshot0 {
                let _ = s.draw_duration_histogram.subtract(&s0.draw_duration_histogram);
                let _ = s.dirty_to_present_histogram.subtract(&s0.dirty_to_present_histogram);
                let _ = s.present_interval_histogram.subtract(&s0.present_interval_histogram);
            }
            for (name, h) in [
                ("draw_ms", &s.draw_duration_histogram),
                ("dirty_to_present_ms", &s.dirty_to_present_histogram),
                ("present_interval_ms", &s.present_interval_histogram),
            ] {
                out.insert(
                    name.into(),
                    serde_json::json!({
                        "n": h.len(), "mean": ms(h.mean()), "p50": ms(h.value_at_quantile(0.5) as f64),
                        "p95": ms(h.value_at_quantile(0.95) as f64), "p99": ms(h.value_at_quantile(0.99) as f64),
                        "max": ms(h.max() as f64),
                    }),
                );
            }
            let mut late = 0u64;
            let mut ontime = 0u64;
            let mut missed = 0u64;
            for r in &s.by_refresh_interval {
                late += r.frames_late;
                ontime += r.frames_on_time;
                missed += r.missed_refreshes;
            }
            out.insert(
                "refresh_interval_ms".into(),
                s.by_refresh_interval
                    .iter()
                    .map(|r| r.refresh_interval.map(|d| d.as_secs_f64() * 1000.).unwrap_or(0.))
                    .collect::<Vec<_>>()
                    .into(),
            );
            out.insert("frames_on_time".into(), ontime.into());
            out.insert("frames_late".into(), late.into());
            out.insert("missed_refreshes".into(), missed.into());
        }
        let us = |v: &mut Vec<u64>| {
            serde_json::json!({"n": v.len(), "p50": pct(v, 0.5) / 1000., "p95": pct(v, 0.95) / 1000., "max": pct(v, 1.0) / 1000.})
        };
        out.insert("md_parse_ms".into(), us(&mut self.stats.parse_us));
        out.insert("event_wake_latency_ms".into(), us(&mut self.stats.latency_us));
        let line = serde_json::Value::Object(out).to_string();
        println!("BENCH {line}");
        let _ = std::fs::write("bench-last.json", &line);
        cx.quit();
    }

    fn selection_text(&self) -> Option<String> {
        let (a, b) = self.shared.borrow().ordered()?;
        let mut out = String::new();
        for m in a.msg..=b.msg.min(self.msgs.len().saturating_sub(1)) {
            let doc = &self.msgs[m].doc;
            // A collapsed row is hidden, so it is not part of what a selection copies.
            if doc.units.is_empty() || self.msgs[m].trace.as_ref().is_some_and(|t| !t.open) {
                continue;
            }
            let u0 = if m == a.msg { a.unit } else { 0 };
            let u1 = if m == b.msg { b.unit } else { doc.units.len() - 1 };
            for u in u0..=u1.min(doc.units.len() - 1) {
                let unit = &doc.units[u];
                let s = if m == a.msg && u == a.unit { a.off } else { 0 };
                let e = if m == b.msg && u == b.unit { b.off } else { unit.text.len() };
                out.push_str(md::unit_slice(unit, s..e));
                if !(m == b.msg && u == u1) {
                    out.push_str(unit.sep);
                }
            }
            if m != b.msg {
                out.push_str("\n\n");
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn copy(&mut self, _: &CopySel, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
    }

    fn click_link(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let pos = self.shared.borrow().exact_pos_at(p);
        let Some(pos) = pos else { return };
        let Some(unit) = self.msgs.get(pos.msg).and_then(|m| m.doc.units.get(pos.unit)) else { return };
        for (r, k) in &unit.spans {
            if let md::SpanKind::Link(url) = k {
                if r.contains(&pos.off) && (url.starts_with("http://") || url.starts_with("https://")) {
                    self.last_link = Some(url.clone());
                    if !self.args.selftest {
                        cx.open_url(url);
                    }
                    return;
                }
            }
        }
    }

    fn mouse_layer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shared = self.shared.clone();
        let root = cx.entity();
        let list = self.list.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (sh, rt) = (shared.clone(), root.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, w, cx| {
                    if std::env::var_os("ZEST_DEBUG").is_some() {
                        eprintln!("mousedown phase={phase:?} pos={:?} bounds={bounds:?}", e.position);
                    }
                    if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !bounds.contains(&e.position) {
                        return;
                    }
                    {
                        let mut s = sh.borrow_mut();
                        let p = s.pos_at(e.position);
                        s.anchor = p;
                        s.focus = p;
                        s.dragging = p.is_some();
                        s.down_at = Some(e.position);
                    }
                    rt.update(cx, |this, cx| {
                        w.focus(&this.focus, cx);
                        cx.notify();
                    });
                });
                let (sh, rt) = (shared.clone(), root.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _w, cx| {
                    if phase != DispatchPhase::Bubble || !sh.borrow().dragging {
                        return;
                    }
                    let mut s = sh.borrow_mut();
                    s.focus = s.pos_at(e.position).or(s.focus);
                    drop(s);
                    if e.position.y < bounds.top() + px(6.) {
                        list.scroll_by(px(-24.));
                    } else if e.position.y > bounds.bottom() - px(6.) {
                        list.scroll_by(px(24.));
                    }
                    rt.update(cx, |_, cx| cx.notify());
                });
                let (sh, rt) = (shared.clone(), root.clone());
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _w, cx| {
                    if phase != DispatchPhase::Bubble || !sh.borrow().dragging {
                        return;
                    }
                    let click = {
                        let mut s = sh.borrow_mut();
                        s.dragging = false;
                        let click = s.anchor == s.focus;
                        if click {
                            s.anchor = None;
                            s.focus = None;
                        }
                        click
                    };
                    rt.update(cx, |this, cx| {
                        if click {
                            this.click_link(e.position, cx);
                        }
                        cx.notify();
                    });
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn render_item(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(m) = self.msgs.get(ix) else { return div().into_any_element() };
        let el = if let Some(tr) = &m.trace {
            let on_toggle = cx.listener(move |this, _: &ClickEvent, _w, cx| this.toggle_trace(ix, cx));
            render_trace(ix, tr, &m.doc, &self.shared, on_toggle)
        } else if m.user {
            render_user(&m.doc, ix, &self.shared, m.attachments)
        } else {
            render_assistant(&m.doc, ix, &self.shared, self.live == Some(ix))
        };
        if self.args.a11y {
            let text: String = m.doc.units.first().map(|u| u.text.chars().take(120).collect()).unwrap_or_default();
            div()
                .id(("row", ix))
                .role(Role::ListItem)
                .aria_label(format!("{}: {text}", if m.trace.is_some() { "Trace" } else if m.user { "You" } else { "Zest" }))
                .child(el)
                .into_any_element()
        } else {
            el
        }
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        {
            let mut s = self.shared.borrow_mut();
            s.hits.clear();
            s.rows.clear();
        }
        let following = self.list.is_following_tail();
        let busy = self.turn_active;
        let n = self.msgs.len();
        let list = self.list.clone();
        div()
            .id("root")
            .when(self.args.a11y, |d| d.role(Role::Application).aria_label("Zest GPUI spike"))
            .key_context("Chat")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::copy))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .font_family(SANS)
            .text_size(px(14.))
            .child(
                div().id("transcript").when(self.args.a11y, |d| d.role(Role::List).aria_label("Conversation")).flex_1().min_h_0().relative().child(
                    gpui::list(self.list.clone(), cx.processor(|this, ix, _w, cx| this.render_item(ix, cx)))
                        .size_full(),
                )
                .child(self.mouse_layer(cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .px_4()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{n} messages \u{2022} {} \u{2022} {}",
                        if busy { "streaming" } else { "idle" },
                        if following { "following tail" } else { "scrolled up (tail released)" }
                    ))
                    .child(
                        div()
                            .id("jump")
                            .cursor_pointer()
                            .when(following, |d| d.invisible())
                            .text_color(rgb(LINK))
                            .child("Jump to latest \u{2193}")
                            .on_click(move |_, _, _| list.set_follow_mode(FollowMode::Tail)),
                    ),
            )
            .child(div().px_4().pb_3().child(self.composer.clone()))
    }
}

fn main() {
    let args = parse_args();
    let hl = md::init_highlighter();
    println!("syntect: loaded default syntaxes+theme in {hl:?}");
    if args.langs {
        let zest = [
            "bash", "c", "cpp", "csharp", "css", "diff", "docker", "go", "graphql", "html", "java", "javascript",
            "json", "jsx", "kotlin", "markdown", "php", "powershell", "python", "ruby", "rust", "scss", "shell",
            "sql", "swift", "toml", "tsx", "typescript", "vue", "yaml",
        ];
        let cov = md::language_coverage(&zest);
        let of = |c: md::Coverage| cov.iter().filter(|x| x.1 == c).map(|x| x.0.as_str()).collect::<Vec<_>>();
        let (native, approx, missing) = (of(md::Coverage::Native), of(md::Coverage::Approx), of(md::Coverage::None));
        println!("LANGS pack has {} grammars; {} Zest languages", md::grammar_count(), cov.len());
        println!("LANGS native {}: {native:?}", native.len());
        println!("LANGS approximate {}: {approx:?}", approx.len());
        println!("LANGS missing {}: {missing:?}", missing.len());
        return;
    }
    let bridge = bridge::start();
    gpui_platform::application().run(move |cx: &mut App| {
        composer::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1000.), px(900.)), cx);
        let a2 = args.clone();
        let a3 = args.clone();
        // Automated modes must not steal focus from whatever the user is doing.
        let interactive = args.focus || !(args.bench || args.scroll || args.selftest || args.idle);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Zest GPUI spike".into()),
                        ..Default::default()
                    }),
                    focus: interactive,
                    kind: if args.topmost { WindowKind::PopUp } else { WindowKind::Normal },
                    // GPUI throttles inactive windows to ~30 fps by default; automated runs are inactive on purpose.
                    inactive_frame_interval: if interactive { WindowOptions::default().inactive_frame_interval } else { None },
                    ..Default::default()
                },
                move |window, cx| {
                    let root = cx.new(|cx| Root::new(a2, bridge, window.window_handle(), cx));
                    if a3.webview {
                        wv::attach(window);
                    }
                    let focus = root.read(cx).composer.read(cx).focus.clone();
                    window.focus(&focus, cx);
                    root
                },
            )
            .unwrap();
        if interactive {
            cx.activate(true);
        }

        if args.bench {
            let (rate, secs) = (args.rate, args.secs);
            let root = handle.update(cx, |r, _, _| r.cmd_tx.clone()).unwrap();
            let agent = args.agent;
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let _ = root.send(if agent {
                    Cmd::Agent { rate, secs, chars: 80_000, tools: 6 }
                } else {
                    Cmd::Stream { rate, secs, chars: 80_000 }
                });
            })
            .detach();
        }
        if args.scroll {
            let secs = args.secs;
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let start = Instant::now();
                let mut dir = -1.0f32;
                let mut step = 0u32;
                while start.elapsed().as_secs() < secs as u64 {
                    cx.background_executor().timer(Duration::from_millis(16)).await;
                    step += 1;
                    if step % 120 == 0 {
                        dir = -dir;
                    }
                    let ok = handle.update(cx, |root, _, cx| {
                        root.list.scroll_by(px(60. * dir));
                        cx.notify();
                    });
                    if ok.is_err() {
                        return;
                    }
                }
                handle
                    .update(cx, |root, window, cx| {
                        let s = window.frame_duration_snapshot();
                        let h = &s.draw_duration_histogram;
                        println!(
                            "SCROLLBENCH draw_ms n={} mean={:.2} p50={:.2} p95={:.2} p99={:.2} max={:.2}",
                            h.len(),
                            h.mean() / 1e6,
                            h.value_at_quantile(0.5) as f64 / 1e6,
                            h.value_at_quantile(0.95) as f64 / 1e6,
                            h.value_at_quantile(0.99) as f64 / 1e6,
                            h.max() as f64 / 1e6
                        );
                        let _ = root;
                        cx.quit();
                    })
                    .ok();
            })
            .detach();
        }
        if !args.show.is_empty() {
            let lang = args.show.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                handle
                    .update(cx, |root, _, cx| {
                        let ix = root.msgs.iter().position(|m| {
                            m.doc.blocks.iter().any(|b| matches!(b, md::Block::Code { lang: l, .. } if *l == lang))
                        });
                        if let Some(ix) = ix {
                            root.list.set_follow_mode(FollowMode::Normal);
                            root.list.scroll_to_reveal_item(ix);
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .detach();
        }
        if args.selftest {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if args.agent {
                    selftest_trace(handle, cx).await;
                } else {
                    selftest(handle, cx).await;
                }
            })
            .detach();
        }
    });
}

include!("selftest.rs");
include!("selftest_trace.rs");
