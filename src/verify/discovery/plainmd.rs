//! Plain-markdown instructions-file checks, shared by the harnesses that read
//! a single root-level markdown file with no frontmatter: GitHub Copilot
//! (`.github/copilot-instructions.md`), AGENTS.md (agents.md standard),
//! CLAUDE.md (Claude Code / Cline / Roo Code), GEMINI.md (Gemini CLI), and
//! CONVENTIONS.md (aider). All five share the same structural contract: plain
//! markdown, no frontmatter, non-empty, starts with a `#` heading.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::super::result::CheckResult;
use super::rel_unix;

/// The single-file path probe shared by every plain-markdown target.
pub(crate) fn find_plain_file(root: &Path, rel: &str) -> Option<PathBuf> {
    let p = root.join(rel);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Validate a plain-markdown instructions file: no frontmatter, non-empty,
/// starts with a `#` heading. `check_id` names the harness's check;
/// `label` is the human-readable file name used in messages; `empty_hint`
/// is the "To fix" text for an empty file (names the generating target).
pub(crate) fn check_plain_md(
    root: &Path,
    path: &Path,
    check_id: &str,
    label: &str,
    empty_hint: &str,
) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw = super::strip_bom(&raw);
    let rel = rel_unix(root, path);

    // Plain markdown spec — no frontmatter. `label` is the file name (e.g.
    // `AGENTS.md`), so the prose reads correctly for every harness.
    if raw.trim_start().starts_with("---") {
        return Ok(CheckResult::fail(
            check_id,
            &format!("{label} is plain markdown (no frontmatter)"),
            "file starts with a `---` frontmatter block",
            format!("To fix: remove the frontmatter block. {label} is plain markdown."),
        ));
    }
    if raw.trim().is_empty() {
        return Ok(CheckResult::fail(
            check_id,
            "file is non-empty",
            format!("{label} is empty"),
            empty_hint,
        ));
    }

    // A leading `#` heading is the structural expectation (matches every
    // example in the harness docs).
    let first_non_blank = raw.lines().find(|l| !l.trim().is_empty());
    match first_non_blank {
        Some(line) if line.trim_start().starts_with('#') => Ok(CheckResult::pass(
            check_id,
            &format!("{label} file validates"),
            format!("{rel} validates"),
        )),
        Some(_) => Ok(CheckResult::warn(
            check_id,
            "file starts with a `#` heading",
            "first non-blank line is not a markdown heading",
            "To fix: start the file with `# <tool name>`.",
        )),
        None => Ok(CheckResult::fail(
            check_id,
            "file is non-empty",
            "file contains only blank lines",
            "To fix: add instructions content.",
        )),
    }
}
