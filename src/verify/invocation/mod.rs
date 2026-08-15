//! Invocation checks — actually run the documented CLI and catch drift between
//! what `SKILL.md` advertises and what `--help` actually offers.
//!
//! Design §5.2 step 3 + §6.3. The spawn is the same guarded, time-boxed spawn
//! as introspect (hard timeout, run in the project root). For pure-library
//! projects this entire suite is a no-op returning `Skipped` per §5.1's
//! "Pure-library path" — critical checks still run, no subprocess is spawned.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::Result;

use super::result::{CheckResult, VerifyReport};
use crate::spawn::HELP_TIMEOUT;

mod drift;
mod parse;

use drift::{check_flag_drift, check_subcommand_drift, check_version_drift};
// Re-export the parse helpers at the module's flat path — the call sites
// (`generate.rs`, `cli_probe.rs`, `verify/mod.rs`, the proptests) reach them
// as `invocation::extract_flags` etc. The `pub` ones are the public API;
// the rest stay crate-internal but need to be visible to `run`/tests below.
pub(crate) use parse::extract_version_token;
pub use parse::{
    command_from_documented, documented_subcommand_bullets, extract_documented_invocation,
    extract_documented_subcommands, extract_flags, extract_subcommands, is_meta_flag,
};

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
    /// Stdin bytes to feed the CLI during verify spawns. For interactive
    /// CLIs that block on stdin. `None` uses `/dev/null` (default).
    pub verify_stdin: Option<String>,
}

