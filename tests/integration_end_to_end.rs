//! Integration test: end-to-end `skillpack init` + `skillpack verify` on a
//! real Rust fixture. Per design §7.2 — "feed skillpack a real OSS-style fixture
//! repo, run `init` then `verify`, assert both succeed and produce the expected
//! files."
//!
//! We run against the compiled `skillpack` binary via `assert_cmd`, in a
//! per-test copy of the fixture so a build run can't leak state between tests.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;

/// Root of the skillpack repo, so tests can locate `tests/fixtures`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Copy a fixture directory recursively into a temp dir and return the
/// destination root. Keeping a clean copy means a failed init never pollutes
/// the committed fixture for the next run.
fn copy_fixture(name: &str) -> PathBuf {
    let src = repo_root().join("tests/fixtures/repos").join(name);
    let dest = tempfile::tempdir().unwrap().keep();
    copy_dir(&src, &dest);
    dest
}

fn copy_dir(src: &Path, dest: &Path) {
    fs::create_dir_all(dest).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// Seed a `skillpack.toml` in `root` so `init` can run non-interactively.
fn write_skillpack_toml(root: &Path, name: &str) {
    let toml = format!(
        "[skill]\n\
         name = \"{name}\"\n\
         one_line_description = \"Print a journal entry to stdout\"\n\
         when_to_use_phrases = [\"log a journal entry\", \"record a quick note\"]\n\
         invocation_command = \"{name} --new \\\"entry\\\"\"\n\
         license = \"MIT\"\n"
    );
    fs::write(root.join("skillpack.toml"), toml).unwrap();
}

/// Replace the FIRST line of `text` that starts with `prefix` with `new_line`.
/// Used by allowed-tools grammar tests to mutate the emitted `allowed-tools:`
/// line in place — inserting above the closing `---` of the frontmatter block
/// lands in the body, where `parse_skill_frontmatter` never sees it. Ponytail:
/// a 6-line char scan beats pulling `regex` into the test deps.
fn replace_first_line_starting_with(text: &str, prefix: &str, new_line: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.trim_end_matches('\n').starts_with(prefix) {
            out.push_str(new_line);
            out.push('\n');
        } else {
            out.push_str(line);
        }
    }
    out
}

#[test]
fn rust_cli_init_then_verify_round_trip() {
    let root = copy_fixture("rust-cli");
    // Build the fixture's binary first so the invocation check can spawn it.
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();

    write_skillpack_toml(&root, "sample-rust");

    // init writes the three files (pre-commit verify must pass on the output).
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // All three distribution files must exist.
    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-rust/SKILL.md").exists());
    assert!(root.join("skillpack.toml").exists());

    // verify on the freshly-generated pack must pass clean.
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK").or(predicate::str::contains("0 failed")));

    // marketplace.json must be valid JSON with source "./".
    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-rust");
    assert_eq!(v["name"], "sample-rust");

    // plugin.json name must be kebab-case matching the tool.
    let pj = fs::read_to_string(root.join(".claude-plugin/plugin.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&pj).unwrap();
    assert_eq!(v["name"], "sample-rust");
    assert_eq!(v["license"], "MIT");
}

#[test]
fn pure_library_init_skips_invocation_and_writes_import_pattern() {
    // node-lib has no bin -> pure-library path. Seed a toml with an import pattern.
    let root = copy_fixture("node-lib");
    let toml = "[skill]\n\
        name = \"sample-lib\"\n\
        one_line_description = \"Parse CSV files with a small library\"\n\
        when_to_use_phrases = [\"ingest csv\", \"convert rows\"]\n\
        import_pattern = \"import { parse } from 'sample-lib'\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    let skill = fs::read_to_string(root.join("skills/sample-lib/SKILL.md")).unwrap();
    // Import pattern must be documented.
    assert!(skill.contains("import { parse } from 'sample-lib'"));
    // Invocation section must be absent for a pure library (design §5.1).
    assert!(!skill.contains("## Invocation"));
    assert!(skill.contains("## Usage"));

    // verify must report the invocation stage as Skipped, not failed.
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("Skipped: pure-library project"));
}

#[test]
fn non_interactive_without_skillpack_toml_refuses_to_write() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    // Deliberately do NOT seed skillpack.toml.
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["init", "--root", ".", "--non-interactive"])
        .current_dir(&root)
        .assert()
        .failure() // fatal: --non-interactive w/o toml exits non-zero
        .stderr(predicate::str::contains("no skillpack.toml found"));
    // No files should have been written.
    assert!(!root.join(".claude-plugin").exists());
}

#[test]
fn broken_cli_verify_flags_drift() {
    // The broken-cli fixture ships a SKILL.md that documents `--nonexistent`,
    // a flag the real `--help` (only `--new`) does not advertise.
    let root = copy_fixture("broken-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("flag_drift") || s.contains("missing from `--help`"),
        "expected a flag-drift failure, got:\n{s}"
    );
}

// Bug 1: init with empty when_to_use_phrases must produce a skill whose verify
// run WARNS about the missing triggers — not silently pass. The old code emitted
// a "(unspecified)" placeholder that bypassed the emptiness check.
#[test]
fn init_with_empty_when_to_use_warns_on_verify() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    let toml = "[skill]\n\
        name = \"sample-rust\"\n\
        one_line_description = \"Print a journal entry to stdout\"\n\
        when_to_use_phrases = []\n\
        invocation_command = \"sample-rust --new \\\"entry\\\"\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // The generated skill must carry an empty when_to_use (not the placeholder).
    let skill = fs::read_to_string(root.join("skills/sample-rust/SKILL.md")).unwrap();
    assert!(skill.contains("when_to_use: \"\""));
    assert!(!skill.contains("(unspecified)"));

    // verify must surface the missing-trigger warning (not silently pass).
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("when_to_use") && s.contains("warn"),
        "expected a when_to_use warning, got:\n{s}"
    );
}

// Improvement B: --format json emits a machine-readable report with the per-
// check ids + an `ok` flag, for CI gating.
#[test]
fn verify_format_json_is_machine_readable() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out)).unwrap();
    assert_eq!(v["ok"], serde_json::Value::Bool(true));
    let results = v["results"].as_array().unwrap();
    assert!(results.iter().all(|r| r["check_id"].is_string()));
    assert!(results
        .iter()
        .any(|r| r["check_id"].as_str().unwrap().starts_with("invocation.")));
    // The score field is always present and numeric. The rust-cli fixture
    // emits one warning (discovery.plugin.author — no author in
    // skillpack.toml) so 4 pass + 1 warn = 4.5/5 = 90, not 100.
    assert!(v["discoverability_score"].is_number());
    assert_eq!(v["discoverability_score"], serde_json::Value::from(90));
}

// Version drift: plugin.json `version` must match the project manifest
// version. `init` writes them in sync; hand-editing plugin.json (or bumping
// the manifest without regenerating) must surface as a `version_drift`
// WARNING — invisible before this check existed (self-dogfood surfaced it).
#[test]
fn verify_warns_on_plugin_json_version_drift() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Control: a freshly-generated pack must have NO version drift.
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert!(
        !v["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["check_id"].as_str().unwrap() == "discovery.plugin.version_drift"),
        "freshly-init'd pack must not report version drift, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Mutate plugin.json version to diverge from the manifest (0.1.0).
    let pj = root.join(".claude-plugin/plugin.json");
    let mut pjv: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pj).unwrap()).unwrap();
    pjv["version"] = serde_json::Value::String("9.9.9-fake".into());
    fs::write(&pj, serde_json::to_string_pretty(&pjv).unwrap()).unwrap();

    // Exercise: verify must surface a `version_drift` WARNING mentioning both
    // the plugin version and the manifest version, and must NOT fail the run.
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "warnings must not fail verify; got status {} and stderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let drift = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check_id"].as_str().unwrap() == "discovery.plugin.version_drift")
        .unwrap_or_else(|| {
            panic!(
                "version_drift warning missing in:\n{}",
                String::from_utf8_lossy(&out.stdout)
            )
        });
    assert_eq!(drift["severity"].as_str().unwrap(), "warn");
    let msg = drift["message"].as_str().unwrap();
    assert!(
        msg.contains("9.9.9-fake"),
        "message must name plugin version: {msg}"
    );
    assert!(
        msg.contains("0.1.0"),
        "message must name manifest version: {msg}"
    );
}

