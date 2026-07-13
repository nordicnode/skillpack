//! Generate the three distribution files from a [`ProjectProfile`] + [`Intent`]
//! via Tera templates. Pure (no disk writes here): returns [`GeneratedFile`]s;
//! the CLI dispatcher decides whether to write them (after pre-commit verify).
//!
//! Design §5.1 step 3 + §6.3. Idempotent: identical inputs produce byte-identical
//! output across runs (templates use sorted/stable iteration where order
//! matters, and `default(value=...)` keeps fields present rather than
//! conditionally-present).

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use std::path::Path;
use tera::{Context as TeraContext, Tera};

use crate::cli::Target;
use crate::types::{Intent, Language, ProjectProfile};
use crate::verify::schema;

/// Name → template source. Embedded via `include_str!` so templates ship inside
/// the binary but still live as editable `.tera` files in the repo for
/// non-Rust contributors (design §6.3).
const MARKETPLACE_TPL: &str = include_str!("../templates/marketplace.json.tera");
const PLUGIN_TPL: &str = include_str!("../templates/plugin.json.tera");
const SKILL_TPL: &str = include_str!("../templates/SKILL.md.tera");
const CURSOR_RULE_TPL: &str = include_str!("../templates/cursor-rule.mdc.tera");
const OPENCODE_AGENT_TPL: &str = include_str!("../templates/opencode-agent.md.tera");
const COPILOT_INSTRUCTIONS_TPL: &str = include_str!("../templates/copilot-instructions.md.tera");
const AGENTS_MD_TPL: &str = include_str!("../templates/AGENTS.md.tera");
const SKILL_BODY_TPL: &str = include_str!("../templates/skill_body.md.tera");

static TERA: Lazy<Tera> = Lazy::new(|| {
    let mut tera = Tera::default();
    tera.add_raw_template("marketplace.json", MARKETPLACE_TPL)
        .expect("marketplace template is valid");
    tera.add_raw_template("plugin.json", PLUGIN_TPL)
        .expect("plugin template is valid");
    tera.add_raw_template("SKILL.md", SKILL_TPL)
        .expect("SKILL template is valid");
    tera.add_raw_template("cursor-rule.mdc", CURSOR_RULE_TPL)
        .expect("cursor rule template is valid");
    tera.add_raw_template("opencode-agent.md", OPENCODE_AGENT_TPL)
        .expect("opencode agent template is valid");
    tera.add_raw_template("skill_body_partial", SKILL_BODY_TPL)
        .expect("skill body partial template is valid");
    tera.add_raw_template("copilot-instructions.md", COPILOT_INSTRUCTIONS_TPL)
        .expect("copilot instructions template is valid");
    tera.add_raw_template("AGENTS.md", AGENTS_MD_TPL)
        .expect("AGENTS.md template is valid");
    // json_encode is built into Tera; nothing custom to register.
    tera
});

/// The three files `init` emits, relative to the project root. Documented for
/// external tooling/tests; the renderer computes paths itself.
#[allow(dead_code)]
pub const OUTPUT_PATHS: [&str; 3] = [
    ".claude-plugin/marketplace.json",
    ".claude-plugin/plugin.json",
    "skills/<tool>/SKILL.md",
];

