//! `skillpack` entry point. Dispatches to `init` or `verify`.
//!
//! The `init` flow (design §5.1 + §5.3) is the load-bearing piece:
//!   1. introspect the repo
//!   2. interview the user (or reuse `skillpack.toml`)
//!   3. render the three files *in memory*
//!   4. run `verify` against that in-memory output (the pre-commit gate)
//!   5. if critical checks fail → report, do NOT write, exit `INIT_FIXABLE`
//!      unless the user explicitly confirms; warnings are advisory
//!   6. write the files + save `skillpack.toml` once cleared

use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;

use skillpack::cli::{Cli, Commands};
use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::{coerce_kebab, render_targets, GeneratedFileOutput};
use skillpack::interview;
use skillpack::introspect;
use skillpack::types;
use skillpack::verify::{self, VerifyInput, VerifyReport};

fn main() {
    let cli = Cli::parse();

    let code = if let Ok(code) = std::panic::catch_unwind(|| match cli.command {
        Commands::Init {
            root,
            non_interactive,
            accept_warnings,
            license,
            target,
        } => run_init(
            &root,
            cli.verbose,
            cli.debug,
            non_interactive,
            accept_warnings,
            license,
            target,
        ),
        Commands::Verify { root, format, fix } => {
            run_verify(&root, cli.verbose, cli.debug, format, fix)
        }
        Commands::Doctor { root, format } => run_doctor(&root, cli.verbose, cli.debug, format),
    }) {
        code
    } else {
        eprintln!("fatal: skillpack crashed (panic)");
        std::process::exit(exit::INIT_FATAL)
    };
    std::process::exit(code);
}

