//! GitHub Copilot `.github/copilot-instructions.md` discovery checks.

use std::path::Path;

use anyhow::Result;

use super::super::result::CheckResult;
use super::super::schema;

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
/// Delegates to the shared plain-markdown check.
pub(crate) fn check_copilot_instructions(root: &Path, path: &Path) -> Result<CheckResult> {
    super::plainmd::check_plain_md(
        root,
        path,
        "discovery.copilot.instructions",
        "`.github/copilot-instructions.md`",
        "To fix: add instructions content, or run `skillpack init --target copilot`.",
    )
}
