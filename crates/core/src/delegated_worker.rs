//! Native provider workers for durable delegation jobs.
//!
//! External CLI/ACP workers stay in `tools::external_agent`. This module only
//! adapts configured Zest providers to the same isolated-worktree boundary and
//! returns bounded, coordinator-friendly results.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::agent::TurnUsageSummary;
use crate::cancel::CancelToken;
use crate::config::Config;
use crate::delegation::{
    AcceptanceCheckResult, AttemptUsage, CheckStatus, DelegationTarget, WorkerResult,
};
use crate::error::{HarnessError, Result};
use crate::provider::StreamEvent;
use crate::runtime::{resolve_provider_target, RuntimeBuilder, RuntimeRole};
use crate::tools::bash::{classify, Clearance};
use crate::tools::capture::drain_bounded;
use crate::tools::isolated_workspace::{prepare, prepare_reviewer};
use crate::usage::Ledger;

pub const MAX_NATIVE_RESULT_CHARS: usize = 16_000;
const MAX_CHECK_STREAM_BYTES: usize = 60 * 1024;
const MAX_CHECK_OUTPUT_CHARS: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct ResolvedWorkerMetadata {
    pub provider_id: String,
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone)]
pub struct NativeWorkerResult {
    pub metadata: ResolvedWorkerMetadata,
    pub result: WorkerResult,
    pub final_text: String,
    pub diff: String,
    pub usage: AttemptUsage,
}

#[derive(Debug, Clone)]
pub struct NativeReviewerResult {
    pub metadata: ResolvedWorkerMetadata,
    pub final_text: String,
    pub reviewer_diff: String,
    pub usage: AttemptUsage,
}

/// Run only explicitly safe acceptance checks in a fresh reviewer worktree.
///
/// Classification is performed before spawning anything. Commands that are
/// disabled, denied, or require approval are returned as `Skipped`; they are
/// never sent through the shell. Auto-run commands are tokenized and spawned
/// directly, with bounded output and cancellation/timeout cleanup.
pub async fn run_acceptance_checks(
    root: &Path,
    config: Config,
    worker_diff: &str,
    commands: &[String],
    cancel: Option<&CancelToken>,
) -> Result<Vec<AcceptanceCheckResult>> {
    let mut workspace = prepare_reviewer(root, worker_diff)
        .await
        .map_err(HarnessError::Other)?;
    let execution = run_checks_in_workspace(&mut workspace, &config, commands, cancel).await;
    let cleanup = workspace.cleanup().await;
    match (execution, cleanup) {
        (Ok(results), Ok(())) => Ok(results),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(HarnessError::Other(format!(
            "clean acceptance-check workspace: {error}"
        ))),
        (Err(error), Err(cleanup)) => Err(HarnessError::Other(format!(
            "{error}; clean acceptance-check workspace: {cleanup}"
        ))),
    }
}

async fn run_checks_in_workspace(
    workspace: &mut crate::tools::isolated_workspace::PreparedWorkspace,
    config: &Config,
    commands: &[String],
    cancel: Option<&CancelToken>,
) -> Result<Vec<AcceptanceCheckResult>> {
    let settings = config.tools.bash.settings();
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        if cancel.map(CancelToken::is_cancelled).unwrap_or(false) {
            return Err(HarnessError::Cancelled);
        }
        if !config.tools.bash.enabled {
            results.push(skipped_check(command, "command checks are disabled"));
            continue;
        }
        if classify(command, &settings.extra_allowlist, &settings.denylist) != Clearance::AutoRun {
            results.push(skipped_check(
                command,
                "command is not on the read-only auto-run allowlist",
            ));
            continue;
        }
        results.push(run_auto_check(workspace.path(), command, settings.timeout_ms, cancel).await?);
    }
    Ok(results)
}

fn skipped_check(command: &str, reason: &str) -> AcceptanceCheckResult {
    AcceptanceCheckResult {
        command: command.to_string(),
        status: CheckStatus::Skipped,
        output: reason.to_string(),
    }
}

