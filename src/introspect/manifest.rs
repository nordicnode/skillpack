//! Per-language manifest field extraction: name, version, authors, license.
//! Sibling to [`super`]'s CLI detection — this module owns the "read the
//! manifest and pull a scalar" concern (name / version / authors / license),
//! while [`super`] owns language detection, CLI candidate resolution, and
//! the guarded `--help` spawn.
//!
//! Extracted from `introspect.rs` (v0.8.3): the manifest parsers were ~1/3
//! of that file and form a self-contained concern with zero calls into the
//! CLI/spawn machinery. The shared `select_csproj` helper lives here (it
//! parses a csproj manifest field) and is re-exported by [`super`] for
//! `csharp_cli_candidate`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::types::Language;

/// Select the best csproj at root for CLI invocation. Prefers one with
/// `<OutputType>Exe</OutputType>`, skipping `WinExe` (GUI — no stdout).
/// Ties broken lexicographically by filename for cross-platform determinism.
/// Returns the path to the csproj, or `None` if none are suitable.
pub(crate) fn select_csproj(root: &Path) -> Option<PathBuf> {
    let mut csprojs: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("csproj"))
        .collect();
    // Deterministic order for cross-platform parity (dir iteration order
    // varies by OS/filesystem).
    csprojs.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    // Pass 1: a csproj explicitly declaring Exe/Console.
    for p in &csprojs {
        if let Ok(raw) = fs::read_to_string(p) {
            match extract_xml_tag(&raw, "OutputType").as_deref() {
                Some("WinExe") => continue,
                Some("Exe") | Some("Console") => return Some(p.clone()),
                _ => {}
            }
        }
    }
    // Pass 2: no explicit OutputType — assume first non-WinExe csproj is a CLI.
    for p in &csprojs {
        if let Ok(raw) = fs::read_to_string(p) {
            if extract_xml_tag(&raw, "OutputType").as_deref() == Some("WinExe") {
                continue;
            }
        }
        return Some(p.clone());
    }
    None
}

