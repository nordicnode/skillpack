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

use crate::types::{DiagTrace, Language, ProjectProfile};

/// We only read the first slice of the README to bound cost.
const README_HEAD_LINES: usize = 500;

/// Introspect the project at `root`. `root` must be the OSS project root
/// (the directory containing the language manifest).
pub fn introspect(root: &Path) -> Result<ProjectProfile> {
    anyhow::ensure!(root.is_dir(), "{} is not a directory", root.display());

    let mut diag = DiagTrace::default();

    let language = detect_language(root, &mut diag);
    let mut manifest_name = project_manifest_name(root, language);
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
    let repo_url = detect_repo_url(root);
    let license = detect_license(root).or_else(|| manifest_license(root, language));
    let version = project_manifest_version(root, language);
    let authors = project_manifest_authors(root, language).map(strip_author_email);
    let description_hint = read_readme_hint(root);
    let d = detect_cli(root, language, manifest_name.clone(), &mut diag);
    let has_cli = d.has_cli;
    let cli_command = d.command;
    let cli_help_output = d.help_output;
    let cli_subcommand_help = d.subcommand_help;

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
        cli_subcommand_help,
        diag,
        repo_url,
        license,
        version,
        authors,
        description_hint,
    })
}

/// Detect the dominant language by checking for known manifests. Each falsy
/// branch (manifest absent) pushes a `DiagNote` so `skillpack doctor` can
/// explain why an `Unknown` language came out, and the workspace-only edge
/// case (a `Cargo.toml` with `[workspace]` members but no `[package]`)
/// surfaces as a note pointing at member walking.
fn detect_language(root: &Path, diag: &mut DiagTrace) -> Language {
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
    } else if root.join("Gemfile").exists() || has_gemspec(root) {
        Language::Ruby
    } else {
        diag.push(
            "detect_language",
            "no known manifest found (none of: Cargo.toml, package.json, ".to_string()
                + "pyproject.toml, setup.py, setup.cfg, go.mod, Gemfile, *.gemspec); "
                + "language detected as Unknown",
        );
        Language::Unknown
    }
}

/// True iff `Cargo.toml` at `root` has a `[workspace]` table but no
/// `[package]` table. A pure workspace root ships no binary of its own;
/// its members may. Used by the diag-trace path, not detection itself.
fn is_cargo_workspace_only(root: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(root.join("Cargo.toml")) else {
        return false;
    };
    let Ok(v) = toml::from_str::<toml::Value>(&raw) else {
        return false;
    };
    v.get("workspace").is_some() && v.get("package").is_none()
}

/// True iff `package.json` at `root` has a `workspaces` field but no `bin`.
fn is_npm_workspace_only(root: &Path) -> bool {
    let Some(raw) = fs::read_to_string(root.join("package.json")).ok() else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    v.get("workspaces").is_some() && v.get("bin").is_none()
}
/// True iff `pyproject.toml` at `root` has a `[tool.<name>]` table.
/// Detects uv (`[tool.uv]`) and poetry (`[tool.poetry]`) managed monorepos
/// so doctor can explain the "not yet walked" gap.
fn pyproject_has_tool(root: &Path, name: &str) -> bool {
    let Some(raw) = fs::read_to_string(root.join("pyproject.toml")).ok() else {
        return false;
    };
    let Ok(v) = toml::from_str::<toml::Value>(&raw) else {
        return false;
    };
    v.get("tool").and_then(|t| t.get(name)).is_some()
}
/// First `[package].name` from a Cargo workspace member dir. Mirrors the
/// parse in [`walk_cargo_workspace`] but stops at name resolution (no
/// candidate/spawn probe) — used by [`introspect`] so `detect_cli` gets a
/// name to probe. Returns `None` if no member has a `[package].name`.
fn first_cargo_member_name(root: &Path, diag: &mut DiagTrace) -> Option<String> {
    let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let v = toml::from_str::<toml::Value>(&raw).ok()?;
    let members = v.get("workspace")?.get("members")?.as_array()?;
    for m in members {
        let Some(rel) = m.as_str() else { continue };
        let member_root = root.join(rel);
        let name = fs::read_to_string(member_root.join("Cargo.toml"))
            .ok()
            .and_then(|r| toml::from_str::<toml::Value>(&r).ok())
            .and_then(|mv| {
                mv.get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .map(String::from)
            });
        if let Some(n) = name {
            diag.push(
                "detect_language.rust.workspace",
                format!("workspace member `{rel}` supplied tool name `{n}`"),
            );
            return Some(n);
        }
    }
    diag.push(
        "detect_language.rust.workspace",
        "no workspace member has a [package].name — name fell back to dir tail".to_string(),
    );
    None
}

