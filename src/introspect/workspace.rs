//! Workspace-root heuristics shared between [`super::detect_language`] and
//! [`super::detect_cli`]. These are pure manifest-structure reads: they never
//! spawn a candidate (that's `walk_*_workspace` in the parent) — they only
//! answer "is this root a workspace-only manifest?" and "what name does the
//! first member expose?" so the orchestrators can decide whether to walk.
//!
//! Kept separate from [`super::manifest`] because `manifest`'s contract is
//! "pull a scalar (`name`/`version`/`authors`/`license`) from one manifest",
//! whereas these fns walk `[workspace].members` / `workspaces` arrays and
//! reason about *structure*, not values.

use std::fs;
use std::path::Path;

use crate::types::DiagTrace;

/// True iff `Cargo.toml` at `root` has a `[workspace]` table but no
/// `[package]` table. A pure workspace root ships no binary of its own;
/// its members may. Used by the diag-trace path, not detection itself.
pub(crate) fn is_cargo_workspace_only(root: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(root.join("Cargo.toml")) else {
        return false;
    };
    let Ok(v) = toml::from_str::<toml::Value>(&raw) else {
        return false;
    };
    v.get("workspace").is_some() && v.get("package").is_none()
}

/// True iff `package.json` at `root` has a `workspaces` field but no `bin`.
pub(crate) fn is_npm_workspace_only(root: &Path) -> bool {
    let Some(raw) = fs::read_to_string(root.join("package.json")).ok() else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    v.get("workspaces").is_some() && v.get("bin").is_none()
}

/// True iff `pyproject.toml` at `root` has a `[tool.<name>]` table.
/// Detects uv (`[tool.uv]`) and poetry (`[tool.poetry]`) managed monorepos
/// so doctor can explain the "not yet walked" gap.
pub(crate) fn pyproject_has_tool(root: &Path, name: &str) -> bool {
    let Some(raw) = fs::read_to_string(root.join("pyproject.toml")).ok() else {
        return false;
    };
    let Ok(v) = toml::from_str::<toml::Value>(&raw) else {
        return false;
    };
    v.get("tool").and_then(|t| t.get(name)).is_some()
}

/// First `[package].name` from a Cargo workspace member dir. Mirrors the
/// parse in `walk_cargo_workspace` (parent) but stops at name resolution (no
/// candidate/spawn probe) — used by `introspect` so `detect_cli` gets a name
/// to probe. Returns `None` if no member has a `[package].name`.
pub(crate) fn first_cargo_member_name(root: &Path, diag: &mut DiagTrace) -> Option<String> {
    let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let v = toml::from_str::<toml::Value>(&raw).ok()?;
    let members = v.get("workspace")?.get("members")?.as_array()?;
    for m in members {
        let Some(rel) = m.as_str() else { continue };
        let member_root = root.join(rel);
        let name = fs::read_to_string(member_root.join("Cargo.toml"))
            .ok()
            .and_then(|r| toml::from_str::<toml::Value>(&r).ok())
            .and_then(|mv| {
                mv.get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .map(String::from)
            });
        if let Some(n) = name {
            diag.push(
                "detect_language.rust.workspace",
                format!("workspace member `{rel}` supplied tool name `{n}`"),
            );
            return Some(n);
        }
    }
    diag.push(
        "detect_language.rust.workspace",
        "no workspace member has a [package].name — name fell back to dir tail".to_string(),
    );
    None
}

/// First `name` from an npm workspace member `package.json`. Mirrors
/// `walk_npm_workspace` (parent) but stops at name resolution. Returns `None`
/// if no member has a `name` field.
pub(crate) fn first_npm_member_name(root: &Path, diag: &mut DiagTrace) -> Option<String> {
    let raw = fs::read_to_string(root.join("package.json")).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let ws = v.get("workspaces")?;
    let paths: Vec<String> = match ws {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|e| e.as_str().map(String::from))
            .collect(),
        _ => return None,
    };
    for rel in paths {
        let pkg = root.join(&rel).join("package.json");
        let name = fs::read_to_string(&pkg)
            .ok()
            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
            .and_then(|mv| mv.get("name").and_then(|n| n.as_str()).map(String::from));
        if let Some(n) = name {
            diag.push(
                "detect_language.node.workspace",
                format!("workspace member `{rel}` supplied tool name `{n}`"),
            );
            return Some(n);
        }
    }
    diag.push(
        "detect_language.node.workspace",
        "no workspace member has a package.json `name` — name fell back to dir tail".to_string(),
    );
    None
}
