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
/// (`verify` dispatcher) reads the SKILL.md + holds the spawn command, while
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
    /// The command to run with `--help`, argv-style: `["chronicle", "--help"]`.
    /// `None` when introspect found no runnable binary. CLI *presence* itself is
    /// derived from the SKILL.md (see [`run`]); this only gates whether we can
    /// actually spawn.
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
        cli_command: Option<&[String]>,
        debug: bool,
    ) -> Self {
        Self {
            skill_md: skill_md.to_string(),
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
/// `cli_command` (which only reflects whether introspect found a runnable binary
/// on *this* machine). This keeps `verify` honest about what the skill claims
/// even when run on a hand-written pack with no source tree (design §4.2).
/// `cli_command` only gates whether we can actually *spawn*.
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

    // Per-subcommand drift: if the SKILL.md documents subcommands, spawn each
    // one's `--help` and set-diff its flags against what the SKILL.md advertises
    // for that subcommand. Skipped (no check pushed) when the skill documents
    // no subcommands — non-subcommand CLIs behave exactly as before.
    check_subcommand_drift(cmd, &input.spawn_cwd, &input.skill_md, input.debug, report)?;

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
/// Stops at any `### ` (deeper) heading too: a subsection under `## Invocation`
/// (the `### Subcommands` block the CLI template emits) owns its own flags and
/// is drift-checked separately — including it here would let per-subcommand
/// flags like `--root` read as top-level drift against `<cli> --help`.
fn heading_block(skill_md: &str, heading: &str) -> Option<String> {
    let want = format!("## {heading}");
    let mut in_block = false;
    let mut out = String::new();
    for line in skill_md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") || trimmed.starts_with("### ") {
            if trimmed.starts_with("## ") && trimmed.eq_ignore_ascii_case(&want) {
                in_block = true;
                continue;
            }
            if in_block {
                break;
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
    } else {
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
            format!(
                "To fix: remove `{first}` from SKILL.md, or add `{first}` to your CLI's `--help`."
            ),
        );
        fail.location = Some(("SKILL.md".to_string(), line_hint));
        report.push(fail);
    }

    // Reverse drift always runs — including on the no-forward-drift success
    // path — so a CLI advertising flags a hand-written skill never documents
    // still warns (the feature the README advertises). Previously the pass
    // branch returned early, gating this off entirely. Warn, don't fail:
    // undocumented flags are a discoverability gap, not a correctness bug.
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
pub fn is_meta_flag(flag: &str) -> bool {
    matches!(flag, "--help" | "-h" | "--version" | "-V" | "--help-all")
}

/// Parse the subcommand names advertised in a top-level `--help` body.
/// Recognizes the clap-standard section header (`Commands:` / `Subcommands:`,
/// case-tolerant) and reads each following indented line's first token as the
/// subcommand name. clap's auto-added `help` subcommand is filtered. Lines
/// that aren't indented under the header (`Options:`, `Arguments:`, a blank
/// gap, or the usage line) end the section.
///
/// Returns `[]` for CLIs with no subcommands — so a non-subcommand `--help`
/// (a flat `Usage: chronicle [OPTIONS] ...`) yields nothing, and the
/// subcommand-aware template/verify paths stay dormant (byte-identical
/// snapshots, no extra checks). ponytail: one level deep — nested subcommands
/// (`git remote add`) aren't recursed; upgrade by recursing on
/// `<base> <sub> --help` when a fixture needs it.
pub fn extract_subcommands(help_output: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in help_output.lines() {
        let trimmed = line.trim();
        let is_header = matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "commands:" | "subcommands:"
        );
        if is_header {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        // A blank line or a new un-indented section header ends the block.
        if trimmed.is_empty() {
            // clap separates the last subcommand line from `Options:` with a
            // blank line — end here so `Options:` entries aren't swept in.
            break;
        }
        // Only count lines that are indented under the header (clap uses 2
        // spaces). An un-indented line is the start of the next section.
        if line == trimmed {
            break;
        }
        let Some(name) = trimmed.split_whitespace().next() else {
            continue;
        };
        if name == "help" {
            continue;
        }
        if !out.contains(&name.to_string()) {
            out.push(name.to_string());
        }
    }
    out
}

/// Pull the subcommand names a SKILL.md *documents* (the `### Subcommands`
/// bullets), so verify checks drift against exactly what the skill advertises
/// — the published surface, not the introspected one. Mirrors
/// [`extract_documented_invocation`]: a template section is the signal.
/// `### Subcommands` is an h3 (deliberately a subsection under the `## Invocation`
/// h2), so this is its own scan rather than reusing the h2-only `heading_block`.
pub fn extract_documented_subcommands(skill_md: &str) -> Vec<String> {
    let want = "### Subcommands";
    let mut in_block = false;
    let mut block = String::new();
    for line in skill_md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("### ") || trimmed.starts_with("## ") {
            if in_block {
                break;
            }
            if trimmed.eq_ignore_ascii_case(want) {
                in_block = true;
            }
            continue;
        }
        if in_block {
            block.push_str(line);
            block.push('\n');
        }
    }
    let mut out = Vec::new();
    for line in block.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("- ") {
            continue;
        }
        // Each bullet is `- `name` ...`; the first backticked token is the name.
        let after = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let name = after
            .split('`')
            .nth(1)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(n) = name {
            if !out.contains(&n.to_string()) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// The captured stdout+stderr of a subcommand `--help` spawn, or `None` if it
/// couldn't be spawned/reaped. Mirrors `run_help`'s poll loop without pushing
/// the top-level `help_present` check — per-subcommand drift owns its own
/// single `subcommand_drift` result.
fn spawn_capture(cmd: &[String], root: &Path, timeout: Duration) -> Option<String> {
    let mut c = Command::new(&cmd[0]);
    for arg in &cmd[1..] {
        c.arg(arg);
    }
    c.current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

/// For each subcommand the SKILL.md documents, spawn `<base> <sub> --help` and
/// set-diff the documented flags against the real `--help`. Pushes one
/// `invocation.subcommand_drift` result per documented subcommand. A documented
/// subcommand whose `--help` won't spawn here fails (honest, like
/// `invocation.help_present`); a documented flag the real help omits fails;
/// reverse drift (help advertises a flag the skill doesn't) warns.
fn check_subcommand_drift(
    base_cmd: &[String],
    spawn_cwd: &Path,
    skill_md: &str,
    debug: bool,
    report: &mut VerifyReport,
) -> Result<()> {
    let documented = extract_documented_subcommands(skill_md);
    if documented.is_empty() {
        return Ok(());
    }
    // The base argv already carries `--help` (introspect appends it); drop the
    // trailing `--help` so we can rebuild `<base> <sub> --help`.
    let mut base = base_cmd.to_vec();
    if base.last().is_some_and(|t| t == "--help") {
        base.pop();
    }

    for sub in &documented {
        let mut cmd = base.clone();
        cmd.push(sub.clone());
        cmd.push("--help".to_string());
        if debug {
            eprintln!(
                "[debug] spawn (cwd={}): {}",
                spawn_cwd.display(),
                cmd.join(" ")
            );
        }
        let captured = spawn_capture(&cmd, spawn_cwd, HELP_TIMEOUT);
        let Some(help) = captured else {
            report.push(CheckResult::fail(
                "invocation.subcommand_drift",
                "every documented subcommand can be spawned for drift checks",
                format!("documented subcommand `{sub}` could not be spawned for `--help` (missing runtime / non-zero exit / timeout)"),
                "To fix: build/install the CLI so the subcommand is runnable, or remove the subcommand from SKILL.md.",
            ));
            continue;
        };

        // Documented flags for THIS subcommand: the SKILL.md's subcommand
        // bullet, parsed back out. Reusing extract_flags on the bullet line
        // keeps the comparison consistent with the top-level drift check.
        let bullet = subcommand_bullet(skill_md, sub);
        let doc_flags: Vec<String> = extract_flags(&bullet)
            .into_iter()
            .filter(|f| !is_meta_flag(f))
            .collect();
        let help_flags = extract_flags(&help);

        let drifted: Vec<String> = doc_flags
            .iter()
            .filter(|f| !help_flags.contains(*f))
            .cloned()
            .collect();

        let check_name = format!("documented subcommand `{sub}` flags match `--help`");
        if drifted.is_empty() {
            report.push(CheckResult::pass(
                "invocation.subcommand_drift",
                &check_name,
                format!("`{sub}` documented flags all present in --help"),
            ));
        } else {
            report.push(CheckResult::fail(
                "invocation.subcommand_drift",
                &check_name,
                format!("subcommand `{sub}` documents flags missing from `--help`: {}", drifted.join(", ")),
                format!("To fix: remove the flags from SKILL.md's `{sub}` bullet, or add them to `{sub}`'s `--help`."),
            ));
        }

        // Reverse drift (help advertises flags the skill's bullet omits) → warn.
        let undocumented: Vec<String> = help_flags
            .iter()
            .filter(|f| !is_meta_flag(f) && !doc_flags.contains(*f))
            .cloned()
            .collect();
        if !undocumented.is_empty() {
            let warn_name = format!("subcommand `{sub}` advertises no undocumented flags");
            report.push(CheckResult::warn(
                "invocation.subcommand_drift",
                &warn_name,
                format!(
                    "`{sub} --help` advertises flags the skill doesn't document: {}",
                    undocumented.join(", ")
                ),
                "To fix: document these flags in SKILL.md's `{sub}` bullet.",
            ));
        }
    }
    Ok(())
}

/// The single SKILL.md bullet line for a documented subcommand (the line
/// starting with `- \`<sub>\``), so per-sub drift reads only that subcommand's
/// advertised flags — not the whole `### Subcommands` block or the body.
fn subcommand_bullet(skill_md: &str, sub: &str) -> String {
    let needle = format!("- `{sub}`");
    skill_md
        .lines()
        .find(|l| l.trim().starts_with(&needle))
        .unwrap_or_default()
        .to_string()
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

    // --- subcommand discovery (the gap this change fixes) --------------------

    /// skillpack's own top-level `--help` is the canonical subcommand-shaped
    /// body: a `Commands:` section with two real subcommands plus clap's
    /// auto-added `help`. `help` is filtered; init/verify survive in order.
    #[test]
    fn extract_subcommands_from_skillpack_help() {
        let help = "\
Generate and verify the agent-distribution layer for any OSS project.

Usage: skillpack [OPTIONS] <COMMAND>

Commands:
  init    Scaffold the distribution layer
  verify  Check the distribution files against the schema
  help    Print this message or the help of the given subcommand(s)

Options:
      --verbose  Print what skillpack detected in the repo
  -h, --help     Print help
  -V, --version  Print version
";
        assert_eq!(
            extract_subcommands(help),
            vec!["init".to_string(), "verify".to_string()]
        );
    }

    /// A flat, non-subcommand `--help` (the rust-cli/broken-cli fixtures'
    /// hand-printed `Usage:` line) has no `Commands:` section → `[]`. This is
    /// the property that keeps the existing snapshots + integration tests
    /// byte-identical: no subcommands → no template block, no drift checks.
    #[test]
    fn extract_subcommands_empty_for_non_subcommand_help() {
        assert_eq!(
            extract_subcommands("Usage: chronicle [--new <entry>] [--verbose]"),
            Vec::<String>::new()
        );
        assert_eq!(extract_subcommands(""), Vec::<String>::new());
    }

    /// The subcommand section ends at the blank line before `Options:` — the
    /// global flags under `Options:` must NOT be read as subcommands.
    #[test]
    fn extract_subcommands_stops_at_blank_gap() {
        let help = "\
Usage: x <COMMAND>

Commands:
  foo  one
  bar  two

Options:
  --global  g
";
        assert_eq!(
            extract_subcommands(help),
            vec!["foo".to_string(), "bar".to_string()]
        );
    }

    /// `### Subcommands` bullets are the SKILL.md source of truth for what's
    /// documented; verify drift checks exactly these.
    #[test]
    fn extract_documented_subcommands_from_skill_bullets() {
        let skill = "\
# x

## Invocation

```
x --new
```

### Subcommands

- `init` — flags: `--root`, `--non-interactive`
- `verify` — flags: `--format`
";
        assert_eq!(
            extract_documented_subcommands(skill),
            vec!["init".to_string(), "verify".to_string()]
        );
    }

    /// A SKILL.md with no `### Subcommands` block documents nothing → no
    /// per-subcommand drift checks run.
    #[test]
    fn extract_documented_subcommands_empty_when_no_block() {
        let skill = "## Invocation\n\n```\nchronicle --new\n```\n";
        assert!(extract_documented_subcommands(skill).is_empty());
    }
}
