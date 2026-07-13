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
use clap::{Parser, ValueEnum};

use skillpack::cli::{resolve_targets, Cli, Commands, Target};
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
            force,
            template_dir,
        } => run_init(
            &root,
            cli.verbose,
            cli.debug,
            non_interactive,
            accept_warnings,
            license,
            target,
            force,
            template_dir.as_deref(),
        ),
        Commands::Verify {
            root,
            format,
            fix,
            min_score,
            watch,
            template_dir,
        } => run_verify(
            &root,
            cli.verbose,
            cli.debug,
            format,
            fix,
            min_score,
            watch,
            template_dir.as_deref(),
        ),
        Commands::Doctor { root, format } => run_doctor(&root, cli.verbose, cli.debug, format),
        Commands::Update {
            root,
            target,
            force,
            template_dir,
        } => run_update(
            &root,
            cli.verbose,
            cli.debug,
            target,
            force,
            template_dir.as_deref(),
        ),
        Commands::Diff {
            root,
            target,
            force,
            template_dir,
        } => run_diff(
            &root,
            cli.verbose,
            cli.debug,
            target,
            force,
            template_dir.as_deref(),
        ),
    }) {
        code
    } else {
        eprintln!("fatal: skillpack crashed (panic)");
        std::process::exit(exit::INIT_FATAL)
    };
    std::process::exit(code);
}

