//! clap argument parsing + subcommand dispatch. Per design §6.3, supports a
//! `--non-interactive` flag for CI, plus `--verbose` / `--debug` diagnostics.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "skillpack",
    bin_name = "skillpack",
    version,
    about = "Generate and verify the agent-distribution layer for any OSS project (Claude Code, Cursor, Codex, OpenCode, GitHub Copilot, AGENTS.md)."
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
        /// `claude` only (backward compatible). Pass the special value
        /// `all` to emit every supported target.
        #[arg(long, num_args = 1.., value_name = "ECOSYSTEM")]
        target: Vec<String>,

        /// Overwrite an existing root-level `AGENTS.md` (the `--target agentsmd`
        /// target writes to repo root, not a skillpack-owned directory). Without
        /// `--force`, an existing `AGENTS.md` is skipped with a warning. Has no
        /// effect on other targets — their paths are always skillpack-owned.
        #[arg(long)]
        force: bool,

        /// Override one or more Tera templates from a directory of `.tera`
        /// files. Missing templates fall back to embedded defaults — override
        /// just the files you need. Filenames must match the templates/
        /// directory (e.g. `SKILL.md.tera`, `plugin.json.tera`).
        #[arg(long, value_name = "DIR")]
        template_dir: Option<PathBuf>,
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

        /// Minimum discoverability score (0–100) the verify run must reach
        /// to exit zero. Independent of `--fix`: if `--fix` is also passed,
        /// the gate runs against the post-fix report. Omitted by default —
        /// projects opt in to harder enforcement (a low score is otherwise
        /// surfaced but never fails the run). Useful as a CI gate: pair with
        /// `--format json` for a structured exit.
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(0..=100))]
        min_score: Option<u8>,
        /// Watch for file changes and re-run verify on each change (debounced).
        /// Useful during iterative SKILL.md / skillpack.toml edits — get
        /// instant feedback without manually re-running verify each time.
        /// Ctrl-C stops the watcher (terminates the process). Only valid
        /// mode prints a new report per cycle; JSON output isn't meaningful
        /// for a streaming watcher).
        #[arg(long)]
        watch: bool,

        /// Override embedded Tera templates during `--fix` re-rendering. Same
        /// semantics as `init --template-dir`. Without this flag, `--fix`
        /// always uses embedded templates — if you initialized with custom
        /// templates, pass the same `--template-dir` here or `--fix` will
        /// produce output that drifts from your custom-rendered files.
        #[arg(long, value_name = "DIR")]
        template_dir: Option<PathBuf>,
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
    /// Incrementally regenerate distribution files from an existing
    /// `skillpack.toml` (no interview, no pre-commit verify gate — it's a
    /// refresh, not a scaffold). Re-introspects the repo, re-renders every
    /// target, and writes ONLY files whose content changed. For
    /// frontmatter-bearing files (`SKILL.md`, cursor `.mdc`, opencode
    /// `.md`) the body prose is preserved by splicing the fresh
    /// frontmatter onto the committed body — same surgery the auto-fix
    /// applies for `SKILL.md`. Note: the frontmatter block is regenerated
    /// wholesale, so any hand-tailored frontmatter fields skillpack
    /// doesn't manage (e.g. cursor `globs`, opencode `mode`) are
    /// overwritten. To preserve those, edit the template or keep them in
    /// `skillpack.toml`-driven fields.
    Update {
        /// Project root to operate on. Defaults to the current directory.
        #[arg(long, value_name = "DIR", default_value = ".")]
        root: PathBuf,

        /// Agent ecosystem(s) to regenerate for. Defaults to `claude`
        /// only. Pass `all` to refresh every supported target. Repeats.
        #[arg(long, num_args = 1.., value_name = "ECOSYSTEM")]
        target: Vec<String>,

        /// Overwrite an existing root-level `AGENTS.md` (same collision guard
        /// as `init --target agentsmd`). Without `--force`, an existing
        /// `AGENTS.md` is skipped with a warning. No effect on other targets.
        #[arg(long)]
        force: bool,

        /// Override one or more Tera templates from a directory of `.tera`
        /// files. Missing templates fall back to embedded defaults. Same
        /// semantics as `init --template-dir`.
        #[arg(long, value_name = "DIR")]
        template_dir: Option<PathBuf>,
    },
    /// Check whether distribution files are stale: re-render every target
    /// in memory, compare against on-disk content, report files that
    /// differ, and exit 1 if any do (0 if all clean). A CI gate for
    /// stale distribution files — run after `init`/`update` to verify
    /// committed artifacts match what skillpack would regenerate. Uses
    /// the same candidate computation as `update` (frontmatter splice
    /// for body files, wholesale overwrite for fully-generated files),
    /// so `diff` and `update` agree on what counts as drift.
    Diff {
        /// Project root to operate on. Defaults to the current directory.
        #[arg(long, value_name = "DIR", default_value = ".")]
        root: PathBuf,

        /// Agent ecosystem(s) to check. Defaults to `claude` only.
        /// Pass `all` to check every target. Repeats.
        #[arg(long, num_args = 1.., value_name = "ECOSYSTEM")]
        target: Vec<String>,

        /// Check `AGENTS.md` too (same collision guard as `update` —
        /// skipped without `--force` if it exists). No effect on other
        /// targets.
        #[arg(long)]
        force: bool,

        /// Override one or more Tera templates from a directory of `.tera`
        /// files. Missing templates fall back to embedded defaults. Same
        /// semantics as `init --template-dir`. Use when `diff` is checking
        /// a pack generated with custom templates.
        #[arg(long, value_name = "DIR")]
        template_dir: Option<PathBuf>,
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
    /// AGENTS.md: a root-level `AGENTS.md` instructions file read natively by
    /// 20+ coding agents (Codex, Cursor, Windsurf, Copilot, Aider, Zed, Warp,
    /// JetBrains Junie, etc.). Per agents.md (Linux Foundation stewarded) —
    /// plain markdown, no frontmatter, no required fields.
    #[clap(name = "agentsmd")]
    AgentsMd,
}

/// Expand a list of targets, resolving the string `"all"` into every concrete
/// target. Called from `run_init_inner` after the init subcommand parses
/// `--target` values; the hidden `all` value clears here so dispatch sites
/// (`generate::run`, verify, fix) never see a synthetic variant.
pub fn resolve_targets(raw: &[String]) -> anyhow::Result<Vec<Target>> {
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        if r == "all" {
            // Canonical order — `Target` declaration order minus the sentinel.
            out.extend([
                Target::Claude,
                Target::Cursor,
                Target::Codex,
                Target::OpenCode,
                Target::Copilot,
                Target::AgentsMd,
            ]);
        } else {
            out.push(Target::from_str(r, true).map_err(|s| {
                anyhow::anyhow!(
                    "invalid --target `{s}`; expected claude|cursor|codex|opencode|copilot|agentsmd|all"
                )
            })?);
        }
    }
    // Dedup preserving canonical order — `--target all --target claude`
    // must not emit Claude twice (double-writes files).
    let mut seen = Vec::new();
    for t in out {
        if !seen.contains(&t) {
            seen.push(t);
        }
    }
    Ok(seen)
}
