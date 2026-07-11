//! Snapshot tests for `generate::render` (design §7.1: "gold-standard expected
//! outputs ... the diff must be zero. Forces explicit review of any templating
//! changes").
//!
//! These pin the **byte-identical** output of all three generated files
//! (`marketplace.json`, `plugin.json`, `SKILL.md`) for two representative
//! profiles: a CLI project and a pure-library project. Any template edit will
//! surface here as a snapshot diff the contributor must deliberately accept.
//!
//! # Inputs are deliberately deterministic
//!
//! The generated SKILL.md body embeds `documented_flags` parsed from the
//! profile's `cli_help_output`, and frontmatter derived from `repo_url`,
//! `license`, `author`. The profile/intent built below set these explicitly
//! (NOT via introspection), so the snapshot is reproducible across machines and
//! PATH states — we assert against literal text, not probed environment. If you
//! change a template and these fail: review with `cargo insta review`, or
//! regenerate with `INSTA_UPDATE=always cargo test --test snapshots` then
//! commit the updated `tests/snapshots/*.snap`.

use skillpack::cli::Target;
use skillpack::generate::{render, render_targets};
use skillpack::types::{Intent, Language, ProjectProfile};

/// A fixed CLI profile: name, language, a real `--help` blob (so the
/// documented-flags list is stable), explicit URL/license/author. No PATH, no
/// introspection. Built literally (not via the lib's `test_default`, which is
/// `#[cfg(test)]`-private and unreachable from this integration-test crate).
fn cli_profile() -> ProjectProfile {
    ProjectProfile {
        name: "chronicle".to_string(),
        language: Language::Rust,
        has_cli: true,
        cli_command: Some(vec!["chronicle".to_string(), "--help".to_string()]),
        cli_help_output: Some(
            "Usage: chronicle [OPTIONS]\n\
             --new <entry>   Create an entry\n\
             --verbose       Verbose output\n"
                .to_string(),
        ),
        cli_subcommand_help: Vec::new(),
        diag: skillpack::types::DiagTrace::default(),
        repo_url: Some("https://github.com/example/chronicle".to_string()),
        license: Some("MIT".to_string()),
        description_hint: None,
        version: Some("0.1.0".to_string()),
        authors: None,
    }
}

fn cli_intent() -> Intent {
    Intent {
        one_line_description: "Journal events to a chronological log".to_string(),
        when_to_use_phrases: vec![
            "log a journal entry".to_string(),
            "record an incident".to_string(),
        ],
        invocation_command: Some("chronicle --new \"entry\"".to_string()),
        import_pattern: None,
        author: Some("Mikey".to_string()),
        license: Some("MIT".to_string()),
    }
}

fn lib_profile() -> ProjectProfile {
    ProjectProfile {
        name: "fastcsv".to_string(),
        language: Language::Node,
        has_cli: false,
        cli_command: None,
        cli_help_output: None,
        cli_subcommand_help: Vec::new(),
        diag: skillpack::types::DiagTrace::default(),
        repo_url: Some("https://github.com/example/fastcsv".to_string()),
        license: Some("MIT".to_string()),
        description_hint: None,
        version: Some("0.1.0".to_string()),
        authors: None,
    }
}

fn lib_intent() -> Intent {
    Intent {
        one_line_description: "Parse CSV files fast".to_string(),
        when_to_use_phrases: vec!["ingest csv".to_string(), "convert rows".to_string()],
        invocation_command: None,
        import_pattern: Some("import { parse } from 'fastcsv'".to_string()),
        author: None,
        license: Some("MIT".to_string()),
    }
}

#[test]
fn snapshot_cli_marketplace_json() {
    let files = render(&cli_profile(), &cli_intent()).unwrap();
    let mp = files
        .iter()
        .find(|f| f.rel_path.ends_with("marketplace.json"))
        .unwrap();
    insta::assert_snapshot!("marketplace_cli", mp.contents);
}

#[test]
fn snapshot_cli_plugin_json() {
    let files = render(&cli_profile(), &cli_intent()).unwrap();
    let pj = files
        .iter()
        .find(|f| f.rel_path.ends_with("plugin.json"))
        .unwrap();
    insta::assert_snapshot!("plugin_cli", pj.contents);
}

#[test]
fn snapshot_cli_skill_md() {
    let files = render(&cli_profile(), &cli_intent()).unwrap();
    let skill = files
        .iter()
        .find(|f| f.rel_path.ends_with("SKILL.md"))
        .unwrap();
    insta::assert_snapshot!("skill_cli", skill.contents);
}

