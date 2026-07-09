//! Invocation checks — actually run the documented CLI and catch drift between
//! what `SKILL.md` advertises and what `--help` actually offers.
//!
//! Design §5.2 step 3 + §6.3. The spawn is the same guarded, time-boxed spawn
//! as introspect (hard timeout, run in the project root). For pure-library
//! projects this entire suite is a no-op returning `Skipped` per §5.1's
//! "Pure-library path" — critical checks still run, no subprocess is spawned.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;

use super::result::{CheckResult, VerifyReport};

const HELP_TIMEOUT: Duration = Duration::from_secs(8);

/// Inputs the invocation checker needs. Kept as a plain struct so the caller
/// (`verify` dispatcher) reads the SKILL.md + knows the `has_cli` flag, while
/// this module stays focused on spawning + diffing.
///
/// `skill_root` and `spawn_cwd` are deliberately separate (design §5.3 + §6.3):
/// - `skill_root` is where the SKILL.md / marketplace / plugin files live. For
///   the `verify` subcommand that's the project root; for `init`'s pre-commit
///   gate it's the temp dir the rendered files were written to (we verify the
///   ACTUAL files we're about to ship).
/// - `spawn_cwd` is the working directory the documented CLI runs in — always
///   the real project root, where the source tree / built artifact lives. The
///   pre-commit gate would false-fail a real Go (`go run .`) or Node
///   (`node ./bin/cli.js`) CLI if it spawned from the skill-only temp dir.
#[derive(Debug, Clone)]
pub struct InvocationInput {
    /// The raw SKILL.md text, so we can extract documented flags/invocations.
    pub skill_md: String,
    /// True iff introspect/the user confirmed a CLI exists. When false, every
    /// invocation check returns `Skipped` (design §5.1 pure-library path).
    pub has_cli: bool,
    /// The command to run with `--help`, argv-style: `["chronicle", "--help"]`.
    /// `None` when `has_cli` is false.
    pub cli_command: Option<Vec<String>>,
    /// Where the SKILL.md / manifest files live (project root, or the temp
    /// dir for `init`'s pre-commit gate).
    pub skill_root: std::path::PathBuf,
    /// Working directory for the CLI spawn — always the real project root.
    pub spawn_cwd: std::path::PathBuf,
}

impl InvocationInput {
    /// `skill_root` is where the skill files live; `spawn_cwd` is where the
    /// documented CLI actually runs (the project root).
    pub fn new(
        skill_root: &Path,
        spawn_cwd: &Path,
        skill_md: &str,
        has_cli: bool,
        cli_command: Option<&[String]>,
    ) -> Self {
        Self {
            skill_md: skill_md.to_string(),
            has_cli,
            cli_command: cli_command.map(<[std::string::String]>::to_vec),
            skill_root: skill_root.to_path_buf(),
            spawn_cwd: spawn_cwd.to_path_buf(),
        }
    }
}

/// Run every invocation check, appending to `report`.
pub fn run(input: &InvocationInput, report: &mut VerifyReport) -> Result<()> {
    // Pure-library path: no subprocess, no drift to check.
    if !input.has_cli {
        report.push(CheckResult::skipped(
            "invocation",
            "CLI invocation drift checks",
            "Skipped: pure-library project (no CLI to invoke)",
        ));
        return Ok(());
    }

    let Some(cmd) = input.cli_command.as_ref() else {
        report.push(CheckResult::skipped(
            "invocation",
            "CLI invocation drift checks",
            "Skipped: has_cli set but no command recorded",
        ));
        return Ok(());
    };
    if cmd.is_empty() {
        report.push(CheckResult::skipped(
            "invocation",
            "CLI invocation drift checks",
            "Skipped: empty command vector",
        ));
        return Ok(());
    }

    let help = run_help(cmd, &input.spawn_cwd, report)?;
    if report.has_critical_failure() {
        return Ok(());
    }

    // Flag-drift: every `--flag` mentioned in SKILL.md prose must exist in --help.
    check_flag_drift(&help, &input.skill_md, report);

    Ok(())
}

