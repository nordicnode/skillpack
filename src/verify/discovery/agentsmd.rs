//! AGENTS.md discovery checks. Per agents.md (Linux Foundation stewarded):
//! plain markdown, no frontmatter, no required fields. Read natively by 20+
//! coding agents (Codex, Cursor, Windsurf, Copilot, Aider, Zed, Warp, JetBrains
//! Junie, etc.).

use std::path::Path;

use anyhow::Result;

use super::super::result::CheckResult;
use super::super::schema;

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
/// start with a `#` heading (structural, no grammar). Delegates to the shared
/// plain-markdown check.
pub(crate) fn check_agents_md(root: &Path, path: &Path) -> Result<CheckResult> {
    super::plainmd::check_plain_md(
        root,
        path,
        "discovery.agentsmd",
        "AGENTS.md",
        "To fix: add instructions content, or run `skillpack init --target agentsmd`.",
    )
}
