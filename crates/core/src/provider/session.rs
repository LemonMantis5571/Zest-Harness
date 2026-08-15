//! Shell-free JSONL process transport shared by CLI-owned providers.
//!
//! The transport deliberately knows nothing about Claude or Codex messages.
//! It owns only process lifetime, bounded stderr, line framing, timeout, and
//! cancellation. Provider adapters remain responsible for protocol semantics.

use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::cancel::{wait_cancel, CancelToken};
use crate::error::{HarnessError, Result};

const MAX_STDERR_BYTES: usize = 64 * 1024;

pub struct JsonlProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

impl JsonlProcess {
    pub async fn spawn_command(mut command: Command, label: &str) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| HarnessError::Other(format!("could not start `{label}`: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Other("provider stdin was not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Other("provider stdout was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HarnessError::Other("provider stderr was not piped".into()))?;
        let stderr = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut bytes = Vec::new();
            let _ = tokio::io::AsyncReadExt::take(&mut reader, MAX_STDERR_BYTES as u64)
                .read_to_end(&mut bytes)
                .await;
            bytes
        });

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr: Some(stderr),
        })
    }

    pub async fn send(&mut self, value: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|error| HarnessError::Other(format!("provider stdin failed: {error}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| HarnessError::Other(format!("provider stdin flush failed: {error}")))
    }

    pub async fn next(
        &mut self,
        timeout_duration: Duration,
        cancel: Option<&CancelToken>,
    ) -> Result<Option<Value>> {
        let Some(line) = self.next_line(timeout_duration, cancel).await? else {
            return Ok(None);
        };
        let value = serde_json::from_str(line.trim()).map_err(|error| HarnessError::Stream {
            kind: "invalid_jsonl".into(),
            message: error.to_string(),
        })?;
        Ok(Some(value))
    }

    pub async fn next_line(
        &mut self,
        timeout_duration: Duration,
        cancel: Option<&CancelToken>,
    ) -> Result<Option<String>> {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line);
        let result = tokio::select! {
            result = timeout(timeout_duration, read) => result
                .map_err(|_| HarnessError::StreamIdleTimeout)?,
            _ = wait_cancel(cancel) => return Err(HarnessError::Cancelled),
        };
        let count = result
            .map_err(|error| HarnessError::Other(format!("provider stdout failed: {error}")))?;
        if count == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim_end_matches(&['\r', '\n'][..]).to_string()))
    }

    pub async fn stderr_text(mut self) -> String {
        let bytes = match self.stderr.take() {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        };
        String::from_utf8_lossy(&bytes).trim().to_string()
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        self.child
            .wait()
            .await
            .map_err(|error| HarnessError::Other(format!("provider process failed: {error}")))
    }
}

impl Drop for JsonlProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        if let Some(stderr) = self.stderr.take() {
            stderr.abort();
        }
    }
}
