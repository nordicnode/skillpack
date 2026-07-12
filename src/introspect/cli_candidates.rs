//! Per-language CLI candidate resolution. Pure filesystem reads + PATH probes
//! that build the `argv` `detect_cli` spawns `--help` against; no subprocess
//! of its own. Each `*_cli_candidate` returns `None` honestly when the
//! language runtime is missing rather than failing — an honest `has_cli=false`
//! is better than a spurious error.
//!
//! Split out of `introspect.rs` (0.8.5); the spawn/walk orchestrator now
//! lives in `super::cli_probe`, and the manifest-scalar readers in `super::manifest`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::types::Language;

/// The captured CLI surface: `detect_cli`'s return. Named (not a bare 4-tuple)
/// so the call site reads `d.has_cli` / `d.command` rather than decoding
/// positional fields — and clippy's `type_complexity` stops firing on the
/// `Option<Vec<...>>` pile.
pub(crate) struct DetectCli {
    pub(crate) has_cli: bool,
    pub(crate) command: Option<Vec<String>>,
    pub(crate) help_output: Option<String>,
    pub(crate) subcommand_help: Vec<(String, String)>,
}

impl DetectCli {
    pub(crate) fn none() -> Self {
        Self {
            has_cli: false,
            command: None,
            help_output: None,
            subcommand_help: Vec::new(),
        }
    }
}

/// A resolved CLI invocation ready to spawn `--help`. The argv excludes the
/// trailing `--help` (which `detect_cli` appends). `spawn_cwd` is the working
/// directory the CLI needs to run in — the project root for relative-invocation
/// CLIs (`go run .`, a `package.json` bin script); for CLIs resolved to an
/// absolute path it's still the root so the spawn matches what `verify` does.
#[derive(Debug, Clone)]
pub(crate) struct CliCandidate {
    /// Full argv excluding `--help`, e.g. `["node","/abs/bin/cli.js"]`,
    /// `["go","run","."]`, or `["/abs/target/debug/sample-rust"]`.
    pub(crate) argv: Vec<String>,
    /// Working directory for the spawn (the project root).
    pub(crate) spawn_cwd: PathBuf,
}

/// Windows-aware PATH lookup. cmd.exe appends `PATHEXT` (`cmd` → `cmd.exe`) to a
/// bare name; Rust's `Command::new` does not. Probe `name` plus `name{ext}`
/// for each ext in `PATHEXT` (e.g. `.EXE;.CMD;.BAT`) so a PATH lookup resolves
/// `node` to `node.exe`. On Unix the bare-name probe is unchanged (no
/// `PATHEXT`). Returns the resolved file path or `None` when not on PATH.
pub(crate) fn which_on_path(name: &str) -> Option<PathBuf> {
    let exts: Vec<String> = std::env::var("PATHEXT")
        .ok()
        .map(|p| p.split(';').map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let bare = dir.join(name);
        if bare.is_file() {
            return Some(bare);
        }
        for ext in &exts {
            let with_ext = match dir.join(format!("{name}{ext}")) {
                p if p.is_file() => p,
                _ => continue,
            };
            return Some(with_ext);
        }
    }
    None
}

/// Resolve the CLI invocation for the detected language. Returns `None` when no
/// runnable CLI can be established on this machine (an honest `has_cli = false`
/// — the runtime may be missing, no build artifact present, no entry point).
/// Module-private; the unit tests in the parent module call it directly to
/// assert per-language argv without spawning a process.
pub(crate) fn primary_cli_candidate(
    root: &Path,
    language: Language,
    name: &str,
) -> Option<CliCandidate> {
    match language {
        Language::Rust => rust_cli_candidate(root, name),
        Language::Node => node_cli_candidate(root, name),
        Language::Go => go_cli_candidate(root, name),
        Language::Python => python_cli_candidate(root, name),
        Language::Ruby => ruby_cli_candidate(root, name),
        Language::Php => php_cli_candidate(root, name),
        Language::Jvm => jvm_cli_candidate(root, name),
        Language::CSharp => csharp_cli_candidate(root, name),
        Language::Unknown => which_on_path(name).map(|_| CliCandidate {
            argv: vec![name.to_string()],
            spawn_cwd: root.to_path_buf(),
        }),
    }
}

