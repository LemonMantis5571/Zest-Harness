// Included into main.rs after selftest.rs (shares its helpers: report, settle, inject, down, up, ...).
// `--selftest --agent`: nested Markdown, reasoning and tool rows, expand/collapse and the tail.

fn describe(m: &Msg) -> String {
    match &m.trace {
        Some(Trace { kind: TraceKind::Thinking { .. }, .. }) => "think".into(),
        Some(Trace { kind: TraceKind::Tool { name, status, .. }, .. }) => format!("tool:{name}:{status:?}"),
        None if m.user => "user".into(),
        None => "assistant".into(),
    }
}

/// First row with a header fully inside the viewport, as (index, header click point).
fn visible_trace(r: &Root) -> Option<(usize, Point<Pixels>)> {
    let vp = r.list.viewport_bounds();
    (0..r.msgs.len()).find_map(|ix| {
        r.msgs[ix].trace.as_ref()?;
        let b = *r.shared.borrow().rows.get(&ix)?;
        (b.top() >= vp.top() + px(4.) && b.top() + px(30.) <= vp.bottom())
            .then(|| (ix, point(b.left() + px(100.), b.top() + px(14.))))
    })
}

async fn click_at(cx: &mut AsyncApp, handle: WindowHandle<Root>, p: Point<Pixels>) {
    inject(cx, handle, |w, cx| {
        w.dispatch_event(down(p), cx);
        w.dispatch_event(up(p), cx);
    });
    settle(cx, 300).await;
}

async fn wheel(cx: &mut AsyncApp, handle: WindowHandle<Root>, dy: Pixels) {
    inject(cx, handle, |w, cx| {
        w.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: point(px(500.), px(300.)),
                delta: ScrollDelta::Pixels(point(px(0.), dy)),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            }),
            cx,
        );
    });
    settle(cx, 250).await;
}

