//! Discovery checks — structural validation against the documented Claude Code
//! plugin schema. Pure functions; the only I/O is the file reads the caller
//! hands us. See [`crate::verify::schema`] for the cited source of each rule.
//!
//! Checks run against the *generated* files, but `verify` also accepts
//! hand-written files written without `init` (design §4.2), so every check
//! must degrade gracefully on missing/empty inputs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;

use super::result::CheckResult;
use super::schema;

static NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(schema::NAME_KEBAB_REGEX).expect("compiled constant regex"));
use super::super::introspect::{detect_language, project_manifest_version};
use crate::types::DiagTrace;

mod copilot;
mod cursor;
mod opencode;

use copilot::{check_copilot_instructions, find_copilot_instructions};
use cursor::{check_one_mdc, find_cursor_mdc_files};
pub use cursor::{parse_cursor_mdc_frontmatter, CursorFrontmatter};
use opencode::{check_one_opencode_agent, find_opencode_agent_files};
pub use opencode::{parse_opencode_agent_frontmatter, OpenCodeFrontmatter};

/// Render `path` as a forward-slash-separated string relative-ish to `root`.
/// Windows `Path` uses `\`, but the verify report and snapshot paths are
/// cross-OS canonical — marketplace.json schema requires `./` + forward
/// slashes only, and a `\` in the human/JSON report would break downstream
/// tools + snapshot equality. Strips `root` prefix when present.
pub(crate) fn rel_unix(root: &Path, path: &Path) -> String {
    let stripped = path.strip_prefix(root).unwrap_or(path);
    stripped.to_string_lossy().replace('\\', "/")
}

/// Run every discovery check, returning one [`CheckResult`] per check.
///
/// `root` is the plugin root (e.g. the dir containing `.claude-plugin/`,
/// `.cursor/rules/`, or `.codex/skills/`). Each ecosystem present is checked
/// independently — discovery degrades gracefully when an ecosystem's files
/// are absent (a `--target cursor`-only pack shouldn't fail on a missing
/// `.claude-plugin/`).
pub fn run(root: &Path) -> Result<Vec<CheckResult>> {
    let mut out = Vec::new();

    // Claude Code: marketplace.json + plugin.json + skills/<name>/SKILL.md.
    // The marketplace/plugin checks only run when the Claude distribution is
    // present — a `--target cursor`-only pack legitimately has no
    // `.claude-plugin/` and must not fail on its absence.
    if claude_present(root) {
        out.push(check_marketplace(root)?);
        out.push(check_plugin_json(root)?);
        let skills = find_skill_files(root);
        if skills.is_empty() {
            out.push(CheckResult::fail(
                "discovery.skill.missing",
                "a SKILL.md exists (skills/<name>/SKILL.md or root)",
                "no SKILL.md found",
                "To fix: run `skillpack init`, or add skills/<your-tool>/SKILL.md.",
            ));
        } else {
            for skill_path in skills {
                out.push(check_one_skill_md(root, &skill_path, "discovery.skill")?);
            }
        }
    }

    // Codex CLI: `.codex/skills/<name>/SKILL.md` — same frontmatter shape as
    // Claude, different output path and check_id prefix.
    let codex_skills = find_codex_skill_files(root);
    if codex_skills.is_empty() && root.join(schema::CODEX_SKILLS_DIR).is_dir() {
        out.push(CheckResult::fail(
            "discovery.codex.skill.missing",
            "at least one Codex skill exists",
            ".codex/skills/ exists but contains no SKILL.md",
            "To fix: run `skillpack init --target codex` or add a skill under .codex/skills/<name>/SKILL.md.",
        ));
    } else {
        for skill_path in codex_skills {
            out.push(check_one_skill_md(
                root,
                &skill_path,
                "discovery.codex.skill",
            )?);
        }
    }

    // Cursor: `.cursor/rules/<name>.mdc` — distinct frontmatter
    // (`description` / `alwaysApply` / `globs`).
    let cursor_mdcs = find_cursor_mdc_files(root);
    if cursor_mdcs.is_empty() && root.join(schema::CURSOR_RULES_DIR).is_dir() {
        out.push(CheckResult::fail(
            "discovery.cursor.mdc.missing",
            "at least one Cursor rule exists",
            ".cursor/rules/ exists but contains no .mdc file",
            "To fix: run `skillpack init --target cursor` or add a rule under .cursor/rules/<name>.mdc.",
        ));
    } else {
        for mdc_path in cursor_mdcs {
            out.push(check_one_mdc(root, &mdc_path)?);
        }
    }

    // OpenCode: `.opencode/agents/<name>.md` — frontmatter `description`
    // (required), `mode` (optional). Reuses the same `---`-delimited YAML
    // parser as Cursor.mdc; the per-key struct differs.
    let opencode_agents = find_opencode_agent_files(root);
    if opencode_agents.is_empty() && root.join(schema::OPENCODE_AGENTS_DIR).is_dir() {
        out.push(CheckResult::fail(
            "discovery.opencode.agent.missing",
            "at least one OpenCode agent exists",
            ".opencode/agents/ exists but contains no agent file",
            "To fix: run `skillpack init --target opencode` or add an agent under .opencode/agents/<name>.md.",
        ));
    } else {
        for agent_path in opencode_agents {
            out.push(check_one_opencode_agent(root, &agent_path)?);
        }
    }

    // GitHub Copilot: `.github/copilot-instructions.md` — plain markdown,
    // no frontmatter. Validation is structural: file exists, non-empty, and
    // starts with a `#` heading.
    if let Some(p) = find_copilot_instructions(root) {
        out.push(check_copilot_instructions(root, &p)?);
    }

    // When no ecosystem files are present at all, the plugin is malformed —
    // emit a single honest failure so a bare `skillpack verify` on an empty
    // repo doesn't silently pass.
    if out.is_empty() {
        out.push(CheckResult::fail(
            "discovery.empty",
            "at least one ecosystem is present (Claude / Codex / Cursor / OpenCode / Copilot)",
            "no distribution files found (none of: .claude-plugin/, .codex/skills/, .cursor/rules/, .opencode/agents/, .github/copilot-instructions.md)",
            "To fix: run `skillpack init --target <ecosystem>` first.",
        ));
    }

    Ok(out)
}

