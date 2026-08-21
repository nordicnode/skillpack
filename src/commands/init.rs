//! `skillpack init` — introspect, interview (or derive from flags/config),
//! render in memory, run the pre-commit verify gate, then write the
//! distribution files + `skillpack.toml`. Also owns the intent-derivation
//! helpers shared with `add` (`bootstrap_intent`) and the polyglot
//! `auto_intents` machinery.

use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::ValueEnum;

use skillpack::cli::{resolve_targets, Target};
use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::{self, coerce_kebab, render_all, GeneratedFileOutput};
use skillpack::introspect;
use skillpack::types;
use skillpack::verify::{self, VerifyInput, VerifyReport};

use super::{
    handle_list_request, interview_run, is_json, print_diff_preview, print_profile,
    reject_report_format, trace_detected, write_files, Confirm, CONFIRM,
};

/// Build one skill per detected language for a polyglot monorepo. The primary
/// (dominant) language keeps the project name + full auto-derived intent; each
/// secondary language becomes a library-style skill named `{name}-{lang}` with
/// a generic description/trigger the maintainer can refine via
/// `skillpack update` / `skillpack add`. Single-language repos return the
/// primary skill only (byte-identical to the old `--auto` path).
///
/// The secondary intents are deliberately LIBRARY-shaped: they carry an
/// `import_pattern` (derived from the secondary language's manifest where
/// possible), language-correct `category`/`globs`/`opencode_mode` overrides,
/// and no invocation — so a TS frontend in a Rust-CLI repo renders as a
/// library skill about TypeScript, never as a second "run the Rust binary"
/// skill.
pub(crate) fn auto_intents(
    profile: &types::ProjectProfile,
    root: &Path,
    triggers: &[String],
    import: Option<&str>,
    description: Option<&str>,
) -> Result<Vec<(String, types::Intent)>> {
    let primary_name = coerce_kebab(&profile.name);
    let mut out = vec![(
        primary_name.clone(),
        auto_intent(profile, triggers, import, description)?,
    )];
    for lang in &profile.secondary_languages {
        let lang_str = lang.as_str();
        let skill_name = format!("{primary_name}-{lang_str}");
        // The manifest may live in a nested subdirectory (web/, frontend/,
        // packages/*) — resolve it so the import pattern uses the real
        // package/crate/module name and the globs auto-attach only under
        // that subdirectory.
        let manifest_dir =
            introspect::language_manifest_dir(root, *lang).unwrap_or_else(|| root.to_path_buf());
        let globs = generate::cursor_globs_hint(*lang);
        let globs = match manifest_dir.strip_prefix(root) {
            Ok(rel) if !rel.as_os_str().is_empty() => {
                let prefix = rel.to_string_lossy().replace('\\', "/");
                globs
                    .into_iter()
                    .map(|g| format!("{prefix}/{g}"))
                    .collect::<Vec<_>>()
            }
            _ => globs,
        };
        out.push((
            skill_name.clone(),
            types::Intent {
                one_line_description: format!("Manage the {lang_str} surface of {primary_name}"),
                when_to_use_phrases: vec![format!("touch the {lang_str} code")],
                invocation_command: None,
                // Library-style: the pattern is derived from the secondary
                // language's manifest (package/crate/module name) so the skill
                // renders the import branch, not the primary CLI's invocation.
                import_pattern: Some(secondary_import_pattern(*lang, &manifest_dir, &skill_name)),
                // Language-correct derived-field overrides: the profile's
                // dominant language (e.g. Rust) must not leak into the
                // secondary skill's category/globs/opencode-mode.
                category: Some(generate::category_hint(*lang).to_string()),
                globs: Some(globs),
                opencode_mode: Some("subagent".to_string()),
                author: profile.authors.clone(),
                license: profile.license.clone().or_else(|| Some("MIT".to_string())),
                ..Default::default()
            },
        ));
    }
    Ok(out)
}

/// Derive a library import pattern for a secondary language in a polyglot
/// monorepo, using the language's own manifest name when present (the
/// package/crate/module the agent would actually import) and falling back to
/// the secondary skill name. One language-shaped pattern per ecosystem — the
/// per-language formatting lives in `introspect::manifest`'s `LanguageSpec`
/// implementations, so adding a language updates this function's data, not
/// its shape.
fn secondary_import_pattern(lang: types::Language, root: &Path, fallback: &str) -> String {
    let name =
        introspect::project_manifest_name(root, lang).unwrap_or_else(|| fallback.to_string());
    introspect::language_spec(lang).import_pattern(&name)
}

