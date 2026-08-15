//! Rust: `Cargo.toml`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{canonicalize_for_argv, which_on_path, CliCandidate};

pub(crate) struct Rust;

impl LanguageSpec for Rust {
    fn present(&self, dir: &Path) -> bool {
        dir.join("Cargo.toml").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        // Parse Cargo.toml with the real toml crate (same path as Python)
        // instead of hand-rolling line scans: a hand-scan misreads `name="x"`
        // (no space before `=`) and `name = { workspace = true }` (extracts
        // "{ workspace" as the name). toml does both correctly, and returns
        // None for workspace-inherited names so the caller falls through.
        let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    fn version(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("package")
            .and_then(|p| p.get("version"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                v.get("workspace")
                    .and_then(|w| w.get("package"))
                    .and_then(|p| p.get("version"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
    }

    fn authors(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("package")
            .and_then(|p| p.get("authors"))
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
    }

    fn license(&self, root: &Path) -> Option<String> {
        let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
        let v = toml::from_str::<toml::Value>(&raw).ok()?;
        v.get("package")
            .and_then(|p| p.get("license"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    fn category_hint(&self) -> &'static str {
        "the Rust tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.rs".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("use {crate_name}::…;", crate_name = name.replace('-', "_"))
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let mut candidates: Vec<String> = cargo_bin_names(root);
        if !candidates.iter().any(|c| c == name) {
            candidates.push(name.to_string());
        }
        let probe_names: Vec<String> = candidates
            .into_iter()
            .map(|c| format!("{c}{suffix}"))
            .collect();

        // Check target/ under root, and also under ancestor directories (for
        // Cargo workspace members).
        let mut search_roots = vec![root.to_path_buf()];
        if let Some(parent) = root.parent() {
            search_roots.push(parent.to_path_buf());
            if let Some(grandparent) = parent.parent() {
                search_roots.push(grandparent.to_path_buf());
            }
        }

        for s_root in &search_roots {
            for dir in &["target/release", "target/debug"] {
                for probe in &probe_names {
                    let candidate = s_root.join(dir).join(probe);
                    if candidate.is_file() {
                        let canon = canonicalize_for_argv(&candidate);
                        return Some(CliCandidate {
                            argv: vec![canon],
                            spawn_cwd: root.to_path_buf(),
                        });
                    }
                }
            }
        }

        // Installed bin on PATH.
        if let Some(bin) = which_on_path(name) {
            return Some(CliCandidate {
                argv: vec![bin.to_string_lossy().to_string()],
                spawn_cwd: root.to_path_buf(),
            });
        }

        None
    }
}

/// Parse `[[bin]].name` entries from `Cargo.toml`. Returns bin names in
/// declaration order; empty when no `[[bin]]` tables (implicit single-bin
/// crate where the artifact matches the package name).
fn cargo_bin_names(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(raw) = fs::read_to_string(root.join("Cargo.toml")) {
        if let Ok(v) = toml::from_str::<toml::Value>(&raw) {
            if let Some(arr) = v.get("bin").and_then(|b| b.as_array()) {
                for t in arr {
                    if let Some(n) = t.get("name").and_then(|n| n.as_str()) {
                        names.push(n.to_string());
                    }
                }
            }
        }
    }
    // Probe src/bin/*.rs for implicit Cargo binary targets
    let bin_dir = root.join("src").join("bin");
    if bin_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(bin_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|e| e == "rs") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !names.contains(&stem.to_string()) {
                            names.push(stem.to_string());
                        }
                    }
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    //! Bug #1 + #2: the Rust manifest name/license parsers used to hand-scan
    //! Cargo.toml lines, which misread `name="x"` (no space) and `name = { workspace
    //! = true }` (extracted "{ workspace" as the name). Now go through the real
    //! toml crate — these tests pin both regressions.

    use crate::introspect::manifest::{manifest_license, project_manifest_name, testutil};
    use crate::types::Language;

    #[test]
    fn rust_name_with_no_spaces_around_equals() {
        // name="revtool" — the old `starts_with("name =")` scan missed this.
        let root = testutil::scratch(&[(
            "Cargo.toml",
            "[package]\nname=\"revtool\"\nversion=\"0.1\"\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Rust).as_deref(),
            Some("revtool")
        );
        testutil::cleanup(&root);
    }

    #[test]
    fn rust_name_workspace_inherited_is_none() {
        // name = { workspace = true } — the old extract returned Some("{ workspace"),
        // which coerce_kebab turned into a plugin literally named "workspace".
        let root = testutil::scratch(&[(
            "Cargo.toml",
            "[package]\nname = { workspace = true }\nversion = \"0.1\"\n",
        )]);
        assert_eq!(project_manifest_name(&root, Language::Rust), None);
        testutil::cleanup(&root);
    }

    #[test]
    fn rust_license_with_no_spaces_around_equals() {
        // license="MIT" — same brittle scan hit license= (Bug #1).
        let root =
            testutil::scratch(&[("Cargo.toml", "[package]\nname = \"x\"\nlicense=\"MIT\"\n")]);
        assert_eq!(
            manifest_license(&root, Language::Rust).as_deref(),
            Some("MIT")
        );
        testutil::cleanup(&root);
    }

    #[test]
    fn rust_license_workspace_inherited_is_none() {
        let root = testutil::scratch(&[(
            "Cargo.toml",
            "[package]\nname = \"x\"\nlicense = { workspace = true }\n",
        )]);
        assert_eq!(manifest_license(&root, Language::Rust), None);
        testutil::cleanup(&root);
    }
}