// `verify --fix` mechanically repairs the `discovery.plugin.version_drift`
// warning by regenerating ONLY `.claude-plugin/plugin.json` (surgical: the
// committed `SKILL.md` + `marketplace.json` stay intact). After the fix,
// the re-run report must have no version_drift warning and exit 0. The fix
// summary line is printed for the human via stdout.
#[test]
fn verify_fix_repairs_version_drift_surgically() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Snapshot SKILL.md body — assert unchanged after `--fix` (surgical guard).
    let skill_path = root.join("skills/sample-rust/SKILL.md");
    let skill_before = fs::read_to_string(&skill_path).unwrap();

    // Inject drift: rewrite plugin.json version to diverge from manifest.
    let pj = root.join(".claude-plugin/plugin.json");
    let mut pjv: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pj).unwrap()).unwrap();
    pjv["version"] = serde_json::Value::String("9.9.9-fake".into());
    fs::write(&pj, serde_json::to_string_pretty(&pjv).unwrap()).unwrap();

    // Run `verify --fix --format json`. The final report (post-fix re-run)
    // is the JSON body on stdout. Pre-fix report is suppressed; an
    // "✓ applied N fix(es), wrote: ..." summary line precedes it.
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--fix", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "verify --fix must exit 0 after repair, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let raw = String::from_utf8_lossy(&out.stdout);
    // The human-readable fix summary line precedes the JSON body.
    assert!(
        raw.contains("✓ applied 1 fix(es), wrote: .claude-plugin/plugin.json"),
        "must report the surgical fix in stdout: {raw}"
    );
    // The final JSON report must NOT contain the version_drift warning.
    let json_str = raw
        .lines()
        .skip_while(|l| !l.trim_start().starts_with('{'))
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let has_drift = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["check_id"].as_str().unwrap() == "discovery.plugin.version_drift");
    assert!(
        !has_drift,
        "post-fix report must not still emit version_drift, got:\n{raw}"
    );

    // Plugin.json version is back to the manifest version.
    let pjv2: serde_json::Value = serde_json::from_str(&fs::read_to_string(&pj).unwrap()).unwrap();
    assert_ne!(
        pjv2["version"].as_str().unwrap(),
        "9.9.9-fake",
        "plugin.json version must have been rewritten, got:\n{}",
        fs::read_to_string(&pj).unwrap()
    );
    assert_eq!(pjv2["version"].as_str().unwrap(), "0.1.0");

    // Surgical guard: SKILL.md untouched.
    let skill_after = fs::read_to_string(&skill_path).unwrap();
    assert_eq!(
        skill_before, skill_after,
        "verify --fix must NOT touch SKILL.md (surgical to plugin.json only)"
    );
}

/// `verify --fix` with no fixable drift is a no-op: no "✓ applied" summary,
/// just the normal verify report. Guards against `--fix` miscategorizing
/// warns/errors (which would clobber files unexpectedly).
#[test]
fn verify_fix_is_noop_when_no_fixable_drift() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Freshly init'd: no version_drift (we'd instead see discovery.plugin.author
    // warn — not a fixable drift). --fix must NOT emit "✓ applied" and must
    // exit 0.
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--fix", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "no-op --fix must exit 0, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let raw = String::from_utf8_lossy(&out.stdout);
    assert!(
        !raw.contains("✓ applied"),
        "no-op --fix must NOT emit fix summary, got: {raw}"
    );
    // Post-json JSON body still parses — verify functionality intact.
    let json_str = raw
        .lines()
        .skip_while(|l| !l.trim_start().starts_with('{'))
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(v["ok"], serde_json::Value::Bool(true));
}

// Improvement C: a plugin shipping multiple skills must verify each (the old
// code checked only an arbitrary first one — non-deterministic).
#[test]
fn verify_checks_all_skills_in_a_multi_skill_plugin() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Add a second skill whose description is empty -> must produce its OWN
    // failure tagged with the second skill's path, not be silently skipped.
    fs::create_dir_all(root.join("skills/second-tool")).unwrap();
    fs::write(
        root.join("skills/second-tool/SKILL.md"),
        "---\nname: second-tool\ndescription: \"\"\nwhen_to_use: \"x\"\n---\nbody\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("skills/second-tool/SKILL.md"),
        "expected the second skill's path in the report, got:\n{s}"
    );
}

// Bug 4: reverse drift — flags the CLI advertises in `--help` that the skill
// never documents — must fire on the SUCCESS path (no forward drift), via the
// real `verify` flow (not a direct `reverse_drift` call). The old code returned
// early at the pass-branch, gating reverse drift off entirely. The rust-cli
// fixture advertises `--verbose` in `--help`; a hand-written skill documenting
// only `--new` must surface `--verbose` as an undocumented-flag WARNING.
#[test]
fn reverse_drift_warns_on_success_path_via_verify_run() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();

    // Hand-written pack: skill documents only --new, but --help advertises
    // --verbose too. No forward drift (every documented flag exists).
    fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    fs::create_dir_all(root.join("skills/sample-rust")).unwrap();
    fs::write(
        root.join(".claude-plugin/marketplace.json"),
        "{\"name\":\"mp\",\"owner\":{\"name\":\"x\"},\"plugins\":[{\"name\":\"sample-rust\",\"source\":\"./\"}]}",
    )
    .unwrap();
    fs::write(
        root.join(".claude-plugin/plugin.json"),
        "{\"name\":\"sample-rust\",\"description\":\"Do thing\"}",
    )
    .unwrap();
    fs::write(
        root.join("skills/sample-rust/SKILL.md"),
        "---\nname: sample-rust\ndescription: \"Run the sample rust thing\"\nwhen_to_use: \"run sample rust\"\n---\n\n## Invocation\n\n```\nsample-rust --new entry\n```\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success() // warnings don't fail
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("undocumented_flags") || s.contains("--verbose"),
        "expected reverse-drift warning for --verbose via verify, got:\n{s}"
    );
}

// GAP #2: a plugin shipping >1 skill each documenting a CLI must warn that the
// invocation drift check only ran against the first — the others were skipped.
// Previously this was a documented-but-silent cliff.
#[test]
fn multi_cli_plugin_warns_invocation_only_checked_first() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();

    fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    fs::create_dir_all(root.join("skills/sample-rust")).unwrap();
    fs::write(
        root.join(".claude-plugin/marketplace.json"),
        "{\"name\":\"mp\",\"owner\":{\"name\":\"x\"},\"plugins\":[{\"name\":\"sample-rust\",\"source\":\"./\"}]}",
    )
    .unwrap();
    fs::write(
        root.join(".claude-plugin/plugin.json"),
        "{\"name\":\"sample-rust\",\"description\":\"Do thing\"}",
    )
    .unwrap();
    // First skill documents the real CLI (sample-rust --new).
    fs::write(
        root.join("skills/sample-rust/SKILL.md"),
        "---\nname: sample-rust\ndescription: \"Run the sample rust thing\"\nwhen_to_use: \"run sample rust\"\n---\n\n## Invocation\n\n```\nsample-rust --new entry\n```\n",
    )
    .unwrap();
    // Second skill documents a DIFFERENT CLI invocation. Use a dir name that
    // sorts AFTER sample-rust so the first-skill spawn still hits sample-rust
    // (find_skill_files sorts by file_name) — the point of this test is the
    // multi-CLI warning, not a false forward-drift from the second skill's flags.
    fs::create_dir_all(root.join("skills/zzz-other-tool")).unwrap();
    fs::write(
        root.join("skills/zzz-other-tool/SKILL.md"),
        "---\nname: zzz-other-tool\ndescription: \"Run the other thing\"\nwhen_to_use: \"run other\"\n---\n\n## Invocation\n\n```\nzzz-other-tool --flag\n```\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success() // warnings don't fail
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("invocation.multi_cli") || s.contains("only run against the first"),
        "expected a multi-CLI invocation warning, got:\n{s}"
    );
}

