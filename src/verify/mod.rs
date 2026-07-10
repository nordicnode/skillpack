//! The `skillpack verify` subcommand: load the generated distribution files
//! and run discovery + invocation checks against them.
//!
//! Design §5.2. `verify` works even on hand-written plugin files (not just
//! `init` output) — see §4.2 — so the loader is tolerant of missing pieces and
//! each check degrades gracefully.

pub mod discovery;
pub mod invocation;
pub mod result;
pub mod schema;

use anyhow::Result;

use self::invocation::InvocationInput;
use self::result::CheckResult;

// Re-export the pieces the rest of the crate touches. `find_skill_file` is
// crate-visible (the dispatcher in main.rs is the only external caller).
pub(crate) use self::discovery::find_skill_file;
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
    /// Print every subprocess spawn to stderr (design §8.2 --debug).
    pub debug: bool,
}

/// Run the full verify suite against `root`, returning the aggregate report.
pub fn run(input: &VerifyInput) -> Result<VerifyReport> {
    let root = &input.root;
    let mut report = VerifyReport::default();

    // Discovery checks (pure, file reads only).
    for check in discovery::run(root)? {
        report.push(check);
    }

    // Invocation checks. Build the input from the SKILL.md we located. Note we
    // use the FIRST skill's text for the invocation spawn — the documented CLI
    // belongs to exactly one skill. Discovery above still checks all skills.
    let skill_path = find_skill_file(root);
    let skill_md = skill_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();

    // GAP #2: invocation only spawns the FIRST skill's documented CLI. If more
    // than one skill documents a CLI invocation, the other CLIs' drift is
    // silently unchecked — warn so the maintainer knows the check didn't cover
    // them (the multi-CLI limit is documented, but a silent skip is a cliff).
    let cli_skills = discovery::find_skill_files(root)
        .into_iter()
        .filter(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|s| invocation::extract_documented_invocation(&s))
                .is_some()
        })
        .count();
    if cli_skills > 1 {
        report.push(CheckResult::warn(
            "invocation.multi_cli",
            "invocation drift checks cover every documented CLI",
            format!(
                "{cli_skills} skills document a CLI invocation, but invocation checks only run against the first — the others were skipped"
            ),
            "To fix: verify is single-CLI by default; split multi-CLI plugins into one plugin per CLI, or run verify per-skill manually.",
        ));
    }

    let inv = InvocationInput::new(
        root,
        &input.spawn_root,
        &skill_md,
        input.cli_command.as_deref(),
        input.debug,
    );
    invocation::run(&inv, &mut report)?;

    Ok(report)
}

/// How `verify` presents its results (Improvement B). The human format is the
/// default; `json` is for CI gating / scripting and uses the machine-readable
/// `check_id`s already on each [`CheckResult`](self::result::CheckResult).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

/// Pretty-print a report as the human-facing output (design §5.2 step 4).
/// Returns a single string the CLI writes to stdout.
pub fn render(report: &VerifyReport) -> String {
    use self::result::Severity;
    let mut out = String::new();
    let (pass, warn, fail, skip) = report.counts();
    for r in &report.results {
        let glyph = match r.severity {
            Severity::Pass => "✓",
            Severity::Warn => "!",
            Severity::Error => "✗",
            Severity::Skipped => "·",
        };
        out.push_str(&format!(
            "{} {} — {}\n",
            glyph,
            r.severity.as_str(),
            r.check_name
        ));
        if r.severity != Severity::Pass && !r.message.is_empty() {
            out.push_str(&format!("    {}\n", r.message));
        }
        if let Some(s) = &r.suggestion {
            out.push_str(&format!("    {s}\n"));
        }
    }
    let _ = &pass;
    let _ = &skip;
    let _ = &warn;
    out.push_str(&format!(
        "\n{pass} passed, {warn} warning(s), {fail} failed"
    ));
    out.push_str(if fail > 0 {
        " — verify FAILED\n"
    } else {
        " — verify OK\n"
    });
    out
}

/// Render the report as a stable JSON object for CI / scripting. Shape:
/// `{ "ok": bool, "counts": {pass,warn,fail,skip}, "results": [ {check_id,
/// check_name, severity, message, suggestion?, location?} ... ] }`.
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
