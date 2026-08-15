//! Repo introspection. Produces a [`ProjectProfile`] from pure filesystem
//! reads, plus one guarded `--help` spawn when a CLI binary is detected.
//!
//! Design §6.3: "No side effects. Pure filesystem reads. Spawns `--help` only
//! when a CLI binary is detected ... guarded by a hard timeout and runs in a
//! working directory restricted to the project root."
//!
//! This module is the thin top-level orchestrator: it calls
//! `detect_language` then delegates each concern to a sibling submodule —
//! `cli_candidates` (resolve a candidate argv), `cli_probe` (spawn
//! `--help` + walk workspace members), `manifest` (pull scalar fields
//! from a language manifest), `repo` (git origin, LICENSE, README hint),
//! and `workspace` (workspace-only root + member-name heuristics).
//!
//! Detection order is deliberate: if both a `Cargo.toml` and a `package.json`
//! exist we pick the one most likely to *ship a CLI* (Rust, then node), which
//! matches the polyglot-monorepo reality.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::types::{DiagTrace, Language, ProjectProfile};
mod cli_candidates;
mod cli_probe;
mod manifest;
mod repo;
mod workspace;

// Re-export the symbols external callers reach by the flat path:
// `verify::discovery` uses `detect_language` + `project_manifest_version`,
// `verify` uses `which_on_path` for multi-skill CLI probes, and
// `csharp_cli_candidate` (now in `cli_candidates`) uses `select_csproj`.
// The re-exports keep those call sites unchanged after the split.
pub(crate) use cli_candidates::which_on_path;
// `project_manifest_name` is re-exported `pub` (not `pub(crate)`) because the
// bin target (`main.rs`) derives per-secondary-language import patterns from
// each secondary manifest — a `pub(crate)` re-export is invisible to the bin
// crate. The other two stay internal to the lib.
pub use manifest::project_manifest_name;
pub(crate) use manifest::{project_manifest_version, select_csproj};
pub(crate) use repo::{normalize_git_url, urls_equivalent};
pub(crate) use workspace::{
    first_cargo_member_name, first_npm_member_name, is_cargo_workspace_only, is_npm_workspace_only,
};

/// Introspect the project at `root`. `root` must be the OSS project root
/// (the directory containing the language manifest).
pub fn introspect(root: &Path) -> Result<ProjectProfile> {
    anyhow::ensure!(root.is_dir(), "{} is not a directory", root.display());

    let mut diag = DiagTrace::default();

    let language = detect_language(root, &mut diag);
    let secondary_languages: Vec<Language> = detect_all_languages(root)
        .into_iter()
        .filter(|l| *l != language)
        .collect();
    let mut manifest_name = manifest::project_manifest_name(root, language);
    // A workspace-only root (no [package]) has no name of its own; its CLI
    // lives in a member. Probe the first member with a name so `detect_cli`
    // (which needs a name to probe candidates) actually walks the workspace
    // rather than bailing at the name gate. The member name also becomes the
    // profile name — the tool the agent discovers — so downstream files key
    // off the right binary.
    if manifest_name.is_none() {
        if language == Language::Rust && is_cargo_workspace_only(root) {
            manifest_name = first_cargo_member_name(root, &mut diag);
        } else if language == Language::Node && is_npm_workspace_only(root) {
            manifest_name = first_npm_member_name(root, &mut diag);
        }
    }
    let repo_url = repo::detect_repo_url(root);
    let license = repo::detect_license(root).or_else(|| manifest::manifest_license(root, language));
    let version = manifest::project_manifest_version(root, language);
    // Manifest authors first; fall back to `git config user.name` so `--auto`
    // and `doctor` can fill plugin.json's author without a prompt.
    let authors =
        manifest::project_manifest_authors(root, language).or_else(|| repo::detect_author(root));
    let description_hint =
        repo::read_readme_hint(root).or_else(|| manifest::manifest_description(root, language));
    let d = cli_probe::detect_cli(root, language, manifest_name.clone(), &mut diag);
    let has_cli = d.has_cli;
    let cli_command = d.command;
    let cli_help_output = d.help_output;
    let cli_subcommand_tree = d.subcommand_tree;

    let name = manifest_name
        .or_else(|| repo::repo_url_name(&repo_url))
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
        secondary_languages,
        has_cli,
        cli_command,
        cli_help_output,
        cli_subcommand_tree,
        diag,
        repo_url,
        license,
        version,
        authors,
        description_hint,
    })
}

