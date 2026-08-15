//! Zig: `build.zig` / `build.zig.zon`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{canonicalize_for_argv, which_on_path, CliCandidate};

pub(crate) struct Zig;

impl LanguageSpec for Zig {
    fn present(&self, dir: &Path) -> bool {
        dir.join("build.zig").exists() || dir.join("build.zig.zon").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("build.zig.zon")) {
            if let Some(n) = extract_zig_zon_field(&raw, "name") {
                return Some(n);
            }
        }
        None
    }

    fn version(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("build.zig.zon")) {
            if let Some(v) = extract_zig_zon_field(&raw, "version") {
                return Some(v);
            }
        }
        None
    }

    fn license(&self, root: &Path) -> Option<String> {
        if let Ok(raw) = fs::read_to_string(root.join("build.zig.zon")) {
            if let Some(lic) = extract_zig_zon_field(&raw, "license") {
                return Some(lic);
            }
        }
        None
    }

    fn category_hint(&self) -> &'static str {
        "the Zig tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.zig".into(), "build.zig".into(), "build.zig.zon".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("const {name} = @import(\"{name}\");")
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        for dir in &["zig-out/bin", "bin"] {
            let bin = root.join(dir).join(format!("{name}{suffix}"));
            if bin.is_file() {
                let canon = canonicalize_for_argv(&bin);
                return Some(CliCandidate {
                    argv: vec![canon],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }
        which_on_path(name).map(|p| CliCandidate {
            argv: vec![p.to_string_lossy().to_string()],
            spawn_cwd: root.to_path_buf(),
        })
    }
}

fn extract_zig_zon_field(raw: &str, field: &str) -> Option<String> {
    let dot_field = format!(".{field}");
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&dot_field) || trimmed.starts_with(field) {
            if let Some(eq) = trimmed.find('=') {
                let val = trimmed[eq + 1..].trim();
                let clean = val.trim_matches(|c: char| {
                    c == ',' || c == '"' || c == '\'' || c == '.' || c.is_whitespace()
                });
                if !clean.is_empty() {
                    return Some(clean.to_string());
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
    fn zig_manifest_parses_name_and_version() {
        let root = testutil::scratch(&[(
            "build.zig.zon",
            ".{\n    .name = \"zig-frob\",\n    .version = \"0.4.2\",\n    .paths = .{\"\"},\n}\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Zig).as_deref(),
            Some("zig-frob")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Zig).as_deref(),
            Some("0.4.2")
        );
        testutil::cleanup(&root);
    }
}