async fn run_auto_check(
    root: &Path,
    command: &str,
    timeout_ms: u64,
    cancel: Option<&CancelToken>,
) -> Result<AcceptanceCheckResult> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return Ok(skipped_check(command, "empty command"));
    }
    let timeout = Duration::from_millis(timeout_ms.clamp(1, crate::tools::bash::MAX_TIMEOUT_MS));
    let mut child = tokio::process::Command::new(tokens[0]);
    child
        .args(&tokens[1..])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        child.creation_flags(0x0800_0000);
    }
    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(AcceptanceCheckResult {
                command: command.to_string(),
                status: CheckStatus::Failed,
                output: format!("could not run `{command}`: {error}"),
            })
        }
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let collect = async {
        let out = drain_bounded(stdout.as_mut(), MAX_CHECK_STREAM_BYTES);
        let err = drain_bounded(stderr.as_mut(), MAX_CHECK_STREAM_BYTES);
        let (out, err) = tokio::join!(out, err);
        let status = child.wait().await;
        (status, out, err)
    };
    let collected = tokio::select! {
        biased;
        _ = crate::cancel::wait_cancel(cancel) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(HarnessError::Cancelled);
        }
        result = tokio::time::timeout(timeout, collect) => result,
    };
    let (status, stdout, stderr) = match collected {
        Ok((status, stdout, stderr)) => (
            status
                .map_err(|error| HarnessError::Other(format!("wait for `{command}`: {error}")))?,
            stdout,
            stderr,
        ),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Ok(AcceptanceCheckResult {
                command: command.to_string(),
                status: CheckStatus::Failed,
                output: format!("`{command}` timed out after {}ms", timeout.as_millis()),
            });
        }
    };
    let output = bounded_check_output(command, status.code(), &stdout, &stderr);
    Ok(AcceptanceCheckResult {
        command: command.to_string(),
        status: if status.success() {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        output,
    })
}

fn bounded_check_output(
    command: &str,
    code: Option<i32>,
    stdout: &crate::tools::capture::Captured,
    stderr: &crate::tools::capture::Captured,
) -> String {
    let mut output = format!(
        "$ {command}\nexit {}",
        code.map_or_else(|| "unknown".into(), |c| c.to_string())
    );
    let out = String::from_utf8_lossy(&stdout.bytes);
    let err = String::from_utf8_lossy(&stderr.bytes);
    if !out.trim().is_empty() {
        output.push_str("\n\n");
        output.push_str(&out);
    }
    if !err.trim().is_empty() {
        output.push_str(if output.ends_with('\n') { "" } else { "\n" });
        output.push_str(&err);
    }
    if stdout.dropped + stderr.dropped > 0 {
        output.push_str(&format!(
            "\n[… {} bytes dropped from the middle …]",
            stdout.dropped + stderr.dropped
        ));
    }
    output.chars().take(MAX_CHECK_OUTPUT_CHARS).collect()
}

pub async fn run_provider_worker(
    root: &Path,
    config: Config,
    target: &DelegationTarget,
    prompt: &str,
    ledger: Option<Arc<Mutex<Ledger>>>,
    cancel: Option<&CancelToken>,
) -> Result<NativeWorkerResult> {
    let target = provider_target(root, &config, target)?;
    let mut workspace = prepare(root).await.map_err(HarnessError::Other)?;
    let result = run_in_workspace(
        workspace.path(),
        config,
        &target,
        prompt,
        RuntimeRole::DelegationWorker,
        ledger,
        cancel,
    )
    .await;
    let diff = match result {
        Ok((final_text, usage)) => workspace
            .collect_diff()
            .await
            .map_err(HarnessError::Other)
            .map(|diff| {
                let result =
                    WorkerResult::from_external(&final_text, &diff).unwrap_or(WorkerResult {
                        summary: clip(&final_text),
                        changed_files: crate::delegation::diff_paths(&diff),
                        checks_attempted: Vec::new(),
                        blockers: Vec::new(),
                    });
                NativeWorkerResult {
                    metadata: target.metadata(),
                    result,
                    final_text: clip(&final_text),
                    diff,
                    usage,
                }
            }),
        Err(error) => Err(error),
    };
    let cleanup = workspace.cleanup().await;
    match (diff, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(HarnessError::Other(format!(
            "clean native worker workspace: {error}"
        ))),
        (Err(error), Err(cleanup)) => Err(HarnessError::Other(format!(
            "{error}; clean native worker workspace: {cleanup}"
        ))),
    }
}

pub async fn run_provider_reviewer(
    root: &Path,
    config: Config,
    target: &DelegationTarget,
    worker_diff: &str,
    prompt: &str,
    ledger: Option<Arc<Mutex<Ledger>>>,
    cancel: Option<&CancelToken>,
) -> Result<NativeReviewerResult> {
    let target = provider_target(root, &config, target)?;
    let mut workspace = prepare_reviewer(root, worker_diff)
        .await
        .map_err(HarnessError::Other)?;
    let result = run_in_workspace(
        workspace.path(),
        config,
        &target,
        prompt,
        RuntimeRole::DelegationReviewer,
        ledger,
        cancel,
    )
    .await;
    let output = match result {
        Ok((final_text, usage)) => workspace
            .collect_diff()
            .await
            .map_err(HarnessError::Other)
            .map(|reviewer_diff| NativeReviewerResult {
                metadata: target.metadata(),
                final_text: clip(&final_text),
                reviewer_diff,
                usage,
            }),
        Err(error) => Err(error),
    };
    let cleanup = workspace.cleanup().await;
    match (output, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(HarnessError::Other(format!(
            "clean native reviewer workspace: {error}"
        ))),
        (Err(error), Err(cleanup)) => Err(HarnessError::Other(format!(
            "{error}; clean native reviewer workspace: {cleanup}"
        ))),
    }
}

