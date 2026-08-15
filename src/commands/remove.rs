//! `skillpack remove <name>` — drop a skill from the pack. Edits the
//! committed `skillpack.toml`, deletes the orphaned per-skill distribution
//! files, and regenerates the remaining targets. Symmetric with `add`.

use std::path::Path;

use anyhow::{bail, Context, Result};

use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::coerce_kebab;
use skillpack::types;
use skillpack::verify;

use super::update::run_update_inner;
use super::{coerce_add_name, handle_list_request, is_json, reject_report_format};

pub(crate) fn run_remove(
    root: &Path,
    verbose: bool,
    name: &str,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> i32 {
    if let Some(code) = handle_list_request("remove", &raw_targets, format) {
        return code;
    }
    match run_remove_inner(
        root,
        verbose,
        name,
        raw_targets,
        force,
        template_dir,
        format,
    ) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_remove_inner(
    root: &Path,
    verbose: bool,
    name: &str,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> Result<i32> {
    reject_report_format(format)?;
    let Some(cfg) = Config::load(root)? else {
        bail!(
            "no skillpack.toml at {}: `remove` drops a skill from an existing pack.\n\
             To fix: run `skillpack init` first to seed the pack, then `skillpack remove <name>`.",
            root.display()
        );
    };

    let skill_name = coerce_add_name(name)?;
    let intents = cfg.to_intents();
    if intents.is_empty() {
        bail!("skillpack.toml has no skills to remove");
    }
    let original_len = intents.len();
    let remaining: Vec<(String, types::Intent)> = intents
        .into_iter()
        .filter(|(n, _)| coerce_kebab(n) != skill_name)
        .collect();
    if remaining.len() == original_len {
        bail!("skill `{skill_name}` not found in skillpack.toml; nothing removed");
    }
    // Refuse to remove the LAST skill: a pack needs at least one skill to
    // render any distribution file, and deleting the last one would leave
    // skillpack.toml skill-less (broken for `init`/`update`/`verify`) while
    // the pack-level files (plugin.json / marketplace.json) still reference
    // the deleted skill. Refuse up front with an actionable message instead
    // of half-removing the pack and then failing mid-way.
    if remaining.is_empty() {
        bail!(
            "`{skill_name}` is the only skill in skillpack.toml; removing it would \
             leave the pack empty. Re-create the pack with `skillpack init`, or \
             add another skill first via `skillpack add <name>`."
        );
    }

    // Persist the shrunken pack first (so a failure later doesn't leave the
    // config claiming a skill whose files are gone), then delete the
    // orphaned per-skill files, then regenerate the remaining targets.
    Config::from_intents(&remaining).save(root)?;

    let mut removed = Vec::new();
    for rel in skillpack::generate::orphaned_skill_rel_paths(&skill_name) {
        let disk = root.join(&rel);
        if disk.is_file() {
            std::fs::remove_file(&disk).with_context(|| format!("removing {rel}"))?;
            removed.push(rel.clone());
            // Best-effort: drop the now-empty parent dirs (`skills/<name>/`,
            // `.claude/skills/<name>/`, rule dirs are shared and stay).
            if let Some(parent) = disk.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }

    if is_json(format) {
        println!(
            "{}",
            serde_json::json!({
                "command": "remove",
                "skill": skill_name,
                "removed_files": removed,
            })
        );
        return Ok(exit::INIT_OK);
    }
    println!(
        "✓ removed skill `{skill_name}` ({} file(s) deleted)",
        removed.len()
    );

    run_update_inner(root, verbose, raw_targets, force, template_dir, format)
}
