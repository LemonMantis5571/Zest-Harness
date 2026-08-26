//! Reading a child process's output without letting it decide how much memory
//! we use.
//!
//! Every consumer of a subprocess here already clips what it keeps — a build log
//! is truncated to both ends, a worker's stderr is truncated to a few hundred
//! characters. Reading the whole thing into a `Vec` first and clipping
//! afterwards makes that a display bound rather than a memory bound, which is
//! the same as having no bound at all: `yes`, a build loop, or a binary written
//! to stdout by accident will produce gigabytes, and the reason that was
//! survivable is only that nothing had done it yet.
//!
//! So the clipping happens while reading. Both ends are kept and the middle is
//! discarded, which is what the consumers wanted anyway — the useful parts of a
//! log are the first error and the final summary.

use tokio::io::{AsyncRead, AsyncReadExt};

/// One stream's output, already bounded.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Captured {
    /// The start and end of the stream, joined. Never larger than the ceiling.
    pub bytes: Vec<u8>,
    /// Bytes seen and discarded from the middle. Zero when nothing was lost.
    pub dropped: usize,
}

impl Captured {
    pub fn to_lossy_string(&self) -> String {
        String::from_utf8_lossy(&self.bytes).to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Read to EOF while holding at most `limit` bytes.
///
/// A read error ends the capture rather than failing it: whatever arrived
/// before the pipe broke is still the most informative thing available, and a
/// command that produced output and then died is exactly when you want it.
pub async fn drain_bounded<R>(reader: Option<&mut R>, limit: usize) -> Captured
where
    R: AsyncRead + Unpin,
{
    let Some(reader) = reader else {
        return Captured::default();
    };

    let head_cap = limit / 2;
    let tail_cap = limit - head_cap;
    let mut head: Vec<u8> = Vec::new();
    let mut tail: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
    let mut dropped = 0usize;
    let mut chunk = vec![0u8; 8 * 1024];

    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut rest = &chunk[..n];
                if head.len() < head_cap {
                    let take = rest.len().min(head_cap - head.len());
                    head.extend_from_slice(&rest[..take]);
                    rest = &rest[take..];
                }
                for byte in rest {
                    tail.push_back(*byte);
                    if tail.len() > tail_cap {
                        tail.pop_front();
                        dropped += 1;
                    }
                }
            }
        }
    }

    head.extend(tail);
    Captured {
        bytes: head,
        dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    async fn through_pipe(write: Vec<u8>, limit: usize) -> Captured {
        let (mut writer, mut reader) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let _ = writer.write_all(&write).await;
            let _ = writer.shutdown().await;
        });
        drain_bounded(Some(&mut reader), limit).await
    }

    #[tokio::test]
    async fn output_that_fits_is_kept_whole() {
        let captured = through_pipe(b"hello world".to_vec(), 1024).await;
        assert_eq!(captured.bytes, b"hello world");
        assert_eq!(captured.dropped, 0);
    }

    #[tokio::test]
    async fn a_flood_is_bounded_while_it_is_read() {
        // Ten times the ceiling goes in through a real pipe; the ceiling is
        // what is held, and nothing is unaccounted for.
        let limit = 4 * 1024;
        let captured = through_pipe(vec![b'x'; limit * 10], limit).await;
        assert_eq!(captured.bytes.len(), limit);
        assert_eq!(captured.bytes.len() + captured.dropped, limit * 10);
    }

    #[tokio::test]
    async fn both_ends_survive() {
        // The ends are the point: the first error and the final summary.
        let limit = 4 * 1024;
        let mut payload = b"FIRST".to_vec();
        payload.extend(vec![b'-'; limit * 2]);
        payload.extend(b"LAST");

        let captured = through_pipe(payload, limit).await;
        assert!(captured.bytes.starts_with(b"FIRST"));
        assert!(captured.bytes.ends_with(b"LAST"));
        assert!(captured.dropped > 0);
    }

    #[tokio::test]
    async fn an_odd_limit_still_splits_cleanly() {
        // head + tail must add up to the limit exactly, or the ceiling drifts.
        let captured = through_pipe(vec![b'z'; 1000], 7).await;
        assert_eq!(captured.bytes.len(), 7);
        assert_eq!(captured.dropped, 993);
    }

    #[tokio::test]
    async fn a_stream_that_was_never_piped_is_empty_not_an_error() {
        let captured = drain_bounded(Option::<&mut tokio::io::DuplexStream>::None, 1024).await;
        assert!(captured.is_empty());
        assert_eq!(captured.dropped, 0);
    }
}
