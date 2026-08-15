//! The rules-directory format family: plain markdown (no frontmatter) under a
//! per-ecosystem directory. Shared by Cline (flat `.clinerules/`), Roo, Kilo,
//! Qoder, Continue, Augment, Amazon Q, and Trae (`<ecosystem>/rules/`).

use anyhow::{Context, Result};
use tera::{Context as TeraContext, Tera};

use crate::cli::Target;
use crate::generate::GeneratedFileOutput;

use super::TargetFormat;

pub(crate) struct RulesMarkdown;

impl TargetFormat for RulesMarkdown {
    fn render_full(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let mut c = ctx.clone();
        c.insert("noun", "rule");
        let rule = tera
            .render("CLAUDE.md", &c)
            .context("rendering rules-directory rule")?;
        Ok(vec![GeneratedFileOutput {
            rel_path: format!("{}/{name}.md", rule_dir(target)),
            contents: rule,
        }])
    }

    fn render_skill_only(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        self.render_full(tera, ctx, target, name)
    }

    fn skill_rel_paths(&self, target: Target, name: &str) -> Vec<String> {
        vec![format!("{}/{name}.md", rule_dir(target))]
    }
}

/// The rules directory for a plain-markdown rule target. Cline uses a flat
/// `.clinerules/`; the rest nest under `<ecosystem>/rules/`. Only valid for
/// the rules-directory targets.
pub(crate) fn rule_dir(target: Target) -> &'static str {
    match target {
        Target::Cline => ".clinerules",
        Target::Roo => ".roo/rules",
        Target::Kilo => ".kilocode/rules",
        Target::Qoder => ".qoder/rules",
        Target::Continue => ".continue/rules",
        Target::Augment => ".augment/rules",
        Target::AmazonQ => ".amazonq/rules",
        Target::Trae => ".trae/rules",
        _ => unreachable!("not a rules-directory target"),
    }
}