/// True if the Claude Code distribution files (`.claude-plugin/`) are present.
fn claude_present(root: &Path) -> bool {
    root.join(schema::CLAUDE_PLUGIN_DIR).is_dir()
}

// ----- marketplace.json ------------------------------------------------------

fn check_marketplace(root: &Path) -> Result<CheckResult> {
    let path = root.join(schema::MARKETPLACE_JSON_PATH);
    let raw = match read_optional(&path)? {
        Some(r) => r,
        None => {
            return Ok(CheckResult::fail(
                "discovery.marketplace.missing",
                "marketplace.json exists and is valid JSON",
                format!(
                    "{} not found — re-run `skillpack init` or check the path",
                    schema::MARKETPLACE_JSON_PATH
                ),
                format!(
                    "To fix: create {} at the project root and re-run `skillpack verify`.",
                    schema::MARKETPLACE_JSON_PATH
                ),
            ));
        }
    };

    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(CheckResult::fail(
                "discovery.marketplace.json",
                "marketplace.json is valid JSON",
                format!("marketplace.json does not parse: {e}"),
                "To fix: correct the JSON syntax (trailing comma? unquoted key?) and re-run.",
            ));
        }
    };

    // Required: name (kebab), owner, plugins (non-empty).
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    if name.is_empty() {
        return Ok(CheckResult::fail(
            "discovery.marketplace.name",
            "marketplace.json has a kebab-case `name`",
            "marketplace.json is missing `name`",
            "To fix: add a `\"name\": \"your-marketplace-name\"` field (kebab-case, no spaces).",
        ));
    }
    if !is_valid_kebab(name) {
        return Ok(CheckResult::fail(
            "discovery.marketplace.name",
            "marketplace.json has a kebab-case `name`",
            format!("marketplace name `{name}` is not kebab-case"),
            "To fix: use lowercase letters, digits, and hyphens only; start and end with a letter/digit; no consecutive hyphens.",
        ));
    }
    if schema::RESERVED_NAMES.contains(&name) {
        return Ok(CheckResult::warn(
            "discovery.marketplace.name",
            "marketplace.json name not reserved",
            format!("marketplace name `{name}` is on the Anthropic reserved-names blocklist"),
            "To fix: rename to something not owned by Anthropic (e.g. prefix with your org).",
        ));
    }

    if v.get("owner").map_or(true, |o| {
        o.is_null() || o == &serde_json::Value::Object(Default::default())
    }) {
        return Ok(CheckResult::fail(
            "discovery.marketplace.owner",
            "marketplace.json has an `owner` object",
            "marketplace.json is missing `owner`",
            "To fix: add `\"owner\": { \"name\": \"Your Name\" }`.",
        ));
    }

    let plugins = match v.get("plugins").and_then(|p| p.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            return Ok(CheckResult::fail(
                "discovery.marketplace.plugins",
                "marketplace.json has a non-empty `plugins` array",
                "marketplace.json `plugins` is missing or empty",
                "To fix: add at least one plugin entry with `name` and `source`.",
            ));
        }
    };

    // Each plugin entry: name (kebab, not reserved) + source (./ prefix for
    // relative paths).
    for (i, entry) in plugins.iter().enumerate() {
        let pname = entry.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if pname.is_empty() {
            return Ok(CheckResult::fail(
                "discovery.marketplace.plugin_name",
                "every marketplace plugin entry has a kebab-case `name`",
                format!("plugin entry #{i} is missing `name`"),
                "To fix: add a `\"name\": \"...\"` (kebab-case) to the entry.",
            ));
        }
        if !is_valid_kebab(pname) {
            return Ok(CheckResult::fail(
                "discovery.marketplace.plugin_name",
                "every marketplace plugin entry has a kebab-case `name`",
                format!("plugin name `{pname}` is not kebab-case"),
                "To fix: lowercase letters/digits/hyphens only, no consecutive hyphens.",
            ));
        }

        let src = entry.get("source");
        match src {
            Some(serde_json::Value::String(s)) => {
                if let Err(reason) = validate_relative_source(s) {
                    return Ok(CheckResult::fail(
                        "discovery.marketplace.source",
                        "relative plugin `source` paths start with `./` and avoid `../`",
                        format!("plugin `{pname}` has an invalid source `{s}`: {reason}"),
                        "To fix: make the source a path that starts with `./`, uses forward slashes, and stays inside the repo (no `../`).",
                    ));
                }
            }
            Some(serde_json::Value::Object(_obj)) => {
                // github/url/git-subdir/npm/pip — we don't deep-validate remote
                // source objects in V1; flag only if clearly malformed.
            }
            _ => {
                return Ok(CheckResult::fail(
                    "discovery.marketplace.source",
                    "every marketplace plugin entry has a `source`",
                    format!("plugin `{pname}` is missing `source`"),
                    "To fix: add `\"source\": \"./\"` (local) or a github/url object.",
                ));
            }
        }
    }

    Ok(CheckResult::pass(
        "discovery.marketplace",
        "marketplace.json is structurally valid",
        format!(
            "{} validates ({} plugin(s))",
            schema::MARKETPLACE_JSON_PATH,
            plugins.len()
        ),
    ))
}

