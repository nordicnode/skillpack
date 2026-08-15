//! The `skillpack verify` subcommand: load the generated distribution files
//! and run discovery + invocation checks against them.
//!
//! Design §5.2. `verify` works even on hand-written plugin files (not just
//! `init` output) — see §4.2 — so the loader is tolerant of missing pieces and
//! each check degrades gracefully.

pub mod discovery;
pub mod fix;
pub mod invocation;
pub mod result;
pub mod schema;

use anyhow::Result;

use self::invocation::InvocationInput;
use self::result::CheckResult;

// Re-export the pieces the rest of the crate touches.
pub use self::result::VerifyReport;

/// Where the invocation stage should look for skill text. Passed in so the
/// dispatcher owns the single `find_skill_file` call.
///
/// `root` is where the skill/manifest files live (the project root for the
/// `verify` subcommand, the temp dir for `init`'s pre-commit gate).
/// `spawn_root` is the real project root the CLI spawns from — it must be
/// separate from `root` so the pre-commit gate can spawn the real CLI in its
/// source tree while still verifying the rendered files (design §5.3 + §6.3).
#[derive(Debug, Clone)]
pub struct VerifyInput {
    pub root: std::path::PathBuf,
    /// The real project root the documented CLI runs in. For the `verify`
    /// subcommand this equals `root`; for `init`'s pre-commit gate it's the
    /// project root while `root` is the temp dir holding the rendered files.
    pub spawn_root: std::path::PathBuf,
    pub cli_command: Option<Vec<String>>,
    /// The repo URL `git remote get-url origin` produced at introspection
    /// (cached on `ProjectProfile.repo_url`, threaded here so `discovery`'s
    /// URL-drift check stays free of a subprocess spawn — see module doc).
    /// `None` when no git origin is configured.
    pub repo_url: Option<String>,
    /// The kebab-coerced project name (`coerce_kebab(&profile.name)`,
    /// already computed for rendering). Threaded so `discovery`'s
    /// `discovery.skill.name_drift` check can compare the SKILL.md `name:`
    /// frontmatter against the canonical value the template renders — without
    /// `discovery` itself calling `coerce_kebab` or building a `ProjectProfile`.
    /// `None` only when introspection couldn't derive a name at all.
    pub profile_name: Option<String>,
    /// Stdin bytes to feed the CLI during `verify` spawns. For interactive
    /// CLIs that block on stdin. `None` uses `/dev/null` (default).
    pub verify_stdin: Option<String>,
}

