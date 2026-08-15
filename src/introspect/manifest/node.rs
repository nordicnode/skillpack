//! Node/JavaScript: `package.json`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

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
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        let node = which_on_path("node")
            .or_else(|| which_on_path("nodejs"))
            .map(|p| p.to_string_lossy().to_string())?;

        // 1. package.json `bin` entry: `"bin": "./bin/run.js"` OR
        //    `"bin": { "cli": "./bin/run.js" }`
        if let Some(script_rel) = package_json_bin_script(root, name) {
            let script = root.join(&script_rel);
            if script.is_file() {
                return Some(CliCandidate {
                    argv: vec![node, script.to_string_lossy().to_string()],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }

        // 2. Conventional bin locations: `./bin/<name>.js`, `./bin/cli.js`, `./cli.js`
        for rel in &[
            format!("bin/{name}.js"),
            "bin/cli.js".to_string(),
            "bin/index.js".to_string(),
            "cli.js".to_string(),
        ] {
            let script = root.join(rel);
            if script.is_file() {
                return Some(CliCandidate {
                    argv: vec![node, script.to_string_lossy().to_string()],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }

        // 3. Global command on PATH (installed via `npm i -g`).
        if let Some(bin) = which_on_path(name) {
            return Some(CliCandidate {
                argv: vec![bin.to_string_lossy().to_string()],
                spawn_cwd: root.to_path_buf(),
            });
        }

        None
    }
}

fn package_json_bin_script(root: &Path, name: &str) -> Option<String> {
    let raw = fs::read_to_string(root.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let bin = v.get("bin")?;
    if let Some(s) = bin.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = bin.as_object() {
        if let Some(s) = obj.get(name).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
        // Fall back to the first key in the bin map.
        if let Some((_k, v)) = obj.iter().next() {
            if let Some(s) = v.as_str() {
                return Some(s.to_string());
            }
        }
    }
    None
}
