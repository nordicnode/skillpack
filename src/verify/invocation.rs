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
    /// True iff introspect/the user confirmed a CLI exists on this machine.
    /// Used together with `cli_command` to decide whether we can *spawn*; CLI
    /// *presence* itself is derived from the SKILL.md (see [`run`]).
    pub has_cli: bool,
    /// The command to run with `--help`, argv-style: `["chronicle", "--help"]`.
    /// `None` when introspect found no runnable binary.
    pub cli_command: Option<Vec<String>>,
    /// Where the SKILL.md / manifest files live (project root, or the temp
    /// dir for `init`'s pre-commit gate).
    pub skill_root: std::path::PathBuf,
    /// Working directory for the CLI spawn — always the real project root.
    pub spawn_cwd: std::path::PathBuf,
    /// When true, print every subprocess spawn to stderr (design §8.2 --debug).
    pub debug: bool,
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
        debug: bool,
    ) -> Self {
        Self {
            skill_md: skill_md.to_string(),
            has_cli,
            cli_command: cli_command.map(<[std::string::String]>::to_vec),
            skill_root: skill_root.to_path_buf(),
            spawn_cwd: spawn_cwd.to_path_buf(),
            debug,
        }
    }
}

/// Run every invocation check, appending to `report`.
///
/// CLI presence is derived from whether the SKILL.md *documents* an invocation
/// (a `## Invocation`/`## Usage` block or a fenced command), NOT from
/// `input.has_cli` (which reflects whether introspect found a runnable binary
/// on *this* machine). This keeps `verify` honest about what the skill claims
/// even when run on a hand-written pack with no source tree (design §4.2).
/// `input.has_cli`/`cli_command` only gate whether we can actually *spawn*.
pub fn run(input: &InvocationInput, report: &mut VerifyReport) -> Result<()> {
    let skill_invocation = extract_documented_invocation(&input.skill_md);

    // A pure-library skill (no documented CLI invocation): nothing to spawn.
    if skill_invocation.is_none() {
        report.push(CheckResult::skipped(
            "invocation",
            "CLI invocation drift checks",
            "Skipped: pure-library project (no CLI documented in SKILL.md)",
        ));
        return Ok(());
    }

    // The skill documents a CLI. Can we actually spawn it here? If introspect
    // found no runnable command on this machine, we can't exercise drift — but
    // we surface that honestly as a WARNING, never a silent skip, so the
    // maintainer knows the invocation check didn't actually run (design §5.3).
    let Some(cmd) = input.cli_command.as_ref() else {
        report.push(CheckResult::warn(
            "invocation.not_runnable_here",
            "documented CLI can be spawned for drift checks",
            "SKILL.md documents a CLI invocation, but no runnable command was \
             found on this machine (no built artifact / runtime missing)",
            "To fix: build/install the CLI so `skillpack verify` can spawn its \
             `--help`, or run verify on a machine where the CLI is installed.",
        ));
        return Ok(());
    };
    if cmd.is_empty() {
        report.push(CheckResult::warn(
            "invocation.not_runnable_here",
            "documented CLI can be spawned for drift checks",
            "SKILL.md documents a CLI invocation, but the recorded command is empty",
            "To fix: re-run `skillpack init` so a CLI command is recorded.",
        ));
        return Ok(());
    }

    let help = run_help(cmd, &input.spawn_cwd, input.debug, report)?;
    if report.has_critical_failure() {
        return Ok(());
    }

    // Drift, scoped to the documented invocation block only (not the whole
    // SKILL.md body) so templated prose/footguns don't read as false flags.
    check_flag_drift(&help, skill_invocation.as_deref().unwrap_or(""), report);

    Ok(())
}

/// Pull the text of the SKILL.md that documents the CLI invocation, so flag-
/// drift extraction reads only the documented invocation area (not the templated
/// prose/footguns/metadata). Returns `None` when the skill is a pure library.
///
/// Two signals, in order:
/// 1. A `## Invocation` heading — the section the skillpack CLI template emits.
///    skillpack *libraries* use `## Usage` (never `## Invocation`), so this
///    cleanly separates the two for generated packs.
/// 2. A fenced code block containing a `--flag` token — the fallback for
///    *hand-written* skills (e.g. the `broken-cli` fixture) that document a CLI
///    without the `## Invocation` heading. A pure-library import block
///    (`import { parse } from 'x'`) has no `--flag`, so it correctly stays a
///    library (Bug 2 + Improvement F, without the prose false-positives that
///    scoping to the *whole* body would reintroduce).
pub fn extract_documented_invocation(skill_md: &str) -> Option<String> {
    // (1) Prefer an explicit `## Invocation` section.
    if let Some(block) = heading_block(skill_md, "invocation") {
        return Some(block);
    }

    // (2) Fallback: any fenced ``` block whose text contains a `--flag`.
    let mut in_fence = false;
    let mut block = String::new();
    for line in skill_md.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            if in_fence {
                // closing fence: does this block name a flag?
                if extract_flags(&block).iter().any(|f| !is_meta_flag(f)) {
                    return Some(block.clone());
                }
                block.clear();
            }
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            block.push_str(line);
            block.push('\n');
        }
    }
    None
}