/// Spawn `<cmd[0]> [cmd[1..]]` (e.g. `chronicle --help`) under a hard timeout,
/// push the outcome as a check, and return the captured stdout+stderr on
/// success.
fn run_help(cmd: &[String], root: &Path, report: &mut VerifyReport) -> Result<String> {
    let program = &cmd[0];
    let mut c = Command::new(program);
    for arg in &cmd[1..] {
        c.arg(arg);
    }
    c.current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            report.push(CheckResult::fail(
                "invocation.help_present",
                "documented CLI is installed and runnable",
                format!("CLI binary `{program}` not found on PATH"),
                format!(
                    "To fix: build/install `{program}` so it's on PATH, then re-run `skillpack verify`."
                ),
            ));
            return Ok(String::new());
        }
        Err(e) => {
            report.push(CheckResult::fail(
                "invocation.help_present",
                "documented CLI is installed and runnable",
                format!("could not spawn `{program}`: {e}"),
                "To fix: check that the binary path in skillpack.toml is correct.",
            ));
            return Ok(String::new());
        }
    };

    let deadline = Instant::now() + HELP_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            report.push(CheckResult::fail(
                "invocation.help_present",
                "CLI prints `--help` quickly",
                format!("`{program} --help` exceeded {}s timeout", HELP_TIMEOUT.as_secs()),
                "To fix: the CLI may hang waiting on input; guard it with `</dev/null` or fix the hang before shipping.",
            ));
            return Ok(String::new());
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let output = child
        .wait_with_output()
        .map(|o| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        })
        .unwrap_or_default();

    let status = if let Some(s) = status {
        s
    } else {
        // We already pushed the timeout failure inside the loop, but guard
        // against the Err-broken case too.
        if !report.has_critical_failure() {
            report.push(CheckResult::fail(
                "invocation.help_present",
                "documented `--help` exits cleanly",
                format!("`{program}` could not be waited on (killed or errored)"),
                "To fix: ensure the CLI exits promptly when given `--help`.",
            ));
        }
        return Ok(output);
    };

    if !status.success() {
        report.push(CheckResult::fail(
            "invocation.help_present",
            "documented `--help` exits cleanly",
            format!("`{program}` returned non-zero (exit {status})"),
            "To fix: make `--help` exit 0, or correct the command in skillpack.toml.",
        ));
        return Ok(output);
    }

    if output.trim().is_empty() {
        report.push(CheckResult::fail(
            "invocation.help_present",
            "documented `--help` produces output",
            format!("`{program} --help` printed nothing"),
            "To fix: implement/generate `--help` output so an agent knows the available flags.",
        ));
        return Ok(output);
    }

    report.push(CheckResult::pass(
        "invocation.help_present",
        "documented `--help` runs and produces output",
        format!("`{program}` printed {} bytes of help", output.len()),
    ));
    Ok(output)
}

