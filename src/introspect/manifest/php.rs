//! PHP: `composer.json`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{canonicalize_for_argv, which_on_path, CliCandidate};

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
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        let php = which_on_path("php")?;
        let php_bin = php.to_string_lossy().to_string();
        let raw = fs::read_to_string(root.join("composer.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let bin = v.get("bin")?;
        // `bin` may be a string ("./bin/cli.php") or an object mapping name → script.
        // Pick the entry keyed by the tool name if present, otherwise the first script.
        let script = match bin {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(map) => map
                .get(name)
                .and_then(|v| v.as_str())
                .or_else(|| map.iter().next().and_then(|(_, v)| v.as_str()))?
                .to_string(),
            // composer.json `bin` may also be an array of paths; pick the first.
            serde_json::Value::Array(arr) => arr.first()?.as_str()?.to_string(),
            _ => return None,
        };
        if script.trim().is_empty() {
            return None;
        }
        // Resolve to an absolute path so `php <abs script> --help` works whether
        // or not the package is installed, and survives the temp-dir spawn cwd.
        let script_path = root.join(&script);
        let abs_script = canonicalize_for_argv(&script_path);
        Some(CliCandidate {
            argv: vec![php_bin, abs_script],
            spawn_cwd: root.to_path_buf(),
        })
    }
}
