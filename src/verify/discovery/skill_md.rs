//! SKILL.md parsing + structural checks shared by the Claude and Codex
//! distributions: frontmatter parsing (`SkillFrontmatter`), the
//! per-skill check, and the skill-file finders.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::find_kv_colon;
use super::rel_unix;
use super::schema;
use super::strip_bom;
use crate::verify::result::CheckResult;

// ----- SKILL.md -------------------------------------------------------------

/// Parse the YAML frontmatter out of a SKILL.md. Returns the frontmatter as a
/// `serde_json::Value` (parsing YAML loosely via serde_yaml-free path: we use a
/// tiny hand parser for the few keys we care about, to avoid a heavy YAML
/// dependency). Exposed for unit tests.
///
/// `closed` on the returned struct is false when the opening `---` was found
/// but no closing `---` delimiter appeared before EOF — the caller can fail
/// the file honestly instead of treating the swallowed body as frontmatter.
pub fn parse_skill_frontmatter(raw: &str) -> Option<SkillFrontmatter> {
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
    let mut fm = SkillFrontmatter::parse(&body);
    fm.closed = closed;
    Some(fm)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub allowed_tools: Option<String>,
    /// True when the `---` block was terminated by a closing `---` line.
    /// False for an unterminated block (or when constructed via `Default`
    /// from a file with no frontmatter at all — the caller distinguishes
    /// the two via `parse_skill_frontmatter` returning `None`).
    pub closed: bool,
}

impl SkillFrontmatter {
    pub(crate) fn parse(block: &str) -> Self {
        let mut fm = Self::default();
        let mut current_key: Option<String> = None;
        let mut current_val = String::new();
        for line in block.lines() {
            let trimmed = line.trim_end();
            // New `key: value` line starts a new key (we don't handle nested
            // blocks; the keys we care about are all scalar).
            if let Some(idx) = find_kv_colon(trimmed) {
                // Flush previous.
                if let Some(k) = current_key.take() {
                    let clean = current_val.trim().trim_matches('"').trim();
                    store(&mut fm, &k, clean);
                    current_val.clear();
                }
                let key = trimmed[..idx].trim().to_string();
                let val = trimmed[idx + 1..].trim().to_string();
                current_key = Some(key);
                current_val = val;
            } else if !trimmed.is_empty() && current_key.is_some() {
                // Continuation line for a block-ish value; append.
                current_val.push('\n');
                current_val.push_str(trimmed);
            }
        }
        if let Some(k) = current_key.take() {
            let clean = current_val.trim().trim_matches('"').trim();
            store(&mut fm, &k, clean);
        }
        fm
    }
}

fn store(fm: &mut SkillFrontmatter, key: &str, val: &str) {
    match key {
        "name" => fm.name = Some(val.to_string()),
        "description" => fm.description = Some(val.to_string()),
        "when_to_use" => fm.when_to_use = Some(val.to_string()),
        "allowed-tools" => fm.allowed_tools = Some(val.to_string()),
        _ => {}
    }
}