// ----- plugin.json ----------------------------------------------------------

fn check_plugin_json(root: &Path) -> Result<CheckResult> {
    let path = root.join(schema::PLUGIN_JSON_PATH);
    let raw = match read_optional(&path)? {
        Some(r) => r,
        None => {
            return Ok(CheckResult::fail(
                "discovery.plugin.missing",
                "plugin.json exists and is valid JSON",
                format!("{} not found", schema::PLUGIN_JSON_PATH),
                "To fix: run `skillpack init`; the manifest lives at .claude-plugin/plugin.json.",
            ));
        }
    };

    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(CheckResult::fail(
                "discovery.plugin.json",
                "plugin.json is valid JSON",
                format!("plugin.json does not parse: {e}"),
                "To fix: fix the JSON syntax and re-run.",
            ));
        }
    };

    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    if name.is_empty() {
        return Ok(CheckResult::fail(
            "discovery.plugin.name",
            "plugin.json has a kebab-case `name`",
            "plugin.json is missing `name`",
            "To fix: add `\"name\": \"your-plugin-name\"` (kebab-case).",
        ));
    }
    if !is_valid_kebab(name) {
        return Ok(CheckResult::fail(
            "discovery.plugin.name",
            "plugin.json name is kebab-case",
            format!("plugin name `{name}` is not kebab-case"),
            "To fix: lowercase letters/digits/hyphens only, no consecutive hyphens.",
        ));
    }

    // description (optional but recommended) and author (optional).
    // We don't hard-fail on missing author (the docs say it's optional), but a
    // missing description on a plugin is a real discoverability problem for an
    // agent — warn, don't fail.
    if v.get("description")
        .map_or(true, |d| d.as_str().map_or(true, str::is_empty))
    {
        return Ok(CheckResult::warn(
            "discovery.plugin.description",
            "plugin.json has a `description`",
            "plugin.json has no `description`",
            "To fix: add a one-line `\"description\": \"...\"`; Claude shows this in the plugin manager.",
        ));
    }

    let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("");
    if version.is_empty() {
        return Ok(CheckResult::warn(
            "discovery.plugin.version",
            "plugin.json has a `version`",
            "plugin.json has no `version`",
            "To fix: set `version` in your manifest (Cargo.toml [package].version, package.json \"version\", pyproject.toml [project].version); then re-run `skillpack init`.",
        ));
    }

    // Version drift: plugin.json version should match the project manifest
    // version. `init` writes plugin.json from the manifest, so drift means a
    // stale hand-edited plugin.json or a manifest bump that wasn't regenerated.
    // We warn (not fail) — maintainers may intentionally pin a different
    // plugin version (e.g. a pre-release plugin for a stable library).
    let mut diag = DiagTrace::default();
    let lang = detect_language(root, &mut diag);
    if let Some(mv) = project_manifest_version(root, lang) {
        if mv != version {
            return Ok(CheckResult::warn(
                "discovery.plugin.version_drift",
                "plugin.json version matches the project manifest version",
                format!("plugin.json version `{version}` != manifest version `{mv}`"),
                "To fix: re-run `skillpack init` to regenerate plugin.json from the manifest, or intentionally pin the plugin version.",
            ));
        }
    }

    let author = v
        .get("author")
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");
    if author.is_empty() || author == "Unspecified" {
        return Ok(CheckResult::warn(
            "discovery.plugin.author",
            "plugin.json has a real `author`",
            "plugin.json has no author (or defaults to \"Unspecified\")",
            "To fix: set `authors` in your manifest (Cargo.toml [package].authors, package.json \"author\", pyproject.toml [project].authors, *.gemspec spec.authors), or pass --author; then re-run `skillpack init`.",
        ));
    }

    Ok(CheckResult::pass(
        "discovery.plugin",
        "plugin.json is structurally valid",
        format!(
            "{} validates (name={name}, version={version}, author={author})",
            schema::PLUGIN_JSON_PATH
        ),
    ))
}

