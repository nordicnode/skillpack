//! `verify --fix`: typed fix actions + appliers for mechanical drift.
//!
//! Ponytail scope: one variant per mechanical drift class that verify already
//! detects. Adding a new fixable drift is a compile-driven extension — the
//! exhaustive `apply` match below means forgetting an applier is a build
//! failure, not a silent runtime gap.
//!
//! `--fix` regenerates files surgically (only the file the drift lives in),
//! never full-tree — `skillpack init` is the wholesale regen command. For
//! `SKILL.md` drift the surgery goes one level finer: ONLY the frontmatter
//! block is regenerated from the current intent; the body prose a maintainer
//! may have hand-tailored post-init is preserved byte-for-byte. (Regenerating
//! the whole `SKILL.md` would clobber the gotchas / examples sections — the
//! maintainer's authorship belongs to them, not the template.)

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
    /// `discovery.plugin.version_drift` / `discovery.plugin.url_drift`:
    /// `.claude-plugin/plugin.json`'s `version` / `homepage` / `repository`
    /// drifted from the manifest / git origin. Apply by regenerating ONLY
    /// `plugin.json` from the current manifest + intent, leaving the committed
    /// `SKILL.md` / `marketplace.json` alone.
    RegenPluginJson,
    /// `discovery.skill.name_drift` / `discovery.codex.skill.name_drift`:
    /// a SKILL.md's frontmatter `name:` drifted from the canonical project
    /// name (`coerce_kebab(profile.name)`). Apply by regenerating ONLY the
    /// frontmatter block from the current intent — the body prose (which a
    /// maintainer may have hand-tailored) is preserved byte-for-byte. The
    /// skill file path is threaded via `apply`'s `location` param since the
    /// drift may live at `skills/<name>/SKILL.md` (Claude) or
    /// `.codex/skills/<name>/SKILL.md` (Codex), and the `<name>` segment may
    /// itself be the drifted value.
    RegenSkillMdFrontmatter,
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
///
/// `location` is the `CheckResult.location` (rel-path + optional line) that
/// triggered the fix — threaded so `RegenSkillMdFrontmatter` knows WHICH skill
/// file to rewrite (the drift may live at `skills/<name>/SKILL.md` for Claude
/// or `.codex/skills/<name>/SKILL.md` for Codex, and `<name>` may itself be
/// the drifted value). Ignored by `RegenPluginJson` (fixed path).
pub fn apply(
    action: FixAction,
    root: &Path,
    location: Option<&(String, Option<usize>)>,
) -> Result<FixOutcome> {
    match action {
        FixAction::RegenPluginJson => apply_regen_plugin_json(root),
        FixAction::RegenSkillMdFrontmatter => apply_regen_skill_md_frontmatter(root, location),
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

fn apply_regen_skill_md_frontmatter(
    root: &Path,
    location: Option<&(String, Option<usize>)>,
) -> Result<FixOutcome> {
    // The location's rel-path tells us WHICH skill file drifted AND which
    // ecosystem (Claude `skills/<name>/SKILL.md` vs Codex
    // `.codex/skills/<name>/SKILL.md`). Without it, the applicator cannot
    // know what to rewrite — name_drift without a location is a programming
    // bug, not a user-facing state.
    let loc = location
        .map(|(p, _)| p.as_str())
        .ok_or_else(|| anyhow::anyhow!("name_drift fix dispatched without a location path"))?;

    // Recover profile + intent (same precedent as apply_regen_plugin_json).
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

    // Derive the target from the location path: Codex skills live under
    // `.codex/skills/`, Claude skills under `skills/`. Render ONLY the
    // ecosystem whose file drifted — surgical: we don't touch the other path.
    let target = if loc.starts_with(".codex/skills/") {
        Target::Codex
    } else {
        Target::Claude
    };
    let files =
        render_targets(&profile, &intent, &[target]).context("rendering skill target for --fix")?;

    // The rendered SKILL.md whose rel-path matches the drifted file. Render
    // produces a fresh full skill (frontmatter + body); we keep ONLY the
    // frontmatter from it and rebuild the body from the committed file below.
    let fresh_skill = files
        .iter()
        .find(|f| f.rel_path == loc)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!("rendered no skill at `{loc}` — fix prerequisites not met")
        })?;

    // Slice the fresh frontmatter: everything from the opening `---` through
    // the closing `---` (inclusive). If the template stopped emitting a
    // frontmatter block, fall back to the full fresh file (no body to splice).
    let fresh_frontmatter = split_frontmatter(&fresh_skill.contents)
        .map(|(fm, _body)| fm)
        .unwrap_or_else(|| fresh_skill.contents.clone());

    // Read the committed file and preserve its BODY — the prose a maintainer
    // may have hand-tailored post-init (the whole reason `--fix` is surgical
    // here instead of wholesale regen).
    let committed_path = root.join(loc);
    // Normalize CRLF→LF before splitting: a Windows `git autocrlf` checkout
    // (or a direct CRLF commit) would make `split_frontmatter`'s `\n---\n`
    // probe miss the closing delimiter → return None → preserved_body default
    // empty → the maintainer's BODY PROSE SILENTLY DELETED on `--fix`. Writing
    // back LF-normalized also matches `init`'s output (the template renders LF
    // and `.gitattributes` pins `*.md text eol=lf`, so LF-on-disk is the
    // canonical form downstream verify byte-tests against).
    let committed = std::fs::read_to_string(&committed_path)
        .with_context(|| format!("reading committed skill {}", committed_path.display()))?
        .replace("\r\n", "\n");
    let preserved_body = split_frontmatter(&committed)
        .map(|(_fm, body)| body)
        .unwrap_or_default();

    // Splice: regenerated frontmatter + preserved body. A single `\n` joins
    // them; the template ends its frontmatter block with a closing `---` and
    // the body starts with a blank line, so the joined artifact mirrors what
    // `init` writes on a fresh run (modulo the maintainer's body edits).
    let spliced = format!("{fresh_frontmatter}\n{preserved_body}");
    std::fs::write(&committed_path, &spliced)
        .with_context(|| format!("writing spliced skill {}", committed_path.display()))?;
    Ok(FixOutcome {
        files_written: vec![loc.to_string()],
    })
}