/// Build the full Tera context from profile + intent.
pub fn build_context(profile: &ProjectProfile, intent: &Intent) -> TeraContext {
    let name = coerce_kebab(&profile.name);
    let keywords = Keywords {
        inner: derive_keywords(profile, intent),
    };
    // `display_name` is the human label for the tool in prose ("Do not use
    // this skill if the user only wants to *read* {{ display_name }}"). It is
    // the tool *name*, not the README blurp (which can read as a sentence and
    // mangle the surrounding prose).
    let display_name = name.clone();
    let has_cli = profile.has_cli;
    // `cli_binary` is the bare name agents/users would type to invoke the tool
    // (e.g. `fd`, not the crate name `fd-find`). Derive from the actual CLI
    // command argv (the built binary path) so a `[[bin]].name` rename surfaces
    // in the invocation prose. Falls back to the skill `name` for libraries.
    let cli_binary = profile
        .cli_command
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|cmd| {
            std::path::Path::new(cmd)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.clone());
    // `documented_flags` come from the captured --help output: the flags a
    // user can actually pass. Used to populate the "Documented flags" list.
    let documented_flags = profile
        .cli_help_output
        .as_deref()
        .map(crate::verify::invocation::extract_flags)
        .unwrap_or_default();

    // Subcommands: each advertised subcommand + the flags its own `--help`
    // exposes (parsed from the captured per-sub help). Order = declaration
    // order (clap), preserved by the `Vec` on the profile → deterministic
    // snapshots. Empty for non-subcommand CLIs and pure libraries.
    let documented_subcommands: Vec<serde_json::Value> = profile
        .cli_subcommand_help
        .iter()
        .map(|(name, help)| {
            // Drop the universal --help/-h/--version/-V meta-flags (per
            // invocation::is_meta_flag) so a subcommand bullet shows the
            // tool-specific flags an agent would actually pass, not the
            // help/version every CLI implicitly answers to.
            let flags: Vec<String> = crate::verify::invocation::extract_flags(help)
                .into_iter()
                .filter(|f| !crate::verify::invocation::is_meta_flag(f))
                .collect();
            serde_json::json!({ "name": name, "flags": flags })
        })
        .collect();

    // Precompute the joined when-to-use list so the template stays a thin
    // presentation layer (no Tera filter syntax for non-Rust contributors to
    // trip over). Empty list -> empty string: we deliberately do NOT emit a
    // placeholder like "(unspecified)" here, because that non-empty sentinel
    // would bypass verify's own `when_to_use` emptiness warning (design §5.3 —
    // the worst failure mode is a skill pack that looks fine but has no real
    // triggers). An empty `when_to_use:` keeps the verifier honest.
    let when_concat = intent.when_to_use_phrases.join(", ");

    tera::Context::from_serialize(serde_json::json!({
        "name": name,
        "display_name": display_name,
        "one_line_description": one_line_description_yaml(&intent.one_line_description),
        "one_line_description_raw": &intent.one_line_description,
        "when_to_use_phrases": intent.when_to_use_phrases,
        "when_concat": escape_yaml(&when_concat),
        "author": intent.author.as_deref().or(profile.authors.as_deref()),
        "license": intent.license,
        "repo_url": profile.repo_url,
        "keywords": keywords,
        "version": profile.version.as_deref().unwrap_or_default(),
        "has_cli": has_cli,
        "cli_binary": cli_binary,
        "invocation_command": intent.invocation_command,
        "import_pattern": intent.import_pattern,
        "documented_flags": documented_flags,
        "documented_subcommands": documented_subcommands,
        "category_hint": category_hint(profile.language),
        "allowed_tools": allowed_tools_hint(profile.language),
        "globs": cursor_globs_yaml(profile.language),
        "opencode_mode": opencode_mode_hint(profile.language),
    }))
    .expect("Tera context serializes from JSON literal")
}

/// Escape a string so it's safe to embed inside YAML double-quoted scalar.
/// We escape backslash and double-quote — colons-followed-by-space are fine
/// inside quotes so we don't touch them.
fn escape_yaml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The one-line description can itself contain a colon; wrap it through the
/// same YAML escaper so the `description: "..."` line stays a single scalar.
fn one_line_description_yaml(s: &str) -> String {
    escape_yaml(s)
}