fn provider_target(
    root: &Path,
    config: &Config,
    target: &DelegationTarget,
) -> Result<ResolvedWorkerMetadata> {
    let DelegationTarget::Provider {
        provider_id,
        model,
        effort,
    } = target
    else {
        return Err(HarnessError::Other(
            "native provider runner requires a provider delegation target".into(),
        ));
    };
    let (registry, skipped) =
        crate::provider::registry::ProviderRegistry::from_config_at(config, root);
    let resolved = resolve_provider_target(
        &registry,
        &skipped,
        provider_id,
        model.as_deref(),
        effort.as_deref(),
    )?;
    Ok(ResolvedWorkerMetadata {
        provider_id: resolved.provider_id,
        model: resolved.model,
        effort: resolved.effort,
    })
}

async fn run_in_workspace(
    root: &Path,
    config: Config,
    target: &ResolvedWorkerMetadata,
    prompt: &str,
    role: RuntimeRole,
    ledger: Option<Arc<Mutex<Ledger>>>,
    cancel: Option<&CancelToken>,
) -> Result<(String, AttemptUsage)> {
    let mut runtime = RuntimeBuilder::new(root)
        .with_config(config)
        .with_provider(target.provider_id.clone())
        .with_model(target.model.clone())
        .with_effort(target.effort.clone())
        .with_role(role)
        .enable_external_agents(false)
        .register_exec_tools(false);
    if let Some(ledger) = ledger {
        runtime = runtime.with_ledger(ledger);
    }
    let mut session = runtime.build()?;
    let mut sink = discard_event;
    session
        .agent
        .send_cancellable(prompt, &mut sink, cancel)
        .await?;
    let final_text = final_text(&session.agent);
    let usage = attempt_usage(session.agent.turn_usage().as_ref());
    Ok((final_text, usage))
}

fn discard_event(_event: StreamEvent<'_>) {}

fn final_text(agent: &crate::agent::Agent) -> String {
    agent
        .messages
        .last()
        .and_then(|message| {
            message.content.iter().rev().find_map(|block| {
                (block.get("type").and_then(|value| value.as_str()) == Some("text"))
                    .then(|| block.get("text").and_then(|value| value.as_str()))
                    .flatten()
            })
        })
        .map(str::to_string)
        .unwrap_or_default()
}

fn attempt_usage(summary: Option<&TurnUsageSummary>) -> AttemptUsage {
    let Some(summary) = summary.filter(|summary| summary.usage_available) else {
        return AttemptUsage::default();
    };
    AttemptUsage {
        input_tokens: Some(u64::from(summary.usage.input_tokens)),
        output_tokens: Some(u64::from(summary.usage.output_tokens)),
        cache_read_tokens: Some(u64::from(summary.usage.cache_read_input_tokens)),
        cache_write_tokens: Some(u64::from(summary.usage.cache_creation_input_tokens)),
    }
}

fn clip(value: &str) -> String {
    value.chars().take(MAX_NATIVE_RESULT_CHARS).collect()
}

impl ResolvedWorkerMetadata {
    fn metadata(&self) -> ResolvedWorkerMetadata {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn native_result_is_bounded() {
        assert_eq!(
            clip(&"x".repeat(MAX_NATIVE_RESULT_CHARS + 20)).len(),
            MAX_NATIVE_RESULT_CHARS
        );
    }

    #[test]
    fn native_runner_rejects_external_target() {
        let root = std::env::temp_dir();
        let config = Config::default();
        let error = provider_target(
            &root,
            &config,
            &DelegationTarget::ExternalAgent {
                agent_id: "claude".into(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("provider delegation target"));
    }

    #[tokio::test]
    async fn acceptance_checks_report_pass_fail_skip_and_clean_worktree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let mut config = Config::find(root).unwrap();
        config.tools.bash.extra_allowlist = vec![vec!["rustc".into()]];
        let before = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout;
        let results = run_acceptance_checks(
            root,
            config,
            "",
            &[
                "rustc --version".into(),
                "rustc --definitely-not-a-real-flag".into(),
                "cargo fmt".into(),
            ],
            None,
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].status, CheckStatus::Passed);
        assert_eq!(results[1].status, CheckStatus::Failed);
        assert_eq!(results[2].status, CheckStatus::Skipped);
        assert!(results[2].output.contains("allowlist"));

        let after = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(root)
            .output()
            .unwrap()
            .stdout;
        assert_eq!(before, after, "acceptance checks must clean their worktree");
    }
}
