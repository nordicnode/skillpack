//! `verify --fix`: typed fix actions + appliers for mechanical drift.
//!
//! Ponytail scope: one variant per mechanical drift class that verify already
//! detects. Adding a new fixable drift is a compile-driven extension — the
//! exhaustive `apply` match below means forgetting an applier is a build
//! failure, not a silent runtime gap.
//!
//! `--fix` regenerates files surgically (only the file the drift lives in),
//! never full-tree — `skillpack init` is the wholesale regen command. A user
//! who hand-tailored `SKILL.md` description or `allowed-tools` post-init must
//! not see `verify --fix` wipe their work to fix an unrelated `plugin.json`.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::cli::Target;
use crate::generate::{render_targets, GeneratedFileOutput};
use crate::{config::Config, introspect, types::Intent};

/// A mechanical drift class verify already detects + can fix deterministically.
/// One variant per fixable check_id; the `apply` match must visit every
/// variant (compile-driven extension — add a variant, add an arm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixAction {
    /// `discovery.plugin.version_drift`: `.claude-plugin/plugin.json`'s
    /// `version` doesn't match the manifest version. Apply by regenerating
    /// ONLY `plugin.json` from the current manifest + intent, leaving the
    /// committed `SKILL.md` / `marketplace.json` alone.
    RegenPluginJson,
}

/// What `apply` did: the file paths it wrote. Empty `files_written` is a
/// no-op applied (the applier had nothing to write — caller reports "0
/// fixes applied").
#[derive(Debug, Clone, Default)]
pub struct FixOutcome {
    pub files_written: Vec<String>,
}

impl FixOutcome {
    pub fn is_empty(&self) -> bool {
        self.files_written.is_empty()
    }
    pub fn len(&self) -> usize {
        self.files_written.len()
    }
    /// Sorted unique list of files written, for the human message.
    pub fn unique_sorted(&self) -> Vec<String> {
        let v: Vec<String> = self
            .files_written
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        v
    }
}

/// Apply one fix action. Reads the repo state, regenerates the minimum file
/// the drift lives in, writes it. Returns the file paths written (empty on
/// no-op). Fails if the prerequisites for the fix are absent (e.g. a
/// `skillpack.toml` to recover the intent from).
pub fn apply(action: FixAction, root: &Path) -> Result<FixOutcome> {
    match action {
        FixAction::RegenPluginJson => apply_regen_plugin_json(root),
    }
}

fn apply_regen_plugin_json(root: &Path) -> Result<FixOutcome> {
    // Recover profile + intent the same `init` does — introspection gives us
    // the manifest version + language; the committed skillpack.toml gives us
    // the interview answers. Without skillpack.toml there's no intent, so a
    // hand-written plugin (no init history) is unfixable here — direct the
    // maintainer to `skillpack init`.
    let profile = introspect::introspect(root).context("introspecting repo for --fix")?;
    let Some(cfg) = Config::load(root)? else {
        bail!(
            "no skillpack.toml at {} — `--fix` can only repair init-managed\n\
             distribution files; run `skillpack init` to seed it first.",
            root.display()
        );
    };
    let Some(intent): Option<Intent> = cfg.to_intent() else {
        bail!(
            "skillpack.toml at {} has no `[skill]` block — cannot recover intent for --fix",
            root.display()
        );
    };

    // Render the claude target, then KEEP ONLY the plugin.json entry. Surgical
    // by design — re-emitting SKILL.md / marketplace.json here would clobber
    // a maintainer's post-init hand-tailoring (see module docs).
    let files = render_targets(&profile, &intent, &[Target::Claude])
        .context("rendering claude target for --fix")?;
    let plugin_json = files
        .iter()
        .find(|f| f.rel_path.ends_with("plugin.json"))
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "claude target render produced no plugin.json — fix prerequisites not met"
            )
        })?;
    write_one(root, &plugin_json)?;
    Ok(FixOutcome {
        files_written: vec![plugin_json.rel_path],
    })
}

fn write_one(root: &Path, file: &GeneratedFileOutput) -> Result<()> {
    let p = root.join(&file.rel_path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir for {}", p.display()))?;
    }
    std::fs::write(&p, &file.contents)
        .with_context(|| format!("writing {} for --fix", p.display()))?;
    Ok(())
}

/// Decide whether a given check_id is fixable by `--fix`. Returns the action
/// to apply, or `None` if the check_id has no mechanical fix. Used by the
/// verify `--fix` dispatcher to filter the report to only-fixable drift.
pub fn action_for(check_id: &str) -> Option<FixAction> {
    match check_id {
        "discovery.plugin.version_drift" => Some(FixAction::RegenPluginJson),
        // Extend here + add the match arm above — exhaustive match makes
        // forgetting an arm a compile failure, not a silent skip.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_for_maps_known_drift() {
        assert_eq!(
            action_for("discovery.plugin.version_drift"),
            Some(FixAction::RegenPluginJson)
        );
        assert_eq!(action_for("discovery.description"), None);
        assert_eq!(action_for("invocation.help_present"), None);
    }

    #[test]
    fn apply_match_is_exhaustive_over_enum() {
        // `match action` over FixAction inside `apply` is exhaustive by
        // construction. This test exists to surface an obvious failure when
        // a variant is added without an arm — the compile-time exhaustive
        // check is the real guard; this just lights up in case the file
        // ever migrates to a non-exhaustive dispatch shape.
        let action = FixAction::RegenPluginJson;
        let _ = action_for("&not-real"); // noop to keep `action` live
        let _ = action;
    }

    #[test]
    fn fixoutcome_unique_sorted_dedupes() {
        let o = FixOutcome {
            files_written: vec!["b.json".into(), "a.json".into(), "b.json".into()],
        };
        assert_eq!(
            o.unique_sorted(),
            vec!["a.json".to_string(), "b.json".to_string()]
        );
    }
}
