//! JVM: `pom.xml` / `build.gradle` / `build.gradle.kts`.

use std::fs;
use std::path::Path;

use super::{extract_xml_tag, LanguageSpec};
use crate::introspect::cli_candidates::{canonicalize_for_argv, which_on_path, CliCandidate};

pub(crate) struct Jvm;

impl LanguageSpec for Jvm {
    fn present(&self, dir: &Path) -> bool {
        dir.join("pom.xml").exists()
            || dir.join("build.gradle").exists()
            || dir.join("build.gradle.kts").exists()
    }

    fn name(&self, root: &Path) -> Option<String> {
        // pom.xml: <name>...</name> or <artifactId>...</artifactId>;
        // build.gradle: rootProject.name = '...' or rootProject.name = "..."
        if let Ok(raw) = fs::read_to_string(root.join("pom.xml")) {
            if let Some(n) = extract_xml_tag(&raw, "name") {
                return Some(n);
            }
            if let Some(n) = extract_xml_tag(&raw, "artifactId") {
                return Some(n);
            }
        }
        for gradle in &["build.gradle", "build.gradle.kts"] {
            if let Ok(raw) = fs::read_to_string(root.join(gradle)) {
                if let Some(n) = extract_gradle_string(&raw, "rootProject.name") {
                    return Some(n);
                }
            }
        }
        None
    }

    fn version(&self, root: &Path) -> Option<String> {
        // pom.xml: <version>...</version>; build.gradle: version = '...'
        if let Ok(raw) = fs::read_to_string(root.join("pom.xml")) {
            if let Some(v) = extract_xml_tag(&raw, "version") {
                return Some(v);
            }
        }
        for gradle in &["build.gradle", "build.gradle.kts"] {
            if let Ok(raw) = fs::read_to_string(root.join(gradle)) {
                if let Some(v) = extract_gradle_string(&raw, "version") {
                    return Some(v);
                }
            }
        }
        None
    }

    fn authors(&self, root: &Path) -> Option<String> {
        // pom.xml: <developers><developer><name>...</name></developer></developers>
        if let Ok(raw) = fs::read_to_string(root.join("pom.xml")) {
            if let Some(devs) = extract_xml_tag(&raw, "developers") {
                if let Some(name) = extract_xml_tag(&devs, "name") {
                    return Some(name);
                }
            }
        }
        // build.gradle has no standard authors field.
        None
    }

    fn category_hint(&self) -> &'static str {
        "the JVM tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec![
            "*.java".into(),
            "*.kt".into(),
            "*.scala".into(),
            "pom.xml".into(),
            "build.gradle".into(),
            "build.gradle.kts".into(),
        ]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("import {pkg}.*;", pkg = name.replace('-', "."))
    }
    fn cli_candidate(&self, root: &Path, name: &str) -> Option<CliCandidate> {
        // Gradle `application` plugin: build/install/<name>/bin/<name> (script
        // form; `.bat` variant on Windows is handled by `canonicalize_for_argv`).
        // Present only after `gradle installDist`; we never run it here.
        let install_bin = root.join("build/install").join(name).join("bin").join(name);
        if install_bin.exists() {
            let abs = canonicalize_for_argv(&install_bin);
            return Some(CliCandidate {
                argv: vec![abs],
                spawn_cwd: root.to_path_buf(),
            });
        }

        let java = which_on_path("java")?;
        let java_bin = java.to_string_lossy().to_string();

        // Maven shade/spring-boot: target/<name>-*.jar (shaded, runnable).
        // Glob by prefix to avoid hardcoding the version.
        for dir in &["target", "build/libs"] {
            if let Ok(entries) = fs::read_dir(root.join(dir)) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("jar") {
                        continue;
                    }
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        if stem.starts_with(name) {
                            let abs = canonicalize_for_argv(&p);
                            return Some(CliCandidate {
                                argv: vec![java_bin.clone(), "-jar".to_string(), abs],
                                spawn_cwd: root.to_path_buf(),
                            });
                        }
                    }
                }
            }
        }

        // Fallback to PATH probe for an installed JAR/script on PATH.
        which_on_path(name).map(|p| CliCandidate {
            argv: vec![p.to_string_lossy().to_string()],
            spawn_cwd: root.to_path_buf(),
        })
    }
}

/// Extract a `key = "value"` or `key = 'value'` string from a Gradle build
/// file. Best-effort line scan mirroring the Ruby gemspec extractor.
/// Handles both `rootProject.name = '...'` and `version = '...'` forms.
fn extract_gradle_string(raw: &str, key: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                if let Some(s) = rest
                    .strip_prefix('"')
                    .and_then(|r| r.strip_suffix('"'))
                    .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}