/// Detect EVERY language present at `root`, in the same priority order as
/// [`detect_language`] (Rust first, then node, ...). Used to surface polyglot
/// monorepos: the dominant language wins for the primary skill; the rest are
/// recorded as `secondary_languages` and get their own skill in `init --auto`.
/// `Makefile`-only is deliberately NOT a C/C++ signal here — a bare Makefile
/// is ubiquitous in polyglot repos and would false-flag almost every one.
pub(crate) fn detect_all_languages(root: &Path) -> Vec<Language> {
    // Declaration order = canonical order (mirrors `detect_language`'s
    // precedence for the root: Rust beats Node, etc.).
    const ORDER: [Language; 24] = [
        Language::Rust,
        Language::Node,
        Language::Python,
        Language::Go,
        Language::Php,
        Language::Jvm,
        Language::CSharp,
        Language::Ruby,
        Language::Zig,
        Language::Swift,
        Language::CCpp,
        Language::Elixir,
        Language::Deno,
        Language::Nix,
        Language::Dart,
        Language::Haskell,
        Language::Lua,
        Language::Julia,
        Language::Crystal,
        Language::Clojure,
        Language::Ocaml,
        Language::Erlang,
        Language::R,
        Language::Perl,
    ];
    // Walk the nested subdirectories once and reuse the list across every
    // language check (a 24× walk of a 50-dir monorepo would be wasteful).
    let nested = nested_dirs(root);
    let mut langs = Vec::new();
    for lang in ORDER {
        if language_present(root, lang) || nested.iter().any(|d| language_present(d, lang)) {
            langs.push(lang);
        }
    }
    langs
}

/// The directory holding `lang`'s manifest: the root when the manifest sits
/// there, otherwise the first nested match (bounded walk, depth ≤ 2, noise
/// dirs skipped). `pub` (not `pub(crate)`) because the bin target's
/// `auto_intents` uses it to scope a secondary skill's import pattern and
/// cursor globs to the subdirectory the manifest lives in.
pub fn language_manifest_dir(root: &Path, lang: Language) -> Option<PathBuf> {
    if language_present(root, lang) {
        return Some(root.to_path_buf());
    }
    nested_dirs(root)
        .into_iter()
        .find(|d| language_present(d, lang))
}

/// Bounded list of candidate subdirectories that could hold a secondary
/// language's manifest: depth-1 then depth-2 entries, sorted for
/// cross-platform determinism, skipping noise dirs and dot-dirs (VCS
/// metadata, vendored/generated trees, test fixtures).
fn nested_dirs(root: &Path) -> Vec<PathBuf> {
    fn depth1_dirs(dir: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !is_noise_dir(p))
            .collect();
        dirs.sort();
        dirs
    }

    let mut out = Vec::new();
    for d in depth1_dirs(root) {
        out.push(d.clone());
        out.extend(depth1_dirs(&d));
    }
    out
}

/// Skip VCS/metadata, vendored/generated trees, and test-fixture dirs when
/// hunting for secondary-language manifests.
fn is_noise_dir(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with('.')
        || matches!(
            name,
            "node_modules"
                | "target"
                | "dist"
                | "build"
                | "out"
                | "vendor"
                | "venv"
                | "__pycache__"
                | "coverage"
                | "Pods"
                | "bazel-bin"
                | "bazel-out"
                | "tests"
                | "test"
                | "docs"
        )
}

