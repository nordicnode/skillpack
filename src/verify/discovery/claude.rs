//! Claude Code distribution checks: `.claude-plugin/` presence, the
//! marketplace.json + plugin.json structural validation, and the shared
//! relative-source / file-read helpers they use.

use std::path::Path;

use anyhow::{Context, Result};

use super::is_valid_kebab;
use super::schema;
use crate::introspect::{detect_language, project_manifest_version};
use crate::types::DiagTrace;
use crate::verify::result::CheckResult;

pub(crate) fn claude_present(root: &Path) -> bool {
    root.join(schema::CLAUDE_PLUGIN_DIR).is_dir()
}

// ----- marketplace.json ------------------------------------------------------

pub(crate) fn check_marketplace(root: &Path) -> Result<CheckResult> {
    let path = root.join(schema::MARKETPLACE_JSON_PATH);
    let raw = match read_optional(&path)? {
        Some(r) => r,
        None => {
            return Ok(CheckResult::fail(
                "discovery.marketplace.missing",
                "marketplace.json exists and is valid JSON",
                format!(
                    "{} not found: re-run `skillpack init` or check the path",
                    schema::MARKETPLACE_JSON_PATH
                ),
                format!(
                    "To fix: create {} at the project root and re-run `skillpack verify`.",
                    schema::MARKETPLACE_JSON_PATH
                ),
            ));
        }
    };

    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(CheckResult::fail(
                "discovery.marketplace.json",
                "marketplace.json is valid JSON",
                format!("marketplace.json does not parse: {e}"),
                "To fix: correct the JSON syntax (trailing comma? unquoted key?) and re-run.",
            ));
        }
    };

    // Required: name (kebab), owner, plugins (non-empty).
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    if name.is_empty() {
        return Ok(CheckResult::fail(
            "discovery.marketplace.name",
            "marketplace.json has a kebab-case `name`",
            "marketplace.json is missing `name`",
            "To fix: add a `\"name\": \"your-marketplace-name\"` field (kebab-case, no spaces).",
        ));
    }
    if !is_valid_kebab(name) {
        return Ok(CheckResult::fail(
            "discovery.marketplace.name",
            "marketplace.json has a kebab-case `name`",
            format!("marketplace name `{name}` is not kebab-case"),
            "To fix: use lowercase letters, digits, and hyphens only; start and end with a letter/digit; no consecutive hyphens.",
        ));
    }
    if schema::RESERVED_NAMES.contains(&name) {
        return Ok(CheckResult::warn(
            "discovery.marketplace.name",
            "marketplace.json name not reserved",
            format!("marketplace name `{name}` is on the Anthropic reserved-names blocklist"),
            "To fix: rename to something not owned by Anthropic (e.g. prefix with your org).",
        ));
    }

    if v.get("owner")
        .is_none_or(|o| o.is_null() || o == &serde_json::Value::Object(Default::default()))
    {
        return Ok(CheckResult::fail(
            "discovery.marketplace.owner",
            "marketplace.json has an `owner` object",
            "marketplace.json is missing `owner`",
            "To fix: add `\"owner\": { \"name\": \"Your Name\" }`.",
        ));
    }

    let plugins = match v.get("plugins").and_then(|p| p.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            return Ok(CheckResult::fail(
                "discovery.marketplace.plugins",
                "marketplace.json has a non-empty `plugins` array",
                "marketplace.json `plugins` is missing or empty",
                "To fix: add at least one plugin entry with `name` and `source`.",
            ));
        }
    };

    // Each plugin entry: name (kebab, not reserved) + source (./ prefix for
    // relative paths).
    for (i, entry) in plugins.iter().enumerate() {
        let pname = entry.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if pname.is_empty() {
            return Ok(CheckResult::fail(
                "discovery.marketplace.plugin_name",
                "every marketplace plugin entry has a kebab-case `name`",
                format!("plugin entry #{i} is missing `name`"),
                "To fix: add a `\"name\": \"...\"` (kebab-case) to the entry.",
            ));
        }
        if !is_valid_kebab(pname) {
            return Ok(CheckResult::fail(
                "discovery.marketplace.plugin_name",
                "every marketplace plugin entry has a kebab-case `name`",
                format!("plugin name `{pname}` is not kebab-case"),
                "To fix: lowercase letters/digits/hyphens only, no consecutive hyphens.",
            ));
        }

        let src = entry.get("source");
        match src {
            Some(serde_json::Value::String(s)) => {
                if let Err(reason) = validate_relative_source(s) {
                    return Ok(CheckResult::fail(
                        "discovery.marketplace.source",
                        "relative plugin `source` paths start with `./` and avoid `../`",
                        format!("plugin `{pname}` has an invalid source `{s}`: {reason}"),
                        "To fix: make the source a path that starts with `./`, uses forward slashes, and stays inside the repo (no `../`).",
                    ));
                }
            }
            Some(serde_json::Value::Object(_obj)) => {
                // github/url/git-subdir/npm/pip — we don't deep-validate remote
                // source objects in V1; flag only if clearly malformed.
            }
            _ => {
                return Ok(CheckResult::fail(
                    "discovery.marketplace.source",
                    "every marketplace plugin entry has a `source`",
                    format!("plugin `{pname}` is missing `source`"),
                    "To fix: add `\"source\": \"./\"` (local) or a github/url object.",
                ));
            }
        }
    }

    Ok(CheckResult::pass(
        "discovery.marketplace",
        "marketplace.json is structurally valid",
        format!(
            "{} validates ({} plugin(s))",
            schema::MARKETPLACE_JSON_PATH,
            plugins.len()
        ),
    ))
}