fn run_init(
    root: &Path,
    verbose: bool,
    debug: bool,
    non_interactive: bool,
    accept_warnings: bool,
    license_override: Option<String>,
    targets: Vec<skillpack::cli::Target>,
) -> i32 {
    match run_init_inner(
        root,
        verbose,
        debug,
        non_interactive,
        accept_warnings,
        license_override,
        targets,
    ) {
        Ok(c) => c,
        Err(e) => {
            // teach, don't just complain (design §8.1).
            eprintln!("fatal: {e:#}");
            std::process::exit(exit::INIT_FATAL);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_init_inner(
    root: &Path,
    verbose: bool,
    debug: bool,
    non_interactive: bool,
    accept_warnings: bool,
    license_override: Option<String>,
    targets: Vec<skillpack::cli::Target>,
) -> Result<i32> {
    let profile = introspect::introspect(root).context("introspecting repo")?;
    if verbose {
        print_profile(&profile);
    }
    if debug {
        eprintln!(
            "[debug] detected name={} language={} has_cli={}",
            profile.name,
            profile.language.as_str(),
            profile.has_cli
        );
    }

    // Step 2 — interview or reuse config.
    let existing_cfg = Config::load(root)?;
    let intent = if non_interactive {
        let Some(cfg) = &existing_cfg else {
            bail!(
                "--non-interactive set but no skillpack.toml found at {}.\n\
                 To fix: run `skillpack init` once interactively to seed skillpack.toml, \
                 then retry in CI.",
                Config::path(root).display()
            );
        };
        match cfg.to_intent() {
            Some(i) => i,
            None => bail!(
                "skillpack.toml at {} is missing its [skill] table.\n\
                 To fix: re-run `skillpack init` interactively.",
                Config::path(root).display()
            ),
        }
    } else if let Some(cfg) = &existing_cfg {
        if let Some(i) = cfg.to_intent() {
            i
        } else {
            interview_run(&profile)?
        }
    } else {
        interview_run(&profile)?
    };

    let mut intent = intent;
    if let Some(ref lic) = license_override {
        intent.license = Some(lic.clone());
    }

    // Step 3 — render in memory.
    let targets = if targets.is_empty() {
        vec![skillpack::cli::Target::Claude]
    } else {
        targets
    };
    let files =
        render_targets(&profile, &intent, &targets).context("rendering distribution files")?;

    // Step 4 — pre-commit verify against the rendered output (design §5.3).
    let report = verify_rendered(&files, &profile, root, debug)?;

    if report.has_critical_failure() {
        eprintln!("\n❌ pre-commit verification FAILED. skillpack will NOT write files.");
        eprintln!("{}", verify::render(&report));
        if non_interactive {
            eprintln!(
                "Critical checks failed in --non-interactive mode; refusing to write. \
                 Fix the issues above and re-run."
            );
            return Ok(exit::INIT_FIXABLE);
        }
        let proceed = CONFIRM.keep_anyway();
        if !proceed {
            eprintln!("Aborted. No files written.");
            // A fixable problem the user chose to address rather than ship a
            // broken pack — exit code 2, not the clean-abort 1 (design §8.1:
            // "2 if a fixable problem occurred"). INIT_ABORTED is reserved for
            // a user declining with no underlying verify problem.
            return Ok(exit::INIT_FIXABLE);
        }
        // User chose to keep — fall through to write.
    } else {
        let (_pass, warn, _fail, _skip) = report.counts();
        // Warnings are advisory but NOT silently ignored: in interactive mode
        // without --accept-warnings, we print them and ask the user to proceed
        // (matching the --help doc: "Without this flag, any non-pass result
        // prompts the user"). In --non-interactive mode warnings never block —
        // CI runs are gated on criticals only ("critical still blocks").
        if warn > 0 {
            eprintln!("\n⚠ verification passed with warnings:");
            eprintln!("{}", verify::render(&report));
            if !accept_warnings && !non_interactive {
                let proceed = CONFIRM.proceed_with_warnings();
                if !proceed {
                    eprintln!("Aborted. No files written.");
                    return Ok(exit::INIT_ABORTED);
                }
            } else if non_interactive {
                eprintln!(
                    "Written in --non-interactive mode (warnings are advisory; \
                     use --accept-warnings to suppress this notice)."
                );
            }
        }
    }

    // Step 5 — write files + save config.
    write_files(root, &files)?;
    let name = coerce_kebab(&profile.name);
    Config::from_intent(&name, &intent).save(root)?;
    println!("✓ wrote {} file(s) under {}:", files.len(), root.display());
    for f in &files {
        println!("   - {}", f.rel_path);
    }
    println!("   - {}", Config::path(root).display());
    Ok(exit::INIT_OK)
}

fn interview_run(profile: &types::ProjectProfile) -> Result<types::Intent> {
    println!("\nNo skillpack.toml found — a few quick questions to scaffold your skill pack.\n");
    let prompter = interview::DialoguerPrompter;
    let intent = interview::run(profile, &prompter).context("during interview")?;
    Ok(intent)
}

fn verify_rendered(
    files: &[GeneratedFileOutput],
    profile: &types::ProjectProfile,
    root: &Path,
    debug: bool,
) -> Result<VerifyReport> {
    // Materialize the rendered files into a temp dir so verify (which expects
    // files on disk) can read them exactly as an agent coming in cold would.
    let tmp = tempfile::tempdir().context("creating temp dir for pre-commit verify")?;
    for f in files {
        let p = tmp.path().join(&f.rel_path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&p, &f.contents).with_context(|| format!("writing {}", p.display()))?;
    }
    let input = VerifyInput {
        // Discovery reads the rendered files from the temp dir (we verify the
        // ACTUAL files we're about to ship — design §5.3).
        root: tmp.path().to_path_buf(),
        // But the documented CLI runs in the real project root, where the
        // source tree / built artifact lives — spawning from the temp dir
        // (which holds only the rendered files) would false-fail any
        // relative-invocation CLI like `go run .` or `node ./bin/cli.js`.
        spawn_root: root.to_path_buf(),
        cli_command: profile.cli_command.clone(),
        repo_url: profile.repo_url.clone(),
        debug,
    };
    verify::run(&input)
}

fn write_files(root: &Path, files: &[GeneratedFileOutput]) -> Result<()> {
    for f in files {
        let p = root.join(&f.rel_path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&p, &f.contents).with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(())
}

// --- pre-commit confirmation (Improvement E: testable) ---------------------
//
// The critical-failure and warnings gates both want a yes/no prompt. The
// interview already pulled in `dialoguer`, but re-attaching to a TTY mid-run
// is finicky in tests, so the pre-commit gate uses a bare stdin readline.
// Wrapping it behind a trait + a thread-local override lets tests inject a
// canned answer instead of driving a real TTY (mirrors interview::Prompter).

trait Confirm {
    fn confirm(&self, prompt: &str) -> bool;

    /// Pre-commit gate: critical failures, "write anyway?" (defaults to NO).
    fn keep_anyway(&self) -> bool {
        self.confirm(&prompt_keep_anyway_text())
    }

    /// Pre-commit gate: warnings present, "proceed?" (defaults to NO).
    fn proceed_with_warnings(&self) -> bool {
        self.confirm(
            "Verification passed with warnings (see above). \
             Write the files? [y/N] ",
        )
    }
}

/// Read one line from stdin; `y`/`yes` (any case) → true, anything else → false.
struct StdinConfirm;
impl Confirm for StdinConfirm {
    fn confirm(&self, prompt: &str) -> bool {
        use std::io::{self, Write};
        let mut input = String::new();
        print!("{prompt}");
        let _ = io::stdout().flush();
        if io::stdin().read_line(&mut input).is_err() {
            return false;
        }
        matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
    }
}

thread_local! {
    /// Tests override this with a canned answer; production leaves the default
    /// `StdinConfirm`.
    static CONFIRM_REF: std::cell::RefCell<Box<dyn Confirm>> =
        std::cell::RefCell::new(Box::new(StdinConfirm));
}

#[cfg(test)]
struct ConfirmGuard;

#[cfg(test)]
impl Drop for ConfirmGuard {
    fn drop(&mut self) {
        // Restore the default on scope exit so a test's override can't leak to
        // a sibling test (Rust runs unit tests in threads, but a thread-local
        // is still restored here for cleanliness).
        CONFIRM_REF.with(|c| c.replace(Box::new(StdinConfirm)));
    }
}

/// The dispatch point the pre-commit gates call. Routes through the
/// (possibly test-overridden) confirm backend.
struct ConfirmDispatch;
impl Confirm for ConfirmDispatch {
    fn confirm(&self, prompt: &str) -> bool {
        CONFIRM_REF.with(|c| c.borrow().confirm(prompt))
    }
}
static CONFIRM: ConfirmDispatch = ConfirmDispatch;

fn prompt_keep_anyway_text() -> String {
    "Critical verification failures were found (see above).\n\
     Write the files anyway? [y/N] "
        .to_string()
}

/// Canned yes/no for tests. Lives at module scope so it can be boxed behind
/// the `Confirm` trait object before its definition point in `with_confirm`.
#[cfg(test)]
struct CannedConfirm(bool);
#[cfg(test)]
impl Confirm for CannedConfirm {
    fn confirm(&self, _p: &str) -> bool {
        self.0
    }
}

/// Run `f` with confirmations overridden so every prompt answers `answer`.
/// Returns `f()`'s result. Test-only: the override is restored on drop.
#[cfg(test)]
pub(crate) fn with_confirm<R>(answer: bool, f: impl FnOnce() -> R) -> R {
    CONFIRM_REF.with(|c| c.replace(Box::new(CannedConfirm(answer))));
    let _g = ConfirmGuard;
    f()
}

fn print_profile(profile: &types::ProjectProfile) {
    println!("— introspection —");
    println!("  name:        {}", profile.name);
    println!("  language:    {}", profile.language.as_str());
    println!("  has_cli:     {}", profile.has_cli);
    if let Some(cmd) = &profile.cli_command {
        println!("  cli_command: {}", cmd.join(" "));
    }
    if let Some(url) = &profile.repo_url {
        println!("  repo_url:    {url}");
    }
    if let Some(lic) = &profile.license {
        println!("  license:     {lic}");
    }
    if let Some(hint) = &profile.description_hint {
        if hint.chars().count() > 120 {
            println!(
                "  desc_hint:   {}…",
                hint.chars().take(120).collect::<String>()
            );
        } else {
            println!("  desc_hint:   {hint}");
        }
    }
}
fn run_verify(
    root: &Path,
    verbose: bool,
    debug: bool,
    format: verify::OutputFormat,
    fix: bool,
) -> i32 {
    match run_verify_inner(root, verbose, debug, format, fix) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            std::process::exit(exit::INIT_FATAL);
        }
    }
}

fn run_verify_inner(
    root: &Path,
    verbose: bool,
    debug: bool,
    format: verify::OutputFormat,
    fix: bool,
) -> Result<i32> {
    // Defer to introspect only to recover has_cli + cli_command for the
    // *spawn* stage. CLI *presence* is now derived from the SKILL.md itself
    // (a hand-written pack with a documented invocation should be checked
    // even if no source tree is present — Bug 2 / design §4.2); introspect's
    // `cli_command` only decides whether we can actually spawn `--help` here.
    // If the skill documents a CLI but introspect found none, `verify` emits
    // a warning (not a silent skip) so the gap is visible.
    let profile = introspect::introspect(root).context("introspecting repo for verify")?;
    if verbose {
        print_profile(&profile);
    }
    let render = |report: &verify::VerifyReport| match format {
        verify::OutputFormat::Human => verify::render(report),
        verify::OutputFormat::Json => format!("{}\n", verify::render_json(report)),
    };
    let run_verify = || -> Result<verify::VerifyReport> {
        let input = VerifyInput {
            root: root.to_path_buf(),
            spawn_root: root.to_path_buf(),
            cli_command: profile.cli_command.clone(),
            debug,
            repo_url: profile.repo_url.clone(),
        };
        verify::run(&input)
    };

    let report = run_verify()?;
    // Without `--fix`, render + exit on the single report. With `--fix`,
    // collect the mechanically-fixable drifts (warn OR error severities),
    // apply each, then re-render from the post-fix report. The pre-fix
    // report is NOT printed when `--fix` takes effect — the post-fix report
    // surfaces what (if anything) still drifts, plus a one-line summary of
    // the files rewritten.
    let (final_report, applied_summary) = if !fix {
        (report, None)
    } else {
        let actions: Vec<verify::fix::FixAction> = report
            .results
            .iter()
            .filter(|r| {
                matches!(
                    r.severity,
                    verify::result::Severity::Warn | verify::result::Severity::Error
                )
            })
            .filter_map(|r| verify::fix::action_for(&r.check_id))
            .collect();
        if actions.is_empty() {
            (report, None)
        } else {
            let mut written: Vec<String> = Vec::new();
            for action in actions {
                let outcome =
                    verify::fix::apply(action, root).context("applying a `--fix` action")?;
                written.extend(outcome.files_written);
            }
            let summary: Vec<String> = verify::fix::FixOutcome {
                files_written: written,
            }
            .unique_sorted();
            let summary_line = format!(
                "✓ applied {} fix(es), wrote: {}",
                summary.len(),
                summary.join(", ")
            );
            (run_verify()?, Some(summary_line))
        }
    };

    if let Some(line) = applied_summary {
        println!("{line}");
    }
    print!("{}", render(&final_report));
    Ok(if final_report.has_critical_failure() {
        exit::VERIFY_FAIL
    } else {
        exit::VERIFY_OK
    })
}
/// `skillpack doctor` — diagnose why introspection chose what it did.
/// Read-only: prints the detected profile + the decision trace (`diag`),
/// never writes files. The trace is empty until candidate fns push notes
/// (the `detect_*` falsy branches); doctor surfaces exactly why `has_cli`
/// came out false so the maintainer can act.
fn run_doctor(root: &Path, verbose: bool, debug: bool, format: crate::verify::OutputFormat) -> i32 {
    match run_doctor_inner(root, verbose, debug, format) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            std::process::exit(exit::INIT_FATAL);
        }
    }
}