// Bug 2: a hand-written skill pack (no `init` output) that documents a CLI but
// ships no source tree / built artifact must NOT silently pass — `verify`
// reports the gap as a warning so the maintainer knows the invocation check
// didn't actually run (the old code skipped it silently under introspect-gated
// has_cli).
#[test]
fn hand_written_pack_documenting_unrunnable_cli_warns() {
    let dest = tempfile::tempdir().unwrap().keep();
    fs::create_dir_all(dest.join(".claude-plugin")).unwrap();
    fs::create_dir_all(dest.join("skills/foo")).unwrap();
    fs::write(
        dest.join(".claude-plugin/marketplace.json"),
        "{\"name\":\"foo-marketplace\",\"owner\":{\"name\":\"x\"},\"plugins\":[{\"name\":\"foo\",\"source\":\"./\"}]}",
    )
    .unwrap();
    fs::write(
        dest.join(".claude-plugin/plugin.json"),
        "{\"name\":\"foo\",\"description\":\"Do thing\"}",
    )
    .unwrap();
    // A skill with ## Invocation documenting a CLI, but no source tree / binary.
    fs::write(
        dest.join("skills/foo/SKILL.md"),
        "---\nname: foo\ndescription: \"Run the foo thing\"\nwhen_to_use: \"run foo\"\n---\n\n## Invocation\n\n```\nfoo --new entry\n```\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&dest)
        .assert()
        .success() // warnings don't fail
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(
        s.contains("not_runnable_here") || s.contains("no runnable command"),
        "expected a not-runnable warning, got:\n{s}"
    );
    assert!(
        !s.contains("pure-library project"),
        "a documented CLI must not read as a pure library, got:\n{s}"
    );
}

// design §8.1: a fixable verify critical (the user can fix it and re-run) must
// exit `INIT_FIXABLE` (2), distinct from `INIT_FATAL` (3) and the clean
// `INIT_ABORTED` (1). The `bad-help` fixture's CLI exits non-zero on `--help`,
// so `init`'s pre-commit gate hits a real critical. We decline "keep anyway"
// (pipe `n`) and assert NO files are written AND the exit code is 2, not 1 —
// the regression this pins is "the decline path used to return INIT_ABORTED".
#[test]
fn init_critical_decline_exits_fixable_not_aborted() {
    let root = copy_fixture("bad-help");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "bad-help");

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["init", "--root", "."])
        .arg("--accept-warnings") // only criticals matter; warnings won't gate
        .current_dir(&root)
        // Decline "Write the files anyway? [y/N]" — `n` defaults to No.
        .write_stdin("n\n")
        .assert()
        .get_output()
        .clone();
    let code = out.status.code().unwrap_or(-1);
    assert_eq!(
        code, 2,
        "declining a fixable critical must exit INIT_FIXABLE (2); got {code}"
    );
    // The exit-code mapping is only meaningful if we actually refused to write.
    assert!(
        !root.join(".claude-plugin/marketplace.json").exists(),
        "declined init must not write the marketplace manifest"
    );
}

// --- multi-ecosystem init+verify round trips (design §11: all eight ecosystems)
//
// Node and Python run end-to-end here because their runtimes are present on
// this dev machine. Go and Ruby are `#[ignore]`-gated: they don't run in the
// default `cargo test` (this dev machine lacks those runtimes) but DO run on
// CI's `ubuntu-latest` runner (which ships Go + Ruby) via `--include-ignored`.
// Detection of Go/Ruby candidate resolution is covered structurally in
// `src/introspect.rs::candidate_tests` regardless.

/// `node` must be on PATH for this test to mean anything; if it's absent the
/// fixture would (correctly) report `has_cli=false` and we'd assert against
/// nothing, so skip when the runtime is missing.
fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn node_cli_init_then_verify_round_trip() {
    if !node_available() {
        eprintln!("skipped: node not on PATH");
        return;
    }
    let root = copy_fixture("node-cli");
    let toml = "[skill]\n\
        name = \"sample-node\"\n\
        one_line_description = \"Build and run a sample Node CLI\"\n\
        when_to_use_phrases = [\"build a node sample\", \"run the node demo\"]\n\
        invocation_command = \"sample-node --build\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-node/SKILL.md").exists());
    assert!(root.join("skillpack.toml").exists());

    // The skill must document the CLI invocation, not the import pattern.
    let skill = fs::read_to_string(root.join("skills/sample-node/SKILL.md")).unwrap();
    assert!(skill.contains("## Invocation"));
    assert!(!skill.contains("## Usage"));

    // verify must pass clean, including the real `node <abs>/bin/cli.js --help`
    // invocation check (this exercises the spawn_cwd/project-root fix).
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"))
        // The invocation check must have actually run --help and found no flag
        // drift (the human render names these checks, not the machine ids).
        .stdout(predicate::str::contains(
            "documented `--help` runs and produces output",
        ))
        .stdout(predicate::str::contains(
            "every documented flag exists in `--help`",
        ));

    // marketplace.json source must be "./" and the name kebab-case.
    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-node");
}

/// `python`/`python3` must be on PATH. The python-cli fixture ships a runnable
/// `sample_python/` package so `python -m sample_python --help` works.
fn python_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn python_cli_init_then_verify_round_trip() {
    if !python_available() {
        eprintln!("skipped: python not on PATH");
        return;
    }
    let root = copy_fixture("python-cli");
    let toml = "[skill]\n\
        name = \"sample-python\"\n\
        one_line_description = \"Lint and fix a sample Python project\"\n\
        when_to_use_phrases = [\"lint python code\", \"apply auto-fixes\"]\n\
        invocation_command = \"sample-python --lint\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-python/SKILL.md").exists());
    assert!(root.join("skillpack.toml").exists());

    let skill = fs::read_to_string(root.join("skills/sample-python/SKILL.md")).unwrap();
    assert!(skill.contains("## Invocation"));
    assert!(!skill.contains("## Usage"));

    // verify must pass clean, including the real `python -m sample_python
    // --help` invocation check (run from the project root cwd).
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"))
        .stdout(predicate::str::contains(
            "documented `--help` runs and produces output",
        ))
        .stdout(predicate::str::contains(
            "every documented flag exists in `--help`",
        ));

    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-python");
}

// --- Go + Ruby: `#[ignore]`-gated spawn round trips -------------------------
//
// These run only on CI (via `cargo test -- --include-ignored`) where the
// runtimes are installed. A runtime probe at the top of each test makes them
// self-skip cleanly if invoked anywhere the runtime is missing — `#[ignore]`
// plus a probe is belt-and-suspenders.