/// Renders all three files and returns them with their root-relative paths.
/// The skill path uses the kebab name.
pub fn render(
    profile: &ProjectProfile,
    intent: &Intent,
    template_dir: Option<&Path>,
) -> Result<Vec<GeneratedFileOutput>> {
    let tera = build_tera(template_dir)?;
    let mut ctx = build_context(profile, intent);
    ctx.insert("noun", "skill");
    let name = coerce_kebab(&profile.name);

    let marketplace = tera
        .render("marketplace.json", &ctx)
        .context("rendering marketplace.json")?;
    let plugin = tera
        .render("plugin.json", &ctx)
        .context("rendering plugin.json")?;
    let skill = tera
        .render("SKILL.md", &ctx)
        .context("rendering SKILL.md")?;

    Ok(vec![
        GeneratedFileOutput {
            rel_path: ".claude-plugin/marketplace.json".to_string(),
            contents: marketplace,
        },
        GeneratedFileOutput {
            rel_path: ".claude-plugin/plugin.json".to_string(),
            contents: plugin,
        },
        GeneratedFileOutput {
            rel_path: format!("skills/{name}/SKILL.md"),
            contents: skill,
        },
    ])
}
pub fn render_targets(
    profile: &ProjectProfile,
    intent: &Intent,
    targets: &[Target],
    template_dir: Option<&Path>,
) -> Result<Vec<GeneratedFileOutput>> {
    let tera = build_tera(template_dir)?;
    let ctx = build_context(profile, intent);
    let name = coerce_kebab(&profile.name);
    let mut out = Vec::new();

    // Dedupe: emit each target once.
    let mut seen = std::collections::HashSet::new();
    for &target in targets {
        if !seen.insert(target) {
            continue;
        }
        match target {
            Target::Claude => out.extend(render(profile, intent, template_dir)?),
            Target::Cursor => {
                let mut c = ctx.clone();
                c.insert("noun", "rule");
                let mdc = tera
                    .render("cursor-rule.mdc", &c)
                    .context("rendering cursor-rule.mdc")?;
                out.push(GeneratedFileOutput {
                    rel_path: format!(".cursor/rules/{name}.mdc"),
                    contents: mdc,
                });
            }
            Target::Codex => {
                // Codex reads SKILL.md with the same frontmatter as Claude —
                // reuse the same template, different output path.
                let mut c = ctx.clone();
                c.insert("noun", "skill");
                let skill = tera
                    .render("SKILL.md", &c)
                    .context("rendering codex SKILL.md")?;
                out.push(GeneratedFileOutput {
                    rel_path: format!(".codex/skills/{name}/SKILL.md"),
                    contents: skill,
                });
            }
            Target::OpenCode => {
                // OpenCode: .opencode/agents/<name>.md with `description`
                // (required) + `mode` frontmatter. Per opencode.ai/docs/agents.
                let mut c = ctx.clone();
                c.insert("noun", "agent");
                let agent = tera
                    .render("opencode-agent.md", &c)
                    .context("rendering opencode-agent.md")?;
                out.push(GeneratedFileOutput {
                    rel_path: format!(".opencode/agents/{name}.md"),
                    contents: agent,
                });
            }
            Target::Copilot => {
                // GitHub Copilot: .github/copilot-instructions.md — plain
                // markdown, no frontmatter. Per docs.github.com/copilot.
                let mut c = ctx.clone();
                c.insert("noun", "tool");
                let instr = tera
                    .render("copilot-instructions.md", &c)
                    .context("rendering copilot-instructions.md")?;
                out.push(GeneratedFileOutput {
                    rel_path: ".github/copilot-instructions.md".to_string(),
                    contents: instr,
                });
            }
            Target::AgentsMd => {
                // AGENTS.md: root-level instructions file, plain markdown, no
                // frontmatter. Per agents.md (Linux Foundation stewarded) —
                // read natively by 20+ coding agents.
                let mut c = ctx.clone();
                c.insert("noun", "tool");
                let agents = tera
                    .render("AGENTS.md", &c)
                    .context("rendering AGENTS.md")?;
                out.push(GeneratedFileOutput {
                    rel_path: schema::AGENTS_MD_PATH.to_string(),
                    contents: agents,
                });
            }
        }
    }

    Ok(out)
}

