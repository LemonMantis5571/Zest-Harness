// Included into main.rs (textually) so it can reach Root's private fields.
// In-process end-to-end checks: events are injected through `Window::dispatch_event`, so they take
// the same path as OS input after the platform layer, and the clipboard is the real Windows one.
// `inject` runs outside a lease on Root, because Root's own listeners update Root.

fn report(name: &str, ok: bool, detail: impl std::fmt::Display) {
    println!("SELFTEST {name}: {} {detail}", if ok { "PASS" } else { "FAIL" });
}

async fn settle(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

fn inject<R>(cx: &mut AsyncApp, handle: WindowHandle<Root>, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
    cx.update_window(handle.into(), |_, w, cx| f(w, cx)).unwrap()
}

fn key(s: &'static str) -> Keystroke {
    Keystroke::parse(s).unwrap()
}
fn down(p: Point<Pixels>) -> PlatformInput {
    PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: p,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    })
}
fn up(p: Point<Pixels>) -> PlatformInput {
    PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position: p,
        modifiers: Modifiers::default(),
        click_count: 1,
    })
}
fn mv(p: Point<Pixels>) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position: p,
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}
fn clipboard_text(cx: &mut AsyncApp, handle: WindowHandle<Root>) -> String {
    inject(cx, handle, |_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default())
}