fn go_available() -> bool {
    std::process::Command::new("go")
        .arg("version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
#[ignore = "requires `go` on PATH; runs on CI via --include-ignored"]
fn go_cli_init_then_verify_round_trip() {
    if !go_available() {
        eprintln!("skipped: go not on PATH");
        return;
    }
    let root = copy_fixture("go-cli");
    let toml = "[skill]\n\
        name = \"sample-go\"\n\
        one_line_description = \"Lint and fix a sample Go project\"\n\
        when_to_use_phrases = [\"lint go code\", \"run the go demo\"]\n\
        invocation_command = \"sample-go --lint\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-go/SKILL.md").exists());
    assert!(root.join("skillpack.toml").exists());

    let skill = fs::read_to_string(root.join("skills/sample-go/SKILL.md")).unwrap();
    assert!(skill.contains("## Invocation"));
    assert!(!skill.contains("## Usage"));

    // verify must pass clean, including the real `go run . --help` invocation
    // check spawned from the project root (the spawn_cwd fix). The go-cli
    // fixture's `main.go` advertises `--lint` and `--fix`.
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"))
        .stdout(predicate::str::contains(
            "documented `--help` runs and produces output",
        ))
        .stdout(predicate::str::contains(
            "every documented flag exists in `--help`",
        ));

    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-go");
}

fn ruby_available() -> bool {
    std::process::Command::new("ruby")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
#[ignore = "requires `ruby` on PATH; runs on CI via --include-ignored"]
fn ruby_cli_init_then_verify_round_trip() {
    if !ruby_available() {
        eprintln!("skipped: ruby not on PATH");
        return;
    }
    let root = copy_fixture("ruby-cli");
    let toml = "[skill]\n\
        name = \"sample-ruby\"\n\
        one_line_description = \"Lint and fix a sample Ruby project\"\n\
        when_to_use_phrases = [\"lint ruby code\", \"run the ruby demo\"]\n\
        invocation_command = \"sample-ruby --lint\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-ruby/SKILL.md").exists());
    assert!(root.join("skillpack.toml").exists());

    let skill = fs::read_to_string(root.join("skills/sample-ruby/SKILL.md")).unwrap();
    assert!(skill.contains("## Invocation"));
    assert!(!skill.contains("## Usage"));

    // verify must pass clean, including the real `ruby exe/sample-ruby --help`
    // invocation check (the fixture's binstub prints usage on --help).
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"))
        .stdout(predicate::str::contains(
            "documented `--help` runs and produces output",
        ))
        .stdout(predicate::str::contains(
            "every documented flag exists in `--help`",
        ));

    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-ruby");
}

fn php_available() -> bool {
    std::process::Command::new("php")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
#[ignore = "requires `php` on PATH; runs on CI via --include-ignored"]
fn php_cli_init_then_verify_round_trip() {
    if !php_available() {
        eprintln!("skipped: php not on PATH");
        return;
    }
    let root = copy_fixture("php-cli");
    let toml = "[skill]\n\
        name = \"sample-php\"\n\
        one_line_description = \"Print a journal entry from PHP\"\n\
        when_to_use_phrases = [\"log a php entry\", \"record a quick note\"]\n\
        invocation_command = \"sample-php --new \\\"entry\\\"\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-php/SKILL.md").exists());

    let skill = fs::read_to_string(root.join("skills/sample-php/SKILL.md")).unwrap();
    assert!(skill.contains("## Invocation"));
    assert!(!skill.contains("## Usage"));

    // verify must pass — the real `php <abs script> --help` invocation
    // check spawned from the project root.
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"))
        .stdout(predicate::str::contains(
            "documented `--help` runs and produces output",
        ))
        .stdout(predicate::str::contains(
            "every documented flag exists in `--help`",
        ));

    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-php");
}

#[cfg(unix)]
fn jvm_available() -> bool {
    // Unix-only: the fixture ships a #!/bin/sh installDist script with no
    // .bat sibling; the spawn round-trip is Unix-only. Structural Jvm
    // coverage stays cross-OS via snapshot_jvm_cursor_globs.
    std::process::Command::new("sh")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(unix)]
#[test]
#[ignore = "requires `sh` on PATH; runs on CI via --include-ignored"]
fn jvm_cli_init_then_verify_round_trip() {
    if !jvm_available() {
        eprintln!("skipped: sh not on PATH");
        return;
    }
    let root = copy_fixture("jvm-cli");
    let toml = "[skill]\n\
        name = \"sample-jvm\"\n\
        one_line_description = \"Print a journal entry from the JVM\"\n\
        when_to_use_phrases = [\"log a jvm entry\", \"record a quick note\"]\n\
        invocation_command = \"sample-jvm --new \\\"entry\\\"\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-jvm/SKILL.md").exists());

    let skill = fs::read_to_string(root.join("skills/sample-jvm/SKILL.md")).unwrap();
    assert!(skill.contains("## Invocation"));
    assert!(!skill.contains("## Usage"));

    // verify must pass — the installDist script's `--help` handler
    // produces output for the invocation check.
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"))
        .stdout(predicate::str::contains(
            "documented `--help` runs and produces output",
        ))
        .stdout(predicate::str::contains(
            "every documented flag exists in `--help`",
        ));

    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-jvm");
}

fn csharp_available() -> bool {
    std::process::Command::new("dotnet")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
#[ignore = "requires `dotnet` on PATH; runs on CI via --include-ignored"]
fn csharp_cli_init_then_verify_round_trip() {
    if !csharp_available() {
        eprintln!("skipped: dotnet not on PATH");
        return;
    }
    let root = copy_fixture("csharp-cli");
    let toml = "[skill]\n\
        name = \"sample-csharp\"\n\
        one_line_description = \"Print a journal entry from C#\"\n\
        when_to_use_phrases = [\"log a csharp entry\", \"record a quick note\"]\n\
        invocation_command = \"sample-csharp --new \\\"entry\\\"\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    // Pre-restore + build so `dotnet run` in verify is fast (cold NuGet
    // restore exceeds HELP_TIMEOUT otherwise — same reason rust-cli
    // calls `cargo build --quiet` first). `-v q` is `dotnet build`'s quiet
    // verbosity (dotnet has no `--quiet` flag).
    Command::new("dotnet")
        .args(["build", "-v", "q"])
        .current_dir(&root)
        .assert()
        .success();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());
    assert!(root.join("skills/sample-csharp/SKILL.md").exists());

    let skill = fs::read_to_string(root.join("skills/sample-csharp/SKILL.md")).unwrap();
    assert!(skill.contains("## Invocation"));
    assert!(!skill.contains("## Usage"));

    // verify must pass — `dotnet run --project <csproj> --help` produces
    // output for the invocation check.
    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"))
        .stdout(predicate::str::contains(
            "documented `--help` runs and produces output",
        ))
        .stdout(predicate::str::contains(
            "every documented flag exists in `--help`",
        ));

    let mp = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&mp).unwrap();
    assert_eq!(v["plugins"][0]["source"], "./");
    assert_eq!(v["plugins"][0]["name"], "sample-csharp");
}

