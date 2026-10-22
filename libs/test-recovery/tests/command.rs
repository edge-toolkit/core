//! Exercise recovery command output, bounded diagnostics and deadlines using a Cargo-built subprocess fixture.
#![cfg(test)]

use std::io;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use clap::Parser as _;
use et_recovery_supervisor::RecoveryError;
use et_recovery_supervisor::command::{capture_stderr, output};
use et_test_recovery::command::Arguments;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;
use tracing::subscriber::{NoSubscriber, set_default};
use tracing_subscriber::fmt::MakeWriter;

/// Capture diagnostic fields without using a global subscriber shared by parallel tests.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl io::Write for Logs {
    /// Append each formatted event atomically to the fixture's captured log.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Ok(mut bytes) = self.0.lock() else {
            return Err(io::Error::other("diagnostic log mutex poisoned"));
        };
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Memory-backed capture has no pending writes to flush.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for Logs {
    type Writer = Self;

    /// Share the fixture's buffer across event-formatting calls.
    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

/// Launch the Cargo-built fixture; an empty mode omits the argument to exercise missing-input rejection.
fn probe(mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_recovery-command-probe"));
    if !mode.is_empty() {
        let _command = command.arg(mode);
    }
    command
}

/// Verify long-wait mode selection without waiting for its fallback termination delay.
#[test]
fn fixture_modes_select_finite_completion_delays() {
    for (mode, delay) in [
        ("complete", Duration::ZERO),
        ("pause", Duration::from_millis(25)),
        ("hang", Duration::from_secs(120)),
    ] {
        let arguments = Arguments::try_parse_from(["recovery-command-probe", mode]).unwrap();
        assert_eq!(arguments.delay(), delay, "mode={mode}");
    }
}

/// Collect both streams on a normal process exit.
#[tokio::test]
async fn successful_commands_preserve_stdout_and_stderr() {
    let _logging = set_default(NoSubscriber::default());
    for mode in ["complete", "pause"] {
        let result = output(&mut probe(mode), Duration::from_secs(60)).await.unwrap();
        assert!(result.status.success(), "mode={mode}");
        assert_eq!(result.stdout, b"fixture stdout\n");
        assert_eq!(result.stderr, b"fixture stderr\n");
    }
}

/// Reject a mistyped fixture mode rather than silently running the wrong subprocess behaviour.
#[tokio::test]
async fn probe_rejects_missing_or_unknown_modes() {
    for (mode, expected) in [("", "required arguments"), ("invalid", "invalid value")] {
        let result = output(&mut probe(mode), Duration::from_secs(60)).await.unwrap();
        assert_eq!(result.status.code(), Some(2_i32), "mode={mode}");
        assert_eq!(result.stdout, b"");
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(error.contains(expected), "mode={mode}: {error}");
        assert!(error.contains("--help"), "mode={mode}: {error}");
    }
}

/// Reject trailing arguments and unknown flags before emitting any fixture output.
#[tokio::test]
async fn probe_rejects_unexpected_arguments() {
    for argument in ["unexpected", "--unknown"] {
        let mut command = probe("complete");
        let _command = command.arg(argument);
        let result = output(&mut command, Duration::from_secs(60)).await.unwrap();
        assert_eq!(result.status.code(), Some(2_i32), "argument={argument}");
        assert_eq!(result.stdout, b"");
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(error.contains("unexpected argument"), "{error}");
        assert!(error.contains(argument), "{error}");
    }
}

/// Describe every accepted mode without running the subprocess behaviour.
#[tokio::test]
async fn probe_help_lists_modes_without_emitting_diagnostics() {
    let result = output(&mut probe("--help"), Duration::from_secs(60)).await.unwrap();
    assert!(result.status.success());
    assert_eq!(result.stderr, b"");
    let help = String::from_utf8(result.stdout).unwrap();
    for mode in ["complete", "pause", "hang"] {
        assert!(help.contains(mode), "missing mode {mode}: {help}");
    }
    assert!(!help.contains("fixture stdout"));
}

/// Report the actual PID, spawn timing, process state and stderr field when a child exceeds its deadline.
#[tokio::test]
async fn timeout_retains_its_error_type_and_logs_child_diagnostics() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _logging = set_default(subscriber);
    let error = output(&mut probe("hang"), Duration::from_secs(1)).await.unwrap_err();
    assert!(matches!(error, RecoveryError::CommandTimeout(_)));
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for expected in [
        "kubectl process spawned",
        "pid=",
        "state=",
        "stderr=",
        "stderr_limit_reached=false",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    for field in ["spawn_seconds", "elapsed_seconds"] {
        let prefix = format!("{field}=");
        let value = text
            .split_whitespace()
            .find_map(|word| word.strip_prefix(&prefix))
            .unwrap()
            .parse::<f64>()
            .unwrap();
        assert!(value.is_finite() && value >= 0.0_f64, "invalid {field}: {text}");
    }
}

/// Disabling diagnostics must not change the deadline error or prevent termination of the child.
#[tokio::test]
async fn timeout_remains_an_error_without_a_logging_subscriber() {
    let _logging = set_default(NoSubscriber::default());
    let error = output(&mut probe("hang"), Duration::from_secs(1)).await.unwrap_err();
    assert!(matches!(error, RecoveryError::CommandTimeout(_)));
}

/// Retain bytes already received even when an open pipe prevents the read future from completing.
#[tokio::test]
async fn cancelled_stderr_capture_keeps_the_partial_prefix() {
    let (mut reader, mut writer) = tokio::io::duplex(128);
    writer.write_all(b"partial diagnostic").await.unwrap();
    let mut bytes = Vec::new();
    {
        let read = capture_stderr(&mut reader, &mut bytes);
        tokio::pin!(read);
        assert!(matches!(futures_util::poll!(read.as_mut()), Poll::Pending));
    }
    assert_eq!(bytes, b"partial diagnostic");
}

/// Drain oversized stderr to EOF while retaining only the configured diagnostic prefix.
#[tokio::test]
async fn stderr_capture_is_bounded_without_blocking_the_writer() {
    let (mut reader, mut writer) = tokio::io::duplex(128);
    let mut bytes = Vec::new();
    let (read, ()) = tokio::join!(capture_stderr(&mut reader, &mut bytes), async move {
        writer.write_all(&vec![b'x'; 70 * 1024]).await.unwrap();
    });
    read.unwrap();
    assert_eq!(bytes.len(), 64 * 1024);
    assert!(bytes.iter().all(|byte| *byte == b'x'));
}

/// Reject clock-overflowing deadlines before attempting to spawn an executable.
#[tokio::test]
async fn unrepresentable_deadlines_fail_before_process_creation() {
    let mut command = Command::new("et-intentionally-absent-kubectl-probe");
    let error = output(&mut command, Duration::from_secs(u64::MAX)).await.unwrap_err();
    assert!(matches!(error, RecoveryError::Invalid(_)));
    assert!(error.to_string().contains("clock limits"));
}
