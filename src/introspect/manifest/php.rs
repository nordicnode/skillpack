//! PHP: `composer.json`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Php;

impl LanguageSpec for Php {
    fn present(&self, dir: &Path) -> bool {
        dir.join("composer.json").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("composer.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("name")?
            .as_str()
            .map(std::string::ToString::to_string)
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("composer.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("version")?
            .as_str()
            .map(std::string::ToString::to_string)
    }

    fn authors(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("composer.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        // composer.json "authors" is [{"name": "...", "email": "..."}]
        v.get("authors")
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|e| {
                e.get("name")
                    .and_then(|n| n.as_str())
                    .or_else(|| e.as_str())
            })
            .map(|s| s.to_string())
    }

    fn license(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("composer.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("license")?
            .as_str()
            .map(std::string::ToString::to_string)
    }

    fn category_hint(&self) -> &'static str {
        "the PHP tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.php".into(), "composer.json".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("require '{name}'")
    }
}
