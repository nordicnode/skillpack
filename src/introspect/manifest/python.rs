//! Python: `pyproject.toml` / `setup.py` / `setup.cfg`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

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
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        let python = which_on_path("python")
            .or_else(|| which_on_path("python3"))
            .map(|p| p.to_string_lossy().to_string())?;

        // A `pyproject.toml` `[project.scripts]` entry maps the console-script
        // name to `<pkg>.<module>:<func>`. We extract the package and, if it's
        // importable as a directory at root or under src/, invoke `python -m <pkg>`.
        if let Some(pkg) = python_script_package(root, name) {
            if root.join(&pkg).is_dir() {
                return Some(CliCandidate {
                    argv: vec![python, "-m".to_string(), pkg],
                    spawn_cwd: root.to_path_buf(),
                });
            }
            if root.join("src").join(&pkg).is_dir() {
                return Some(CliCandidate {
                    argv: vec![python, "-m".to_string(), pkg],
                    spawn_cwd: root.join("src"),
                });
            }
        }

        // Installed console script on PATH (e.g. `pip install -e .` already run).
        if let Some(script) = which_on_path(name) {
            return Some(CliCandidate {
                argv: vec![script.to_string_lossy().to_string()],
                spawn_cwd: root.to_path_buf(),
            });
        }

        None
    }
}

/// Extract the top-level package name from a `pyproject.toml` `[project.scripts]`
/// or `[tool.poetry.scripts]` entry whose key matches `name` (e.g.
/// `sample-python = "sample_python.cli:main"` → `sample_python`).
/// Returns `None` if no such entry / no importable target.
fn python_script_package(root: &Path, name: &str) -> Option<String> {
    let raw = fs::read_to_string(root.join("pyproject.toml")).ok()?;
    let v: toml::Value = toml::from_str(&raw).ok()?;
    let scripts = v
        .get("project")
        .and_then(|p| p.get("scripts"))
        .and_then(|s| s.as_table())
        .or_else(|| {
            v.get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("scripts"))
                .and_then(|s| s.as_table())
        })?;
    let target = scripts.get(name)?.as_str()?;
    // target is "<pkg>.<module>:<func>" — take the segment before the colon,
    // then the top-level package segment before the first dot.
    let before_colon = target.split(':').next()?;
    let top_pkg = before_colon.split('.').next()?;
    Some(top_pkg.to_string())
}