/// Run the full verify suite against `root`, returning the aggregate report.
pub fn run(input: &VerifyInput) -> Result<VerifyReport> {
    let root = &input.root;
    let mut report = VerifyReport::default();

    // Discovery checks (pure, file reads only + the threaded repo_url +
    // profile_name for the plugin.json / SKILL.md drift sub-checks).
    for check in discovery::run(root, &input.repo_url, &input.profile_name)? {
        report.push(check);
    }

    // Invocation checks run against EVERY skill that documents a CLI, so a
    // multi-skill pack can't hide drift in a secondary skill. The primary
    // (first) CLI spawns from the introspected `cli_command` (which may be a
    // resolved absolute path); each secondary skill derives its program from
    // its own documented invocation and must be on PATH to be spawnable.
    let skill_files = discovery::find_skill_files(root);
    let mut spawned_primary = false;
    // The Claude target emits the same SKILL.md at BOTH `skills/<name>/` and
    // the native `.claude/skills/<name>/`. They are one skill, not two —
    // dedupe by skill directory name so invocation checks run once per skill.
    // (Discovery still checks both copies structurally; this only avoids a
    // redundant double spawn of the same CLI.)
    let mut seen_skill_dirs = std::collections::HashSet::new();
    for skill_path in &skill_files {
        if let Some(dir) = skill_path.parent().and_then(|p| p.file_name()) {
            if !seen_skill_dirs.insert(dir.to_string_lossy().to_string()) {
                continue;
            }
        }
        let skill_md = match std::fs::read_to_string(skill_path) {
            Ok(s) => s,
            Err(e) => {
                // Path exists (find_skill_files returned it) — read failure is
                // non-missing (permissions, non-UTF8, EBUSY). Discovery's
                // `check_one_skill_md` would abort verify on the same file;
                // surface a WARN here so the maintainer sees the read failure.
                report.push(CheckResult::warn(
                    "invocation.read_failed",
                    "skills a verify can spawn should be readable",
                    format!("{}: read failed ({}); invocation drift check skipped for this skill", discovery::rel_unix(root, skill_path), e),
                    "To fix: check file permissions, ensure UTF-8 encoding (no Latin-1), and re-run.",
                ));
                continue;
            }
        };
        // A pure-library skill (or one with no documented CLI) still goes
        // through `invocation::run`, which emits its "Skipped: pure-library
        // project" result — silently `continue`-ing here would drop that
        // signal from the report.
        let is_cli = invocation::extract_documented_invocation(&skill_md).is_some();
        let cmd = if !is_cli {
            None
        } else if !spawned_primary {
            spawned_primary = true;
            input.cli_command.clone()
        } else {
            // Secondary skill: derive the command from its own invocation. If
            // the binary is not on PATH (expected on many machines, since
            // introspection only resolved the primary), warn and skip rather
            // than false-fail a legitimately multi-CLI pack.
            match invocation::command_from_documented(&skill_md) {
                Some(c) if crate::introspect::which_on_path(&c[0]).is_some() => Some(c),
                Some(c) => {
                    report.push(CheckResult::warn(
                        "invocation.secondary_not_runnable",
                        "every documented CLI can be spawned for drift checks",
                        format!("secondary skill documents CLI `{}`, which is not on PATH; its drift checks were skipped", c[0]),
                        "To fix: install/build the secondary CLI so it is on PATH, then re-run verify.",
                    ));
                    continue;
                }
                None => {
                    report.push(CheckResult::warn(
                        "invocation.secondary_unparseable",
                        "every documented CLI can be spawned for drift checks",
                        format!("could not derive a command from {}'s documented invocation; its drift checks were skipped", discovery::rel_unix(root, skill_path)),
                        "To fix: document the CLI with a plain command line in the `## Invocation` section.",
                    ));
                    continue;
                }
            }
        };
        let inv = InvocationInput::new(
            root,
            &input.spawn_root,
            &skill_md,
            cmd.as_deref(),
            input.verify_stdin.as_deref(),
        );
        invocation::run(&inv, &mut report)?;
    }

    Ok(report)
}

/// How `verify` presents its results (Improvement B). The human format is the
/// default; `json` is for CI gating / scripting and uses the machine-readable
/// `check_id`s already on each [`CheckResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
    Sarif,
    /// GitHub Actions workflow commands: one `::error`/`::warning` annotation
    /// per failed/warned check, so CI failures surface inline on the PR diff.
    Github,
}

/// Pretty-print a report as the human-facing output (design §5.2 step 4).
/// Returns a single string the CLI writes to stdout.
pub fn render(report: &VerifyReport) -> String {
    use self::result::Severity;
    let mut out = String::new();
    let (pass, warn, fail, _skip) = report.counts();
    for r in &report.results {
        let glyph = match r.severity {
            Severity::Pass => "✓",
            Severity::Warn => "!",
            Severity::Error => "✗",
            Severity::Skipped => "·",
        };
        out.push_str(&format!(
            "{} {}: {}\n",
            glyph,
            r.severity.as_str(),
            r.check_name
        ));
        if !r.message.is_empty() {
            out.push_str(&format!("    {}\n", r.message));
        }
        if let Some(s) = &r.suggestion {
            out.push_str(&format!("    {s}\n"));
        }
    }

    out.push_str(&format!(
        "\n{pass} passed, {warn} warning(s), {fail} failed, discoverability score {}/100",
        report.discoverability_score()
    ));
    out.push_str(if fail > 0 {
        ": verify FAILED\n"
    } else {
        ": verify OK\n"
    });
    out
}