// ----- SKILL.md -------------------------------------------------------------

/// Parse the YAML frontmatter out of a SKILL.md. Returns the frontmatter as a
/// `serde_json::Value` (parsing YAML loosely via serde_yaml-free path: we use a
/// tiny hand parser for the few keys we care about, to avoid a heavy YAML
/// dependency). Exposed for unit tests.
pub fn parse_skill_frontmatter(raw: &str) -> Option<SkillFrontmatter> {
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
    Some(SkillFrontmatter::parse(&body))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub allowed_tools: Option<String>,
}

impl SkillFrontmatter {
    fn parse(block: &str) -> Self {
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
                    store(&mut fm, &k, current_val.trim());
                    current_val.clear();
                }
                let key = trimmed[..idx].trim().to_string();
                let val = trimmed[idx + 1..].trim().trim_matches('"').to_string();
                current_key = Some(key);
                current_val = val;
            } else if !trimmed.is_empty() && current_key.is_some() {
                // Continuation line for a block-ish value; append.
                current_val.push('\n');
                current_val.push_str(trimmed);
            }
        }
        if let Some(k) = current_key.take() {
            store(&mut fm, &k, current_val.trim());
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

pub(crate) fn find_kv_colon(line: &str) -> Option<usize> {
    // First `:` not inside quotes.
    let mut in_s = false;
    let mut in_d = false;
    for (i, c) in line.char_indices() {
        match c {
            '\'' if !in_d => in_s = !in_s,
            '"' if !in_s => in_d = !in_d,
            ':' if !in_s && !in_d => return Some(i),
            _ => {}
        }
    }
    None
}

fn check_one_skill_md(root: &Path, path: &Path, prefix: &str) -> Result<CheckResult> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let fm = parse_skill_frontmatter(&raw).unwrap_or_default();

    let rel = rel_unix(root, path);

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
            format!("{rel}: `description` is empty"),
            "To fix: write one sentence describing the task the skill does.",
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

    // description leads with an alpha word (action-verb heuristic).
    let first_word = combined.split_whitespace().next().unwrap_or("");
    let starts_alpha = first_word.chars().next().is_some_and(char::is_alphabetic);
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
        .map_or(true, |w| w.trim().is_empty())
    {
        return Ok(CheckResult::warn(
            &format!("{prefix}.when_to_use"),
            "SKILL.md has non-empty `when_to_use` trigger phrases",
            format!("{rel}: `when_to_use` is missing or empty"),
            "To fix: list 2-5 trigger verbs/scenarios, e.g. \"Use when: the user asks to ...\".",
        ));
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
            return Ok(CheckResult::warn(
                &format!("{prefix}.allowed_tools"),
                "SKILL.md `allowed-tools` tokens match the Anthropic grammar",
                format!(
                    "{rel}: `allowed-tools` has malformed token(s): {}",
                    bad.iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "To fix: each comma-separated token must be a bare identifier \
                 (e.g. `Read`) or a namespaced call (e.g. `Bash(npm test:*)`).",
            ));
        }
    }

    // name: if present, kebab + <=64 chars.
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

    Ok(CheckResult::pass(
        prefix,
        "SKILL.md is structurally valid",
        format!("{rel} validates"),
    ))
}

