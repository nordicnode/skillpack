//! `skillpack` entry point. Dispatches to `init` or `verify`.
//!
//! The `init` flow (design §5.1 + §5.3) is the load-bearing piece:
//!   1. introspect the repo
//!   2. interview the user (or reuse `skillpack.toml`)
//!   3. render the three files *in memory*
//!   4. run `verify` against that in-memory output (the pre-commit gate)
//!   5. if critical checks fail → report, do NOT write, exit `INIT_FIXABLE`
//!      unless the user explicitly confirms; warnings are advisory
//!   6. write the files + save `skillpack.toml` once cleared

use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, ValueEnum};

use skillpack::cli::{resolve_targets, Cli, Commands, LogFormat, Target};
use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::{self, coerce_kebab, render_all, GeneratedFileOutput};
use skillpack::interview;
use skillpack::introspect;
use skillpack::types;
use skillpack::verify::{self, VerifyInput, VerifyReport};

fn main() {
    skillpack::spawn::reset_sigpipe();
    let cli = Cli::parse();
    init_logging(cli.effective_log_filter(), cli.log_format);

    let code = match std::panic::catch_unwind(|| match cli.command {
        Commands::Init {
            root,
            non_interactive,
            auto,
            accept_warnings,
            license,
            target,
            force,
            dry_run,
            template_dir,
            description,
            trigger,
            author,
            invocation,
            import,
            format,
        } => run_init(
            &root,
            cli.verbose,
            non_interactive,
            auto,
            accept_warnings,
            license,
            target,
            force,
            dry_run,
            template_dir.as_deref(),
            description,
            trigger,
            author,
            invocation,
            import,
            format,
        ),
        Commands::Verify {
            root,
            format,
            fix,
            min_score,
            watch,
            template_dir,
        } => run_verify(
            &root,
            cli.verbose,
            format,
            fix,
            min_score,
            watch,
            template_dir.as_deref(),
        ),
        Commands::Doctor { root, format } => run_doctor(&root, cli.verbose, format),
        Commands::Update {
            root,
            target,
            force,
            template_dir,
            format,
        } => run_update(
            &root,
            cli.verbose,
            target,
            force,
            template_dir.as_deref(),
            format,
        ),
        Commands::Diff {
            root,
            target,
            force,
            template_dir,
            format,
        } => run_diff(
            &root,
            cli.verbose,
            target,
            force,
            template_dir.as_deref(),
            format,
        ),
        Commands::Add {
            name,
            root,
            non_interactive,
            description,
            trigger,
            author,
            invocation,
            import,
            license,
            target,
            force,
            template_dir,
            format,
        } => run_add(
            &root,
            cli.verbose,
            &name,
            non_interactive,
            description,
            trigger,
            author,
            invocation,
            import,
            license,
            target,
            force,
            template_dir.as_deref(),
            format,
        ),
        Commands::Remove {
            name,
            root,
            target,
            force,
            template_dir,
            format,
        } => run_remove(
            &root,
            cli.verbose,
            &name,
            target,
            force,
            template_dir.as_deref(),
            format,
        ),
        Commands::Config { root, validate } => run_config(&root, validate),
        Commands::Completions { shell } => {
            let mut cmd = <Cli as CommandFactory>::command();
            clap_complete::generate(shell, &mut cmd, "skillpack", &mut std::io::stdout());
            exit::INIT_OK
        }
    }) {
        Ok(code) => code,
        Err(payload) => {
            eprintln!(
                "fatal: skillpack crashed (panic): {}",
                panic_message(&*payload)
            );
            std::process::exit(exit::INIT_FATAL)
        }
    };
    std::process::exit(code);
}

/// Read a human message out of a caught panic payload so `main`'s
/// `catch_unwind` can name the failure instead of printing a bare "crashed".
/// Handles both `panic!("msg")` (payload `&str`) and `panic!("msg {}", x)`
/// (payload `String`); anything else falls back to a hint to enable a
/// backtrace.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic (re-run with RUST_BACKTRACE=1 for a backtrace)".to_string()
    }
}

/// Configure the structured-diagnostics logger. Everything routed through it
/// (spawn calls, introspection traces) lands on stderr; `--log-format json`
/// switches to one-JSON-object-per-event for CI/log pipelines. Called once at
/// process start, before any work (and before `catch_unwind`, so a panic in
/// the logger can't mask the real error path).
fn init_logging(filter: tracing::level_filters::LevelFilter, format: LogFormat) {
    let builder = tracing_subscriber::fmt()
        .with_max_level(filter)
        .with_writer(std::io::stderr);
    match format {
        // Compact = `LEVEL target: message`; without_time keeps the human
        // output deterministic (a wall-clock timestamp is noise in a CLI and
        // makes byte-diffing debug runs impossible).
        LogFormat::Human => builder.compact().without_time().init(),
        LogFormat::Json => builder.json().init(),
    }
}

