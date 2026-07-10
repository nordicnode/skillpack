//! Repo introspection. Produces a [`ProjectProfile`] from pure filesystem
//! reads, plus one guarded `--help` spawn when a CLI binary is detected.
//!
//! Design §6.3: "No side effects. Pure filesystem reads. Spawns `--help` only
//! when a CLI binary is detected ... guarded by a hard timeout and runs in a
//! working directory restricted to the project root."
//!
//! The five supported ecosystems (design §11): Rust, npm, Python, Go, Ruby.
//! Detection order is deliberate: if both a `Cargo.toml` and a `package.json`
//! exist we pick the one most likely to *ship a CLI* (Rust, then node), which
//! matches the polyglot-monorepo reality.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Result;

use crate::types::{Language, ProjectProfile};

/// Hard cap on the `--help` spawn. A CLI that can't print its help in 5s is
/// not something we want an agent invoking anyway, so this protects both the
/// tool and the agent downstream.
const HELP_TIMEOUT: Duration = Duration::from_secs(5);
/// We only read the first slice of the README to bound cost.
const README_HEAD_LINES: usize = 500;

/// Introspect the project at `root`. `root` must be the OSS project root
/// (the directory containing the language manifest).
pub fn introspect(root: &Path) -> Result<ProjectProfile> {
    anyhow::ensure!(root.is_dir(), "{} is not a directory", root.display());

    let language = detect_language(root);
    let manifest_name = project_manifest_name(root, language);
    let repo_url = detect_repo_url(root);
    let license = detect_license(root).or_else(|| manifest_license(root, language));
    let description_hint = read_readme_hint(root);
    let (has_cli, cli_command, cli_help_output) = detect_cli(root, language, manifest_name.clone());

    let name = manifest_name
        .or_else(|| repo_url_name(&repo_url))
        .unwrap_or_else(|| {
            // Last resort: the directory name itself. Canonicalize first so a
            // bare `--root .` (the documented default) resolves to the real cwd
            // tail instead of `Path::new(".").file_name() == None` → "unknown-tool".
            std::fs::canonicalize(root)
                .ok()
                .and_then(|c| c.file_name().map(|n| n.to_string_lossy().to_string()))
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .and_then(|c| c.file_name().map(|n| n.to_string_lossy().to_string()))
                })
                .unwrap_or_else(|| "unknown-tool".to_string())
        });

    Ok(ProjectProfile {
        name,
        language,
        has_cli,
        cli_command,
        cli_help_output,
        repo_url,
        license,
        description_hint,
    })
}

/// Detect the dominant language by checking for known manifests.
pub fn detect_language(root: &Path) -> Language {
    if root.join("Cargo.toml").exists() {
        Language::Rust
    } else if root.join("package.json").exists() {
        Language::Node
    } else if root.join("pyproject.toml").exists()
        || root.join("setup.py").exists()
        || root.join("setup.cfg").exists()
    {
        Language::Python
    } else if root.join("go.mod").exists() {
        Language::Go
    } else if root.join("Gemfile").exists() || has_gemspec(root) {
        Language::Ruby
    } else {
        Language::Unknown
    }
}

/// True if the root contains any `*.gemspec` file.
fn has_gemspec(root: &Path) -> bool {
    fs::read_dir(root).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("gemspec"))
    })
}

/// Pull the project name out of the language manifest, best-effort.
fn project_manifest_name(root: &Path, language: Language) -> Option<String> {
    match language {
        Language::Rust => {
            // Parse Cargo.toml with the real toml crate (same path as Python)
            // instead of hand-rolling line scans: a hand-scan misreads `name="x"`
            // (no space before `=`) and `name = { workspace = true }` (extracts
            // "{ workspace" as the name). toml does both correctly, and returns
            // None for workspace-inherited names so the caller falls through.
            let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        }
        Language::Node => {
            let raw = fs::read_to_string(root.join("package.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            v.get("name")?
                .as_str()
                .map(std::string::ToString::to_string)
        }
        Language::Python => {
            // pyproject.toml [project] name = "..."
            if let Ok(raw) = fs::read_to_string(root.join("pyproject.toml")) {
                if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
                    if let Some(name) = v
                        .get("project")
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                    {
                        return Some(name.to_string());
                    }
                }
            }
            None
        }
        Language::Go => {
            // Go: derive a name from the module path's last segment.
            let raw = fs::read_to_string(root.join("go.mod")).ok()?;
            let module_line = raw
                .lines()
                .find(|l| l.trim_start().starts_with("module "))?;
            let path = module_line
                .trim()
                .strip_prefix("module ")
                .map(|s| s.trim().to_string())?;
            let last = path.rsplit('/').next()?.to_string();
            Some(last)
        }
        Language::Ruby => {
            // *.gemspec: spec.name = "..."
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("gemspec") {
                        if let Ok(raw) = fs::read_to_string(&p) {
                            if let Some(line) = raw
                                .lines()
                                .find(|l| l.contains("spec.name") || l.contains(".name ="))
                            {
                                if let Some(name) = extract_ruby_string_value(line) {
                                    return Some(name);
                                }
                            }
                        }
                    }
                }
            }
            None
        }
        Language::Unknown => None,
    }
}

