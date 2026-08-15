//! Haskell: `*.cabal` / `stack.yaml` / `cabal.project`.

use std::fs;
use std::path::Path;

use super::{extract_key_colon_value, first_file_with_ext, pascal_name, LanguageSpec};
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

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
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        // `stack run <name> --` (when a `stack.yaml` exists) else
        // `cabal run <name> --`. The trailing `--` separates the build tool's
        // own flags from the program's argv so the appended `--help` reaches
        // the executable. Requires the runtime on PATH (honest `None` otherwise).
        if root.join("stack.yaml").exists() {
            if let Some(stack) = which_on_path("stack") {
                return Some(CliCandidate {
                    argv: vec![
                        stack.to_string_lossy().to_string(),
                        "run".to_string(),
                        name.to_string(),
                        "--".to_string(),
                    ],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }
        if let Some(cabal) = which_on_path("cabal") {
            return Some(CliCandidate {
                argv: vec![
                    cabal.to_string_lossy().to_string(),
                    "run".to_string(),
                    name.to_string(),
                    "--".to_string(),
                ],
                spawn_cwd: root.to_path_buf(),
            });
        }
        None
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