/// Emit the `--debug` introspection trace (name/language/has_cli/diag-notes)
/// through the structured logger. Shared by init/update/diff/doctor so the
/// debug contract is one code path and one JSON shape.
fn trace_detected(profile: &types::ProjectProfile) {
    tracing::debug!(
        name = %profile.name,
        language = %profile.language.as_str(),
        secondary = profile.secondary_languages.len(),
        has_cli = profile.has_cli,
        diag_notes = profile.diag.0.len(),
        "detected"
    );
}

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
fn auto_intents(
    profile: &types::ProjectProfile,
    root: &Path,
    triggers: &[String],
    import: Option<&str>,
) -> Result<Vec<(String, types::Intent)>> {
    let primary_name = coerce_kebab(&profile.name);
    let mut out = vec![(
        primary_name.clone(),
        auto_intent(profile, triggers, import)?,
    )];
    for lang in &profile.secondary_languages {
        let lang_str = lang.as_str();
        let skill_name = format!("{primary_name}-{lang_str}");
        out.push((
            skill_name.clone(),
            types::Intent {
                one_line_description: format!("Manage the {lang_str} surface of {primary_name}"),
                when_to_use_phrases: vec![format!("touch the {lang_str} code")],
                invocation_command: None,
                // Library-style: the pattern is derived from the secondary
                // language's manifest (package/crate/module name) so the skill
                // renders the import branch, not the primary CLI's invocation.
                import_pattern: Some(secondary_import_pattern(*lang, root, &skill_name)),
                // Language-correct derived-field overrides: the profile's
                // dominant language (e.g. Rust) must not leak into the
                // secondary skill's category/globs/opencode-mode.
                category: Some(generate::category_hint(*lang).to_string()),
                globs: Some(generate::cursor_globs_hint(*lang)),
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
/// the secondary skill name. One language-shaped pattern per ecosystem so the
/// rendered skill tells an agent how to consume the package in that language.
fn secondary_import_pattern(lang: types::Language, root: &Path, fallback: &str) -> String {
    let name =
        introspect::project_manifest_name(root, lang).unwrap_or_else(|| fallback.to_string());
    match lang {
        types::Language::Rust => {
            format!("use {crate_name}::…;", crate_name = name.replace('-', "_"))
        }
        types::Language::Node => format!("import {{ … }} from '{name}'"),
        types::Language::Python => format!("import {module}", module = name.replace('-', "_")),
        types::Language::Go => format!("import \"{name}\""),
        types::Language::Ruby => format!("require '{name}'"),
        types::Language::Php => format!("require '{name}'"),
        types::Language::Jvm => format!("import {pkg}.*;", pkg = name.replace('-', ".")),
        types::Language::CSharp => format!("using {ns};", ns = name.replace('-', "")),
        types::Language::Zig => format!("const {name} = @import(\"{name}\");"),
        types::Language::Swift => format!("import {name}"),
        types::Language::CCpp => format!("#include <{name}.h>"),
        types::Language::Elixir => format!("import {mod}", mod = pascal_name(&name)),
        types::Language::Deno => format!("import {{ … }} from \"{name}\""),
        types::Language::Nix => format!("{{ inputs, ... }}: inputs.{name}"),
        types::Language::Dart => format!("import 'package:{name}/{name}.dart';"),
        types::Language::Haskell => format!("import {mod}", mod = pascal_name(&name)),
        types::Language::Lua => format!("require(\"{name}\")"),
        types::Language::Julia => format!("using {name}"),
        types::Language::Crystal => format!("require \"./{name}\""),
        types::Language::Clojure => format!("(require '[{name} :refer :all])"),
        types::Language::Ocaml => format!("open {mod}", mod = pascal_name(&name)),
        types::Language::Erlang => format!("application:ensure_all_started({name})."),
        types::Language::R => format!("library({name})"),
        types::Language::Perl => format!("use {name};"),
        types::Language::Unknown => {
            format!("(no standard import form for {name}; document it via `skillpack update`)")
        }
    }
}

/// Upper-camel-case a kebab/snake name for languages whose import statement
/// names a module (`import Foo.Bar`), e.g. `my-lib` → `MyLib`.
fn pascal_name(name: &str) -> String {
    name.split(['-', '_', '.'])
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut c = seg.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
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
fn auto_intent(
    profile: &types::ProjectProfile,
    triggers: &[String],
    import: Option<&str>,
) -> Result<types::Intent> {
    let one_line_description = profile
        .description_hint
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--auto could not derive a description (no README hint found). \
                 Pass --description, or run `skillpack init` interactively."
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
fn bootstrap_intent(
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
fn run_init(
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
    if let Some(code) = handle_list_request(&raw_targets) {
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
        auto_intents(&profile, root, &triggers, import.as_deref())?
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

/// True when an output format is machine-readable (JSON). SARIF/Github are
/// rejected before any init/update/diff path reaches this point (see
/// [`reject_report_format`]), so only Human/Json survive.
fn is_json(format: verify::OutputFormat) -> bool {
    !matches!(format, verify::OutputFormat::Human)
}

/// `--format sarif`/`--format github` only make sense for `verify` (which has
/// file-level results to annotate or upload). init/update/diff accept
/// `--format` for the human/json summary only; reject the report-only formats
/// instead of silently degrading them to JSON (doctor already bails the same
/// way).
fn reject_report_format(format: verify::OutputFormat) -> Result<()> {
    if matches!(
        format,
        verify::OutputFormat::Sarif | verify::OutputFormat::Github | verify::OutputFormat::Junit
    ) {
        bail!(
            "--format sarif/github/junit is only valid for `verify`; this command \
             supports `human` or `json`"
        );
    }
    Ok(())
}

/// Handle the special `--target list` value: print the canonical target names
/// and return the exit code to return early with. Returns `None` when no
/// `list` value was requested.
fn handle_list_request(raw: &[String]) -> Option<i32> {
    if raw.iter().any(|r| r == "list") {
        println!("supported --target values (repeat the flag; `all` = every target):");
        for name in skillpack::cli::target_names() {
            println!("  {name}");
        }
        Some(exit::INIT_OK)
    } else {
        None
    }
}

fn interview_run(profile: &types::ProjectProfile) -> Result<types::Intent> {
    println!("\nNo skillpack.toml found. A few quick questions to scaffold your skill pack.\n");
    let prompter = interview::DialoguerPrompter;
    let intent = interview::run(profile, &prompter).context("during interview")?;
    Ok(intent)
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

/// Refuse to write through a symlink. `rel_path` is root-relative and the
/// write targets `root.join(rel_path)`; if any ancestor directory (or the
/// target itself, when it already exists) is a symlink, `create_dir_all` +
/// `write` would follow it and write outside the project root. Returns an
/// error naming the offending path instead of escaping the repo.
fn ensure_no_symlink_ancestors(root: &Path, rel_path: &str) -> Result<()> {
    let mut cur = root.to_path_buf();
    for comp in Path::new(rel_path).components() {
        cur.push(comp.as_os_str());
        if let Ok(meta) = std::fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                bail!(
                    "refusing to write through a symlink at {}; remove it or re-run in a non-symlinked checkout",
                    cur.display()
                );
            }
        }
    }
    Ok(())
}

fn write_files<'a>(
    root: &Path,
    files: &'a [GeneratedFileOutput],
    force: bool,
) -> Result<(Vec<&'a GeneratedFileOutput>, Vec<&'a GeneratedFileOutput>)> {
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    for f in files {
        let p = root.join(&f.rel_path);
        ensure_no_symlink_ancestors(root, &f.rel_path)?;
        // Collision guard: root-level instruction files (AGENTS.md, CLAUDE.md,
        // GEMINI.md, CONVENTIONS.md) live at repo root (not a skillpack-owned
        // directory). If one already exists and --force was not passed, skip it
        // with a warning so we never silently stomp a hand-written file.
        if is_collision_guarded(&f.rel_path) && p.exists() && !force {
            eprintln!(
                "⚠ {} already exists at {}; skipping (pass --force to overwrite).",
                f.rel_path,
                p.display()
            );
            skipped.push(f);
            continue;
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&p, &f.contents).with_context(|| format!("writing {}", p.display()))?;
        written.push(f);
    }
    Ok((written, skipped))
}

/// Print a preview of which files are new, changed, or unchanged before
/// writing. Only prints when at least one file differs from disk — a
/// fully-clean re-init prints nothing (no noise).
fn print_diff_preview(root: &Path, files: &[GeneratedFileOutput]) {
    let mut new = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = 0u32;
    for f in files {
        let p = root.join(&f.rel_path);
        match std::fs::read_to_string(&p) {
            Ok(existing) if existing == f.contents => unchanged += 1,
            Ok(_) => changed.push(&f.rel_path),
            Err(_) => new.push(&f.rel_path),
        }
    }
    if new.is_empty() && changed.is_empty() {
        return;
    }
    eprintln!("\n📝 distribution file preview:");
    for r in &new {
        eprintln!("   + {r} (new)");
    }
    for r in &changed {
        eprintln!("   ~ {r} (changed)");
    }
    if unchanged > 0 {
        eprintln!("   = {unchanged} file(s) unchanged");
    }
}

// --- pre-commit confirmation (Improvement E: testable) ---------------------
//
// The critical-failure and warnings gates both want a yes/no prompt. The
// interview already pulled in `dialoguer`, but re-attaching to a TTY mid-run
// is finicky in tests, so the pre-commit gate uses a bare stdin readline.
// Wrapping it behind a trait + a thread-local override lets tests inject a
// canned answer instead of driving a real TTY (mirrors interview::Prompter).

trait Confirm {
    fn confirm(&self, prompt: &str) -> bool;

    /// Pre-commit gate: critical failures, "write anyway?" (defaults to NO).
    fn keep_anyway(&self) -> bool {
        self.confirm(&prompt_keep_anyway_text())
    }

    /// Pre-commit gate: warnings present, "proceed?" (defaults to NO).
    fn proceed_with_warnings(&self) -> bool {
        self.confirm(
            "Verification passed with warnings (see above). \
             Write the files? [y/N] ",
        )
    }
}

/// Read one line from stdin; `y`/`yes` (any case) → true, anything else → false.
struct StdinConfirm;
impl Confirm for StdinConfirm {
    fn confirm(&self, prompt: &str) -> bool {
        use std::io::{self, Write};
        let mut input = String::new();
        print!("{prompt}");
        let _ = io::stdout().flush();
        if io::stdin().read_line(&mut input).is_err() {
            return false;
        }
        matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
    }
}

thread_local! {
    /// Tests override this with a canned answer; production leaves the default
    /// `StdinConfirm`.
    static CONFIRM_REF: std::cell::RefCell<Box<dyn Confirm>> =
        std::cell::RefCell::new(Box::new(StdinConfirm));
}

#[cfg(test)]
struct ConfirmGuard;

#[cfg(test)]
impl Drop for ConfirmGuard {
    fn drop(&mut self) {
        // Restore the default on scope exit so a test's override can't leak to
        // a sibling test (Rust runs unit tests in threads, but a thread-local
        // is still restored here for cleanliness).
        CONFIRM_REF.with(|c| c.replace(Box::new(StdinConfirm)));
    }
}

/// The dispatch point the pre-commit gates call. Routes through the
/// (possibly test-overridden) confirm backend.
struct ConfirmDispatch;
impl Confirm for ConfirmDispatch {
    fn confirm(&self, prompt: &str) -> bool {
        CONFIRM_REF.with(|c| c.borrow().confirm(prompt))
    }
}
static CONFIRM: ConfirmDispatch = ConfirmDispatch;

fn prompt_keep_anyway_text() -> String {
    "Critical verification failures were found (see above).\n\
     Write the files anyway? [y/N] "
        .to_string()
}

/// Canned yes/no for tests. Lives at module scope so it can be boxed behind
/// the `Confirm` trait object before its definition point in `with_confirm`.
#[cfg(test)]
struct CannedConfirm(bool);
#[cfg(test)]
impl Confirm for CannedConfirm {
    fn confirm(&self, _p: &str) -> bool {
        self.0
    }
}

/// Run `f` with confirmations overridden so every prompt answers `answer`.
/// Returns `f()`'s result. Test-only: the override is restored on drop.
#[cfg(test)]
pub(crate) fn with_confirm<R>(answer: bool, f: impl FnOnce() -> R) -> R {
    CONFIRM_REF.with(|c| c.replace(Box::new(CannedConfirm(answer))));
    let _g = ConfirmGuard;
    f()
}

fn print_profile(profile: &types::ProjectProfile, to_stderr: bool) {
    // `to_stderr` lets `verify --verbose --format json` show the
    // introspection block without corrupting the JSON body on stdout
    // (stdout stays parseable for `jq`-style CI pipelines).
    macro_rules! emit {
        ($($arg:tt)*) => {
            if to_stderr {
                eprintln!($($arg)*);
            } else {
                println!($($arg)*);
            }
        };
    }
    emit!("introspection");
    emit!("  name:        {}", profile.name);
    emit!("  language:    {}", profile.language.as_str());
    if !profile.secondary_languages.is_empty() {
        let langs: Vec<&str> = profile
            .secondary_languages
            .iter()
            .map(|l| l.as_str())
            .collect();
        emit!("  secondary:   {}", langs.join(", "));
    }
    emit!("  has_cli:     {}", profile.has_cli);
    if let Some(cmd) = &profile.cli_command {
        emit!("  cli_command: {}", cmd.join(" "));
    }
    if let Some(url) = &profile.repo_url {
        emit!("  repo_url:    {url}");
    }
    if let Some(lic) = &profile.license {
        emit!("  license:     {lic}");
    }
    if let Some(hint) = &profile.description_hint {
        if hint.chars().count() > 120 {
            emit!(
                "  desc_hint:   {}…",
                hint.chars().take(120).collect::<String>()
            );
        } else {
            emit!("  desc_hint:   {hint}");
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn run_verify(
    root: &Path,
    verbose: bool,
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    watch: bool,
    template_dir: Option<&Path>,
) -> i32 {
    if watch {
        if format != verify::OutputFormat::Human {
            eprintln!("error: --watch is only valid with --format human");
            return exit::VERIFY_USAGE;
        }
        return run_verify_watch(root, verbose, format, fix, min_score, template_dir);
    }
    match run_verify_inner(root, verbose, format, fix, min_score, template_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_verify_inner(
    root: &Path,
    verbose: bool,
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    template_dir: Option<&Path>,
) -> Result<i32> {
    // Defer to introspect only to recover has_cli + cli_command for the
    // *spawn* stage. CLI *presence* is now derived from the SKILL.md itself
    // (a hand-written pack with a documented invocation should be checked
    // even if no source tree is present — Bug 2 / design §4.2); introspect's
    // `cli_command` only decides whether we can actually spawn `--help` here.
    // If the skill documents a CLI but introspect found none, `verify` emits
    // a warning (not a silent skip) so the gap is visible.
    let profile = introspect::introspect(root).context("introspecting repo for verify")?;
    // Load verify_stdin from skillpack.toml if present (silent fallback to
    // None — verify stays usable on hand-written packs without a config).
    let verify_stdin = Config::load(root)
        .ok()
        .and_then(|opt| opt.and_then(|cfg| cfg.to_intent()))
        .and_then(|intent| intent.verify_stdin);
    if verbose {
        print_profile(
            &profile,
            matches!(
                format,
                verify::OutputFormat::Json | verify::OutputFormat::Sarif
            ),
        );
    }
    let render = |report: &verify::VerifyReport| match format {
        verify::OutputFormat::Human => verify::render(report),
        verify::OutputFormat::Json => format!("{}\n", verify::render_json(report)),
        verify::OutputFormat::Sarif => format!("{}\n", verify::render_sarif(report)),
        verify::OutputFormat::Github => verify::render_github_annotations(report),
        verify::OutputFormat::Junit => verify::render_junit(report),
    };
    let run_verify = || -> Result<verify::VerifyReport> {
        let input = VerifyInput {
            root: root.to_path_buf(),
            spawn_root: root.to_path_buf(),
            cli_command: profile.cli_command.clone(),
            profile_name: Some(coerce_kebab(&profile.name)),
            verify_stdin: verify_stdin.clone(),
            repo_url: profile.repo_url.clone(),
        };
        verify::run(&input)
    };

    let report = run_verify()?;
    // Without `--fix`, render + exit on the single report. With `--fix`,
    // collect the mechanically-fixable drifts (warn OR error severities),
    // apply each, then re-render from the post-fix report. The pre-fix
    // report is NOT printed when `--fix` takes effect — the post-fix report
    // surfaces what (if anything) still drifts, plus a one-line summary of
    // the files rewritten.
    let (final_report, applied_summary) = if !fix {
        (report, None)
    } else {
        let actions: Vec<_> = report
            .results
            .iter()
            .filter(|r| {
                matches!(
                    r.severity,
                    verify::result::Severity::Warn | verify::result::Severity::Error
                )
            })
            .filter_map(|r| verify::fix::action_for(&r.check_id).map(|a| (a, r.location.clone())))
            .collect();
        if actions.is_empty() {
            (report, None)
        } else {
            let mut written: Vec<String> = Vec::new();
            for (action, loc) in actions {
                let outcome = verify::fix::apply(action, root, loc.as_ref(), template_dir)
                    .context("applying a `--fix` action")?;
                written.extend(outcome.files_written);
            }
            let summary: Vec<String> = verify::fix::FixOutcome {
                files_written: written,
            }
            .unique_sorted();
            let summary_line = format!(
                "✓ applied {} fix(es), wrote: {}",
                summary.len(),
                summary.join(", ")
            );
            (run_verify()?, Some(summary_line))
        }
    };

    if let Some(line) = applied_summary {
        eprintln!("{line}");
    }
    print!("{}", render(&final_report));
    // Exit precedence: critical failure (1) > score-below-min (2) > ok (0).
    // A structurally broken pack is more severe than a low score and must
    // surface first; the score gate fires only when structure passed.
    let code = if final_report.has_critical_failure() {
        exit::VERIFY_FAIL
    } else if let Some(min) = min_score {
        let actual = final_report.discoverability_score();
        if actual < min {
            eprintln!(
                "verify: discoverability score {actual} is below the --min-score {min} threshold"
            );
            exit::VERIFY_SCORE_BELOW_MIN
        } else {
            exit::VERIFY_OK
        }
    } else {
        exit::VERIFY_OK
    };
    Ok(code)
}

/// `verify --watch` — re-runs verify on every file change (debounced).
///
/// Uses `notify` to watch the project root. On each debounced event batch,
/// clears the terminal, re-runs a single verify cycle, and prints the
/// report. Ctrl-C terminates the process directly (standard SIGINT
/// behavior — no clean-shutdown handler is installed).
fn run_verify_watch(
    root: &Path,
    verbose: bool,
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    template_dir: Option<&Path>,
) -> i32 {
    use notify::{EventKind, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("fatal: cannot initialize file watcher: {e}");
            return exit::INIT_FATAL;
        }
    };

    // Watch the project root recursively. Ignore common noise dirs.
    if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
        eprintln!("fatal: cannot watch {}: {e}", root.display());
        return exit::INIT_FATAL;
    }

    eprintln!(
        "🔍 watching {} for changes (Ctrl-C to stop)…\n",
        root.display()
    );

    let _ = run_verify_single(root, verbose, format, fix, min_score, template_dir);

    let debounce = Duration::from_secs(1);
    let mut last_event: Option<Instant> = None;

    // Skip events from noisy paths (target/, .git/, node_modules/).
    let is_noise = |path: &std::path::Path| -> bool {
        path.components().any(|c| {
            matches!(
                c,
                std::path::Component::Normal(s)
                    if s == "target" || s == ".git" || s == "node_modules"
            )
        })
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ok(event)) => {
                // Only react to content changes, not attribute-only.
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) && !event.paths.iter().all(|p| is_noise(p))
                {
                    last_event = Some(Instant::now());
                }
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {
                // Debounce: fire when 1s has elapsed since the last event
                // with no new events.
                if let Some(t) = last_event {
                    if t.elapsed() >= debounce {
                        last_event = None;
                        // Clear screen for a clean re-render (only when
                        // stdout is a real terminal — never emit ANSI to a
                        // pipe or captured log).
                        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                            print!("\x1b[2J\x1b[H");
                        }
                        let _ =
                            run_verify_single(root, verbose, format, fix, min_score, template_dir);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Channel closed (watcher dropped) — exit.
                break;
            }
        }
    }

    eprintln!("\nstopped.");
    exit::VERIFY_OK
}

/// Run a single verify cycle and print the report. Extracted from
/// `run_verify_watch` so it's testable independently of the watcher.
fn run_verify_single(
    root: &Path,
    verbose: bool,
    format: verify::OutputFormat,
    fix: bool,
    min_score: Option<u8>,
    template_dir: Option<&Path>,
) -> i32 {
    // Exit-code parity with the non-watch `run_verify`: an unrecoverable
    // introspect/render error is INIT_FATAL (3), not VERIFY_FAIL (1). The
    // watcher loop discards this per-cycle code (it re-runs on each change),
    // but the mapping must not silently disagree with a standalone run.
    match run_verify_inner(root, verbose, format, fix, min_score, template_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            exit::INIT_FATAL
        }
    }
}
/// `skillpack doctor` — diagnose why introspection chose what it did.
/// Read-only: prints the detected profile + the decision trace (`diag`),
/// never writes files. The trace is empty until candidate fns push notes
/// (the `detect_*` falsy branches); doctor surfaces exactly why `has_cli`
/// came out false so the maintainer can act.
fn run_doctor(root: &Path, verbose: bool, format: crate::verify::OutputFormat) -> i32 {
    match run_doctor_inner(root, verbose, format) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_doctor_inner(
    root: &Path,
    verbose: bool,
    format: crate::verify::OutputFormat,
) -> Result<i32> {
    let profile = introspect::introspect(root).context("introspecting repo for doctor")?;

    match format {
        crate::verify::OutputFormat::Json => {
            // The serialized `ProjectProfile` IS the doctor JSON report —
            // including the `diag` decision trace + every detected field,
            // exactly what a consumer wants to scrape. No envelope wrapping;
            // the consumer reads fields by name. Exits 0 (doctor is
            // read-only diagnostic, non-gating — matches human form).
            //
            // A `verify_category_preview` field mirrors the human-mode
            // category/target preview so JSON consumers get the same "what
            // would `verify` check" signal without parsing prose.
            let mut v =
                serde_json::to_value(&profile).context("serializing doctor profile to JSON")?;
            let targets: Vec<String> = Target::value_variants()
                .iter()
                .filter_map(|t| t.to_possible_value())
                .map(|pv| pv.get_name().to_string())
                .collect();
            v["verify_category_preview"] = serde_json::json!({
                "targets": targets,
                "categories": if profile.has_cli {
                    vec!["discovery", "invocation"]
                } else {
                    vec!["discovery"]
                },
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&v).context("serializing doctor profile to JSON")?
            );
        }
        crate::verify::OutputFormat::Human => render_doctor_human(&profile, verbose),
        crate::verify::OutputFormat::Sarif
        | crate::verify::OutputFormat::Github
        | crate::verify::OutputFormat::Junit => {
            bail!("doctor does not support this format; use `verify` for machine-readable reports")
        }
    }

    // Doctor never writes; always exits 0.
    Ok(exit::VERIFY_OK)
}

/// Render the human-facing diagnosis. Lifted verbatim from the pre-format
/// behavior so `doctor` (no flag) and `doctor --format human` are byte-identical.
fn render_doctor_human(profile: &types::ProjectProfile, verbose: bool) {
    trace_detected(profile);
    // Reuse the same profile block --verbose prints so doctor's output starts
    // from a known place.
    if verbose {
        print_profile(profile, false);
    } else {
        println!("skillpack doctor");
        println!("  name:     {}", profile.name);
        println!("  language: {}", profile.language.as_str());
        if !profile.secondary_languages.is_empty() {
            let langs: Vec<&str> = profile
                .secondary_languages
                .iter()
                .map(|l| l.as_str())
                .collect();
            println!("  secondary: {}", langs.join(", "));
        }
        println!("  has_cli:  {}", profile.has_cli);
        if let Some(cmd) = &profile.cli_command {
            println!("  cli:      {}", cmd.join(" "));
        }
    }

    println!();
    if profile.diag.0.is_empty() {
        if profile.has_cli {
            println!("decision trace: (empty; CLI detected cleanly, no falsy branches fired)");
        } else {
            println!("decision trace: (empty; no candidate notes were pushed)");
            println!();
            println!("hint: candidate fns only push notes on falsy branches, so an empty trace");
            println!("      means either detection succeeded silently or this language has no");
            println!("      probed candidate. Check --verbose for the raw profile.");
        }
    } else {
        println!("decision trace ({}):", profile.diag.0.len());
        for note in &profile.diag.0 {
            if note.note.contains("run `") {
                println!("  💡 [{}] {}", note.stage, note.note);
            } else {
                println!("  [{}] {}", note.stage, note.note);
            }
        }
    }

    // Discoverability category preview: what `verify` would check, grouped
    // by namespace. doctor is read-only and runs on pre-init repos (no pack
    // generated yet), so we can't run the real verify — but we can show the
    // check-id namespaces so the user knows what to expect after `init`.
    println!();
    println!("verify category preview (run `skillpack verify` after `init` for the real score):");
    println!("  discovery.*: structural validation of every generated file per ecosystem");
    println!("    (Claude plugin + native skills, Codex, Cursor, OpenCode, Copilot, AGENTS.md,");
    println!("     CLAUDE.md, GEMINI.md, Windsurf, Aider, Cline, Roo Code, Kilo Code, Goose,");
    println!("     Qoder, Continue, Augment, Amazon Q)");
    if profile.has_cli {
        println!("  invocation.*: runs the CLI: --help, flag drift, subcommand drift");
        println!("    --version drift (advisory)");
    } else {
        println!("  invocation.*: N/A (no CLI detected; checks will be skipped)");
    }

    println!();
    println!(
        "next step: `skillpack init --target all --auto` to scaffold guidance for every ecosystem."
    );
}

/// `skillpack update` — incrementally regenerate distribution files from an
/// existing `skillpack.toml`. No interview, no pre-commit verify gate. Reads
/// the committed config, re-introspects, re-renders every target, and writes
/// ONLY files whose content changed. For frontmatter-bearing files the body
/// is preserved via the same splice `--fix` uses; frontmatter is regenerated
/// wholesale. Returns exit 0 on success.
fn run_update(
    root: &Path,
    verbose: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> i32 {
    if let Some(code) = handle_list_request(&raw_targets) {
        return code;
    }
    match run_update_inner(root, verbose, raw_targets, force, template_dir, format) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

/// Result of comparing one rendered file against its on-disk content.
struct CandidateResult<'a> {
    file: &'a GeneratedFileOutput,
    /// What we would write (spliced frontmatter + preserved body for
    /// frontmatter files; raw render for fully-generated files).
    candidate: String,
    /// On-disk content (BOM-stripped, CRLF-normalized).
    committed: Option<String>,
    /// None = file not on disk (new). Some = file exists.
    status: CandidateStatus,
    /// True if the AGENTS.md collision guard skipped this file.
    held: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum CandidateStatus {
    /// File not on disk — would be created.
    Missing,
    /// Committed == candidate — no drift.
    Clean,
    /// Committed != candidate — drift detected.
    Drifted,
}

/// Compute candidates for each rendered file, comparing against on-disk
/// content. Shared by `update` (writes drifted) and `diff` (reports only).
/// The AGENTS.md collision guard mirrors `init`: skip if it exists and
/// `--force` is not passed.
fn compute_candidates<'f>(
    root: &Path,
    files: &'f [GeneratedFileOutput],
    force: bool,
) -> Result<Vec<CandidateResult<'f>>> {
    let mut results = Vec::with_capacity(files.len());
    for file in files {
        let disk_path = root.join(&file.rel_path);

        // Read the committed content (normalized: CRLF -> LF, BOM stripped) so
        // both the frontmatter splice and the collision guard compare against
        // the same bytes the agent actually reads.
        let committed = if disk_path.exists() {
            let raw = std::fs::read_to_string(&disk_path)
                .with_context(|| format!("reading {}", disk_path.display()))?
                .replace("\r\n", "\n");
            Some(skillpack::verify::discovery::strip_bom(&raw).to_string())
        } else {
            None
        };

        let candidate = if is_frontmatter_target(&file.rel_path) {
            match &committed {
                // Existing file: splice the fresh frontmatter onto the
                // committed body so a maintainer's hand-tailored prose
                // survives regeneration.
                Some(committed) => {
                    let fresh_fm = skillpack::verify::fix::split_frontmatter(&file.contents)
                        .map(|(fm, _body)| fm)
                        .unwrap_or_else(|| file.contents.clone());
                    let preserved_body = skillpack::verify::fix::split_frontmatter(committed)
                        .map(|(_fm, body)| body)
                        .unwrap_or_default();
                    format!("{fresh_fm}\n{preserved_body}")
                }
                // New file (not on disk yet): the full fresh render. The old
                // splice reused the same path for both, so a target added
                // since the last `update` was written as frontmatter-only
                // (empty preserved body) — the body prose silently dropped
                // and invisible to `diff`/`verify` afterwards.
                None => file.contents.clone(),
            }
        } else {
            file.contents.clone()
        };

        // Root-level plain instruction files are guarded: without --force, a
        // file that differs from the fresh render is HELD (it may be
        // hand-written). A file that already matches is left clean — this is
        // what lets a skillpack-generated AGENTS.md be tracked without noise
        // while still protecting a hand-written one.
        if is_collision_guarded(&file.rel_path) && committed.is_some() && !force {
            let held = committed.as_deref() != Some(candidate.as_str());
            results.push(CandidateResult {
                file,
                candidate,
                committed,
                status: CandidateStatus::Clean,
                held,
            });
            continue;
        }

        let status = match &committed {
            None => CandidateStatus::Missing,
            Some(c) if *c == candidate => CandidateStatus::Clean,
            Some(_) => CandidateStatus::Drifted,
        };
        results.push(CandidateResult {
            file,
            candidate,
            committed,
            status,
            held: false,
        });
    }
    Ok(results)
}

/// The targets whose distribution files are already present on disk, in
/// canonical [`Target`] declaration order. Used as the default for
/// `update`/`diff` when no `--target` is given, so those commands refresh or
/// check the whole existing distribution instead of silently limiting
/// themselves to the Claude target (the old default) and leaving every other
/// ecosystem stale.
///
/// Both the per-ecosystem directory markers and the root-level single-file
/// targets (AGENTS.md, CLAUDE.md, GEMINI.md, CONVENTIONS.md,
/// `.goose/instructions.md`, `.github/copilot-instructions.md`) are probed by
/// existence. Probing the single files no longer means they are silently
/// dropped from the default run: `update`/`diff` hold a hand-written copy (or
/// one that drifted) via the collision guard, and report a clean
/// skillpack-generated copy as unchanged — so a generated root file is
/// tracked instead of ignored.
fn detect_present_targets(root: &Path) -> Vec<Target> {
    let mut present = Vec::new();
    for (target, marker) in [
        (Target::Claude, ".claude-plugin"),
        (Target::Cursor, ".cursor/rules"),
        (Target::Codex, ".codex/skills"),
        (Target::OpenCode, ".opencode/agents"),
        (Target::Windsurf, ".windsurf/rules"),
        (Target::Cline, ".clinerules"),
        (Target::Roo, ".roo/rules"),
        (Target::Kilo, ".kilocode/rules"),
        (Target::Qoder, ".qoder/rules"),
        (Target::Continue, ".continue/rules"),
        (Target::Augment, ".augment/rules"),
        (Target::AmazonQ, ".amazonq/rules"),
        (
            Target::Copilot,
            crate::verify::schema::COPILOT_INSTRUCTIONS_PATH,
        ),
        (Target::AgentsMd, crate::verify::schema::AGENTS_MD_PATH),
        (Target::ClaudeMd, crate::verify::schema::CLAUDE_MD_PATH),
        (Target::Gemini, crate::verify::schema::GEMINI_MD_PATH),
        (Target::Aider, crate::verify::schema::CONVENTIONS_MD_PATH),
        (
            Target::Goose,
            crate::verify::schema::GOOSE_INSTRUCTIONS_PATH,
        ),
    ] {
        if root.join(marker).exists() {
            present.push(target);
        }
    }
    present
}

/// Resolve the default target set for `update`/`diff` (and `add`, which
/// delegates to `update`). Prefers the targets already present on disk; when
/// none are detected (a committed config with no generated files yet), falls
/// back to `all` so a refresh regenerates the full distribution rather than
/// silently limiting itself to Claude.
fn default_refresh_targets(root: &Path) -> Result<Vec<Target>> {
    let present = detect_present_targets(root);
    if present.is_empty() {
        resolve_targets(&["all".to_string()])
    } else {
        Ok(present)
    }
}

/// Shared preamble: introspect, load config, resolve targets, render.
/// Returns the profile, the full skill list (one entry for single-skill
/// packs, one per `[[skills]]` entry for multi-skill packs), and every
/// rendered file — pack-level files from the primary skill, per-skill files
/// for every skill.
#[allow(clippy::type_complexity)]
fn render_from_config(
    root: &Path,
    raw_targets: &[String],
    template_dir: Option<&Path>,
) -> Result<(
    types::ProjectProfile,
    Vec<(String, types::Intent)>,
    Vec<GeneratedFileOutput>,
)> {
    let profile = introspect::introspect(root).context("introspecting repo")?;
    let existing_cfg = Config::load(root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no skillpack.toml at {}: a committed config is required.\n\
             To fix: run `skillpack init` first to seed it.",
            Config::path(root).display()
        )
    })?;
    let skills = existing_cfg.to_intents();
    if skills.is_empty() {
        bail!(
            "skillpack.toml at {} is missing its [skill] table.\n\
         To fix: re-run `skillpack init` interactively to regenerate the config.",
            Config::path(root).display()
        );
    }
    let targets = if raw_targets.is_empty() {
        default_refresh_targets(root)?
    } else {
        resolve_targets(raw_targets)?
    };
    let files = render_all(&profile, &skills, &targets, template_dir)
        .context("rendering distribution files")?;
    Ok((profile, skills, files))
}

fn run_update_inner(
    root: &Path,
    verbose: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> Result<i32> {
    reject_report_format(format)?;
    let (profile, skills, files) = render_from_config(root, &raw_targets, template_dir)?;
    if verbose {
        print_profile(&profile, false);
    }
    trace_detected(&profile);
    let results = compute_candidates(root, &files, force)?;

    let mut written: Vec<&GeneratedFileOutput> = Vec::new();
    let mut unchanged = 0usize;
    let mut skipped: Vec<&GeneratedFileOutput> = Vec::new();

    for r in &results {
        if r.held {
            skipped.push(r.file);
            continue;
        }
        match r.status {
            CandidateStatus::Missing => {
                let disk_path = root.join(&r.file.rel_path);
                ensure_no_symlink_ancestors(root, &r.file.rel_path)?;
                if let Some(parent) = disk_path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("creating parent dir for {}", disk_path.display())
                    })?;
                }
                std::fs::write(&disk_path, &r.candidate)
                    .with_context(|| format!("writing {}", disk_path.display()))?;
                written.push(r.file);
            }
            CandidateStatus::Clean => {
                unchanged += 1;
            }
            CandidateStatus::Drifted => {
                let disk_path = root.join(&r.file.rel_path);
                ensure_no_symlink_ancestors(root, &r.file.rel_path)?;
                std::fs::write(&disk_path, &r.candidate)
                    .with_context(|| format!("writing {}", disk_path.display()))?;
                written.push(r.file);
            }
        }
    }

    // Update skillpack.toml with current introspection (version/name may have
    // changed) — but only when the serialized form actually differs, so a
    // no-op `update` doesn't churn the config's mtime or rewrite a
    // hand-formatted file.
    Config::from_intents(&skills).save_if_changed(root)?;

    // Summary.
    if is_json(format) {
        println!(
            "{}",
            serde_json::json!({
                "command": "update",
                "written": written.iter().map(|f| &f.rel_path).collect::<Vec<_>>(),
                "unchanged": unchanged,
                "skipped": skipped.iter().map(|f| &f.rel_path).collect::<Vec<_>>(),
            })
        );
        return Ok(exit::INIT_OK);
    }
    println!(
        "✓ updated {} file(s), {} unchanged, under {}:",
        written.len(),
        unchanged,
        root.display()
    );
    for f in &written {
        println!("   - {}", f.rel_path);
    }
    if unchanged > 0 {
        eprintln!("  ({unchanged} file(s) already up-to-date)");
    }
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

/// `skillpack diff` — check whether distribution files are stale. Report
/// drifted/missing files and exit 1 if any. A CI gate for stale artifacts.
fn run_diff(
    root: &Path,
    verbose: bool,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> i32 {
    if let Some(code) = handle_list_request(&raw_targets) {
        return code;
    }
    match run_diff_inner(root, verbose, &raw_targets, force, template_dir, format) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            exit::INIT_FATAL
        }
    }
}

fn run_diff_inner(
    root: &Path,
    verbose: bool,
    raw_targets: &[String],
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> Result<i32> {
    reject_report_format(format)?;
    let (profile, _skills, files) = render_from_config(root, raw_targets, template_dir)?;
    if verbose {
        print_profile(&profile, false);
    }
    trace_detected(&profile);
    let results = compute_candidates(root, &files, force)?;

    let mut drifted = 0usize;
    let mut missing = 0usize;
    let mut unchanged = 0usize;
    let mut held = 0usize;

    for r in &results {
        if r.held {
            held += 1;
            eprintln!("  held: {} (pass --force to check)", r.file.rel_path);
            continue;
        }
        match r.status {
            CandidateStatus::Missing => {
                missing += 1;
                eprintln!("  missing: {}", r.file.rel_path);
            }
            CandidateStatus::Clean => {
                unchanged += 1;
            }
            CandidateStatus::Drifted => {
                drifted += 1;
                let first_diff =
                    first_differing_line(r.committed.as_deref().unwrap_or_default(), &r.candidate);
                eprintln!("  drifted: {} (first diff: {first_diff})", r.file.rel_path);
            }
        }
    }

    if is_json(format) {
        println!(
            "{}",
            serde_json::json!({
                "command": "diff",
                "clean": drifted == 0 && missing == 0,
                "drifted": drifted,
                "missing": missing,
                "unchanged": unchanged,
                "held": held,
            })
        );
        return Ok(if drifted == 0 && missing == 0 {
            exit::INIT_OK
        } else {
            exit::DIFF_DRIFT
        });
    }
    if drifted == 0 && missing == 0 {
        println!(
            "✓ all {unchanged} file(s) up-to-date ({})",
            if held > 0 {
                format!("{held} held")
            } else {
                "none held".into()
            },
        );
        Ok(exit::INIT_OK)
    } else {
        eprintln!(
            "\n✗ {drifted} drifted, {missing} missing, {unchanged} up-to-date{}: \
             run `skillpack update{}` to fix.",
            if held > 0 {
                format!(", {held} held")
            } else {
                String::new()
            },
            if force { " --force" } else { "" },
        );
        Ok(exit::DIFF_DRIFT)
    }
}

/// Return the first line that differs between `committed` and `candidate`
/// (with `-`/`+` prefix). For `diff`'s CI gate output — avoids pulling a
/// diff crate for what a char scan suffices.
fn first_differing_line(committed: &str, candidate: &str) -> String {
    for (c, n) in committed.lines().zip(candidate.lines()) {
        if c != n {
            return format!("- {c}\n+ {n}");
        }
    }
    let extra = if committed.lines().count() > candidate.lines().count() {
        committed
    } else {
        candidate
    };
    extra
        .lines()
        .nth(committed.lines().count().min(candidate.lines().count()))
        .map(|l| format!("± {l}"))
        .unwrap_or_else(|| "(no lines differ)".into())
}

/// True if the given rel-path is a frontmatter-bearing file that needs body
/// preservation during `update` (SKILL.md, cursor .mdc, opencode .md).
/// Plain-markdown files (AGENTS.md, copilot-instructions.md, CLAUDE.md,
/// GEMINI.md, CONVENTIONS.md, `.goose/instructions.md`) and the plain rule
/// files (`.clinerules/`, `.roo/rules/`, `.kilocode/rules/`) are NOT included
/// — `split_frontmatter` would return None on them.
fn is_frontmatter_target(rel_path: &str) -> bool {
    if rel_path.starts_with(".clinerules/")
        || rel_path.starts_with(".roo/rules/")
        || rel_path.starts_with(".kilocode/rules/")
        || rel_path.starts_with(".qoder/rules/")
        || rel_path.starts_with(".continue/rules/")
        || rel_path.starts_with(".augment/rules/")
        || rel_path.starts_with(".amazonq/rules/")
    {
        return false;
    }
    rel_path.ends_with("SKILL.md")
        || rel_path.ends_with(".mdc")
        || (rel_path.ends_with(".md")
            && !rel_path.ends_with("AGENTS.md")
            && !rel_path.ends_with("copilot-instructions.md")
            && !rel_path.ends_with("CLAUDE.md")
            && !rel_path.ends_with("GEMINI.md")
            && !rel_path.ends_with("CONVENTIONS.md")
            && !rel_path.ends_with("instructions.md"))
}

/// True if the given rel-path is a plain instructions file that agents
/// commonly hand-write, which should be protected by the collision guard when
/// --force is omitted. Copilot instructions live under `.github/` rather than
/// the repo root, but are equally likely to be hand-authored (they are a
/// well-known `docs.github.com/copilot` convention), so they are guarded too.
fn is_collision_guarded(rel_path: &str) -> bool {
    matches!(
        rel_path,
        crate::verify::schema::AGENTS_MD_PATH
            | crate::verify::schema::CLAUDE_MD_PATH
            | crate::verify::schema::GEMINI_MD_PATH
            | crate::verify::schema::CONVENTIONS_MD_PATH
            | crate::verify::schema::GOOSE_INSTRUCTIONS_PATH
            | crate::verify::schema::COPILOT_INSTRUCTIONS_PATH
    )
}

/// `skillpack add` — append a new skill to an existing `skillpack.toml` pack
/// and regenerate the distribution files. The new skill's intent comes from
/// the interview (or the `--non-interactive` bootstrap flags); the existing
/// skills are left untouched.
#[allow(clippy::too_many_arguments)]
fn run_add(
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
    if let Some(code) = handle_list_request(&raw_targets) {
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

/// Validate a user-supplied `skillpack add <name>` and coerce it to kebab-case.
/// Rejects empty or letter-free input that would silently coerce to the `tool`
/// fallback (`coerce_kebab("")` / `coerce_kebab("!!!")` / `coerce_kebab("123")`
/// all yield `tool`) — or worse, collide with another garbage name that also
/// coerces to `tool`. The user gets an actionable error instead of a surprise
/// `tool` skill.
fn coerce_add_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("skill name must not be empty (run `skillpack add <name>` with a kebab-case name)");
    }
    if !trimmed.chars().any(|c| c.is_ascii_alphabetic()) {
        bail!(
            "skill name `{trimmed}` contains no letters; use a name like `my-tool` \
             (skillpack coerces it to kebab-case)"
        );
    }
    Ok(coerce_kebab(name))
}

/// `skillpack remove <name>` — drop a skill from the pack. Edits the
/// committed `skillpack.toml`, deletes the orphaned per-skill distribution
/// files, and regenerates the remaining targets. Symmetric with `add`.
fn run_remove(
    root: &Path,
    verbose: bool,
    name: &str,
    raw_targets: Vec<String>,
    force: bool,
    template_dir: Option<&Path>,
    format: verify::OutputFormat,
) -> i32 {
    if let Some(code) = handle_list_request(&raw_targets) {
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

/// `skillpack config` — validate (or summarize) the committed `skillpack.toml`.
/// `--validate` exits non-zero on an invalid config (mirrors the load-time
/// structural invariants: kebab-case names, well-formed TOML).
fn run_config(root: &Path, validate: bool) -> i32 {
    match Config::load(root) {
        Ok(Some(cfg)) => {
            let intents = cfg.to_intents();
            if validate {
                println!("skillpack.toml is valid ({} skill(s))", intents.len());
            } else {
                println!("skillpack.toml summary:");
                println!("  skills: {}", intents.len());
                for (name, intent) in &intents {
                    println!("    - {name}: {}", intent.one_line_description);
                }
                if let Some(a) = &cfg.defaults.author {
                    println!("  defaults.author: {a}");
                }
                if let Some(l) = &cfg.defaults.license {
                    println!("  defaults.license: {l}");
                }
            }
            exit::INIT_OK
        }
        Ok(None) => {
            eprintln!(
                "no skillpack.toml at {} (run `skillpack init` first)",
                Config::path(root).display()
            );
            exit::INIT_FATAL
        }
        Err(e) => {
            eprintln!("invalid skillpack.toml: {e:#}");
            exit::INIT_FATAL
        }
    }
}

#[cfg(test)]
mod confirm_tests {
    use super::*;

    #[test]
    fn keep_anyway_routes_through_overridable_confirm() {
        // A canned "no" aborts; a canned "yes" proceeds. Both go through the
        // same CONFIRM dispatch the real pre-commit gate uses (Improvement E).
        assert!(!with_confirm(false, || CONFIRM.keep_anyway()));
        assert!(with_confirm(true, || CONFIRM.keep_anyway()));
    }

    #[test]
    fn proceed_with_warnings_routes_through_overridable_confirm() {
        assert!(!with_confirm(false, || CONFIRM.proceed_with_warnings()));
        assert!(with_confirm(true, || CONFIRM.proceed_with_warnings()));
    }

    // `--auto` on a crate with a renamed [[bin]] (fd-find ships `fd`) must
    // document the RESOLVED binary's stem, not the package name — otherwise
    // the pre-commit verify fails spawning `fd-find --help` and `--auto`
    // refuses to write. Runtime-style argvs (`go run .`, `node script.js`)
    // fall back to the package name.
    #[test]
    fn auto_intent_uses_resolved_binary_stem_for_renamed_bins() {
        // Point at a real file so the resolver trusts it.
        let bin = std::env::current_exe().unwrap(); // an existing executable
        let stem = bin.file_stem().unwrap().to_str().unwrap().to_string();
        let profile = types::ProjectProfile {
            name: "fd-find".into(),
            language: types::Language::Rust,
            secondary_languages: Vec::new(),
            has_cli: true,
            cli_command: Some(vec![bin.to_string_lossy().to_string(), "--help".into()]),
            cli_help_output: Some("usage".into()),
            cli_subcommand_tree: Vec::new(),
            repo_url: None,
            license: Some("MIT".into()),
            version: None,
            authors: None,
            description_hint: Some("Find files by name".into()),
            diag: types::DiagTrace::default(),
        };
        let intent = auto_intent(&profile, &[], None).unwrap();
        assert_eq!(
            intent.invocation_command.as_deref(),
            Some(stem.as_str()),
            "renamed bin must be documented as its real name"
        );

        // `go run .` — a bare runtime name is NOT a resolvable file → package
        // name wins.
        let mut go = profile.clone();
        go.cli_command = Some(vec!["go".into(), "run".into(), ".".into()]);
        let intent = auto_intent(&go, &[], None).unwrap();
        assert_eq!(intent.invocation_command.as_deref(), Some("fd-find"));
    }

    // Regression: a README hint with a multibyte char across byte 120 must
    // not panic. The old `&hint[..120]` byte-slice hit "byte index 120 is
    // not a char boundary" → catch_unwind → false INIT_FATAL exit.
    #[test]
    fn print_profile_multibyte_desc_hint_does_not_panic() {
        // 118 ASCII chars + a 3-byte emoji = 121 bytes; byte 120 lands mid-char.
        let mut hint = "x".repeat(118);
        hint.push('🦀');
        let profile = types::ProjectProfile {
            name: "test".into(),
            language: types::Language::Rust,
            secondary_languages: Vec::new(),
            has_cli: false,
            cli_command: None,
            cli_help_output: None,
            cli_subcommand_tree: Vec::new(),
            repo_url: None,
            license: Some("MIT".into()),
            version: None,
            authors: None,
            description_hint: Some(hint),
            diag: types::DiagTrace::default(),
        };
        // Must not panic.
        print_profile(&profile, false);
    }

    // The root-file collision guard is content-aware: an on-disk root file
    // that already matches the fresh render is clean (not held), while one
    // that differs is held without --force (protected as hand-written) and
    // becomes drifted with --force.
    #[test]
    fn compute_candidates_holds_divergent_root_file_and_passes_clean() {
        let root = scratch_dir("guard");
        let clean = GeneratedFileOutput {
            rel_path: "AGENTS.md".to_string(),
            contents: "# skillpack\n\ngenerated\n".to_string(),
        };

        std::fs::write(root.join("AGENTS.md"), "# skillpack\n\ngenerated\n").unwrap();
        let results = compute_candidates(&root, std::slice::from_ref(&clean), false).unwrap();
        assert!(
            !results[0].held,
            "identical AGENTS.md must be clean, not held"
        );
        assert_eq!(results[0].status, CandidateStatus::Clean);

        std::fs::write(root.join("AGENTS.md"), "# hand-written\n\ncustom\n").unwrap();
        let results = compute_candidates(&root, std::slice::from_ref(&clean), false).unwrap();
        assert!(
            results[0].held,
            "divergent AGENTS.md must be held without --force"
        );

        let results = compute_candidates(&root, std::slice::from_ref(&clean), true).unwrap();
        assert!(!results[0].held, "--force must release the guard");
        assert_eq!(results[0].status, CandidateStatus::Drifted);
        let _ = std::fs::remove_dir_all(&root);
    }

    // `skillpack add` must reject names that would silently coerce to the
    // `tool` fallback (empty, punctuation-only, digits-only) instead of
    // creating a surprise `tool` skill — or colliding with another garbage
    // name that also coerces to `tool`.
    #[test]
    fn coerce_add_name_rejects_garbage_and_coerces_valid() {
        assert!(coerce_add_name("").is_err());
        assert!(coerce_add_name("   ").is_err());
        assert!(coerce_add_name("!!!").is_err());
        assert!(coerce_add_name("123").is_err());
        assert!(coerce_add_name("123-456").is_err());

        // Valid names coerce to kebab; the literal `tool` is allowed.
        assert_eq!(coerce_add_name("My Tool").unwrap(), "my-tool");
        assert_eq!(coerce_add_name("tool").unwrap(), "tool");
        assert_eq!(coerce_add_name("123-foo").unwrap(), "foo");
    }

    // `--format sarif`/`--format github` only make sense for `verify`;
    // init/update/diff must reject them rather than silently emit JSON.
    #[test]
    fn reject_report_format_allows_only_human_and_json() {
        assert!(reject_report_format(verify::OutputFormat::Human).is_ok());
        assert!(reject_report_format(verify::OutputFormat::Json).is_ok());
        assert!(reject_report_format(verify::OutputFormat::Sarif).is_err());
        assert!(reject_report_format(verify::OutputFormat::Github).is_err());
    }

    // `main`'s catch_unwind names the panic payload (`panic!("msg")` carries
    // a `&str`, `panic!("msg {}", x)` a `String`) instead of printing a bare
    // "crashed".
    #[test]
    fn panic_message_reads_str_and_string_payloads() {
        let s: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(panic_message(&*s), "boom");
        let s: Box<dyn std::any::Any + Send> = Box::new("boom".to_string());
        assert_eq!(panic_message(&*s), "boom");
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "skillpack-targets-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // `update`/`diff` default to the ecosystems already generated, not just
    // Claude. Directory markers AND the root-level single-file targets are
    // probed by existence, so a generated AGENTS.md is tracked (and later
    // held-or-refreshed by the collision guard) instead of silently dropped.
    #[test]
    fn detect_present_targets_finds_directory_and_single_file_targets() {
        let root = scratch_dir("present");
        for d in [".claude-plugin", ".cursor/rules", ".codex/skills"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("AGENTS.md"), "# hand-written").unwrap();

        let present = detect_present_targets(&root);
        assert!(present.contains(&Target::Claude));
        assert!(present.contains(&Target::Cursor));
        assert!(present.contains(&Target::Codex));
        assert!(
            present.contains(&Target::AgentsMd),
            "an existing AGENTS.md must be probed as present"
        );
        assert!(
            !present.contains(&Target::Copilot),
            "an absent copilot-instructions.md must not be probed as present"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // When nothing has been generated yet (config committed, files absent),
    // the default falls back to `all` so `update`/`diff` cover the full
    // distribution instead of silently limiting themselves to Claude.
    #[test]
    fn default_refresh_targets_falls_back_to_all_when_nothing_present() {
        let root = scratch_dir("empty");
        let targets = default_refresh_targets(&root).unwrap();
        assert_eq!(targets.len(), 18, "fallback must be the full target set");
        assert!(targets.contains(&Target::Claude));
        assert!(targets.contains(&Target::Goose));
        assert!(targets.contains(&Target::Qoder));
        assert!(targets.contains(&Target::AmazonQ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