/// First `name` from an npm workspace member `package.json`. Mirrors
/// [`walk_npm_workspace`] but stops at name resolution. Returns `None` if no
/// member has a `name` field.
fn first_npm_member_name(root: &Path, diag: &mut DiagTrace) -> Option<String> {
    let raw = fs::read_to_string(root.join("package.json")).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let ws = v.get("workspaces")?;
    let paths: Vec<String> = match ws {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|e| e.as_str().map(String::from))
            .collect(),
        _ => return None,
    };
    for rel in paths {
        let pkg = root.join(&rel).join("package.json");
        let name = fs::read_to_string(&pkg)
            .ok()
            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
            .and_then(|mv| mv.get("name").and_then(|n| n.as_str()).map(String::from));
        if let Some(n) = name {
            diag.push(
                "detect_language.node.workspace",
                format!("workspace member `{rel}` supplied tool name `{n}`"),
            );
            return Some(n);
        }
    }
    diag.push(
        "detect_language.node.workspace",
        "no workspace member has a package.json `name` — name fell back to dir tail".to_string(),
    );
    None
}

/// Walk a Cargo workspace's members looking for a crate with a CLI binary.
/// Parses `Cargo.toml` `[workspace].members` (literal paths only — globs
/// not expanded, keeping V1 simple), then for each `members/<m>` probes
/// `primary_cli_candidate` against the member's `[package].name`. Pushes a
/// diag note per member tried so doctor explains the walk; returns `Some`
/// on the first member that yields a runnable CLI, `None` if none do.
fn walk_cargo_workspace(root: &Path, _name: &str, diag: &mut DiagTrace) -> Option<DetectCli> {
    let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let v = toml::from_str::<toml::Value>(&raw).ok()?;
    let members = v.get("workspace")?.get("members")?.as_array()?;
    diag.push(
        "detect_cli.rust.workspace",
        format!(
            "Cargo workspace root — {} member(s) to probe",
            members.len()
        ),
    );
    for m in members {
        let Some(member_rel) = m.as_str() else {
            continue;
        };
        let member_root = root.join(member_rel);
        if !member_root.join("Cargo.toml").is_file() {
            diag.push(
                "detect_cli.rust.workspace",
                format!("member `{member_rel}` has no Cargo.toml — skipped"),
            );
            continue;
        }
        // Prefer the member's own [package].name; fall back to the dir tail.
        let manifest_name = fs::read_to_string(member_root.join("Cargo.toml"))
            .ok()
            .and_then(|r| toml::from_str::<toml::Value>(&r).ok())
            .and_then(|v| {
                v.get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .map(String::from)
            });
        let Some(member_name) = manifest_name.or_else(|| {
            member_root
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        }) else {
            diag.push(
                "detect_cli.rust.workspace",
                format!("member `{member_rel}` has no name in manifest, skipping"),
            );
            continue;
        };
        match primary_cli_candidate(&member_root, Language::Rust, &member_name) {
            Some(candidate) => {
                diag.push(
                    "detect_cli.rust.workspace",
                    format!(
                        "member `{member_rel}` yielded candidate `{}`",
                        candidate.argv.join(" ")
                    ),
                );
                return Some(spawn_candidate(&candidate, diag));
            }
            None => diag.push(
                "detect_cli.rust.workspace",
                format!("member `{member_rel}` (`{member_name}`): no built/installed artifact"),
            ),
        }
    }
    diag.push(
        "detect_cli.rust.workspace",
        "no workspace member yielded a runnable CLI — has_cli=false \
         (run `skillpack init` inside the member crate that ships the binary)"
            .to_string(),
    );
    None
}

