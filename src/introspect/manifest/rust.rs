//! Rust: `Cargo.toml`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Rust;

impl LanguageSpec for Rust {
    fn present(&self, dir: &Path) -> bool {
        dir.join("Cargo.toml").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
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

    fn version(&self, root: &Path) -> Option<String> {
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

    fn authors(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("package")
            .and_then(|p| p.get("authors"))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
    }

    fn license(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("package")
            .and_then(|p| p.get("license"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    fn category_hint(&self) -> &'static str {
        "the Rust tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.rs".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("use {crate_name}::…;", crate_name = name.replace('-', "_"))
    }
}

#[cfg(test)]
mod tests {
    //! Bug #1 + #2: the Rust manifest name/license parsers used to hand-scan
    //! Cargo.toml lines, which misread `name="x"` (no space) and `name = { workspace
    //! = true }` (extracted "{ workspace" as the name). Now go through the real
    //! toml crate — these tests pin both regressions.

    use crate::introspect::manifest::{manifest_license, project_manifest_name, testutil};
    use crate::types::Language;

    #[test]
    fn rust_name_with_no_spaces_around_equals() {
        // name="revtool" — the old `starts_with("name =")` scan missed this.
        let root = testutil::scratch(&[(
            "Cargo.toml",
            "[package]\nname=\"revtool\"\nversion=\"0.1\"\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Rust).as_deref(),
            Some("revtool")
        );
        testutil::cleanup(&root);
    }

    #[test]
    fn rust_name_workspace_inherited_is_none() {
        // name = { workspace = true } — the old extract returned Some("{ workspace"),
        // which coerce_kebab turned into a plugin literally named "workspace".
        let root = testutil::scratch(&[(
            "Cargo.toml",
            "[package]\nname = { workspace = true }\nversion = \"0.1\"\n",
        )]);
        assert_eq!(project_manifest_name(&root, Language::Rust), None);
        testutil::cleanup(&root);
    }

    #[test]
    fn rust_license_with_no_spaces_around_equals() {
        // license="MIT" — same brittle scan hit license= (Bug #1).
        let root =
            testutil::scratch(&[("Cargo.toml", "[package]\nname = \"x\"\nlicense=\"MIT\"\n")]);
        assert_eq!(
            manifest_license(&root, Language::Rust).as_deref(),
            Some("MIT")
        );
        testutil::cleanup(&root);
    }

    #[test]
    fn rust_license_workspace_inherited_is_none() {
        let root = testutil::scratch(&[(
            "Cargo.toml",
            "[package]\nname = \"x\"\nlicense = { workspace = true }\n",
        )]);
        assert_eq!(manifest_license(&root, Language::Rust), None);
        testutil::cleanup(&root);
    }
}