/// Map `.tera` filenames → embedded Tera template names.
/// Most are identity after stripping `.tera`; the two exceptions are
/// `skill_body.md.tera` → `skill_body_partial` (a shared partial)
/// and the `.mdc` file → `cursor-rule.mdc`.
const TEMPLATE_MAP: &[(&str, &str)] = &[
    ("marketplace.json.tera", "marketplace.json"),
    ("plugin.json.tera", "plugin.json"),
    ("SKILL.md.tera", "SKILL.md"),
    ("cursor-rule.mdc.tera", "cursor-rule.mdc"),
    ("opencode-agent.md.tera", "opencode-agent.md"),
    ("copilot-instructions.md.tera", "copilot-instructions.md"),
    ("AGENTS.md.tera", "AGENTS.md"),
    ("skill_body.md.tera", "skill_body_partial"),
];

/// Build a `Tera` instance, optionally overriding embedded templates from
/// a directory of `.tera` files. Missing templates fall back to the embedded
/// defaults — a user can override just one or two templates without
/// re-declaring the others. Template names must match the `.tera` filenames
/// in `templates/` (see `TEMPLATE_MAP`).
fn build_tera(template_dir: Option<&Path>) -> Result<Tera> {
    let Some(dir) = template_dir else {
        return Ok(TERA.clone());
    };
    let mut tera = Tera::clone(&*TERA);
    for (filename, internal_name) in TEMPLATE_MAP {
        let path = dir.join(filename);
        if let Ok(src) = std::fs::read_to_string(&path) {
            tera.add_raw_template(internal_name, &src)
                .map_err(|e| anyhow::anyhow!("failed to load template {filename}: {e}"))?;
        }
    }
    Ok(tera)
}

// ----- helpers --------------------------------------------------------------

/// A transparent newtype wrapper so the JSON / Tera context exposes the inner
/// array under the field name directly (the templates iterate `keywords` as a
/// list, not `keywords.inner`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Keywords {
    pub inner: Vec<String>,
}

/// Derive a small, stable keyword list from language + intent + CLI surface
/// so the generated marketplace entry is discoverable without the maintainer
/// hand-curating it. Always seeded with language + cli/library, then enriched:
///   - all trigger-phrase first-words (deduped)
///   - CLI subcommand NAMES (from `cli_subcommand_help`) — high-signal verbs
///     like `init`, `verify`, `doctor` that a marketplace searcher would type
///   - one content word from the README `description_hint` (longest non-stopword)
fn derive_keywords(profile: &ProjectProfile, intent: &Intent) -> Vec<String> {
    let mut kws = vec![profile.language.as_str().to_string()];
    if profile.has_cli {
        kws.push("cli".to_string());
    } else {
        kws.push("library".to_string());
    }

    // All trigger-phrase first-words (deduped, lowercased, alphanumeric only).
    for phrase in &intent.when_to_use_phrases {
        if let Some(word) = phrase.split_whitespace().next().map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        }) {
            if !word.is_empty() && !kws.contains(&word) {
                kws.push(word);
            }
        }
    }

    // CLI subcommand NAMES — high-signal marketplace keywords.
    for (sub, _help) in &profile.cli_subcommand_help {
        let sub = sub.to_lowercase();
        if !sub.is_empty() && !kws.contains(&sub) {
            kws.push(sub);
        }
    }

    // One content word from the README description_hint — the longest non-stopword.
    if let Some(hint) = &profile.description_hint {
        let best = hint
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|w| w.len() > 4 && w.chars().all(|c| c.is_alphanumeric()) && !is_stopword(w))
            .max_by_key(|w| w.len());
        if let Some(w) = best {
            let w = w.to_lowercase();
            if !w.is_empty() && !kws.contains(&w) {
                kws.push(w);
            }
        }
    }

    kws
}