// Subcommand-drift e2e: the subcommand-cli fixture prints a clap-shaped
// `Commands:` section in `--help` and per-subcommand `--help` with distinct
// flags. `capture_subcommand_help` (introspect) captures each sub's help;
// `check_subcommand_drift` (verify) spawns `<base> <sub> --help` and set-diffs.
// This is the only test that exercises the full subcommand spawn reassembly
// (`base.pop()` trailing `--help` + `<base> <sub> --help`) end-to-end.
#[test]
fn subcommand_cli_init_then_verify_round_trip() {
    let root = copy_fixture("subcommand-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();

    let toml = "[skill]\n\
        name = \"sample-sub\"\n\
        one_line_description = \"Scaffold and verify a skill pack\"\n\
        when_to_use_phrases = [\"scaffold a skill pack\", \"verify a skill pack\"]\n\
        invocation_command = \"sample-sub init --root\"\n\
        license = \"MIT\"\n";
    fs::write(root.join("skillpack.toml"), toml).unwrap();

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // init's introspect must have captured the subcommand help and the
    // template must have emitted the `### Subcommands` block with the real
    // sub names + their flags.
    let skill = fs::read_to_string(root.join("skills/sample-sub/SKILL.md")).unwrap();
    assert!(
        skill.contains("### Subcommands"),
        "SKILL.md must contain ### Subcommands block, got:\n{skill}"
    );
    assert!(
        skill.contains("`init`"),
        "SKILL.md must document the `init` subcommand, got:\n{skill}"
    );
    assert!(
        skill.contains("`verify`"),
        "SKILL.md must document the `verify` subcommand, got:\n{skill}"
    );
    // Per-sub flags captured from the real per-sub `--help`:
    assert!(
        skill.contains("--non-interactive"),
        "SKILL.md must list init's --non-interactive flag, got:\n{skill}"
    );
    assert!(
        skill.contains("--format"),
        "SKILL.md must list verify's --format flag, got:\n{skill}"
    );

    // verify must pass, including the real per-subcommand `--help` drift
    // checks (the feature this test exists to exercise). Parse the JSON
    // output directly (avoids string-matching whitespace in pretty-print).
    let json_out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        json_out.status.success(),
        "verify must exit 0, got:\n{}",
        String::from_utf8_lossy(&json_out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(
        json["ok"],
        serde_json::Value::Bool(true),
        "verify ok must be true, got:\n{json}"
    );

    let sub_results: Vec<&serde_json::Value> = json["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["check_id"] == "invocation.subcommand_drift")
        .collect();
    assert!(
        !sub_results.is_empty(),
        "verify must emit invocation.subcommand_drift results, got:\n{json}"
    );
    for r in &sub_results {
        assert_eq!(
            r["severity"], "pass",
            "subcommand_drift must pass for all documented subs, got:\n{json}"
        );
    }
}

#[test]
fn multi_target_init_then_verify_round_trip() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "claude",
            "--target",
            "cursor",
            "--target",
            "codex",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join("skills/sample-rust/SKILL.md").exists());
    assert!(root.join(".cursor/rules/sample-rust.mdc").exists());
    assert!(root.join(".codex/skills/sample-rust/SKILL.md").exists());
    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".claude-plugin/plugin.json").exists());

    let json_out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        json_out.status.success(),
        "verify must exit 0, got:\n{}",
        String::from_utf8_lossy(&json_out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(
        json["ok"],
        serde_json::Value::Bool(true),
        "verify ok must be true, got:\n{json}"
    );

    let results = json["results"].as_array().unwrap();
    for (check_id, label) in [
        ("discovery.skill", "claude"),
        ("discovery.codex.skill", "codex"),
        ("discovery.cursor.mdc", "cursor"),
    ] {
        let matches: Vec<&serde_json::Value> = results
            .iter()
            .filter(|r| r["check_id"] == check_id)
            .collect();
        assert!(
            !matches.is_empty(),
            "verify must emit {check_id} result, got:\n{json}"
        );
        for r in &matches {
            assert_eq!(
                r["severity"], "pass",
                "{label} check {check_id} must pass, got:\n{json}"
            );
        }
    }
}

#[test]
fn cursor_only_init_does_not_fail_on_missing_claude_plugin_dir() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "cursor",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".cursor/rules/sample-rust.mdc").exists());
    assert!(!root.join(".claude-plugin").exists());

    Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("verify OK"));
}

#[test]
fn broken_mdc_missing_description_fails_verify() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "cursor",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    fs::write(
        root.join(".cursor/rules/sample-rust.mdc"),
        "---\nalwaysApply: false\n---\n\n# sample-rust\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "verify must exit non-zero on missing description, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let name = "discovery.cursor.mdc.description";
    let matches: Vec<&serde_json::Value> =
        results.iter().filter(|r| r["check_id"] == name).collect();
    assert!(
        !matches.is_empty(),
        "verify must emit {name} result, got:\n{json}"
    );
    for r in &matches {
        assert_eq!(
            r["severity"], "fail",
            "{name} must be fail severity, got:\n{json}"
        );
    }
}

// `allowed-tools` grammar check (discovery.skill.allowed_tools): valid
// tokens (bare identifiers + namespaced calls) MUST NOT warn; malformed
// tokens (unbalanced parens, non-alpha identifiers, empty entries) MUST
// warn with `discovery.skill.allowed_tools`. Grammar-only — we don't
// validate membership against an Anthropic allowlist (which would false-fail
// the moment new tools ship).
#[test]
fn verify_warns_on_malformed_allowed_tools_grammar() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Mutate skills/<name>/SKILL.md frontmatter to inject malformed tokens.
    // `Read` + `Bash(npm test:*)` are VALID (control); `Bash(` (unbalanced),
    // `4R3ad` (non-alpha identifier) are INVALID. Replace the emitted
    // `allowed-tools:` line IN PLACE — inserting ABOVE the closing `---`
    // would land in the body outside the frontmatter block and the grammar
    // check never sees it.
    let skill_path = root.join("skills/sample-rust/SKILL.md");
    let raw = fs::read_to_string(&skill_path).unwrap();
    let new_raw = replace_first_line_starting_with(
        &raw,
        "allowed-tools:",
        "allowed-tools: Read, Bash(npm test:*), Bash(, 4R3ad",
    );
    assert_ne!(
        new_raw, raw,
        "test setup failed: emitted SKILL.md had no `allowed-tools:` line to replace"
    );
    fs::write(&skill_path, new_raw).unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    // Grammar failures are WARN-level — verify must still exit 0.
    assert!(
        out.status.success(),
        "verify must exit 0 on a warn (grammar only), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let matches: Vec<&serde_json::Value> = results
        .iter()
        .filter(|r| r["check_id"] == "discovery.skill.allowed_tools")
        .collect();
    assert!(
        !matches.is_empty(),
        "verify must emit discovery.skill.allowed_tools, got:\n{json}"
    );
    for r in &matches {
        assert_eq!(
            r["severity"], "warn",
            "allowed_tools grammar must be warn, got:\n{json}"
        );
        let msg = r["message"].as_str().unwrap_or("");
        // Both invalid tokens must be named in the message.
        assert!(msg.contains("`Bash(`"), "message must name `Bash(`: {msg}");
        assert!(msg.contains("`4R3ad`"), "message must name `4R3ad`: {msg}");
        // Valid tokens must NOT appear as bad.
        assert!(
            !msg.contains("`Read`"),
            "valid `Read` must not be flagged bad: {msg}"
        );
    }
}

/// Control: a SKILL.md with ONLY valid allowed-tools tokens (bare
/// identifiers + namespaced calls) must NOT emit `allowed_tools` warn.
/// Guards against the grammar check over-firing on well-formed input.
#[test]
fn verify_passes_on_valid_allowed_tools_grammar() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");
    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Replace the emitted `allowed-tools:` line IN PLACE with a well-formed
    // body: two bare identifiers + a namespaced call + a wildcard-arg call.
    // Mutating inside the frontmatter block is what the grammar check sees —
    // inserting above the closing `---` lands in the body and never parses.
    let skill_path = root.join("skills/sample-rust/SKILL.md");
    let raw = fs::read_to_string(&skill_path).unwrap();
    let new_raw = replace_first_line_starting_with(
        &raw,
        "allowed-tools:",
        "allowed-tools: Read, Edit, Bash(npm test:*), Grep(*)",
    );
    assert_ne!(
        new_raw, raw,
        "test setup failed: emitted SKILL.md had no `allowed-tools:` line to replace"
    );
    fs::write(&skill_path, new_raw).unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "verify must exit 0, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let matches: Vec<&serde_json::Value> = results
        .iter()
        .filter(|r| r["check_id"] == "discovery.skill.allowed_tools")
        .collect();
    assert!(
        matches.is_empty(),
        "valid allowed-tools grammar must NOT emit a warn, got:\n{json}"
    );
}

