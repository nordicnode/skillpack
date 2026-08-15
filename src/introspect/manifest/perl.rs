//! Perl: `META.json` / `Makefile.PL` / `cpanfile`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

pub(crate) struct Perl;

impl LanguageSpec for Perl {
    fn present(&self, dir: &Path) -> bool {
        dir.join("cpanfile").exists()
            || dir.join("Makefile.PL").exists()
            || dir.join("META.json").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        if let Some(n) = meta_json_field(root, "name") {
            return Some(n);
        }
        let raw = fs::read_to_string(root.join("Makefile.PL")).ok()?;
        extract_makefile_pl_field(&raw, "NAME")
    }

    fn version(&self, root: &Path) -> Option<String> {
        if let Some(v) = meta_json_field(root, "version") {
            return Some(v);
        }
        let raw = fs::read_to_string(root.join("Makefile.PL")).ok()?;
        extract_makefile_pl_field(&raw, "VERSION")
    }

    fn authors(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("META.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(a) = v.get("author") {
                    if let Some(s) = a.as_str() {
                        return Some(s.to_string());
                    }
                    if let Some(arr) = a.as_array() {
                        if let Some(first) = arr.first().and_then(|s| s.as_str()) {
                            return Some(first.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    fn license(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("META.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                // CPAN META.json spells the license as a list of SPDX ids.
                if let Some(lic) = v.get("license") {
                    if let Some(s) = lic.as_str() {
                        return Some(s.to_string());
                    }
                    if let Some(arr) = lic.as_array() {
                        if let Some(first) = arr.first().and_then(|s| s.as_str()) {
                            return Some(first.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    fn category_hint(&self) -> &'static str {
        "the Perl tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.pm".into(),
            "*.pl".into(),
            "cpanfile".into(),
            "Makefile.PL".into(),
            "META.json".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("use {name};")
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        // `perl <script>` against a conventional entry point. Requires `perl`
        // on PATH; honest `None` otherwise.
        let perl = which_on_path("perl")?.to_string_lossy().to_string();
        for script in &[
            format!("bin/{name}"),
            format!("bin/{name}.pl"),
            "script/main.pl".to_string(),
            "script/cli.pl".to_string(),
            format!("{name}.pl"),
        ] {
            if root.join(script).is_file() {
                return Some(CliCandidate {
                    argv: vec![perl, script.clone()],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }
        None
    }
}

fn meta_json_field(root: &Path, field: &str) -> Option<String> {
    let raw = fs::read_to_string(root.join("META.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get(field).and_then(|n| n.as_str()).map(|s| s.to_string())
}

/// Extract a `WriteMakefile(NAME => 'Foo-Bar', ...)` key/value pair from a
/// Perl `Makefile.PL`.
fn extract_makefile_pl_field(raw: &str, key: &str) -> Option<String> {
    let needle = format!("{key} =>");
    let line = raw.lines().find(|l| l.contains(&needle))?;
    let after = line.split(&needle).nth(1)?.trim();
    let value = after
        .split([',', ')'])
        .next()?
        .trim()
        .trim_matches(|c: char| c == '\'' || c == '"' || c.is_whitespace());
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{
        manifest_license, project_manifest_authors, project_manifest_name,
        project_manifest_version, testutil,
    };
    use crate::types::Language;

    #[test]
    fn perl_meta_json_parses_name_version_license() {
        let root = testutil::scratch(&[(
            "META.json",
            "{\"name\":\"MyPerlTool\",\"version\":\"0.7.0\",\"license\":[\"perl_5\"],\"author\":[\"L. Wall\"]}\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Perl).as_deref(),
            Some("MyPerlTool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Perl).as_deref(),
            Some("0.7.0")
        );
        assert_eq!(
            manifest_license(&root, Language::Perl).as_deref(),
            Some("perl_5")
        );
        assert_eq!(
            project_manifest_authors(&root, Language::Perl).as_deref(),
            Some("L. Wall")
        );
        testutil::cleanup(&root);
    }
}