/// Collect the body under a `## <heading>` section up to the next `## ` heading.
fn heading_block(skill_md: &str, heading: &str) -> Option<String> {
    let want = format!("## {heading}");
    let mut in_block = false;
    let mut out = String::new();
    for line in skill_md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            if in_block {
                break;
            }
            if trimmed.eq_ignore_ascii_case(&want) {
                in_block = true;
            }
            continue;
        }
        if in_block {
            out.push_str(line);
            out.push('\n');
        }
    }
    if in_block && !out.trim().is_empty() {
        Some(out)
    } else {
        None
    }
}

/// Spawn `<cmd[0]> [cmd[1..]]` (e.g. `chronicle --help`) under a hard timeout,
/// push the outcome as a check, and return the captured stdout+stderr on
/// success. When `debug` is set, the spawn argv is printed to stderr (§8.2).
fn run_help(cmd: &[String], root: &Path, debug: bool, report: &mut VerifyReport) -> Result<String> {
    let program = &cmd[0];
    if debug {
        eprintln!(
            "[debug] spawn (cwd={}): {}{}",
            root.display(),
            program,
            cmd[1..].iter().map(|a| format!(" {a}")).collect::<String>()
        );
    }
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
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => break,
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
    }

    // The poll loop established the child exited (or errored); `wait_with_output`
    // drains the buffered stdout/stderr AND yields the exit status, so we use
    // its status rather than the one `try_wait` probed — one wait, not two.
    let (status, output) = match child.wait_with_output() {
        Ok(o) => (
            Some(o.status),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        ),
        Err(_) => (None, String::new()),
    };

    let Some(status) = status else {
        // `wait_with_output` couldn't reap the child — surface it rather than
        // silently treating an empty status as success.
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

    // Reverse drift (Improvement A): flags the CLI advertises in `--help` that
    // the skill never documents. This is the drift direction that's real for
    // `init`-generated output (whose documented list is derived from --help,
    // so it can't itself drift forward) — an agent misses flags the skill
    // could have told it about. Warn, don't fail: undocumented flags aren't a
    // correctness bug, just a discoverability gap.
    reverse_drift(&help_flags, &doc_flags, report);
}

fn reverse_drift(help_flags: &[String], doc_flags: &[String], report: &mut VerifyReport) {
    let mut undocumented: Vec<String> = help_flags
        .iter()
        .filter(|f| !is_meta_flag(f) && !doc_flags.contains(f))
        .cloned()
        .collect();
    undocumented.sort();
    undocumented.dedup();
    if undocumented.is_empty() {
        return;
    }
    report.push(CheckResult::warn(
        "invocation.undocumented_flags",
        "every `--help` flag is documented in SKILL.md",
        format!(
            "`--help` advertises flags the skill doesn't document: {}",
            undocumented.join(", ")
        ),
        "To fix: document these flags in SKILL.md so an agent knows it can pass them.",
    ));
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

    #[test]
    fn documented_invocation_from_heading() {
        // The skillpack CLI template emits ## Invocation. That's the signal.
        let skill = "---\nname: foo\n---\n\n## Invocation\n\n```\nfoo --new\n```\n";
        let block = extract_documented_invocation(skill).expect("heading block");
        assert!(block.contains("foo --new"));
        assert!(extract_flags(&block).contains(&"--new".to_string()));
    }

    #[test]
    fn documented_invocation_from_fenced_flags_for_handwritten_skill() {
        // broken-cli fixture: a fenced block with flags but no ## Invocation heading.
        let skill = "---\nname: sample-broken\n---\n\n# sample-broken\n\n```\nsample-broken --nonexistent --new\n```\n";
        assert!(extract_documented_invocation(skill).is_some());
    }

    #[test]
    fn documented_invocation_none_for_pure_library() {
        // Pure-library ## Usage import block has no --flag => not a CLI.
        let skill = "---\nname: x\n---\n\n## Usage\n\n```\nimport { parse } from 'fastcsv'\n```\n";
        assert!(extract_documented_invocation(skill).is_none());
    }

    #[test]
    fn reverse_drift_warns_on_undocumented_help_flag() {
        // help advertises --verbose; skill documents only --new => --verbose is
        // flagged as undocumented (Improvement A, warning not failure).
        let mut report = super::super::result::VerifyReport::default();
        reverse_drift(
            &["--new".to_string(), "--verbose".to_string()],
            &["--new".to_string()],
            &mut report,
        );
        assert_eq!(report.results.len(), 1);
        assert_eq!(
            report.results[0].severity,
            super::super::result::Severity::Warn
        );
        assert!(report.results[0].message.contains("--verbose"));
    }
}