/// Pull the project name out of the language manifest, best-effort.
pub(crate) fn project_manifest_name(root: &Path, language: Language) -> Option<String> {
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
            // pyproject.toml [project] name = "...", fallback [tool.poetry] name = "..."
            if let Ok(raw) = fs::read_to_string(root.join("pyproject.toml")) {
                if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
                    if let Some(name) = v
                        .get("project")
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                    {
                        return Some(name.to_string());
                    }
                    if let Some(name) = v
                        .get("tool")
                        .and_then(|t| t.get("poetry"))
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                    {
                        return Some(name.to_string());
                    }
                    if let Some(name) = v
                        .get("tool")
                        .and_then(|t| t.get("flit"))
                        .and_then(|f| f.get("metadata"))
                        .and_then(|m| m.get("module"))
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
        Language::Php => {
            let raw = fs::read_to_string(root.join("composer.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            v.get("name")?
                .as_str()
                .map(std::string::ToString::to_string)
        }
        Language::Jvm => {
            // pom.xml: <name>...</name> or <artifactId>...</artifactId>;
            // build.gradle: rootProject.name = '...' or rootProject.name = "..."
            if let Ok(raw) = fs::read_to_string(root.join("pom.xml")) {
                if let Some(n) = extract_xml_tag(&raw, "name") {
                    return Some(n);
                }
                if let Some(n) = extract_xml_tag(&raw, "artifactId") {
                    return Some(n);
                }
            }
            for gradle in &["build.gradle", "build.gradle.kts"] {
                if let Ok(raw) = fs::read_to_string(root.join(gradle)) {
                    if let Some(n) = extract_gradle_string(&raw, "rootProject.name") {
                        return Some(n);
                    }
                }
            }
            None
        }
        Language::CSharp => {
            if let Some(csproj) = select_csproj(root) {
                if let Ok(raw) = fs::read_to_string(&csproj) {
                    if let Some(n) = extract_xml_tag(&raw, "AssemblyName") {
                        return Some(n);
                    }
                    if let Some(n) = extract_xml_tag(&raw, "RootNamespace") {
                        return Some(n);
                    }
                }
            }
            None
        }
        Language::Zig => {
            if let Ok(raw) = fs::read_to_string(root.join("build.zig.zon")) {
                if let Some(n) = extract_zig_zon_field(&raw, "name") {
                    return Some(n);
                }
            }
            None
        }
        Language::Swift => {
            if let Ok(raw) = fs::read_to_string(root.join("Package.swift")) {
                if let Some(n) = extract_swift_package_name(&raw) {
                    return Some(n);
                }
            }
            None
        }
        Language::CCpp => {
            if let Ok(raw) = fs::read_to_string(root.join("CMakeLists.txt")) {
                if let Some(n) = extract_cmake_project_name(&raw) {
                    return Some(n);
                }
            }
            if let Ok(raw) = fs::read_to_string(root.join("meson.build")) {
                if let Some(n) = extract_meson_project_name(&raw) {
                    return Some(n);
                }
            }
            None
        }
        Language::Elixir => {
            if let Ok(raw) = fs::read_to_string(root.join("mix.exs")) {
                if let Some(n) = extract_elixir_app_name(&raw) {
                    return Some(n);
                }
            }
            None
        }
        Language::Deno => {
            for f in &["deno.json", "deno.jsonc"] {
                if let Ok(raw) = fs::read_to_string(root.join(f)) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(n) = v.get("name").and_then(|n| n.as_str()) {
                            return Some(n.to_string());
                        }
                    }
                }
            }
            None
        }
        Language::Dart => {
            let raw = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
            extract_yaml_scalar(&raw, "name")
        }
        Language::Haskell => {
            let cabal = first_file_with_ext(root, "cabal")?;
            let raw = fs::read_to_string(&cabal).ok()?;
            extract_key_colon_value(&raw, "name")
        }
        Language::Lua => {
            let rockspec = first_file_with_ext(root, "rockspec")?;
            let raw = fs::read_to_string(&rockspec).ok()?;
            extract_rockspec_field(&raw, "package")
        }
        Language::Julia => {
            let raw = fs::read_to_string(root.join("Project.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        }
        Language::Crystal => {
            let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
            extract_yaml_scalar(&raw, "name")
        }
        Language::Clojure => {
            let raw = fs::read_to_string(root.join("project.clj")).ok()?;
            extract_clojure_defproject(&raw).map(|(n, _)| n)
        }
        Language::Ocaml => {
            if let Some(opam) = first_file_with_ext(root, "opam") {
                if let Ok(raw) = fs::read_to_string(&opam) {
                    if let Some(n) = extract_key_colon_value(&raw, "name") {
                        return Some(n);
                    }
                }
            }
            let raw = fs::read_to_string(root.join("dune-project")).ok()?;
            extract_dune_field(&raw, "name")
        }
        Language::Erlang => {
            let app_src = first_file_ending_with(root, ".app.src")?;
            let raw = fs::read_to_string(&app_src).ok()?;
            extract_app_src_name(&raw)
        }
        Language::R => {
            let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
            extract_key_colon_value(&raw, "Package")
        }
        Language::Perl => {
            if let Ok(raw) = fs::read_to_string(root.join("META.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(n) = v.get("name").and_then(|n| n.as_str()) {
                        return Some(n.to_string());
                    }
                }
            }
            let raw = fs::read_to_string(root.join("Makefile.PL")).ok()?;
            extract_makefile_pl_field(&raw, "NAME")
        }
        Language::Nix | Language::Unknown => None,
    }
}

/// Pull the project version out of the language manifest, best-effort.
/// Mirrors [`project_manifest_name`] per language. Returns `None` for Go
/// (`go.mod` has no version field — versioning is via Git tags or a
/// separately-versioned file) and for manifests lacking a version key.
pub(crate) fn project_manifest_version(root: &Path, language: Language) -> Option<String> {
    match language {
        Language::Rust => {
            let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("package")
                .and_then(|p| p.get("version"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    v.get("workspace")
                        .and_then(|w| w.get("package"))
                        .and_then(|p| p.get("version"))
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
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
                    if let Some(ver) = v
                        .get("tool")
                        .and_then(|t| t.get("poetry"))
                        .and_then(|p| p.get("version"))
                        .and_then(|n| n.as_str())
                    {
                        return Some(ver.to_string());
                    }
                    if let Some(ver) = v
                        .get("tool")
                        .and_then(|t| t.get("flit"))
                        .and_then(|f| f.get("metadata"))
                        .and_then(|m| m.get("version"))
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
        Language::Php => {
            let raw = fs::read_to_string(root.join("composer.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            v.get("version")?
                .as_str()
                .map(std::string::ToString::to_string)
        }
        Language::Jvm => {
            // pom.xml: <version>...</version>; build.gradle: version = '...'
            if let Ok(raw) = fs::read_to_string(root.join("pom.xml")) {
                if let Some(v) = extract_xml_tag(&raw, "version") {
                    return Some(v);
                }
            }
            for gradle in &["build.gradle", "build.gradle.kts"] {
                if let Ok(raw) = fs::read_to_string(root.join(gradle)) {
                    if let Some(v) = extract_gradle_string(&raw, "version") {
                        return Some(v);
                    }
                }
            }
            None
        }
        Language::CSharp => select_csproj(root)
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|raw| extract_xml_tag(&raw, "Version")),
        Language::Zig => {
            if let Ok(raw) = fs::read_to_string(root.join("build.zig.zon")) {
                if let Some(v) = extract_zig_zon_field(&raw, "version") {
                    return Some(v);
                }
            }
            None
        }
        Language::Elixir => {
            if let Ok(raw) = fs::read_to_string(root.join("mix.exs")) {
                if let Some(v) = extract_elixir_version(&raw) {
                    return Some(v);
                }
            }
            None
        }
        Language::CCpp => {
            if let Ok(raw) = fs::read_to_string(root.join("CMakeLists.txt")) {
                if let Some(v) = extract_cmake_project_version(&raw) {
                    return Some(v);
                }
            }
            if let Ok(raw) = fs::read_to_string(root.join("meson.build")) {
                if let Some(v) = extract_meson_project_version(&raw) {
                    return Some(v);
                }
            }
            None
        }
        Language::Deno => {
            for f in &["deno.json", "deno.jsonc"] {
                if let Ok(raw) = fs::read_to_string(root.join(f)) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(ver) = v.get("version").and_then(|n| n.as_str()) {
                            return Some(ver.to_string());
                        }
                    }
                }
            }
            None
        }
        Language::Dart => {
            let raw = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
            extract_yaml_scalar(&raw, "version")
        }
        Language::Haskell => {
            let cabal = first_file_with_ext(root, "cabal")?;
            let raw = fs::read_to_string(&cabal).ok()?;
            extract_key_colon_value(&raw, "version")
        }
        Language::Lua => {
            let rockspec = first_file_with_ext(root, "rockspec")?;
            let raw = fs::read_to_string(&rockspec).ok()?;
            extract_rockspec_field(&raw, "version")
        }
        Language::Julia => {
            let raw = fs::read_to_string(root.join("Project.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("version")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        }
        Language::Crystal => {
            let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
            extract_yaml_scalar(&raw, "version")
        }
        Language::Clojure => {
            let raw = fs::read_to_string(root.join("project.clj")).ok()?;
            extract_clojure_defproject(&raw).map(|(_, v)| v)
        }
        Language::Ocaml => {
            if let Some(opam) = first_file_with_ext(root, "opam") {
                if let Ok(raw) = fs::read_to_string(&opam) {
                    if let Some(v) = extract_key_colon_value(&raw, "version") {
                        return Some(v);
                    }
                }
            }
            let raw = fs::read_to_string(root.join("dune-project")).ok()?;
            extract_dune_field(&raw, "version")
        }
        Language::Erlang => {
            let app_src = first_file_ending_with(root, ".app.src")?;
            let raw = fs::read_to_string(&app_src).ok()?;
            extract_app_src_vsn(&raw)
        }
        Language::R => {
            let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
            extract_key_colon_value(&raw, "Version")
        }
        Language::Perl => {
            if let Ok(raw) = fs::read_to_string(root.join("META.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(ver) = v.get("version").and_then(|n| n.as_str()) {
                        return Some(ver.to_string());
                    }
                }
            }
            let raw = fs::read_to_string(root.join("Makefile.PL")).ok()?;
            extract_makefile_pl_field(&raw, "VERSION")
        }
        Language::Swift | Language::Go | Language::Nix | Language::Unknown => None,
    }
}

/// Pull the author(s) out of the language manifest, best-effort.
/// Mirrors [`project_manifest_version`] per language. Returns the first
/// author as a display string. `None` when the manifest has no author field
pub(crate) fn project_manifest_authors(root: &Path, language: Language) -> Option<String> {
    project_manifest_authors_raw(root, language).map(strip_author_email)
}

fn project_manifest_authors_raw(root: &Path, language: Language) -> Option<String> {
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
                    // Poetry: [tool.poetry.authors] = ["Name <email>"]
                    if let Some(arr) = v
                        .get("tool")
                        .and_then(|t| t.get("poetry"))
                        .and_then(|p| p.get("authors"))
                        .and_then(|a| a.as_array())
                    {
                        if let Some(first) = arr.first().and_then(|s| s.as_str()) {
                            return Some(first.to_string());
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
        Language::Php => {
            let raw = fs::read_to_string(root.join("composer.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            // composer.json "authors" is [{"name": "...", "email": "..."}]
            v.get("authors")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|e| {
                    e.get("name")
                        .and_then(|n| n.as_str())
                        .or_else(|| e.as_str())
                })
                .map(|s| s.to_string())
        }
        Language::Jvm => {
            // pom.xml: <developers><developer><name>...</name></developer></developers>
            if let Ok(raw) = fs::read_to_string(root.join("pom.xml")) {
                if let Some(devs) = extract_xml_tag(&raw, "developers") {
                    if let Some(name) = extract_xml_tag(&devs, "name") {
                        return Some(name);
                    }
                }
            }
            // build.gradle has no standard authors field.
            None
        }
        Language::CSharp => select_csproj(root)
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|raw| extract_xml_tag(&raw, "Authors"))
            .and_then(|a| a.split(',').next().map(|s| s.trim().to_string())),
        Language::Julia => {
            let raw = fs::read_to_string(root.join("Project.toml")).ok()?;
            let v = toml::from_str::<toml::Value>(&raw).ok()?;
            v.get("authors")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        }
        Language::Crystal => {
            let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
            extract_yaml_list_first(&raw, "authors")
        }
        Language::Ocaml => {
            if let Some(opam) = first_file_with_ext(root, "opam") {
                if let Ok(raw) = fs::read_to_string(&opam) {
                    if let Some(a) = extract_key_colon_value(&raw, "authors") {
                        return Some(a);
                    }
                }
            }
            None
        }
        Language::R => {
            let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
            extract_key_colon_value(&raw, "Author")
        }
        Language::Perl => {
            if let Ok(raw) = fs::read_to_string(root.join("META.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(a) = v.get("author") {
                        if let Some(s) = a.as_str() {
                            return Some(s.to_string());
                        }
                        if let Some(arr) = a.as_array() {
                            if let Some(first) = arr.first().and_then(|s| s.as_str()) {
                                return Some(first.to_string());
                            }
                        }
                    }
                }
            }
            None
        }
        _ => None,
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

pub(crate) fn manifest_license(root: &Path, language: Language) -> Option<String> {
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
        Language::Python => {
            let raw = fs::read_to_string(root.join("pyproject.toml")).ok()?;
            let v: toml::Value = toml::from_str(&raw).ok()?;
            // PEP 621: [project] license = "MIT" or license = { text = "MIT" }
            if let Some(lic) = v.get("project").and_then(|p| p.get("license")) {
                if let Some(s) = lic.as_str() {
                    return Some(s.to_string());
                }
                if let Some(text) = lic.get("text").and_then(|t| t.as_str()) {
                    return Some(text.to_string());
                }
            }
            // Poetry: [tool.poetry] license = "MIT"
            v.get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("license"))
                .and_then(|l| l.as_str())
                .map(|s| s.to_string())
        }
        Language::Php => {
            let raw = fs::read_to_string(root.join("composer.json")).ok()?;
            let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
            v.get("license")?
                .as_str()
                .map(std::string::ToString::to_string)
        }
        Language::Ruby => {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("gemspec") {
                        if let Ok(raw) = fs::read_to_string(&p) {
                            if let Some(line) = raw
                                .lines()
                                .find(|l| l.contains("spec.license") || l.contains(".license ="))
                            {
                                if let Some(lic) = extract_ruby_string_value(line) {
                                    return Some(lic.to_string());
                                }
                            }
                        }
                    }
                }
            }
            None
        }
        Language::CSharp => select_csproj(root)
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|raw| extract_xml_tag(&raw, "PackageLicenseExpression")),
        Language::Deno => {
            for f in &["deno.json", "deno.jsonc"] {
                if let Ok(raw) = fs::read_to_string(root.join(f)) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(lic) = v.get("license").and_then(|n| n.as_str()) {
                            return Some(lic.to_string());
                        }
                    }
                }
            }
            None
        }
        Language::Zig => {
            if let Ok(raw) = fs::read_to_string(root.join("build.zig.zon")) {
                if let Some(lic) = extract_zig_zon_field(&raw, "license") {
                    return Some(lic);
                }
            }
            None
        }
        Language::Crystal => {
            let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
            extract_yaml_scalar(&raw, "license")
        }
        Language::R => {
            let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
            extract_key_colon_value(&raw, "License")
        }
        Language::Perl => {
            if let Ok(raw) = fs::read_to_string(root.join("META.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    // CPAN META.json spells the license as a list of SPDX ids.
                    if let Some(lic) = v.get("license") {
                        if let Some(s) = lic.as_str() {
                            return Some(s.to_string());
                        }
                        if let Some(arr) = lic.as_array() {
                            if let Some(first) = arr.first().and_then(|s| s.as_str()) {
                                return Some(first.to_string());
                            }
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Fallback description hint for languages whose manifest carries a top-level
/// `description` (Nix `flake.nix`, Dart `pubspec.yaml`, Crystal `shard.yml`).
/// Used when the README has no extractable prose — so `--auto`/`doctor` still
/// surface something for a flake-only repo.
pub(crate) fn manifest_description(root: &Path, language: Language) -> Option<String> {
    match language {
        Language::Nix => {
            let raw = fs::read_to_string(root.join("flake.nix")).ok()?;
            extract_flake_description(&raw)
        }
        Language::Dart => {
            let raw = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
            extract_yaml_scalar(&raw, "description")
        }
        Language::Crystal => {
            let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
            extract_yaml_scalar(&raw, "description")
        }
        _ => None,
    }
}

/// Extract the first `<tag>...</tag>` content from raw XML. Best-effort
/// string find — avoids pulling in an XML parser for scalar field extraction
/// (pom.xml name, version, artifactId). Trims whitespace around the value.
fn extract_xml_tag(raw: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = raw.find(&open)? + open.len();
    let rest = &raw[start..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim().to_string())
}

/// Extract a `key = "value"` or `key = 'value'` string from a Gradle build
/// file. Best-effort line scan mirroring [`extract_ruby_string_value`].
/// Handles both `rootProject.name = '...'` and `version = '...'` forms.
fn extract_gradle_string(raw: &str, key: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                if let Some(s) = rest
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

fn extract_ruby_string_value(line: &str) -> Option<String> {
    let after = line.split('=').nth(1)?.trim();
    let s = after.trim_start_matches(['"', '\'']);
    let s = s.split(['"', '\'']).next()?.trim();
    Some(s.to_string())
}

fn extract_cmake_project_name(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.to_ascii_lowercase().find("project(") {
            let inside = &trimmed[i + 8..];
            if let Some(end) = inside.find(')') {
                let inner = inside[..end].trim();
                let first_tok = inner.split_whitespace().next()?;
                let clean = first_tok.trim_matches(|c| c == '"' || c == '\'');
                if !clean.is_empty() {
                    return Some(clean.to_string());
                }
            }
        }
    }
    None
}

fn extract_cmake_project_version(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.to_ascii_lowercase().find("project(") {
            let inside = &trimmed[i + 8..];
            let upper = inside.to_uppercase();
            if let Some(v_idx) = upper.find("VERSION") {
                let after = &inside[v_idx + 7..].trim_start();
                let tok = after
                    .split(|c: char| c.is_whitespace() || c == ')' || c == '"' || c == '\'')
                    .next()?;
                if !tok.is_empty() && tok.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Some(tok.to_string());
                }
            }
        }
    }
    None
}

fn extract_meson_project_name(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(inside) = trimmed.strip_prefix("project(") {
            if let Some(comma_or_end) = inside.find([',', ')']) {
                let first = &inside[..comma_or_end].trim();
                let clean = first.trim_matches(|c| c == '"' || c == '\'');
                if !clean.is_empty() {
                    return Some(clean.to_string());
                }
            }
        }
    }
    None
}

fn extract_meson_project_version(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.find("version:") {
            let after = trimmed[i + 8..].trim();
            let clean = after
                .trim_matches(|c: char| c == ',' || c == '"' || c == '\'' || c.is_whitespace());
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }
    None
}

fn extract_zig_zon_field(raw: &str, field: &str) -> Option<String> {
    let dot_field = format!(".{field}");
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&dot_field) || trimmed.starts_with(field) {
            if let Some(eq) = trimmed.find('=') {
                let val = trimmed[eq + 1..].trim();
                let clean = val.trim_matches(|c: char| {
                    c == ',' || c == '"' || c == '\'' || c == '.' || c.is_whitespace()
                });
                if !clean.is_empty() {
                    return Some(clean.to_string());
                }
            }
        }
    }
    None
}

fn extract_swift_package_name(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.find("name:") {
            let after = trimmed[i + 5..].trim();
            let clean = after
                .trim_matches(|c: char| c == ',' || c == '"' || c == '\'' || c.is_whitespace());
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }
    None
}

fn extract_elixir_app_name(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.find("app:") {
            let after = trimmed[i + 4..].trim();
            let clean = after.trim_matches(|c: char| {
                c == ':' || c == ',' || c == '"' || c == '\'' || c.is_whitespace()
            });
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }
    None
}

fn extract_elixir_version(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.find("version:") {
            let after = trimmed[i + 8..].trim();
            let clean = after
                .trim_matches(|c: char| c == ',' || c == '"' || c == '\'' || c.is_whitespace());
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }
    None
}

/// First top-level file with the given extension (case-sensitive), or `None`.
pub(crate) fn first_file_with_ext(root: &Path, ext: &str) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    files.into_iter().next()
}

/// First top-level file whose name ends with `suffix` (e.g. `.app.src` — a
/// double extension that [`first_file_with_ext`] can't match because
/// `Path::extension` returns only the last segment). Deterministic (sorted).
pub(crate) fn first_file_ending_with(root: &Path, suffix: &str) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
        })
        .collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    files.into_iter().next()
}