impl InvocationInput {
    /// `skill_root` is where the skill files live; `spawn_cwd` is where the
    /// documented CLI actually runs (the project root).
    pub fn new(
        skill_root: &Path,
        spawn_cwd: &Path,
        skill_md: &str,
        cli_command: Option<&[String]>,
        verify_stdin: Option<&str>,
    ) -> Self {
        Self {
            skill_md: skill_md.to_string(),
            cli_command: cli_command.map(<[std::string::String]>::to_vec),
            skill_root: skill_root.to_path_buf(),
            spawn_cwd: spawn_cwd.to_path_buf(),
            verify_stdin: verify_stdin.map(|s| s.to_string()),
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

    let help = run_help(cmd, &input.spawn_cwd, input.verify_stdin.as_deref(), report)?;
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
    check_subcommand_drift(
        cmd,
        &input.spawn_cwd,
        &input.skill_md,
        input.verify_stdin.as_deref(),
        report,
    )?;

    // Version drift: spawn `<cli> --version`, compare against plugin.json version.
    // Tolerates non-zero exit (some CLIs print version to stdout but exit 1).
    // Skips silently when `--version` produces no output or the CLI lacks the flag.
    check_version_drift(cmd, &input.spawn_cwd, &input.skill_root, report);

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
fn run_help(
    cmd: &[String],
    root: &Path,
    stdin: Option<&str>,
    report: &mut VerifyReport,
) -> Result<String> {
    let program = &cmd[0];
    let mut c = Command::new(program);
    for arg in &cmd[1..] {
        c.arg(arg);
    }
    c.current_dir(root);

    // The shared spawn helper forces piped stdout/stderr and drains them on
    // reader threads while polling, so a >64KB `--help` can't deadlock (the
    // old poll-without-draining loop would false-fail a fat CLI).
    match crate::spawn::run_with_stdin(&mut c, HELP_TIMEOUT, stdin.map(|s| s.as_bytes())) {
        crate::spawn::SpawnOutcome::NotFound => {
            report.push(CheckResult::fail(
                "invocation.help_present",
                "documented CLI is installed and runnable",
                format!("CLI binary `{program}` not found on PATH"),
                format!(
                    "To fix: build/install `{program}` so it's on PATH, then re-run `skillpack verify`."
                ),
            ));
            Ok(String::new())
        }
        crate::spawn::SpawnOutcome::SpawnFailed(e) => {
            report.push(CheckResult::fail(
                "invocation.help_present",
                "documented CLI is installed and runnable",
                format!("could not spawn `{program}`: {e}"),
                "To fix: check that the binary path in skillpack.toml is correct.",
            ));
            Ok(String::new())
        }
        crate::spawn::SpawnOutcome::TimedOut => {
            report.push(CheckResult::fail(
                "invocation.help_present",
                "CLI prints `--help` quickly",
                format!("`{program} --help` exceeded {}s timeout", HELP_TIMEOUT.as_secs()),
                "To fix: the CLI may hang waiting on input; guard it with `</dev/null` or fix the hang before shipping.",
            ));
            Ok(String::new())
        }
        crate::spawn::SpawnOutcome::RanNonZero(output) => {
            let mut msg = format!("`{program}` returned non-zero on `--help`");
            if !output.trim().is_empty() {
                msg.push_str(" (captured: ");
                msg.push_str(&snippet(&output, 160));
                msg.push(')');
            }
            report.push(CheckResult::fail(
                "invocation.help_present",
                "documented `--help` exits cleanly",
                msg,
                "To fix: make `--help` exit 0, or correct the command in skillpack.toml.",
            ));
            Ok(String::new())
        }
        crate::spawn::SpawnOutcome::RanClean(output) => {
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
    }
}

/// Truncate captured subprocess output for a human message: flatten whitespace
/// and cap to the first `max` chars so a huge `--help`/`--version` body can't
/// flood the verify report.
pub(crate) fn snippet(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(max).collect();
    if flat.chars().count() > max {
        out.push('…');
    }
    out
}

/// Compare documented flags in SKILL.md against those advertised in `--help`,
/// flagging any documented flag that does not actually exist (drift).
fn spawn_capture(
    cmd: &[String],
    root: &Path,
    timeout: Duration,
    stdin: Option<&str>,
) -> Option<String> {
    let mut c = Command::new(&cmd[0]);
    for arg in &cmd[1..] {
        c.arg(arg);
    }
    c.current_dir(root);
    match crate::spawn::run_with_stdin(&mut c, timeout, stdin.map(|s| s.as_bytes())) {
        crate::spawn::SpawnOutcome::RanClean(out) => Some(out),
        // Tolerate non-zero exit: some CLIs print `--version` to stdout/stderr
        // but exit 1, and version-drift checking wants that output.
        crate::spawn::SpawnOutcome::RanNonZero(out) => Some(out),
        _ => None,
    }
}

/// For each subcommand the SKILL.md documents, spawn `<base> <sub> --help` and
/// set-diff the documented flags against the real `--help`. Pushes one
/// `invocation.subcommand_drift` result per documented subcommand. A documented
/// subcommand whose `--help` won't spawn here fails (honest, like
/// `invocation.help_present`); a documented flag the real help omits fails;
/// reverse drift (help advertises a flag the skill doesn't) warns.
#[cfg(test)]
mod checks {
    use super::drift::{diff_one_subcommand, reverse_drift};
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

    /// clap emits `--flag[=<value>]` for optional args. `extract_flags` must
    /// strip the `[=...]` suffix *before* punctuation trim, otherwise the raw
    /// `--help` token (interior `[`) and the backtick-wrapped SKILL.md token
    /// (edge `[`) normalize differently — the asymmetry that broke fd.
    #[test]
    fn strips_clap_optional_arg_suffix_consistently() {
        // Raw --help form: `[=<when>]` glued, `[` interior → survives trim.
        assert_eq!(
            extract_flags("--hyperlink[=<when>]"),
            vec!["--hyperlink".to_string()]
        );
        // Backtick-wrapped SKILL.md form: `[` at edge → stripped by trim.
        // Both must yield the SAME clean flag (this is the fd regression).
        assert_eq!(
            extract_flags("`--hyperlink`"),
            vec!["--hyperlink".to_string()]
        );
        // Multi-word flag name with optional-arg suffix.
        assert_eq!(
            extract_flags("--strip-cwd-prefix[=<when>]"),
            vec!["--strip-cwd-prefix".to_string()]
        );
    }

    /// Rich rust\\ clap help (fd, rg, bat) contains prose examples that
    /// `extract_flags` must NOT swallow as documented flags. Three classes:
    /// multi-letter short flags (`-tf` documenting `--type f`), example
    /// patterns (`-foo` from "fd -- '-foo'"), and short/long pair prose
    /// (`-x'/'--exec` from "place the -x/--exec option last"), and find(1)
    /// comparisons (`-mount`, `-xdev`).
    #[test]
    fn ignores_prose_examples_in_help_text() {
        // Multi-letter short flags are prose, not real clap flags.
        let f = extract_flags("fd -tf -tl -tx -te -td");
        assert!(f.is_empty(), "multi-char short flags from prose: got {f:?}");
        // Example pattern from help prose.
        let f = extract_flags("fd -- '-foo' pattern");
        assert!(
            !f.iter().any(|s| s == "-foo"),
            "example pattern leaked: got {f:?}"
        );
        // Short/long pair separators with `/` or `'` are prose.
        let f = extract_flags("place the -x'/\u{27}--exec option last");
        assert!(f.is_empty(), "prose separators leaked: got {f:?}");
        // find(1) comparison prose.
        let f = extract_flags("Comparable to the -mount or -xdev filters of find(1)");
        assert!(f.is_empty(), "find(1) prose leaked: got {f:?}");
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

    /// Regression: `command_from_documented` must extract the program from the
    /// FENCED command line, never from the surrounding prose. The old code
    /// returned "The" (the first word of "The exact command an agent should
    /// run..."), which surfaced as "secondary skill documents CLI `The`".
    #[test]
    fn command_from_documented_skips_prose_and_reads_fence() {
        let skill = "\
## Invocation

The exact command an agent should run to use this tool:

```
chronicle --new \"entry\"
```
";
        assert_eq!(
            command_from_documented(skill),
            Some(vec!["chronicle".to_string(), "--help".to_string()])
        );
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

    /// cobra (Go) help lists subcommands under `Available Commands:` — the
    /// most common Go CLI framework. The parser must read it the same way it
    /// reads clap's `Commands:` (indented names, `help` filtered, blank gap
    /// ends the section).
    #[test]
    fn extract_subcommands_from_cobra_help() {
        let help = "\
A CLI for gophers.

Usage:
  gocli [command]

Available Commands:
  completion  Generate the autocompletion script for the specified shell
  serve       Start the server
  help        Help about any command

Flags:
  -h, --help   help for gocli
";
        assert_eq!(
            extract_subcommands(help),
            vec!["completion".to_string(), "serve".to_string()]
        );
    }

    /// argparse (Python) has no dedicated `Commands:` section — subcommands
    /// appear inline in the usage line as `{cmd1,cmd2}`. The parser's fallback
    /// must read that group (and only the first one).
    #[test]
    fn extract_subcommands_from_argparse_usage() {
        let help = "\
usage: pycli [-h] {build,test,run} ...

positional arguments:
  command     subcommand to run
";
        assert_eq!(
            extract_subcommands(help),
            vec!["build".to_string(), "test".to_string(), "run".to_string()]
        );
    }

    /// argparse with a single subcommand and `help` mixed in: `help` is
    /// filtered, and the single real subcommand survives.
    #[test]
    fn extract_subcommands_from_argparse_usage_filters_help() {
        assert_eq!(
            extract_subcommands("usage: prog [-h] {serve,help}"),
            vec!["serve".to_string()]
        );
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
            vec![vec!["init".to_string()], vec!["verify".to_string()]]
        );
    }

    /// Nested `### Subcommands` bullets (the template emits two spaces per
    /// depth) must parse into multi-segment paths, and a following top-level
    /// bullet must pop back to the root (not inherit the nested parent).
    #[test]
    fn documented_subcommand_bullets_parse_nested_paths() {
        let skill = "\
### Subcommands

- `remote` — flags: `--verbose`
  - `add` — flags: `--name`
  - `remove` — flags: `--name`
- `status` — flags: `--porcelain`
";
        assert_eq!(
            documented_subcommand_bullets(skill),
            vec![
                (
                    vec!["remote".to_string()],
                    "- `remote` — flags: `--verbose`".to_string()
                ),
                (
                    vec!["remote".to_string(), "add".to_string()],
                    "  - `add` — flags: `--name`".to_string()
                ),
                (
                    vec!["remote".to_string(), "remove".to_string()],
                    "  - `remove` — flags: `--name`".to_string()
                ),
                (
                    vec!["status".to_string()],
                    "- `status` — flags: `--porcelain`".to_string()
                ),
            ]
        );
    }

    /// A SKILL.md with no `### Subcommands` block documents nothing → no
    /// per-subcommand drift checks run.
    #[test]
    fn extract_documented_subcommands_empty_when_no_block() {
        let skill = "## Invocation\n\n```\nchronicle --new\n```\n";
        assert!(extract_documented_subcommands(skill).is_empty());
    }

    /// `extract_version_token` finds the first standalone version token and
    /// normalizes the conventional `v`/`V` prefix away, so `check_version_drift`
    /// compares versions exactly instead of via loose substring containment.
    #[test]
    fn extract_version_token_normalizes_common_shapes() {
        assert_eq!(
            extract_version_token("skillpack 0.13.0"),
            Some("0.13.0".into())
        );
        assert_eq!(extract_version_token("v0.13.0"), Some("0.13.0".into()));
        assert_eq!(extract_version_token("V1.2.3"), Some("1.2.3".into()));
        assert_eq!(
            extract_version_token("0.13.0 (build abc)"),
            Some("0.13.0".into())
        );
        // Semver pre-release + build metadata survive intact.
        assert_eq!(
            extract_version_token("1.0.0-rc.1+build2"),
            Some("1.0.0-rc.1+build2".into())
        );
        // Parenthesized/glued punctuation is stripped around the token.
        assert_eq!(extract_version_token("(0.1.0)"), Some("0.1.0".into()));
    }

    /// Non-version tokens (a git SHA, a date-less word, prose) return `None`
    /// so the caller falls back to substring containment rather than a bogus
    /// exact compare.
    #[test]
    fn extract_version_token_returns_none_for_non_version_output() {
        assert_eq!(extract_version_token("build abc123def"), None);
        assert_eq!(extract_version_token(""), None);
        assert_eq!(extract_version_token("unknown"), None);
    }

    /// The substring fallback's false positive is gone: `0.1` used to "match"
    /// a CLI printing `0.1.0` (prefix), and `1` used to match `11.0`. Exact
    /// token comparison distinguishes them.
    #[test]
    fn version_drift_exact_match_rejects_prefix_substrings() {
        // A version token present but different → must NOT be considered equal.
        assert_ne!(
            extract_version_token("0.1.0").as_deref(),
            Some("0.1"),
            "0.1.0 must not equal 0.1 under exact token matching"
        );
        assert_ne!(
            extract_version_token("11.0").as_deref(),
            Some("1"),
            "11.0 must not equal 1 under exact token matching"
        );
    }

    /// Regression for ce2a892 + the `no`-inversion it shipped: the reverse-drift
    /// "To fix" hint was a bare `&str` literal so `{sub}` rendered verbatim, and
    /// the warn check_name read "advertises no undocumented flags" — a double
    /// negative in a branch that fires when undocumented flags DO exist. Reach
    /// the production strings via `diff_one_subcommand` (no spawn) and assert
    /// both the suggestion (interpolated name, no `{sub}`) and the check_name
    /// ("undocumented flags", no leading "no"). If someone reverts either
    /// fix this test fails.
    #[test]
    fn subcommand_reverse_drift_hint_interpolates_name() {
        // `init` bullet documents `--foo`; the help advertises `--foo` PLUS
        // `--secret` → reverse drift fires with `init` as the sub name.
        let bullet = "- `init`: create\n  `--foo`";
        let help = "Usage: init\n\nOptions:\n  --foo   f\n  --secret   s\n";
        let mut report = VerifyReport::default();
        diff_one_subcommand(bullet, &["init".to_string()], help, &mut report);
        let warn = report
            .results
            .iter()
            .find(|r| r.suggestion.is_some())
            .expect("reverse-drift warn should produce a suggestion");
        let s = warn.suggestion.as_deref().unwrap();
        assert!(
            s.contains("`init`"),
            "suggestion must carry the real subcommand name `init`, got: {s}"
        );
        assert!(
            !s.contains("{sub}"),
            "suggestion must NOT leak the literal `{{sub}}` placeholder, got: {s}"
        );
        assert!(s.contains("SKILL.md"), "suggestion should name SKILL.md");
        // The warn check_name must read "advertises undocumented flags", NOT
        // "advertises no undocumented flags" — the branch fires when the set
        // is non-empty, so the old negation inverted the meaning.
        assert!(
            warn.check_name.contains("undocumented flags"),
            "check_name should describe undocumented flags, got: {}",
            warn.check_name
        );
        assert!(
            !warn.check_name.contains("no undocumented"),
            "check_name must NOT carry the old `no undocumented` inversion, got: {}",
            warn.check_name
        );
    }
}