/// Per-directory signal check for one language, mirroring the conditions in
/// `detect_language`'s chain — keep the two in sync. Used by
/// `detect_all_languages` / `language_manifest_dir` to probe the root and
/// nested subdirectories alike. `CCpp` deliberately omits the bare
/// `Makefile` signal here (weak and shared across ecosystems), matching the
/// pre-nested-walk behavior; `detect_language` still honors Makefile for the
/// PRIMARY.
fn language_present(dir: &Path, lang: Language) -> bool {
    match lang {
        Language::Rust => dir.join("Cargo.toml").exists(),
        Language::Node => dir.join("package.json").exists(),
        Language::Python => {
            dir.join("pyproject.toml").exists()
                || dir.join("setup.py").exists()
                || dir.join("setup.cfg").exists()
        }
        Language::Go => dir.join("go.mod").exists(),
        Language::Php => dir.join("composer.json").exists(),
        Language::Jvm => {
            dir.join("pom.xml").exists()
                || dir.join("build.gradle").exists()
                || dir.join("build.gradle.kts").exists()
        }
        Language::CSharp => cli_probe::has_csproj(dir),
        Language::Ruby => dir.join("Gemfile").exists() || cli_probe::has_gemspec(dir),
        Language::Zig => dir.join("build.zig").exists() || dir.join("build.zig.zon").exists(),
        Language::Swift => dir.join("Package.swift").exists(),
        Language::CCpp => dir.join("CMakeLists.txt").exists() || dir.join("meson.build").exists(),
        Language::Elixir => dir.join("mix.exs").exists(),
        Language::Deno => dir.join("deno.json").exists() || dir.join("deno.jsonc").exists(),
        Language::Nix => {
            dir.join("flake.nix").exists()
                || dir.join("shell.nix").exists()
                || dir.join("default.nix").exists()
        }
        Language::Dart => dir.join("pubspec.yaml").exists(),
        Language::Haskell => {
            dir.join("stack.yaml").exists()
                || dir.join("cabal.project").exists()
                || cli_probe::has_cabal_file(dir)
        }
        Language::Lua => cli_probe::has_file_with_ext(dir, "rockspec"),
        Language::Julia => dir.join("Project.toml").exists(),
        Language::Crystal => dir.join("shard.yml").exists(),
        Language::Clojure => dir.join("deps.edn").exists() || dir.join("project.clj").exists(),
        Language::Ocaml => {
            dir.join("dune-project").exists() || cli_probe::has_file_with_ext(dir, "opam")
        }
        Language::Erlang => {
            dir.join("rebar.config").exists() || cli_probe::has_file_ending_with(dir, ".app.src")
        }
        Language::R => cli_probe::root_file_contains(dir, "DESCRIPTION", "Package:"),
        Language::Perl => {
            dir.join("cpanfile").exists()
                || dir.join("Makefile.PL").exists()
                || dir.join("META.json").exists()
        }
        // Script-first ecosystems are PRIMARY-only: `detect_language` covers
        // them via `shell_project`/`powershell_project`, but weak signals
        // like a stray `*.sh` must not mint a secondary skill.
        Language::Shell | Language::Powershell | Language::Unknown => false,
    }
}

/// True when the repo's primary surface is shell scripting: a shebang'd
/// `*.sh` at the root (excluding the ubiquitous install/setup/configure
/// helpers that tag along with non-shell projects) or under `bin/`/
/// `scripts/`/`src/`. Weak but sufficient — this branch only fires when NO
/// other language manifest exists.
fn shell_project(root: &Path) -> bool {
    fn has_shell_shebang(p: &Path) -> bool {
        fs::read_to_string(p)
            .ok()
            .and_then(|c| c.lines().next().map(str::to_string))
            .is_some_and(|l| {
                l.starts_with("#!")
                    && (l.contains("bash") || l.contains("/sh") || l.contains("zsh"))
            })
    }
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("sh") {
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if !matches!(stem, "install" | "setup" | "configure") && has_shell_shebang(&p) {
                    return true;
                }
            }
        }
    }
    for sub in ["bin", "scripts", "src"] {
        if let Ok(rd) = fs::read_dir(root.join(sub)) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("sh") && has_shell_shebang(&p) {
                    return true;
                }
            }
        }
    }
    false
}

/// True when the repo ships PowerShell scripts/modules (`.ps1`/`.psm1`/
/// `.psd1`) at the root or under `bin`/`src`/`scripts`/`tools`. Same weak
/// primary-only status as [`shell_project`].
fn powershell_project(root: &Path) -> bool {
    for sub in ["", "bin", "src", "scripts", "tools"] {
        if let Ok(rd) = fs::read_dir(root.join(sub)) {
            for e in rd.flatten() {
                if matches!(
                    e.path().extension().and_then(|x| x.to_str()),
                    Some("ps1") | Some("psm1") | Some("psd1")
                ) {
                    return true;
                }
            }
        }
    }
    false
}

