//! The root-level instructions-markdown format family: a single plain
//! markdown file (no frontmatter) per repo. Copilot (`.github/
//! copilot-instructions.md`), AGENTS.md, CLAUDE.md, GEMINI.md,
//! CONVENTIONS.md (aider), and `.goose/instructions.md` all share this shape
//! — one file per repo, so there is no per-skill form and nothing for
//! `remove` to clean up.

use anyhow::{Context, Result};
use tera::{Context as TeraContext, Tera};

use crate::cli::Target;
use crate::generate::GeneratedFileOutput;
use crate::verify::schema;

use super::TargetFormat;

pub(crate) struct InstructionsMarkdown;

impl TargetFormat for InstructionsMarkdown {
    fn render_full(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        target: Target,
        _name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let mut c = ctx.clone();
        c.insert("noun", "tool");
        let (template, rel_path) = match target {
            Target::Copilot => (
                "copilot-instructions.md",
                schema::COPILOT_INSTRUCTIONS_PATH.to_string(),
            ),
            Target::AgentsMd => ("AGENTS.md", schema::AGENTS_MD_PATH.to_string()),
            Target::ClaudeMd => ("CLAUDE.md", schema::CLAUDE_MD_PATH.to_string()),
            Target::Gemini => ("GEMINI.md", schema::GEMINI_MD_PATH.to_string()),
            Target::Aider => ("CONVENTIONS.md", schema::CONVENTIONS_MD_PATH.to_string()),
            Target::Goose => ("CLAUDE.md", schema::GOOSE_INSTRUCTIONS_PATH.to_string()),
            _ => unreachable!("not an instructions target"),
        };
        let contents = tera
            .render(template, &c)
            .with_context(|| format!("rendering {template}"))?;
        Ok(vec![GeneratedFileOutput { rel_path, contents }])
    }

    // No per-skill form — these are single instructions files per repo.
}
