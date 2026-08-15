//! `skillpack` entry point: clap dispatch, logging bootstrap, and the panic
//! hook. All subcommand implementations live in `commands/` (one module per
//! subcommand), so this file stays a thin dispatch — adding a command means
//! a new module there plus one arm in the match below.

use clap::{CommandFactory, Parser};

use skillpack::cli::{Cli, Commands, LogFormat};

mod commands;

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
        } => commands::run_init(
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
        } => commands::run_verify(
            &root,
            cli.verbose,
            format,
            fix,
            min_score,
            watch,
            template_dir.as_deref(),
        ),
        Commands::Doctor { root, format } => commands::run_doctor(&root, cli.verbose, format),
        Commands::Update {
            root,
            target,
            force,
            template_dir,
            format,
        } => commands::run_update(
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
        } => commands::run_diff(
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
        } => commands::run_add(
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
        } => commands::run_remove(
            &root,
            cli.verbose,
            &name,
            target,
            force,
            template_dir.as_deref(),
            format,
        ),
        Commands::Config { root, validate } => commands::run_config(&root, validate),
        Commands::Completions { shell } => {
            let mut cmd = <Cli as CommandFactory>::command();
            clap_complete::generate(shell, &mut cmd, "skillpack", &mut std::io::stdout());
            skillpack::exit::INIT_OK
        }
    }) {
        Ok(code) => code,
        Err(payload) => {
            eprintln!(
                "fatal: skillpack crashed (panic): {}",
                panic_message(&*payload)
            );
            std::process::exit(skillpack::exit::INIT_FATAL)
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

#[cfg(test)]
mod tests {
    // `main`'s catch_unwind names the panic payload (`panic!("msg")` carries
    // a `&str`, `panic!("msg {}", x)` a `String`) instead of printing a bare
    // "crashed".
    #[test]
    fn panic_message_reads_str_and_string_payloads() {
        let s: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(super::panic_message(&*s), "boom");
        let s: Box<dyn std::any::Any + Send> = Box::new("boom".to_string());
        assert_eq!(super::panic_message(&*s), "boom");
    }
}
