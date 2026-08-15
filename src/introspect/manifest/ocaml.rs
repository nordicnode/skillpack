//! OCaml: `*.opam` / `dune-project`.

use std::fs;
use std::path::Path;

use super::{extract_key_colon_value, first_file_with_ext, pascal_name, LanguageSpec};
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

pub(crate) struct Ocaml;

impl LanguageSpec for Ocaml {
    fn present(&self, dir: &Path) -> bool {
        dir.join("dune-project").exists()
            || crate::introspect::cli_probe::has_file_with_ext(dir, "opam")
    }

    fn name(&self, root: &Path) -> Option<String> {
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

    fn version(&self, root: &Path) -> Option<String> {
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

    fn authors(&self, root: &Path) -> Option<String> {
        if let Some(opam) = first_file_with_ext(root, "opam") {
            if let Ok(raw) = fs::read_to_string(&opam) {
                if let Some(a) = extract_key_colon_value(&raw, "authors") {
                    return Some(a);
                }
            }
        }
        None
    }

    fn category_hint(&self) -> &'static str {
        "the OCaml tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.ml".into(),
            "*.mli".into(),
            "dune-project".into(),
            "*.opam".into(),
            "dune".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("open {mod}", mod = pascal_name(name))
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        // `dune exec <name>` from a dune project. Requires `dune` on PATH;
        // honest `None` otherwise.
        if !root.join("dune-project").is_file() && !root.join("dune").is_dir() {
            return None;
        }
        which_on_path("dune").map(|dune| CliCandidate {
            argv: vec![
                dune.to_string_lossy().to_string(),
                "exec".to_string(),
                name.to_string(),
            ],
            spawn_cwd: root.to_path_buf(),
        })
    }
}

/// Extract a `(name <value>)` s-expression field from a `dune-project` file.
fn extract_dune_field(raw: &str, key: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{
        project_manifest_authors, project_manifest_name, project_manifest_version, testutil,
    };
    use crate::types::Language;

    #[test]
    fn ocaml_opam_parses_name_version_authors() {
        let root = testutil::scratch(&[(
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
        testutil::cleanup(&root);
    }
}
