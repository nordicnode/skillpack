//! Julia: `Project.toml`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Julia;

impl LanguageSpec for Julia {
    fn present(&self, dir: &Path) -> bool {
        dir.join("Project.toml").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Project.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Project.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("version")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    fn authors(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Project.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("authors")
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
    }

    fn category_hint(&self) -> &'static str {
        "the Julia tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.jl".into(), "Project.toml".into(), "Manifest.toml".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("using {name}")
    }
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{
        project_manifest_authors, project_manifest_name, project_manifest_version, testutil,
    };
    use crate::types::Language;

    #[test]
    fn julia_project_toml_parses_name_version_and_authors() {
        let root = testutil::scratch(&[(
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
        testutil::cleanup(&root);
    }
}