// ----- plugin.json ----------------------------------------------------------

pub(crate) fn check_plugin_json(root: &Path, repo_url: &Option<String>) -> Result<CheckResult> {
    let path = root.join(schema::PLUGIN_JSON_PATH);
    let raw = match read_optional(&path)? {
        Some(r) => r,
        None => {
            return Ok(CheckResult::fail(
                "discovery.plugin.missing",
                "plugin.json exists and is valid JSON",
                format!("{} not found", schema::PLUGIN_JSON_PATH),
                "To fix: run `skillpack init`; the manifest lives at .claude-plugin/plugin.json.",
            ));
        }
    };

    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(CheckResult::fail(
                "discovery.plugin.json",
                "plugin.json is valid JSON",
                format!("plugin.json does not parse: {e}"),
                "To fix: fix the JSON syntax and re-run.",
            ));
        }
    };

    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    if name.is_empty() {
        return Ok(CheckResult::fail(
            "discovery.plugin.name",
            "plugin.json has a kebab-case `name`",
            "plugin.json is missing `name`",
            "To fix: add `\"name\": \"your-plugin-name\"` (kebab-case).",
        ));
    }
    if !is_valid_kebab(name) {
        return Ok(CheckResult::fail(
            "discovery.plugin.name",
            "plugin.json name is kebab-case",
            format!("plugin name `{name}` is not kebab-case"),
            "To fix: lowercase letters/digits/hyphens only, no consecutive hyphens.",
        ));
    }

    // description (optional but recommended) and author (optional).
    // We don't hard-fail on missing author (the docs say it's optional), but a
    // missing description on a plugin is a real discoverability problem for an
    // agent — warn, don't fail.
    if v.get("description")
        .is_none_or(|d| d.as_str().is_none_or(str::is_empty))
    {
        return Ok(CheckResult::warn(
            "discovery.plugin.description",
            "plugin.json has a `description`",
            "plugin.json has no `description`",
            "To fix: set `one_line_description` in skillpack.toml (or re-run `skillpack init`), \
             then `skillpack verify --fix` regenerates plugin.json from it; Claude shows \
             this in the plugin manager.",
        ));
    }

    let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("");
    if version.is_empty() {
        return Ok(CheckResult::warn(
            "discovery.plugin.version",
            "plugin.json has a `version`",
            "plugin.json has no `version`",
            "To fix: set `version` in your manifest (Cargo.toml [package].version, package.json \"version\", pyproject.toml [project].version); then re-run `skillpack init`.",
        ));
    }

    // Version drift: plugin.json version should match the project manifest
    // version. `init` writes plugin.json from the manifest, so drift means a
    // stale hand-edited plugin.json or a manifest bump that wasn't regenerated.
    // We warn (not fail) — maintainers may intentionally pin a different
    // plugin version (e.g. a pre-release plugin for a stable library).
    let mut diag = DiagTrace::default();
    let lang = detect_language(root, &mut diag);
    if let Some(mv) = project_manifest_version(root, lang) {
        if mv != version {
            return Ok(CheckResult::warn(
                "discovery.plugin.version_drift",
                "plugin.json version matches the project manifest version",
                format!("plugin.json version `{version}` != manifest version `{mv}`"),
                "To fix: run `skillpack verify --fix` to regenerate plugin.json from the manifest \
                 (or re-run `skillpack init`), or intentionally pin the plugin version.",
            ));
        }
    }

    // URL drift: plugin.json `homepage` and `repository` both render from
    // `repo_url` (the git origin detected at introspection time — see
    // `introspect::detect_repo_url`). `init` writes plugin.json, so drift
    // means a hand-edited plugin.json or a renamed/stale remote that wasn't
    // regenerated. Warn (not fail): a maintainer may intentionally host the
    // plugin elsewhere. Skipped entirely when `repo_url` is None (no git
    // origin) — a non-git or fresh repo cannot drift on a URL there's no
    // canonical source for.
    if let Some(canonical) = repo_url {
        // Collect EVERY drifted field before warning — an early return on the
        // first mismatch would hide a second drift (homepage AND repository
        // both stale) behind a single warning.
        let drifted: Vec<&str> = ["homepage", "repository"]
            .iter()
            .filter(|field| {
                let current = v.get(**field).and_then(|x| x.as_str()).unwrap_or("");
                !crate::introspect::urls_equivalent(current, canonical)
            })
            .copied()
            .collect();
        if !drifted.is_empty() {
            let detail = drifted
                .iter()
                .map(|f| {
                    let current = v.get(*f).and_then(|x| x.as_str()).unwrap_or("");
                    format!("`{f}` `{current}`")
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(CheckResult::warn(
                "discovery.plugin.url_drift",
                &format!(
                    "plugin.json `{}` matches the git origin URL",
                    drifted.join("`, `")
                ),
                format!("plugin.json {detail} != git origin `{canonical}`"),
                "To fix: run `skillpack verify --fix` to regenerate plugin.json from the current \
                 remote (or re-run `skillpack init`), or intentionally pin the URL.",
            ));
        }
    }

    let author = v
        .get("author")
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");
    if author.is_empty() || author == "Unspecified" {
        return Ok(CheckResult::warn(
            "discovery.plugin.author",
            "plugin.json has a real `author`",
            "plugin.json has no author (or defaults to \"Unspecified\")",
            "To fix: set `author`/`defaults.author` in skillpack.toml, `authors` in your \
             manifest (Cargo.toml [package].authors, package.json \"author\", pyproject.toml \
             [project].authors, *.gemspec spec.authors), or pass --author on `init`; then run \
             `skillpack verify --fix` to regenerate plugin.json from it.",
        ));
    }

    Ok(CheckResult::pass(
        "discovery.plugin",
        "plugin.json is structurally valid",
        format!(
            "{} validates (name={name}, version={version}, author={author})",
            schema::PLUGIN_JSON_PATH
        ),
    ))
}

/// Validate a relative-path `source`. Returns `Err(reason)` if invalid.
pub fn validate_relative_source(src: &str) -> Result<(), String> {
    if !src.starts_with("./") {
        return Err("must start with `./`".to_string());
    }
    if src.contains("../") {
        return Err("must not contain `../` (escapes the marketplace root)".to_string());
    }
    if src.contains('\\') {
        return Err("must use forward slashes only".to_string());
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))
        .map(Some)
}