async fn selftest_trace(handle: WindowHandle<Root>, cx: &mut AsyncApp) {
    // Nested blocks under list items keep their indent.
    let doc = md::parse(
        "1. First step\n\n   ```rust\n   fn a() {}\n   ```\n\n2. Second step\n\n   | a | b |\n   | - | - |\n   | 1 | 2 |\n\nTop level.\n",
    );
    let code = doc.blocks.iter().find_map(|b| if let md::Block::Code { indent, .. } = b { Some(*indent) } else { None });
    let table = doc.blocks.iter().find_map(|b| if let md::Block::Table { indent, .. } = b { Some(*indent) } else { None });
    let items = doc.blocks.iter().filter(|b| matches!(b, md::Block::Item { .. })).count();
    report(
        "md_nested_blocks_keep_list_indent",
        code == Some(1) && table == Some(1) && items == 2,
        format!("code indent {code:?}, table indent {table:?}, {items} items"),
    );

    // The agent-shaped history is made of reasoning and tool rows.
    let (nthink, ntool, n) = handle
        .update(cx, |r, _, _| {
            let d: Vec<String> = r.msgs.iter().map(describe).collect();
            (d.iter().filter(|s| *s == "think").count(), d.iter().filter(|s| s.starts_with("tool:")).count(), d.len())
        })
        .unwrap();
    report("agent_history_has_trace_rows", nthink >= 80 && ntool >= 240, format!("{n} rows: {nthink} reasoning, {ntool} tool"));

    // A live turn through the bridge: reasoning, three tool calls, then the answer, in that order.
    let (before, turns0) = handle.update(cx, |r, _, _| (r.msgs.len(), r.turns_done)).unwrap();
    handle
        .update(cx, |r, _, _| {
            let _ = r.cmd_tx.send(Cmd::Agent { rate: 300, secs: 10, chars: 1500, tools: 3 });
        })
        .unwrap();
    for _ in 0..200 {
        settle(cx, 100).await;
        if handle.update(cx, |r, _, _| r.turns_done).unwrap() > turns0 {
            break;
        }
    }
    settle(cx, 300).await;
    let rows: Vec<String> = handle.update(cx, |r, _, _| r.msgs[before..].iter().map(describe).collect()).unwrap();
    let ok = rows.len() == 5
        && rows[0] == "think"
        && rows[1..4].iter().all(|s| s.starts_with("tool:") && !s.ends_with("Running"))
        && rows[4] == "assistant";
    report("live_turn_row_order", ok, format!("{rows:?}"));
    let titles = handle
        .update(cx, |r, _, _| {
            r.msgs[before + 1..before + 4].iter().filter_map(|m| m.trace.as_ref().map(|t| t.title.clone())).collect::<Vec<_>>()
        })
        .unwrap();
    report("tool_rows_carry_result_title", titles.iter().all(|t| !t.is_empty()), format!("{titles:?}"));

    // Following the tail: a click on a tool header expands it and the bottom stays pinned. The answer above
    // is taller than the viewport, so run a second turn that ends on tool rows (no answer text).
    let turns1 = handle.update(cx, |r, _, _| r.turns_done).unwrap();
    handle
        .update(cx, |r, _, _| {
            let _ = r.cmd_tx.send(Cmd::Agent { rate: 300, secs: 10, chars: 0, tools: 3 });
        })
        .unwrap();
    for _ in 0..100 {
        settle(cx, 100).await;
        if handle.update(cx, |r, _, _| r.turns_done).unwrap() > turns1 {
            break;
        }
    }
    settle(cx, 400).await;
    let tail = handle.update(cx, |r, _, _| r.list.is_following_tail()).unwrap();
    // The new turn's own tool rows can sit above the viewport when the answer is tall, so any visible row will do.
    let target = handle.update(cx, |r, _, _| visible_trace(r)).unwrap();
    if let (true, Some((ix, p))) = (tail, target) {
        let h0 = handle.update(cx, |r, _, _| r.shared.borrow().rows.get(&ix).map(|b| b.size.height)).unwrap();
        click_at(cx, handle, p).await;
        let (open, h1, still_tail, last_bottom, vp_bottom) = handle
            .update(cx, |r, _, _| {
                let last = r.msgs.len() - 1;
                (
                    r.msgs[ix].trace.as_ref().is_some_and(|t| t.open),
                    r.shared.borrow().rows.get(&ix).map(|b| b.size.height),
                    r.list.is_following_tail(),
                    r.shared.borrow().rows.get(&last).map(|b| b.bottom()),
                    r.list.viewport_bounds().bottom(),
                )
            })
            .unwrap();
        let pinned = last_bottom.is_some_and(|b| (b - vp_bottom).abs() < px(2.));
        report(
            "click_expands_row_tail_stays_pinned",
            open && h1 > h0 && still_tail && pinned,
            format!("row {ix}: open={open}, height {h0:?} -> {h1:?}, following={still_tail}, last item bottom {last_bottom:?} vs viewport {vp_bottom:?}"),
        );
    } else {
        report("click_expands_row_tail_stays_pinned", false, format!("following={tail}, no visible trace row: {target:?}"));
    }

    // Scrolled up: expanding a row leaves its own top edge where it was.
    let mut released = false;
    let mut up_dy = px(400.);
    for dy in [px(400.), px(-400.)] {
        wheel(cx, handle, dy).await;
        if !handle.update(cx, |r, _, _| r.list.is_following_tail()).unwrap() {
            released = true;
            up_dy = dy;
            break;
        }
    }
    // Keep scrolling up until a reasoning or tool header is fully on screen.
    let mut target = handle.update(cx, |r, _, _| visible_trace(r)).unwrap();
    for _ in 0..12 {
        if target.is_some() || !released {
            break;
        }
        wheel(cx, handle, up_dy).await;
        target = handle.update(cx, |r, _, _| visible_trace(r)).unwrap();
    }
    if let (true, Some((ix, p))) = (released, target) {
        let (t0, h0) =
            handle.update(cx, |r, _, _| r.shared.borrow().rows.get(&ix).map(|b| (b.top(), b.size.height))).unwrap().unwrap();
        click_at(cx, handle, p).await;
        let (open, t1, h1, tail) = handle
            .update(cx, |r, _, _| {
                let b = *r.shared.borrow().rows.get(&ix).unwrap();
                (r.msgs[ix].trace.as_ref().is_some_and(|t| t.open), b.top(), b.size.height, r.list.is_following_tail())
            })
            .unwrap();
        report(
            "click_expands_row_in_place_when_scrolled_up",
            open && h1 > h0 && (t1 - t0).abs() < px(2.) && !tail,
            format!("row {ix}: top {t0:?} -> {t1:?}, height {h0:?} -> {h1:?}, following={tail}"),
        );
        // Collapse again so the copy check below starts from the seeded state.
        click_at(cx, handle, p).await;
    } else {
        report("click_expands_row_in_place_when_scrolled_up", false, format!("released={released}, visible row {target:?}"));
    }

    // Copy: a collapsed reasoning row is hidden, so a selection across it must not carry its text.
    let marker = "The user is asking about";
    let select_0_to_6 = |open: bool, cx: &mut AsyncApp| {
        handle
            .update(cx, |r, _, cx| {
                r.msgs[1].trace.as_mut().unwrap().open = open;
                let end = r.msgs[6].doc.units[0].text.len();
                let mut s = r.shared.borrow_mut();
                s.anchor = Some(Pos { msg: 0, unit: 0, off: 0 });
                s.focus = Some(Pos { msg: 6, unit: 0, off: end });
                drop(s);
                cx.notify();
            })
            .unwrap()
    };
    select_0_to_6(false, cx);
    inject(cx, handle, |w, cx| w.dispatch_action(Box::new(CopySel), cx));
    settle(cx, 100).await;
    let hidden = clipboard_text(cx, handle);
    select_0_to_6(true, cx);
    inject(cx, handle, |w, cx| w.dispatch_action(Box::new(CopySel), cx));
    settle(cx, 100).await;
    let shown = clipboard_text(cx, handle);
    report(
        "copy_skips_collapsed_rows",
        !hidden.contains(marker) && shown.contains(marker) && shown.len() > hidden.len(),
        format!(
            "collapsed copy {} chars (has reasoning text: {}), expanded copy {} chars (has it: {})",
            hidden.len(),
            hidden.contains(marker),
            shown.len(),
            shown.contains(marker)
        ),
    );
    inject(cx, handle, |_, cx| cx.quit());
}
