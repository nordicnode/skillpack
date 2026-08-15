//! Elixir: `mix.exs`.

use std::fs;
use std::path::Path;

use super::{pascal_name, LanguageSpec};

pub(crate) struct Elixir;

impl LanguageSpec for Elixir {
    fn present(&self, dir: &Path) -> bool {
        dir.join("mix.exs").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("mix.exs")) {
            if let Some(n) = extract_elixir_app_name(&raw) {
                return Some(n);
            }
        }
        None
    }

    fn version(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("mix.exs")) {
            if let Some(v) = extract_elixir_version(&raw) {
                return Some(v);
            }
        }
        None
    }

    fn category_hint(&self) -> &'static str {
        "the Elixir tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.ex".into(), "*.exs".into(), "mix.exs".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import {mod}", mod = pascal_name(name))
    }
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
    use crate::introspect::manifest::{project_manifest_name, project_manifest_version, testutil};
    use crate::types::Language;

    #[test]
    fn elixir_manifest_parses_name_and_version() {
        let root = testutil::scratch(&[(
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
        testutil::cleanup(&root);
    }
}
