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
use clap::Parser;

use skillpack::cli::{Cli, Commands};
use skillpack::config::Config;
use skillpack::exit;
use skillpack::generate::{coerce_kebab, render, GeneratedFileOutput};
use skillpack::interview;
use skillpack::introspect;
use skillpack::types;
use skillpack::verify::{self, VerifyInput, VerifyReport};

fn main() {
    let cli = Cli::parse();

    let code = if let Ok(code) = std::panic::catch_unwind(|| match cli.command {
        Commands::Init {
            root,
            non_interactive,
            accept_warnings,
            license,
        } => run_init(
            &root,
            cli.verbose,
            cli.debug,
            non_interactive,
            accept_warnings,
            license,
        ),
        Commands::Verify { root } => run_verify(&root, cli.verbose, cli.debug),
    }) {
        code
    } else {
        eprintln!("fatal: skillpack crashed (panic)");
        std::process::exit(exit::INIT_FATAL)
    };
    std::process::exit(code);
}

fn run_init(
    root: &Path,
    verbose: bool,
    debug: bool,
    non_interactive: bool,
    accept_warnings: bool,
    license_override: Option<String>,
) -> i32 {
    match run_init_inner(
        root,
        verbose,
        debug,
        non_interactive,
        accept_warnings,
        license_override,
    ) {
        Ok(c) => c,
        Err(e) => {
            // teach, don't just complain (design §8.1).
            eprintln!("fatal: {e:#}");
            std::process::exit(exit::INIT_FATAL);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_init_inner(
    root: &Path,
    verbose: bool,
    debug: bool,
    non_interactive: bool,
    accept_warnings: bool,
    license_override: Option<String>,
) -> Result<i32> {
    let profile = introspect::introspect(root).context("introspecting repo")?;
    if verbose {
        print_profile(&profile);
    }
    if debug {
        eprintln!(
            "[debug] detected name={} language={} has_cli={}",
            profile.name,
            profile.language.as_str(),
            profile.has_cli
        );
    }

    // Step 2 — interview or reuse config.
    let existing_cfg = Config::load(root)?;
    let intent = if non_interactive {
        let Some(cfg) = &existing_cfg else {
            bail!(
                "--non-interactive set but no skillpack.toml found at {}.\n\
                 To fix: run `skillpack init` once interactively to seed skillpack.toml, \
                 then retry in CI.",
                Config::path(root).display()
            );
        };
        match cfg.to_intent() {
            Some(i) => i,
            None => bail!(
                "skillpack.toml at {} is missing its [skill] table.\n\
                 To fix: re-run `skillpack init` interactively.",
                Config::path(root).display()
            ),
        }
    } else if let Some(cfg) = &existing_cfg {
        if let Some(i) = cfg.to_intent() {
            i
        } else {
            interview_run(&profile)?
        }
    } else {
        interview_run(&profile)?
    };

    let mut intent = intent;
    if let Some(ref lic) = license_override {
        intent.license = Some(lic.clone());
    }

    // Step 3 — render in memory.
    let files = render(&profile, &intent).context("rendering distribution files")?;

    // Step 4 — pre-commit verify against the rendered output (design §5.3).
    let report = verify_rendered(&files, &profile, root)?;

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
        let proceed = confirm_keep_anyway();
        if !proceed {
            eprintln!("Aborted. No files written.");
            return Ok(exit::INIT_ABORTED);
        }
        // User chose to keep — fall through to write.
    } else {
        let (_pass, warn, _fail, _skip) = report.counts();
        if warn > 0 && !accept_warnings && !non_interactive {
            eprintln!("\n⚠ verification passed with warnings:");
            eprintln!("{}", verify::render(&report));
        }
    }

    // Step 5 — write files + save config.
    write_files(root, &files)?;
    let name = coerce_kebab(&profile.name);
    Config::from_intent(&name, &intent).save(root)?;
    println!("✓ wrote {} file(s) under {}:", files.len(), root.display());
    for f in &files {
        println!("   - {}", f.rel_path);
    }
    println!("   - {}", Config::path(root).display());
    Ok(exit::INIT_OK)
}

fn interview_run(profile: &types::ProjectProfile) -> Result<types::Intent> {
    println!("\nNo skillpack.toml found — a few quick questions to scaffold your skill pack.\n");
    let prompter = interview::DialoguerPrompter;
    let intent = interview::run(profile, &prompter).context("during interview")?;
    Ok(intent)
}

fn verify_rendered(
    files: &[GeneratedFileOutput],
    profile: &types::ProjectProfile,
    root: &Path,
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
    let input = VerifyInput {
        // Discovery reads the rendered files from the temp dir (we verify the
        // ACTUAL files we're about to ship — design §5.3).
        root: tmp.path().to_path_buf(),
        // But the documented CLI runs in the real project root, where the
        // source tree / built artifact lives — spawning from the temp dir
        // (which holds only the rendered files) would false-fail any
        // relative-invocation CLI like `go run .` or `node ./bin/cli.js`.
        spawn_root: root.to_path_buf(),
        has_cli: profile.has_cli,
        cli_command: profile.cli_command.clone(),
    };
    verify::run(&input)
}

fn write_files(root: &Path, files: &[GeneratedFileOutput]) -> Result<()> {
    for f in files {
        let p = root.join(&f.rel_path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&p, &f.contents).with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(())
}

fn confirm_keep_anyway() -> bool {
    // A crude but dependency-free confirmation. The interview already pulled in
    // dialoguer, but re-attaching to a TTY here is finicky in tests; a simple
    // stdin readline is enough for the pre-commit gate.
    use std::io::{self, Write};
    let mut input = String::new();
    print!(
        "Critical verification failures were found (see above).\n\
         Write the files anyway? [y/N] "
    );
    let _ = io::stdout().flush();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn print_profile(profile: &types::ProjectProfile) {
    println!("— introspection —");
    println!("  name:        {}", profile.name);
    println!("  language:    {}", profile.language.as_str());
    println!("  has_cli:     {}", profile.has_cli);
    if let Some(cmd) = &profile.cli_command {
        println!("  cli_command: {}", cmd.join(" "));
    }
    if let Some(url) = &profile.repo_url {
        println!("  repo_url:    {url}");
    }
    if let Some(lic) = &profile.license {
        println!("  license:     {lic}");
    }
    if let Some(hint) = &profile.description_hint {
        if hint.len() > 120 {
            println!("  desc_hint:   {}…", &hint[..120]);
        } else {
            println!("  desc_hint:   {hint}");
        }
    }
}

fn run_verify(root: &Path, verbose: bool, _debug: bool) -> i32 {
    match run_verify_inner(root, verbose) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            std::process::exit(exit::INIT_FATAL);
        }
    }
}

fn run_verify_inner(root: &Path, verbose: bool) -> Result<i32> {
    // Defer to introspect only to recover has_cli + cli_command for the
    // invocation stage. (Debatable: we could read those from skillpack.toml.
    // Introspecting is the source of truth for "does the CLI actually exist
    // on this machine right now", which is what the invocation test cares about.)
    let profile = introspect::introspect(root).context("introspecting repo for verify")?;
    if verbose {
        print_profile(&profile);
    }
    let input = VerifyInput {
        root: root.to_path_buf(),
        spawn_root: root.to_path_buf(),
        has_cli: profile.has_cli,
        cli_command: profile.cli_command.clone(),
    };
    let report = verify::run(&input)?;
    print!("{}", verify::render(&report));
    Ok(if report.has_critical_failure() {
        exit::VERIFY_FAIL
    } else {
        exit::VERIFY_OK
    })
}

// --- tiny tempfile helper for the pre-commit verify step -------------------
// We avoid pulling `tempfile` into the binary's runtime deps; this inline
// module handles the one tempdir `init`'s pre-commit verify needs.
mod tempfile {
    use anyhow::Result;
    use std::path::{Path, PathBuf};

    pub fn tempdir() -> Result<TempDir> {
        let base = std::env::temp_dir();
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pid = std::process::id();
        let path: PathBuf = base.join(format!("skillpack-precommit-{pid}-{n}"));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }

    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