/// Walk an npm workspace's members (literal `workspaces` paths, no globs)
/// looking for a package with a `bin`. Parses `package.json` `workspaces`
/// (string or array of strings). Returns `Some` on the first member that
/// yields a runnable CLI; `None` otherwise. Pushes a diag note per member.
fn walk_npm_workspace(root: &Path, _name: &str, diag: &mut DiagTrace) -> Option<DetectCli> {
    let raw = fs::read_to_string(root.join("package.json")).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let ws = v.get("workspaces")?;
    let paths: Vec<String> = match ws {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|e| e.as_str().map(String::from))
            .collect(),
        _ => return None,
    };
    diag.push(
        "detect_cli.node.workspace",
        format!("npm workspace root — {} member(s) to probe", paths.len()),
    );
    for member_rel in paths {
        let member_root = root.join(&member_rel);
        let pkg_json = member_root.join("package.json");
        if !pkg_json.is_file() {
            diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` has no package.json — skipped"),
            );
            continue;
        }
        let Ok(mraw) = fs::read_to_string(&pkg_json) else {
            continue;
        };
        let Ok(mv) = serde_json::from_str::<serde_json::Value>(&mraw) else {
            continue;
        };
        let Some(member_name) = mv
            .get("name")
            .and_then(|n| n.as_str())
            .map(String::from)
            .or_else(|| {
                member_root
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
        else {
            diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` has no name in manifest, skipping"),
            );
            continue;
        };
        if mv.get("bin").is_none() {
            diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` (`{member_name}`): no `bin` field — skipped"),
            );
            continue;
        }
        match primary_cli_candidate(&member_root, Language::Node, &member_name) {
            Some(candidate) => {
                diag.push(
                    "detect_cli.node.workspace",
                    format!(
                        "member `{member_rel}` yielded candidate `{}`",
                        candidate.argv.join(" ")
                    ),
                );
                return Some(spawn_candidate(&candidate, diag));
            }
            None => diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` (`{member_name}`): candidate None (node missing?)"),
            ),
        }
    }
    diag.push(
        "detect_cli.node.workspace",
        "no workspace member yielded a runnable CLI — has_cli=false \
         (run `skillpack init` inside the member package that ships the bin)"
            .to_string(),
    );
    None
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
            let last = module_line
                .trim()
                .strip_prefix("module ")
                // Take only the first whitespace-delimited token so a trailing
                // `// ...` line comment cannot bleed into the module path
                // (e.g. `module github.com/foo/bar // bar tool` → "bar").
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string())?
                .rsplit('/')
                .next()?
                .to_string();
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

/// Pull the project version out of the language manifest, best-effort.
/// Mirrors [`project_manifest_name`] per language. Returns `None` for Go
/// (`go.mod` has no version field — versioning is via Git tags or a
/// separately-versioned file) and for manifests lacking a version key.
fn project_manifest_version(root: &Path, language: Language) -> Option<String> {
    match language {
        Language::Rust => {
            let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("package")
                .and_then(|p| p.get("version"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        }
        Language::Node => {
            let raw = fs::read_to_string(root.join("package.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            v.get("version")?
                .as_str()
                .map(std::string::ToString::to_string)
        }
        Language::Python => {
            if let Ok(raw) = fs::read_to_string(root.join("pyproject.toml")) {
                if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
                    if let Some(ver) = v
                        .get("project")
                        .and_then(|p| p.get("version"))
                        .and_then(|n| n.as_str())
                    {
                        return Some(ver.to_string());
                    }
                }
            }
            None
        }
        Language::Ruby => {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("gemspec") {
                        if let Ok(raw) = fs::read_to_string(&p) {
                            if let Some(line) = raw
                                .lines()
                                .find(|l| l.contains("spec.version") || l.contains(".version ="))
                            {
                                if let Some(ver) = extract_ruby_string_value(line) {
                                    return Some(ver.to_string());
                                }
                            }
                        }
                    }
                }
            }
            None
        }
        Language::Go | Language::Unknown => None,
    }
}

/// Pull the author(s) out of the language manifest, best-effort.
/// Mirrors [`project_manifest_version`] per language. Returns the first
/// author as a display string. `None` when the manifest has no author field
/// or the language has no author-bearing manifest (e.g. Go `go.mod`).
fn project_manifest_authors(root: &Path, language: Language) -> Option<String> {
    match language {
        Language::Rust => {
            let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("package")
                .and_then(|p| p.get("authors"))
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        }
        Language::Node => {
            let raw = fs::read_to_string(root.join("package.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            // package.json "author" is a string or { "name": "..." } object.
            if let Some(a) = v.get("author") {
                if let Some(s) = a.as_str() {
                    return Some(s.to_string());
                }
                if let Some(name) = a.get("name").and_then(|n| n.as_str()) {
                    return Some(name.to_string());
                }
            }
            None
        }
        Language::Python => {
            if let Ok(raw) = fs::read_to_string(root.join("pyproject.toml")) {
                if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
                    // PEP 621: [project.authors] = [{ name = "..." }]
                    if let Some(arr) = v
                        .get("project")
                        .and_then(|p| p.get("authors"))
                        .and_then(|a| a.as_array())
                    {
                        if let Some(first) = arr.first() {
                            if let Some(name) = first.get("name").and_then(|n| n.as_str()) {
                                return Some(name.to_string());
                            }
                        }
                    }
                }
            }
            None
        }
        Language::Ruby => {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("gemspec") {
                        if let Ok(raw) = fs::read_to_string(&p) {
                            if let Some(line) = raw
                                .lines()
                                .find(|l| l.contains("spec.author") || l.contains(".author ="))
                            {
                                if let Some(author) = extract_ruby_string_value(line) {
                                    return Some(author.to_string());
                                }
                            }
                        }
                    }
                }
            }
            None
        }
        Language::Go | Language::Unknown => None,
    }
}