/// Derive the [`Intent`] entirely from the repo — `init --auto`. Zero
/// prompts, zero required flags: description from the README hint, triggers
/// from `--trigger` (falling back to the description hint itself), author
/// from the manifest / git config, license from the LICENSE file (caller's
/// `--license` override applies after), invocation from the detected CLI.
///
/// Fails with an actionable message when something essential can't be
/// derived: no README hint → no description; a library with no `--import`
/// → no way to tell the agent how to consume it.
pub(crate) fn auto_intent(
    profile: &types::ProjectProfile,
    triggers: &[String],
    import: Option<&str>,
    description: Option<&str>,
) -> Result<types::Intent> {
    // Description precedence: README/manifest hint first, then the explicit
    // `--description` flag — so a README-less repo can still `init --auto`
    // (the old error told users to "Pass --description" but never honored it).
    let one_line_description = profile
        .description_hint
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            description
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--auto could not derive a description (no README hint or manifest \
                 description found). Pass --description, or run `skillpack init` \
                 interactively."
            )
        })?;

    let when_to_use_phrases: Vec<String> = if triggers.is_empty() {
        // Fall back to the description hint itself as the single trigger —
        // better than an empty when_to_use (which verify warns on), and the
        // maintainer can refine via `skillpack update --trigger ...`.
        vec![one_line_description.clone()]
    } else {
        triggers
            .iter()
            .flat_map(|t| t.split([',', ';']))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    // CLI projects: prefer the RESOLVED binary's file stem (a crate with a
    // renamed `[[bin]]` — fd-find → `fd` — would otherwise be documented as
    // the package name and the pre-commit verify would fail spawning it),
    // falling back to the package name for runtime-style argvs (`go run .`,
    // `node script.js`). Libraries: --import is required — a library skill
    // with no import pattern can't tell the agent how to use it.
    let (invocation_command, import_pattern) = if profile.has_cli {
        let invocation = profile
            .cli_command
            .as_ref()
            .and_then(|c| c.first())
            // Only trust an absolute path to an existing executable (the
            // built artifact / resolved PATH script); a bare runtime name
            // like `go` or `node` says nothing about the tool's name.
            .filter(|s| Path::new(s).is_file())
            .and_then(|s| Path::new(s).file_stem())
            .and_then(|s| s.to_str())
            .map(String::from)
            .unwrap_or_else(|| profile.name.clone());
        (Some(invocation), None)
    } else {
        match import.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => (None, Some(p.to_string())),
            None => bail!(
                "--auto: no CLI detected, so this looks like a library; pass \
                 --import <PATTERN> so the skill can document how an agent \
                 consumes it"
            ),
        }
    };

    Ok(types::Intent {
        one_line_description,
        when_to_use_phrases,
        invocation_command,
        import_pattern,
        author: profile.authors.clone(),
        license: profile.license.clone().or(Some("MIT".to_string())),
        ..Default::default()
    })
}

