//! Deno: `deno.json` / `deno.jsonc`.

use std::fs;
use std::path::Path;

use super::LanguageSpec;

pub(crate) struct Deno;

impl LanguageSpec for Deno {
    fn present(&self, dir: &Path) -> bool {
        dir.join("deno.json").exists() || dir.join("deno.jsonc").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        deno_json_field(root, "name")
    }

    fn version(&self, root: &Path) -> Option<String> {
        deno_json_field(root, "version")
    }

    fn license(&self, root: &Path) -> Option<String> {
        deno_json_field(root, "license")
    }

    fn category_hint(&self) -> &'static str {
        "the Deno tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.ts".into(),
            "*.js".into(),
            "deno.json".into(),
            "deno.jsonc".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import {{ … }} from \"{name}\"")
    }
}

fn deno_json_field(root: &Path, field: &str) -> Option<String> {
    for f in &["deno.json", "deno.jsonc"] {
        if let Ok(raw) = fs::read_to_string(root.join(f)) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(val) = v.get(field).and_then(|n| n.as_str()) {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::introspect::manifest::{
        manifest_license, project_manifest_name, project_manifest_version, testutil,
    };
    use crate::types::Language;

    #[test]
    fn deno_manifest_parses_name_version_and_license() {
        let root = testutil::scratch(&[(
            "deno.json",
            "{\n  \"name\": \"@scope/deno-tool\",\n  \"version\": \"0.9.1\",\n  \"license\": \"MIT\"\n}\n",
        )]);
        assert_eq!(
            project_manifest_name(&root, Language::Deno).as_deref(),
            Some("@scope/deno-tool")
        );
        assert_eq!(
            project_manifest_version(&root, Language::Deno).as_deref(),
            Some("0.9.1")
        );
        assert_eq!(
            manifest_license(&root, Language::Deno).as_deref(),
            Some("MIT")
        );
        testutil::cleanup(&root);
    }
}
