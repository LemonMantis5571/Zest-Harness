use super::*;

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Zest Test",
            "-c",
            "user.email=zest@example.invalid",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

pub(super) fn init_repository(root: &std::path::Path) {
    git(root, &["init", "-q", "-b", "native-test"]);
    std::fs::write(root.join(".gitignore"), ".zest/\n").unwrap();
    git(root, &["add", ".gitignore"]);
    git(root, &["commit", "--no-verify", "-qm", "Initial checkout"]);
}

// Service runners lack an interactive Windows desktop. Exercise the same
// workspace scenario with a GPUI test window and the real Tokio agent.
#[::core::prelude::v1::test]
fn offline_workspace_smoke() {
    use futures::FutureExt;

    let project = tempfile::tempdir().unwrap();
    init_repository(project.path());
    let project_state = Project::open(project.path().to_owned(), true).unwrap();
    let mut app = gpui::TestAppContext::single();
    app.update(composer::bind_keys);
    let window = app.open_window(size(px(1150.), px(790.)), |w, cx| {
        Workspace::new(project_state, w, cx)
    });
    let mut cx = app.to_async();
    let mut scenario = std::pin::pin!(check(window, &mut cx));
    let tick = Duration::from_millis(10);
    let deadline = Instant::now() + Duration::from_secs(180);
    while scenario.as_mut().now_or_never().is_none() {
        app.run_until_parked();
        assert!(
            Instant::now() < deadline,
            "native workspace smoke test timed out"
        );
        // Tokio, Git and the delegation subprocess use real time. Advance
        // GPUI's simulated clock at their pace while their results arrive.
        std::thread::sleep(tick);
        app.executor().advance_clock(tick);
    }
    app.quit();
}

async fn changes(handle: WindowHandle<Workspace>, cx: &mut AsyncApp) -> String {
    handle
        .update(cx, |ws, w, cx| ws.page(Panel::Changes, w, cx))
        .unwrap();
    for _ in 0..150 {
        let report = handle.update(cx, |ws, _, _| ws.report.clone()).unwrap();
        if !report.contains("Reading Git") {
            return report;
        }
        settle(cx, 50).await;
    }
    panic!("Changes panel timed out");
}

