//! clap argument parsing + subcommand dispatch. Per design §6.3, supports a
//! `--non-interactive` flag for CI, plus `--verbose` / `--debug` diagnostics.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "skillpack",
    bin_name = "skillpack",
    version,
    about = "Generate and verify the agent-distribution layer for any OSS project (Claude Code, Cursor, Codex, OpenCode, GitHub Copilot)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Print what skillpack detected in the repo (introspection output).
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Print every subprocess call skillpack makes.
    #[arg(long, global = true)]
    pub debug: bool,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Scaffold the distribution layer (introspect → interview → generate).
    Init {
        /// Project root to operate on. Defaults to the current directory.
        #[arg(long, value_name = "DIR", default_value = ".")]
        root: PathBuf,

        /// Skip interactive prompts; require a `skillpack.toml` to exist.
        /// Intended for CI — never offers to keep unverified output.
        #[arg(long)]
        non_interactive: bool,

        /// Accept the pre-commit verification and write files even when `verify`
        /// flags warnings. Critical (`fail`) results still block the write.
        /// Without this flag, any non-pass result prompts the user.
        #[arg(long)]
        accept_warnings: bool,

        /// Override the license SPDX id for this run (writes it to skillpack.toml).
        #[arg(long, value_name = "SPDX")]
        license: Option<String>,

        /// Agent ecosystem(s) to generate distribution files for. Repeat to
        /// emit multiple: `--target claude --target cursor`. Defaults to
        /// `claude` only (backward compatible).
        #[arg(long, value_enum, num_args = 1.., value_name = "ECOSYSTEM")]
        target: Vec<Target>,
    },
    /// Check the distribution files against the agent schemas + CLI drift.
    Verify {
        #[arg(long, value_name = "DIR", default_value = ".")]
        root: PathBuf,

        /// Output format. `human` (default) prints a readable report; `json`
        /// prints a machine-readable object with per-check ids for CI gating.
        #[arg(long, value_enum, default_value_t = crate::verify::OutputFormat::Human)]
        format: crate::verify::OutputFormat,

        /// Apply mechanical fixes for detected drift (e.g. regenerate a stale
        /// `.claude-plugin/plugin.json` whose `version` drifted from the
        /// project manifest). Surgical: only the file the drift lives in is
        /// rewritten — your hand-tailored `SKILL.md` / `marketplace.json`
        /// stay intact. After fixes are applied, verify re-runs and prints
        /// the post-fix report. Use `skillpack init` for wholesale regen.
        #[arg(long)]
        fix: bool,
    },
    /// Diagnose why introspection chose `has_cli` / language as it did.
    /// Prints the detected profile + a chronological trace of the decision
    /// branches that fired (which candidate was tried, why it was rejected,
    /// what would make it succeed). Read-only — never writes files.
    Doctor {
        /// Project root to operate on. Defaults to the current directory.
        #[arg(long, value_name = "DIR", default_value = ".")]
        root: PathBuf,

        /// Output format. `human` (default) prints the readable diagnosis;
        /// `json` emits the serialized `ProjectProfile` (including the
        /// decision trace) for CI/scripts. Mirrors `verify --format`.
        #[arg(long, value_enum, default_value_t = crate::verify::OutputFormat::Human)]
        format: crate::verify::OutputFormat,
    },
}

/// Which agent ecosystem to generate distribution files for.
/// Per design §10 (Phase 4: multi-ecosystem delivery).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum, Default)]
pub enum Target {
    /// Claude Code: `.claude-plugin/` + `skills/<name>/SKILL.md`.
    #[default]
    Claude,
    /// Cursor: `.cursor/rules/<name>.mdc` rule file.
    Cursor,
    Codex,
    /// OpenCode: `.opencode/agents/<name>.md` agent definition file.
    /// Per opencode.ai/docs/agents — frontmatter (`description` required,
    /// `mode`/`temperature`/`permissions` optional); no `.claude-plugin/`.
    #[clap(name = "opencode")]
    OpenCode,
    /// GitHub Copilot: `.github/copilot-instructions.md` custom instructions.
    /// Per docs.github.com/copilot — plain markdown, no frontmatter.
    Copilot,
}