// Self-dogfood: `verify` against the skillpack repo's own committed
// distribution files — Claude (skills/skillpack/SKILL.md + .claude-plugin/),
// Cursor (.cursor/rules/skillpack.mdc), Codex (.codex/skills/skillpack/).
// These files live in the committed repo (generated via `init --target
// claude --target cursor --target codex` and committed), and `verify`
// must pass against them end-to-end: the regression guard for the
// multi-ecosystem discovery suite (a check regression here surfaces a
// defect in `check_one_skill_md` / `check_one_mdc` / `check_marketplace` /
// `check_plugin_json` against real, schema-conformant files).
#[test]
fn self_dogfood_verify_on_repos_committed_files() {
    // The rendered `skillpack` binary's docs embed a `target/release/skillpack`
    // path that only resolves after `cargo build --release`; the invocation
    // check in `--debug` builds embeds the debug-binary path. Verify the
    // release image exists so the invocation stage can spawn the documented
    // CLI, then run verify against the repo itself.
    Command::new("cargo")
        .args(["build", "--release", "--quiet"])
        .current_dir(repo_root())
        .assert()
        .success();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", "."])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "self-dogfood verify must pass, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("verify OK"), "expected verify OK, got:\n{s}");
    // Each ecosystem's pass message must be visible (multi-ecosystem verify
    // actually covers the file — the regression target of this test).
    assert!(s.contains(".claude-plugin/marketplace.json validates"));
    assert!(s.contains("skills/skillpack/SKILL.md validates"));
    assert!(s.contains(".codex/skills/skillpack/SKILL.md validates"));
    assert!(s.contains(".cursor/rules/skillpack.mdc validates"));
}

// Self-dogfood stronger guard: the COMMITTED distribution artifacts must be
// byte-identical to what a fresh `skillpack init --target <all 5>` produces.
// `self_dogfood_verify_on_repos_committed_files` above only asserts verify
// PASSES — but the 0.8.8 dogfood surface surfaced drift (cursor `globs:`
// missing, opencode `mode: subagent` instead of `primary`, trailing-newline
// drift) that all pass `verify` silently. Byte-diff catches what verify-passes
// hides.
//
// Skipped URLs: we DON'T byte-diff `.claude-plugin/marketplace.json` or
// `.claude-plugin/plugin.json` because their `repository` / `url` /
// `homepage` fields carry the git origin URL — the temp-dir copy has no
// `.git`, so init emits an empty URL there, while the committed files carry
// the GitHub URL. The 5 body files below never carry URL-derived fields, so
// byte-equality is the right assertion (they embed name, description,
// when_to_use, allowed-tools, globs, opencode mode, language-derived
// defaults — all deterministic from Cargo.toml + skillpack.toml).
#[test]
fn self_dogfood_regenerated_artifacts_match_committed_byte_identical() {
    // `init` needs a built skillpack binary on PATH (it probes
    // target/release/skillpack for `has_cli`). Each test runs in its own
    // target/debug/deps binary, so build release first.
    Command::new("cargo")
        .args(["build", "--release", "--quiet"])
        .current_dir(repo_root())
        .assert()
        .success();

    // Stage a minimal copy of the repo in a temp dir: the source tree +
    // templates so `init` can resolve the binary (Cargo.toml's [[bin]])
    // and the README description hint. Skip `.git` (no origin URL in the
    // temp), `target/` (we'll rebuild here), test fixtures, and the
    // existing committed distribution dirs — those would taint the
    // regenerated output if init inherited them as input.
    let dest = tempfile::tempdir().unwrap().keep();
    for entry in &[
        "Cargo.toml",
        "skillpack.toml",
        "README.md",
        "LICENSE",
        "rust-toolchain.toml",
    ] {
        fs::copy(repo_root().join(entry), dest.join(entry)).unwrap();
    }
    fs::create_dir_all(dest.join("docs")).unwrap();
    fs::copy(
        repo_root().join("docs/logo.png"),
        dest.join("docs/logo.png"),
    )
    .unwrap();
    for dir in &["src", "templates"] {
        copy_dir(&repo_root().join(dir), &dest.join(dir));
    }

    // Build the binary in the temp dir so `has_cli=true` probes target/release.
    // (Without this, init folds to the pure-library branch and the SKILL.md
    // would omit the `## Invocation` block, mismatching the committed file.)
    Command::new("cargo")
        .args(["build", "--release", "--quiet"])
        .current_dir(&dest)
        .assert()
        .success();

    // Regenerate all 5 targets non-interactively from the copied
    // skillpack.toml (no interview prompts).
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--non-interactive",
            "--accept-warnings",
            "--target",
            "claude",
            "--target",
            "cursor",
            "--target",
            "codex",
            "--target",
            "opencode",
            "--target",
            "copilot",
        ])
        .current_dir(&dest)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init must exit 0, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Byte-equalize the 5 body files that don't carry URL-derived fields.
    // Drift in `globs:`, `mode:`, language-derived `allowed-tools`, the
    // `## Invocation` block, trailing newlines, or template polish all
    // surface here.
    for rel in &[
        "skills/skillpack/SKILL.md",
        ".codex/skills/skillpack/SKILL.md",
        ".cursor/rules/skillpack.mdc",
        ".opencode/agents/skillpack.md",
        ".github/copilot-instructions.md",
    ] {
        let regen = fs::read_to_string(dest.join(rel)).unwrap_or_default();
        let committed = fs::read_to_string(repo_root().join(rel)).unwrap_or_default();
        assert_eq!(
            regen, committed,
            "regenerated `{rel}` drifted from committed:\n--- committed ---\n{committed}\
             \n--- regenerated ---\n{regen}"
        );
    }

    let _ = fs::remove_dir_all(&dest);
}

// `doctor --format json` emits the serialized `ProjectProfile` as a stable
// JSON object for CI/scripts. Pin the contract: top-level scalar fields by
// type, `diag` ALWAYS present as an array (empty on clean runs, non-empty
// when a candidate fn pushed a falsy branch), and each diag entry shaped as
// { stage: string, note: string }. Mirrors `verify_format_json_is_machine
// _readable`'s role for the verify report.
#[test]
fn doctor_format_json_is_machine_readable() {
    // Fixture without a built binary pushes one falsy-branch note into
    // `diag` — exercising the populated array shape.
    let root = copy_fixture("rust-cli");
    write_skillpack_toml(&root, "sample-rust");
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["doctor", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out)).unwrap();

    // Top-level scalar contract.
    assert!(v["name"].is_string(), "name must be a string: {v}");
    assert!(
        matches!(
            v["language"].as_str(),
            Some("rust" | "node" | "python" | "go" | "ruby" | "unknown")
        ),
        "language must be a known value: {v}"
    );
    assert!(v["has_cli"].is_boolean(), "has_cli must be bool: {v}");

    // `diag` always present as an array — the stability contract for
    // consumers (`profile["diag"]` MUST NOT KeyError, even on clean runs).
    let diag = v["diag"]
        .as_array()
        .expect("diag must always serialize as an array, even when empty");
    // Fixture binary un-built → has_cli=false → at least one falsy branch
    // pushed a note; assert the populated-entry shape here.
    assert!(
        !diag.is_empty(),
        "expected diag notes on unbuilt fixture: {v}"
    );
    for note in diag {
        assert!(
            note["stage"].is_string(),
            "diag entry stage must be string: {note}"
        );
        assert!(
            note["note"].is_string(),
            "diag entry note must be string: {note}"
        );
    }
    assert!(
        diag.iter()
            .any(|n| n["stage"].as_str() == Some("detect_cli")),
        "expected at least one detect_cli-stage note: {v}"
    );

    // Now exercise the EMPTY-diag contract: doctor against the skillpack
    // repo itself (built binary on PATH → has_cli=true → clean trace).
    // `diag` must still serialize as an empty array, NOT be omitted.
    Command::new("cargo")
        .args(["build", "--release", "--quiet"])
        .current_dir(repo_root())
        .assert()
        .success();
    let clean_out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["doctor", "--root", ".", "--format", "json"])
        .current_dir(repo_root())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let cv: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&clean_out)).unwrap();
    assert_eq!(cv["has_cli"], serde_json::Value::Bool(true));
    assert_eq!(
        cv["diag"].as_array().map(std::vec::Vec::len),
        Some(0),
        "diag must be present as [] on clean runs, not omitted: {cv}"
    );
}

