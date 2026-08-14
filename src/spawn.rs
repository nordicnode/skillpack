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

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Hard cap on the `--help` spawn shared by `introspect` and `verify::invocation`.
/// 30s covers Windows CI cold-cache compile cost for `go run .` (first-call
/// GOCACHE build + AV scan under parallel-test load) and any `node_modules`
/// resolution, while still bounding a hung CLI. History: raised 8s → 15s for
/// the same Windows `go run .` flake, then 15s → 30s when it recurred under
/// heavier parallel CI load. Ponytail: ceiling is CI cold-cache; if a real CLI
/// genuinely needs >30s to print `--help` the agent shouldn't invoke it
/// anyway, so this cap is also the fail-safe.
pub const HELP_TIMEOUT: Duration = Duration::from_secs(30);

/// Outcome of a guarded spawn — mirrors the old `SpawnOutcome` in
/// `introspect.rs`, shared so both call sites agree on semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnOutcome {
    /// Exited 0; stdout+stderr captured (concatenated).
    RanClean(String),
    /// Exited non-zero; stdout+stderr still captured (concatenated) so callers
    /// can inspect what a CLI printed even when it exits 1 (e.g. a `--version`
    /// line on stdout/stderr with a non-zero exit).
    RanNonZero(String),
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

/// How stdin should be configured for a spawn.
enum StdinMode {
    /// `/dev/null` — the default for all non-interactive spawns.
    Null,
    /// Piped, written then closed before polling so the child sees EOF.
    /// For interactive CLIs that block on stdin (otherwise they hang until
    /// timeout and false-flag drift).
    Piped(Vec<u8>),
}

/// Shared spawn-and-poll core used by both [`run`] and [`run_with_stdin`].
fn run_inner(cmd: &mut Command, timeout: Duration, stdin_mode: StdinMode) -> SpawnOutcome {
    let maybe_data: Option<Vec<u8>> = match stdin_mode {
        StdinMode::Null => {
            cmd.stdin(Stdio::null());
            None
        }
        StdinMode::Piped(d) => {
            cmd.stdin(Stdio::piped());
            Some(d)
        }
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    isolate_process_group(cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SpawnOutcome::NotFound,
        Err(e) => return SpawnOutcome::SpawnFailed(e.to_string()),
    };

    // Write stdin payload then drop the handle so the child sees EOF.
    if let Some(data) = &maybe_data {
        if let Some(mut child_stdin) = child.stdin.take() {
            let _ = child_stdin.write_all(data);
            drop(child_stdin);
        }
    }

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
            kill_tree(&mut child); // reap so drain threads hit EOF
            return SpawnOutcome::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    if !exited {
        kill_tree(&mut child);
        return SpawnOutcome::TimedOut;
    }

    // Collect whatever the readers got. After the child exited, the pipes hit
    // EOF → read_to_end returned → send fired. recv() can't block past that
    // (the only sender dropped when the thread finished), so no deadlock.
    let stdout_buf = stdout_rx.and_then(|rx| rx.recv().ok()).unwrap_or_default();
    let stderr_buf = stderr_rx.and_then(|rx| rx.recv().ok()).unwrap_or_default();

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&stdout_buf),
        String::from_utf8_lossy(&stderr_buf)
    );

    // try_wait consumed the exit status; wait() reaps cleanly (cached in std).
    // A reaped-but-failed child still surfaces its captured output so callers
    // can distinguish "printed version/help then exited 1" from a hard
    // failure (a wait() error also lands here, with whatever the readers got).
    match child.wait() {
        Ok(s) if s.success() => SpawnOutcome::RanClean(combined),
        _ => SpawnOutcome::RanNonZero(combined),
    }
}

/// Put the child in its own process group so a timeout can kill the whole
/// tree (the child plus any grandchildren it spawned, e.g. `go run .` forking
/// the compiled binary) instead of orphaning descendants.
#[cfg(unix)]
fn isolate_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn isolate_process_group(_cmd: &mut Command) {}

/// Kill the child and everything it spawned, then reap it.
///
/// Unix: the child leads its own process group (see `isolate_process_group`),
/// so `killpg` signals descendants too. Windows: `taskkill /T` walks the tree;
/// `Child::kill` alone would only terminate the direct child. The `unsafe`
/// block is the single audited exception to the crate's `unsafe_code = deny`;
/// `killpg` is safe in practice (an invalid pgid just returns -1, which we
/// ignore because the following `wait()` still reaps the direct child).
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id();
        #[allow(unsafe_code)]
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Spawn `cmd` (already configured by the caller) under a hard `timeout`.
///
/// The caller sets `current_dir`, args, and stdin via `Command`. This function
/// forces piped stdout/stderr (it needs the handles to drain them), polls the
/// child until it exits or the deadline fires, and kills on timeout.
/// stdin is `/dev/null` — use [`run_with_stdin`] for interactive CLIs.
pub fn run(cmd: &mut Command, timeout: Duration) -> SpawnOutcome {
    run_inner(cmd, timeout, StdinMode::Null)
}

/// Spawn `cmd` with an optional stdin payload — for interactive CLIs that
/// block on stdin (otherwise they hang until timeout and false-flag drift).
///
/// `None` → `Stdio::null()` (identical to [`run`]). `Some(bytes)` → piped,
/// written then closed before polling so the child sees EOF.
///
/// Only the `verify` path should use this; `introspect` and `git` spawns stay
/// on [`run`] (feeding stdin during flag extraction would corrupt the parse).
pub fn run_with_stdin(cmd: &mut Command, timeout: Duration, stdin: Option<&[u8]>) -> SpawnOutcome {
    match stdin {
        Some(data) => run_inner(cmd, timeout, StdinMode::Piped(data.to_vec())),
        None => run_inner(cmd, timeout, StdinMode::Null),
    }
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
        assert!(matches!(out, SpawnOutcome::RanNonZero(_)));
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

    /// `run_with_stdin` feeds bytes then closes stdin so the child sees EOF.
    /// `cat` echoes stdin to stdout — proves the write+close path works.
    #[cfg(unix)]
    #[test]
    fn run_with_stdin_feeds_bytes_and_closes() {
        let mut cmd = Command::new("cat");
        let out = run_with_stdin(&mut cmd, Duration::from_secs(5), Some(b"hello stdin"));
        let SpawnOutcome::RanClean(s) = out else {
            panic!("expected RanClean, got {out:?}");
        };
        assert!(
            s.contains("hello stdin"),
            "expected cat to echo fed stdin, got: {s}"
        );
    }

    /// `run_with_stdin(None)` must behave identically to `run` — null stdin.
    #[cfg(unix)]
    #[test]
    fn run_with_stdin_none_is_null_stdin() {
        let mut cmd = Command::new("echo");
        cmd.arg("no stdin needed");
        let out = run_with_stdin(&mut cmd, Duration::from_secs(5), None);
        let SpawnOutcome::RanClean(s) = out else {
            panic!("expected RanClean, got {out:?}");
        };
        assert!(s.contains("no stdin needed"));
    }
}