#[allow(clippy::too_many_arguments)]
fn run_init(
    root: &Path,
    verbose: bool,
    debug: bool,
    non_interactive: bool,
    accept_warnings: bool,
    license_override: Option<String>,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
) -> i32 {
    match run_init_inner(
        root,
        verbose,
        debug,
        non_interactive,
        accept_warnings,
        license_override,
        raw_targets,
        force,
        template_dir,
    ) {
        Ok(c) => c,
        Err(e) => {
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
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
) -> Result<i32> {
    let profile = introspect::introspect(root).context("introspecting repo")?;
    if verbose {
        print_profile(&profile, false);
    }

    // Resolve `--target all` + validate every value. Empty → `[Claude]`.
    let targets = if raw_targets.is_empty() {
        vec![Target::Claude]
    } else {
        resolve_targets(&raw_targets)?
    };
    if verbose {
        let names: Vec<String> = targets
            .iter()
            .map(|t| t.to_possible_value().unwrap().get_name().to_string())
            .collect();
        eprintln!("targets: {}", names.join(", "));
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
    let files = render_targets(&profile, &intent, &targets, template_dir)
        .context("rendering distribution files")?;

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

    // Step 4b — preview: which files are new, changed, or unchanged?
    print_diff_preview(root, &files);

    // Step 5 — write files + save config.
    let (written, skipped) = write_files(root, &files, force)?;
    let name = coerce_kebab(&profile.name);
    Config::from_intent(&name, &intent).save(root)?;
    println!(
        "✓ wrote {} file(s) under {}:",
        written.len(),
        root.display()
    );
    for f in &written {
        println!("   - {}", f.rel_path);
    }
    println!("   - {}", Config::path(root).display());
    // Surface any targets the collision guard skipped so the summary never
    // hides a user-requested target as silent success (design §8.2: "exit
    // 0 unless critical fail" — collision is not critical; the footer makes
    // the skip visible without changing the exit code).
    if !skipped.is_empty() {
        eprintln!(
            "ℹ skipped {} target file(s) (existing file held; pass --force to overwrite):",
            skipped.len()
        );
        for f in &skipped {
            eprintln!("   - {}", f.rel_path);
        }
    }
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
        profile_name: Some(coerce_kebab(&profile.name)),
        debug,
    };
    verify::run(&input)
}

fn write_files<'a>(
    root: &Path,
    files: &'a [GeneratedFileOutput],
    force: bool,
) -> Result<(Vec<&'a GeneratedFileOutput>, Vec<&'a GeneratedFileOutput>)> {
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    for f in files {
        let p = root.join(&f.rel_path);
        // Collision guard: AGENTS.md lives at repo root (not a skillpack-owned
        // directory). If it already exists and --force was not passed, skip it
        // with a warning so we never silently stomp a hand-written file.
        if f.rel_path == crate::verify::schema::AGENTS_MD_PATH && p.exists() && !force {
            eprintln!(
                "⚠ AGENTS.md already exists at {}; skipping (pass --force to overwrite).",
                p.display()
            );
            skipped.push(f);
            continue;
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&p, &f.contents).with_context(|| format!("writing {}", p.display()))?;
        written.push(f);
    }
    Ok((written, skipped))
}

/// Print a preview of which files are new, changed, or unchanged before
/// writing. Only prints when at least one file differs from disk — a
/// fully-clean re-init prints nothing (no noise).
fn print_diff_preview(root: &Path, files: &[GeneratedFileOutput]) {
    let mut new = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = 0u32;
    for f in files {
        let p = root.join(&f.rel_path);
        match std::fs::read_to_string(&p) {
            Ok(existing) if existing == f.contents => unchanged += 1,
            Ok(_) => changed.push(&f.rel_path),
            Err(_) => new.push(&f.rel_path),
        }
    }
    if new.is_empty() && changed.is_empty() {
        return;
    }
    eprintln!("\n📝 distribution file preview:");
    for r in &new {
        eprintln!("   + {r} (new)");
    }
    for r in &changed {
        eprintln!("   ~ {r} (changed)");
    }
    if unchanged > 0 {
        eprintln!("   = {unchanged} file(s) unchanged");
    }
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

fn print_profile(profile: &types::ProjectProfile, to_stderr: bool) {
    // `to_stderr` lets `verify --verbose --format json` show the
    // introspection block without corrupting the JSON body on stdout
    // (stdout stays parseable for `jq`-style CI pipelines).
    macro_rules! emit {
        ($($arg:tt)*) => {
            if to_stderr {
                eprintln!($($arg)*);
            } else {
                println!($($arg)*);
            }
        };
    }
    emit!("— introspection —");
    emit!("  name:        {}", profile.name);
    emit!("  language:    {}", profile.language.as_str());
    emit!("  has_cli:     {}", profile.has_cli);
    if let Some(cmd) = &profile.cli_command {
        emit!("  cli_command: {}", cmd.join(" "));
    }
    if let Some(url) = &profile.repo_url {
        emit!("  repo_url:    {url}");
    }
    if let Some(lic) = &profile.license {
        emit!("  license:     {lic}");
    }
    if let Some(hint) = &profile.description_hint {
        if hint.chars().count() > 120 {
            emit!(
                "  desc_hint:   {}…",
                hint.chars().take(120).collect::<String>()
            );
        } else {
            emit!("  desc_hint:   {hint}");
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn run_verify(
    root: &Path,
    verbose: bool,
    debug: bool,
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    watch: bool,
    template_dir: Option<&Path>,
) -> i32 {
    if watch {
        if format != verify::OutputFormat::Human {
            eprintln!("error: --watch is only valid with --format human");
            return exit::VERIFY_USAGE;
        }
        return run_verify_watch(root, verbose, debug, format, fix, min_score, template_dir);
    }
    match run_verify_inner(root, verbose, debug, format, fix, min_score, template_dir) {
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
    min_score: Option<u8>,
    template_dir: Option<&Path>,
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
        print_profile(
            &profile,
            matches!(
                format,
                verify::OutputFormat::Json | verify::OutputFormat::Sarif
            ),
        );
    }
    let render = |report: &verify::VerifyReport| match format {
        verify::OutputFormat::Human => verify::render(report),
        verify::OutputFormat::Json => format!("{}\n", verify::render_json(report)),
        verify::OutputFormat::Sarif => format!("{}\n", verify::render_sarif(report)),
    };
    let run_verify = || -> Result<verify::VerifyReport> {
        let input = VerifyInput {
            root: root.to_path_buf(),
            spawn_root: root.to_path_buf(),
            cli_command: profile.cli_command.clone(),
            profile_name: Some(coerce_kebab(&profile.name)),
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
        let actions: Vec<_> = report
            .results
            .iter()
            .filter(|r| {
                matches!(
                    r.severity,
                    verify::result::Severity::Warn | verify::result::Severity::Error
                )
            })
            .filter_map(|r| verify::fix::action_for(&r.check_id).map(|a| (a, r.location.clone())))
            .collect();
        if actions.is_empty() {
            (report, None)
        } else {
            let mut written: Vec<String> = Vec::new();
            for (action, loc) in actions {
                let outcome = verify::fix::apply(action, root, loc.as_ref(), template_dir)
                    .context("applying a `--fix` action")?;
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
        eprintln!("{line}");
    }
    print!("{}", render(&final_report));
    // Exit precedence: critical failure (1) > score-below-min (2) > ok (0).
    // A structurally broken pack is more severe than a low score and must
    // surface first; the score gate fires only when structure passed.
    let code = if final_report.has_critical_failure() {
        exit::VERIFY_FAIL
    } else if let Some(min) = min_score {
        let actual = final_report.discoverability_score();
        if actual < min {
            eprintln!(
                "verify: discoverability score {actual} is below the --min-score {min} threshold"
            );
            exit::VERIFY_SCORE_BELOW_MIN
        } else {
            exit::VERIFY_OK
        }
    } else {
        exit::VERIFY_OK
    };
    Ok(code)
}

/// `verify --watch` — re-runs verify on every file change (debounced).
///
/// Uses `notify` to watch the project root. On each debounced event batch,
/// clears the terminal, re-runs a single verify cycle, and prints the
/// report. Ctrl-C terminates the process directly (standard SIGINT
/// behavior — no clean-shutdown handler is installed).
fn run_verify_watch(
    root: &Path,
    verbose: bool,
    debug: bool,
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    template_dir: Option<&Path>,
) -> i32 {
    use notify::{EventKind, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("fatal: cannot initialize file watcher: {e}");
            return exit::INIT_FATAL;
        }
    };

    // Watch the project root recursively. Ignore common noise dirs.
    if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
        eprintln!("fatal: cannot watch {}: {e}", root.display());
        return exit::INIT_FATAL;
    }

    eprintln!(
        "🔍 watching {} for changes (Ctrl-C to stop)…\n",
        root.display()
    );

    let _ = run_verify_single(root, verbose, debug, format, fix, min_score, template_dir);

    let debounce = Duration::from_secs(1);
    let mut last_event: Option<Instant> = None;

    // Skip events from noisy paths (target/, .git/, node_modules/).
    let is_noise = |path: &std::path::Path| -> bool {
        path.components().any(|c| {
            matches!(
                c,
                std::path::Component::Normal(s)
                    if s == "target" || s == ".git" || s == "node_modules"
            )
        })
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ok(event)) => {
                // Only react to content changes, not attribute-only.
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) && !event.paths.iter().all(|p| is_noise(p))
                {
                    last_event = Some(Instant::now());
                }
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {
                // Debounce: fire when 1s has elapsed since the last event
                // with no new events.
                if let Some(t) = last_event {
                    if t.elapsed() >= debounce {
                        last_event = None;
                        // Clear screen for a clean re-render.
                        print!("\x1b[2J\x1b[H");
                        let _ = run_verify_single(
                            root,
                            verbose,
                            debug,
                            format,
                            fix,
                            min_score,
                            template_dir,
                        );
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Channel closed (watcher dropped) — exit.
                break;
            }
        }
    }

    eprintln!("\nstopped.");
    exit::VERIFY_OK
}

/// Run a single verify cycle and print the report. Extracted from
/// `run_verify_watch` so it's testable independently of the watcher.
fn run_verify_single(
    root: &Path,
    verbose: bool,
    debug: bool,
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    template_dir: Option<&Path>,
) -> i32 {
    match run_verify_inner(root, verbose, debug, format, fix, min_score, template_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            exit::VERIFY_FAIL
        }
    }
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
        crate::verify::OutputFormat::Sarif => {
            bail!("doctor does not support SARIF output; use `verify --format sarif`")
        }
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
        print_profile(profile, false);
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
            if note.note.contains("run `") {
                println!("  💡 [{}] {}", note.stage, note.note);
            } else {
                println!("  [{}] {}", note.stage, note.note);
            }
        }
    }

    // Discoverability category preview: what `verify` would check, grouped
    // by namespace. doctor is read-only and runs on pre-init repos (no pack
    // generated yet), so we can't run the real verify — but we can show the
    // check-id namespaces so the user knows what to expect after `init`.
    println!();
    println!("verify category preview (run `skillpack verify` after `init` for the real score):");
    println!("  discovery.*     — structural validation of generated files per ecosystem");
    println!("    (marketplace.json, plugin.json, SKILL.md frontmatter, .mdc, AGENTS.md");
    println!("     presence, copilot-instructions.md)");
    if profile.has_cli {
        println!("  invocation.*    — runs the CLI: --help, flag drift, subcommand drift");
        println!("    --version drift (advisory)");
    } else {
        println!("  invocation.*    — N/A (no CLI detected; checks will be skipped)");
    }
}

/// `skillpack update` — incrementally regenerate distribution files from an
/// existing `skillpack.toml`. No interview, no pre-commit verify gate. Reads
/// the committed config, re-introspects, re-renders every target, and writes
/// ONLY files whose content changed. For frontmatter-bearing files the body
/// is preserved via the same splice `--fix` uses; frontmatter is regenerated
/// wholesale. Returns exit 0 on success.
fn run_update(
    root: &Path,
    _verbose: bool,
    _debug: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
) -> i32 {
    match run_update_inner(root, raw_targets, force, template_dir) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

/// Result of comparing one rendered file against its on-disk content.
struct CandidateResult<'a> {
    file: &'a GeneratedFileOutput,
    /// What we would write (spliced frontmatter + preserved body for
    /// frontmatter files; raw render for fully-generated files).
    candidate: String,
    /// On-disk content (BOM-stripped, CRLF-normalized).
    committed: Option<String>,
    /// None = file not on disk (new). Some = file exists.
    status: CandidateStatus,
    /// True if the AGENTS.md collision guard skipped this file.
    held: bool,
}

#[derive(PartialEq, Eq)]
enum CandidateStatus {
    /// File not on disk — would be created.
    Missing,
    /// Committed == candidate — no drift.
    Clean,
    /// Committed != candidate — drift detected.
    Drifted,
}

/// Compute candidates for each rendered file, comparing against on-disk
/// content. Shared by `update` (writes drifted) and `diff` (reports only).
/// The AGENTS.md collision guard mirrors `init`: skip if it exists and
/// `--force` is not passed.
fn compute_candidates<'f>(
    root: &Path,
    files: &'f [GeneratedFileOutput],
    force: bool,
) -> Result<Vec<CandidateResult<'f>>> {
    let mut results = Vec::with_capacity(files.len());
    for file in files {
        let disk_path = root.join(&file.rel_path);

        // AGENTS.md collision guard.
        if file.rel_path == "AGENTS.md" && disk_path.exists() && !force {
            results.push(CandidateResult {
                file,
                candidate: file.contents.clone(),
                committed: None,
                status: CandidateStatus::Clean,
                held: true,
            });
            continue;
        }

        if !disk_path.exists() {
            results.push(CandidateResult {
                file,
                candidate: file.contents.clone(),
                committed: None,
                status: CandidateStatus::Missing,
                held: false,
            });
            continue;
        }

        let committed = std::fs::read_to_string(&disk_path)
            .with_context(|| format!("reading {}", disk_path.display()))?
            .replace("\r\n", "\n");
        let committed = skillpack::verify::discovery::strip_bom(&committed).to_string();

        let candidate = if is_frontmatter_target(&file.rel_path) {
            let fresh_fm = skillpack::verify::fix::split_frontmatter(&file.contents)
                .map(|(fm, _body)| fm)
                .unwrap_or_else(|| file.contents.clone());
            let preserved_body = skillpack::verify::fix::split_frontmatter(&committed)
                .map(|(_fm, body)| body)
                .unwrap_or_default();
            format!("{fresh_fm}\n{preserved_body}")
        } else {
            file.contents.clone()
        };

        let status = if committed == candidate {
            CandidateStatus::Clean
        } else {
            CandidateStatus::Drifted
        };
        results.push(CandidateResult {
            file,
            candidate,
            committed: Some(committed),
            status,
            held: false,
        });
    }
    Ok(results)
}

/// Shared preamble: introspect, load config, resolve targets, render.
fn render_from_config(
    root: &Path,
    raw_targets: &[String],
    template_dir: Option<&Path>,
) -> Result<(
    types::ProjectProfile,
    types::Intent,
    Vec<GeneratedFileOutput>,
)> {
    let profile = introspect::introspect(root).context("introspecting repo")?;
    let existing_cfg = Config::load(root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no skillpack.toml at {} — a committed config is required.\n\
             To fix: run `skillpack init` first to seed it.",
            Config::path(root).display()
        )
    })?;
    let intent = existing_cfg.to_intent().ok_or_else(|| {
        anyhow::anyhow!(
            "skillpack.toml at {} is missing its [skill] table.\n\
         To fix: re-run `skillpack init` interactively to regenerate the config.",
            Config::path(root).display()
        )
    })?;
    let targets = if raw_targets.is_empty() {
        vec![Target::Claude]
    } else {
        resolve_targets(raw_targets)?
    };
    let files = render_targets(&profile, &intent, &targets, template_dir)
        .context("rendering distribution files")?;
    Ok((profile, intent, files))
}

