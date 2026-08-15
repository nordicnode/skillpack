//! Python: `pyproject.toml` / `setup.py` / `setup.cfg`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Python;

impl LanguageSpec for Python {
    fn present(&self, dir: &Path) -> bool {
        dir.join("pyproject.toml").exists()
            || dir.join("setup.py").exists()
            || dir.join("setup.cfg").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        // pyproject.toml [project] name = "...", fallback [tool.poetry] name = "..."
        if let Ok(raw) = fs::read_to_string(root.join("pyproject.toml")) {
            if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
                if let Some(name) = v
                    .get("project")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                {
                    return Some(name.to_string());
                }
                if let Some(name) = v
                    .get("tool")
                    .and_then(|t| t.get("poetry"))
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                {
                    return Some(name.to_string());
                }
                if let Some(name) = v
                    .get("tool")
                    .and_then(|t| t.get("flit"))
                    .and_then(|f| f.get("metadata"))
                    .and_then(|m| m.get("module"))
                    .and_then(|n| n.as_str())
                {
                    return Some(name.to_string());
                }
            }
        }
        None
    }

    fn version(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("pyproject.toml")) {
            if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
                if let Some(ver) = v
                    .get("project")
                    .and_then(|p| p.get("version"))
                    .and_then(|n| n.as_str())
                {
                    return Some(ver.to_string());
                }
                if let Some(ver) = v
                    .get("tool")
                    .and_then(|t| t.get("poetry"))
                    .and_then(|p| p.get("version"))
                    .and_then(|n| n.as_str())
                {
                    return Some(ver.to_string());
                }
                if let Some(ver) = v
                    .get("tool")
                    .and_then(|t| t.get("flit"))
                    .and_then(|f| f.get("metadata"))
                    .and_then(|m| m.get("version"))
                    .and_then(|n| n.as_str())
                {
                    return Some(ver.to_string());
                }
            }
        }
        None
    }

    fn authors(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("pyproject.toml")) {
            if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
                // PEP 621: [project.authors] = [{ name = "..." }]
                if let Some(arr) = v
                    .get("project")
                    .and_then(|p| p.get("authors"))
                    .and_then(|a| a.as_array())
                {
                    if let Some(first) = arr.first() {
                        if let Some(name) = first.get("name").and_then(|n| n.as_str()) {
                            return Some(name.to_string());
                        }
                    }
                }
                // Poetry: [tool.poetry.authors] = ["Name <email>"]
                if let Some(arr) = v
                    .get("tool")
                    .and_then(|t| t.get("poetry"))
                    .and_then(|p| p.get("authors"))
                    .and_then(|a| a.as_array())
                {
                    if let Some(first) = arr.first().and_then(|s| s.as_str()) {
                        return Some(first.to_string());
                    }
                }
            }
        }
        None
    }

    fn license(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("pyproject.toml")).ok()?;
        let v: toml::Value = toml::from_str(&raw).ok()?;
        // PEP 621: [project] license = "MIT" or license = { text = "MIT" }
        if let Some(lic) = v.get("project").and_then(|p| p.get("license")) {
            if let Some(s) = lic.as_str() {
                return Some(s.to_string());
            }
            if let Some(text) = lic.get("text").and_then(|t| t.as_str()) {
                return Some(text.to_string());
            }
        }
        // Poetry: [tool.poetry] license = "MIT"
        v.get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("license"))
            .and_then(|l| l.as_str())
            .map(|s| s.to_string())
    }

    fn category_hint(&self) -> &'static str {
        "the Python tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.py".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import {module}", module = name.replace('-', "_"))
    }
}
