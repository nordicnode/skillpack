//! `skillpack update` — incrementally regenerate distribution files from an
//! existing `skillpack.toml`. No interview, no pre-commit verify gate. Reads
//! the committed config, re-introspects, re-renders every target, and writes
//! ONLY files whose content changed. For frontmatter-bearing files the body
//! is preserved via the same splice `--fix` uses; frontmatter is regenerated
//! wholesale. Returns exit 0 on success.
//!
//! Also owns the shared refresh machinery (`compute_candidates`,
//! `detect_present_targets`, `default_refresh_targets`, `render_from_config`)
//! that `diff` and `add`/`remove` reuse.

use std::path::Path;

use anyhow::{bail, Context, Result};

use skillpack::cli::{resolve_targets, Target};
use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::{render_all, GeneratedFileOutput};
use skillpack::introspect;
use skillpack::types;
use skillpack::verify;

use super::{
    ensure_no_symlink_ancestors, handle_list_request, is_collision_guarded, is_frontmatter_target,
    is_json, print_profile, reject_report_format, trace_detected,
};

pub(crate) fn run_update(
    root: &Path,
    verbose: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> i32 {
    // Validate the format before honoring `--target list` (see `run_init`).
    if let Err(e) = reject_report_format(format) {
        eprintln!("fatal: {e:#}");
        return exit::INIT_FATAL;
    }
    if let Some(code) = handle_list_request("update", &raw_targets, format) {
        return code;
    }
    match run_update_inner(root, verbose, raw_targets, force, template_dir, format) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

/// Result of comparing one rendered file against its on-disk content.
pub(crate) struct CandidateResult<'a> {
    pub(crate) file: &'a GeneratedFileOutput,
    /// What we would write (spliced frontmatter + preserved body for
    /// frontmatter files; raw render for fully-generated files).
    pub(crate) candidate: String,
    /// On-disk content (BOM-stripped, CRLF-normalized).
    pub(crate) committed: Option<String>,
    /// None = file not on disk (new). Some = file exists.
    pub(crate) status: CandidateStatus,
    /// True if the AGENTS.md collision guard skipped this file.
    pub(crate) held: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CandidateStatus {
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
pub(crate) fn compute_candidates<'f>(
    root: &Path,
    files: &'f [GeneratedFileOutput],
    force: bool,
) -> Result<Vec<CandidateResult<'f>>> {
    let mut results = Vec::with_capacity(files.len());
    for file in files {
        let disk_path = root.join(&file.rel_path);

        // Read the committed content (normalized: CRLF -> LF, BOM stripped) so
        // both the frontmatter splice and the collision guard compare against
        // the same bytes the agent actually reads.
        let committed = if disk_path.exists() {
            let raw = std::fs::read_to_string(&disk_path)
                .with_context(|| format!("reading {}", disk_path.display()))?
                .replace("\r\n", "\n");
            Some(skillpack::verify::discovery::strip_bom(&raw).to_string())
        } else {
            None
        };

        let candidate = if is_frontmatter_target(&file.rel_path) {
            match &committed {
                // Existing file: splice the fresh frontmatter onto the
                // committed body so a maintainer's hand-tailored prose
                // survives regeneration.
                Some(committed) => {
                    let fresh_fm = skillpack::verify::fix::split_frontmatter(&file.contents)
                        .map(|(fm, _body)| fm)
                        .unwrap_or_else(|| file.contents.clone());
                    let preserved_body = skillpack::verify::fix::split_frontmatter(committed)
                        .map(|(_fm, body)| body)
                        .unwrap_or_default();
                    format!("{fresh_fm}\n{preserved_body}")
                }
                // New file (not on disk yet): the full fresh render. The old
                // splice reused the same path for both, so a target added
                // since the last `update` was written as frontmatter-only
                // (empty preserved body) — the body prose silently dropped
                // and invisible to `diff`/`verify` afterwards.
                None => file.contents.clone(),
            }
        } else {
            file.contents.clone()
        };

        // Root-level plain instruction files are guarded: without --force, a
        // file that differs from the fresh render is HELD (it may be
        // hand-written). A file that already matches is left clean — this is
        // what lets a skillpack-generated AGENTS.md be tracked without noise
        // while still protecting a hand-written one.
        if is_collision_guarded(&file.rel_path) && committed.is_some() && !force {
            let held = committed.as_deref() != Some(candidate.as_str());
            results.push(CandidateResult {
                file,
                candidate,
                committed,
                status: CandidateStatus::Clean,
                held,
            });
            continue;
        }

        let status = match &committed {
            None => CandidateStatus::Missing,
            Some(c) if *c == candidate => CandidateStatus::Clean,
            Some(_) => CandidateStatus::Drifted,
        };
        results.push(CandidateResult {
            file,
            candidate,
            committed,
            status,
            held: false,
        });
    }
    Ok(results)
}

/// The targets whose distribution files are already present on disk, in
/// canonical [`Target`] declaration order. Used as the default for
/// `update`/`diff` when no `--target` is given, so those commands refresh or
/// check the whole existing distribution instead of silently limiting
/// themselves to the Claude target (the old default) and leaving every other
/// ecosystem stale.
///
/// Both the per-ecosystem directory markers and the root-level single-file
/// targets (AGENTS.md, CLAUDE.md, GEMINI.md, CONVENTIONS.md,
/// `.goose/instructions.md`, `.github/copilot-instructions.md`) are probed by
/// existence. Probing the single files no longer means they are silently
/// dropped from the default run: `update`/`diff` hold a hand-written copy (or
/// one that drifted) via the collision guard, and report a clean
/// skillpack-generated copy as unchanged — so a generated root file is
/// tracked instead of ignored.
pub(crate) fn detect_present_targets(root: &Path) -> Vec<Target> {
    let mut present = Vec::new();
    for (target, marker) in [
        (Target::Claude, ".claude-plugin"),
        (Target::Cursor, ".cursor/rules"),
        (Target::Codex, ".codex/skills"),
        (Target::OpenCode, ".opencode/agents"),
        (Target::Windsurf, ".windsurf/rules"),
        (Target::Cline, ".clinerules"),
        (Target::Roo, ".roo/rules"),
        (Target::Kilo, ".kilocode/rules"),
        (Target::Qoder, ".qoder/rules"),
        (Target::Continue, ".continue/rules"),
        (Target::Augment, ".augment/rules"),
        (Target::AmazonQ, ".amazonq/rules"),
        (Target::Trae, ".trae/rules"),
        (
            Target::Copilot,
            skillpack::verify::schema::COPILOT_INSTRUCTIONS_PATH,
        ),
        (Target::AgentsMd, skillpack::verify::schema::AGENTS_MD_PATH),
        (Target::ClaudeMd, skillpack::verify::schema::CLAUDE_MD_PATH),
        (Target::Gemini, skillpack::verify::schema::GEMINI_MD_PATH),
        (
            Target::Aider,
            skillpack::verify::schema::CONVENTIONS_MD_PATH,
        ),
        (
            Target::Goose,
            skillpack::verify::schema::GOOSE_INSTRUCTIONS_PATH,
        ),
    ] {
        if root.join(marker).exists() {
            present.push(target);
        }
    }
    present
}

/// Resolve the default target set for `update`/`diff` (and `add`, which
/// delegates to `update`). Prefers the targets already present on disk; when
/// none are detected (a committed config with no generated files yet), falls
/// back to `all` so a refresh regenerates the full distribution rather than
/// silently limiting itself to Claude.
pub(crate) fn default_refresh_targets(root: &Path) -> Result<Vec<Target>> {
    let present = detect_present_targets(root);
    if present.is_empty() {
        resolve_targets(&["all".to_string()])
    } else {
        Ok(present)
    }
}

/// Shared preamble: introspect, load config, resolve targets, render.
/// Returns the profile, the full skill list (one entry for single-skill
/// packs, one per `[[skills]]` entry for multi-skill packs), and every
/// rendered file — pack-level files from the primary skill, per-skill files
/// for every skill.
#[allow(clippy::type_complexity)]
pub(crate) fn render_from_config(
    root: &Path,
    raw_targets: &[String],
    template_dir: Option<&Path>,
) -> Result<(
    types::ProjectProfile,
    Vec<(String, types::Intent)>,
    Vec<GeneratedFileOutput>,
)> {
    let profile = introspect::introspect(root).context("introspecting repo")?;
    let existing_cfg = Config::load(root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no skillpack.toml at {}: a committed config is required.\n\
             To fix: run `skillpack init` first to seed it.",
            Config::path(root).display()
        )
    })?;
    let skills = existing_cfg.to_intents();
    if skills.is_empty() {
        bail!(
            "skillpack.toml at {} is missing its [skill] table.\n\
         To fix: re-run `skillpack init` interactively to regenerate the config.",
            Config::path(root).display()
        );
    }
    let targets = if raw_targets.is_empty() {
        default_refresh_targets(root)?
    } else {
        resolve_targets(raw_targets)?
    };
    let files = render_all(&profile, &skills, &targets, template_dir)
        .context("rendering distribution files")?;
    Ok((profile, skills, files))
}

