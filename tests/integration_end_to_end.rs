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

// --- multi-ecosystem init+verify round trips (design §11: all five ecosystems)
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