// --- workspace member walks -------------------------------------------------
//
// `walk_cargo_workspace` and `walk_npm_workspace` silently change which target
// `has_cli` resolves to: the binary lives in a *member*, not the root. If a
// regression turns the walk off, doctor must still honestly report
// `has_cli=false` with a trace — it must NOT claim `true` and then leave the
// agent unable to spawn. These pin both directions: a member that exists
// yields `true`; a workspace with no matching member yields `false`.

/// Scratch a cargo workspace root + one member crate with a `[[bin]]`.
/// Returns the temp dir path (kept alive via `tempdir().keep()`).
fn cargo_workspace_scratch() -> PathBuf {
    let root = tempfile::tempdir().unwrap().keep();
    // Workspace-only Cargo.toml (no [package]).
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"cli-crate\"]\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("cli-crate")).unwrap();
    fs::create_dir_all(root.join("cli-crate/src")).unwrap();
    fs::write(
        root.join("cli-crate/Cargo.toml"),
        "[package]\nname = \"cli-crate\"\nversion = \"0.1.0\"\n[[bin]]\nname = \"cli-crate\"\npath = \"src/main.rs\"\n",
    )
    .unwrap();
    // Trivial main.rs so the crate is runnable.
    fs::write(root.join("cli-crate/src/main.rs"), "fn main() {}\n").unwrap();
    root
}

/// Build the member crate's binary so `rust_cli_candidate` resolves it, then
/// `doctor` against the workspace root and assert `has_cli: true`.
///
/// Cargo hoists member artifacts to the workspace-root `target/`, so
/// `primary_cli_candidate` finds the binary at the root level — the
/// member-walk trace may or may not fire (it only fires when the binary
/// isn't hoisted/on PATH). Assert `has_cli: true` + the language note; don't
/// over-assert on which code path found it.
#[test]
fn doctor_cargo_workspace_finds_member_cli() {
    let root = cargo_workspace_scratch();

    // Build the member crate's binary; cargo hoists it to the workspace-root
    // target/release/cli-crate.
    Command::new("cargo")
        .args(["build", "--release", "--quiet"])
        .current_dir(root.join("cli-crate"))
        .assert()
        .success();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["doctor", "--root", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "doctor must exit 0 (read-only), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("has_cli:  true"),
        "workspace with a member bin must report has_cli=true, got:\n{s}"
    );
    // The workspace detection note must fire (explains why detection probed
    // member artifacts at all). The CLI-walk trace only fires when the binary
    // isn't found at the hoist point — so assert the language note, not the
    // walk path.
    assert!(
        s.contains("workspace"),
        "doctor trace must reference the workspace detection, got:\n{s}"
    );
}

/// A cargo workspace whose member has NO built/installed binary must report
/// `has_cli=false` with a trace ending "no workspace member yielded a
/// runnable CLI" — the honest no-promise path.
#[test]
fn doctor_cargo_workspace_no_member_cli_reports_false() {
    let root = cargo_workspace_scratch();
    // Do NOT build — no target/ artifact, no PATH bin → candidate None.

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["doctor", "--root", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(out.status.success(), "doctor is read-only (exit 0)");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("has_cli:  false"),
        "workspace with unbuilt member must report has_cli=false, got:\n{s}"
    );
    assert!(
        s.contains("no workspace member yielded a runnable CLI"),
        "trace must explain the false, got:\n{s}"
    );
}

