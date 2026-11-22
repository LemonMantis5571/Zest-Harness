//! Timings for shared Rust operations performed by both desktop frontends.
use std::hint::black_box;
use std::time::Instant;
use zest_core::{CancelToken, MessageWindow, THREAD_WINDOW_USER_TURNS, Thread, ThreadStore};

#[allow(dead_code)]
#[path = "../src/seed.rs"]
mod seed;

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_secs_f64() * 1000.)
}

fn main() {
    let marker = "a uniquely searchable sentence in the long transcript";
    for _ in 0..3 {
        let folder = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(folder.path()).unwrap();
        for i in 0..99 {
            let mut small = Thread::new();
            small.apply_user("prompt", &format!("Small chat {i}"));
            store.save(&small).unwrap();
        }
        let mut thread = Thread::new();
        for i in 0..2500 {
            thread.apply_user(&format!("q{i}"), &seed::user_text(i));
            let id = format!("a{i}");
            thread.apply_assistant_start(&id, None);
            thread.apply_text_delta(&id, &seed::assistant_text(i).repeat(6));
            thread.apply_done(&id);
        }
        thread.apply_text_delta("a2499", marker);
        let (_, save_ms) = timed(|| store.save(&thread).unwrap());
        let bytes = store
            .dir()
            .join(format!("{}.json", thread.id))
            .metadata()
            .unwrap()
            .len();
        assert!(bytes > 8 * 1024 * 1024);
        let (loaded, reopen_ms) = timed(|| store.load(&thread.id).unwrap());
        assert_eq!(loaded.messages.len(), 5000);
        let (page, page_ms) =
            timed(|| MessageWindow::tail(&loaded.messages, THREAD_WINDOW_USER_TURNS));
        assert_eq!(page.messages.len(), 20);
        let (summaries, list_ms) = timed(|| store.list().unwrap());
        assert_eq!(summaries.len(), 100);
        let search = || {
            let hits = store
                .search_chats(marker, false, &CancelToken::new())
                .unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].message_id.as_deref(), Some("a2499"));
        };
        let (_, index_ms) = timed(search);
        let (_, search_ms) = timed(search);
        let diff = (0..100)
            .map(|i| {
                format!(
                    "diff --git a/f{i} b/f{i}\n--- a/f{i}\n+++ b/f{i}\n@@ -1 +1 @@\n-old\n+new\n"
                )
            })
            .collect::<String>();
        let (snapshot, diff_ms) = timed(|| {
            zest_core::workspace_changes::snapshot_from_unified_diff(
                "synthetic",
                black_box(diff.as_bytes()),
                None,
                None,
            )
        });
        assert_eq!(snapshot.changed_files.len(), 100);
        println!(
            "{}",
            serde_json::json!({
                "thread_bytes": bytes, "save_ms": save_ms, "reopen_ms": reopen_ms, "project_window_ms": page_ms,
                "list_100_chats_ms": list_ms, "build_index_and_find_ms": index_ms, "repeat_find_ms": search_ms,
                "diff_summary_100_files_ms": diff_ms
            })
        );
    }
}