/// Strip a trailing `<email>` from an author string. Cargo.toml's
/// `[package].authors` format is `"Name <email@example.com>"`; the
/// `plugin.json` `author.name` field wants a display name only, so we drop
/// the angle-bracketed email suffix. npm/Python/gemspec authors can also
/// carry the same convention.
fn strip_author_email(author: String) -> String {
    if let Some(idx) = author.rfind(" <") {
        author[..idx].trim().to_string()
    } else {
        author.trim().to_string()
    }
}

/// Detect whether the project ships an invokable CLI, and if so capture its
/// `--help` output under a hard timeout. Returns
/// `(has_cli, command, output, subcommand_help)`.
///
/// `command` is the full multi-token `--help` argv the verifier re-spawns (e.g.
/// `["node","/abs/bin/cli.js","--help"]`, `["go","run",".","--help"]`). The
/// bare human-facing invocation that SKILL.md publishes is derived separately
/// from the profile name + interview — this is the internal, machine-specific
/// spawn argv (design §5.1, §6.3).
///
/// `subcommand_help` holds `<cli> <sub> --help` per subcommand (clap-style),
/// in declaration order, so the generated SKILL.md can document the real
/// command surface and `verify` can drift-check it. Empty for non-subcommand
/// CLIs — a flat `--help` yields no `Commands:` section.
///
/// Every falsy branch (no name, no root candidate, spawn failure) pushes a
/// `DiagNote` so `skillpack doctor` explains why `has_cli=false` rather than
/// silently reporting it. Workspace-only roots (Cargo `[workspace]` only,
/// npm `workspaces` no `bin`) trigger a member walk before giving up.
fn detect_cli(
    root: &Path,
    language: Language,
    name: Option<String>,
    diag: &mut DiagTrace,
) -> DetectCli {
    let Some(name) = name else {
        diag.push(
            "detect_cli",
            "no tool name derivable from the manifest or repo; ".to_string()
                + "cannot probe for a CLI without a name",
        );
        return DetectCli::none();
    };

    let Some(candidate) = primary_cli_candidate(root, language, &name) else {
        // The root didn't yield a runnable CLI. For workspace roots the binary
        // lives in a member crate/package; walk members before reporting a
        // final `has_cli=false`. uv/poetry monorepos are NOT walked yet —
        // doctor notes the gap so the maintainer can run init in the member.
        if language == Language::Rust && is_cargo_workspace_only(root) {
            if let Some(d) = walk_cargo_workspace(root, &name, diag) {
                return d;
            }
        }
        if language == Language::Node && is_npm_workspace_only(root) {
            if let Some(d) = walk_npm_workspace(root, &name, diag) {
                return d;
            }
        }
        diag.push(
            "detect_cli",
            format!(
                "primary_cli_candidate for language `{}` returned None — \
                 runtime may be missing, no build artifact present, or no bin \
                 entry point. Run `skillpack doctor --verbose` to see the raw \
                 profile; if this is a monorepo member, try running \
                 `skillpack init` inside the member directory.",
                language.as_str()
            ),
        );
        // uv / poetry Python monorepo: explicitly NOT walked yet.
        if language == Language::Python
            && (root.join("uv.toml").exists()
                || pyproject_has_tool(root, "uv")
                || pyproject_has_tool(root, "poetry"))
        {
            diag.push(
                "detect_cli.python",
                "uv/poetry workspace detected; member walking not yet \
                 implemented — run `skillpack init` in the member package dir"
                    .to_string(),
            );
        }
        return DetectCli::none();
    };
    spawn_candidate(&candidate, diag)
}