/// Render the report as a stable JSON object for CI / scripting. Shape:
/// `{ "ok": bool, "discoverability_score": u8, "counts": {pass,warn,fail,skip},
/// "results": [ {check_id, check_name, severity, message, suggestion?,
/// location?} ... ] }`. The score weights Pass=1.0, Warn=0.5, Error=0;
/// Skipped excluded from the denominator.
pub fn render_json(report: &VerifyReport) -> String {
    let (pass, warn, fail, skip) = report.counts();
    let results: Vec<_> = report
        .results
        .iter()
        .map(|r| {
            let mut o = serde_json::json!({
                "check_id": r.check_id,
                "check_name": r.check_name,
                "severity": r.severity.as_str(),
                "message": r.message,
            });
            if let Some(s) = &r.suggestion {
                o["suggestion"] = serde_json::Value::String(s.clone());
            }
            if let Some((file, line)) = &r.location {
                let mut loc = serde_json::Map::new();
                loc.insert("file".to_string(), serde_json::Value::String(file.clone()));
                if let Some(n) = line {
                    loc.insert("line".to_string(), serde_json::Value::from(*n));
                }
                o["location"] = serde_json::Value::Object(loc);
            }
            o
        })
        .collect();
    let body = serde_json::json!({
        "ok": !report.has_critical_failure(),
        "discoverability_score": report.discoverability_score(),
        "counts": {
            "pass": pass,
            "warn": warn,
            "fail": fail,
            "skip": skip,
        },
        "results": results,
    });
    serde_json::to_string_pretty(&body).expect("verify report serializes to JSON")
}

/// Render the report as GitHub Actions workflow commands (`::error` /
/// `::warning`). Emitted to stdout so a CI step like
/// `skillpack verify --format github` annotates the PR diff inline. Each
/// `Error` maps to `::error`, each `Warn` to `::warning`; `file`/`line` are
/// threaded from the result's `location` (absent when the check has no file
/// location). Newlines in messages are flattened to spaces — a workflow
/// command is a single line.
pub fn render_github_annotations(report: &VerifyReport) -> String {
    use self::result::Severity;

    let mut out = String::new();
    for r in &report.results {
        let kind = match r.severity {
            Severity::Error => "error",
            Severity::Warn => "warning",
            _ => continue,
        };
        // Build the property list as `key=value` pairs joined by commas, then
        // escape the message. A workflow command is a single line: `::kind
        // key=value,key=value::message`. The old code emitted a bare leading
        // comma when a result had no file location (`::error,title=...`),
        // which is malformed; properties are now only emitted when present.
        let mut props: Vec<String> = Vec::new();
        if let Some((file, line)) = &r.location {
            props.push(format!("file={}", gh_escape(file)));
            if let Some(n) = line {
                props.push(format!("line={n}"));
            }
        }
        // title is the human check label; the machine check_id stays available
        // in the JSON/SARIF formats for pipelines that need it.
        props.push(format!("title={}", gh_escape(&r.check_name)));

        // Flatten newlines to spaces and %-escape the message so a raw `%` or
        // embedded control char can't break the command grammar.
        let mut message = r.message.replace(['\r', '\n'], " ");
        if let Some(s) = &r.suggestion {
            message.push(' ');
            message.push_str(&s.replace(['\r', '\n'], " "));
        }
        message = gh_escape(&message);

        out.push_str(&format!("::{kind} {}::{message}\n", props.join(",")));
    }
    out
}

/// Escape a value for a GitHub workflow command (properties and message).
/// Per the Actions toolkit spec, `%`, `\r` and `\n` always need escaping, and
/// `:`/`,` additionally need escaping inside a property *value* (they are the
/// key/value and property separators). Escaping them in the message too is
/// harmless (GitHub decodes uniformly).
fn gh_escape(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace(':', "%3A")
        .replace(',', "%2C")
}

