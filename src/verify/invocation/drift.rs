//! Drift checks: diff the SKILL.md-documented CLI surface (flags,
//! subcommands, version) against what the real `--help`/`--version` outputs
//! report. Pure-ish — the only subprocess spawns are delegated to
//! `super::spawn_capture`, which returns captured output without pushing
//! checks; every check function below is a set-diff over that output.

use std::path::Path;

use anyhow::Result;

use super::documented_subcommand_bullets;
use super::extract_flags;
use super::extract_subcommands;
use super::extract_version_token;
use super::is_meta_flag;
use super::spawn_capture;
use crate::spawn::HELP_TIMEOUT;
use crate::verify::result::{CheckResult, VerifyReport};

pub(crate) fn check_flag_drift(help_output: &str, skill_md: &str, report: &mut VerifyReport) {
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
        let subcommands = extract_subcommands(help_output);
        if !subcommands.is_empty() {
            return;
        }
        report.push(CheckResult::warn(
            "invocation.flag_drift",
            "SKILL.md documents flags that match `--help`",
            "no flags appear to be documented in SKILL.md (no `--flag` tokens found)",
            "To fix: document the CLI's flags so an agent knows what to pass — or re-run \
             `skillpack init --target all` to regenerate them from `--help` (note: `update`/\
             `--fix` preserve body prose and can't refresh the flags).",
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
                "To fix: remove `{first}` from SKILL.md, or add `{first}` to your CLI's `--help` \
                 (then re-run `skillpack init --target all` to regenerate the documented flags \
                 — `update`/`--fix` preserve body prose and can't refresh them)."
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

pub(crate) fn reverse_drift(
    help_flags: &[String],
    doc_flags: &[String],
    report: &mut VerifyReport,
) {
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

/// For each subcommand the SKILL.md documents, spawn `<base> <sub> --help` and
/// set-diff the documented flags against the real `--help`. Pushes one
/// `invocation.subcommand_drift` result per documented subcommand. A documented
/// subcommand whose `--help` won't spawn here fails (honest, like
/// `invocation.help_present`); a documented flag the real help omits fails;
/// reverse drift (help advertises a flag the skill doesn't) warns.
pub(crate) fn check_subcommand_drift(
    base_cmd: &[String],
    spawn_cwd: &Path,
    skill_md: &str,
    stdin: Option<&str>,
    report: &mut VerifyReport,
) -> Result<()> {
    let bullets = documented_subcommand_bullets(skill_md);
    if bullets.is_empty() {
        return Ok(());
    }
    // The base argv already carries `--help` (introspect appends it); drop the
    // trailing `--help` so we can rebuild `<base> <path...> --help`.
    let mut base = base_cmd.to_vec();
    if base.last().is_some_and(|t| t == "--help") {
        base.pop();
    }

    for (path, bullet) in &bullets {
        let mut cmd = base.clone();
        cmd.extend(path.iter().cloned());
        cmd.push("--help".to_string());
        let captured = spawn_capture(&cmd, spawn_cwd, HELP_TIMEOUT, stdin);
        let Some(help) = captured else {
            report.push(CheckResult::fail(
                "invocation.subcommand_drift",
                "every documented subcommand can be spawned for drift checks",
                format!("documented subcommand `{}` could not be spawned for `--help` (missing runtime / non-zero exit / timeout)", path.join(" ")),
                "To fix: build/install the CLI so the subcommand is runnable, or remove the subcommand from SKILL.md.",
            ));
            continue;
        };
        // Documented flags for THIS subcommand: the SKILL.md's subcommand
        // bullet, parsed back out. Reusing extract_flags on the bullet line
        // keeps the comparison consistent with the top-level drift check.
        diff_one_subcommand(bullet, path, &help, report);
    }
    Ok(())
}

/// Pure drift + reverse-drift diff for one documented subcommand path against
/// its captured `--help` text. Split out of `check_subcommand_drift` so the
/// suggestion strings can be regression-tested without spawning a real CLI —
/// `check_subcommand_drift` is responsible only for the spawn + the
/// spawn-fail result; this fn owns every message that depends on actually
/// reading the captured help. `path` is the subcommand chain
/// (`["remote", "add"]`), `bullet` its SKILL.md bullet line (verbatim).
pub(crate) fn diff_one_subcommand(
    bullet: &str,
    path: &[String],
    help: &str,
    report: &mut VerifyReport,
) {
    let sub = path.join(" ");
    let doc_flags: Vec<String> = extract_flags(bullet)
        .into_iter()
        .filter(|f| !is_meta_flag(f))
        .collect();
    let help_flags = extract_flags(help);

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
            format!(
                "subcommand `{sub}` documents flags missing from `--help`: {}",
                drifted.join(", ")
            ),
            format!(
                "To fix: remove the flags from SKILL.md's `{sub}` bullet, or add them to `{sub}`'s \
                 `--help` (then re-run `skillpack init --target all` to regenerate the subcommand \
                 surface — `update`/`--fix` preserve body prose and can't refresh it)."
            ),
        ));
    }

    // Reverse drift (help advertises flags the skill's bullet omits) → warn.
    let undocumented: Vec<String> = help_flags
        .iter()
        .filter(|f| !is_meta_flag(f) && !doc_flags.contains(*f))
        .cloned()
        .collect();
    if !undocumented.is_empty() {
        let warn_name = format!("subcommand `{sub}` advertises undocumented flags");
        report.push(CheckResult::warn(
            "invocation.subcommand_drift",
            &warn_name,
            format!(
                "`{sub} --help` advertises flags the skill doesn't document: {}",
                undocumented.join(", ")
            ),
            format!(
                "To fix: document these flags in SKILL.md's `{sub}` bullet — or re-run `skillpack \
                 init --target all` to regenerate them from `{sub} --help`."
            ),
        ));
    }
}

/// Spawn `<cli> --version`, read the output as a version string, and compare
/// against the `.version` in `plugin.json` (under `skill_root`). Skips
/// silently when `--version` won't spawn (non-zero / timeout / not found)
/// or when plugin.json is missing — this is advisory, not a gate. Warns on
/// mismatch. The compare is substring-based: `stdout.contains(plugin_version)`
/// so `chronicle 0.1.0\n` matches `0.1.0`.
pub(crate) fn check_version_drift(
    base_cmd: &[String],
    spawn_cwd: &Path,
    skill_root: &Path,
    report: &mut VerifyReport,
) {
    // Read plugin.json version (advisory — skip if missing/unparseable).
    let plugin_path = skill_root.join(".claude-plugin").join("plugin.json");
    let plugin_version = std::fs::read_to_string(&plugin_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from));

    let Some(plugin_version) = plugin_version else {
        report.push(CheckResult::skipped(
            "invocation.version_drift",
            "CLI --version matches plugin.json version",
            "Skipped: no plugin.json found or no version field",
        ));
        return;
    };

    // Build `<cli> --version` from base_cmd (drop trailing --help if present).
    let mut version_cmd = base_cmd.to_vec();
    if version_cmd.last().is_some_and(|t| t == "--help") {
        version_cmd.pop();
    }
    version_cmd.push("--version".to_string());
    let Some(stdout) = spawn_capture(&version_cmd, spawn_cwd, HELP_TIMEOUT, None) else {
        report.push(CheckResult::skipped(
            "invocation.version_drift",
            "CLI --version matches plugin.json version",
            "Skipped: `--version` could not be spawned or produced no output (some CLIs lack --version)",
        ));
        return;
    };

    let stdout = stdout.trim();
    if stdout.is_empty() {
        report.push(CheckResult::skipped(
            "invocation.version_drift",
            "CLI --version matches plugin.json version",
            "Skipped: `--version` produced empty output",
        ));
        return;
    }

    // Exact-match the first standalone version token when the CLI prints one
    // (e.g. `skillpack 0.13.0` → `0.13.0`, `v0.13.0` → `0.13.0`). The old
    // substring check made `plugin 0.1` "match" a CLI printing `0.1.0`, or
    // `plugin 1` match a CLI printing `11.0` — a false pass is worse than a
    // false warn. Substring containment remains only as the fallback for CLIs
    // that embed the version inside a longer token with no standalone version.
    let matches = extract_version_token(stdout)
        .map(|tok| tok == plugin_version)
        .unwrap_or_else(|| stdout.contains(&plugin_version));

    if !matches {
        report.push(CheckResult::warn(
            "invocation.version_drift",
            "CLI --version matches plugin.json version",
            format!("`--version` output `{stdout}` does not match plugin.json version `{plugin_version}`"),
            "To fix: re-run `skillpack update` to sync plugin.json with the CLI's version, or pin the version intentionally.",
        ));
    } else {
        report.push(CheckResult::pass(
            "invocation.version_drift",
            "CLI --version matches plugin.json version",
            format!("`{stdout}` matches plugin.json version `{plugin_version}`"),
        ));
    }
}
