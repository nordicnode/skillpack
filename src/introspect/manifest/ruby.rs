//! Ruby: `*.gemspec` (+ `Gemfile` detection signal).

use std::fs;
use std::path::Path;

use crate::introspect::cli_probe::has_gemspec;

use super::LanguageSpec;
use crate::introspect::cli_candidates::{canonicalize_for_argv, which_on_path, CliCandidate};

pub(crate) struct Ruby;

impl LanguageSpec for Ruby {
    fn present(&self, dir: &Path) -> bool {
        dir.join("Gemfile").exists() || has_gemspec(dir)
    }

    fn name(&self, root: &Path) -> Option<String> {
        gemspec_field(root, &["spec.name", ".name ="])
    }

    fn version(&self, root: &Path) -> Option<String> {
        gemspec_field(root, &["spec.version", ".version ="])
    }

    fn authors(&self, root: &Path) -> Option<String> {
        gemspec_field(root, &["spec.author", ".author ="])
    }

    fn license(&self, root: &Path) -> Option<String> {
        gemspec_field(root, &["spec.license", ".license ="])
    }

    fn category_hint(&self) -> &'static str {
        "the Ruby tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.rb".into(), "*.gemspec".into(), "Gemfile".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("require '{name}'")
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        let ruby = which_on_path("ruby")
            .or_else(|| which_on_path("bundle"))
            .map(|b| b.to_string_lossy().to_string())?;
        for dir in &["exe", "bin"] {
            let p = root.join(dir).join(name);
            if p.is_file() {
                let abs = canonicalize_for_argv(&p);
                return Some(CliCandidate {
                    argv: vec![ruby.clone(), abs],
                    spawn_cwd: root.to_path_buf(),
                });
            }
        }
        None
    }
}

/// Scan the root for a `*.gemspec` and pull the first line carrying one of
/// `needles` (e.g. `spec.name = "..."`), extracting the string value.
fn gemspec_field(root: &Path, needles: &[&str]) -> Option<String> {
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("gemspec") {
                if let Ok(raw) = fs::read_to_string(&p) {
                    if let Some(line) = raw.lines().find(|l| needles.iter().any(|n| l.contains(n)))
                    {
                        if let Some(value) = extract_ruby_string_value(line) {
                            return Some(value);
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_ruby_string_value(line: &str) -> Option<String> {
    let after = line.split('=').nth(1)?.trim();
    let s = after.trim_start_matches(['"', '\'']);
    let s = s.split(['"', '\'']).next()?.trim();
    Some(s.to_string())
}