/// Tiny stopword set for keyword extraction. Keeps "generate" but drops
/// "this", "with", "about". Ponytail: inline set beats pulling a crate.
fn is_stopword(w: &str) -> bool {
    matches!(
        w.to_lowercase().as_str(),
        "the"
            | "this"
            | "that"
            | "with"
            | "from"
            | "about"
            | "your"
            | "have"
            | "will"
            | "they"
            | "them"
            | "their"
            | "what"
            | "when"
            | "which"
            | "would"
            | "could"
            | "should"
            | "into"
            | "onto"
            | "over"
            | "under"
            | "also"
            | "just"
            | "only"
            | "than"
            | "then"
            | "these"
            | "those"
            | "using"
            | "being"
            | "been"
            | "more"
            | "most"
            | "such"
            | "some"
    )
}

fn category_hint(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "the Rust tooling",
        Language::Node => "the JavaScript/Node tooling",
        Language::Python => "the Python tooling",
        Language::Go => "the Go tooling",
        Language::Ruby => "the Ruby tooling",
        Language::Php => "the PHP tooling",
        Language::Jvm => "the JVM tooling",
        Language::CSharp => "the .NET/C# tooling",
        Language::Unknown => "the tooling",
    }
}

fn allowed_tools_hint(lang: Language) -> Option<&'static str> {
    // The skill describes a CLI a user runs; it can use Bash to run the CLI
    // and Read to consult output. We keep this conservative — a library skill
    // leans on the host project's tooling, so we leave it blank. Comma-
    // separated per the Anthropic `allowed-tools` grammar (matches
    // `verify`'s discovery.skill.allowed_tools grammar check).
    if let Language::Unknown = lang {
        None
    } else {
        Some("Read, Bash")
    }
}

/// Cursor auto-attach is driven by `globs` (file-pattern match) OR
/// `alwaysApply: true`. With neither, generated rules won't auto-trigger.
/// Derive glob patterns from the detected language so the rule activates on
/// the relevant files. Empty for Unknown — the maintainer should curate.
fn cursor_globs_hint(lang: Language) -> Vec<String> {
    match lang {
        Language::Rust => vec!["*.rs".into()],
        Language::Node => vec![
            "*.js".into(),
            "*.ts".into(),
            "*.jsx".into(),
            "*.tsx".into(),
            "package.json".into(),
        ],
        Language::Python => vec!["*.py".into()],
        Language::Go => vec!["*.go".into(), "go.mod".into()],
        Language::Ruby => vec!["*.rb".into(), "*.gemspec".into(), "Gemfile".into()],
        Language::Php => vec!["*.php".into(), "composer.json".into()],
        Language::Jvm => vec![
            "*.java".into(),
            "*.kt".into(),
            "*.scala".into(),
            "pom.xml".into(),
            "build.gradle".into(),
            "build.gradle.kts".into(),
        ],
        Language::CSharp => vec!["*.cs".into(), "*.csproj".into(), "*.sln".into()],
        Language::Unknown => vec![],
    }
}