/// Build an [`Intent`] from `init --non-interactive` flags when no
/// `skillpack.toml` exists — CI bootstrap. Mirrors the interview's field
/// semantics (description + triggers + exactly one of invocation/import;
/// author optional; license defaults to MIT and is overridable via
/// `--license`, which the caller applies afterwards).
///
/// Validation is deliberate: a pack with no description or no triggers would
/// ship a SKILL.md that `verify` soft-fails, so we refuse at the flag level
/// with an actionable message instead of writing a knowingly-weak pack.
pub(crate) fn bootstrap_intent(
    profile: &types::ProjectProfile,
    description: Option<&str>,
    triggers: &[String],
    author: Option<&str>,
    invocation: Option<&str>,
    import: Option<&str>,
) -> Result<types::Intent> {
    let one_line_description = description.unwrap_or("").trim().to_string();
    if one_line_description.is_empty() {
        bail!(
            "--non-interactive bootstrap needs --description <TEXT> \
             (one sentence describing the task an agent would use this for)"
        );
    }

    let when_to_use_phrases: Vec<String> = triggers
        .iter()
        .flat_map(|t| t.split([',', ';']))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if when_to_use_phrases.is_empty() {
        bail!(
            "--non-interactive bootstrap needs at least one --trigger <PHRASE> \
             (repeat the flag, or comma/semicolon-separate values inside one)"
        );
    }

    if invocation.is_some() && import.is_some() {
        bail!(
            "pass only one of --invocation (CLI project) or --import (library \
             project), not both"
        );
    }
    let invocation = invocation.map(str::trim).filter(|s| !s.is_empty());
    let import = import.map(str::trim).filter(|s| !s.is_empty());
    let (invocation_command, import_pattern) = match (invocation, import) {
        (Some(cmd), None) => (Some(cmd.to_string()), None),
        (None, Some(pat)) => (None, Some(pat.to_string())),
        (None, None) => {
            // Use the has_cli hint to make the error actionable.
            if profile.has_cli {
                bail!(
                    "--non-interactive bootstrap needs --invocation <CMD> \
                     (the exact command an agent should run)"
                );
            } else {
                bail!(
                    "--non-interactive bootstrap needs --import <PATTERN> \
                     (the import pattern an agent should use)"
                );
            }
        }
        (Some(_), Some(_)) => unreachable!("guarded above"),
    };

    Ok(types::Intent {
        one_line_description,
        when_to_use_phrases,
        invocation_command,
        import_pattern,
        author: author
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from),
        license: Some("MIT".to_string()),
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_init(
    root: &Path,
    verbose: bool,
    non_interactive: bool,
    auto: bool,
    accept_warnings: bool,
    license_override: Option<String>,
    raw_targets: Vec<String>,
    force: bool,
    dry_run: bool,
    template_dir: Option<&Path>,
    description: Option<String>,
    triggers: Vec<String>,
    author: Option<String>,
    invocation: Option<String>,
    import: Option<String>,
    format: verify::OutputFormat,
) -> i32 {
    // Validate the format before honoring `--target list`, so an invalid
    // `--format sarif/github/junit` errors instead of silently degrading
    // the listing to human output.
    if let Err(e) = reject_report_format(format) {
        eprintln!("fatal: {e:#}");
        return exit::INIT_FATAL;
    }
    if let Some(code) = handle_list_request("init", &raw_targets, format) {
        return code;
    }
    match run_init_inner(
        root,
        verbose,
        non_interactive,
        auto,
        accept_warnings,
        license_override,
        raw_targets,
        force,
        dry_run,
        template_dir,
        description,
        triggers,
        author,
        invocation,
        import,
        format,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_init_inner(
    root: &Path,
    verbose: bool,
    non_interactive: bool,
    auto: bool,
    accept_warnings: bool,
    license_override: Option<String>,
    raw_targets: Vec<String>,
    force: bool,
    dry_run: bool,
    template_dir: Option<&Path>,
    description: Option<String>,
    triggers: Vec<String>,
    author: Option<String>,
    invocation: Option<String>,
    import: Option<String>,
    format: verify::OutputFormat,
) -> Result<i32> {
    reject_report_format(format)?;
    let profile = introspect::introspect(root).context("introspecting repo")?;
    if verbose {
        print_profile(&profile, false);
    }

    // `--auto` implies non-interactive: zero-interaction init must never
    // stop to prompt — on a critical verification failure it refuses to write
    // (exit 2) exactly like `--non-interactive` does.
    let non_interactive = non_interactive || auto;

    // Resolve `--target all` + validate every value. Empty → `[Claude]`.
    let targets = if raw_targets.is_empty() {
        vec![Target::Claude]
    } else {
        resolve_targets(&raw_targets)?
    };
    if verbose {
        let names: Vec<String> = targets
            .iter()
            .map(|t| t.to_possible_value().unwrap().get_name().to_string())
            .collect();
        eprintln!("targets: {}", names.join(", "));
    }
    trace_detected(&profile);

    // Step 2 — interview or reuse config. `--auto` and `--non-interactive`
    // never prompt. A committed config always wins in those modes (re-runs
    // stay deterministic); without a config, `--auto` derives everything from
    // the repo and `--non-interactive` bootstraps from the explicit flags.
    let existing_cfg = Config::load(root)?;

    // Determine the full skill list. A committed config is authoritative: its
    // `[skill]` or `[[skills]]` entries are used as-is, so re-running `init`
    // preserves a hand-authored multi-skill pack instead of collapsing it to
    // the primary skill and silently rewriting skillpack.toml. Fresh scaffolds
    // (interview / `--auto` / `--non-interactive` bootstrap) produce a single
    // skill named after the project.
    let mut skills: Vec<(String, types::Intent)> = if let Some(cfg) = &existing_cfg {
        let s = cfg.to_intents();
        if s.is_empty() {
            if auto || non_interactive {
                bail!(
                    "skillpack.toml at {} is missing its [skill] table.\n\
                     To fix: re-run `skillpack init` interactively.",
                    Config::path(root).display()
                );
            }
            vec![(coerce_kebab(&profile.name), interview_run(&profile)?)]
        } else {
            s
        }
    } else if auto {
        // No committed config: --auto derives the intent from the repo
        // (description from the README hint, author from git config,
        // license from the LICENSE file, invocation from the detected
        // CLI) so a fresh checkout inits with ZERO flags and zero prompts.
        // A polyglot monorepo gets one skill per detected language.
        auto_intents(
            &profile,
            root,
            &triggers,
            import.as_deref(),
            description.as_deref(),
        )?
    } else if non_interactive {
        // Plain --non-interactive without a config: the intent comes from
        // the bootstrap flags (description/trigger/author + invocation or
        // import), each validated to be present.
        vec![(
            coerce_kebab(&profile.name),
            bootstrap_intent(
                &profile,
                description.as_deref(),
                &triggers,
                author.as_deref(),
                invocation.as_deref(),
                import.as_deref(),
            )?,
        )]
    } else {
        vec![(coerce_kebab(&profile.name), interview_run(&profile)?)]
    };

    // `--license` overrides the pack license. Applied to the primary skill —
    // `Config::from_intents` seeds `defaults.license` from it, so the override
    // cascades to any secondary skills that don't pin their own license.
    if let Some(ref lic) = license_override {
        if let Some((_, intent)) = skills.first_mut() {
            intent.license = Some(lic.clone());
        }
    }

    // Step 3 — render in memory. `render_all` handles both single-skill and
    // multi-skill packs (a one-skill list renders byte-identically to
    // `render_targets`), so one code path covers re-init on either shape.
    let files = render_all(&profile, &skills, &targets, template_dir)
        .context("rendering distribution files")?;

    // Step 4 — pre-commit verify against the rendered output (design §5.3).
    // The primary skill's verify_stdin drives the CLI spawn — the pre-commit
    // gate uses one stdin mode, matching `verify`'s own single-stdin behavior.
    let verify_stdin = skills.first().and_then(|(_, i)| i.verify_stdin.clone());
    let report = verify_rendered(&files, &profile, root, verify_stdin)?;

    if report.has_critical_failure() {
        eprintln!("\n❌ pre-commit verification FAILED. skillpack will NOT write files.");
        eprintln!("{}", verify::render(&report));
        if non_interactive {
            eprintln!(
                "Critical checks failed in --non-interactive mode; refusing to write. \
                 Fix the issues above and re-run."
            );
            return Ok(exit::INIT_FIXABLE);
        }
        let proceed = CONFIRM.keep_anyway();
        if !proceed {
            eprintln!("Aborted. No files written.");
            // A fixable problem the user chose to address rather than ship a
            // broken pack — exit code 2, not the clean-abort 1 (design §8.1:
            // "2 if a fixable problem occurred"). INIT_ABORTED is reserved for
            // a user declining with no underlying verify problem.
            return Ok(exit::INIT_FIXABLE);
        }
        // User chose to keep — fall through to write.
    } else {
        let (_pass, warn, _fail, _skip) = report.counts();
        // Warnings are advisory but NOT silently ignored: in interactive mode
        // without --accept-warnings, we print them and ask the user to proceed
        // (matching the --help doc: "Without this flag, any non-pass result
        // prompts the user"). In --non-interactive mode warnings never block —
        // CI runs are gated on criticals only ("critical still blocks").
        if warn > 0 {
            eprintln!("\n⚠ verification passed with warnings:");
            eprintln!("{}", verify::render(&report));
            if !accept_warnings && !non_interactive {
                let proceed = CONFIRM.proceed_with_warnings();
                if !proceed {
                    eprintln!("Aborted. No files written.");
                    return Ok(exit::INIT_ABORTED);
                }
            } else if non_interactive {
                eprintln!(
                    "Written in --non-interactive mode (warnings are advisory; \
                     use --accept-warnings to suppress this notice)."
                );
            }
        }
    }

    // Step 4b — preview: which files are new, changed, or unchanged?
    print_diff_preview(root, &files);

    // Step 5 — write files + save config.
    if dry_run {
        if is_json(format) {
            println!(
                "{}",
                serde_json::json!({
                    "command": "init",
                    "dry_run": true,
                    "written": [],
                    "skipped": [],
                    "would_write": files.iter().map(|f| &f.rel_path).collect::<Vec<_>>(),
                })
            );
            return Ok(exit::INIT_OK);
        }
        println!(
            "dry run: would write {} file(s) under {} (no changes made):",
            files.len(),
            root.display()
        );
        for f in &files {
            println!("   - {}", f.rel_path);
        }
        return Ok(exit::INIT_OK);
    }
    let (written, skipped) = write_files(root, &files, force)?;
    Config::from_intents(&skills).save_if_changed(root)?;
    if is_json(format) {
        println!(
            "{}",
            serde_json::json!({
                "command": "init",
                "dry_run": false,
                "written": written.iter().map(|f| &f.rel_path).collect::<Vec<_>>(),
                "skipped": skipped.iter().map(|f| &f.rel_path).collect::<Vec<_>>(),
                "config": Config::path(root).display().to_string(),
            })
        );
        return Ok(exit::INIT_OK);
    }
    println!(
        "✓ wrote {} file(s) under {}:",
        written.len(),
        root.display()
    );
    for f in &written {
        println!("   - {}", f.rel_path);
    }
    println!("   - {}", Config::path(root).display());
    // Surface any targets the collision guard skipped so the summary never
    // hides a user-requested target as silent success (design §8.2: "exit
    // 0 unless critical fail" — collision is not critical; the footer makes
    // the skip visible without changing the exit code).
    if !skipped.is_empty() {
        eprintln!(
            "ℹ skipped {} target file(s) (existing file held; pass --force to overwrite):",
            skipped.len()
        );
        for f in &skipped {
            eprintln!("   - {}", f.rel_path);
        }
    }
    Ok(exit::INIT_OK)
}

fn verify_rendered(
    files: &[GeneratedFileOutput],
    profile: &types::ProjectProfile,
    root: &Path,
    verify_stdin: Option<String>,
) -> Result<VerifyReport> {
    // Materialize the rendered files into a temp dir so verify (which expects
    // files on disk) can read them exactly as an agent coming in cold would.
    let tmp = tempfile::tempdir().context("creating temp dir for pre-commit verify")?;
    for f in files {
        let p = tmp.path().join(&f.rel_path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&p, &f.contents).with_context(|| format!("writing {}", p.display()))?;
    }
    // Also drop the committed `skillpack.toml` into the temp dir when one
    // exists: `verify`'s discovery stage reads it to learn which `name:`
    // values are legitimate (`[[skills]]` entries) before firing the
    // name_drift warning. Without it, every multi-skill re-init would report
    // spurious name_drift warnings for the secondary skills, prompt the user
    // in interactive mode, and drag the pre-commit score down (Bug: the temp
    // dir held only the rendered files, so `allowed_skill_names` was empty).
    if let Some(cfg) = Config::load(root)? {
        cfg.save(tmp.path())?;
    }
    let input = VerifyInput {
        // Discovery reads the rendered files from the temp dir (we verify the
        // ACTUAL files we're about to ship — design §5.3).
        root: tmp.path().to_path_buf(),
        // But the documented CLI runs in the real project root, where the
        // source tree / built artifact lives — spawning from the temp dir
        // (which holds only the rendered files) would false-fail any
        // relative-invocation CLI like `go run .` or `node ./bin/cli.js`.
        spawn_root: root.to_path_buf(),
        cli_command: profile.cli_command.clone(),
        repo_url: profile.repo_url.clone(),
        profile_name: Some(coerce_kebab(&profile.name)),
        verify_stdin,
    };
    verify::run(&input)
}
