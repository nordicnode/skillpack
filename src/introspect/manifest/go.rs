//! Go: `go.mod`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

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
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        let go = which_on_path("go")?.to_string_lossy().to_string();

        // 1. Root directory is `package main`.
        if is_go_main_package(root) {
            return Some(CliCandidate {
                argv: vec![go, "run".to_string(), ".".to_string()],
                spawn_cwd: root.to_path_buf(),
            });
        }

        // 2. `./cmd/<name>` or `./cmd/...` is `package main`.
        let cmd_named = root.join("cmd").join(name);
        if cmd_named.is_dir() && is_go_main_package(&cmd_named) {
            return Some(CliCandidate {
                argv: vec![go, "run".to_string(), format!("./cmd/{name}")],
                spawn_cwd: root.to_path_buf(),
            });
        }
        let cmd_root = root.join("cmd");
        if cmd_root.is_dir() {
            if let Ok(entries) = fs::read_dir(&cmd_root) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() && is_go_main_package(&p) {
                        if let Some(sub) = p.file_name().and_then(|s| s.to_str()) {
                            return Some(CliCandidate {
                                argv: vec![go, "run".to_string(), format!("./cmd/{sub}")],
                                spawn_cwd: root.to_path_buf(),
                            });
                        }
                    }
                }
            }
        }

        // 3. Installed binary on PATH.
        if let Some(bin) = which_on_path(name) {
            return Some(CliCandidate {
                argv: vec![bin.to_string_lossy().to_string()],
                spawn_cwd: root.to_path_buf(),
            });
        }

        None
    }
}

fn is_go_main_package(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "go") {
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            if raw.lines().any(|l| l.trim() == "package main") {
                return true;
            }
        }
    }
    false
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
