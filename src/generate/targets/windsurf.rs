//! Windsurf (Cascade) rules: `.windsurf/rules/<name>.md` with the same
//! frontmatter shape as Cursor rules.

use anyhow::{Context, Result};
use tera::{Context as TeraContext, Tera};

use crate::cli::Target;
use crate::generate::GeneratedFileOutput;

use super::TargetFormat;

pub(crate) struct WindsurfRule;

impl TargetFormat for WindsurfRule {
    fn render_full(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        _target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let mut c = ctx.clone();
        c.insert("noun", "rule");
        let rule = tera
            .render("windsurf-rule.md", &c)
            .context("rendering windsurf-rule.md")?;
        Ok(vec![GeneratedFileOutput {
            rel_path: format!(".windsurf/rules/{name}.md"),
            contents: rule,
        }])
    }

    fn render_skill_only(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        _target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        self.render_full(tera, ctx, Target::Windsurf, name)
    }

    fn skill_rel_paths(&self, _target: Target, name: &str) -> Vec<String> {
        vec![format!(".windsurf/rules/{name}.md")]
    }
}
