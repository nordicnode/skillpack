//! Go: `go.mod`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Go;

impl LanguageSpec for Go {
    fn present(&self, dir: &Path) -> bool {
        dir.join("go.mod").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        // Go: derive a name from the module path's last segment.
        let raw = fs::read_to_string(root.join("go.mod")).ok()?;
        let module_line = raw
            .lines()
            .find(|l| l.trim_start().starts_with("module "))?;
        let last = module_line
            .trim()
            .strip_prefix("module ")
            // Take only the first whitespace-delimited token so a trailing
            // `// ...` line comment cannot bleed into the module path
            // (e.g. `module github.com/foo/bar // bar tool` → "bar").
            .map(|s| s.split_whitespace().next().unwrap_or("").to_string())?
            .rsplit('/')
            .next()?
            .to_string();
        Some(last)
    }

    // `go.mod` has no version field — versioning is via Git tags or a
    // separately-versioned file, so `version` stays at the trait default None.

    fn category_hint(&self) -> &'static str {
        "the Go tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.go".into(), "go.mod".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import \"{name}\"")
    }
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{project_manifest_name, testutil};
    use crate::types::Language;

    // go.mod `module` line may carry a trailing `// ...` comment. The old
    // parser only trimmed outer whitespace, so the comment bled into the
    // path and the last `/`-segment became a comment fragment (e.g.
    // `github.com/foo/bar // bar tool` → "tool" or worse). Now the first
    // whitespace token is taken before splitting, so the name is "bar".
    #[test]
    fn go_module_name_strips_trailing_line_comment() {
        let root = testutil::scratch(&[(
            "go.mod",
            "module github.com/acme/widget // widget CLI\n\ngo 1.21\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Go).as_deref(),
            Some("widget")
        );
        testutil::cleanup(&root);
    }
}
