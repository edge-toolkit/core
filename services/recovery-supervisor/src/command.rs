//! Preserve bounded subprocess diagnostics when Kubernetes commands exceed their configured deadline.

use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::time::{Instant, timeout_at};
use tracing::{info, warn};

use crate::RecoveryError;

const STDERR_LIMIT: usize = 64 * 1024;

/// Drain stderr without blocking the child, retaining a bounded prefix even if this future is cancelled.
pub async fn capture_stderr<R>(reader: &mut R, bytes: &mut Vec<u8>) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 4096];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        let remaining = STDERR_LIMIT.saturating_sub(bytes.len());
        bytes.extend(chunk.iter().take(count.min(remaining)).copied());
    }
}

/// Collect kubectl output within one deadline, logging actual child state and partial stderr on timeout.
pub async fn output(command: &mut Command, timeout: Duration) -> Result<Output, RecoveryError> {
    let started = Instant::now();
    let deadline = started
        .checked_add(timeout)
        .ok_or_else(|| RecoveryError::Invalid("kubectl command deadline exceeds clock limits".into()))?;
    let (mut child, mut stdout, mut stderr) = spawn_piped(command, started)?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let result = timeout_at(deadline, async {
        tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut stdout_bytes),
            capture_stderr(&mut stderr, &mut stderr_bytes),
        )
    })
    .await;
    match result {
        Ok(outcome) => {
            let (status, _, ()) = outcome?;
            Ok(Output {
                status,
                stdout: stdout_bytes,
                stderr: stderr_bytes,
            })
        }
        Err(error) => Err(timeout_error(&mut child, started, &stderr_bytes, error)),
    }
}

/// Start the child with owned pipes and record startup timing before collecting its output.
#[expect(
    clippy::single_call_fn,
    reason = "process setup is distinct from deadline-bound output collection"
)]
fn spawn_piped(command: &mut Command, started: Instant) -> Result<(Child, ChildStdout, ChildStderr), RecoveryError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id();
    let spawn_seconds = started.elapsed().as_secs_f64();
    info!(?pid, spawn_seconds, "kubectl process spawned");
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RecoveryError::Invalid("kubectl stdout pipe is unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RecoveryError::Invalid("kubectl stderr pipe is unavailable".into()))?;
    Ok((child, stdout, stderr))
}

/// Preserve timeout semantics even if the operating system cannot report the child's current state.
#[expect(
    clippy::single_call_fn,
    reason = "timeout diagnostics are distinct from concurrent output collection"
)]
fn timeout_error(
    child: &mut Child,
    started: Instant,
    stderr: &[u8],
    error: tokio::time::error::Elapsed,
) -> RecoveryError {
    let pid = child.id();
    let state = child.try_wait();
    let elapsed_seconds = started.elapsed().as_secs_f64();
    let stderr_limit_reached = stderr.len() == STDERR_LIMIT;
    let stderr = String::from_utf8_lossy(stderr);
    warn!(
        ?pid,
        ?state,
        elapsed_seconds,
        stderr = %stderr,
        stderr_limit_reached,
        "kubectl deadline exceeded; dropping the child requests termination"
    );
    error.into()
}
