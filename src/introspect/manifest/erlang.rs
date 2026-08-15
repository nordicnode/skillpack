//! Erlang: `rebar.config` (detection) + `*.app.src` (fields).

use std::fs;
use std::path::Path;

use super::{first_file_ending_with, LanguageSpec};

pub(crate) struct Erlang;

impl LanguageSpec for Erlang {
    fn present(&self, dir: &Path) -> bool {
        dir.join("rebar.config").exists()
            || crate::introspect::cli_probe::has_file_ending_with(dir, ".app.src")
    }

    fn name(&self, root: &Path) -> Option<String> {
        let app_src = first_file_ending_with(root, ".app.src")?;
        let raw = fs::read_to_string(&app_src).ok()?;
        extract_app_src_name(&raw)
    }

    fn version(&self, root: &Path) -> Option<String> {
        let app_src = first_file_ending_with(root, ".app.src")?;
        let raw = fs::read_to_string(&app_src).ok()?;
        extract_app_src_vsn(&raw)
    }

    fn category_hint(&self) -> &'static str {
        "the Erlang/OTP tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.erl".into(),
            "*.hrl".into(),
            "*.app.src".into(),
            "rebar.config".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("application:ensure_all_started({name}).")
    }
}

/// Extract the application name from an Erlang `.app.src`
/// `{application, my_app, [...]}` term.
fn extract_app_src_name(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .find(|l| l.trim().starts_with("{application,"))?;
    let after = line.trim().strip_prefix("{application,")?;
    let name = after.split(',').next()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Extract the `vsn` value from an Erlang `.app.src` term.
fn extract_app_src_vsn(raw: &str) -> Option<String> {
    let line = raw.lines().find(|l| {
        let t = l.trim();
        t.starts_with("vsn") || t.starts_with("{vsn")
    })?;
    let t = line.trim().trim_start_matches('{');
    let after = t.strip_prefix("vsn")?.trim_start();
    let value = after
        .trim_start_matches(',')
        .split([',', '}', ']', ')'])
        .next()?
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'');
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{project_manifest_name, project_manifest_version, testutil};
    use crate::types::Language;

    #[test]
    fn erlang_app_src_parses_name_and_vsn() {
        let root = testutil::scratch(&[(
            "myerlang.app.src",
            "{application, myerlang,\n  [{description, \"x\"},\n   {vsn, \"2.1.0\"}]}.\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Erlang).as_deref(),
            Some("myerlang")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Erlang).as_deref(),
            Some("2.1.0")
        );
        testutil::cleanup(&root);
    }
}
