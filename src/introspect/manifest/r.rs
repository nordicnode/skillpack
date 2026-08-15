//! R: `DESCRIPTION` (DCF format).

use std::fs;
use std::path::Path;

use super::{extract_key_colon_value, LanguageSpec};
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

pub(crate) struct R;

impl LanguageSpec for R {
    fn present(&self, dir: &Path) -> bool {
        crate::introspect::cli_probe::root_file_contains(dir, "DESCRIPTION", "Package:")
    }

    fn name(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
        extract_key_colon_value(&raw, "Package")
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
        extract_key_colon_value(&raw, "Version")
    }

    fn authors(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
        extract_key_colon_value(&raw, "Author")
    }

    fn license(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("DESCRIPTION")).ok()?;
        extract_key_colon_value(&raw, "License")
    }

    fn category_hint(&self) -> &'static str {
        "the R tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.R".into(),
            "*.r".into(),
            "DESCRIPTION".into(),
            "NAMESPACE".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("library({name})")
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        // `Rscript <script>` against a conventional entry point (R packages ship
        // CLIs under `inst/` or `exec/`). Requires `Rscript` on PATH; honest
        // `None` otherwise.
        let rscript = which_on_path("Rscript")?.to_string_lossy().to_string();
        for script in &[
            "inst/cli.R".to_string(),
            format!("inst/{name}.R"),
            "exec/cli.R".to_string(),
            "cli.R".to_string(),
            "main.R".to_string(),
        ] {
            if root.join(script).is_file() {
                return Some(CliCandidate {
                    argv: vec![rscript, script.clone()],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{
        manifest_license, project_manifest_authors, project_manifest_name,
        project_manifest_version, testutil,
    };
    use crate::types::Language;

    #[test]
    fn r_description_parses_package_version_license() {
        let root = testutil::scratch(&[(
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
        testutil::cleanup(&root);
    }
}
