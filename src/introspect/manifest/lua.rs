//! Lua: `*.rockspec`.

use std::fs;
use std::path::Path;

use super::{first_file_with_ext, LanguageSpec};

pub(crate) struct Lua;

impl LanguageSpec for Lua {
    fn present(&self, dir: &Path) -> bool {
        crate::introspect::cli_probe::has_file_with_ext(dir, "rockspec")
    }

    fn name(&self, root: &Path) -> Option<String> {
        let rockspec = first_file_with_ext(root, "rockspec")?;
        let raw = fs::read_to_string(&rockspec).ok()?;
        extract_rockspec_field(&raw, "package")
    }

    fn version(&self, root: &Path) -> Option<String> {
        let rockspec = first_file_with_ext(root, "rockspec")?;
        let raw = fs::read_to_string(&rockspec).ok()?;
        extract_rockspec_field(&raw, "version")
    }

    fn category_hint(&self) -> &'static str {
        "the Lua tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.lua".into(), "*.rockspec".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("require(\"{name}\")")
    }
}

/// Extract a `field = "value"` scalar from a Lua rockspec (Lua table syntax).
fn extract_rockspec_field(raw: &str, field: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(field) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
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
    use crate::introspect::manifest::{project_manifest_name, project_manifest_version, testutil};
    use crate::types::Language;

    #[test]
    fn lua_rockspec_parses_package_and_version() {
        let root = testutil::scratch(&[(
            "mylua-1.0-1.rockspec",
            "package = \"mylua\"\nversion = \"1.0-1\"\ndescription = { summary = \"x\" }\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Lua).as_deref(),
            Some("mylua")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Lua).as_deref(),
            Some("1.0-1")
        );
        testutil::cleanup(&root);
    }
}
