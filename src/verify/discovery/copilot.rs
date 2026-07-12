//! GitHub Copilot `.github/copilot-instructions.md` discovery checks.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::super::result::CheckResult;
use super::super::schema;
use super::rel_unix;

/// The single Copilot instructions path (one file, not a directory scan).
pub(crate) fn find_copilot_instructions(root: &Path) -> Option<std::path::PathBuf> {
    let p = root.join(schema::COPILOT_INSTRUCTIONS_PATH);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Validate `.github/copilot-instructions.md`: plain markdown, no frontmatter.
/// Must be non-empty and start with a `#` heading (structural, no grammar).
pub(crate) fn check_copilot_instructions(root: &Path, path: &Path) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let check_id = "discovery.copilot.instructions";
    let rel = rel_unix(root, path);

    // The Copilot spec (see schema.rs) says "Plain markdown, no frontmatter."
    // A file starting with `---` is a hard spec violation.
    if raw.trim_start().starts_with("---") {
        return Ok(CheckResult::fail(
            check_id,
            "Copilot instructions are plain markdown (no frontmatter)",
            "file starts with a `---` frontmatter block",
            "To fix: remove the frontmatter block. Copilot instructions are plain markdown.",
        ));
    }
    if raw.trim().is_empty() {
        return Ok(CheckResult::fail(
            check_id,
            "file is non-empty",
            "copilot-instructions.md is empty",
            "To fix: add instructions content, or run `skillpack init --target copilot`.",
        ));
    }

    // Copilot instructions are plain markdown; a leading `#` heading is the
    // structural expectation (matches every example in the GitHub docs).
    let first_non_blank = raw.lines().find(|l| !l.trim().is_empty());
    match first_non_blank {
        Some(line) if line.trim_start().starts_with('#') => Ok(CheckResult::pass(
            check_id,
            "Copilot instructions file validates",
            format!("{} validates", rel),
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