/// Extract a top-level `key: value` scalar from a YAML manifest (pubspec.yaml,
/// shard.yml). A line starting with `key:` at column 0 whose value is a bare
/// scalar (not a nested map/list) is returned with quotes trimmed. Nested keys
/// (indented `key:`) are skipped so `flutter:` doesn't shadow `name:`.
pub(crate) fn extract_yaml_scalar(raw: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in raw.lines() {
        if !line.starts_with(&prefix) {
            continue;
        }
        let value = line[prefix.len()..].trim();
        // A `key:` with nothing after it (or a trailing `|` block) is not a
        // scalar — the value continues on later lines. Skip it.
        if value.is_empty() || value == "|" || value == ">" {
            continue;
        }
        let clean =
            value.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c.is_whitespace());
        if !clean.is_empty() {
            return Some(clean.to_string());
        }
    }
    None
}

/// First entry of a top-level `key:` YAML list (e.g. Crystal `authors:`).
/// Handles inline flow lists (`authors: [A, B]`) and the first indented
/// `- item` of a block list. Returns the bare item with quotes/dashes trimmed.
pub(crate) fn extract_yaml_list_first(raw: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let mut lines = raw.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with(&prefix) {
            continue;
        }
        let value = line[prefix.len()..].trim();
        // Inline flow list: authors: ["A <a@b>", "B"]
        if let Some(rest) = value.strip_prefix('[') {
            let first = rest
                .split([',', ']'])
                .next()?
                .trim()
                .trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
        // Block list: the next indented `- item` line(s).
        for next in lines.by_ref() {
            let t = next.trim();
            if let Some(item) = t.strip_prefix('-') {
                let first = item.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
                if !first.is_empty() {
                    return Some(first.to_string());
                }
                return None;
            }
            if !t.is_empty() && !next.starts_with(char::is_whitespace) {
                break;
            }
        }
        return None;
    }
    None
}

