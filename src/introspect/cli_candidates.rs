//! CLI-candidate infrastructure + dispatch. Pure filesystem reads + PATH
//! probes that build the `argv` `detect_cli` spawns `--help` against; no
//! subprocess of its own. Each per-language probe returns `None` honestly
//! when the language runtime is missing rather than failing — an honest
//! `has_cli=false` is better than a spurious error.
//!
//! Split out of `introspect.rs` (0.8.5); the spawn/walk orchestrator now
//! lives in `super::cli_probe`. The per-language probes themselves live in
//! `super::manifest`'s `LanguageSpec` implementations (each language's
//! `cli_candidate` method); this module keeps the shared `CliCandidate` /
//! `DetectCli` types, `which_on_path`, the Windows-safe argv canonicalizer,
//! and the `primary_cli_candidate` dispatch.

use std::path::{Path, PathBuf};

use crate::types::{Language, SubcommandNode};

/// The captured CLI surface: `detect_cli`'s return. Named (not a bare 4-tuple)
/// so the call site reads `d.has_cli` / `d.command` rather than decoding
/// positional fields — and clippy's `type_complexity` stops firing on the
/// `Option<Vec<...>>` pile.
pub(crate) struct DetectCli {
    pub(crate) has_cli: bool,
    pub(crate) command: Option<Vec<String>>,
    pub(crate) help_output: Option<String>,
    pub(crate) subcommand_tree: Vec<SubcommandNode>,
}

impl DetectCli {
    pub(crate) fn none() -> Self {
        Self {
            has_cli: false,
            command: None,
            help_output: None,
            subcommand_tree: Vec::new(),
        }
    }
}

/// A resolved CLI invocation ready to spawn `--help`. The argv excludes the
/// trailing `--help` (which `detect_cli` appends). `spawn_cwd` is the working
/// directory the CLI needs to run in — the project root for relative-invocation
/// CLIs (`go run .`, a `package.json` bin script); for CLIs resolved to an
/// absolute path it's still the root so the spawn matches what `verify` does.
///
/// `pub` (not `pub(crate)`): the public [`LanguageSpec`] trait's
/// `cli_candidate` method returns this type, so the per-language spec modules
/// implement their probes against it.
#[derive(Debug, Clone)]
pub struct CliCandidate {
    /// Full argv excluding `--help`, e.g. `["node","/abs/bin/cli.js"]`,
    /// `["go","run","."]`, or `["/abs/target/debug/sample-rust"]`.
    pub argv: Vec<String>,
    /// Working directory for the spawn (the project root).
    pub spawn_cwd: PathBuf,
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
    // Per-language candidate resolution lives in the language's spec module
    // (`super::manifest::language_spec`) — this dispatcher is the single
    // entry point `cli_probe` uses.
    super::manifest::language_spec(language).cli_candidate(root, name)
}

/// Parse `[[bin]].name` entries from `Cargo.toml`. Returns bin names in
/// declaration order; empty when no `[[bin]]` tables (implicit single-bin
/// crate where the artifact matches the package name).
pub(crate) fn canonicalize_for_argv(p: &Path) -> String {
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
        // `which_on_path` returns `go.exe` on Windows, but PATHEXT casing
        // (`go.EXE`) is not guaranteed and a string suffix check is
        // case-sensitive — compare the file stem case-insensitively instead
        // (same pattern as the node candidate test above).
        let go_stem = Path::new(&cand.argv[0])
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        assert!(
            go_stem.eq_ignore_ascii_case("go"),
            "expected go executable, got {:?}",
            cand.argv
        );
        assert_eq!(&cand.argv[1..], &["run", "."]);
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
    /// publishes the `fd` binary). The Rust spec's `cli_candidate` must probe
    /// the `[[bin]].name` artifact, not just the package-name artifact.
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

    #[test]
    fn zig_candidate_finds_zig_out_bin() {
        let root = scratch_root(&[("build.zig", "const std = @import(\"std\");\n")]);
        let bin_dir = root.join("zig-out").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin_name = if cfg!(windows) {
            "my-zig.exe"
        } else {
            "my-zig"
        };
        std::fs::write(bin_dir.join(bin_name), "#!/bin/sh\necho zig\n").unwrap();
        let cand = primary_cli_candidate(&root, Language::Zig, "my-zig").unwrap();
        assert!(cand.argv[0].ends_with(bin_name));
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }

    #[test]
    fn swift_candidate_finds_build_debug_bin() {
        let root = scratch_root(&[("Package.swift", "// swift\n")]);
        let bin_dir = root.join(".build").join("debug");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin_name = if cfg!(windows) {
            "swift-cli.exe"
        } else {
            "swift-cli"
        };
        std::fs::write(bin_dir.join(bin_name), "#!/bin/sh\necho swift\n").unwrap();
        let cand = primary_cli_candidate(&root, Language::Swift, "swift-cli").unwrap();
        assert!(cand.argv[0].ends_with(bin_name));
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }

    #[test]
    fn c_cpp_candidate_finds_build_bin() {
        let root = scratch_root(&[("CMakeLists.txt", "project(MyCpp)\n")]);
        let bin_dir = root.join("build");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin_name = if cfg!(windows) {
            "my_cpp.exe"
        } else {
            "my_cpp"
        };
        std::fs::write(bin_dir.join(bin_name), "#!/bin/sh\necho cpp\n").unwrap();
        let cand = primary_cli_candidate(&root, Language::CCpp, "my_cpp").unwrap();
        assert!(cand.argv[0].ends_with(bin_name));
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }

    #[test]
    fn elixir_candidate_finds_release_bin() {
        let root = scratch_root(&[("mix.exs", "defmodule App ...")]);
        let bin_dir = root
            .join("_build")
            .join("dev")
            .join("rel")
            .join("my_app")
            .join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("my_app"), "#!/bin/sh\necho elixir\n").unwrap();
        let cand = primary_cli_candidate(&root, Language::Elixir, "my_app").unwrap();
        assert!(cand.argv[0].ends_with("my_app"));
        assert_eq!(cand.spawn_cwd, root);
        cleanup(&root);
    }
}