#[test]
fn snapshot_lib_marketplace_json() {
    let files = render(&lib_profile(), &lib_intent()).unwrap();
    let mp = files
        .iter()
        .find(|f| f.rel_path.ends_with("marketplace.json"))
        .unwrap();
    insta::assert_snapshot!("marketplace_lib", mp.contents);
}

#[test]
fn snapshot_lib_plugin_json() {
    let files = render(&lib_profile(), &lib_intent()).unwrap();
    let pj = files
        .iter()
        .find(|f| f.rel_path.ends_with("plugin.json"))
        .unwrap();
    insta::assert_snapshot!("plugin_lib", pj.contents);
}

#[test]
fn snapshot_lib_skill_md() {
    let files = render(&lib_profile(), &lib_intent()).unwrap();
    let skill = files
        .iter()
        .find(|f| f.rel_path.ends_with("SKILL.md"))
        .unwrap();
    insta::assert_snapshot!("skill_lib", skill.contents);
}

#[test]
fn snapshot_render_is_idempotent_byte_identical() {
    // Two independent renders of the same inputs must be byte-identical — the
    // property that lets snapshots be meaningful. Guards against any hidden
    // nondeterminism (e.g. a template filter that inserts iteration order).
    let a = render(&cli_profile(), &cli_intent()).unwrap();
    let b = render(&cli_profile(), &cli_intent()).unwrap();
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.contents, y.contents, "{} not idempotent", x.rel_path);
    }
}

// ----- non-Claude target snapshots -----------------------------------------
// Lock the Cursor globs, OpenCode mode, and Copilot noun ("tool") that the
// shared partial + per-target frontmatter produce. Mirrors the Claude pattern
// but via `render_targets` so the full multi-ecosystem path is exercised.

#[test]
fn snapshot_cli_cursor_mdc() {
    let files = render_targets(&cli_profile(), &cli_intent(), &[Target::Cursor]).unwrap();
    let mdc = files.iter().find(|f| f.rel_path.ends_with(".mdc")).unwrap();
    insta::assert_snapshot!("cursor_cli", mdc.contents);
}

#[test]
fn snapshot_cli_opencode_agent() {
    let files = render_targets(&cli_profile(), &cli_intent(), &[Target::OpenCode]).unwrap();
    let agent = files.iter().find(|f| f.rel_path.ends_with(".md")).unwrap();
    insta::assert_snapshot!("opencode_cli", agent.contents);
}

#[test]
fn snapshot_cli_copilot_instructions() {
    let files = render_targets(&cli_profile(), &cli_intent(), &[Target::Copilot]).unwrap();
    let instr = files
        .iter()
        .find(|f| f.rel_path.ends_with("copilot-instructions.md"))
        .unwrap();
    insta::assert_snapshot!("copilot_cli", instr.contents);
}

// PHP: verify Cursor globs derive from Language::Php (locks *.php + composer.json).
#[test]
fn snapshot_php_cursor_globs() {
    let mut p = cli_profile();
    p.language = Language::Php;
    let files = render_targets(&p, &cli_intent(), &[Target::Cursor]).unwrap();
    let mdc = files.iter().find(|f| f.rel_path.ends_with(".mdc")).unwrap();
    insta::assert_snapshot!("cursor_php", mdc.contents);
}

// JVM: verify Cursor globs derive from Language::Jvm (locks *.java, *.kt,
// *.scala, pom.xml, build.gradle, build.gradle.kts).
#[test]
fn snapshot_jvm_cursor_globs() {
    let mut p = cli_profile();
    p.language = Language::Jvm;
    let files = render_targets(&p, &cli_intent(), &[Target::Cursor]).unwrap();
    let mdc = files.iter().find(|f| f.rel_path.ends_with(".mdc")).unwrap();
    insta::assert_snapshot!("cursor_jvm", mdc.contents);
}

// C#: verify Cursor globs derive from Language::CSharp (locks *.cs, *.csproj, *.sln).
#[test]
fn snapshot_csharp_cursor_globs() {
    let mut p = cli_profile();
    p.language = Language::CSharp;
    let files = render_targets(&p, &cli_intent(), &[Target::Cursor]).unwrap();
    let mdc = files.iter().find(|f| f.rel_path.ends_with(".mdc")).unwrap();
    insta::assert_snapshot!("cursor_csharp", mdc.contents);
}
