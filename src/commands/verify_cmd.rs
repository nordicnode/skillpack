//! `skillpack verify` — run discovery + invocation checks against the
//! committed distribution files, with `--fix` drift repair and `--watch`
//! re-run support.

use std::path::Path;

use anyhow::{Context, Result};

use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::coerce_kebab;
use skillpack::introspect;
use skillpack::verify::{self, VerifyInput};

use super::print_profile;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_verify(
    root: &Path,
    verbose: bool,
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
        return run_verify_watch(root, verbose, format, fix, min_score, template_dir);
    }
    match run_verify_inner(root, verbose, format, fix, min_score, template_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_verify_inner(
    root: &Path,
    verbose: bool,
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
    // Load verify_stdin from skillpack.toml if present (silent fallback to
    // None — verify stays usable on hand-written packs without a config).
    let verify_stdin = Config::load(root)
        .ok()
        .and_then(|opt| opt.and_then(|cfg| cfg.to_intent()))
        .and_then(|intent| intent.verify_stdin);
    if verbose {
        // Every machine-readable format (json, sarif, github annotations,
        // junit XML) owns stdout; the introspection block must go to stderr
        // so `--verbose --format github` / `--format junit` output stays
        // parseable, not just json/sarif. Human mode keeps it on stdout.
        print_profile(&profile, format != verify::OutputFormat::Human);
    }
    let render = |report: &verify::VerifyReport| match format {
        verify::OutputFormat::Human => verify::render(report),
        verify::OutputFormat::Json => format!("{}\n", verify::render_json(report)),
        verify::OutputFormat::Sarif => format!("{}\n", verify::render_sarif(report)),
        verify::OutputFormat::Github => verify::render_github_annotations(report),
        verify::OutputFormat::Junit => verify::render_junit(report),
    };
    let run_verify = || -> Result<verify::VerifyReport> {
        let input = VerifyInput {
            root: root.to_path_buf(),
            spawn_root: root.to_path_buf(),
            cli_command: profile.cli_command.clone(),
            profile_name: Some(coerce_kebab(&profile.name)),
            verify_stdin: verify_stdin.clone(),
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

    let _ = run_verify_single(root, verbose, format, fix, min_score, template_dir);

    let debounce = Duration::from_secs(1);
    let mut last_event: Option<Instant> = None;

    // Skip events from noisy paths: build/vendored/cache trees (the same set
    // `introspect::is_noise_dir` skips — a watch re-run on `cargo build`
    // churn is pure noise), plus the agent benchmark run artifacts, so editing
    // benchmark results doesn't re-verify.
    let is_noise = |path: &std::path::Path| -> bool {
        let s = path.to_string_lossy();
        if s.contains("scripts/benchmark/results") {
            return true;
        }
        path.components().any(|c| {
            matches!(
                c,
                std::path::Component::Normal(s)
                    if matches!(
                        s.to_str(),
                        Some(
                            "target" | ".git" | "node_modules" | "dist" | "build" | "out"
                                | "vendor" | "venv" | "__pycache__" | "coverage" | "Pods"
                                | "bazel-bin" | "bazel-out" | ".freebuff"
                        )
                    )
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
                        // Clear screen for a clean re-render (only when
                        // stdout is a real terminal — never emit ANSI to a
                        // pipe or captured log).
                        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                            print!("\x1b[2J\x1b[H");
                        }
                        let _ =
                            run_verify_single(root, verbose, format, fix, min_score, template_dir);
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
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    template_dir: Option<&Path>,
) -> i32 {
    // Exit-code parity with the non-watch `run_verify`: an unrecoverable
    // introspect/render error is INIT_FATAL (3), not VERIFY_FAIL (1). The
    // watcher loop discards this per-cycle code (it re-runs on each change),
    // but the mapping must not silently disagree with a standalone run.
    match run_verify_inner(root, verbose, format, fix, min_score, template_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            exit::INIT_FATAL
        }
    }
}
