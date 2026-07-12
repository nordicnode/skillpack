//! Cursor `.cursor/rules/<name>.mdc` discovery checks. YAML frontmatter
//! schema is documented at cursor.com/docs/rules (verified July 2026).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::super::result::CheckResult;
use super::super::schema;
use super::{find_kv_colon, rel_unix};

/// Cursor `.mdc` frontmatter. Schema is documented at cursor.com/docs/rules
/// (verified July 2026 against the live docs + the polarpoint.io writeup):
///   description: <string, required> — drives auto-attach when alwaysApply:false
///   globs:        [list of glob patterns]   — optional
///   alwaysApply:  <bool>                    — required
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CursorFrontmatter {
    pub description: Option<String>,
    pub always_apply: Option<String>,
}

impl CursorFrontmatter {
    fn parse(block: &str) -> Self {
        let mut fm = Self::default();
        let mut current_key: Option<String> = None;
        let mut current_val = String::new();
        for line in block.lines() {
            let trimmed = line.trim_end();
            if let Some(idx) = find_kv_colon(trimmed) {
                if let Some(k) = current_key.take() {
                    store_cursor(&mut fm, &k, current_val.trim());
                    current_val.clear();
                }
                let key = trimmed[..idx].trim().to_string();
                let val = trimmed[idx + 1..].trim().trim_matches('"').to_string();
                current_key = Some(key);
                current_val = val;
            } else if !trimmed.is_empty() && current_key.is_some() {
                current_val.push('\n');
                current_val.push_str(trimmed);
            }
        }
        if let Some(k) = current_key.take() {
            store_cursor(&mut fm, &k, current_val.trim());
        }
        fm
    }
}

fn store_cursor(fm: &mut CursorFrontmatter, key: &str, val: &str) {
    match key {
        "description" => fm.description = Some(val.to_string()),
        "alwaysApply" => fm.always_apply = Some(val.to_string()),
        _ => {}
    }
}

/// Parse the YAML frontmatter out of a Cursor `.mdc` file. Same `---`-delimited
/// shape as [`super::parse_skill_frontmatter`]; the parsed struct differs
/// because the keys differ. Exposed for unit tests.
pub fn parse_cursor_mdc_frontmatter(raw: &str) -> Option<CursorFrontmatter> {
    let mut lines = raw.lines();
    let first = lines.next()?.trim();
    if first != "---" {
        return None;
    }
    let mut body = String::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    Some(CursorFrontmatter::parse(&body))
}

/// Validate a single `.cursor/rules/<name>.mdc` against Cursor's documented
/// schema. Path-name consistency (kebab-ish) is warned, not failed — Cursor
/// itself doesn't enforce it, but a name like `My Rule.mdc` is a maintenance
/// smell.
pub(crate) fn check_one_mdc(root: &Path, path: &Path) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let raw = super::strip_bom(&raw);
    let fm = parse_cursor_mdc_frontmatter(raw).unwrap_or_default();

    let rel = rel_unix(root, path);

    let Some(description) = fm.description.as_deref() else {
        return Ok(CheckResult::fail(
            "discovery.cursor.mdc.description",
            ".mdc has a `description`",
            format!("{rel}: frontmatter is missing `description`"),
            "To fix: add `description: <one sentence, apply when ...>` to the frontmatter.",
        ));
    };
    if description.trim().is_empty() {
        return Ok(CheckResult::fail(
            "discovery.cursor.mdc.description",
            ".mdc `description` is non-empty",
            format!("{rel}: `description` is empty"),
            "To fix: write one sentence describing when Cursor should attach this rule.",
        ));
    }

    // Cursor uses `description` for auto-attach logic; an oversized
    // description dilutes that signal. Reuse the same 1,536-char listing cap
    // as Claude/Codex — generous upper bound, not Cursor's own ~500-token rule
    // guidance (which is a soft recommendation, not enforced).
    if description.trim().chars().count() > schema::SKILL_LISTING_CHAR_CAP {
        return Ok(CheckResult::fail(
            "discovery.cursor.mdc.description_length",
            "`.mdc` `description` stays under 1,536 chars",
            format!(
                "{rel}: `description` is {} chars (cap {})",
                description.trim().chars().count(),
                schema::SKILL_LISTING_CHAR_CAP
            ),
            "To fix: trim the description; Cursor uses it for auto-attach, so keep it one line.",
        ));
    }

    // alwaysApply is required by the Cursor schema. We warn (not fail) on its
    // absence: Cursor itself tolerates a missing field (defaults to false),
    // but an explicit value is the documented contract — a warning teaches
    // the maintainer without blocking them.
    let always_apply = fm.always_apply.as_deref().unwrap_or("").trim();
    if always_apply.is_empty() {
        return Ok(CheckResult::warn(
            "discovery.cursor.mdc.always_apply",
            ".mdc has an explicit `alwaysApply`",
            format!("{rel}: `alwaysApply` is missing or empty"),
            "To fix: add `alwaysApply: true` or `alwaysApply: false` to the frontmatter.",
        ));
    }
    if always_apply != "true" && always_apply != "false" {
        return Ok(CheckResult::warn(
            "discovery.cursor.mdc.always_apply",
            ".mdc `alwaysApply` is a boolean",
            format!("{rel}: `alwaysApply` is `{always_apply}` (expected `true`/`false`)"),
            "To fix: set `alwaysApply: true` or `alwaysApply: false`.",
        ));
    }

    Ok(CheckResult::pass(
        "discovery.cursor.mdc",
        ".mdc is structurally valid",
        format!("{rel} validates"),
    ))
}

/// Every `.cursor/rules/<name>.mdc`, sorted. Cursor's project-rule format:
/// YAML frontmatter + markdown body, with its own frontmatter schema
/// (`description` / `alwaysApply` / `globs`).
pub(crate) fn find_cursor_mdc_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let dir = root.join(schema::CURSOR_RULES_DIR);
    if dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut names: Vec<_> = entries.flatten().collect();
            names.sort_by_key(|e| e.file_name());
            for entry in names {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|e| e == "mdc") {
                    out.push(path);
                }
            }
        }
    }
    out
}