// ----- helpers --------------------------------------------------------------

/// Every SKILL.md under `skills/*/SKILL.md` plus a root `SKILL.md`, sorted for
/// deterministic verification (read_dir order is unspecified). A plugin may
/// legitimately ship multiple skills (Improvement C).
pub(crate) fn find_skill_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let skills_dir = root.join("skills");
    if skills_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&skills_dir) {
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
    let root_skill = root.join("SKILL.md");
    if root_skill.is_file() {
        out.push(root_skill);
    }
    out
}

/// The first skill file found — kept for the invocation stage, which only
/// spawns one CLI (the documented one). [`find_skill_files`] is the
/// deterministic plural form used by discovery.
pub(crate) fn find_skill_file(root: &Path) -> Option<std::path::PathBuf> {
    find_skill_files(root).into_iter().next()
}

/// Validate an `allowed-tools` token against the Anthropic grammar:
/// a bare identifier (`Read`, `Grep`) or a namespaced call
/// (`Bash(npm test:*)`, `Edit(*)`). Returns false for empty, malformed,
/// or unbalanced-paren tokens. Ponytail: regex-free char scan — the
/// grammar is small enough to validate inline without pulling `regex`.
pub(crate) fn is_valid_allowed_tool_token(token: &str) -> bool {
    let t = token.trim();
    if t.is_empty() {
        return false;
    }
    // Bare identifier: all alphabetic.
    if t.chars().all(char::is_alphabetic) {
        return true;
    }
    // Namespaced call: `Name(args)`. Split on the first `(`.
    let Some(open) = t.find('(') else {
        return false;
    };
    let (name, rest) = t.split_at(open);
    if !name.chars().all(char::is_alphabetic) || name.is_empty() {
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

/// True for a valid kebab-case plugin/skill/marketplace name.
pub fn is_valid_kebab(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name.len() == 1 {
        return name.chars().next().unwrap().is_ascii_lowercase();
    }
    NAME_RE.is_match(name) && !name.contains("--")
}

/// Validate a relative-path `source`. Returns `Err(reason)` if invalid.
pub fn validate_relative_source(src: &str) -> Result<(), String> {
    if !src.starts_with("./") {
        return Err("must start with `./`".to_string());
    }
    if src.contains("../") {
        return Err("must not contain `../` (escapes the marketplace root)".to_string());
    }
    if src.contains('\\') {
        return Err("must use forward slashes only".to_string());
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))
        .map(Some)
}