/// Detect whether the project ships an invokable CLI, and if so capture its
/// `--help` output under a hard timeout. Returns `(has_cli, command, output)`.
///
/// `command` is the full multi-token `--help` argv the verifier re-spawns (e.g.
/// `["node","/abs/bin/cli.js","--help"]`, `["go","run",".","--help"]`). The
/// bare human-facing invocation that SKILL.md publishes is derived separately
/// from the profile name + interview — this is the internal, machine-specific
/// spawn argv (design §5.1, §6.3).
fn detect_cli(
    root: &Path,
    language: Language,
    name: Option<String>,
) -> (bool, Option<Vec<String>>, Option<String>) {
    let Some(name) = name else {
        return (false, None, None);
    };

    let Some(candidate) = primary_cli_candidate(root, language, &name) else {
        // No runnable CLI could be established on this machine (e.g. the
        // language runtime — `go`/`ruby` — isn't installed). We honestly
        // report `has_cli = false` rather than guessing; the maintainer gets
        // the pure-library interview path, which is the safe default.
        return (false, None, None);
    };

    // Build the spawn command from the multi-token argv (program + args, minus
    // `--help`), then append `--help` for the help capture.
    let mut command = candidate.argv.clone();
    command.push("--help".to_string());

    let mut cmd = Command::new(&candidate.argv[0]);
    for arg in &candidate.argv[1..] {
        cmd.arg(arg);
    }
    cmd.arg("--help")
        .current_dir(&candidate.spawn_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match spawn_with_timeout(&mut cmd, HELP_TIMEOUT) {
        SpawnOutcome::RanClean(output) => (true, Some(command), Some(output)),
        SpawnOutcome::RanNonZero(_output) => (true, Some(command), None),
        SpawnOutcome::TimedOut => (true, Some(command), None),
        SpawnOutcome::NotFound => (false, None, None),
    }
}

/// A resolved CLI invocation ready to spawn `--help`. The argv excludes the
/// trailing `--help` (which `detect_cli` appends). `spawn_cwd` is the working
/// directory the CLI needs to run in — the project root for relative-invocation
/// CLIs (`go run .`, a `package.json` bin script); for CLIs resolved to an
/// absolute path it's still the root so the spawn matches what `verify` does.
#[derive(Debug, Clone)]
struct CliCandidate {
    /// Full argv excluding `--help`, e.g. `["node","/abs/bin/cli.js"]`,
    /// `["go","run","."]`, or `["/abs/target/debug/sample-rust"]`.
    argv: Vec<String>,
    /// Working directory for the spawn (the project root).
    spawn_cwd: PathBuf,
}

/// Resolve the CLI invocation for the detected language. Returns `None` when no
/// runnable CLI can be established on this machine (an honest `has_cli = false`
/// — the runtime may be missing, no build artifact present, no entry point).
/// Module-private; the unit tests in this file (same module) call it directly
/// to assert per-language argv without spawning a process.
fn primary_cli_candidate(root: &Path, language: Language, name: &str) -> Option<CliCandidate> {
    match language {
        Language::Rust => rust_cli_candidate(root, name),
        Language::Node => node_cli_candidate(root, name),
        Language::Go => go_cli_candidate(root, name),
        Language::Python => python_cli_candidate(root, name),
        Language::Ruby => ruby_cli_candidate(root, name),
        Language::Unknown => which_on_path(name).map(|_| CliCandidate {
            argv: vec![name.to_string()],
            spawn_cwd: root.to_path_buf(),
        }),
    }
}

/// Rust: a built artifact under `target/{release,debug}/<name>`, canonicalized
/// to an absolute path so it survives a later cwd change (the pre-commit
/// verify spawns from a temp dir). Falls back to a PATH probe for an installed
/// bin, then to the dir-derived name.
fn rust_cli_candidate(root: &Path, name: &str) -> Option<CliCandidate> {
    for profile in &["release", "debug"] {
        let p = root.join("target").join(profile).join(name);
        if p.exists() {
            // Canonicalize so the stored argv survives a later cwd change (the
            // pre-commit verify spawns from a temp dir). Falls back to the
            // joined path if canonicalize fails on some platforms.
            let abs = std::fs::canonicalize(&p)
                .ok()
                .and_then(|c| c.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| p.to_string_lossy().to_string());
            return Some(CliCandidate {
                argv: vec![abs],
                spawn_cwd: root.to_path_buf(),
            });
        }
    }
    // A package may rename its bin via [[bin]] name; falling back to the
    // directory-derived name on PATH is acceptable for introspection.
    which_on_path(name).map(|_| CliCandidate {
        argv: vec![name.to_string()],
        spawn_cwd: root.to_path_buf(),
    })
}

/// Node: a `package.json` `bin` field (string or object) points at a JS
/// script. Resolve it to an absolute path and run `node <abs script>` so the
/// project's CLI works uninstalled and survives a cwd change. Requires `node`
/// on PATH (honest `None` otherwise).
fn node_cli_candidate(root: &Path, name: &str) -> Option<CliCandidate> {
    let node = which_on_path("node")?;
    let node_bin = node.to_string_lossy().to_string();
    let raw = fs::read_to_string(root.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let bin = v.get("bin")?;
    // `bin` may be a string ("./cli.js") or an object mapping name → script.
    // We pick the first script (preferring an entry keyed by the tool name).
    let script = match bin {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            // Pick the entry keyed by the tool name if present (the primary
            // bin), otherwise fall back to the first script entry. Handles
            // multi-bin packages while keeping single-bin packages simple.
            map.get(name)
                .and_then(|v| v.as_str())
                .or_else(|| map.iter().next().and_then(|(_, v)| v.as_str()))?
                .to_string()
        }
        _ => return None,
    };
    if script.trim().is_empty() {
        return None;
    }
    // Resolve to an absolute path so `node <abs script> --help` works whether
    // or not the package is installed, and survives the temp-dir spawn cwd.
    let script_path = root.join(&script);
    let abs_script = std::fs::canonicalize(&script_path)
        .ok()
        .and_then(|c| c.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| script_path.to_string_lossy().to_string());
    Some(CliCandidate {
        argv: vec![node_bin, abs_script],
        spawn_cwd: root.to_path_buf(),
    })
}

/// Go: invoke `go run .` from the project root (the canonical way to run an
/// uninstalled Go CLI). Requires `go` on PATH and a `package main` source at
/// root. Honest `None` when `go` is missing (the dev-machine case here).
fn go_cli_candidate(root: &Path, _name: &str) -> Option<CliCandidate> {
    which_on_path("go")?;
    if !has_go_main(root) {
        return None;
    }
    // `go run .` is cwd-relative by design; the spawn runs in the project root
    // (the pre-commit verify passes root as spawn_cwd, so this stays correct).
    Some(CliCandidate {
        argv: vec!["go".to_string(), "run".to_string(), ".".to_string()],
        spawn_cwd: root.to_path_buf(),
    })
}

/// True iff `root` contains a non-test `.go` file declaring `package main`.
fn has_go_main(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("go") {
            continue;
        }
        // _test.go files aren't runnable entry points.
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_test.go"))
        {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&p) {
            if raw.lines().any(|l| l.trim() == "package main") {
                return true;
            }
        }
    }
    false
}

