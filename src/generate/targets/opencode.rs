//! OpenCode agents: `.opencode/agents/<name>.md` with `description`
//! (required) + `mode` frontmatter. Per opencode.ai/docs/agents.

use anyhow::{Context, Result};
use tera::{Context as TeraContext, Tera};

use crate::cli::Target;
use crate::generate::GeneratedFileOutput;

use super::TargetFormat;

pub(crate) struct OpenCodeAgent;

impl TargetFormat for OpenCodeAgent {
    fn render_full(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        _target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let mut c = ctx.clone();
        c.insert("noun", "agent");
        let agent = tera
            .render("opencode-agent.md", &c)
            .context("rendering opencode-agent.md")?;
        Ok(vec![GeneratedFileOutput {
            rel_path: format!(".opencode/agents/{name}.md"),
            contents: agent,
        }])
    }

    fn render_skill_only(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        _target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        self.render_full(tera, ctx, Target::OpenCode, name)
    }

    fn skill_rel_paths(&self, _target: Target, name: &str) -> Vec<String> {
        vec![format!(".opencode/agents/{name}.md")]
    }
}