pub(crate) fn run_update_inner(
    root: &Path,
    verbose: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> Result<i32> {
    reject_report_format(format)?;
    let (profile, skills, files) = render_from_config(root, &raw_targets, template_dir)?;
    if verbose {
        print_profile(&profile, false);
    }
    trace_detected(&profile);
    let results = compute_candidates(root, &files, force)?;

    let mut written: Vec<&GeneratedFileOutput> = Vec::new();
    let mut unchanged = 0usize;
    let mut skipped: Vec<&GeneratedFileOutput> = Vec::new();

    for r in &results {
        if r.held {
            skipped.push(r.file);
            continue;
        }
        match r.status {
            CandidateStatus::Missing => {
                let disk_path = root.join(&r.file.rel_path);
                ensure_no_symlink_ancestors(root, &r.file.rel_path)?;
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
                ensure_no_symlink_ancestors(root, &r.file.rel_path)?;
                std::fs::write(&disk_path, &r.candidate)
                    .with_context(|| format!("writing {}", disk_path.display()))?;
                written.push(r.file);
            }
        }
    }

    // Update skillpack.toml with current introspection (version/name may have
    // changed) — but only when the serialized form actually differs, so a
    // no-op `update` doesn't churn the config's mtime or rewrite a
    // hand-formatted file.
    Config::from_intents(&skills).save_if_changed(root)?;

    // Summary.
    if is_json(format) {
        println!(
            "{}",
            serde_json::json!({
                "command": "update",
                "written": written.iter().map(|f| &f.rel_path).collect::<Vec<_>>(),
                "unchanged": unchanged,
                "skipped": skipped.iter().map(|f| &f.rel_path).collect::<Vec<_>>(),
            })
        );
        return Ok(exit::INIT_OK);
    }
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