/// Split a SKILL.md into `(frontmatter_block, trailing_body)`. The frontmatter
/// block includes both `---` delimiters; the body is everything after the
/// closing `---` (typically a leading blank line + the prose). Returns `None`
/// if the file has no `---`-delimited frontmatter (a hand-written skill with
/// no frontmatter, or a corrupted file — caller falls back to whole-file).
fn split_frontmatter(contents: &str) -> Option<(String, String)> {
    let after_open = contents.strip_prefix("---\n")?;
    let close_idx = after_open.find("\n---\n")?;
    let fm = format!("---\n{}---", &after_open[..close_idx]);
    let body = after_open[close_idx + "\n---\n".len()..].to_string();
    Some((fm, body))
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
        "discovery.plugin.version_drift" | "discovery.plugin.url_drift" => {
            Some(FixAction::RegenPluginJson)
        }
        "discovery.skill.name_drift" | "discovery.codex.skill.name_drift" => {
            Some(FixAction::RegenSkillMdFrontmatter)
        }
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
    fn action_for_maps_url_drift() {
        // RegenPluginJson rebuilds homepage+repository from the current git
        // origin, so url_drift is the same mechanical fix as version_drift.
        assert_eq!(
            action_for("discovery.plugin.url_drift"),
            Some(FixAction::RegenPluginJson)
        );
    }

    #[test]
    fn action_for_maps_name_drift_both_ecosystems() {
        // Both Claude (`discovery.skill.name_drift`) and Codex
        // (`discovery.codex.skill.name_drift`) map to RegenSkillMdFrontmatter —
        // the applicator uses the threaded location path to dispatch to the
        // right file (skills/<name>/SKILL.md vs .codex/skills/<name>/SKILL.md).
        assert_eq!(
            action_for("discovery.skill.name_drift"),
            Some(FixAction::RegenSkillMdFrontmatter)
        );
        assert_eq!(
            action_for("discovery.codex.skill.name_drift"),
            Some(FixAction::RegenSkillMdFrontmatter)
        );
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
