//! Cursor rules: `.cursor/rules/<name>.mdc` with `description`/`globs`/
//! `alwaysApply` frontmatter.

use anyhow::{Context, Result};
use tera::{Context as TeraContext, Tera};

use crate::cli::Target;
use crate::generate::GeneratedFileOutput;

use super::TargetFormat;

pub(crate) struct CursorRule;

impl TargetFormat for CursorRule {
    fn render_full(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        _target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let mut c = ctx.clone();
        c.insert("noun", "rule");
        let mdc = tera
            .render("cursor-rule.mdc", &c)
            .context("rendering cursor-rule.mdc")?;
        Ok(vec![GeneratedFileOutput {
            rel_path: format!(".cursor/rules/{name}.mdc"),
            contents: mdc,
        }])
    }

    fn render_skill_only(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        _target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        self.render_full(tera, ctx, Target::Cursor, name)
    }

    fn skill_rel_paths(&self, _target: Target, name: &str) -> Vec<String> {
        vec![format!(".cursor/rules/{name}.mdc")]
    }
}