pub(super) async fn check(handle: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    settle(cx, 300).await;
    let (first, root) = handle
        .update(cx, |ws, _, _| {
            (ws.active.clone(), ws.chats[&ws.active].clone())
        })
        .unwrap();
    let composer = root.read_with(cx, |r, _| r.composer.clone());
    for prompt in ["First native turn", "Second native turn"] {
        cx.update_window(handle.into(), |_, w, cx| {
            composer.update(cx, |c, cx| c.replace_text_in_range(None, prompt, w, cx));
            w.dispatch_keystroke(key("enter"), cx);
        })
        .unwrap();
    }
    for _ in 0..150 {
        if root.read_with(cx, |r, _| r.turns_done == 2) {
            break;
        }
        settle(cx, 75).await;
    }
    handle
        .update(cx, |ws, w, cx| {
            let saved = ws.project.store.load(&first).unwrap();
            assert_eq!(
                saved.messages.len(),
                4,
                "two saved turns: {:?}",
                root.read(cx).note
            );
            assert_eq!(saved.agent_messages.len(), 4);
            assert!(saved.pending_inputs.is_empty());
            assert_eq!(saved.checkpoints.len(), 2);
            assert!(
                saved.messages[3]
                    .searchable_text()
                    .contains("3 wire messages")
            );
            ws.split(w, cx);
            assert_eq!(
                ws.layout.as_ref().unwrap().ids(),
                [first.clone(), ws.active.clone()]
            );
            let second = ws.active.clone();
            ws.add_pane(Direction::Down, w, cx);
            let third = ws.active.clone();
            ws.zoom_active(cx);
            assert!(!ws.layout.as_ref().unwrap().visible(&second));
            ws.zoom_active(cx);
            ws.minimize_active(w, cx);
            assert!(!ws.layout.as_ref().unwrap().visible(&third));
            ws.split(w, cx);
            assert!(ws.layout.is_none());
            ws.reopen_group(ws.saved_layouts.len() - 1, w, cx);
            assert_eq!(
                ws.layout.as_ref().unwrap().ids(),
                [first.clone(), second.clone(), third.clone()]
            );
            ws.open(&third, w, cx);
            assert!(ws.layout.as_ref().unwrap().visible(&third));
            ws.close_pane(&third, w, cx);
            assert_eq!(ws.layout.as_ref().unwrap().ids(), [first.clone(), second]);
            ws.open(&first, w, cx);
            assert_eq!(ws.active, first);
            ws.fork(Some(&saved.checkpoints[1].id), w, cx);
            assert_ne!(ws.active, first);
            assert_eq!(ws.chat(cx).thread.lock().unwrap().messages.len(), 2);
            for page in [
                Panel::Settings,
                Panel::Search,
                Panel::Usage,
                Panel::Commands,
                Panel::Checkpoints,
                Panel::Jobs,
            ] {
                ws.page(page, w, cx);
                let _ = ws.panel_body(cx);
            }
        })
        .unwrap();
    assert_eq!(changes(handle, cx).await, "Working tree clean.");
    handle
        .update(cx, |ws, w, cx| ws.open(&first, w, cx))
        .unwrap();
    for (prompt, response, count) in [
        ("Demo: write", interaction::Response::Deny, 3),
        ("Demo: write", interaction::Response::Allow, 4),
        (
            "Demo: question",
            interaction::Response::Answer(vec!["Second".into()]),
            5,
        ),
    ] {
        root.update(cx, |r, cx| r.submit_native(prompt, &[], cx));
        for _ in 0..150 {
            if root.read_with(cx, |r, _| !r.cards.is_empty()) {
                break;
            }
            settle(cx, 50).await;
        }
        root.update(cx, |r, cx| {
            let card = r.cards.front().unwrap_or_else(|| {
                panic!(
                    "{prompt} expected a decision, busy={}, queued={}, turns={}, status={}",
                    r.turn_active,
                    r.pending.len(),
                    r.turns_done,
                    r.note
                )
            });
            match &card.request {
                interaction::Request::Approval { preview, .. } => {
                    assert!(preview.contains("demo.txt"))
                }
                interaction::Request::Question { choices, .. } => {
                    assert_eq!(choices, &["First", "Second"])
                }
            }
            r.respond(response, cx);
        });
        for _ in 0..150 {
            if root.read_with(cx, |r, _| r.turns_done == count) {
                break;
            }
            settle(cx, 50).await;
        }
        handle
            .update(cx, |ws, _, cx| {
                assert_eq!(root.read(cx).turns_done, count);
                assert_eq!(ws.project.path.join("demo.txt").is_file(), count >= 4);
            })
            .unwrap();
    }
    let project_path = handle
        .update(cx, |ws, _, _| ws.project.path.clone())
        .unwrap();
    git(&project_path, &["add", "demo.txt"]);
    git(
        &project_path,
        &[
            "commit",
            "--no-verify",
            "-qm",
            "Commit approved native edit",
        ],
    );
    assert!(git(&project_path, &["status", "--porcelain"]).is_empty());
    let committed_report = changes(handle, cx).await;
    assert!(
        committed_report.contains("+Approved in GPUI"),
        "{committed_report}"
    );
    // A split opened after the commit has its own baseline, independent of the first chat.
    handle
        .update(cx, |ws, w, cx| {
            if ws.layout.is_some() {
                ws.split(w, cx);
            }
            ws.split(w, cx);
        })
        .unwrap();
    assert_eq!(changes(handle, cx).await, "Working tree clean.");
    handle
        .update(cx, |ws, w, cx| {
            let saved = ws.project.store.load(&first).unwrap();
            assert!(
                serde_json::to_string(&saved.agent_messages)
                    .unwrap()
                    .contains("Second")
            );
            ws.chats.remove(&first);
            ws.open(&first, w, cx);
            assert_eq!(
                ws.chat(cx).thread.lock().unwrap().agent_messages.len(),
                saved.agent_messages.len()
            );
        })
        .unwrap();
    assert_eq!(changes(handle, cx).await, committed_report);
    handle
        .update(cx, |ws, w, cx| {
            ws.open(&first, w, cx);
            let view = ws.chats[&first].clone();
            let attachment = attachments::prepare_paths(&[ws.project.path.join("demo.txt")])
                .pop()
                .unwrap();
            assert_eq!(attachment.status, "done");
            view.update(cx, |r, cx| {
                r.files.push(native::prepared(attachment));
                r.submit_native("Read attachment", &[], cx);
            });
        })
        .unwrap();
    let reopened = handle
        .update(cx, |ws, _, _| ws.chats[&first].clone())
        .unwrap();
    for _ in 0..150 {
        if reopened.read_with(cx, |r, _| r.turns_done == 1) {
            break;
        }
        settle(cx, 50).await;
    }
    let n = reopened.update(cx, |r, cx| {
        let chat = r.native.clone().unwrap();
        let t = chat.project.store.load(&first).unwrap();
        let input = &t.agent_messages[t.agent_messages.len() - 2];
        assert!(
            serde_json::to_string(input)
                .unwrap()
                .contains("Approved in GPUI")
        );
        r.submit_native("/btw side question", &[], cx);
        t.messages.len()
    });
    for _ in 0..150 {
        if reopened.read_with(cx, |r, _| r.side.finished_answer()) {
            break;
        }
        settle(cx, 50).await;
    }
    reopened.update(cx, |r, cx| {
        assert!(r.side.finished_answer());
        assert_eq!(
            r.native
                .as_ref()
                .unwrap()
                .project
                .store
                .load(&first)
                .unwrap()
                .messages
                .len(),
            n
        );
        r.close_side(cx);
    });
    handle
        .update(cx, |ws, w, cx| ws.page(Panel::Files, w, cx))
        .unwrap();
    for _ in 0..100 {
        if handle
            .update(cx, |ws, _, _| {
                ws.entries.iter().any(|e| e.path == "demo.txt")
            })
            .unwrap()
        {
            break;
        }
        settle(cx, 50).await;
    }
    handle
        .update(cx, |ws, _, cx| {
            assert!(ws.entries.iter().any(|e| e.path == "demo.txt"));
            ws.browse("demo.txt".into(), false, cx);
        })
        .unwrap();
    for _ in 0..100 {
        if handle.update(cx, |ws, _, _| ws.preview.is_some()).unwrap() {
            break;
        }
        settle(cx, 50).await;
    }
    handle
        .update(cx, |ws, w, cx| {
            assert!(
                ws.preview
                    .as_ref()
                    .unwrap()
                    .content
                    .contains("Approved in GPUI")
            );
            ws.page(Panel::Search, w, cx);
            ws.filter.update(cx, |f, cx| {
                f.replace_text_in_range(None, "Second native turn", w, cx)
            });
        })
        .unwrap();
    for _ in 0..100 {
        if handle.update(cx, |ws, _, _| !ws.hits.is_empty()).unwrap() {
            break;
        }
        settle(cx, 50).await;
    }
    handle
        .update(cx, |ws, w, cx| {
            let hit = ws.hits.iter().find(|h| h.thread == first).unwrap().clone();
            ws.open_hit(&hit, w, cx);
        })
        .unwrap();
    let wire_len = reopened.update(cx, |r, cx| {
        let n = r
            .native
            .as_ref()
            .unwrap()
            .thread
            .lock()
            .unwrap()
            .agent_messages
            .len();
        r.submit_native("Demo: write", &[], cx);
        n
    });
    for _ in 0..100 {
        if reopened.read_with(cx, |r, _| !r.cards.is_empty()) {
            break;
        }
        settle(cx, 50).await;
    }
    reopened.update(cx, |r, cx| {
        assert!(!r.cards.is_empty());
        r.submit_native("Pending after stop", &[], cx);
        r.stop(cx);
    });
    for _ in 0..100 {
        if reopened.read_with(cx, |r, _| !r.turn_active) {
            break;
        }
        settle(cx, 50).await;
    }
    let restored = handle
        .update(cx, |ws, w, cx| {
            let t = ws.project.store.load(&first).unwrap();
            assert_eq!(t.agent_messages.len(), wire_len);
            assert_eq!(t.pending_inputs.len(), 1);
            let runs = zest_core::ChatPersistence::open(&ws.project.path)
                .unwrap()
                .runs
                .list_by_thread(&first)
                .unwrap();
            assert!(
                runs.iter()
                    .any(|r| r.status == zest_core::RunStatus::Aborted)
            );
            assert!(
                runs.iter()
                    .any(|r| r.status == zest_core::RunStatus::Completed)
            );
            ws.new_chat(w, cx);
            ws.chats.remove(&first);
            ws.open(&first, w, cx);
            ws.chats[&first].clone()
        })
        .unwrap();
    handle
        .update(cx, |_, w, cx| {
            restored.update(cx, |r, cx| {
                assert!(!r.turn_active);
                let key = r.pending[0].saved_id.clone().unwrap();
                r.edit_queued(&key, w, cx);
                r.queue_input.update(cx, |c, cx| {
                    c.replace_text_in_range(None, "Edited after restart", w, cx);
                });
            });
        })
        .unwrap();
    settle(cx, 75).await;
    handle
        .update(cx, |_, w, cx| {
            w.dispatch_keystroke(key("enter"), cx);
        })
        .unwrap();
    settle(cx, 75).await;
    restored.update(cx, |r, cx| {
        assert_eq!(&r.pending[0].text, "Edited after restart");
        r.start_next();
        cx.notify();
    });
    for _ in 0..150 {
        if restored.read_with(cx, |r, _| r.turns_done == 1) {
            break;
        }
        settle(cx, 50).await;
    }
    restored.read_with(cx, |r, _| {
        let chat = r.native.as_ref().unwrap();
        let t = chat.project.store.load(&first).unwrap();
        assert!(t.pending_inputs.is_empty());
        assert_eq!(t.agent_messages.len(), wire_len + 2);
        assert_eq!(
            t.messages[t.messages.len() - 2].searchable_text(),
            "Edited after restart"
        );
    });
    println!(
        "SELFTEST native_workspace: PASS chat, queue edit and resume after reopen, run records, wire context, checkpoint fork, split, committed changes after reopen, deny/allow, question, file attachment, file preview, transcript search, side chat isolation and workbench"
    );
    board::selftest::check(handle, cx).await;
    cx.update(|cx| cx.quit());
}

#[cfg(test)]
#[::core::prelude::v1::test]
fn attention_route_follows_window_and_chat_visibility() {
    assert_eq!(chat_alert_route(true, true), AlertRoute::Quiet);
    assert_eq!(chat_alert_route(true, false), AlertRoute::Banner);
    assert_eq!(chat_alert_route(false, true), AlertRoute::System);
    assert_eq!(chat_alert_route(false, false), AlertRoute::System);
}
