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
    if let Some(code) = handle_list_request("diff", &raw_targets, format) {
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
/// (with `-`/`+` prefix), or a trailing-newline note when the only difference
/// is line-level-invisible (a missing/added final `\n`). For `diff`'s CI gate
/// output — avoids pulling a diff crate for what a char scan suffices. Must
/// never print "(no lines differ)" for content that actually differs:
/// `str::lines()` normalizes away a trailing newline, so `"a\n"` vs `"a"`
/// would otherwise report a drift with no visible diff.
fn first_differing_line(committed: &str, candidate: &str) -> String {
    for (lineno, (c, n)) in (1usize..).zip(committed.lines().zip(candidate.lines())) {
        if c != n {
            return format!("- {c}\n+ {n} (line {lineno})");
        }
    }
    let cl = committed.lines().count();
    let nl = candidate.lines().count();
    if cl > nl {
        if let Some(extra) = committed.lines().nth(nl) {
            return format!("± {extra} (line {})", nl + 1);
        }
    } else if nl > cl {
        if let Some(extra) = candidate.lines().nth(cl) {
            return format!("+ {extra} (line {})", cl + 1);
        }
    }
    match (committed.ends_with('\n'), candidate.ends_with('\n')) {
        (true, false) => "± candidate is missing the trailing newline".to_string(),
        (false, true) => "± candidate adds a trailing newline".to_string(),
        _ => "± bytes differ only in line endings".to_string(),
    }
}

#[cfg(test)]
mod first_diff_tests {
    use super::first_differing_line;

    #[test]
    fn differing_middle_line_reports_both_sides() {
        assert_eq!(
            first_differing_line("a\nb\nc", "a\nB\nc"),
            "- b\n+ B (line 2)"
        );
    }

    #[test]
    fn candidate_has_extra_trailing_lines() {
        assert_eq!(first_differing_line("a\nb", "a\nb\nc"), "+ c (line 3)");
    }

    #[test]
    fn committed_has_extra_trailing_lines() {
        assert_eq!(first_differing_line("a\nb\nc", "a\nb"), "± c (line 3)");
    }

    #[test]
    fn trailing_newline_only_diff_is_reported() {
        // `lines()` sees both as ["a"]; the raw bytes differ. Must not say
        // "(no lines differ)".
        assert_eq!(
            first_differing_line("a\n", "a"),
            "± candidate is missing the trailing newline"
        );
        assert_eq!(
            first_differing_line("a", "a\n"),
            "± candidate adds a trailing newline"
        );
    }
}
