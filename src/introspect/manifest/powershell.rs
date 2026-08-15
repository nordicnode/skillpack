//! PowerShell: `.ps1` / `.psm1` / `.psd1` — no name-bearing manifest, so the
//! profile falls back to the dir/repo name.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Powershell;

impl LanguageSpec for Powershell {
    /// True when the repo ships PowerShell scripts/modules (`.ps1`/`.psm1`/
    /// `.psd1`) at the root or under `bin`/`src`/`scripts`/`tools`. Same weak
    /// primary-only status as [`super::shell::Shell`].
    fn present(&self, root: &Path) -> bool {
        for sub in ["", "bin", "src", "scripts", "tools"] {
            if let Ok(rd) = fs::read_dir(root.join(sub)) {
                for e in rd.flatten() {
                    if matches!(
                        e.path().extension().and_then(|x| x.to_str()),
                        Some("ps1") | Some("psm1") | Some("psd1")
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn name(&self, _root: &Path) -> Option<String> {
        None
    }

    fn category_hint(&self) -> &'static str {
        "the PowerShell tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.ps1".into(), "*.psm1".into(), "*.psd1".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("Import-Module {name}")
    }
}
