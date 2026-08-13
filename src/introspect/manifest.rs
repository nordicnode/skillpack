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
        Language::Unknown => None,
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
        Language::Swift | Language::Go | Language::Unknown => None,
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
}
