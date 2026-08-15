//! `skillpack doctor` — diagnose why introspection chose what it did.
//! Read-only: prints the detected profile + the decision trace (`diag`),
//! never writes files. The trace is empty until candidate fns push notes
//! (the `detect_*` falsy branches); doctor surfaces exactly why `has_cli`
//! came out false so the maintainer can act.

use std::path::Path;

use anyhow::{bail, Context, Result};

use clap::ValueEnum;

use skillpack::cli::Target;
use skillpack::exit;
use skillpack::introspect;
use skillpack::types;

use super::{print_profile, trace_detected};

pub(crate) fn run_doctor(
    root: &Path,
    verbose: bool,
    format: skillpack::verify::OutputFormat,
) -> i32 {
    match run_doctor_inner(root, verbose, format) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_doctor_inner(
    root: &Path,
    verbose: bool,
    format: skillpack::verify::OutputFormat,
) -> Result<i32> {
    let profile = introspect::introspect(root).context("introspecting repo for doctor")?;

    match format {
        skillpack::verify::OutputFormat::Json => {
            // The serialized `ProjectProfile` IS the doctor JSON report —
            // including the `diag` decision trace + every detected field,
            // exactly what a consumer wants to scrape. No envelope wrapping;
            // the consumer reads fields by name. Exits 0 (doctor is
            // read-only diagnostic, non-gating — matches human form).
            //
            // A `verify_category_preview` field mirrors the human-mode
            // category/target preview so JSON consumers get the same "what
            // would `verify` check" signal without parsing prose.
            let mut v =
                serde_json::to_value(&profile).context("serializing doctor profile to JSON")?;
            let targets: Vec<String> = Target::value_variants()
                .iter()
                .filter_map(|t| t.to_possible_value())
                .map(|pv| pv.get_name().to_string())
                .collect();
            v["verify_category_preview"] = serde_json::json!({
                "targets": targets,
                "categories": if profile.has_cli {
                    vec!["discovery", "invocation"]
                } else {
                    vec!["discovery"]
                },
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&v).context("serializing doctor profile to JSON")?
            );
        }
        skillpack::verify::OutputFormat::Human => render_doctor_human(&profile, verbose),
        skillpack::verify::OutputFormat::Sarif
        | skillpack::verify::OutputFormat::Github
        | skillpack::verify::OutputFormat::Junit => {
            bail!("doctor does not support this format; use `verify` for machine-readable reports")
        }
    }

    // Doctor never writes; always exits 0.
    Ok(exit::VERIFY_OK)
}

/// Render the human-facing diagnosis. Lifted verbatim from the pre-format
/// behavior so `doctor` (no flag) and `doctor --format human` are byte-identical.
fn render_doctor_human(profile: &types::ProjectProfile, verbose: bool) {
    trace_detected(profile);
    // Reuse the same profile block --verbose prints so doctor's output starts
    // from a known place.
    if verbose {
        print_profile(profile, false);
    } else {
        println!("skillpack doctor");
        println!("  name:     {}", profile.name);
        println!("  language: {}", profile.language.as_str());
        if !profile.secondary_languages.is_empty() {
            let langs: Vec<&str> = profile
                .secondary_languages
                .iter()
                .map(|l| l.as_str())
                .collect();
            println!("  secondary: {}", langs.join(", "));
        }
        println!("  has_cli:  {}", profile.has_cli);
        if let Some(cmd) = &profile.cli_command {
            println!("  cli:      {}", cmd.join(" "));
        }
    }

    println!();
    if profile.diag.0.is_empty() {
        if profile.has_cli {
            println!("decision trace: (empty; CLI detected cleanly, no falsy branches fired)");
        } else {
            println!("decision trace: (empty; no candidate notes were pushed)");
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
    println!("  discovery.*: structural validation of every generated file per ecosystem");
    println!("    (Claude plugin + native skills, Codex, Cursor, OpenCode, Copilot, AGENTS.md,");
    println!("     CLAUDE.md, GEMINI.md, Windsurf, Aider, Cline, Roo Code, Kilo Code, Goose,");
    println!("     Qoder, Continue, Augment, Amazon Q)");
    if profile.has_cli {
        println!("  invocation.*: runs the CLI: --help, flag drift, subcommand drift");
        println!("    --version drift (advisory)");
    } else {
        println!("  invocation.*: N/A (no CLI detected; checks will be skipped)");
    }

    println!();
    println!(
        "next step: `skillpack init --target all --auto` to scaffold guidance for every ecosystem."
    );
}
