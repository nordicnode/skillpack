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
///   alwaysApply:  `<bool>`                  — required
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CursorFrontmatter {
    pub description: Option<String>,
    pub always_apply: Option<String>,
    /// True when the `---` block was terminated by a closing `---` line.
    /// False for an unterminated block (or `Default` when the caller found
    /// no leading `---` at all — `parse_cursor_mdc_frontmatter` returns
    /// `None` for that case, so the two are distinguishable).
    pub closed: bool,
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
                    let clean = current_val.trim().trim_matches('"').trim();
                    store_cursor(&mut fm, &k, clean);
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
            let clean = current_val.trim().trim_matches('"').trim();
            store_cursor(&mut fm, &k, clean);
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
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    let mut fm = CursorFrontmatter::parse(&body);
    fm.closed = closed;
    Some(fm)
}

/// Validate a single `.cursor/rules/<name>.mdc` against Cursor's documented
/// schema. Path-name consistency (kebab-ish) is warned, not failed — Cursor
/// itself doesn't enforce it, but a name like `My Rule.mdc` is a maintenance
/// smell.
pub(crate) fn check_one_mdc(root: &Path, path: &Path) -> Result<CheckResult> {
    check_one_rule_md(root, path, "discovery.cursor.mdc", ".mdc", "Cursor")
}

/// Validate a single `.windsurf/rules/<name>.md` — the Windsurf (Cascade)
/// rules format uses the SAME frontmatter schema as Cursor rules, so the
/// checks share one implementation with a different check_id prefix and
/// label.
pub(crate) fn check_one_windsurf_rule(root: &Path, path: &Path) -> Result<CheckResult> {
    check_one_rule_md(root, path, "discovery.windsurf.rule", ".md", "Windsurf")
}

/// Shared rule-file check for Cursor `.mdc` and Windsurf `.md` (identical
/// `description`/`globs`/`alwaysApply` frontmatter schemas). `prefix` is the
/// check_id namespace, `ext` the file-extension label in messages, `product`
/// the harness name in fix hints.
fn check_one_rule_md(
    root: &Path,
    path: &Path,
    prefix: &str,
    ext: &str,
    product: &str,
) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let raw = super::strip_bom(&raw);
    let rel = rel_unix(root, path);

    // No frontmatter at all → the existing description-missing fail.
    let Some(fm) = parse_cursor_mdc_frontmatter(raw) else {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description"),
            &format!("{ext} has a `description`"),
            format!("{rel}: frontmatter is missing `description`"),
            "To fix: add `description: <one sentence, apply when ...>` to the frontmatter.",
        ));
    };

    // Unterminated `---` block → the body is being parsed as frontmatter.
    if !fm.closed {
        return Ok(CheckResult::fail(
            &format!("{prefix}.frontmatter_unclosed"),
            &format!("{ext} frontmatter block is closed by a `---` delimiter"),
            format!("{rel}: frontmatter block is not closed (missing the closing `---`)"),
            "To fix: add a closing `---` line after the last frontmatter field.",
        ));
    }

    let Some(description) = fm.description.as_deref() else {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description"),
            &format!("{ext} has a `description`"),
            format!("{rel}: frontmatter is missing `description`"),
            "To fix: add `description: <one sentence, apply when ...>` to the frontmatter.",
        ));
    };
    if description.trim().is_empty() {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description"),
            &format!("{ext} `description` is non-empty"),
            format!("{rel}: `description` is empty"),
            format!(
                "To fix: write one sentence describing when {product} should attach this rule."
            ),
        ));
    }

    // Cursor uses `description` for auto-attach logic; an oversized
    // description dilutes that signal. Reuse the same 1,536-char listing cap
    // as Claude/Codex — generous upper bound, not Cursor's own ~500-token rule
    // guidance (which is a soft recommendation, not enforced).
    if description.trim().chars().count() > schema::SKILL_LISTING_CHAR_CAP {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description_length"),
            &format!("`{ext}` `description` stays under 1,536 chars"),
            format!(
                "{rel}: `description` is {} chars (cap {})",
                description.trim().chars().count(),
                schema::SKILL_LISTING_CHAR_CAP
            ),
            format!(
                "To fix: trim the description; {product} uses it for auto-attach, so keep it one line."
            ),
        ));
    }

    // alwaysApply is required by the rule schema. We warn (not fail) on its
    // absence: the harness itself tolerates a missing field (defaults to
    // false), but an explicit value is the documented contract — a warning
    // teaches the maintainer without blocking them.
    let always_apply = fm.always_apply.as_deref().unwrap_or("").trim();
    if always_apply.is_empty() {
        return Ok(CheckResult::warn(
            &format!("{prefix}.always_apply"),
            &format!("{ext} has an explicit `alwaysApply`"),
            format!("{rel}: `alwaysApply` is missing or empty"),
            "To fix: add `alwaysApply: true` or `alwaysApply: false` to the frontmatter.",
        ));
    }
    if always_apply != "true" && always_apply != "false" {
        return Ok(CheckResult::warn(
            &format!("{prefix}.always_apply"),
            &format!("{ext} `alwaysApply` is a boolean"),
            format!("{rel}: `alwaysApply` is `{always_apply}` (expected `true`/`false`)"),
            "To fix: set `alwaysApply: true` or `alwaysApply: false`.",
        ));
    }

    Ok(CheckResult::pass(
        prefix,
        &format!("{ext} is structurally valid"),
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

/// Every `.windsurf/rules/<name>.md`, sorted — the Windsurf rule directory
/// (same frontmatter shape as Cursor, `.md` extension).
pub(crate) fn find_windsurf_rule_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let dir = root.join(schema::WINDSURF_RULES_DIR);
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
