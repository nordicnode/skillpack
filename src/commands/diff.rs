//! `skillpack diff` — check whether distribution files are stale. Report
//! drifted/missing files and exit 1 if any. A CI gate for stale artifacts.

use std::path::Path;

use anyhow::Result;

use skillpack::exit;
use skillpack::verify;

use super::update::{compute_candidates, render_from_config, CandidateStatus};
use super::{handle_list_request, is_json, print_profile, reject_report_format, trace_detected};

pub(crate) fn run_diff(
    root: &Path,
    verbose: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> i32 {
    if let Some(code) = handle_list_request(&raw_targets) {
        return code;
    }
    match run_diff_inner(root, verbose, &raw_targets, force, template_dir, format) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_diff_inner(
    root: &Path,
    verbose: bool,
    raw_targets: &[String],
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> Result<i32> {
    reject_report_format(format)?;
    let (profile, _skills, files) = render_from_config(root, raw_targets, template_dir)?;
    if verbose {
        print_profile(&profile, false);
    }
    trace_detected(&profile);
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

    if is_json(format) {
        println!(
            "{}",
            serde_json::json!({
                "command": "diff",
                "clean": drifted == 0 && missing == 0,
                "drifted": drifted,
                "missing": missing,
                "unchanged": unchanged,
                "held": held,
            })
        );
        return Ok(if drifted == 0 && missing == 0 {
            exit::INIT_OK
        } else {
            exit::DIFF_DRIFT
        });
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
            "\n✗ {drifted} drifted, {missing} missing, {unchanged} up-to-date{}: \
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