pub(crate) fn check_one_skill_md(
    root: &Path,
    path: &Path,
    prefix: &str,
    profile_name: &Option<String>,
    allowed_skill_names: &std::collections::HashSet<String>,
) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let raw = strip_bom(&raw);
    let rel = rel_unix(root, path);

    // A file with no frontmatter at all → the existing description-missing
    // fail (kept byte-for-byte so hand-written skills degrade the same way).
    let Some(fm) = parse_skill_frontmatter(raw) else {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description"),
            "SKILL.md has a `description`",
            format!("{rel}: frontmatter is missing `description`"),
            "To fix: add `description: <one sentence, use when ...>` to the frontmatter.",
        ));
    };

    // An unterminated `---` block swallows the whole body as frontmatter —
    // the description checks below could otherwise pass on a malformed file.
    // Fail BEFORE the field checks so the structural defect is what surfaces.
    if !fm.closed {
        return Ok(CheckResult::fail(
            &format!("{prefix}.frontmatter_unclosed"),
            "SKILL.md frontmatter block is closed by a `---` delimiter",
            format!("{rel}: frontmatter block is not closed (missing the closing `---`)"),
            "To fix: add a closing `---` line after the last frontmatter field.",
        ));
    }

    // description present.
    let Some(description) = fm.description.as_deref() else {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description"),
            "SKILL.md has a `description`",
            format!("{rel}: frontmatter is missing `description`"),
            "To fix: add `description: <one sentence, use when ...>` to the frontmatter.",
        ));
    };
    if description.trim().is_empty() {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description"),
            "SKILL.md `description` is non-empty",
            format!("{rel}: `description` is empty (it may be missing, or the line `description:` has no value on the same line, e.g. a nested map)"),
            "To fix: write one sentence describing the skill on the SAME line as `description:`, e.g. `description: Generate an agent skill pack`. Avoid indented YAML blocks under `description:`.",
        ));
    }

    // combined description + when_to_use under the 1,536 listing cap.
    let when = fm.when_to_use.as_deref().unwrap_or("").trim();
    let combined = if when.is_empty() {
        description.trim().to_string()
    } else {
        format!("{} {}", description.trim(), when)
    };
    if combined.chars().count() > schema::SKILL_LISTING_CHAR_CAP {
        return Ok(CheckResult::fail(
            &format!("{prefix}.description_length"),
            "combined description + when_to_use stays under 1,536 chars",
            format!(
                "{rel}: combined description + when_to_use is {} chars (cap {})",
                combined.chars().count(),
                schema::SKILL_LISTING_CHAR_CAP
            ),
            "To fix: trim your description/when_to_use; the first sentence is what the agent sees first.",
        ));
    }

    // name: if present, kebab + <=64 chars. MUST be before name_drift (a WARN)
    // so a long or reserved name FAILs rather than being shadowed by the
    // drift WARN's early return — same ordering invariant the 0.9.2 fix
    // locked down for description (name_drift_warn_does_not_shadow_description_fail).
    if let Some(name) = fm.name.as_deref() {
        if name.chars().count() > schema::SKILL_NAME_MAX_CHARS {
            return Ok(CheckResult::fail(
                &format!("{prefix}.name_length"),
                "SKILL.md `name` is ≤ 64 characters",
                format!(
                    "{rel}: `name` is {} chars (max {})",
                    name.chars().count(),
                    schema::SKILL_NAME_MAX_CHARS
                ),
                "To fix: shorten the skill name.",
            ));
        }
        if schema::RESERVED_NAMES.contains(&name) {
            return Ok(CheckResult::warn(
                &format!("{prefix}.name_reserved"),
                "SKILL.md name is not reserved",
                format!("{rel}: skill name `{name}` is a reserved name"),
                "To fix: pick a non-Anthropic-owned name.",
            ));
        }
    }

    // name_drift: frontmatter `name:` (if present) must match the canonical
    // project name — `coerce_kebab(profile.name)`, the same value the SKILL.md
    // template renders. `init` writes them in sync; drift means a hand-edited
    // frontmatter or a renamed project repo/manifest that wasn't regenerated.
    // Warn (not fail): a maintainer may intentionally ship a skill under a
    // divergent name. Placed AFTER ALL fail-severity checks (description,
    // description_empty, description_length, name_length) so a structurally
    // broken skill surfaces its fail first — drift is a warn and must not
    // shadow a fail. Skipped when either side is absent (no frontmatter name,
    // or introspection couldn't derive a canonical name). A name matching a
    // configured `[[skills]]` entry is legitimate (multi-skill packs) and not
    // drift.
    if let (Some(fm_name), Some(canonical)) = (fm.name.as_deref(), profile_name.as_deref()) {
        if fm_name != canonical && !allowed_skill_names.contains(fm_name) {
            let mut r = CheckResult::warn(
                &format!("{prefix}.name_drift"),
                "SKILL.md `name` matches the canonical project name",
                format!("{rel}: `name: {fm_name}` != canonical `{canonical}`"),
                "To fix: run `skillpack verify --fix` to regenerate the frontmatter, or re-run `skillpack init` to refresh the whole skill, or intentionally pin the skill name.",
            );
            r.location = Some((rel.clone(), None));
            return Ok(r);
        }
    }

    // Skill-directory vs frontmatter name: agents load skills by DIRECTORY
    // (`skills/<name>/SKILL.md` / `.claude/skills/<name>/SKILL.md` /
    // `.codex/skills/<name>/SKILL.md`), so a directory that disagrees with
    // the advertised `name:` is a discoverability defect even when the name
    // itself matches the canonical project name. Only fires for paths whose
    // grandparent is a `skills` directory (the native `.claude/skills/` grand-
    // parent is named `skills` too) — a root `SKILL.md` has no skill directory
    // to disagree with. Warn (not fail): some agents read the frontmatter
    // `name` and ignore the path.
    let under_skills_dir = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|gp| gp.file_name())
        .is_some_and(|n| n == "skills");
    if under_skills_dir {
        if let (Some(dir), Some(fm_name)) = (
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            fm.name.as_deref(),
        ) {
            if dir != fm_name {
                let mut r = CheckResult::warn(
                    &format!("{prefix}.dir_name_mismatch"),
                    "skill directory name matches its frontmatter `name`",
                    format!("{rel}: directory `{dir}` != frontmatter `name: {fm_name}`"),
                    "To fix: rename the skill directory (or the frontmatter `name`) so they match; agents load skills by directory.",
                );
                r.location = Some((rel.clone(), None));
                return Ok(r);
            }
        }
    }

    // description leads with an alpha word (action-verb heuristic).
    let first_word = combined.split_whitespace().next().unwrap_or("");
    // A description may legitimately open with a code-formatted name
    // (`` `fd` is a program to ... ``) — strip a wrapping pair of backticks
    // before the alpha heuristic so the check rewards real prose instead of
    // punishing code fences. Badge/markup leakage (``[![CICD]...]``) still
    // fails: its first token has no alpha core.
    let word_core = first_word
        .strip_prefix('`')
        .and_then(|w| w.strip_suffix('`'))
        .unwrap_or(first_word);
    let starts_alpha = word_core.chars().next().is_some_and(char::is_alphabetic);
    if !starts_alpha {
        return Ok(CheckResult::warn(
            &format!("{prefix}.description_action_verb"),
            "SKILL.md description leads with an action",
            format!("{rel}: description does not start with a word (got `{first_word}`)"),
            "To fix: lead with an action verb (e.g. \"Generate ...\", \"Lint ...\") so the agent knows what this does.",
        ));
    }

    // when_to_use present and non-empty (the trigger field is what makes the
    // skill discoverable to the model).
    if fm
        .when_to_use
        .as_deref()
        .is_none_or(|w| w.trim().is_empty())
    {
        let mut r = CheckResult::warn(
            &format!("{prefix}.when_to_use"),
            "SKILL.md has non-empty `when_to_use` trigger phrases",
            format!("{rel}: `when_to_use` is missing or empty"),
            "To fix: add 2-5 trigger verbs/scenarios to `when_to_use_phrases` in \
             skillpack.toml (or re-run `skillpack init`), then run `skillpack \
             verify --fix` — the fix regenerates the frontmatter FROM the \
             config, so the config must carry the phrases first.",
        );
        r.location = Some((rel.clone(), None));
        return Ok(r);
    }

    // allowed_tools grammar (apply only when the field is present). The
    // Anthropic spec describes a GRAMMAR (comma-separated tokens, each either a
    // bare identifier like `Read` or a namespaced call like `Bash(npm test:*)`),
    // not an enumerated allowlist. Validating membership would false-fail the
    // moment Anthropic ships a new tool, so we validate the grammar shape only:
    // each comma-split token non-empty + matches `^[A-Za-z]+(\([^)]*\))?$`.
    // Warn severity — malformed tools degrade discoverability but don't gate.
    if let Some(tools) = fm.allowed_tools.as_deref() {
        let bad: Vec<&str> = tools
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty() && !is_valid_allowed_tool_token(t))
            .collect();
        if !bad.is_empty() {
            let mut r = CheckResult::warn(
                &format!("{prefix}.allowed_tools"),
                "SKILL.md `allowed-tools` tokens match the Anthropic grammar",
                format!(
                    "{rel}: `allowed-tools` has malformed token(s): {}",
                    bad.iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "To fix: set `allowed_tools` in skillpack.toml to a valid value \
                 (e.g. `Read, Bash` — each comma-separated token must be a bare \
                 identifier like `Read` or a namespaced call like \
                 `Bash(npm test:*)`), then run `skillpack verify --fix`; the fix \
                 regenerates the frontmatter FROM the config.",
            );
            r.location = Some((rel.clone(), None));
            return Ok(r);
        }
    }

    Ok(CheckResult::pass(
        prefix,
        "SKILL.md is structurally valid",
        format!("{rel} validates"),
    ))
}

