//! `skillpack add` — append a new skill to an existing `skillpack.toml` pack
//! and regenerate the distribution files. The new skill's intent comes from
//! the interview (or the `--non-interactive` bootstrap flags); the existing
//! skills are left untouched.

use std::path::Path;

use anyhow::{bail, Context, Result};

use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::coerce_kebab;
use skillpack::introspect;
use skillpack::verify;

use super::init::bootstrap_intent;
use super::update::run_update_inner;
use super::{coerce_add_name, handle_list_request, interview_run, reject_report_format};

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_add(
    root: &Path,
    verbose: bool,
    name: &str,
    non_interactive: bool,
    description: Option<String>,
    triggers: Vec<String>,
    author: Option<String>,
    invocation: Option<String>,
    import: Option<String>,
    license_override: Option<String>,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> i32 {
    if let Some(code) = handle_list_request("add", &raw_targets, format) {
        return code;
    }
    match run_add_inner(
        root,
        verbose,
        name,
        non_interactive,
        description,
        triggers,
        author,
        invocation,
        import,
        license_override,
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

#[allow(clippy::too_many_arguments)]
fn run_add_inner(
    root: &Path,
    verbose: bool,
    name: &str,
    non_interactive: bool,
    description: Option<String>,
    triggers: Vec<String>,
    author: Option<String>,
    invocation: Option<String>,
    import: Option<String>,
    license_override: Option<String>,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> Result<i32> {
    reject_report_format(format)?;
    let profile = introspect::introspect(root).context("introspecting repo for add")?;
    let Some(cfg) = Config::load(root)? else {
        bail!(
            "no skillpack.toml at {}: `add` appends to an existing pack.\n\
             To fix: run `skillpack init` first to seed the pack, then `skillpack add <name>`.",
            root.display()
        );
    };

    let skill_name = coerce_add_name(name)?;
    let mut intents = cfg.to_intents();
    if intents.iter().any(|(n, _)| coerce_kebab(n) == skill_name) {
        bail!("skill `{skill_name}` already exists in skillpack.toml; pick a different name");
    }

    let mut intent = if non_interactive {
        bootstrap_intent(
            &profile,
            description.as_deref(),
            &triggers,
            author.as_deref(),
            invocation.as_deref(),
            import.as_deref(),
        )?
    } else {
        interview_run(&profile)?
    };
    if let Some(lic) = license_override {
        intent.license = Some(lic);
    }
    intents.push((skill_name, intent));

    // Persist the expanded pack, then delegate to `update` — which re-loads
    // the (now larger) config and re-renders every target.
    Config::from_intents(&intents).save_if_changed(root)?;
    run_update_inner(root, verbose, raw_targets, force, template_dir, format)
}
