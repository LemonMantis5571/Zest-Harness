use super::*;

// Run the same external worker and independent reviewer as the coordinator integration tests.
fn fixture(root: &std::path::Path) -> tempfile::TempDir {
    use std::process::Command;
    let bin = tempfile::tempdir().unwrap();
    let exe = bin
        .path()
        .join(format!("gpui-fixture{}", std::env::consts::EXE_SUFFIX));
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/core/tests/fixtures/external_agent_fixture.rs");
    let result = Command::new("rustc")
        .arg("--edition=2021")
        .arg(source)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::write(root.join("board-fixture.txt"), "base\n").unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "board-fixture.txt"],
        vec![
            "-c",
            "user.name=Zest",
            "-c",
            "user.email=zest@localhost",
            "commit",
            "--no-verify",
            "-qm",
            "fixture",
        ],
    ] {
        let result = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let command = serde_json::to_string(&exe.to_string_lossy()).unwrap();
    std::fs::write(root.join("zest.toml"), format!("[agents.worker]\nmode = \"headless\"\ncommand = {command}\nargs = [\"delegation\", \"{{prompt}}\"]\nworkspace = \"isolated\"\ntimeout_secs = 30\n")).unwrap();
    bin
}

pub(in crate::workspace) async fn check(h: WindowHandle<Workspace>, cx: &mut AsyncApp) {
    let root = h.update(cx, |ws, _, _| ws.project.path.clone()).unwrap();
    let path = root.clone();
    let _binary = cx
        .background_executor()
        .spawn(async move { fixture(&path) })
        .await;
    h.update(cx, |ws, w, cx| {
        ws.page(Panel::Delegation, w, cx);
        let target = DelegationTargetOption {
            target: zest_coordinator::DelegationTargetView::ExternalAgent {
                agent_id: "worker".into(),
            },
            available: true,
            label: "Fixture worker".into(),
            error: None,
        };
        ws.board.targets = vec![target];
        let mut d = Draft::new(ws.active.clone(), cx);
        d.worker = Some(0);
        for (field, text) in [
            (&d.title, "Native delegation"),
            (&d.objective, "Create the fixture change"),
            (&d.scope, "delegated.txt"),
        ] {
            field.update(cx, |f, cx| f.replace_text_in_range(None, text, w, cx));
        }
        ws.board.draft = Some(d);
        ws.save_card(cx);
    })
    .unwrap();

    async fn wait(h: WindowHandle<Workspace>, cx: &mut AsyncApp, pred: impl Fn(&Board) -> bool) {
        for _ in 0..600 {
            if h.update(cx, |ws, _, _| pred(&ws.board)).unwrap() {
                return;
            }
            settle(cx, 100).await;
        }
        let info = h
            .update(cx, |ws, _, _| {
                format!(
                    "{} {:?}",
                    ws.board.message,
                    ws.board.selected_job().map(|j| (&j.status, &j.error))
                )
            })
            .unwrap();
        panic!("Delegation board timed out: {info}");
    }
    wait(h, cx, |b| {
        b.selected_job()
            .is_some_and(|j| j.status == Status::AwaitingApproval)
    })
    .await;
    h.update(cx, |ws, _, cx| {
        let j = ws.board.selected_job().unwrap();
        ws.act_on_card(Action::Approve, j.job_id.clone(), j.updated_at + 1, cx);
    })
    .unwrap();
    wait(h, cx, |b| !b.busy && b.message.contains("changed")).await;
    h.update(cx, |ws, _, cx| {
        let j = ws.board.selected_job().unwrap();
        assert_eq!(j.status, Status::AwaitingApproval);
        ws.act_on_card(Action::Approve, j.job_id.clone(), j.updated_at, cx);
    })
    .unwrap();
    wait(h, cx, |b| {
        b.selected_job().is_some_and(|j| {
            matches!(
                j.status,
                Status::ReadyToApply | Status::Blocked | Status::Failed | Status::ChangesRequested
            )
        })
    })
    .await;
    h.update(cx, |ws, _, cx| {
        let j = ws.board.selected_job().unwrap();
        assert_eq!(j.status, Status::ReadyToApply, "{:?}", j.error);
        assert!(j.attempts.len() >= 2);
        assert_eq!(j.changed_files, ["delegated.txt"]);
        assert!(!ws.project.path.join("delegated.txt").exists());
        ws.show_artifact("worker.diff", 0, cx);
    })
    .unwrap();
    wait(h, cx, |b| {
        b.artifact
            .as_ref()
            .is_some_and(|p| p.content.contains("+fixture worker change"))
    })
    .await;
    h.update(cx, |ws, _, cx| {
        let j = ws.board.selected_job().unwrap();
        ws.act_on_card(Action::Apply, j.job_id.clone(), j.updated_at, cx);
    })
    .unwrap();
    wait(h, cx, |b| {
        b.selected_job()
            .is_some_and(|j| j.status == Status::Accepted)
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("delegated.txt"))
            .unwrap()
            .trim(),
        "fixture worker change"
    );
    println!(
        "SELFTEST delegation_board: PASS create, stale approval, isolated worker, independent review, diff preview and apply"
    );
}
