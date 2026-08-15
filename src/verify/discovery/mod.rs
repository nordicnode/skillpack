//! Discovery checks — structural validation against the documented Claude Code
//! plugin schema. Pure functions over their inputs: file reads of the committed
//! artifacts plus the `repo_url` the caller threads in (so the URL-drift check
//! stays free of git subprocess spawns we'd otherwise own here). See
//! [`crate::verify::schema`] for the cited source of each rule.
//!
//! Checks run against the *generated* files, but `verify` also accepts
//! hand-written files written without `init` (design §4.2), so every check
//! must degrade gracefully on missing/empty inputs.

use std::path::Path;

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;

use super::result::CheckResult;
use super::schema;

static NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(schema::NAME_KEBAB_REGEX).expect("compiled constant regex"));

mod agentsmd;
mod claude;
mod copilot;
mod cursor;
mod opencode;
mod plainmd;
mod skill_md;

use agentsmd::{check_agents_md, find_agents_md};
use claude::{check_marketplace, check_plugin_json, claude_present};
use copilot::{check_copilot_instructions, find_copilot_instructions};
use cursor::{
    check_one_mdc, check_one_windsurf_rule, find_cursor_mdc_files, find_windsurf_rule_files,
};
pub use cursor::{parse_cursor_mdc_frontmatter, CursorFrontmatter};
use opencode::{check_one_opencode_agent, find_opencode_agent_files};
pub use opencode::{parse_opencode_agent_frontmatter, OpenCodeFrontmatter};
// `find_skill_files` is consumed by the invocation stage (`verify/mod.rs`)
// and `parse_skill_frontmatter` by the proptests; both are re-exported.
use skill_md::check_one_skill_md;
pub use skill_md::parse_skill_frontmatter;
pub(crate) use skill_md::{find_codex_skill_files, find_skill_files};

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
    let allowed_skill_names: std::collections::HashSet<String> = match crate::config::Config::load(
        root,
    ) {
        Ok(Some(cfg)) => cfg
            .to_intents()
            .into_iter()
            .map(|(n, _)| crate::generate::coerce_kebab(&n))
            .collect(),
        Ok(None) => std::collections::HashSet::new(),
        // A committed-but-unparseable skillpack.toml is a hard failure: it
        // breaks `init`/`update`/`--fix` replay and the pre-commit gate
        // runs ONLY `verify`, so this check is what keeps a broken config
        // from sailing through the hook.
        Err(e) => {
            out.push(CheckResult::fail(
                    "discovery.config.parse",
                    "skillpack.toml parses and validates",
                    format!("skillpack.toml failed to parse: {e}"),
                    "To fix: repair the config, or run `skillpack config --validate` for the exact error.",
                ));
            std::collections::HashSet::new()
        }
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
        (
            schema::TRAE_RULES_DIR,
            "discovery.trae.rule",
            "skillpack init --target trae",
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

pub fn is_valid_kebab(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name.len() == 1 {
        return name.chars().next().unwrap().is_ascii_lowercase();
    }
    NAME_RE.is_match(name) && !name.contains("--")
}

#[cfg(test)]
mod tests {
    //! Unit tests for the review-hardening checks: both URL-drift fields
    //! surface, unterminated frontmatter fails, and the skill directory name
    //! is cross-checked against the frontmatter `name`.

    use super::skill_md::{is_valid_allowed_tool_token, SkillFrontmatter};
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
