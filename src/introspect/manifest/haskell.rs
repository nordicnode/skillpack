//! Haskell: `*.cabal` / `stack.yaml` / `cabal.project`.

use std::fs;
use std::path::Path;

use super::{extract_key_colon_value, first_file_with_ext, pascal_name, LanguageSpec};

pub(crate) struct Haskell;

impl LanguageSpec for Haskell {
    fn present(&self, dir: &Path) -> bool {
        dir.join("stack.yaml").exists()
            || dir.join("cabal.project").exists()
            || crate::introspect::cli_probe::has_cabal_file(dir)
    }

    fn name(&self, root: &Path) -> Option<String> {
        let cabal = first_file_with_ext(root, "cabal")?;
        let raw = fs::read_to_string(&cabal).ok()?;
        extract_key_colon_value(&raw, "name")
    }

    fn version(&self, root: &Path) -> Option<String> {
        let cabal = first_file_with_ext(root, "cabal")?;
        let raw = fs::read_to_string(&cabal).ok()?;
        extract_key_colon_value(&raw, "version")
    }

    fn category_hint(&self) -> &'static str {
        "the Haskell tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.hs".into(),
            "*.lhs".into(),
            "*.cabal".into(),
            "cabal.project".into(),
            "stack.yaml".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import {mod}", mod = pascal_name(name))
    }
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{project_manifest_name, project_manifest_version, testutil};
    use crate::types::Language;

    #[test]
    fn haskell_cabal_parses_name_and_version() {
        let root = testutil::scratch(&[(
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
        testutil::cleanup(&root);
    }
}