/// Detect the dominant language by checking for known manifests. Each falsy
/// branch (manifest absent) pushes a `DiagNote` so `skillpack doctor` can
/// explain why an `Unknown` language came out, and the workspace-only edge
/// case (a `Cargo.toml` with `[workspace]` members but no `[package]`)
/// surfaces as a note pointing at member walking.
pub(crate) fn detect_language(root: &Path, diag: &mut DiagTrace) -> Language {
    if root.join("Cargo.toml").exists() {
        // A workspace-only `Cargo.toml` (no `[package]`) has no binary of its
        // own; its members may. Push a note so doctor explains the walk below.
        let is_workspace_only = is_cargo_workspace_only(root);
        if is_workspace_only {
            diag.push(
                "detect_language.rust",
                "Cargo.toml found but it is workspace-only (no [package]); ".to_string()
                    + "CLI detection will probe workspace members next",
            );
        }
        Language::Rust
    } else if root.join("package.json").exists() {
        if is_npm_workspace_only(root) {
            diag.push(
                "detect_language.node",
                "package.json found but it declares `workspaces` with no root bin; ".to_string()
                    + "CLI detection will probe workspace packages next",
            );
        }
        Language::Node
    } else if root.join("pyproject.toml").exists()
        || root.join("setup.py").exists()
        || root.join("setup.cfg").exists()
    {
        Language::Python
    } else if root.join("go.mod").exists() {
        Language::Go
    } else if root.join("composer.json").exists() {
        Language::Php
    } else if root.join("pom.xml").exists()
        || root.join("build.gradle").exists()
        || root.join("build.gradle.kts").exists()
    {
        Language::Jvm
    } else if cli_probe::has_csproj(root) {
        Language::CSharp
    } else if root.join("Gemfile").exists() || cli_probe::has_gemspec(root) {
        Language::Ruby
    } else if root.join("build.zig").exists() || root.join("build.zig.zon").exists() {
        Language::Zig
    } else if root.join("Package.swift").exists() {
        Language::Swift
    } else if root.join("CMakeLists.txt").exists()
        || root.join("meson.build").exists()
        || root.join("Makefile").exists()
    {
        if !root.join("CMakeLists.txt").exists() && !root.join("meson.build").exists() {
            diag.push(
                "detect_language.c_cpp",
                "Makefile found with no CMakeLists.txt/meson.build; assuming C/C++ \
                 (a weak signal — a Makefile-only project may be another language). \
                 Run `skillpack doctor` to confirm.",
            );
        }
        Language::CCpp
    } else if root.join("mix.exs").exists() {
        Language::Elixir
    } else if root.join("deno.json").exists() || root.join("deno.jsonc").exists() {
        Language::Deno
    } else if root.join("flake.nix").exists()
        || root.join("shell.nix").exists()
        || root.join("default.nix").exists()
    {
        Language::Nix
    } else if root.join("pubspec.yaml").exists() {
        Language::Dart
    } else if root.join("stack.yaml").exists()
        || root.join("cabal.project").exists()
        || cli_probe::has_cabal_file(root)
    {
        Language::Haskell
    } else if cli_probe::has_file_with_ext(root, "rockspec") {
        Language::Lua
    } else if root.join("Project.toml").exists() {
        Language::Julia
    } else if root.join("shard.yml").exists() {
        Language::Crystal
    } else if root.join("deps.edn").exists() || root.join("project.clj").exists() {
        Language::Clojure
    } else if root.join("dune-project").exists() || cli_probe::has_file_with_ext(root, "opam") {
        Language::Ocaml
    } else if root.join("rebar.config").exists()
        || cli_probe::has_file_ending_with(root, ".app.src")
    {
        Language::Erlang
    } else if cli_probe::root_file_contains(root, "DESCRIPTION", "Package:") {
        Language::R
    } else if root.join("cpanfile").exists()
        || root.join("Makefile.PL").exists()
        || root.join("META.json").exists()
    {
        Language::Perl
    } else if shell_project(root) {
        Language::Shell
    } else if powershell_project(root) {
        Language::Powershell
    } else {
        diag.push(
            "detect_language",
            "no known manifest found (none of: Cargo.toml, package.json, ".to_string()
                + "pyproject.toml, setup.py, setup.cfg, go.mod, composer.json, "
                + "pom.xml, build.gradle, build.gradle.kts, Gemfile, *.gemspec, "
                + "*.csproj, build.zig, Package.swift, CMakeLists.txt, meson.build, Makefile, mix.exs, deno.json, "
                + "flake.nix, shell.nix, pubspec.yaml, stack.yaml, *.cabal, *.rockspec, Project.toml, "
                + "shard.yml, deps.edn, project.clj, dune-project, *.opam, rebar.config, *.app.src, "
                + "DESCRIPTION, cpanfile, Makefile.PL, META.json, shell scripts (*.sh with a shebang), "
                + "powershell (*.ps1/psm1/psd1)); "
                + "language detected as Unknown",
        );
        Language::Unknown
    }
}

