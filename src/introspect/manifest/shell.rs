//! Shell: shebang'd `*.sh` scripts — no name-bearing manifest, so the profile
//! falls back to the dir/repo name.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Shell;

impl LanguageSpec for Shell {
    /// True when the repo's primary surface is shell scripting: a shebang'd
    /// `*.sh` at the root (excluding the ubiquitous install/setup/configure
    /// helpers that tag along with non-shell projects) or under `bin/`/
    /// `scripts/`/`src/`. Weak but sufficient — this branch only fires when
    /// NO other language manifest exists, and `detect_all_languages` never
    /// mints a shell secondary.
    fn present(&self, root: &Path) -> bool {
        if let Ok(rd) = fs::read_dir(root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("sh") {
                    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if !matches!(stem, "install" | "setup" | "configure") && has_shell_shebang(&p) {
                        return true;
                    }
                }
            }
        }
        for sub in ["bin", "scripts", "src"] {
            if let Ok(rd) = fs::read_dir(root.join(sub)) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("sh") && has_shell_shebang(&p)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    // No name/version/authors/license/description — the manifest-less
    // fallbacks apply.
    fn name(&self, _root: &Path) -> Option<String> {
        None
    }

    fn category_hint(&self) -> &'static str {
        "the shell tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.sh".into(), "*.bash".into(), "*.zsh".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("source ./{name}.sh")
    }
}

fn has_shell_shebang(p: &Path) -> bool {
    fs::read_to_string(p)
        .ok()
        .and_then(|c| c.lines().next().map(str::to_string))
        .is_some_and(|l| {
            l.starts_with("#!") && (l.contains("bash") || l.contains("/sh") || l.contains("zsh"))
        })
}
