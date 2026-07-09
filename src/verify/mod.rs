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
    pub has_cli: bool,
    pub cli_command: Option<Vec<String>>,
}

/// Run the full verify suite against `root`, returning the aggregate report.
pub fn run(input: &VerifyInput) -> Result<VerifyReport> {
    let root = &input.root;
    let mut report = VerifyReport::default();

    // Discovery checks (pure, file reads only).
    for check in discovery::run(root)? {
        report.push(check);
    }

    // Invocation checks. Build the input from the SKILL.md we located.
    let skill_path = find_skill_file(root);
    let skill_md = skill_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let inv = InvocationInput::new(
        root,
        &input.spawn_root,
        &skill_md,
        input.has_cli,
        input.cli_command.as_deref(),
    );
    invocation::run(&inv, &mut report)?;

    Ok(report)
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