fn run_doctor_inner(
    root: &Path,
    verbose: bool,
    debug: bool,
    format: crate::verify::OutputFormat,
) -> Result<i32> {
    let profile = introspect::introspect(root).context("introspecting repo for doctor")?;

    match format {
        crate::verify::OutputFormat::Json => {
            // The serialized `ProjectProfile` IS the doctor JSON report —
            // including the `diag` decision trace + every detected field,
            // exactly what a consumer wants to scrape. No envelope wrapping;
            // the consumer reads fields by name. Exits 0 (doctor is
            // read-only diagnostic, non-gating — matches human form).
            println!(
                "{}",
                serde_json::to_string_pretty(&profile)
                    .context("serializing doctor profile to JSON")?
            );
        }
        crate::verify::OutputFormat::Human => render_doctor_human(&profile, verbose, debug),
    }

    // Doctor never writes; always exits 0.
    Ok(exit::VERIFY_OK)
}

/// Render the human-facing diagnosis. Lifted verbatim from the pre-format
/// behavior so `doctor` (no flag) and `doctor --format human` are byte-identical.
fn render_doctor_human(profile: &types::ProjectProfile, verbose: bool, debug: bool) {
    if debug {
        eprintln!(
            "[debug] detected name={} language={} has_cli={} diag_notes={}",
            profile.name,
            profile.language.as_str(),
            profile.has_cli,
            profile.diag.0.len()
        );
    }
    // Reuse the same profile block --verbose prints so doctor's output starts
    // from a known place.
    if verbose {
        print_profile(profile);
    } else {
        println!("— skillpack doctor —");
        println!("  name:     {}", profile.name);
        println!("  language: {}", profile.language.as_str());
        println!("  has_cli:  {}", profile.has_cli);
        if let Some(cmd) = &profile.cli_command {
            println!("  cli:      {}", cmd.join(" "));
        }
    }

    println!();
    if profile.diag.0.is_empty() {
        if profile.has_cli {
            println!("decision trace: (empty — CLI detected cleanly, no falsy branches fired)");
        } else {
            println!("decision trace: (empty — no candidate notes were pushed)");
            println!();
            println!("hint: candidate fns only push notes on falsy branches, so an empty trace");
            println!("      means either detection succeeded silently or this language has no");
            println!("      probed candidate. Check --verbose for the raw profile.");
        }
    } else {
        println!("decision trace ({}):", profile.diag.0.len());
        for note in &profile.diag.0 {
            println!("  [{}] {}", note.stage, note.note);
        }
    }
}

