//! Dart: `pubspec.yaml`.

use std::fs;
use std::path::Path;

use super::{extract_yaml_scalar, LanguageSpec};
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

pub(crate) struct Dart;

impl LanguageSpec for Dart {
    fn present(&self, dir: &Path) -> bool {
        dir.join("pubspec.yaml").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
        extract_yaml_scalar(&raw, "name")
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
        extract_yaml_scalar(&raw, "version")
    }

    fn description(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
        extract_yaml_scalar(&raw, "description")
    }

    fn category_hint(&self) -> &'static str {
        "the Dart/Flutter tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.dart".into(),
            "pubspec.yaml".into(),
            "analysis_options.yaml".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import 'package:{name}/{name}.dart';")
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        // `dart run <entry>` from the project root (the canonical uninstalled
        // invocation). Prefers a concrete `bin/<name>.dart` / `bin/main.dart` /
        // `bin/cli.dart` entry point, then falls back to `dart run <name>`
        // (resolves a `pubspec.yaml` `executables` entry or the default
        // executable). Requires `dart` on PATH (honest `None` otherwise).
        which_on_path("dart")?;
        for script in &[
            format!("bin/{name}.dart"),
            "bin/main.dart".to_string(),
            "bin/cli.dart".to_string(),
        ] {
            if root.join(script).is_file() {
                return Some(CliCandidate {
                    argv: vec!["dart".to_string(), "run".to_string(), script.clone()],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }
        Some(CliCandidate {
            argv: vec!["dart".to_string(), "run".to_string(), name.to_string()],
            spawn_cwd: root.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{
        manifest_description, project_manifest_name, project_manifest_version, testutil,
    };
    use crate::types::Language;

    #[test]
    fn dart_pubspec_parses_name_version_and_description() {
        let root = testutil::scratch(&[(
            "pubspec.yaml",
            "name: my_dart_tool\nversion: 2.1.0\ndescription: A Dart CLI.\nenvironment:\n  sdk: '>=3.0.0 <4.0.0'\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Dart).as_deref(),
            Some("my_dart_tool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Dart).as_deref(),
            Some("2.1.0")
        );
        assert_eq!(
            manifest_description(&root, Language::Dart).as_deref(),
            Some("A Dart CLI.")
        );
        testutil::cleanup(&root);
    }
}
