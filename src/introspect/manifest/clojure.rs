//! Clojure: `deps.edn` / `project.clj`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Clojure;

impl LanguageSpec for Clojure {
    fn present(&self, dir: &Path) -> bool {
        dir.join("deps.edn").exists() || dir.join("project.clj").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("project.clj")).ok()?;
        extract_clojure_defproject(&raw).map(|(n, _)| n)
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("project.clj")).ok()?;
        extract_clojure_defproject(&raw).map(|(_, v)| v)
    }

    fn category_hint(&self) -> &'static str {
        "the Clojure tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.clj".into(),
            "*.cljs".into(),
            "*.cljc".into(),
            "deps.edn".into(),
            "project.clj".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("(require '[{name} :refer :all])")
    }
}

/// Parse the `(defproject name "version" ...)` head of a Leiningen `project.clj`.
/// Returns `(name, version)`.
fn extract_clojure_defproject(raw: &str) -> Option<(String, String)> {
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

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{project_manifest_name, project_manifest_version, testutil};
    use crate::types::Language;

    #[test]
    fn clojure_project_clj_parses_defproject() {
        let root = testutil::scratch(&[(
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
        testutil::cleanup(&root);
    }
}
