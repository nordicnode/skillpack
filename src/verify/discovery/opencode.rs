//! OpenCode `.opencode/agents/<name>.md` discovery checks.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::super::result::CheckResult;
use super::super::schema;
use super::{find_kv_colon, is_valid_kebab, rel_unix};

/// OpenCode agent frontmatter. Per opencode.ai/docs/agents: `description`
/// is required; `mode` is optional (primary|subagent|all, defaults to all).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpenCodeFrontmatter {
    pub description: Option<String>,
    pub mode: Option<String>,
    /// True when the `---` block was terminated by a closing `---` line.
    /// False for an unterminated block (or `Default` when the caller found
    /// no leading `---` at all — `parse_opencode_agent_frontmatter` returns
    /// `None` for that case, so the two are distinguishable).
    pub closed: bool,
}

impl OpenCodeFrontmatter {
    fn parse(block: &str) -> Self {
        let mut fm = Self::default();
        let mut current_key: Option<String> = None;
        let mut current_val = String::new();
        for line in block.lines() {
            let trimmed = line.trim_end();
            if let Some(idx) = find_kv_colon(trimmed) {
                if let Some(k) = current_key.take() {
                    let clean = current_val.trim().trim_matches('"').trim().to_string();
                    match k.as_str() {
                        "description" => fm.description = Some(clean),
                        "mode" => fm.mode = Some(clean),
                        _ => {}
                    }
                    current_val.clear();
                }
                let key = trimmed[..idx].trim().to_string();
                let val = trimmed[idx + 1..].trim().to_string();
                current_key = Some(key);
                current_val = val;
            } else if !trimmed.is_empty() && current_key.is_some() {
                current_val.push('\n');
                current_val.push_str(trimmed);
            }
        }
        if let Some(k) = current_key.take() {
            let clean = current_val.trim().trim_matches('"').trim().to_string();
            match k.as_str() {
                "description" => fm.description = Some(clean),
                "mode" => fm.mode = Some(clean),
                _ => {}
            }
        }
        fm
    }
}

/// Parse the `---`-delimited YAML frontmatter out of an OpenCode agent .md
/// file. Same shape as [`super::parse_cursor_mdc_frontmatter`]. Exposed for
/// tests.
pub fn parse_opencode_agent_frontmatter(raw: &str) -> Option<OpenCodeFrontmatter> {
    let mut lines = raw.lines();
    let first = lines.next()?.trim();
    if first != "---" {
        return None;
    }
    let mut body = String::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    let mut fm = OpenCodeFrontmatter::parse(&body);
    fm.closed = closed;
    Some(fm)
}

/// Validate a single `.opencode/agents/<name>.md` against OpenCode's
/// documented schema. `description` present + non-empty + under the listing
/// cap is a hard fail; `mode` valid range is a warning (docs mark it
/// optional with a default). File-name kebab-ness is warned, not failed.
pub(crate) fn check_one_opencode_agent(root: &Path, path: &Path) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw = super::strip_bom(&raw);
    let Some(fm) = parse_opencode_agent_frontmatter(raw) else {
        return Ok(CheckResult::fail(
            "discovery.opencode.agent.frontmatter",
            "frontmatter block present (--- delimited)",
            "no YAML frontmatter found",
            "To fix: add a `---` frontmatter block with `description:`.",
        ));
    };

    // Unterminated `---` block → the body is being parsed as frontmatter.
    if !fm.closed {
        return Ok(CheckResult::fail(
            "discovery.opencode.agent.frontmatter_unclosed",
            "frontmatter block is closed by a `---` delimiter",
            "frontmatter block is not closed (missing the closing `---`)",
            "To fix: add a closing `---` line after the last frontmatter field.",
        ));
    }

    // `description` is required per opencode.ai/docs/agents.
    let desc = fm.description.as_deref().unwrap_or("");
    if desc.is_empty() {
        return Ok(CheckResult::fail(
            "discovery.opencode.agent.description",
            "frontmatter `description` is present and non-empty",
            "description is missing or empty",
            "To fix: add `description: \"<what this agent does>\"` to the frontmatter.",
        ));
    }
    if desc.chars().count() > schema::SKILL_LISTING_CHAR_CAP {
        return Ok(CheckResult::fail(
            "discovery.opencode.agent.description",
            "description stays under the listing cap",
            format!(
                "description is {} chars (exceeds cap)",
                desc.chars().count()
            ),
            "To fix: shorten the description.",
        ));
    }

    // `mode` is optional; if present, must be primary|subagent|all (warn only).
    let mut warnings: Vec<String> = Vec::new();
    let check_id = "discovery.opencode.agent";
    if let Some(mode) = &fm.mode {
        if !matches!(mode.as_str(), "primary" | "subagent" | "all") {
            warnings.push(format!(
                "mode `{mode}` is not one of primary|subagent|all (defaults to all)"
            ));
        }
    }

    // File-name kebab-ness: warn so `My Agent.md` surfaces as a smell.
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if !is_valid_kebab(stem) {
            warnings.push(format!("file name `{stem}` is not kebab-case"));
        }
    }

    if warnings.is_empty() {
        Ok(CheckResult::pass(
            check_id,
            "OpenCode agent file validates against opencode.ai/docs/agents",
            format!("{} validates", rel_unix(root, path)),
        ))
    } else {
        Ok(CheckResult::warn(
            check_id,
            "OpenCode agent file validates against opencode.ai/docs/agents",
            warnings.join("; "),
            "To fix: address the warnings above (non-blocking).",
        ))
    }
}

/// Every `.opencode/agents/<name>.md`, sorted.
pub(crate) fn find_opencode_agent_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let dir = root.join(schema::OPENCODE_AGENTS_DIR);
    if dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut names: Vec<_> = entries.flatten().collect();
            names.sort_by_key(|e| e.file_name());
            for entry in names {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|e| e == "md") {
                    out.push(path);
                }
            }
        }
    }
    out
}
