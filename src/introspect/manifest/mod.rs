//! Per-language manifest field extraction: name, version, authors, license.
//! Sibling to [`super`]'s CLI detection — this module owns the "read the
//! manifest and pull a scalar" concern (name / version / authors / license /
//! description), while [`super`] owns language detection, CLI candidate
//! resolution, and the guarded `--help` spawn.
//!
//! Extracted from `introspect.rs` (v0.8.3): the manifest parsers were ~1/3
//! of that file and form a self-contained concern with zero calls into the
//! CLI/spawn machinery.
//!
//! Every language lives in its own sibling module implementing [`LanguageSpec`]
//! — detection signal, manifest scalars, and the derived hints (category,
//! cursor globs, import pattern) that used to be spread across
//! `introspect.rs`, `generate.rs`, and `main.rs`. Adding a language is one
//! new module plus one arm in [`language_spec`]'s registry. The shared,
//! multi-language extract helpers (XML tag, YAML scalar, `key: value`, …)
//! stay here so the per-language modules only hold their own format quirks.

use std::fs;
use std::path::{Path, PathBuf};

use crate::types::Language;

mod ccpp;
mod clojure;
mod crystal;
mod csharp;
mod dart;
mod deno;
mod elixir;
mod erlang;
mod go;
mod haskell;
mod julia;
mod jvm;
mod lua;
mod nix;
mod node;
mod ocaml;
mod perl;
mod php;
mod powershell;
mod python;
mod r;
mod ruby;
mod rust;
mod shell;
mod swift;
mod unknown;
mod zig;

// `select_csproj` parses a csproj manifest field; `cli_candidates` resolves
// `csharp_cli_candidate` through it, so it is re-exported for the parent
// module (`super::select_csproj`).
pub(crate) use csharp::select_csproj;

/// Everything skillpack knows about one language: how to detect it on disk,
/// how to pull scalars from its manifest, and the derived hints downstream
/// renderers consume (marketplace category, Cursor auto-attach globs, the
/// library import pattern for polyglot secondaries).
///
/// This is the extension point for new languages: implement it in a new
/// sibling module and register it in [`language_spec`]. Defaults cover the
/// common case (no version/authors/license/description in the manifest).
pub trait LanguageSpec {
    /// Detection signal for `dir`: does the manifest / file pattern for this
    /// language live here? Mirrors the pre-split `language_present` check —
    /// a per-directory probe shared by primary detection and the nested
    /// polyglot walk.
    fn present(&self, dir: &Path) -> bool;
    /// Best-effort package/crate/module name, used by `init --auto` to name
    /// the primary skill and by the polyglot path to derive per-secondary
    /// import patterns.
    fn name(&self, root: &Path) -> Option<String>;
    fn version(&self, _root: &Path) -> Option<String> {
        None
    }
    fn authors(&self, _root: &Path) -> Option<String> {
        None
    }
    fn license(&self, _root: &Path) -> Option<String> {
        None
    }
    /// Fallback description hint for languages whose manifest carries a
    /// top-level `description` — surfaced by `--auto`/`doctor` when the
    /// README has no extractable prose.
    fn description(&self, _root: &Path) -> Option<String> {
        None
    }
    /// Marketplace category phrase ("the Rust tooling", …).
    fn category_hint(&self) -> &'static str;
    /// Cursor auto-attach globs for the language's source files.
    fn cursor_globs(&self) -> Vec<String>;
    /// Library import pattern for a secondary skill in a polyglot monorepo,
    /// given the resolved manifest name (`name` may be a fallback when the
    /// manifest has none).
    fn import_pattern(&self, name: &str) -> String;
}

/// Registry: `Language` → its spec implementation. One arm per language —
/// the single place a new language registers.
pub fn language_spec(lang: Language) -> &'static dyn LanguageSpec {
    match lang {
        Language::Rust => &rust::Rust,
        Language::Node => &node::Node,
        Language::Python => &python::Python,
        Language::Go => &go::Go,
        Language::Ruby => &ruby::Ruby,
        Language::Php => &php::Php,
        Language::Jvm => &jvm::Jvm,
        Language::CSharp => &csharp::CSharp,
        Language::Zig => &zig::Zig,
        Language::Swift => &swift::Swift,
        Language::CCpp => &ccpp::CCpp,
        Language::Elixir => &elixir::Elixir,
        Language::Deno => &deno::Deno,
        Language::Nix => &nix::Nix,
        Language::Dart => &dart::Dart,
        Language::Haskell => &haskell::Haskell,
        Language::Lua => &lua::Lua,
        Language::Julia => &julia::Julia,
        Language::Crystal => &crystal::Crystal,
        Language::Clojure => &clojure::Clojure,
        Language::Ocaml => &ocaml::Ocaml,
        Language::Erlang => &erlang::Erlang,
        Language::R => &r::R,
        Language::Perl => &perl::Perl,
        Language::Shell => &shell::Shell,
        Language::Powershell => &powershell::Powershell,
        Language::Unknown => &unknown::Unknown,
    }
}

/// Pull the project name out of the language manifest, best-effort.
/// Best-effort package/crate/module name from the language manifest at
/// `root`, used by `init --auto` to name the primary skill and by the
/// polyglot path to derive per-secondary-language import patterns. `pub`
/// (not `pub(crate)`) because the bin target consumes it directly.
pub fn project_manifest_name(root: &Path, language: Language) -> Option<String> {
    language_spec(language).name(root)
}

/// Pull the project version out of the language manifest, best-effort.
/// Mirrors [`project_manifest_name`] per language. Returns `None` for Go
/// (`go.mod` has no version field — versioning is via Git tags or a
/// separately-versioned file) and for manifests lacking a version key.
pub(crate) fn project_manifest_version(root: &Path, language: Language) -> Option<String> {
    language_spec(language).version(root)
}

/// Pull the author(s) out of the language manifest, best-effort.
/// Mirrors [`project_manifest_version`] per language. Returns the first
/// author as a display string. `None` when the manifest has no author field
pub(crate) fn project_manifest_authors(root: &Path, language: Language) -> Option<String> {
    language_spec(language)
        .authors(root)
        .map(strip_author_email)
}

pub(crate) fn manifest_license(root: &Path, language: Language) -> Option<String> {
    language_spec(language).license(root)
}

/// Fallback description hint for languages whose manifest carries a top-level
/// `description` (Nix `flake.nix`, Dart `pubspec.yaml`, Crystal `shard.yml`).
/// Used when the README has no extractable prose — so `--auto`/`doctor` still
/// surface something for a flake-only repo.
pub(crate) fn manifest_description(root: &Path, language: Language) -> Option<String> {
    language_spec(language).description(root)
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

// ----- shared extract helpers (used by two or more language modules) -------

/// Extract the first `<tag>...</tag>` content from raw XML. Best-effort
/// string find — avoids pulling in an XML parser for scalar field extraction
/// (pom.xml name, version, artifactId). Trims whitespace around the value.
pub(crate) fn extract_xml_tag(raw: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = raw.find(&open)? + open.len();
    let rest = &raw[start..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim().to_string())
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

/// Upper-camel-case a kebab/snake name for languages whose import statement
/// names a module (`import Foo.Bar`), e.g. `my-lib` → `MyLib`.
pub(crate) fn pascal_name(name: &str) -> String {
    name.split(['-', '_', '.'])
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut c = seg.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Shared scratch-repo helpers for the per-language parser tests.

    use std::path::{Path, PathBuf};

    pub(crate) fn scratch(files: &[(&str, &str)]) -> PathBuf {
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

    pub(crate) fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }
}