/// Build the `--help` command from `candidate`, spawn it under the hard
/// timeout, and map the outcome to a `DetectCli`. Pushes a diag note on
/// every non-clean outcome so `doctor` explains timeouts/non-zero/missing.
/// Returns `DetectCli::none()` when the spawn can't run at all (NotFound /
/// SpawnFailed), `has_cli=true` with `help_output=None` on a RanNonZero or
/// TimedOut result (the binary exists and responded — it's a CLI — but the
/// help text wasn't captured).
fn spawn_candidate(candidate: &CliCandidate, diag: &mut DiagTrace) -> DetectCli {
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
        SpawnOutcome::RanClean(output) => {
            // A subcommand CLI advertises its subcommands in the top-level
            // `--help`; capture each one's `--help` so the generated SKILL.md
            // documents the real surface (init/verify + their flags, not the
            // global flags). Best-effort: a subcommand that fails/times out is
            // omitted here — `verify` surfaces the gap if the skill documents
            // a subcommand we couldn't capture.
            let subs = capture_subcommand_help(candidate, &output);
            DetectCli {
                has_cli: true,
                command: Some(command),
                help_output: Some(output),
                subcommand_help: subs,
            }
        }
        SpawnOutcome::RanNonZero => {
            diag.push(
                "detect_cli",
                format!(
                    "`{} --help` exited non-zero; help output not captured",
                    command.join(" ")
                ),
            );
            DetectCli {
                has_cli: true,
                command: Some(command),
                help_output: None,
                subcommand_help: Vec::new(),
            }
        }
        SpawnOutcome::TimedOut => {
            diag.push(
                "detect_cli",
                format!(
                    "`{} --help` timed out after {HELP_TIMEOUT:?}",
                    command.join(" ")
                ),
            );
            DetectCli {
                has_cli: true,
                command: Some(command),
                help_output: None,
                subcommand_help: Vec::new(),
            }
        }
        SpawnOutcome::NotFound => {
            diag.push(
                "detect_cli",
                format!(
                    "spawn failed — `{}` binary not found on PATH",
                    command.first().unwrap_or(&candidate.argv[0])
                ),
            );
            DetectCli::none()
        }
        // ponytail: permission-denied etc. are rare; mapping to `none()`
        // means `has_cli=false` (pure-library path) rather than crashing.
        // verify's spawn will then surface the gap downstream if the CLI IS
        // documented. The honest path for V1 — doesn't crash.
        SpawnOutcome::SpawnFailed(_) => {
            diag.push(
                "detect_cli",
                "spawn failed (permission-denied or OS error); treated as has_cli=false"
                    .to_string(),
            );
            DetectCli::none()
        }
    }
}

/// The captured CLI surface: `detect_cli`'s return. Named (not a bare 4-tuple)
/// so the call site reads `d.has_cli` / `d.command` rather than decoding
/// positional fields — and clippy's `type_complexity` stops firing on the
/// `Option<Vec<...>>` pile.
struct DetectCli {
    has_cli: bool,
    command: Option<Vec<String>>,
    help_output: Option<String>,
    subcommand_help: Vec<(String, String)>,
}

impl DetectCli {
    fn none() -> Self {
        Self {
            has_cli: false,
            command: None,
            help_output: None,
            subcommand_help: Vec::new(),
        }
    }
}

/// For a subcommand CLI, spawn `<candidate.argv> <sub> --help` per subcommand
/// advertised in the top-level `--help`, returning `(sub, help)` in declaration
/// order. Reuses the same guarded spawn + timeout as the top-level capture.
/// Failures are omitted silently (introspect is best-effort).
fn capture_subcommand_help(
    candidate: &CliCandidate,
    top_level_help: &str,
) -> Vec<(String, String)> {
    let subs = crate::verify::invocation::extract_subcommands(top_level_help);
    let mut out = Vec::with_capacity(subs.len());
    for sub in subs {
        let mut cmd = Command::new(&candidate.argv[0]);
        for arg in &candidate.argv[1..] {
            cmd.arg(arg);
        }
        cmd.arg(&sub)
            .arg("--help")
            .current_dir(&candidate.spawn_cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let SpawnOutcome::RanClean(help) = spawn_with_timeout(&mut cmd, HELP_TIMEOUT) {
            out.push((sub, help));
        }
    }
    out
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
            let abs = std::fs::canonicalize(&p)
                .ok()
                .and_then(|c| c.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| p.to_string_lossy().to_string());
            return Some(CliCandidate {
                argv: vec![ruby.clone(), abs],
                spawn_cwd: root.to_path_buf(),
            });
        }
    }
    None
}