/// Format globs as a YAML flow-sequence string with quoted entries:
/// `"*.rs", "*.go"`. Empty vec → empty string (template's `{% if globs %}`
/// gates the line). The template wraps with `globs: [{{ globs }}]` →
/// `globs: ["*.rs"]`.
fn cursor_globs_yaml(lang: Language) -> String {
    cursor_globs_hint(lang)
        .iter()
        .map(|g| format!("\"{g}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// OpenCode `mode` frontmatter: `primary` for CLI tools (standalone agent),
/// `subagent` for pure libraries (invoked by a parent agent). Conservative
/// default `subagent` for Unknown since the maintainer should confirm.
fn opencode_mode_hint(lang: Language) -> &'static str {
    if let Language::Unknown = lang {
        "subagent"
    } else {
        "primary"
    }
}

/// Coerce an arbitrary detected name into valid kebab-case for the plugin/skill
/// namespace. Lowercases, replaces runs of non-[a-z0-9] with a single hyphen,
/// strips leading/trailing hyphens.
pub fn coerce_kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    // Strip leading/trailing hyphens, then strip leading digits: the schema
    // regex `^[a-z]...` requires the name to start with a letter, so a
    // numeric-prefixed name like "123foo" → "foo" (not "123foo", which
    // would fail verify's own `is_valid_kebab` check). Re-trim hyphens
    // (stripping "123" from "123-foo" leaves "-foo") and re-check empty.
    let s = out.trim_matches('-');
    let s = s.trim_start_matches(|c: char| c.is_ascii_digit());
    let s = s.trim_matches('-');
    if s.is_empty() {
        return "tool".to_string();
    }
    if s.len() == 1 {
        return s.to_string();
    }
    s.to_string()
}

/// Output path + rendered contents, root-relative. Re-exports the shared
/// [`GeneratedFile`](crate::types::GeneratedFile) shape; kept as its own type
/// so callers don't need to import the path tuple awkwardly.
#[derive(Debug, Clone)]
pub struct GeneratedFileOutput {
    pub rel_path: String,
    pub contents: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Intent, Language, ProjectProfile};

    fn cli_profile() -> ProjectProfile {
        let mut p = ProjectProfile::test_default();
        p.name = "chronicle".into();
        p.language = Language::Rust;
        p.has_cli = true;
        p.cli_command = Some(vec!["chronicle".to_string(), "--help".to_string()]);
        p.cli_help_output = Some("Usage: chronicle [OPTIONS]\n  --new <entry>   Create an entry\n  --verbose        verbose\n".into());
        p.cli_subcommand_help = Vec::new();
        p.license = Some("MIT".into());
        p
    }

    fn cli_intent() -> Intent {
        Intent {
            one_line_description: "Journal events to a chronological log".into(),
            when_to_use_phrases: vec!["log a journal entry".into(), "record an incident".into()],
            invocation_command: Some("chronicle --new \"entry\"".into()),
            import_pattern: None,
            author: Some("Mikey".into()),
            license: Some("MIT".into()),
        }
    }

    #[test]
    fn renders_three_files_with_valid_paths() {
        let p = cli_profile();
        let i = cli_intent();
        let files = render(&p, &i, None).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].rel_path, ".claude-plugin/marketplace.json");
        assert_eq!(files[1].rel_path, ".claude-plugin/plugin.json");
        assert_eq!(files[2].rel_path, "skills/chronicle/SKILL.md");
    }

    #[test]
    fn rendered_marketplace_is_valid_json_and_points_at_dot_slash() {
        let p = cli_profile();
        let i = cli_intent();
        let mp = render(&p, &i, None).unwrap()[0].contents.clone();
        let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
        assert_eq!(v["plugins"][0]["source"], "./");
        assert_eq!(v["plugins"][0]["name"], "chronicle");
    }

    #[test]
    fn rendered_plugin_json_has_kebab_name_and_license() {
        let p = cli_profile();
        let i = cli_intent();
        let pj = render(&p, &i, None).unwrap()[1].contents.clone();
        let v: serde_json::Value = serde_json::from_str(&pj).unwrap();
        assert_eq!(v["name"], "chronicle");
        assert_eq!(v["license"], "MIT");
    }

    #[test]
    fn skill_md_has_description_and_when_to_use_in_frontmatter() {
        let p = cli_profile();
        let i = cli_intent();
        let skill = render(&p, &i, None).unwrap()[2].contents.clone();
        assert!(skill.starts_with("---\n"));
        // description holds the one-liner only; when_to_use carries the triggers.
        assert!(skill.contains("description: \"Journal events to a chronological log\""));
        assert!(skill.contains("when_to_use: \"log a journal entry, record an incident\""));
    }

    #[test]
    fn pure_library_renders_import_pattern_not_cli() {
        let mut p = cli_profile();
        p.has_cli = false;
        p.cli_command = None;
        p.cli_help_output = None;
        let i = Intent {
            one_line_description: "Parse CSV files fast".into(),
            when_to_use_phrases: vec!["ingest csv".into()],
            invocation_command: None,
            import_pattern: Some("import { parse } from 'fastcsv'".into()),
            author: None,
            license: Some("MIT".into()),
        };
        let files = render(&p, &i, None).unwrap();
        let skill = &files[2].contents;
        assert!(skill.contains("import { parse } from 'fastcsv'"));
        assert!(!skill.contains("Invocation"));
    }

    #[test]
    fn coerce_kebab_handles_messy_names() {
        assert_eq!(coerce_kebab("My Cool Tool"), "my-cool-tool");
        assert_eq!(coerce_kebab("foo__bar--baz"), "foo-bar-baz");
        assert_eq!(coerce_kebab("UPPER_CASE"), "upper-case");
        assert_eq!(coerce_kebab("a"), "a");
        assert_eq!(coerce_kebab("!!!"), "tool");
        // Leading digits must be stripped — the schema regex `^[a-z]`
        // requires a letter first, so "123foo" → "foo", not "123foo".
        assert_eq!(coerce_kebab("123foo"), "foo");
        assert_eq!(coerce_kebab("123-foo"), "foo");
        // All-digits → fallback, not an empty string.
        assert_eq!(coerce_kebab("123"), "tool");
        assert_eq!(coerce_kebab("9"), "tool");
    }

    #[test]
    fn idempotent_byte_identical_renders() {
        let p = cli_profile();
        let i = cli_intent();
        let a = render(&p, &i, None).unwrap();
        let b = render(&p, &i, None).unwrap();
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.contents, y.contents);
        }
    }

    // Bug 1: empty when_to_use_phrases must NOT emit a "(unspecified)"
    // placeholder that bypasses verify's emptiness warning. The frontmatter
    // should carry an empty when_to_use so the discovery check fires honestly.
    #[test]
    fn empty_when_to_use_emits_empty_not_placeholder() {
        let mut p = cli_profile();
        p.has_cli = false;
        p.cli_command = None;
        p.cli_help_output = None;
        let i = Intent {
            one_line_description: "Do a thing".into(),
            when_to_use_phrases: vec![],
            invocation_command: None,
            import_pattern: Some("import { x } from 'y'".into()),
            author: None,
            license: Some("MIT".into()),
        };
        let skill = render(&p, &i, None).unwrap()[2].contents.clone();
        assert!(
            skill.contains("when_to_use: \"\""),
            "empty phrases must yield when_to_use: \"\", got:\n{skill}"
        );
        assert!(
            !skill.contains("(unspecified)"),
            "the placeholder must not leak into the skill, got:\n{skill}"
        );
    }
    // Bug: non-interactive `init` replay from a skillpack.toml that omits
    // `invocation_command` (elided by `skip_serializing_if = "Option::is_none"`)
    // produced an empty fenced invocation block — the template rendered
    // `{{ invocation_command }}` as the empty string for a CLI project. The
    // template's `default(value=cli_binary)` falls back to the bare binary
    // name derived from `cli_command` (e.g. `chronicle`), so agents see a
    // runnable command instead of empty ticks.
    #[test]
    fn invocation_block_falls_back_to_cli_binary_when_intent_omits_command() {
        let p = cli_profile(); // has_cli=true, cli_command=["chronicle","--help"]
        let mut i = cli_intent();
        i.invocation_command = None; // simulates config replay without the field
        let skill = render(&p, &i, None).unwrap()[2].contents.clone();
        assert!(
            skill.contains("## Invocation"),
            "CLI project must still emit an Invocation section, got:\n{skill}"
        );
        // The fenced block must contain the bare binary name, not be empty.
        assert!(
            skill.contains("```\nchronicle\n```"),
            "invocation block must fall back to cli_binary `chronicle`, got:\n{skill}"
        );
    }
}