/// Render the report as SARIF 2.1.0 for GitHub Code Scanning upload-sarif.
/// Only `Warn` and `Error` results are emitted (SARIF reports failures, not
/// passes). Each result maps `ruleId` → `check_id`, `level` → `"warning"` or
/// `"error"`, `message` → CheckResult message, `locations` → file + line
/// when available. Pass/Skipped results are omitted.
pub fn render_sarif(report: &VerifyReport) -> String {
    use self::result::Severity;

    let results: Vec<_> = report
        .results
        .iter()
        .filter(|r| matches!(r.severity, Severity::Warn | Severity::Error))
        .map(|r| {
            let level = match r.severity {
                Severity::Warn => "warning",
                Severity::Error => "error",
                _ => "none",
            };
            let mut result = serde_json::json!({
                "ruleId": r.check_id,
                "level": level,
                "message": { "text": r.message },
            });

            // suggestion → rule metadata, appended to the message.
            if let Some(s) = &r.suggestion {
                result["message"]["text"] =
                    serde_json::Value::String(format!("{}\nSuggestion: {s}", r.message));
            }

            if let Some((file, line)) = &r.location {
                let mut region = serde_json::Map::new();
                if let Some(n) = line {
                    region.insert("startLine".to_string(), serde_json::Value::from(*n));
                }
                let mut phys_loc = serde_json::json!({
                    "artifactLocation": { "uri": file }
                });
                if !region.is_empty() {
                    phys_loc["region"] = serde_json::Value::Object(region);
                }
                result["locations"] = serde_json::json!([phys_loc]);
            }

            result
        })
        .collect();

    let body = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "skillpack",
                    "informationUri": "https://github.com/nordicnode/skillpack"
                }
            },
            "results": results
        }]
    });

    serde_json::to_string_pretty(&body).expect("verify report serializes to SARIF JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::result::{CheckResult, Severity};

    fn warn_with_location(loc: Option<(String, Option<usize>)>) -> CheckResult {
        CheckResult {
            check_id: "discovery.skill.when_to_use".to_string(),
            check_name: "SKILL.md has non-empty `when_to_use` trigger phrases".to_string(),
            severity: Severity::Warn,
            message: "when_to_use is missing".to_string(),
            suggestion: Some("list 2-5 trigger verbs".to_string()),
            location: loc,
        }
    }

    #[test]
    fn github_annotations_have_no_leading_comma_without_location() {
        let report = VerifyReport {
            results: vec![warn_with_location(None)],
        };
        let out = render_github_annotations(&report);
        assert!(
            !out.contains("::warning,"),
            "must not emit a leading comma before properties, got: {out}"
        );
        assert!(
            out.starts_with("::warning title="),
            "title should be the first property, got: {out}"
        );
    }

    #[test]
    fn github_annotations_include_file_and_line_when_present() {
        let report = VerifyReport {
            results: vec![warn_with_location(Some((
                "skills/foo/SKILL.md".to_string(),
                Some(3),
            )))],
        };
        let out = render_github_annotations(&report);
        assert!(
            out.contains("file=skills/foo/SKILL.md,line=3"),
            "got: {out}"
        );
    }

    #[test]
    fn github_annotations_escape_percent_and_flatten_newlines() {
        let mut r = warn_with_location(None);
        r.message = "100% broken\nsecond line".to_string();
        let report = VerifyReport { results: vec![r] };
        let out = render_github_annotations(&report);
        assert!(
            !out.contains("100% broken"),
            "raw % must be escaped, got: {out}"
        );
        assert!(out.contains("100%25"), "got: {out}");
        assert_eq!(
            out.matches('\n').count(),
            1,
            "only the line terminator may remain, got: {out:?}"
        );
    }
}