use crate::spawn::{self, SpawnOutcome, HELP_TIMEOUT};

fn spawn_with_timeout(cmd: &mut Command, timeout: Duration) -> SpawnOutcome {
    spawn::run(cmd, timeout)
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    // Windows only: cmd.exe appends PATHEXT to a bare name; Rust's
    // Command::new does not. Probe `name` plus `name{ext}` for each ext in
    // PATHEXT (e.g. .EXE;.CMD;.BAT) so a PATH lookup resolves `node` to
    // `node.exe`. On Unix the uname-style probe is unchanged (no PATHEXT).
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

/// `git remote get-url origin`, best-effort. Never errors the caller.
fn detect_repo_url(root: &Path) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["remote", "get-url", "origin"]).current_dir(root);
    match spawn_with_timeout(&mut cmd, Duration::from_secs(3)) {
        SpawnOutcome::RanClean(out) => Some(out.trim().to_string()),
        _ => None,
    }
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
            cli_subcommand_help: Vec::new(),
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
    // go.mod `module` line may carry a trailing `// ...` comment. The old
    // parser only trimmed outer whitespace, so the comment bled into the
    // path and the last `/`-segment became a comment fragment (e.g.
    // `github.com/foo/bar // bar tool` → "tool" or worse). Now the first
    // whitespace token is taken before splitting, so the name is "bar".
    #[test]
    fn go_module_name_strips_trailing_line_comment() {
        let root = scratch(&[(
            "go.mod",
            "module github.com/acme/widget // widget CLI\n\ngo 1.21\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Go).as_deref(),
            Some("widget")
        );
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
    // ponytail: walk_*_workspace skip branch (member with no name in manifest
    // AND dir-tail file_name() None) is unreachable for non-root member paths —
    // the path-tail fallback always yields a name. These tests assert the
    // observable contract we DO hit: the walk continues past every member to the
    // end, not aborting on the first no-artifact member. Skip-and-continue vs
    // early-return-None is indistinguishable here only if a name resolution
    // failure occured; the `?`→`continue` fix guards that pathological case.
    #[test]
    fn walk_cargo_workspace_continues_past_no_artifact_member() {
        let root = std::env::temp_dir().join(format!(
            "skillpack-walk-cargo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("members/m1")).unwrap();
        std::fs::create_dir_all(root.join("members/m2")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"members/m1\", \"members/m2\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m1/Cargo.toml"),
            "[package]\nname = \"m1\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m2/Cargo.toml"),
            "[package]\nname = \"m2\"\n",
        )
        .unwrap();
        let mut diag = DiagTrace::default();
        let res = walk_cargo_workspace(&root, "ws", &mut diag);
        assert!(res.is_none(), "no member has a built artifact → None");
        let notes: Vec<&str> = diag.0.iter().map(|d| d.note.as_str()).collect();
        assert!(
            notes.iter().any(|n| n.contains("m1")),
            "m1 probed: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("m2")),
            "m2 probed: {notes:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn walk_npm_workspace_continues_past_no_cli_member() {
        let root = std::env::temp_dir().join(format!(
            "skillpack-walk-npm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("members/m1")).unwrap();
        std::fs::create_dir_all(root.join("members/m2")).unwrap();
        std::fs::write(
            root.join("package.json"),
            "{ \"workspaces\": [\"members/m1\", \"members/m2\"] }",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m1/package.json"),
            "{ \"name\": \"m1\", \"bin\": {} }",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m2/package.json"),
            "{ \"name\": \"m2\", \"bin\": {} }",
        )
        .unwrap();
        let mut diag = DiagTrace::default();
        let res = walk_npm_workspace(&root, "ws", &mut diag);
        assert!(res.is_none(), "bin:{{}} → both candidate None → walk None");
        let notes: Vec<&str> = diag.0.iter().map(|d| d.note.as_str()).collect();
        assert!(
            notes.iter().any(|n| n.contains("m1")),
            "m1 probed: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("m2")),
            "m2 probed: {notes:?}"
        );
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