fn run_update_inner(
    root: &Path,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
) -> Result<i32> {
    let (profile, intent, files) = render_from_config(root, &raw_targets, template_dir)?;
    let results = compute_candidates(root, &files, force)?;

    let mut written: Vec<&GeneratedFileOutput> = Vec::new();
    let mut unchanged = 0usize;
    let mut skipped: Vec<&GeneratedFileOutput> = Vec::new();
    let name = coerce_kebab(&profile.name);

    for r in &results {
        if r.held {
            skipped.push(r.file);
            continue;
        }
        match r.status {
            CandidateStatus::Missing => {
                let disk_path = root.join(&r.file.rel_path);
                if let Some(parent) = disk_path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("creating parent dir for {}", disk_path.display())
                    })?;
                }
                std::fs::write(&disk_path, &r.candidate)
                    .with_context(|| format!("writing {}", disk_path.display()))?;
                written.push(r.file);
            }
            CandidateStatus::Clean => {
                unchanged += 1;
            }
            CandidateStatus::Drifted => {
                let disk_path = root.join(&r.file.rel_path);
                std::fs::write(&disk_path, &r.candidate)
                    .with_context(|| format!("writing {}", disk_path.display()))?;
                written.push(r.file);
            }
        }
    }

    // Update skillpack.toml with current introspection (version/name may have changed).
    Config::from_intent(&name, &intent).save(root)?;

    // Summary.
    println!(
        "✓ updated {} file(s), {} unchanged, under {}:",
        written.len(),
        unchanged,
        root.display()
    );
    for f in &written {
        println!("   - {}", f.rel_path);
    }
    if unchanged > 0 {
        eprintln!("  ({unchanged} file(s) already up-to-date)");
    }
    if !skipped.is_empty() {
        eprintln!(
            "ℹ skipped {} target file(s) (existing file held; pass --force to overwrite):",
            skipped.len()
        );
        for f in &skipped {
            eprintln!("   - {}", f.rel_path);
        }
    }
    Ok(exit::INIT_OK)
}

