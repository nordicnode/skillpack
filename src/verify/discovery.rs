//! Discovery checks — structural validation against the documented Claude Code
//! plugin schema. Pure functions over their inputs: file reads of the committed
//! artifacts plus the `repo_url` the caller threads in (so the URL-drift check
//! stays free of git subprocess spawns we'd otherwise own here). See
//! [`crate::verify::schema`] for the cited source of each rule.
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

mod agentsmd;
mod copilot;
mod cursor;
mod opencode;
mod plainmd;

use agentsmd::{check_agents_md, find_agents_md};
use copilot::{check_copilot_instructions, find_copilot_instructions};
use cursor::{
    check_one_mdc, check_one_windsurf_rule, find_cursor_mdc_files, find_windsurf_rule_files,
};
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

/// Strip a leading UTF-8 BOM (U+FEFF, bytes EF BB BF) from `s`. Rust's
/// `char::is_whitespace` returns false for U+FEFF (Unicode 3.2+ excludes it),
/// so `str::trim()` and `trim_start()` do NOT strip it — `fs::read_to_string`
/// preserves BOM (valid UTF-8). A BOM-prefixed `---` frontmatter delimiter
/// would otherwise parse as missing frontmatter (false "missing description"
/// FAIL on a valid file), and a BOM-prefixed `#` heading in Copilot
/// instructions would false-warn "not a markdown heading". Windows editors
/// (Notepad, VS Code "UTF-8 with BOM" save) emit this prefix, so any hand-edited
/// SKILL.md / .mdc / agent file from a Windows user is exposed. Applied at each
/// raw-text read boundary (callers below), NOT inside `parse_*_frontmatter`
/// itself — non-BOM paths stay byte-identical (snapshot tests stay green).
pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// Run every discovery check, returning one [`CheckResult`] per check.
///
/// `root` is the plugin root (e.g. the dir containing `.claude-plugin/`,
/// `.cursor/rules/`, or `.codex/skills/`). Each ecosystem present is checked
/// independently — discovery degrades gracefully when an ecosystem's files
/// are absent (a `--target cursor`-only pack shouldn't fail on a missing
/// `.claude-plugin/`).
pub fn run(
    root: &Path,
    repo_url: &Option<String>,
    profile_name: &Option<String>,
) -> Result<Vec<CheckResult>> {
    let mut out = Vec::new();

    // Multi-skill packs: any `[[skills]]` entry name is a legitimate
    // frontmatter `name:` — only the primary skill matches the canonical
    // project name. Build the allowed set from skillpack.toml so the
    // name_drift warn fires only for genuinely unknown names. The canonical
    // name is added by the caller (profile_name), so we only need the extras.
    let allowed_skill_names: std::collections::HashSet<String> =
        match crate::config::Config::load(root) {
            Ok(Some(cfg)) => cfg
                .to_intents()
                .into_iter()
                .map(|(n, _)| crate::generate::coerce_kebab(&n))
                .collect(),
            _ => std::collections::HashSet::new(),
        };

    // Claude Code: marketplace.json + plugin.json + skills/<name>/SKILL.md.
    // The marketplace/plugin checks only run when the Claude distribution is
    // present — a `--target cursor`-only pack legitimately has no
    // `.claude-plugin/` and must not fail on its absence.
    if claude_present(root) {
        out.push(check_marketplace(root)?);
        out.push(check_plugin_json(root, repo_url)?);
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
                out.push(check_one_skill_md(
                    root,
                    &skill_path,
                    "discovery.skill",
                    profile_name,
                    &allowed_skill_names,
                )?);
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
                profile_name,
                &allowed_skill_names,
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

    // Windsurf (Cascade): `.windsurf/rules/<name>.md` — same frontmatter
    // schema as Cursor rules, `.md` extension.
    let windsurf_rules = find_windsurf_rule_files(root);
    if windsurf_rules.is_empty() && root.join(schema::WINDSURF_RULES_DIR).is_dir() {
        out.push(CheckResult::fail(
            "discovery.windsurf.rule.missing",
            "at least one Windsurf rule exists",
            ".windsurf/rules/ exists but contains no rule file",
            "To fix: run `skillpack init --target windsurf` or add a rule under .windsurf/rules/<name>.md.",
        ));
    } else {
        for rule_path in windsurf_rules {
            out.push(check_one_windsurf_rule(root, &rule_path)?);
        }
    }

    // AGENTS.md: root-level instructions file, plain markdown, no frontmatter.
    // Per agents.md (Linux Foundation stewarded) — read natively by 60k+
    // projects' agents (Codex, Cursor, Windsurf, Copilot, Aider, Zed, Warp,
    // JetBrains Junie, Freebuff, ...). Same structural check as Copilot:
    // file exists, non-empty, `#` heading.
    if let Some(p) = find_agents_md(root) {
        out.push(check_agents_md(root, &p)?);
    }

    // CLAUDE.md (Claude Code / Cline / Roo Code), GEMINI.md (Gemini CLI) and
    // CONVENTIONS.md (aider) — root-level plain-markdown instructions files,
    // same structural check as Copilot/AGENTS.md.
    for (rel, check_id, empty_hint) in [
        (
            schema::CLAUDE_MD_PATH,
            "discovery.claude_md",
            "To fix: add instructions content, or run `skillpack init --target claude-md`.",
        ),
        (
            schema::GEMINI_MD_PATH,
            "discovery.gemini",
            "To fix: add instructions content, or run `skillpack init --target gemini`.",
        ),
        (
            schema::CONVENTIONS_MD_PATH,
            "discovery.aider",
            "To fix: add instructions content, or run `skillpack init --target aider`.",
        ),
        (
            schema::GOOSE_INSTRUCTIONS_PATH,
            "discovery.goose",
            "To fix: add instructions content, or run `skillpack init --target goose`.",
        ),
    ] {
        if let Some(p) = plainmd::find_plain_file(root, rel) {
            out.push(plainmd::check_plain_md(
                root, &p, check_id, rel, empty_hint,
            )?);
        }
    }

    // Cline / Roo Code / Kilo Code — plain-markdown workspace rule directories.
    // Loose structural check (non-empty; `#` heading unless a `---` frontmatter
    // block is present, which rule formats legitimately support).
    for (dir, check_id, target_hint) in [
        (
            schema::CLINE_RULES_DIR,
            "discovery.cline.rule",
            "skillpack init --target cline",
        ),
        (
            schema::ROO_RULES_DIR,
            "discovery.roo.rule",
            "skillpack init --target roo",
        ),
        (
            schema::KILOCODE_RULES_DIR,
            "discovery.kilocode.rule",
            "skillpack init --target kilo",
        ),
        (
            schema::QODER_RULES_DIR,
            "discovery.qoder.rule",
            "skillpack init --target qoder",
        ),
        (
            schema::CONTINUE_RULES_DIR,
            "discovery.continue.rule",
            "skillpack init --target continue",
        ),
        (
            schema::AUGMENT_RULES_DIR,
            "discovery.augment.rule",
            "skillpack init --target augment",
        ),
        (
            schema::AMAZONQ_RULES_DIR,
            "discovery.amazonq.rule",
            "skillpack init --target amazonq",
        ),
    ] {
        let rules = plainmd::find_rule_files(root, dir);
        if rules.is_empty() && root.join(dir).is_dir() {
            out.push(CheckResult::fail(
                &format!("{check_id}.missing"),
                "at least one rule file exists",
                format!("{dir}/ exists but contains no .md file"),
                format!("To fix: run `{target_hint}` or add a rule under {dir}/<name>.md."),
            ));
        } else {
            for rule_path in rules {
                out.push(plainmd::check_plain_rule_md(
                    root,
                    &rule_path,
                    check_id,
                    &format!("To fix: add rule content, or run `{target_hint}`."),
                )?);
            }
        }
    }

    // When no ecosystem files are present at all, the plugin is malformed —
    // emit a single honest failure so a bare `skillpack verify` on an empty
    // repo doesn't silently pass.
    if out.is_empty() {
        out.push(CheckResult::fail(
            "discovery.empty",
            "at least one ecosystem is present (Claude / Codex / Cursor / OpenCode / Copilot / AGENTS.md / CLAUDE.md / GEMINI.md / Windsurf / Aider / Cline / Roo / Kilo / Goose / Qoder / Continue / Augment / Amazon Q)",
            "no distribution files found (none of: .claude-plugin/, .claude/skills/, .codex/skills/, .cursor/rules/, .windsurf/rules/, .opencode/agents/, .github/copilot-instructions.md, AGENTS.md, CLAUDE.md, GEMINI.md, CONVENTIONS.md, .clinerules/, .roo/rules/, .kilocode/rules/, .goose/instructions.md, .qoder/rules/, .continue/rules/, .augment/rules/, .amazonq/rules/)",
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
                    "{} not found: re-run `skillpack init` or check the path",
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

    if v.get("owner")
        .is_none_or(|o| o.is_null() || o == &serde_json::Value::Object(Default::default()))
    {
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

fn check_plugin_json(root: &Path, repo_url: &Option<String>) -> Result<CheckResult> {
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
        .is_none_or(|d| d.as_str().is_none_or(str::is_empty))
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

    // URL drift: plugin.json `homepage` and `repository` both render from
    // `repo_url` (the git origin detected at introspection time — see
    // `introspect::detect_repo_url`). `init` writes plugin.json, so drift
    // means a hand-edited plugin.json or a renamed/stale remote that wasn't
    // regenerated. Warn (not fail): a maintainer may intentionally host the
    // plugin elsewhere. Skipped entirely when `repo_url` is None (no git
    // origin) — a non-git or fresh repo cannot drift on a URL there's no
    // canonical source for.
    if let Some(canonical) = repo_url {
        // Collect EVERY drifted field before warning — an early return on the
        // first mismatch would hide a second drift (homepage AND repository
        // both stale) behind a single warning.
        let drifted: Vec<&str> = ["homepage", "repository"]
            .iter()
            .filter(|field| {
                let current = v.get(**field).and_then(|x| x.as_str()).unwrap_or("");
                !crate::introspect::urls_equivalent(current, canonical)
            })
            .copied()
            .collect();
        if !drifted.is_empty() {
            let detail = drifted
                .iter()
                .map(|f| {
                    let current = v.get(*f).and_then(|x| x.as_str()).unwrap_or("");
                    format!("`{f}` `{current}`")
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(CheckResult::warn(
                "discovery.plugin.url_drift",
                &format!(
                    "plugin.json `{}` matches the git origin URL",
                    drifted.join("`, `")
                ),
                format!("plugin.json {detail} != git origin `{canonical}`"),
                "To fix: re-run `skillpack init` to regenerate plugin.json from the current remote, or intentionally pin the URL.",
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

pub(crate) fn find_kv_colon(line: &str) -> Option<usize> {
    // First `:` not inside quotes. Handles escaped quotes (\", \').
    let mut in_s = false;
    let mut in_d = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '\'' if !in_d => in_s = !in_s,
            '"' if !in_s => in_d = !in_d,
            ':' if !in_s && !in_d => return Some(i),
            _ => {}
        }
    }
    None
}

fn check_one_skill_md(
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
            "To fix: list 2-5 trigger verbs/scenarios, e.g. \"Use when: the user asks to ...\".",
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
                "To fix: each comma-separated token must be a bare identifier \
                 (e.g. `Read`) or a namespaced call (e.g. `Bash(npm test:*)`).",
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

#[cfg(test)]
mod tests {
    //! Unit tests for the review-hardening checks: both URL-drift fields
    //! surface, unterminated frontmatter fails, and the skill directory name
    //! is cross-checked against the frontmatter `name`.

    use super::*;
    use crate::verify::result::Severity;
    use std::collections::HashSet;

    fn scratch() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("skillpack-discovery-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(root: &std::path::Path, rel: &str, contents: &str) -> std::path::PathBuf {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn url_drift_reports_both_fields_when_both_drift() {
        let root = scratch();
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{
  "name": "widget",
  "description": "does things",
  "version": "1.0.0",
  "author": { "name": "Jane" },
  "homepage": "https://stale.example/a",
  "repository": "https://stale.example/b"
}
"#,
        );
        let r = check_plugin_json(&root, &Some("https://github.com/acme/widget".into())).unwrap();
        assert_eq!(r.severity, Severity::Warn);
        assert_eq!(r.check_id, "discovery.plugin.url_drift");
        assert!(
            r.message.contains("homepage") && r.message.contains("repository"),
            "both drifted fields must be named, got: {}",
            r.message
        );
        assert!(r.message.contains("stale.example/a"));
        assert!(r.message.contains("stale.example/b"));
        assert!(r.message.contains("github.com/acme/widget"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn url_drift_single_field_only_names_that_field() {
        let root = scratch();
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{
  "name": "widget",
  "description": "does things",
  "version": "1.0.0",
  "author": { "name": "Jane" },
  "homepage": "https://stale.example/a",
  "repository": "https://github.com/acme/widget"
}
"#,
        );
        let r = check_plugin_json(&root, &Some("https://github.com/acme/widget".into())).unwrap();
        assert_eq!(r.check_id, "discovery.plugin.url_drift");
        assert!(
            r.message.contains("homepage") && !r.message.contains("repository"),
            "only homepage drifted; repository must stay out of the message, got: {}",
            r.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn url_drift_no_warning_when_both_match() {
        let root = scratch();
        write(
            &root,
            ".claude-plugin/plugin.json",
            r#"{
  "name": "widget",
  "description": "does things",
  "version": "1.0.0",
  "author": { "name": "Jane" },
  "homepage": "https://github.com/acme/widget",
  "repository": "https://github.com/acme/widget"
}
"#,
        );
        let r = check_plugin_json(&root, &Some("https://github.com/acme/widget".into())).unwrap();
        assert_eq!(r.check_id, "discovery.plugin");
        assert_eq!(r.severity, Severity::Pass);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unterminated_skill_frontmatter_fails() {
        let root = scratch();
        let path = write(
            &root,
            "skills/foo/SKILL.md",
            "---\nname: foo\ndescription: \"looks valid\"\n", // no closing `---`
        );
        let r =
            check_one_skill_md(&root, &path, "discovery.skill", &None, &HashSet::new()).unwrap();
        assert_eq!(r.severity, Severity::Error);
        assert_eq!(r.check_id, "discovery.skill.frontmatter_unclosed");
        assert!(r.message.contains("not closed"), "got: {}", r.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skill_dir_name_mismatch_warns() {
        let root = scratch();
        // Directory `bar`, frontmatter `name: foo` — agents load by directory.
        let path = write(
            &root,
            "skills/bar/SKILL.md",
            "---\nname: foo\ndescription: \"hello\"\nwhen_to_use: \"x\"\n---\n\nbody\n",
        );
        let r =
            check_one_skill_md(&root, &path, "discovery.skill", &None, &HashSet::new()).unwrap();
        assert_eq!(r.severity, Severity::Warn);
        assert_eq!(r.check_id, "discovery.skill.dir_name_mismatch");
        assert!(
            r.message.contains("`bar`") && r.message.contains("foo"),
            "got: {}",
            r.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skill_dir_name_matching_is_clean() {
        let root = scratch();
        let path = write(
            &root,
            "skills/foo/SKILL.md",
            "---\nname: foo\ndescription: \"hello\"\nwhen_to_use: \"x\"\n---\n\nbody\n",
        );
        let r =
            check_one_skill_md(&root, &path, "discovery.skill", &None, &HashSet::new()).unwrap();
        assert_eq!(r.severity, Severity::Pass);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_skill_dir_name_mismatch_warns() {
        let root = scratch();
        let path = write(
            &root,
            ".codex/skills/bar/SKILL.md",
            "---\nname: foo\ndescription: \"hello\"\nwhen_to_use: \"x\"\n---\n\nbody\n",
        );
        let r = check_one_skill_md(
            &root,
            &path,
            "discovery.codex.skill",
            &None,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(r.severity, Severity::Warn);
        assert_eq!(r.check_id, "discovery.codex.skill.dir_name_mismatch");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unterminated_cursor_frontmatter_fails() {
        let root = scratch();
        let path = write(
            &root,
            ".cursor/rules/foo.mdc",
            "---\ndescription: \"apply when x\"\n", // no closing `---`
        );
        let r = super::check_one_mdc(&root, &path).unwrap();
        assert_eq!(r.severity, Severity::Error);
        assert_eq!(r.check_id, "discovery.cursor.mdc.frontmatter_unclosed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unterminated_opencode_frontmatter_fails() {
        let root = scratch();
        let path = write(
            &root,
            ".opencode/agents/foo.md",
            "---\ndescription: \"does things\"\n", // no closing `---`
        );
        let r = super::check_one_opencode_agent(&root, &path).unwrap();
        assert_eq!(r.severity, Severity::Error);
        assert_eq!(r.check_id, "discovery.opencode.agent.frontmatter_unclosed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_kv_colon_handles_escaped_quotes_and_colons() {
        assert_eq!(find_kv_colon("name: foo"), Some(4));
        assert_eq!(find_kv_colon("description: \"test: here\""), Some(11));
        assert_eq!(
            find_kv_colon(r#"description: "test \"with: escaped\" quotes""#),
            Some(11)
        );
        assert_eq!(find_kv_colon("no colon here"), None);
    }

    #[test]
    fn skill_frontmatter_parse_multiline_strips_quotes_cleanly() {
        let block =
            "name: my-tool\ndescription: \"first line\nsecond line\"\nwhen_to_use: \"test\"";
        let fm = SkillFrontmatter::parse(block);
        assert_eq!(fm.name.as_deref(), Some("my-tool"));
        assert_eq!(
            fm.description.as_deref(),
            Some("first line\nsecond line"),
            "multiline description must not have trailing quote"
        );
    }

    #[test]
    fn allowed_tools_token_supports_mcp_and_standard_names() {
        assert!(is_valid_allowed_tool_token("Read"));
        assert!(is_valid_allowed_tool_token("Bash(npm test:*)"));
        assert!(is_valid_allowed_tool_token("mcp__github__create_issue"));
        assert!(is_valid_allowed_tool_token("mcp__server-name__tool_1"));
        assert!(!is_valid_allowed_tool_token(""));
        assert!(!is_valid_allowed_tool_token("Bash(unclosed"));
    }
}