// ----- helpers --------------------------------------------------------------

/// Every SKILL.md under `skills/*/SKILL.md` AND the native
/// `.claude/skills/*/SKILL.md` directory, plus a root `SKILL.md`, sorted for
/// deterministic verification (read_dir order is unspecified). A plugin may
/// legitimately ship multiple skills (Improvement C).
pub(crate) fn find_skill_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for skills_dir in ["skills", schema::CLAUDE_SKILLS_DIR] {
        let dir = root.join(skills_dir);
        if dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&dir) {
                let mut names: Vec<_> = entries.flatten().collect();
                names.sort_by_key(|e| e.file_name());
                for entry in names {
                    let candidate = entry.path().join("SKILL.md");
                    if candidate.is_file() {
                        out.push(candidate);
                    }
                }
            }
        }
    }
    let root_skill = root.join("SKILL.md");
    if root_skill.is_file() {
        out.push(root_skill);
    }
    out
}

/// Validate an `allowed-tools` token against the Anthropic grammar:
/// a bare identifier (`Read`, `Grep`, `mcp__github__create_issue`) or a
/// namespaced call (`Bash(npm test:*)`, `Edit(*)`). Returns false for empty,
/// malformed, or unbalanced-paren tokens.
pub(crate) fn is_valid_allowed_tool_token(token: &str) -> bool {
    let t = token.trim();
    if t.is_empty() {
        return false;
    }
    // Bare identifier: alphanumeric, underscore, or hyphen (e.g. Read, mcp__server__tool).
    if is_valid_tool_ident(t) {
        return true;
    }
    // Namespaced call: `Name(args)`. Split on the first `(`.
    let Some(open) = t.find('(') else {
        return false;
    };
    let (name, rest) = t.split_at(open);
    if !is_valid_tool_ident(name) {
        return false;
    }
    let rest = rest.strip_prefix('(').unwrap_or(rest);
    // Must end with `)` and contain the args (any chars except `)` inside).
    if !rest.ends_with(')') {
        return false;
    }
    let inner = &rest[..rest.len() - 1];
    if inner.contains(')') {
        return false;
    }
    true
}

fn is_valid_tool_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Every `.codex/skills/<name>/SKILL.md`, sorted. Same frontmatter shape
/// as Claude's `skills/<name>/SKILL.md` but a distinct output path per
/// Codex's `.codex/skills/` convention (design §3 Phase 4).
pub(crate) fn find_codex_skill_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let dir = root.join(schema::CODEX_SKILLS_DIR);
    if dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut names: Vec<_> = entries.flatten().collect();
            names.sort_by_key(|e| e.file_name());
            for entry in names {
                let candidate = entry.path().join("SKILL.md");
                if candidate.is_file() {
                    out.push(candidate);
                }
            }
        }
    }
    out
}
