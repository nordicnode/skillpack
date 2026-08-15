//! C/C++: `CMakeLists.txt` / `meson.build`.
//!
//! Note: `present` deliberately omits the bare `Makefile` signal — a
//! Makefile-only repo may be any ecosystem, so the primary-detection chain in
//! `introspect.rs` honors Makefile separately (with a doctor note), while the
//! nested polyglot walk and the per-language spec do not.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct CCpp;

impl LanguageSpec for CCpp {
    fn present(&self, dir: &Path) -> bool {
        dir.join("CMakeLists.txt").exists() || dir.join("meson.build").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("CMakeLists.txt")) {
            if let Some(n) = extract_cmake_project_name(&raw) {
                return Some(n);
            }
        }
        if let Ok(raw) = fs::read_to_string(root.join("meson.build")) {
            if let Some(n) = extract_meson_project_name(&raw) {
                return Some(n);
            }
        }
        None
    }

    fn version(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("CMakeLists.txt")) {
            if let Some(v) = extract_cmake_project_version(&raw) {
                return Some(v);
            }
        }
        if let Ok(raw) = fs::read_to_string(root.join("meson.build")) {
            if let Some(v) = extract_meson_project_version(&raw) {
                return Some(v);
            }
        }
        None
    }

    fn category_hint(&self) -> &'static str {
        "the C/C++ tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.c".into(),
            "*.cpp".into(),
            "*.cc".into(),
            "*.h".into(),
            "*.hpp".into(),
            "CMakeLists.txt".into(),
            "Makefile".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("#include <{name}.h>")
    }
}

fn extract_cmake_project_name(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.to_ascii_lowercase().find("project(") {
            let inside = &trimmed[i + 8..];
            if let Some(end) = inside.find(')') {
                let inner = inside[..end].trim();
                let first_tok = inner.split_whitespace().next()?;
                let clean = first_tok.trim_matches(|c| c == '"' || c == '\'');
                if !clean.is_empty() {
                    return Some(clean.to_string());
                }
            }
        }
    }
    None
}

fn extract_cmake_project_version(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.to_ascii_lowercase().find("project(") {
            let inside = &trimmed[i + 8..];
            let upper = inside.to_uppercase();
            if let Some(v_idx) = upper.find("VERSION") {
                let after = &inside[v_idx + 7..].trim_start();
                let tok = after
                    .split(|c: char| c.is_whitespace() || c == ')' || c == '"' || c == '\'')
                    .next()?;
                if !tok.is_empty() && tok.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Some(tok.to_string());
                }
            }
        }
    }
    None
}

fn extract_meson_project_name(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(inside) = trimmed.strip_prefix("project(") {
            if let Some(comma_or_end) = inside.find([',', ')']) {
                let first = &inside[..comma_or_end].trim();
                let clean = first.trim_matches(|c| c == '"' || c == '\'');
                if !clean.is_empty() {
                    return Some(clean.to_string());
                }
            }
        }
    }
    None
}

fn extract_meson_project_version(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.find("version:") {
            let after = trimmed[i + 8..].trim();
            let clean = after
                .trim_matches(|c: char| c == ',' || c == '"' || c == '\'' || c.is_whitespace());
            if !clean.is_empty() {
                return Some(clean.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{project_manifest_name, project_manifest_version, testutil};
    use crate::types::Language;

    #[test]
    fn cmake_manifest_parses_name_and_version() {
        let root = testutil::scratch(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(SuperEngine VERSION 2.1.0 LANGUAGES CXX)\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::CCpp).as_deref(),
            Some("SuperEngine")
        );
        assert_eq!(
            project_manifest_version(&root, Language::CCpp).as_deref(),
            Some("2.1.0")
        );
        testutil::cleanup(&root);
    }
}
