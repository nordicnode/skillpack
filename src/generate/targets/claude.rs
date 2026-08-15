//! The frontmatter-bearing SKILL.md format, shared by Claude Code and Codex.
//!
//! Claude emits the pack-level pair (`marketplace.json`, `plugin.json`) plus
//! the skill at BOTH the plugin path (`skills/<name>/SKILL.md`) and the native
//! Claude Code path (`.claude/skills/<name>/SKILL.md`) — same content, two
//! directories. Codex reads the same frontmatter from
//! `.codex/skills/<name>/SKILL.md` (no pack-level files).

use anyhow::{Context, Result};
use tera::{Context as TeraContext, Tera};

use crate::cli::Target;
use crate::generate::GeneratedFileOutput;

use super::TargetFormat;

pub(crate) struct SkillMarkdown;

impl TargetFormat for SkillMarkdown {
    fn render_full(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let mut c = ctx.clone();
        c.insert("noun", "skill");
        let skill = tera.render("SKILL.md", &c).context("rendering SKILL.md")?;
        let mut out = Vec::new();
        if target == Target::Claude {
            // Pack-level pair + the skill at both paths. Output order matches
            // the original inline render: marketplace, plugin, plugin-path
            // skill, native-path skill.
            let marketplace = tera
                .render("marketplace.json", &c)
                .context("rendering marketplace.json")?;
            let plugin = tera
                .render("plugin.json", &c)
                .context("rendering plugin.json")?;
            out.push(GeneratedFileOutput {
                rel_path: ".claude-plugin/marketplace.json".to_string(),
                contents: marketplace,
            });
            out.push(GeneratedFileOutput {
                rel_path: ".claude-plugin/plugin.json".to_string(),
                contents: plugin,
            });
            out.push(GeneratedFileOutput {
                rel_path: format!("skills/{name}/SKILL.md"),
                contents: skill.clone(),
            });
            out.push(GeneratedFileOutput {
                rel_path: format!(".claude/skills/{name}/SKILL.md"),
                contents: skill,
            });
        } else {
            out.push(GeneratedFileOutput {
                rel_path: format!(".codex/skills/{name}/SKILL.md"),
                contents: skill,
            });
        }
        Ok(out)
    }

    fn render_skill_only(
        &self,
        tera: &Tera,
        ctx: &TeraContext,
        target: Target,
        name: &str,
    ) -> Result<Vec<GeneratedFileOutput>> {
        let mut c = ctx.clone();
        c.insert("noun", "skill");
        let skill = tera.render("SKILL.md", &c).context("rendering SKILL.md")?;
        let mut out = Vec::new();
        if target == Target::Claude {
            out.push(GeneratedFileOutput {
                rel_path: format!("skills/{name}/SKILL.md"),
                contents: skill.clone(),
            });
            out.push(GeneratedFileOutput {
                rel_path: format!(".claude/skills/{name}/SKILL.md"),
                contents: skill,
            });
        } else {
            out.push(GeneratedFileOutput {
                rel_path: format!(".codex/skills/{name}/SKILL.md"),
                contents: skill,
            });
        }
        Ok(out)
    }

    fn skill_rel_paths(&self, target: Target, name: &str) -> Vec<String> {
        if target == Target::Claude {
            vec![
                format!("skills/{name}/SKILL.md"),
                format!(".claude/skills/{name}/SKILL.md"),
            ]
        } else {
            vec![format!(".codex/skills/{name}/SKILL.md")]
        }
    }
}