/// Python: prefer `python -m <pkg>` against an importable package dir at the
/// root (the canonical uninstalled invocation). Fall back to an installed
/// console-script on PATH. Honest `None` when neither is runnable.
fn python_cli_candidate(root: &Path, name: &str) -> Option<CliCandidate> {
    let python = which_on_path("python")
        .or_else(|| which_on_path("python3"))
        .map(|p| p.to_string_lossy().to_string())?;

    // A `pyproject.toml` `[project.scripts]` entry maps the console-script
    // name to `<pkg>.<module>:<func>`. We extract the package and, if it's
    // importable as a directory at the root, invoke `python -m <pkg>`.
    if let Some(pkg) = python_script_package(root, name) {
        if root.join(&pkg).is_dir() {
            return Some(CliCandidate {
                argv: vec![python, "-m".to_string(), pkg],
                spawn_cwd: root.to_path_buf(),
            });
        }
    }

    // Installed console script on PATH (e.g. `pip install -e .` already run).
    if which_on_path(name).is_some() {
        return Some(CliCandidate {
            argv: vec![name.to_string()],
            spawn_cwd: root.to_path_buf(),
        });
    }

    None
}

/// Extract the top-level package name from a `pyproject.toml` `[project.scripts]`
/// entry whose key matches `name` (e.g. `sample-python = "sample_python.cli:main"`
/// → `sample_python`). Returns `None` if no such entry / no importable target.
fn python_script_package(root: &Path, name: &str) -> Option<String> {
    let raw = fs::read_to_string(root.join("pyproject.toml")).ok()?;
    let v: toml::Value = toml::from_str(&raw).ok()?;
    let scripts = v
        .get("project")
        .and_then(|p| p.get("scripts"))?
        .as_table()?;
    let target = scripts.get(name)?.as_str()?;
    // target is "<pkg>.<module>:<func>" — take the segment before the colon,
    // then the leading dotted path's first component as the package name.
    let module = target.split(':').next()?.trim();
    module.split('.').next().map(|s| s.to_string())
}

