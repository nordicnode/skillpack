//! Crystal: `shard.yml`.

use std::fs;
use std::path::Path;

use super::{extract_yaml_scalar, LanguageSpec};

pub(crate) struct Crystal;

impl LanguageSpec for Crystal {
    fn present(&self, dir: &Path) -> bool {
        dir.join("shard.yml").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
        extract_yaml_scalar(&raw, "name")
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
        extract_yaml_scalar(&raw, "version")
    }

    fn authors(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
        extract_yaml_list_first(&raw, "authors")
    }

    fn license(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
        extract_yaml_scalar(&raw, "license")
    }

    fn description(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("shard.yml")).ok()?;
        extract_yaml_scalar(&raw, "description")
    }

    fn category_hint(&self) -> &'static str {
        "the Crystal tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.cr".into(), "shard.yml".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("require \"./{name}\"")
    }
}

/// First entry of a top-level `key:` YAML list (e.g. Crystal `authors:`).
/// Handles inline flow lists (`authors: [A, B]`) and the first indented
/// `- item` of a block list. Returns the bare item with quotes/dashes trimmed.
fn extract_yaml_list_first(raw: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let mut lines = raw.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with(&prefix) {
            continue;
        }
        let value = line[prefix.len()..].trim();
        // Inline flow list: authors: ["A <a@b>", "B"]
        if let Some(rest) = value.strip_prefix('[') {
            let first = rest
                .split([',', ']'])
                .next()?
                .trim()
                .trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
        // Block list: the next indented `- item` line(s).
        for next in lines.by_ref() {
            let t = next.trim();
            if let Some(item) = t.strip_prefix('-') {
                let first = item.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
                if !first.is_empty() {
                    return Some(first.to_string());
                }
                return None;
            }
            if !t.is_empty() && !next.starts_with(char::is_whitespace) {
                break;
            }
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{
        manifest_license, project_manifest_authors, project_manifest_name,
        project_manifest_version, testutil,
    };
    use crate::types::Language;

    #[test]
    fn crystal_shard_yml_parses_name_version_and_license() {
        let root = testutil::scratch(&[(
            "shard.yml",
            "name: mycrystal\nversion: 1.2.0\nlicense: MIT\nauthors:\n  - Grace Hopper <grace@x.io>\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Crystal).as_deref(),
            Some("mycrystal")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Crystal).as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            manifest_license(&root, Language::Crystal).as_deref(),
            Some("MIT")
        );
        assert_eq!(
            project_manifest_authors(&root, Language::Crystal).as_deref(),
            Some("Grace Hopper")
        );
        testutil::cleanup(&root);
    }
}
