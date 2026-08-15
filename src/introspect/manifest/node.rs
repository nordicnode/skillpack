//! Node/JavaScript: `package.json`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Node;

impl LanguageSpec for Node {
    fn present(&self, dir: &Path) -> bool {
        dir.join("package.json").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("package.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("name")?
            .as_str()
            .map(std::string::ToString::to_string)
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("package.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("version")?
            .as_str()
            .map(std::string::ToString::to_string)
    }

    fn authors(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("package.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        // package.json "author" is a string or { "name": "..." } object.
        if let Some(a) = v.get("author") {
            if let Some(s) = a.as_str() {
                return Some(s.to_string());
            }
            if let Some(name) = a.get("name").and_then(|n| n.as_str()) {
                return Some(name.to_string());
            }
        }
        None
    }

    fn license(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("package.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v.get("license")?
            .as_str()
            .map(std::string::ToString::to_string)
    }

    fn category_hint(&self) -> &'static str {
        "the JavaScript/Node tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.js".into(),
            "*.ts".into(),
            "*.jsx".into(),
            "*.tsx".into(),
            "package.json".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import {{ … }} from '{name}'")
    }
}