/// Ruby: structural only — an `exe/<name>` or `bin/<name>` binstub invoked as
/// `ruby <abs path>`. Honest `None` when there's no binstub or no ruby runtime.
fn ruby_cli_candidate(root: &Path, name: &str) -> Option<CliCandidate> {
    which_on_path("ruby")
        .or_else(|| which_on_path("bundle"))
        .map(|b| b.to_string_lossy().to_string())?;
    for dir in &["exe", "bin"] {
        let p = root.join(dir).join(name);
        if p.is_file() {
            let abs = std::fs::canonicalize(&p)
                .ok()
                .and_then(|c| c.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| p.to_string_lossy().to_string());
            return Some(CliCandidate {
                argv: vec!["ruby".to_string(), abs],
                spawn_cwd: root.to_path_buf(),
            });
        }
    }
    None
}

enum SpawnOutcome {
    /// Exited 0; output captured.
    RanClean(String),
    /// Exited non-zero; partial output available but we treat help as unset.
    RanNonZero(String),
    /// Did not finish within the timeout (killed).
    TimedOut,
    /// Binary not found / could not be spawned.
    NotFound,
}

fn spawn_with_timeout(cmd: &mut Command, timeout: Duration) -> SpawnOutcome {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SpawnOutcome::NotFound,
        Err(_) => return SpawnOutcome::NotFound,
    };
    // Poll the child until it exits or we hit the timeout.
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                // The child has exited. `wait_with_output` drains the piped
                // stdout/stderr (buffered in the handles `try_wait` left
                // untouched) and returns the exit status, so we use it rather
                // than the status we just probed — avoiding a second wait.
                break match child.wait_with_output() {
                    Ok(o) => {
                        let s = format!(
                            "{}{}",
                            String::from_utf8_lossy(&o.stdout),
                            String::from_utf8_lossy(&o.stderr)
                        );
                        if o.status.success() {
                            SpawnOutcome::RanClean(s)
                        } else {
                            SpawnOutcome::RanNonZero(s)
                        }
                    }
                    // Pipes already drained by a prior reap; treat as
                    // non-zero with no captured text.
                    Err(_) => SpawnOutcome::RanNonZero(String::new()),
                };
            }
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return SpawnOutcome::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return SpawnOutcome::TimedOut,
        }
    }
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    // ponytail: this is a Unix-shaped PATH probe. Ceilings:
    //  - a file that exists but lacks the exec bit (cataloged a fixture script
    //    without +x, or a Windows .exe without PATHEXT matching) returns the
    //    file here but `spawn()` then fails → mapped to `NotFound` upstream → an
    //    honest `has_cli = false`, never a crash. Acceptable for V1.
    //  - Windows: PATH lookups don't append PATHEXT, so probing bare "`node`"
    //    misses `node.exe`. A Windows run needs `PATHEXT` enumeration + the
    //    `is_file` test against each `name{ext}`. Add it when skillpack ships a
    //    native-Windows build (V1 is unix-targeted; CI is ubuntu-latest). The
    //    README "Platform" note documents this ceiling so a Windows run's
    //    silent `has_cli=false` isn't a surprise.
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// `git remote get-url origin`, best-effort. Never errors the caller.
fn detect_repo_url(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Heuristic: read LICENSE, look for the SPDX id text.
fn detect_license(root: &Path) -> Option<String> {
    for filename in &["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"] {
        let p = root.join(filename);
        if let Ok(raw) = fs::read_to_string(&p) {
            let head = raw.split('\n').take(3).collect::<Vec<_>>().join("\n");
            let lower = head.to_lowercase();
            if lower.contains("mit license") || lower.contains("permission is hereby granted") {
                return Some("MIT".to_string());
            }
            if lower.contains("apache license") {
                return Some("Apache-2.0".to_string());
            }
            if lower.contains("bsd 3-clause") || lower.contains("neither the name") {
                return Some("BSD-3-Clause".to_string());
            }
            if lower.contains("gnu general public license") {
                return Some("GPL-3.0".to_string());
            }
        }
    }
    None
}

fn manifest_license(root: &Path, language: Language) -> Option<String> {
    match language {
        Language::Node => {
            let raw = fs::read_to_string(root.join("package.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            v.get("license")?
                .as_str()
                .map(std::string::ToString::to_string)
        }
        Language::Rust => {
            let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("package")
                .and_then(|p| p.get("license"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    }
}

/// First paragraph(s) of the README, capped for cost. Used only as a *hint*
/// surfaced under `--verbose`; the interview is the source of truth.
fn read_readme_hint(root: &Path) -> Option<String> {
    for filename in &["README.md", "README", "readme.md"] {
        let p = root.join(filename);
        if let Ok(raw) = fs::read_to_string(&p) {
            let head: String = raw
                .lines()
                .take(README_HEAD_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            // Find the first non-heading, non-empty prose paragraph.
            let paragraph = head
                .lines()
                .skip_while(|l| {
                    let t = l.trim();
                    t.is_empty() || t.starts_with('#') || t.starts_with('!')
                })
                .take_while(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let trimmed = paragraph.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn repo_url_name(repo_url: &Option<String>) -> Option<String> {
    let url = repo_url.as_ref()?;
    let last = url.rsplit('/').next()?.trim_end();
    let stem = last.strip_suffix(".git").unwrap_or(last);
    Some(stem.to_string())
}

fn extract_ruby_string_value(line: &str) -> Option<String> {
    let after = line.split('=').nth(1)?.trim();
    let s = after.trim_start_matches(['"', '\'']);
    let s = s.split(['"', '\'']).next()?.trim();
    Some(s.to_string())
}

#[cfg(test)]
impl ProjectProfile {
    /// Test helper: a profile with everything falsy, for assembling fixtures.
    pub fn test_default() -> Self {
        Self {
            name: "test-tool".to_string(),
            language: Language::Unknown,
            has_cli: false,
            cli_command: None,
            cli_help_output: None,
            repo_url: None,
            license: None,
            description_hint: None,
        }
    }
}

#[cfg(test)]
mod candidate_tests {
    //! Tests for per-language CLI candidate *resolution* (not spawning). These
    //! assert the argv we'd spawn without running a subprocess, so they stay
    //! green on machines that don't have every runtime installed.

    use super::*;
    use crate::types::Language;

    /// Build a throwaway project root under the temp dir, lay down `files`,
    /// and return its path. Each call gets a unique directory — Rust runs unit
    /// tests concurrently in threads, so a shared scratch path would race and
    /// see its files overwritten or removed by a sibling test.
    fn scratch_root(files: &[(&str, &str)]) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir()
            .join(format!("skillpack-test-{}-{}", std::process::id(), n))
            .join("proj");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (rel, contents) in files {
            let p = root.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, contents).unwrap();
        }
        root
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn node_cli_detected_via_bin_absolute_argv() {
        // A `package.json` with a `bin` → script maps to `node <abs script>`.
        if which_on_path("node").is_none() {
            // node isn't on PATH on this machine; the candidate honestly
            // returns None. Assert that rather than skipping, so we still
            // exercise the runtime-present/absent branch.
            let root = scratch_root(&[
                ("package.json", r#"{"bin":{"sample-node":"./bin/cli.js"}}"#),
                ("bin/cli.js", "#!/usr/bin/env node\nconsole.log('x')\n"),
            ]);
            assert!(primary_cli_candidate(&root, Language::Node, "sample-node").is_none());
            cleanup(&root);
            return;
        }
        let root = scratch_root(&[
            ("package.json", r#"{"bin":{"sample-node":"./bin/cli.js"}}"#),
            ("bin/cli.js", "#!/usr/bin/env node\nconsole.log('x')\n"),
        ]);
        let cand = primary_cli_candidate(&root, Language::Node, "sample-node").unwrap();
        assert_eq!(cand.argv.len(), 2, "argv should be [node, <abs script>]");
        assert!(cand.argv[0].ends_with("node"), "got: {:?}", cand.argv);
        // the script path must be absolute so it survives a cwd change
        let script = &cand.argv[1];
        assert!(
            Path::new(script).is_absolute() && script.ends_with("bin/cli.js"),
            "expected absolute script path, got {}",
            script
        );
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }

    #[test]
    fn node_cli_string_bin_form() {
        if which_on_path("node").is_none() {
            return;
        }
        // `bin` as a bare string: {"bin": "./cli.js"}.
        let root = scratch_root(&[
            ("package.json", r#"{"bin":"./cli.js"}"#),
            ("cli.js", "console.log('x')\n"),
        ]);
        let cand = primary_cli_candidate(&root, Language::Node, "anything").unwrap();
        assert_eq!(cand.argv.len(), 2);
        assert!(cand.argv[1].ends_with("cli.js"));
        cleanup(&root);
    }

    #[test]
    fn go_candidate_none_when_go_missing() {
        // If `go` is on PATH (a CI machine) this branch isn't exercised; skip
        // rather than assert, so the test stays green where the runtime exists.
        if which_on_path("go").is_some() {
            return;
        }
        // Missing runtime AND a real main.go → None (honest has_cli=false).
        let root = scratch_root(&[("main.go", "package main\nfunc main(){}\n")]);
        assert!(primary_cli_candidate(&root, Language::Go, "sample-go").is_none());
        cleanup(&root);
    }

    #[test]
    fn go_candidate_uses_run_dot_when_go_present() {
        if which_on_path("go").is_none() {
            return;
        }
        let root = scratch_root(&[("main.go", "package main\nfunc main(){}\n")]);
        let cand = primary_cli_candidate(&root, Language::Go, "sample-go").unwrap();
        assert_eq!(cand.argv, vec!["go", "run", "."]);
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }

    #[test]
    fn go_candidate_none_without_package_main() {
        if which_on_path("go").is_none() {
            return;
        }
        // A library module (package foo, no main) is not a runnable CLI.
        let root = scratch_root(&[("main.go", "package foo\nfunc main(){}\n")]);
        assert!(primary_cli_candidate(&root, Language::Go, "sample-go").is_none());
        cleanup(&root);
    }

    #[test]
    fn python_candidate_uses_m_module_when_importable() {
        if which_on_path("python")
            .or_else(|| which_on_path("python3"))
            .is_none()
        {
            return;
        }
        let root = scratch_root(&[
            (
                "pyproject.toml",
                "[project]\nname = \"sample-python\"\n[project.scripts]\nsample-python = \"sample_python.cli:main\"\n",
            ),
            ("sample_python/__init__.py", ""),
            ("sample_python/cli.py", "def main(): pass\n"),
        ]);
        let cand = primary_cli_candidate(&root, Language::Python, "sample-python").unwrap();
        assert_eq!(cand.argv.len(), 3, "got: {:?}", cand.argv);
        assert!(
            cand.argv[0].ends_with("python"),
            "expected python interpreter, got {}",
            cand.argv[0]
        );
        assert_eq!(cand.argv[1], "-m");
        assert_eq!(cand.argv[2], "sample_python");
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }

    #[test]
    fn ruby_candidate_none_without_runtime() {
        if which_on_path("ruby")
            .or_else(|| which_on_path("bundle"))
            .is_some()
        {
            return;
        }
        // No binstub AND no runtime → None.
        let root = scratch_root(&[("Gemfile", "source \"https://rubygems.org\"\n")]);
        assert!(primary_cli_candidate(&root, Language::Ruby, "sample-ruby").is_none());
        cleanup(&root);
    }

    #[test]
    fn rust_candidate_fallback_to_path_probe() {
        // No built artifact in this scratch root → falls back to PATH, which
        // won't find a "totally-fake-bin-xyz" → None (honest).
        let root = scratch_root(&[("Cargo.toml", "[package]\nname = \"totally-fake-bin-xyz\"\n")]);
        let cand = primary_cli_candidate(&root, Language::Rust, "totally-fake-bin-xyz");
        assert!(cand.is_none());
        cleanup(&root);
    }
}

#[cfg(test)]
mod parse_tests {
    //! Bug #1 + #2: the Rust manifest name/license parsers used to hand-scan
    //! Cargo.toml lines, which misread `name="x"` (no space) and `name = { workspace
    //! = true }` (extracted "{ workspace" as the name). now go through the real
    //! toml crate — these tests pin both regressions.

    use super::*;

    fn scratch(files: &[(&str, &str)]) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir()
            .join(format!("skillpack-parse-{}-{}", std::process::id(), n))
            .join("proj");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (rel, contents) in files {
            std::fs::write(root.join(rel), contents).unwrap();
        }
        root
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rust_name_with_no_spaces_around_equals() {
        // name="revtool" — the old `starts_with("name =")` scan missed this.
        let root = scratch(&[(
            "Cargo.toml",
            "[package]\nname=\"revtool\"\nversion=\"0.1\"\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Rust).as_deref(),
            Some("revtool")
        );
        cleanup(&root);
    }

    #[test]
    fn rust_name_workspace_inherited_is_none() {
        // name = { workspace = true } — the old extract returned Some("{ workspace"),
        // which coerce_kebab turned into a plugin literally named "workspace".
        let root = scratch(&[(
            "Cargo.toml",
            "[package]\nname = { workspace = true }\nversion = \"0.1\"\n",
        )]);
        assert_eq!(project_manifest_name(&root, Language::Rust), None);
        cleanup(&root);
    }

    #[test]
    fn rust_license_with_no_spaces_around_equals() {
        // license="MIT" — same brittle scan hit license= (Bug #1).
        let root = scratch(&[("Cargo.toml", "[package]\nname = \"x\"\nlicense=\"MIT\"\n")]);
        assert_eq!(
            manifest_license(&root, Language::Rust).as_deref(),
            Some("MIT")
        );
        cleanup(&root);
    }

    #[test]
    fn rust_license_workspace_inherited_is_none() {
        let root = scratch(&[(
            "Cargo.toml",
            "[package]\nname = \"x\"\nlicense = { workspace = true }\n",
        )]);
        assert_eq!(manifest_license(&root, Language::Rust), None);
        cleanup(&root);
    }

    // Bug #3: a manifest with no name field and no git remote used to fall back
    // to the directory tail via `Path::new(".").file_name()` — which returns
    // None for `.` — emitting the literal "unknown-tool". Now we canonicalize
    // first, so a bare `--root .` resolves to the real cwd tail.
    #[test]
    fn unknown_root_dot_falls_back_to_canonicalized_dir_name() {
        let root = scratch(&[("package.json", "{}")]);
        let p = introspect(&root).unwrap();
        assert_ne!(
            p.name, "unknown-tool",
            "a real dir must resolve to its tail, not the unknown-tool sentinel"
        );
        assert_eq!(p.name, "proj");
        cleanup(&root);
    }

    // Bug #3 at the real boundary: introspect(".") must canonicalize to the cwd
    // tail, not return "unknown-tool" (Path::new(".").file_name() == None).
    #[test]
    fn introspect_dot_yields_cwd_tail_not_unknown_tool() {
        let p = introspect(Path::new(".")).unwrap();
        assert_ne!(p.name, "unknown-tool");
        let cwd_tail = std::env::current_dir()
            .ok()
            .and_then(|c| c.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        assert_eq!(p.name, cwd_tail);
    }
}
