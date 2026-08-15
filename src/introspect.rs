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

use std::path::Path;

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
pub(crate) use manifest::{project_manifest_version, select_csproj};
pub(crate) use repo::{normalize_git_url, urls_equivalent};
#[cfg(test)]
use std::path::PathBuf;
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
    let mut langs = Vec::new();
    let signals: &[(Language, bool)] = &[
        (Language::Rust, root.join("Cargo.toml").exists()),
        (Language::Node, root.join("package.json").exists()),
        (
            Language::Python,
            root.join("pyproject.toml").exists()
                || root.join("setup.py").exists()
                || root.join("setup.cfg").exists(),
        ),
        (Language::Go, root.join("go.mod").exists()),
        (Language::Php, root.join("composer.json").exists()),
        (
            Language::Jvm,
            root.join("pom.xml").exists()
                || root.join("build.gradle").exists()
                || root.join("build.gradle.kts").exists(),
        ),
        (Language::CSharp, cli_probe::has_csproj(root)),
        (
            Language::Ruby,
            root.join("Gemfile").exists() || cli_probe::has_gemspec(root),
        ),
        (
            Language::Zig,
            root.join("build.zig").exists() || root.join("build.zig.zon").exists(),
        ),
        (Language::Swift, root.join("Package.swift").exists()),
        (
            Language::CCpp,
            root.join("CMakeLists.txt").exists() || root.join("meson.build").exists(),
        ),
        (Language::Elixir, root.join("mix.exs").exists()),
        (
            Language::Deno,
            root.join("deno.json").exists() || root.join("deno.jsonc").exists(),
        ),
        (
            Language::Nix,
            root.join("flake.nix").exists()
                || root.join("shell.nix").exists()
                || root.join("default.nix").exists(),
        ),
        (Language::Dart, root.join("pubspec.yaml").exists()),
        (
            Language::Haskell,
            root.join("stack.yaml").exists()
                || root.join("cabal.project").exists()
                || cli_probe::has_cabal_file(root),
        ),
        (
            Language::Lua,
            cli_probe::has_file_with_ext(root, "rockspec"),
        ),
        (Language::Julia, root.join("Project.toml").exists()),
        (Language::Crystal, root.join("shard.yml").exists()),
        (
            Language::Clojure,
            root.join("deps.edn").exists() || root.join("project.clj").exists(),
        ),
        (
            Language::Ocaml,
            root.join("dune-project").exists() || cli_probe::has_file_with_ext(root, "opam"),
        ),
        (
            Language::Erlang,
            root.join("rebar.config").exists() || cli_probe::has_file_ending_with(root, ".app.src"),
        ),
        (
            Language::R,
            cli_probe::root_file_contains(root, "DESCRIPTION", "Package:"),
        ),
        (
            Language::Perl,
            root.join("cpanfile").exists()
                || root.join("Makefile.PL").exists()
                || root.join("META.json").exists(),
        ),
    ];
    for (lang, present) in signals {
        if *present {
            langs.push(*lang);
        }
    }
    langs
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
    } else {
        diag.push(
            "detect_language",
            "no known manifest found (none of: Cargo.toml, package.json, ".to_string()
                + "pyproject.toml, setup.py, setup.cfg, go.mod, composer.json, "
                + "pom.xml, build.gradle, build.gradle.kts, Gemfile, *.gemspec, "
                + "*.csproj, build.zig, Package.swift, CMakeLists.txt, meson.build, Makefile, mix.exs, deno.json, "
                + "flake.nix, shell.nix, pubspec.yaml, stack.yaml, *.cabal, *.rockspec, Project.toml, "
                + "shard.yml, deps.edn, project.clj, dune-project, *.opam, rebar.config, *.app.src, "
                + "DESCRIPTION, cpanfile, Makefile.PL, META.json); "
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
            std::fs::write(root.join(rel), contents).unwrap();
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
