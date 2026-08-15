//! The target-format registry: `Target` → the module that renders its
//! distribution files. Each format implements [`TargetFormat`]; adding a
//! target means adding one arm to [`format_for`] (when it shares an existing
//! format family) or a new format module plus one arm. The per-skill rel-path
//! list (`orphaned_skill_rel_paths`) is derived from the same registry, so a
//! new target can never be forgotten by `skillpack remove`.

use anyhow::Result;
use tera::{Context as TeraContext, Tera};

use crate::cli::{Target, ALL_TARGETS};
use crate::generate::GeneratedFileOutput;

mod claude;
mod cursor;
mod instructions_markdown;
mod opencode;
mod rules_markdown;
mod windsurf;

/// One distribution format: how to render a target's files for the primary
/// skill and for additional skills in a multi-skill pack, plus the rel-paths
/// a skill owns (for `remove` cleanup).
pub(crate) trait TargetFormat {
    /// Full file set for the primary skill (pack-level files included).
    fn render_full(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>>;
    /// Per-skill file(s) for additional skills; empty for pack-level-only
    /// targets (Copilot/AGENTS.md/Goose have a single instructions file per
    /// repo, not per skill).
    fn render_skill_only(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let _ = (tera, ctx, target, name);
        Ok(Vec::new())
    }
    /// The rel-paths a skill named `name` owns for this target — what
    /// `skillpack remove <name>` deletes. Empty for pack-level-only targets.
    fn skill_rel_paths(&self, target: Target, name: &str) -> Vec<String> {
        let _ = (target, name);
        Vec::new()
    }
}

/// Registry: `Target` → its format. One arm per target — the single place a
/// new target registers.
pub(crate) fn format_for(target: Target) -> &'static dyn TargetFormat {
    match target {
        Target::Claude | Target::Codex => &claude::SkillMarkdown,
        Target::Cursor => &cursor::CursorRule,
        Target::OpenCode => &opencode::OpenCodeAgent,
        Target::Windsurf => &windsurf::WindsurfRule,
        Target::Cline
        | Target::Roo
        | Target::Kilo
        | Target::Qoder
        | Target::Continue
        | Target::Augment
        | Target::AmazonQ
        | Target::Trae => &rules_markdown::RulesMarkdown,
        Target::Copilot
        | Target::AgentsMd
        | Target::ClaudeMd
        | Target::Gemini
        | Target::Aider
        | Target::Goose => &instructions_markdown::InstructionsMarkdown,
    }
}

/// Every per-skill distribution path a skill named `name` would own (the
/// files `render_all` emits for a secondary `[[skills]]` entry, plus the
/// Claude/Codex primary-skill copies). `skillpack remove <name>` deletes
/// these to clean up an orphaned skill — the pack-level single-file targets
/// (AGENTS.md, Copilot, Goose, …) are shared across skills and are NOT here,
/// since `update` regenerates them from the remaining config. Derived from
/// [`format_for`], so a new target automatically joins the cleanup list.
pub fn orphaned_skill_rel_paths(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for target in ALL_TARGETS {
        out.extend(format_for(target).skill_rel_paths(target, name));
    }
    out
}