/// Scratch an npm workspace root + one member package with a `bin`.
/// Requires `node` on PATH (the test skips otherwise — mirrors
/// `node_available()` above).
fn npm_workspace_scratch() -> PathBuf {
    let root = tempfile::tempdir().unwrap().keep();
    // Workspace-only package.json (no root `bin`).
    fs::write(
        root.join("package.json"),
        "{ \"name\": \"ws-root\", \"workspaces\": [\"cli-pkg\"] }\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("cli-pkg/bin")).unwrap();
    fs::write(
        root.join("cli-pkg/package.json"),
        "{ \"name\": \"cli-pkg\", \"bin\": { \"cli-pkg\": \"./bin/cli.js\" } }\n",
    )
    .unwrap();
    // Trivial cli.js with shebang so node runs it cleanly.
    fs::write(
        root.join("cli-pkg/bin/cli.js"),
        "#!/usr/bin/env node\nconsole.log('cli-pkg help');\n",
    )
    .unwrap();
    root
}

/// npm workspace: doctor must find the member's `bin` and report
/// `has_cli=true` with a `detect_cli.node.workspace` trace entry.
#[test]
fn doctor_npm_workspace_finds_member_cli() {
    if !node_available() {
        return;
    }
    let root = npm_workspace_scratch();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["doctor", "--root", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "doctor must exit 0, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("has_cli:  true"),
        "npm workspace with a member bin must report has_cli=true, got:\n{s}"
    );
    assert!(
        s.contains("detect_cli.node.workspace"),
        "trace must mention the npm workspace walk, got:\n{s}"
    );
}

/// npm workspace whose member has no `bin` must report `has_cli=false`.
#[test]
fn doctor_npm_workspace_no_member_bin_reports_false() {
    if !node_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    fs::write(
        root.join("package.json"),
        "{ \"name\": \"ws-root\", \"workspaces\": [\"lib-pkg\"] }\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("lib-pkg")).unwrap();
    fs::write(
        root.join("lib-pkg/package.json"),
        "{ \"name\": \"lib-pkg\" }\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["doctor", "--root", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("has_cli:  false"),
        "npm workspace with no member bin must report has_cli=false, got:\n{s}"
    );
}

// --- OpenCode + Copilot init+verify round trips ----------------------------

/// `init --target opencode --target copilot` writes both ecosystem files,
/// and `verify` passes its discovery checks against them. Asserts the
/// OpenCode agent file has `---` frontmatter with `description:` and the
/// Copilot file starts with a `#` heading.
#[test]
fn opencode_copilot_init_then_verify_round_trip() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "opencode",
            "--target",
            "copilot",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    let opencode_path = root.join(".opencode/agents/sample-rust.md");
    let copilot_path = root.join(".github/copilot-instructions.md");
    assert!(opencode_path.exists(), "OpenCode agent file must exist");
    assert!(
        copilot_path.exists(),
        "Copilot instructions file must exist"
    );

    // Structural assertions: OpenCode file has `---` frontmatter with a
    // `description:` key; Copilot file starts with a `#` heading.
    let opencode_raw = fs::read_to_string(&opencode_path).unwrap();
    assert!(
        opencode_raw.starts_with("---\n"),
        "OpenCode agent file must start with frontmatter, got:\n{opencode_raw}"
    );
    assert!(
        opencode_raw.contains("description:"),
        "OpenCode frontmatter must have description, got:\n{opencode_raw}"
    );

    let copilot_raw = fs::read_to_string(&copilot_path).unwrap();
    assert!(
        copilot_raw.starts_with("# "),
        "Copilot instructions must start with a `#` heading, got:\n{copilot_raw}"
    );

    // Verify passes and emits the discovery checks for both ecosystems.
    let json_out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        json_out.status.success(),
        "verify must exit 0, got:\n{}",
        String::from_utf8_lossy(&json_out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(json["ok"], serde_json::Value::Bool(true));

    let results = json["results"].as_array().unwrap();
    for check_id in ["discovery.opencode.agent", "discovery.copilot.instructions"] {
        let matches: Vec<&serde_json::Value> = results
            .iter()
            .filter(|r| r["check_id"] == check_id)
            .collect();
        assert!(
            !matches.is_empty(),
            "verify must emit {check_id} result, got:\n{json}"
        );
        for r in &matches {
            assert_eq!(r["severity"], "pass", "{check_id} must pass, got:\n{json}");
        }
    }
}

/// An OpenCode agent file with no `---` frontmatter fails verify with
/// `discovery.opencode.agent.frontmatter`. Regression guard for the
/// frontmatter-present check.
#[test]
fn opencode_missing_frontmatter_fails_verify() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "opencode",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Overwrite with a file missing the `---` frontmatter block entirely.
    fs::write(
        root.join(".opencode/agents/sample-rust.md"),
        "# sample-rust\n\nNo frontmatter here, just markdown.\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "verify must exit non-zero on missing frontmatter, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let name = "discovery.opencode.agent.frontmatter";
    let matches: Vec<&serde_json::Value> =
        results.iter().filter(|r| r["check_id"] == name).collect();
    assert!(
        !matches.is_empty(),
        "verify must emit {name} result, got:\n{json}"
    );
    for r in &matches {
        assert_eq!(
            r["severity"], "fail",
            "{name} must be fail severity, got:\n{json}"
        );
    }
}

/// A Copilot instructions file that starts with a `---` frontmatter block
/// fails verify with `discovery.copilot.instructions` at fail severity.
/// The Copilot spec (schema.rs) says "Plain markdown, no frontmatter."
#[test]
fn copilot_frontmatter_fails_verify() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "copilot",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // Overwrite with a file that has a `---` frontmatter block.
    fs::write(
        root.join(".github/copilot-instructions.md"),
        "---\ndescription: x\n---\n# Foo\n",
    )
    .unwrap();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "verify must exit non-zero on frontmatter present, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let name = "discovery.copilot.instructions";
    let matches: Vec<&serde_json::Value> =
        results.iter().filter(|r| r["check_id"] == name).collect();
    assert!(
        !matches.is_empty(),
        "verify must emit {name} result, got:\n{json}"
    );
    for r in &matches {
        assert_eq!(
            r["severity"], "fail",
            "{name} must be fail severity, got:\n{json}"
        );
    }
}

/// An empty `.codex/skills/` directory (dir present, no SKILL.md) emits
/// `discovery.codex.skill.missing` at fail severity. Regression guard for the
/// empty-dir sentinel added alongside the existing Claude `discovery.skill.missing`.
#[test]
fn codex_empty_skills_dir_fails_verify() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "codex",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    assert!(root.join(".codex/skills").is_dir());

    // Wipe the generated SKILL.md, leaving `.codex/skills/` as an empty dir.
    let skill_dir = root.join(".codex/skills/sample-rust");
    fs::remove_file(skill_dir.join("SKILL.md")).unwrap();
    fs::remove_dir(&skill_dir).unwrap();
    assert!(root.join(".codex/skills").exists());

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "verify must exit non-zero on empty .codex/skills/, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let name = "discovery.codex.skill.missing";
    let matches: Vec<&serde_json::Value> =
        results.iter().filter(|r| r["check_id"] == name).collect();
    assert!(
        !matches.is_empty(),
        "verify must emit {name} result, got:\n{json}"
    );
    for r in &matches {
        assert_eq!(
            r["severity"], "fail",
            "{name} must be fail severity, got:\n{json}"
        );
    }
}

// --- All-5-targets round trip ------------------------------------------------

#[test]
fn all_five_targets_init_then_verify_round_trip() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();
    write_skillpack_toml(&root, "sample-rust");

    Command::cargo_bin("skillpack")
        .unwrap()
        .args([
            "init",
            "--target",
            "claude",
            "--target",
            "cursor",
            "--target",
            "codex",
            "--target",
            "opencode",
            "--target",
            "copilot",
            "--root",
            ".",
            "--non-interactive",
            "--accept-warnings",
        ])
        .current_dir(&root)
        .assert()
        .success();

    // All 5 file types exist.
    assert!(root.join(".claude-plugin/marketplace.json").exists());
    assert!(root.join(".cursor/rules/sample-rust.mdc").exists());
    assert!(root.join(".codex/skills/sample-rust/SKILL.md").exists());
    assert!(root.join(".opencode/agents/sample-rust.md").exists());
    assert!(root.join(".github/copilot-instructions.md").exists());

    // Verify all 5 discovery check_ids have severity pass.
    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "verify must pass, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let expected_ids = [
        "discovery.skill",
        "discovery.codex.skill",
        "discovery.cursor.mdc",
        "discovery.opencode.agent",
        "discovery.copilot.instructions",
    ];
    for id in &expected_ids {
        let matches: Vec<&serde_json::Value> =
            results.iter().filter(|r| r["check_id"] == *id).collect();
        assert!(!matches.is_empty(), "verify must emit {id}, got:\n{json}");
        for r in &matches {
            assert_eq!(r["severity"], "pass", "{id} must be pass, got:\n{json}");
        }
    }
}

// --- discovery.empty on a bare repo -------------------------------------------

#[test]
fn verify_on_empty_repo_fails_with_discovery_empty() {
    let root = copy_fixture("rust-cli");
    // Build the binary so it exists, but do NOT run init.
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!out.status.success(), "verify on empty repo must fail");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    let matches: Vec<&serde_json::Value> = results
        .iter()
        .filter(|r| r["check_id"] == "discovery.empty")
        .collect();
    assert!(
        !matches.is_empty(),
        "must emit discovery.empty, got:\n{json}"
    );
    assert_eq!(matches[0]["severity"], "fail");
}

// --- Self-dogfood verifies all ecosystems -----------------------------------

#[test]
fn self_dogfood_verify_asserts_all_ecosystems() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["verify", "--root", ".", "--format", "json"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "self-dogfood verify must pass, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = json["results"].as_array().unwrap();

    let check_expectations = [
        ("discovery.marketplace", "pass"),
        ("discovery.skill", "pass"),
        ("discovery.cursor.mdc", "pass"),
        ("discovery.codex.skill", "pass"),
        ("discovery.opencode.agent", "pass"),
        ("discovery.copilot.instructions", "pass"),
    ];

    for (id, severity) in &check_expectations {
        let matches: Vec<&serde_json::Value> =
            results.iter().filter(|r| r["check_id"] == *id).collect();
        assert!(
            !matches.is_empty(),
            "self-dogfood must emit {id}, got:\n{json}"
        );
        assert_eq!(
            matches[0]["severity"], *severity,
            "{id} must be {severity}, got:\n{json}"
        );
    }
}

// --- Doctor on non-workspace project with built binary ------------------------

#[test]
fn doctor_on_plain_rust_cli_reports_has_cli_true() {
    let root = copy_fixture("rust-cli");
    Command::new("cargo")
        .args(["build", "--quiet"])
        .current_dir(&root)
        .assert()
        .success();

    let out = Command::cargo_bin("skillpack")
        .unwrap()
        .args(["doctor", "--root", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(out.status.success(), "doctor must exit 0");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("has_cli:  true"),
        "doctor should report has_cli: true, got:\n{s}"
    );
}