/// `skillpack diff` — check whether distribution files are stale. Report
/// drifted/missing files and exit 1 if any. A CI gate for stale artifacts.
fn run_diff(
    root: &Path,
    _verbose: bool,
    _debug: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
) -> i32 {
    match run_diff_inner(root, &raw_targets, force, template_dir) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_diff_inner(
    root: &Path,
    raw_targets: &[String],
    force: bool,
    template_dir: Option<&Path>,
) -> Result<i32> {
    let (_profile, _intent, files) = render_from_config(root, raw_targets, template_dir)?;
    let results = compute_candidates(root, &files, force)?;

    let mut drifted = 0usize;
    let mut missing = 0usize;
    let mut unchanged = 0usize;
    let mut held = 0usize;

    for r in &results {
        if r.held {
            held += 1;
            eprintln!("  held: {} (pass --force to check)", r.file.rel_path);
            continue;
        }
        match r.status {
            CandidateStatus::Missing => {
                missing += 1;
                eprintln!("  missing: {}", r.file.rel_path);
            }
            CandidateStatus::Clean => {
                unchanged += 1;
            }
            CandidateStatus::Drifted => {
                drifted += 1;
                let first_diff =
                    first_differing_line(r.committed.as_deref().unwrap_or_default(), &r.candidate);
                eprintln!("  drifted: {} (first diff: {first_diff})", r.file.rel_path);
            }
        }
    }

    if drifted == 0 && missing == 0 {
        println!(
            "✓ all {unchanged} file(s) up-to-date ({})",
            if held > 0 {
                format!("{held} held")
            } else {
                "none held".into()
            },
        );
        Ok(exit::INIT_OK)
    } else {
        eprintln!(
            "\n✗ {drifted} drifted, {missing} missing, {unchanged} up-to-date{} — \
             run `skillpack update{}` to fix.",
            if held > 0 {
                format!(", {held} held")
            } else {
                String::new()
            },
            if force { " --force" } else { "" },
        );
        Ok(exit::DIFF_DRIFT)
    }
}

/// Return the first line that differs between `committed` and `candidate`
/// (with `-`/`+` prefix). For `diff`'s CI gate output — avoids pulling a
/// diff crate for what a char scan suffices.
fn first_differing_line(committed: &str, candidate: &str) -> String {
    for (c, n) in committed.lines().zip(candidate.lines()) {
        if c != n {
            return format!("- {c}\n+ {n}");
        }
    }
    let extra = if committed.lines().count() > candidate.lines().count() {
        committed
    } else {
        candidate
    };
    extra
        .lines()
        .nth(committed.lines().count().min(candidate.lines().count()))
        .map(|l| format!("± {l}"))
        .unwrap_or_else(|| "(no lines differ)".into())
}

/// True if the given rel-path is a frontmatter-bearing file that needs body
/// preservation during `update` (SKILL.md, cursor .mdc, opencode .md).
/// AGENTS.md and copilot-instructions.md are plain markdown (no frontmatter)
/// so they are NOT included — `split_frontmatter` would return None on them.
fn is_frontmatter_target(rel_path: &str) -> bool {
    rel_path.ends_with("SKILL.md")
        || rel_path.ends_with(".mdc")
        || (rel_path.ends_with(".md")
            && !rel_path.ends_with("AGENTS.md")
            && !rel_path.ends_with("copilot-instructions.md"))
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
        print_profile(&profile, false);
    }
}
