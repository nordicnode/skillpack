//! AGENTS.md discovery checks. Per agents.md (Linux Foundation stewarded):
//! plain markdown, no frontmatter, no required fields. Read natively by 20+
//! coding agents (Codex, Cursor, Windsurf, Copilot, Aider, Zed, Warp, JetBrains
//! Junie, etc.).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::super::result::CheckResult;
use super::super::schema;
use super::rel_unix;

/// The single AGENTS.md path (root-level, one file).
pub(crate) fn find_agents_md(root: &Path) -> Option<std::path::PathBuf> {
    let p = root.join(schema::AGENTS_MD_PATH);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Validate `AGENTS.md`: plain markdown, no frontmatter. Must be non-empty and
/// start with a `#` heading (structural, no grammar). Mirrors the Copilot
/// instructions check — same format, different path + check_id.
pub(crate) fn check_agents_md(root: &Path, path: &Path) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw = super::strip_bom(&raw);
    let check_id = "discovery.agentsmd";
    let rel = rel_unix(root, path);

    // AGENTS.md spec: "just standard Markdown." No frontmatter.
    if raw.trim_start().starts_with("---") {
        return Ok(CheckResult::fail(
            check_id,
            "AGENTS.md is plain markdown (no frontmatter)",
            "file starts with a `---` frontmatter block",
            "To fix: remove the frontmatter block. AGENTS.md is plain markdown.",
        ));
    }
    if raw.trim().is_empty() {
        return Ok(CheckResult::fail(
            check_id,
            "file is non-empty",
            "AGENTS.md is empty",
            "To fix: add instructions content, or run `skillpack init --target agentsmd`.",
        ));
    }

    // A leading `#` heading is the structural expectation (matches every
    // example in the agents.md spec).
    let first_non_blank = raw.lines().find(|l| !l.trim().is_empty());
    match first_non_blank {
        Some(line) if line.trim_start().starts_with('#') => Ok(CheckResult::pass(
            check_id,
            "AGENTS.md file validates",
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
