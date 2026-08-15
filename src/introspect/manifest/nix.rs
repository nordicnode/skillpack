//! Nix: `flake.nix` / `shell.nix` / `default.nix`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Nix;

impl LanguageSpec for Nix {
    fn present(&self, dir: &Path) -> bool {
        dir.join("flake.nix").exists()
            || dir.join("shell.nix").exists()
            || dir.join("default.nix").exists()
    }

    // No standard "name" field — the profile falls back to the dir/repo name.
    fn name(&self, _root: &Path) -> Option<String> {
        None
    }

    /// The `description = "..."` scalar from a Nix flake.
    fn description(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("flake.nix")).ok()?;
        extract_flake_description(&raw)
    }

    fn category_hint(&self) -> &'static str {
        "the Nix tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.nix".into(),
            "flake.nix".into(),
            "shell.nix".into(),
            "default.nix".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("{{ inputs, ... }}: inputs.{name}")
    }
}

/// Extract the `description = "..."` scalar from a Nix flake. A Nix attribute
/// value is terminated by a `;` (e.g. `description = "...";`), which must be
/// stripped before unquoting.
fn extract_flake_description(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("description") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                // Strip the trailing `;` (and any whitespace) before unquoting.
                let rest = rest.trim().trim_end_matches(';').trim();
                if let Some(s) = rest
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                {
                    if !s.is_empty() {
                        return Some(s.to_string());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{manifest_description, testutil};
    use crate::types::Language;

    #[test]
    fn nix_flake_description_is_captured() {
        let root = testutil::scratch(&[(
            "flake.nix",
            "{\n  description = \"A reproducible dev environment\";\n  inputs = {};\n}\n",
        )]);
        assert_eq!(
            manifest_description(&root, Language::Nix).as_deref(),
            Some("A reproducible dev environment")
        );
        testutil::cleanup(&root);
    }
}
