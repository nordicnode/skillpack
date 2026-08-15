//! Swift: `Package.swift`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Swift;

impl LanguageSpec for Swift {
    fn present(&self, dir: &Path) -> bool {
        dir.join("Package.swift").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("Package.swift")) {
            if let Some(n) = extract_swift_package_name(&raw) {
                return Some(n);
            }
        }
        None
    }

    fn category_hint(&self) -> &'static str {
        "the Swift tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.swift".into(), "Package.swift".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import {name}")
    }
}

fn extract_swift_package_name(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(i) = trimmed.find("name:") {
            let after = trimmed[i + 5..].trim();
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
    use crate::introspect::manifest::{project_manifest_name, testutil};
    use crate::types::Language;

    #[test]
    fn swift_manifest_parses_package_name() {
        let root = testutil::scratch(&[(
            "Package.swift",
            "// swift-tools-version: 5.9\nimport PackageDescription\nlet package = Package(\n    name: \"SwiftCLI\",\n    products: []\n)\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Swift).as_deref(),
            Some("SwiftCLI")
        );
        testutil::cleanup(&root);
    }
}
