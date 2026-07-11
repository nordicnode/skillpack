//! A guarded, time-boxed subprocess spawn shared by `introspect` and
//! `verify::invocation`.
//!
//! The previous design polled `Child::try_wait` in a busy loop and called
//! `wait_with_output` only *after* the child exited. With piped stdout/stderr
//! that deadlocks once the child writes >64KB (the pipe buffer): the child
//! blocks on the write, `try_wait` keeps returning `Ok(None)`, the deadline
//! fires → a false `TimedOut`. The fix: drain the pipes on reader threads
//! while polling, so the child never blocks on a full buffer. On timeout we
//! `kill()` and `wait()` (reap) so the drain threads complete promptly; the
//! outcome still maps to `TimedOut` upstream, just for real now.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Hard cap on the `--help` spawn shared by `introspect` and `verify::invocation`.
/// 15s is generous: covers Windows CI cold-cache compile cost for `go run .`
/// (first-call GOCACHE build + AV scan) and any `node_modules` resolution,
/// while still bounding a hung CLI. Ponytail: ceiling is CI cold-cache; if a
/// real CLI genuinely needs >15s to print `--help` the agent shouldn't invoke
/// it anyway, so this cap is also the fail-safe.
pub const HELP_TIMEOUT: Duration = Duration::from_secs(15);

/// Outcome of a guarded spawn — mirrors the old `SpawnOutcome` in
/// `introspect.rs`, shared so both call sites agree on semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnOutcome {
    /// Exited 0; stdout+stderr captured (concatenated).
    RanClean(String),
    /// Exited non-zero; output not consumed (callers only act on `RanClean`).
    RanNonZero,
    /// Did not finish within the timeout (killed).
    TimedOut,
    /// Binary not found on PATH.
    NotFound,
    /// Other spawn error (permission denied, etc.). Carries the io::Error
    /// display so the caller can surface it (verify wants this for an
    /// actionable "could not spawn" message, distinct from "not found").
    SpawnFailed(String),
}

/// Drain a pipe handle to a `Vec<u8>` on a thread. Returns a receiver that
/// yields the buffer when the read completes (EOF on the pipe, which happens
/// when the child exits or is killed).
fn pipe_drain(r: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = r;
        let _ = reader.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// Spawn `cmd` (already configured by the caller) under a hard `timeout`.
///
/// The caller sets `current_dir`, args, and stdin via `Command`. This function
/// forces piped stdout/stderr (it needs the handles to drain them), polls the
/// child until it exits or the deadline fires, and kills on timeout.
pub fn run(cmd: &mut Command, timeout: Duration) -> SpawnOutcome {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SpawnOutcome::NotFound,
        Err(e) => return SpawnOutcome::SpawnFailed(e.to_string()),
    };

    // Move the piped handles to reader threads so they drain continuously
    // while we poll. Without this the child blocks on a >64KB write.
    let stdout_rx = child.stdout.take().map(pipe_drain);
    let stderr_rx = child.stderr.take().map(pipe_drain);

    // Poll until exit or deadline. The drain threads keep the pipe buffers
    // empty so the child's writes never block — fixing the >64KB deadlock.
    let deadline = Instant::now() + timeout;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) => {}
            Err(_) => break false,
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait(); // reap so drain threads hit EOF
            return SpawnOutcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        return SpawnOutcome::TimedOut;
    }

    // Collect whatever the readers got. After the child exited, the pipes hit
    // EOF → read_to_end returned → send fired. recv() can't block past that
    // (the only sender dropped when the thread finished), so no deadlock.
    let stdout_buf = stdout_rx.and_then(|rx| rx.recv().ok()).unwrap_or_default();
    let stderr_buf = stderr_rx.and_then(|rx| rx.recv().ok()).unwrap_or_default();

    // try_wait consumed the exit status; wait() reaps cleanly (cached in std).
    let status = match child.wait() {
        Ok(s) => s,
        Err(_) => return SpawnOutcome::RanNonZero,
    };

    if !status.success() {
        return SpawnOutcome::RanNonZero;
    }

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&stdout_buf),
        String::from_utf8_lossy(&stderr_buf)
    );
    SpawnOutcome::RanClean(combined)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn small_help_captures_cleanly() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello world");
        let out = run(&mut cmd, Duration::from_secs(5));
        let SpawnOutcome::RanClean(s) = out else {
            panic!("expected RanClean, got {out:?}");
        };
        assert!(s.contains("hello world"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_binary_is_not_found() {
        let mut cmd = Command::new("/this/does/not/exist/xyz");
        cmd.arg("--help");
        assert_eq!(
            run(&mut cmd, Duration::from_secs(2)),
            SpawnOutcome::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_exit_is_ran_nonzero() {
        // `false` exits 1; we shouldn't crash, just report non-zero.
        let mut cmd = Command::new("false");
        let out = run(&mut cmd, Duration::from_secs(5));
        assert_eq!(out, SpawnOutcome::RanNonZero);
    }

    /// Regression: a CLI that writes MORE than the 64KB pipe buffer must not
    /// deadlock (the old poll-without-draining loop would false-fail this).
    /// `yes hello | head -n 20000` writes ~120KB to stdout; assert it drains
    /// and captures.
    #[cfg(unix)]
    #[test]
    fn writes_beyond_pipe_buffer_do_not_deadlock() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes hello | head -n 20000"]);
        let out = run(&mut cmd, Duration::from_secs(10));
        let SpawnOutcome::RanClean(s) = out else {
            panic!("expected RanClean, got {out:?}");
        };
        // ~20000 lines * 6 bytes = 120KB; check a chunk made it through.
        assert!(
            s.contains("hello"),
            "expected capture beyond 64KB pipe buffer"
        );
    }
}