async fn selftest(handle: WindowHandle<Root>, cx: &mut AsyncApp) {
    // 1/2/3: stick-to-bottom and release on scroll-up.
    let following = handle.update(cx, |r, _, _| r.list.is_following_tail()).unwrap();
    report("follow_initial", following, "list starts following the tail");
    let center = point(px(500.), px(300.));
    let mut released = false;
    for dy in [px(400.), px(-400.)] {
        inject(cx, handle, |w, cx| {
            w.dispatch_event(
                PlatformInput::Scroll(ScrollEvent {
                    position: center,
                    delta: ScrollDelta::Pixels(point(px(0.), dy)),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                    source: ScrollSource::Pointer,
                }),
                cx,
            );
        });
        settle(cx, 200).await;
        if !handle.update(cx, |r, _, _| r.list.is_following_tail()).unwrap() {
            released = true;
            report("scroll_up_releases_tail", true, format!("wheel dy={dy:?} released the tail"));
            break;
        }
    }
    if !released {
        report("scroll_up_releases_tail", false, "tail still followed after wheel events");
    }
    handle
        .update(cx, |r, _, cx| {
            r.list.set_follow_mode(FollowMode::Tail);
            cx.notify();
        })
        .unwrap();
    settle(cx, 300).await;
    let f = handle.update(cx, |r, _, _| r.list.is_following_tail()).unwrap();
    report("jump_to_latest_refollows", f, "");

    // 4: drag-select across blocks, then copy.
    let units = handle
        .update(cx, |r, _, _| {
            let s = r.shared.borrow();
            let mut hits: Vec<((usize, usize), Bounds<Pixels>)> = s.hits.iter().map(|(k, h)| (*k, h.bounds)).collect();
            hits.sort_by(|a, b| a.1.top().partial_cmp(&b.1.top()).unwrap());
            hits
        })
        .unwrap();
    report("visible_units_registered", units.len() > 3, format!("{} painted text units", units.len()));
    if units.len() > 3 {
        let a = units[1].1;
        let b = units[units.len() - 2].1;
        let start = point(a.left() + px(3.), a.top() + px(4.));
        let end = point(b.left() + b.size.width * 0.6, b.top() + px(4.));
        inject(cx, handle, |w, cx| {
            w.dispatch_event(down(start), cx);
            for i in 1..=10 {
                let f = i as f32 / 10.;
                w.dispatch_event(mv(point(start.x + (end.x - start.x) * f, start.y + (end.y - start.y) * f)), cx);
            }
            w.dispatch_event(up(end), cx);
        });
        settle(cx, 250).await;
        let span = handle
            .update(cx, |r, _, _| r.shared.borrow().ordered().map(|(a, b)| (a.msg, a.unit, a.off, b.msg, b.unit, b.off)))
            .unwrap();
        inject(cx, handle, |w, cx| w.dispatch_action(Box::new(CopySel), cx));
        settle(cx, 100).await;
        let clip = clipboard_text(cx, handle);
        let head: String = clip.chars().take(50).collect();
        let tail: String = clip.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
        report(
            "drag_select_copy_multi_block",
            clip.len() > 20 && span.is_some_and(|s| (s.0, s.1) != (s.3, s.4)),
            format!("(msg,unit,off) {span:?} -> {} chars copied: {head:?} ... {tail:?}", clip.chars().count()),
        );
    }

    // A selection that reaches off-screen messages is copied from the model, not from painted units.
    handle
        .update(cx, |r, _, _| {
            let last = r.msgs.len() - 1;
            let lu = r.msgs[last].doc.units.len().saturating_sub(1);
            let llen = r.msgs[last].doc.units.get(lu).map(|u| u.text.len()).unwrap_or(0);
            let mut s = r.shared.borrow_mut();
            s.anchor = Some(Pos { msg: last - 40, unit: 0, off: 0 });
            s.focus = Some(Pos { msg: last, unit: lu, off: llen });
        })
        .unwrap();
    inject(cx, handle, |w, cx| w.dispatch_action(Box::new(CopySel), cx));
    settle(cx, 100).await;
    let n = clipboard_text(cx, handle).len();
    report("copy_spans_offscreen_messages", n > 5000, format!("{n} bytes from 41 messages, most not on screen"));
    handle
        .update(cx, |r, _, cx| {
            let mut s = r.shared.borrow_mut();
            s.anchor = None;
            s.focus = None;
            cx.notify();
        })
        .unwrap();

    // 5: link click.
    settle(cx, 300).await;
    let link_pt = handle
        .update(cx, |r, _, _| {
            let s = r.shared.borrow();
            for (k, h) in &s.hits {
                let Some(u) = r.msgs.get(k.0).and_then(|m| m.doc.units.get(k.1)) else { continue };
                for (range, kind) in &u.spans {
                    if matches!(kind, md::SpanKind::Link(_)) {
                        if let Some(p) = h.layout.position_for_index(range.start + 1) {
                            return Some(point(p.x + px(2.), p.y + px(8.)));
                        }
                    }
                }
            }
            None
        })
        .unwrap();
    if let Some(p) = link_pt {
        inject(cx, handle, |w, cx| {
            w.dispatch_event(down(p), cx);
            w.dispatch_event(up(p), cx);
        });
        settle(cx, 150).await;
        let l = handle.update(cx, |r, _, _| r.last_link.clone()).unwrap();
        report("link_click", l.is_some(), format!("{l:?}"));
    } else {
        report("link_click", false, "no link visible in viewport");
    }

    // 6/7: composer.
    let comp = handle.update(cx, |r, _, _| r.composer.clone()).unwrap();
    inject(cx, handle, |w, cx| {
        let f = comp.read(cx).focus.clone();
        w.focus(&f, cx);
    });
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let text = format!("a{family}b");
    inject(cx, handle, |w, cx| {
        comp.update(cx, |c, cx| c.replace_text_in_range(None, &text, w, cx));
        w.dispatch_action(Box::new(composer::Left), cx);
    });
    let after_one = comp.read_with(cx, |c, _| c.cursor());
    inject(cx, handle, |w, cx| w.dispatch_action(Box::new(composer::Left), cx));
    let after_two = comp.read_with(cx, |c, _| c.cursor());
    report(
        "caret_moves_by_grapheme",
        after_one == 1 + family.len() && after_two == 1,
        format!(
            "ZWJ family ({} bytes, {} chars) crossed in one Left: cursor {after_one} then {after_two}",
            family.len(),
            family.chars().count()
        ),
    );
    inject(cx, handle, |w, cx| {
        comp.update(cx, |c, cx| {
            c.sel = 0..c.text.len();
            c.replace_text_in_range(None, &"wrapping words in the composer ".repeat(14), w, cx)
        });
    });
    settle(cx, 300).await;
    let (lines1, h1) = comp.read_with(cx, |c, _| c.debug());
    inject(cx, handle, |w, cx| {
        w.dispatch_keystroke(Keystroke::parse("shift-enter").unwrap(), cx);
        w.dispatch_keystroke(Keystroke::parse("shift-enter").unwrap(), cx);
        w.dispatch_keystroke(Keystroke::parse("x").unwrap(), cx);
    });
    settle(cx, 300).await;
    let (lines2, h2) = comp.read_with(cx, |c, _| c.debug());
    let nl = comp.read_with(cx, |c, _| c.text.matches('\n').count());
    report(
        "composer_soft_wrap_and_growth",
        lines1 > 1 && h2 > h1,
        format!("{lines1} visual lines / {h1}px; after 2x Shift+Enter + 'x': {lines2} lines / {h2}px; {nl} newlines in text"),
    );
    let before = handle.update(cx, |r, _, _| r.msgs.len()).unwrap();
    inject(cx, handle, |w, cx| {
        w.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
    });
    settle(cx, 400).await;
    let (after, cleared) = handle.update(cx, |r, _, cx| (r.msgs.len(), r.composer.read(cx).text.is_empty())).unwrap();
    report("enter_sends", after > before && cleared, format!("messages {before} -> {after}, composer cleared={cleared}"));
    handle.update(cx, |r, _, _| { let _ = r.cmd_tx.send(Cmd::Cancel); }).unwrap();

    // Caret navigation, selection and clipboard inside the composer.
    let nav_text = "alpha beta\ngamma delta epsilon\nzeta";
    inject(cx, handle, |_, cx| {
        comp.update(cx, |c, cx| {
            c.text = nav_text.to_string();
            let n = c.text.len();
            c.sel = n..n;
            cx.notify();
        })
    });
    settle(cx, 300).await;
    inject(cx, handle, |w, cx| { w.dispatch_keystroke(key("up"), cx); });
    let up_pos = comp.read_with(cx, |c, _| c.cursor());
    inject(cx, handle, |w, cx| { w.dispatch_keystroke(key("home"), cx); });
    let home_pos = comp.read_with(cx, |c, _| c.cursor());
    inject(cx, handle, |w, cx| { w.dispatch_keystroke(key("down"), cx); });
    let down_pos = comp.read_with(cx, |c, _| c.cursor());
    inject(cx, handle, |w, cx| { w.dispatch_keystroke(key("end"), cx); });
    let end_pos = comp.read_with(cx, |c, _| c.cursor());
    let l2 = nav_text.find('\n').unwrap() + 1;
    let l3 = nav_text.rfind('\n').unwrap() + 1;
    report(
        "composer_up_home_down_end",
        (l2..l3 - 1).contains(&up_pos) && home_pos == l2 && down_pos >= l3 && end_pos == nav_text.len(),
        format!("up={up_pos} (line 2 is {l2}..{}), home={home_pos}, down={down_pos} (line 3 starts {l3}), end={end_pos}", l3 - 1),
    );
    inject(cx, handle, |w, cx| { for _ in 0..3 { w.dispatch_keystroke(key("shift-left"), cx); } });
    let sel3 = comp.read_with(cx, |c, _| c.sel.clone());
    inject(cx, handle, |w, cx| { w.dispatch_keystroke(key("ctrl-a"), cx); });
    let all = comp.read_with(cx, |c, _| c.sel.clone());
    inject(cx, handle, |w, cx| { w.dispatch_keystroke(key("ctrl-c"), cx); });
    let copied = clipboard_text(cx, handle);
    inject(cx, handle, |w, cx| { w.dispatch_keystroke(key("ctrl-x"), cx); });
    let (after_cut, clip_cut) = (comp.read_with(cx, |c, _| c.text.clone()), clipboard_text(cx, handle));
    report(
        "composer_select_copy_cut",
        sel3.len() == 3 && all == (0..nav_text.len()) && copied == nav_text && after_cut.is_empty() && clip_cut == nav_text,
        format!("shift-left x3 -> {sel3:?}; ctrl-a -> {all:?}; copied {} chars; after ctrl-x text is empty={}", copied.len(), after_cut.is_empty()),
    );
    inject(cx, handle, |_, cx| {
        comp.update(cx, |c, cx| {
            c.text = "mouse selection in the composer, a long line that wraps over several visual lines when the window is narrow enough to need it. ".repeat(3);
            c.sel = 0..0;
            cx.notify();
        })
    });
    settle(cx, 300).await;
    if let Some(b) = comp.read_with(cx, |c, _| c.bounds()) {
        let (s, e) = (point(b.left() + px(4.), b.top() + px(4.)), point(b.right() - px(40.), b.bottom() - px(4.)));
        inject(cx, handle, |w, cx| {
            w.dispatch_event(down(s), cx);
            for i in 1..=8 {
                let f = i as f32 / 8.;
                w.dispatch_event(mv(point(s.x + (e.x - s.x) * f, s.y + (e.y - s.y) * f)), cx);
            }
            w.dispatch_event(up(e), cx);
        });
        let sel = comp.read_with(cx, |c, _| c.sel.clone());
        report("composer_mouse_drag_select", sel.len() > 20, format!("dragged selection {sel:?}"));
    }
    inject(cx, handle, |_, cx| {
        comp.update(cx, |c, cx| {
            c.text.clear();
            c.sel = 0..0;
            cx.notify();
        })
    });

    // Clipboard paste through the real Windows clipboard.
    inject(cx, handle, |w, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("pasted\r\nsecond line".into()));
        w.dispatch_action(Box::new(composer::PasteSel), cx);
    });
    let t = comp.read_with(cx, |c, _| c.text.clone());
    report("paste_text", t == "pasted\nsecond line", format!("{t:?}"));
    let png = {
        let mut buf = std::io::Cursor::new(Vec::new());
        let img = image::RgbaImage::from_fn(64, 48, |x, y| image::Rgba([(x * 4) as u8, (y * 5) as u8, 160, 255]));
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    };
    inject(cx, handle, |w, cx| {
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, png.clone())));
        let back = cx.read_from_clipboard().map(|c| {
            c.entries()
                .iter()
                .map(|e| match e {
                    ClipboardEntry::Image(i) => format!("image {:?} {}b", i.format, i.bytes().len()),
                    ClipboardEntry::String(_) => "string".to_string(),
                    _ => "other".to_string(),
                })
                .collect::<Vec<_>>()
        });
        println!("SELFTEST clipboard_roundtrip: wrote png {}b, read back {back:?}", png.len());
        w.dispatch_action(Box::new(composer::PasteSel), cx);
    });
    settle(cx, 200).await;
    let imgs = comp.read_with(cx, |c, _| c.images.len());
    let tr = comp.read_with(cx, |c, _| c.trace.last().cloned());
    report("paste_image", imgs == 1, format!("{imgs} image attachment(s); trace: {tr:?}"));
    inject(cx, handle, |_, cx| cx.quit());
}