/// Extract a `field = "value"` scalar from a Lua rockspec (Lua table syntax).
pub(crate) fn extract_rockspec_field(raw: &str, field: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(field) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                if let Some(s) = rest
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                {
                    if !s.is_empty() {
                        return Some(s.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Parse the `(defproject name "version" ...)` head of a Leiningen `project.clj`.
/// Returns `(name, version)`.
pub(crate) fn extract_clojure_defproject(raw: &str) -> Option<(String, String)> {
    let line = raw
        .lines()
        .find(|l| l.trim_start().starts_with("(defproject"))?;
    let after = line.trim_start().strip_prefix("(defproject")?;
    let mut toks = after.split_whitespace().filter(|t| !t.is_empty());
    let name = toks.next()?.to_string();
    let version = toks
        .next()?
        .trim_matches(|c: char| c == '"' || c == '\'')
        .to_string();
    if name.is_empty() || version.is_empty() {
        return None;
    }
    Some((name, version))
}

/// Extract a `key: value` scalar from an OCaml `.opam` file (or an R DCF
/// `Key: Value` file — both share the `Key:` line-prefix shape).
pub(crate) fn extract_key_colon_value(raw: &str, key: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix(':') {
                let value = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Extract a `(name <value>)` s-expression field from a `dune-project` file.
pub(crate) fn extract_dune_field(raw: &str, key: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let value = rest.trim().trim_matches(|c| c == ')' || c == '(');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Extract the application name from an Erlang `.app.src`
/// `{application, my_app, [...]}` term.
pub(crate) fn extract_app_src_name(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .find(|l| l.trim().starts_with("{application,"))?;
    let after = line.trim().strip_prefix("{application,")?;
    let name = after.split(',').next()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Extract the `vsn` value from an Erlang `.app.src` term.
pub(crate) fn extract_app_src_vsn(raw: &str) -> Option<String> {
    let line = raw.lines().find(|l| {
        let t = l.trim();
        t.starts_with("vsn") || t.starts_with("{vsn")
    })?;
    let t = line.trim().trim_start_matches('{');
    let after = t.strip_prefix("vsn")?.trim_start();
    let value = after
        .trim_start_matches(',')
        .split([',', '}', ']', ')'])
        .next()?
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'');
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

/// Extract the `description = "..."` scalar from a Nix flake. A Nix attribute
/// value is terminated by a `;` (e.g. `description = "...";`), which must be
/// stripped before unquoting.
pub(crate) fn extract_flake_description(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("description") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                // Strip the trailing `;` (and any whitespace) before unquoting.
                let rest = rest.trim().trim_end_matches(';').trim();
                if let Some(s) = rest
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                {
                    if !s.is_empty() {
                        return Some(s.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Extract a `WriteMakefile(NAME => 'Foo-Bar', ...)` key/value pair from a
/// Perl `Makefile.PL`.
pub(crate) fn extract_makefile_pl_field(raw: &str, key: &str) -> Option<String> {
    let needle = format!("{key} =>");
    let line = raw.lines().find(|l| l.contains(&needle))?;
    let after = line.split(&needle).nth(1)?.trim();
    let value = after
        .split([',', ')'])
        .next()?
        .trim()
        .trim_matches(|c: char| c == '\'' || c == '"' || c.is_whitespace());
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    //! Bug #1 + #2: the Rust manifest name/license parsers used to hand-scan
    //! Cargo.toml lines, which misread `name="x"` (no space) and `name = { workspace
    //! = true }` (extracted "{ workspace" as the name). Now go through the real
    //! toml crate — these tests pin both regressions.

    use super::*;
    use crate::types::Language;

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

    #[test]
    fn zig_manifest_parses_name_and_version() {
        let root = scratch(&[(
            "build.zig.zon",
            ".{\n    .name = \"zig-frob\",\n    .version = \"0.4.2\",\n    .paths = .{\"\"},\n}\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Zig).as_deref(),
            Some("zig-frob")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Zig).as_deref(),
            Some("0.4.2")
        );
        cleanup(&root);
    }

    #[test]
    fn swift_manifest_parses_package_name() {
        let root = scratch(&[(
            "Package.swift",
            "// swift-tools-version: 5.9\nimport PackageDescription\nlet package = Package(\n    name: \"SwiftCLI\",\n    products: []\n)\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Swift).as_deref(),
            Some("SwiftCLI")
        );
        cleanup(&root);
    }

    #[test]
    fn cmake_manifest_parses_name_and_version() {
        let root = scratch(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(SuperEngine VERSION 2.1.0 LANGUAGES CXX)\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::CCpp).as_deref(),
            Some("SuperEngine")
        );
        assert_eq!(
            project_manifest_version(&root, Language::CCpp).as_deref(),
            Some("2.1.0")
        );
        cleanup(&root);
    }

    #[test]
    fn elixir_manifest_parses_name_and_version() {
        let root = scratch(&[(
            "mix.exs",
            "defmodule MyTool.MixProject do\n  use Mix.Project\n  def project do\n    [\n      app: :my_tool,\n      version: \"1.3.0\",\n      elixir: \"~> 1.14\"\n    ]\n  end\nend\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Elixir).as_deref(),
            Some("my_tool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Elixir).as_deref(),
            Some("1.3.0")
        );
        cleanup(&root);
    }

    #[test]
    fn deno_manifest_parses_name_version_and_license() {
        let root = scratch(&[(
            "deno.json",
            "{\n  \"name\": \"@scope/deno-tool\",\n  \"version\": \"0.9.1\",\n  \"license\": \"MIT\"\n}\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Deno).as_deref(),
            Some("@scope/deno-tool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Deno).as_deref(),
            Some("0.9.1")
        );
        assert_eq!(
            manifest_license(&root, Language::Deno).as_deref(),
            Some("MIT")
        );
        cleanup(&root);
    }

    #[test]
    fn dart_pubspec_parses_name_version_and_description() {
        let root = scratch(&[(
            "pubspec.yaml",
            "name: my_dart_tool\nversion: 2.1.0\ndescription: A Dart CLI.\nenvironment:\n  sdk: '>=3.0.0 <4.0.0'\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Dart).as_deref(),
            Some("my_dart_tool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Dart).as_deref(),
            Some("2.1.0")
        );
        assert_eq!(
            manifest_description(&root, Language::Dart).as_deref(),
            Some("A Dart CLI.")
        );
        cleanup(&root);
    }

    #[test]
    fn haskell_cabal_parses_name_and_version() {
        let root = scratch(&[(
            "mytool.cabal",
            "name:                mytool\nversion:             0.4.1\nbuild-type:          Simple\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Haskell).as_deref(),
            Some("mytool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Haskell).as_deref(),
            Some("0.4.1")
        );
        cleanup(&root);
    }

    #[test]
    fn lua_rockspec_parses_package_and_version() {
        let root = scratch(&[(
            "mylua-1.0-1.rockspec",
            "package = \"mylua\"\nversion = \"1.0-1\"\ndescription = { summary = \"x\" }\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Lua).as_deref(),
            Some("mylua")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Lua).as_deref(),
            Some("1.0-1")
        );
        cleanup(&root);
    }

    #[test]
    fn julia_project_toml_parses_name_version_and_authors() {
        let root = scratch(&[(
            "Project.toml",
            "name = \"MyJuliaTool\"\nuuid = \"...\"\nversion = \"0.3.0\"\nauthors = [\"Ada Lovelace <ada@x.io>\"]\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Julia).as_deref(),
            Some("MyJuliaTool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Julia).as_deref(),
            Some("0.3.0")
        );
        assert_eq!(
            project_manifest_authors(&root, Language::Julia).as_deref(),
            Some("Ada Lovelace")
        );
        cleanup(&root);
    }

    #[test]
    fn crystal_shard_yml_parses_name_version_and_license() {
        let root = scratch(&[(
            "shard.yml",
            "name: mycrystal\nversion: 1.2.0\nlicense: MIT\nauthors:\n  - Grace Hopper <grace@x.io>\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Crystal).as_deref(),
            Some("mycrystal")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Crystal).as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            manifest_license(&root, Language::Crystal).as_deref(),
            Some("MIT")
        );
        assert_eq!(
            project_manifest_authors(&root, Language::Crystal).as_deref(),
            Some("Grace Hopper")
        );
        cleanup(&root);
    }

    #[test]
    fn clojure_project_clj_parses_defproject() {
        let root = scratch(&[(
            "project.clj",
            "(defproject myclj \"0.9.0\"\n  :description \"A Clojure CLI\"\n  :dependencies [[org.clojure/clojure \"1.11.1\"]])\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Clojure).as_deref(),
            Some("myclj")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Clojure).as_deref(),
            Some("0.9.0")
        );
        cleanup(&root);
    }

    #[test]
    fn ocaml_opam_parses_name_version_authors() {
        let root = scratch(&[(
            "myocaml.opam",
            "opam-version: \"2.0\"\nname: \"myocaml\"\nversion: \"0.2.0\"\nauthors: \"A. Turing\"\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Ocaml).as_deref(),
            Some("myocaml")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Ocaml).as_deref(),
            Some("0.2.0")
        );
        assert_eq!(
            project_manifest_authors(&root, Language::Ocaml).as_deref(),
            Some("A. Turing")
        );
        cleanup(&root);
    }

    #[test]
    fn erlang_app_src_parses_name_and_vsn() {
        let root = scratch(&[(
            "myerlang.app.src",
            "{application, myerlang,\n  [{description, \"x\"},\n   {vsn, \"2.1.0\"}]}.\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Erlang).as_deref(),
            Some("myerlang")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Erlang).as_deref(),
            Some("2.1.0")
        );
        cleanup(&root);
    }

    #[test]
    fn r_description_parses_package_version_license() {
        let root = scratch(&[(
            "DESCRIPTION",
            "Package: myrtool\nVersion: 0.5.1\nLicense: MIT\nAuthor: K. Pearson\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::R).as_deref(),
            Some("myrtool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::R).as_deref(),
            Some("0.5.1")
        );
        assert_eq!(manifest_license(&root, Language::R).as_deref(), Some("MIT"));
        assert_eq!(
            project_manifest_authors(&root, Language::R).as_deref(),
            Some("K. Pearson")
        );
        cleanup(&root);
    }

    #[test]
    fn perl_meta_json_parses_name_version_license() {
        let root = scratch(&[(
            "META.json",
            "{\"name\":\"MyPerlTool\",\"version\":\"0.7.0\",\"license\":[\"perl_5\"],\"author\":[\"L. Wall\"]}\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Perl).as_deref(),
            Some("MyPerlTool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Perl).as_deref(),
            Some("0.7.0")
        );
        assert_eq!(
            manifest_license(&root, Language::Perl).as_deref(),
            Some("perl_5")
        );
        assert_eq!(
            project_manifest_authors(&root, Language::Perl).as_deref(),
            Some("L. Wall")
        );
        cleanup(&root);
    }

    #[test]
    fn nix_flake_description_is_captured() {
        let root = scratch(&[(
            "flake.nix",
            "{\n  description = \"A reproducible dev environment\";\n  inputs = {};\n}\n",
        )]);
        assert_eq!(
            manifest_description(&root, Language::Nix).as_deref(),
            Some("A reproducible dev environment")
        );
        cleanup(&root);
    }
}