#[cfg(test)]
mod confirm_tests {
    use super::*;

    #[test]
    fn keep_anyway_routes_through_overridable_confirm() {
        // A canned "no" aborts; a canned "yes" proceeds. Both go through the
        // same CONFIRM dispatch the real pre-commit gate uses (Improvement E).
        assert!(!with_confirm(false, || CONFIRM.keep_anyway()));
        assert!(with_confirm(true, || CONFIRM.keep_anyway()));
    }

    #[test]
    fn proceed_with_warnings_routes_through_overridable_confirm() {
        assert!(!with_confirm(false, || CONFIRM.proceed_with_warnings()));
        assert!(with_confirm(true, || CONFIRM.proceed_with_warnings()));
    }

    // Regression: a README hint with a multibyte char across byte 120 must
    // not panic. The old `&hint[..120]` byte-slice hit "byte index 120 is
    // not a char boundary" → catch_unwind → false INIT_FATAL exit.
    #[test]
    fn print_profile_multibyte_desc_hint_does_not_panic() {
        // 118 ASCII chars + a 3-byte emoji = 121 bytes; byte 120 lands mid-char.
        let mut hint = "x".repeat(118);
        hint.push('🦀');
        let profile = types::ProjectProfile {
            name: "test".into(),
            language: types::Language::Rust,
            has_cli: false,
            cli_command: None,
            cli_help_output: None,
            cli_subcommand_help: Vec::new(),
            repo_url: None,
            license: Some("MIT".into()),
            version: None,
            authors: None,
            description_hint: Some(hint),
            diag: types::DiagTrace::default(),
        };
        // Must not panic.
        print_profile(&profile);
    }
}