#[cfg(test)]
impl ProjectProfile {
    /// Test helper: a profile with everything falsy, for assembling fixtures.
    pub fn test_default() -> Self {
        Self {
            name: "test-tool".to_string(),
            language: Language::Unknown,
            secondary_languages: Vec::new(),
            has_cli: false,
            cli_command: None,
            cli_help_output: None,
            cli_subcommand_tree: Vec::new(),
            diag: DiagTrace::default(),
            repo_url: None,
            license: None,
            version: None,
            authors: None,
            description_hint: None,
        }
    }
}

#[cfg(test)]
mod parse_tests {
    //! Orchestrator tests that stayed in `introspect.rs`: directory-tail
    //! fallback (Bug #3: canonicalize a bare `--root .`) and the
    //! `which_on_path` real-exercise check. Workspace-walk + readme tests
    //! live in `cli_probe::tests` / `repo::tests` now.

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
            let p = root.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, contents).unwrap();
        }
        root
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
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

    #[test]
    fn detect_all_languages_finds_polyglot_monorepo() {
        let root = scratch(&[
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("package.json", "{}"),
        ]);
        let langs = detect_all_languages(&root);
        assert_eq!(langs, vec![Language::Rust, Language::Node]);
        cleanup(&root);
    }

    #[test]
    fn detect_all_languages_empty_for_no_manifests() {
        let root = scratch(&[]);
        assert!(detect_all_languages(&root).is_empty());
        cleanup(&root);
    }

    #[test]
    fn detect_language_recognizes_shell_and_powershell() {
        let mut diag = DiagTrace::default();

        let shell = scratch(&[("bin/tool.sh", "#!/usr/bin/env bash\nset -euo pipefail\n")]);
        assert_eq!(detect_language(&shell, &mut diag), Language::Shell);
        cleanup(&shell);

        // Root-level shebang'd script, excluding generic helper names.
        let shell_root = scratch(&[("myscript.sh", "#!/bin/bash\necho hi\n")]);
        assert_eq!(detect_language(&shell_root, &mut diag), Language::Shell);
        cleanup(&shell_root);

        // `install.sh` alone is not a shell project (ships with every
        // ecosystem); falls through to Unknown.
        let only_install = scratch(&[("install.sh", "#!/bin/sh\nset -e\n")]);
        assert_eq!(detect_language(&only_install, &mut diag), Language::Unknown);
        cleanup(&only_install);

        let ps = scratch(&[("bin/tool.ps1", "Write-Host \"hi\"\n")]);
        assert_eq!(detect_language(&ps, &mut diag), Language::Powershell);
        cleanup(&ps);

        let psm = scratch(&[("MyModule.psm1", "function Invoke-Hi {}\n")]);
        assert_eq!(detect_language(&psm, &mut diag), Language::Powershell);
        cleanup(&psm);
    }

    #[test]
    fn detect_all_languages_finds_nested_monorepo_languages() {
        let root = scratch(&[
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("web/package.json", "{}"),
            ("packages/app/package.json", "{}"),
            // Noise: fixtures and vendored trees must not mint skills.
            ("tests/fixtures/package.json", "{}"),
            ("node_modules/pkg/package.json", "{}"),
            ("target/debug/package.json", "{}"),
        ]);
        let langs = detect_all_languages(&root);
        assert_eq!(langs, vec![Language::Rust, Language::Node]);
        let dir = language_manifest_dir(&root, Language::Node).unwrap();
        assert_ne!(
            dir, root,
            "nested Node manifest must resolve away from the root"
        );
        assert!(
            dir.join("package.json").exists(),
            "resolved dir must actually hold the manifest: {}",
            dir.display()
        );
        cleanup(&root);
    }

    #[test]
    fn detect_all_languages_excludes_shell_secondaries() {
        // A Rust repo with a helper script is Rust-only; shell never becomes
        // a secondary skill.
        let root = scratch(&[
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("scripts/ci.sh", "#!/usr/bin/env bash\nset -e\n"),
        ]);
        assert_eq!(detect_all_languages(&root), vec![Language::Rust]);
        cleanup(&root);
    }

    #[test]
    fn detect_language_recognizes_nix_dart_haskell() {
        let mut diag = DiagTrace::default();

        let nix = scratch(&[("flake.nix", "{}")]);
        assert_eq!(detect_language(&nix, &mut diag), Language::Nix);
        cleanup(&nix);

        let dart = scratch(&[("pubspec.yaml", "name: x")]);
        assert_eq!(detect_language(&dart, &mut diag), Language::Dart);
        cleanup(&dart);

        let haskell = scratch(&[("stack.yaml", "resolver: lts")]);
        assert_eq!(detect_language(&haskell, &mut diag), Language::Haskell);
        cleanup(&haskell);
    }

    #[test]
    fn detect_language_recognizes_eight_new_languages() {
        let mut diag = DiagTrace::default();
        let cases: &[(&[(&str, &str)], Language)] = &[
            (
                &[("mylua-1.0-1.rockspec", "package = \"x\"")],
                Language::Lua,
            ),
            (&[("Project.toml", "name = \"x\"")], Language::Julia),
            (&[("shard.yml", "name: x")], Language::Crystal),
            (&[("deps.edn", "{:paths [\"src\"]}")], Language::Clojure),
            (&[("dune-project", "(lang dune 3.0)")], Language::Ocaml),
            (&[("rebar.config", "{deps, []}.")], Language::Erlang),
            (
                &[("DESCRIPTION", "Package: mypkg\nVersion: 1.0\n")],
                Language::R,
            ),
            (&[("cpanfile", "requires 'Foo';")], Language::Perl),
        ];
        for (files, expected) in cases {
            let root = scratch(files);
            assert_eq!(
                detect_language(&root, &mut diag),
                *expected,
                "detect_language for {:?} failed",
                files
            );
            cleanup(&root);
        }
    }

    #[test]
    fn detect_all_languages_includes_nix_dart_haskell() {
        let root = scratch(&[
            ("flake.nix", "{}"),
            ("pubspec.yaml", "name: x"),
            ("stack.yaml", "resolver: lts"),
        ]);
        let langs = detect_all_languages(&root);
        assert!(langs.contains(&Language::Nix), "got: {langs:?}");
        assert!(langs.contains(&Language::Dart), "got: {langs:?}");
        assert!(langs.contains(&Language::Haskell), "got: {langs:?}");
        cleanup(&root);
    }

    #[test]
    fn which_on_path_returns_existing_file() {
        // Real-exercise check: whatever PATH lookup finds must be an existing
        // file. Probes a binary present on every CI OS we run. PATHEXT enum
        // is exercised end-to-end by the windows-latest CI matrix entry
        // (real `node.exe` / `cmd.exe` lookup), not a synthetic env mutation
        // that would race other parallel tests mutating process-global PATH.
        let probe = if cfg!(windows) {
            which_on_path("cmd")
        } else {
            which_on_path("ls")
        };
        if let Some(p) = probe {
            assert!(p.is_file(), "which_on_path returned non-file: {p:?}");
        }
    }
}