/// Compare documented flags in SKILL.md against those advertised in `--help`,
/// flagging any documented flag that does not actually exist (drift).
fn check_flag_drift(help_output: &str, skill_md: &str, report: &mut VerifyReport) {
    let help_flags = extract_flags(help_output);
    // Strip meta-flags from the documented set: SKILL.md always says things
    // like "Run `<cli> --help`" or "use `--version`", and a tool's own --help
    // output does not list `--help`/`-h`/`--version` as passable flags — so
    // treating them as drift would cause false positives.
    let doc_flags = extract_flags(skill_md)
        .into_iter()
        .filter(|f| !is_meta_flag(f))
        .collect::<Vec<_>>();

    if doc_flags.is_empty() {
        report.push(CheckResult::warn(
            "invocation.flag_drift",
            "SKILL.md documents flags that match `--help`",
            "no flags appear to be documented in SKILL.md (no `--flag` tokens found)",
            "To fix: document the CLI's flags so an agent knows what to pass.",
        ));
        return;
    }

    let mut drifted: Vec<String> = doc_flags
        .iter()
        .filter(|f| !help_flags.contains(*f))
        .cloned()
        .collect();
    drifted.sort();
    drifted.dedup();

    if drifted.is_empty() {
        report.push(CheckResult::pass(
            "invocation.flag_drift",
            "every documented flag exists in `--help`",
            format!(
                "all {} documented flag(s) present in --help",
                doc_flags.len()
            ),
        ));
        return;
    }

    // Find the first documented drift line so the suggestion is actionable.
    let first = &drifted[0];
    let line_hint = skill_md
        .lines()
        .position(|l| l.contains(first.as_str()))
        .map(|n| n + 1);

    let mut fail = CheckResult::fail(
        "invocation.flag_drift",
        "every documented flag exists in `--help`",
        format!(
            "documented flag(s) missing from `--help`: {}",
            drifted.join(", ")
        ),
        format!("To fix: remove `{first}` from SKILL.md, or add `{first}` to your CLI's `--help`."),
    );
    fail.location = Some(("SKILL.md".to_string(), line_hint));
    report.push(fail);
}

/// True for the universal help/version meta-flags that every CLI implicitly
/// supports but does not (and should not) list among its own passable flags.
/// These are excluded from flag-drift comparison so a SKILL.md instruction like
/// "Run `<cli> --help`" doesn't read as drift.
fn is_meta_flag(flag: &str) -> bool {
    matches!(flag, "--help" | "-h" | "--version" | "-V" | "--help-all")
}

/// Extract the set of `--double-dash` and `-single-dash` flags from a blob.
/// Only flags (a token whose first non-whitespace char is `-` followed by a
/// letter) count; `--` alone and bare `-` do not. Short flags require a letter
/// so we don't sweep up hyphenated prose like `two-step`.
pub fn extract_flags(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in text.split_whitespace() {
        // Strip surrounding punctuation we don't care about (commas, quotes),
        // but keep interior/leading dashes since flags begin with them.
        let t = tok
            .trim_matches(|c: char| c.is_ascii_punctuation() && c != '-')
            .to_string();
        if !t.starts_with('-') || t.len() < 2 {
            continue;
        }
        // Find the first non-dash char; it must be a letter (filters `-` alone,
        // `--` alone, numeric `-1`, and `-2 step` prose).
        let first_letter = match t.chars().find(|c| *c != '-') {
            Some(c) => c,
            None => continue,
        };
        if !first_letter.is_ascii_alphabetic() {
            continue;
        }
        // Strip a trailing `=value` (`--foo=bar` -> `--foo`) and trailing
        // punctuation glued to the flag.
        let flag: String = t
            .split('=')
            .next()
            .unwrap_or(&t)
            .trim_end_matches([',', '.', ';', ':', ')', ']', '\''])
            .to_string();
        if flag.len() >= 2 && !out.contains(&flag) {
            out.push(flag);
        }
    }
    out
}

#[cfg(test)]
mod checks {
    use super::*;

    #[test]
    fn extracts_double_and_single_flags() {
        let f = extract_flags("Usage: foo --bar -x --baz=42 end");
        assert!(f.contains(&"--bar".to_string()));
        assert!(f.contains(&"--baz".to_string()));
        assert!(f.contains(&"-x".to_string()));
        assert!(!f.iter().any(|s| s == "Usage:"));
    }

    #[test]
    fn ignores_hyphenated_prose() {
        let f = extract_flags("a two-step process - and dash-2 numbers");
        // `-` alone and `-2` are filtered; `two-step` is not a flag.
        assert!(!f.iter().any(|s| s == "-2"));
        assert!(!f.iter().any(|s| s == "two-step"));
    }
}
