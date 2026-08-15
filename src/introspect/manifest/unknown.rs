//! Unknown: no detection, no manifest fields — the placeholder spec for
//! unrecognized repos. Keeps every `Language` variant registered so the
//! registry match stays exhaustive.

use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Unknown;

impl LanguageSpec for Unknown {
    fn present(&self, _dir: &Path) -> bool {
        false
    }

    fn name(&self, _root: &Path) -> Option<String> {
        None
    }

    fn category_hint(&self) -> &'static str {
        "the tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        Vec::new()
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("(no standard import form for {name}; document it via `skillpack update`)")
    }
}