/// Parse `[[bin]].name` entries from `Cargo.toml`. Returns bin names in
/// declaration order; empty when no `[[bin]]` tables (implicit single-bin
/// crate where the artifact matches the package name).
fn cargo_bin_names(root: &Path) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(root.join("Cargo.toml")) else {
        return Vec::new();
    };
    let Ok(v) = toml::from_str::<toml::Value>(&raw) else {
        return Vec::new();
    };
    v.get("bin")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Rust: a built artifact under `target/{release,debug}/<name>`, canonicalized
/// to an absolute path so it survives a later cwd change (the pre-commit
/// verify spawns from a temp dir). Falls back to a PATH probe for an installed
/// bin, then to the dir-derived name.
fn rust_cli_candidate(root: &Path, name: &str) -> Option<CliCandidate> {
    // Build the list of artifact filenames to probe. `cargo build` writes
    // `<bin_name>.exe` on Windows, bare `<bin_name>` on Unix. A crate may
    // rename its binary via `[[bin]] name = "..."` (e.g. `fd-find` → `fd`),
    // so probe `[[bin]].name` entries first, then the package-name fallback
    // for implicit single-bin crates where artifact == package name.
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let mut candidates: Vec<String> = cargo_bin_names(root);
    if !candidates.iter().any(|c| c == name) {
        candidates.push(name.to_string());
    }
    let probe_names: Vec<String> = candidates
        .into_iter()
        .map(|n| format!("{n}{suffix}"))
        .collect();
    for bin in &probe_names {
        for profile in &["release", "debug"] {
            let p = root.join("target").join(profile).join(bin);
            if p.exists() {
                // Canonicalize so the stored argv survives a later cwd change
                // (the pre-commit verify spawns from a temp dir). Falls back to
                // the joined path if canonicalize fails on some platforms.
                let abs = canonicalize_for_argv(&p);
                return Some(CliCandidate {
                    argv: vec![abs],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }
    }
    // PATH fallback: probe `[[bin]].name` candidates first (a renamed binary
    // like `fd` may be installed even though the crate is `fd-find`), then
    // the package name. which_on_path appends PATHEXT on Windows.
    for cand_name in cargo_bin_names(root) {
        if cand_name == name {
            continue;
        }
        if let Some(p) = which_on_path(&cand_name) {
            return Some(CliCandidate {
                argv: vec![p.to_string_lossy().to_string()],
                spawn_cwd: root.to_path_buf(),
            });
        }
    }
    which_on_path(name).map(|p| CliCandidate {
        argv: vec![p.to_string_lossy().to_string()],
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
    let abs_script = canonicalize_for_argv(&script_path);
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
    if let Some(script) = which_on_path(name) {
        return Some(CliCandidate {
            argv: vec![script.to_string_lossy().to_string()],
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
    let ruby = which_on_path("ruby")
        .or_else(|| which_on_path("bundle"))
        .map(|b| b.to_string_lossy().to_string())?;
    for dir in &["exe", "bin"] {
        let p = root.join(dir).join(name);
        if p.is_file() {
            let abs = canonicalize_for_argv(&p);
            return Some(CliCandidate {
                argv: vec![ruby.clone(), abs],
                spawn_cwd: root.to_path_buf(),
            });
        }
    }
    None
}

/// PHP: a `composer.json` `bin` field (string or object) points at a PHP
/// script. Resolve to an absolute path and run `php <abs script>` so the
/// project's CLI works uninstalled and survives a cwd change. Requires `php`
/// on PATH (honest `None` otherwise). Mirrors [`node_cli_candidate`].
fn php_cli_candidate(root: &Path, name: &str) -> Option<CliCandidate> {
    let php = which_on_path("php")?;
    let php_bin = php.to_string_lossy().to_string();
    let raw = fs::read_to_string(root.join("composer.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let bin = v.get("bin")?;
    // `bin` may be a string ("./bin/cli.php") or an object mapping name → script.
    // Pick the entry keyed by the tool name if present, otherwise the first script.
    let script = match bin {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map
            .get(name)
            .and_then(|v| v.as_str())
            .or_else(|| map.iter().next().and_then(|(_, v)| v.as_str()))?
            .to_string(),
        // composer.json `bin` may also be an array of paths; pick the first.
        serde_json::Value::Array(arr) => arr.first()?.as_str()?.to_string(),
        _ => return None,
    };
    if script.trim().is_empty() {
        return None;
    }
    // Resolve to an absolute path so `php <abs script> --help` works whether
    // or not the package is installed, and survives the temp-dir spawn cwd.
    let script_path = root.join(&script);
    let abs_script = canonicalize_for_argv(&script_path);
    Some(CliCandidate {
        argv: vec![php_bin, abs_script],
        spawn_cwd: root.to_path_buf(),
    })
}

/// JVM: probe for pre-built Gradle `installDist` script, Maven shaded jar, or
/// Gradle shadow jar. No build invocation — only reads existing artifacts
/// (design: "Pure filesystem reads"). Requires `java` on PATH for jar-based
/// invocations; the `installDist` script is self-contained. Honest `None`
/// when no artifact present — same posture as other languages.
fn jvm_cli_candidate(root: &Path, name: &str) -> Option<CliCandidate> {
    // Gradle `application` plugin: build/install/<name>/bin/<name> (script
    // form; `.bat` variant on Windows is handled by `canonicalize_for_argv`).
    // Present only after `gradle installDist`; we never run it here.
    let install_bin = root.join("build/install").join(name).join("bin").join(name);
    if install_bin.exists() {
        let abs = canonicalize_for_argv(&install_bin);
        return Some(CliCandidate {
            argv: vec![abs],
            spawn_cwd: root.to_path_buf(),
        });
    }

    let java = which_on_path("java")?;
    let java_bin = java.to_string_lossy().to_string();

    // Maven shade/spring-boot: target/<name>-*.jar (shaded, runnable).
    // Glob by prefix to avoid hardcoding the version.
    for dir in &["target", "build/libs"] {
        if let Ok(entries) = fs::read_dir(root.join(dir)) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) != Some("jar") {
                    continue;
                }
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if stem.starts_with(name) {
                        let abs = canonicalize_for_argv(&p);
                        return Some(CliCandidate {
                            argv: vec![java_bin.clone(), "-jar".to_string(), abs],
                            spawn_cwd: root.to_path_buf(),
                        });
                    }
                }
            }
        }
    }

    // Fallback to PATH probe for an installed JAR/script on PATH.
    which_on_path(name).map(|p| CliCandidate {
        argv: vec![p.to_string_lossy().to_string()],
        spawn_cwd: root.to_path_buf(),
    })
}

/// C# / .NET: `dotnet run --project <csproj>` from the project root (the
/// canonical uninstalled invocation — mirrors `go run .`). Requires `dotnet`
/// on PATH (honest `None` otherwise). `select_csproj` skips `WinExe` projects
/// (GUI — no stdout) for deterministic, cross-platform CLI invocation.
/// The trailing `--` separates `dotnet run`'s own flags from the app's argv
/// so an appended `--help` reaches the app, not dotnet (dotnet would print
/// its own help and never invoke the program).
fn csharp_cli_candidate(root: &Path, _name: &str) -> Option<CliCandidate> {
    which_on_path("dotnet")?;
    let csproj = super::select_csproj(root)?;
    let csproj_arg = csproj.to_string_lossy().to_string();
    Some(CliCandidate {
        argv: vec![
            "dotnet".to_string(),
            "run".to_string(),
            "--project".to_string(),
            csproj_arg,
            "--".to_string(),
        ],
        spawn_cwd: root.to_path_buf(),
    })
}

/// Canonicalize a path and strip the `\\?\` verbatim-UNC prefix that
/// `std::fs::canonicalize` emits on Windows. Node's module loader rejects
/// `\\?\` paths (ESM resolve / fs.readFile error out), and a `\\?\C:\foo`
/// argv survives as a literal string an embedded V8 refuses to load. The
/// kernel's CreateProcess accepts `\\?\` for native exes, so the removed
/// prefix is cosmetic for Rust binaries — but keeping it consistent across
/// the rust/node/ruby argvs avoids node-side load failures. Unix is a no-op.
fn canonicalize_for_argv(p: &Path) -> String {
    let path = std::fs::canonicalize(p)
        .ok()
        .and_then(|c| c.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| p.to_string_lossy().to_string());
    if cfg!(windows) && path.starts_with(r"\\?\") {
        path[4..].to_string()
    } else {
        path
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
        let node_stem = Path::new(&cand.argv[0])
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        assert!(
            node_stem.eq_ignore_ascii_case("node"),
            "got: {:?}",
            cand.argv
        );
        // the script path must be absolute and end with `bin/cli.js`. Use
        // Path component comparison (ends_with) so it holds cross-platform —
        // Windows separators are `\` so a string suffix check would miss.
        let script = Path::new(&cand.argv[1]);
        assert!(
            script.is_absolute() && script.ends_with("bin/cli.js"),
            "expected absolute script path, got {}",
            cand.argv[1]
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
        let stem = Path::new(&cand.argv[0])
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        assert!(
            stem.eq_ignore_ascii_case("python"),
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

    /// A crate may rename its binary via `[[bin]] name = "..."` (e.g. fd-find
    /// publishes the `fd` binary). `rust_cli_candidate` must probe the
    /// `[[bin]].name` artifact, not just the package-name artifact.
    #[test]
    fn rust_candidate_probes_bin_name_not_package_name() {
        let root = scratch_root(&[(
            "Cargo.toml",
            "[package]\nname = \"fd-find\"\n[[bin]]\nname = \"fd\"\n",
        )]);
        // Pre-built artifact named after [[bin]].name, NOT package name.
        let bin_dir = root.join("target").join("release");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin_name = if cfg!(windows) { "fd.exe" } else { "fd" };
        std::fs::write(bin_dir.join(bin_name), "#!/bin/sh\necho fd\n").unwrap();
        let cand = primary_cli_candidate(&root, Language::Rust, "fd-find");
        assert!(cand.is_some(), "expected [[bin]].name artifact probed");
        let cand = cand.unwrap();
        assert!(
            cand.argv[0].ends_with(bin_name),
            "expected argv to target [[bin]] artifact, got {}",
            cand.argv[0]
        );
        // Package-name artifact must NOT be probed first when [[bin]] differs.
        assert!(!cand.argv[0].ends_with("fd-find"));
        cleanup(&root);
    }

    #[test]
    fn csharp_candidate_uses_dotnet_run_with_dash_dash_separator() {
        if which_on_path("dotnet").is_none() {
            return;
        }
        let csproj = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
  </PropertyGroup>
</Project>
"#;
        let root = scratch_root(&[("sample.csproj", csproj)]);
        let cand = primary_cli_candidate(&root, Language::CSharp, "sample").unwrap();
        // The trailing "--" separates dotnet's flags from the app's argv
        // so an appended --help reaches the app, not dotnet.
        assert_eq!(cand.argv[0], "dotnet");
        assert_eq!(cand.argv[1], "run");
        assert_eq!(cand.argv[2], "--project");
        assert!(cand.argv[3].ends_with("sample.csproj"));
        assert_eq!(cand.argv[4], "--");
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }
}
