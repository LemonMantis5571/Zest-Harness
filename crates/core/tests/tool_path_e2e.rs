//! Manual end-to-end smoke test for the model-facing execution path.
//!
//! This is intentionally ignored in the normal suite because it starts nested
//! Cargo processes. Run it explicitly with:
//!
//!     cargo test -p zest-core --test tool_path_e2e -- --ignored --nocapture

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use zest_core::tools::bash::BashSettings;
use zest_core::{
    register_exec_tools_with_jobs, JobRead, JobRegistry, JobSnapshot, JobStatus, ToolRegistry,
};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("zest-core must remain inside the workspace")
}

fn server_id(body: &str) -> String {
    body.lines()
        .find_map(|line| line.strip_prefix("server_id:"))
        .map(str::trim)
        .and_then(|value| value.split_whitespace().next())
        .map(str::to_string)
        .expect("bash background result should include server_id")
}

#[tokio::test]
#[ignore = "starts real nested Cargo processes through the Zest tools"]
async fn actual_bash_and_job_tools_run_and_cancel_real_work() {
    let root = workspace_root();
    let jobs = Arc::new(JobRegistry::new());
    let owner = Some("tool-path-e2e".to_string());
    let mut tools = ToolRegistry::new();
    register_exec_tools_with_jobs(
        &mut tools,
        root,
        BashSettings::default(),
        jobs.clone(),
        owner.clone(),
    )
    .expect("register the actual bash and job tools");

    let foreground = tools
        .run(
            "bash",
            json!({
                "command": "cargo test -p zest-core --lib jobs::tests --quiet",
                "cwd": ".",
                "timeout_ms": 120_000
            }),
        )
        .await
        .expect("actual bash foreground test should succeed");
    assert!(
        foreground.body.contains("test result: ok"),
        "foreground bash output did not contain a passing test result:\n{}",
        foreground.body
    );

    let background = tools
        .run(
            "bash",
            json!({
                "command": "cargo test -p zest-core --lib tools::jobs::tests --quiet",
                "cwd": ".",
                "background": true
            }),
        )
        .await
        .expect("actual bash background test should start");
    let background_id = server_id(&background.body);

    let listed = tools
        .run("job_list", json!({}))
        .await
        .expect("job_list should see the bash-owned job");
    assert!(listed.body.contains(&background_id), "{listed:?}");

    let mut offset = 0;
    let mut output_text = String::new();
    let mut completed_read = None;
    for _ in 0..60 {
        let output = tools
            .run(
                "job_output",
                json!({
                    "job_id": background_id,
                    "offset": offset,
                    "wait": true,
                    "timeout_ms": 30_000
                }),
            )
            .await
            .expect("job_output should wait for the background Cargo test");
        let read: JobRead = serde_json::from_str(&output.body)
            .expect("job_output should return its documented JSON shape");
        output_text.push_str(&read.text);
        offset = read.next_offset;
        if read.snapshot.status.terminal() {
            completed_read = Some(read);
            break;
        }
    }
    let completed_read =
        completed_read.expect("background Cargo test should reach a terminal state");
    assert_eq!(completed_read.snapshot.status, JobStatus::Completed);
    assert!(
        output_text.contains("test result: ok"),
        "background job output did not contain a passing test result:\n{}",
        output_text
    );

    let second_output = tools
        .run(
            "job_output",
            json!({
                "job_id": background_id,
                "offset": completed_read.next_offset
            }),
        )
        .await
        .expect("job_output should accept its cursor");
    let second_read: JobRead = serde_json::from_str(&second_output.body)
        .expect("cursor read should return its documented JSON shape");
    assert!(
        second_read.text.is_empty(),
        "output was reread: {second_read:?}"
    );

    let mut other_owner = ToolRegistry::new();
    zest_core::register_job_tools(
        &mut other_owner,
        jobs.clone(),
        Some("different-thread".to_string()),
    );
    assert!(
        other_owner
            .run("job_output", json!({ "job_id": background_id }))
            .await
            .is_err(),
        "job output crossed the owner fence"
    );

    let long_running = if cfg!(windows) {
        "ping 127.0.0.1 -n 30"
    } else {
        "sleep 30"
    };
    let running = tools
        .run(
            "bash",
            json!({
                "command": long_running,
                "cwd": ".",
                "background": true
            }),
        )
        .await
        .expect("actual bash should start the cancellable process");
    let running_id = server_id(&running.body);

    let killed = tools
        .run(
            "job_kill",
            json!({
                "job_id": running_id,
                "reason": "tool-path smoke-test cancellation"
            }),
        )
        .await
        .expect("job_kill should stop the real process");
    let killed: JobSnapshot =
        serde_json::from_str(&killed.body).expect("job_kill should return job state JSON");
    assert_eq!(killed.status, JobStatus::Killed, "{killed:?}");
}
