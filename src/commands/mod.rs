//! `skillpack` subcommand implementations + shared helpers, split out of
//! `main.rs` (which keeps only the clap dispatch, logging bootstrap, and panic
//! hook). One module per subcommand (`init`, `verify`, `doctor`, `update`,
//! `diff`, `add`, `remove`, `config`); cross-command helpers (confirm
//! overrides, profile printing, candidate computation, target detection) live
//! here in the module root so every subcommand reaches them at
//! `crate::commands::…`.

use std::path::Path;

use anyhow::{bail, Context, Result};

use skillpack::exit;
use skillpack::generate::{coerce_kebab, GeneratedFileOutput};
use skillpack::interview;
use skillpack::types;
use skillpack::verify;

mod add;
mod config;
mod diff;
mod doctor;
mod init;
mod remove;
mod update;
mod verify_cmd;

// Entry points main.rs dispatches to.
pub(crate) use add::run_add;
pub(crate) use config::run_config;
pub(crate) use diff::run_diff;
pub(crate) use doctor::run_doctor;
pub(crate) use init::run_init;
pub(crate) use remove::run_remove;
pub(crate) use update::run_update;
pub(crate) use verify_cmd::run_verify;

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

/// True if the given rel-path is a frontmatter-bearing file that needs body
/// preservation during `update` (SKILL.md, cursor .mdc, opencode .md).
/// Plain-markdown files (AGENTS.md, copilot-instructions.md, CLAUDE.md,
/// GEMINI.md, CONVENTIONS.md, `.goose/instructions.md`) and the plain rule
/// files (`.clinerules/`, `.roo/rules/`, `.kilocode/rules/`) are NOT included
/// — `split_frontmatter` would return None on them.
fn is_frontmatter_target(rel_path: &str) -> bool {
    // The plain-markdown rules-directory targets have NO YAML frontmatter, so
    // they must take the full-render path — splicing "fresh frontmatter + body"
    // onto a frontmatter-less file would append a spurious blank line and make
    // an update-written file look permanently drifted. Keep this list in sync
    // with `generate::rule_dir` (every rules-directory target must appear in
    // both places).
    if rel_path.starts_with(".clinerules/")
        || rel_path.starts_with(".roo/rules/")
        || rel_path.starts_with(".kilocode/rules/")
        || rel_path.starts_with(".qoder/rules/")
        || rel_path.starts_with(".continue/rules/")
        || rel_path.starts_with(".augment/rules/")
        || rel_path.starts_with(".amazonq/rules/")
        || rel_path.starts_with(".trae/rules/")
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
        skillpack::verify::schema::AGENTS_MD_PATH
            | skillpack::verify::schema::CLAUDE_MD_PATH
            | skillpack::verify::schema::GEMINI_MD_PATH
            | skillpack::verify::schema::CONVENTIONS_MD_PATH
            | skillpack::verify::schema::GOOSE_INSTRUCTIONS_PATH
            | skillpack::verify::schema::COPILOT_INSTRUCTIONS_PATH
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::init::auto_intent;
    use crate::commands::update::{
        compute_candidates, default_refresh_targets, detect_present_targets, CandidateStatus,
    };
    use skillpack::cli::Target;

    #[test]
    fn is_frontmatter_target_excludes_every_rules_directory() {
        // Every plain-markdown rules-directory target must take the full-render
        // path (a frontmatter-less file spliced as "frontmatter + body" grows a
        // spurious blank line and looks permanently drifted). Keep in sync with
        // `generate::rule_dir` — a new rules target that misses this list would
        // hit exactly that bug.
        for dir in [
            ".clinerules",
            ".roo/rules",
            ".kilocode/rules",
            ".qoder/rules",
            ".continue/rules",
            ".augment/rules",
            ".amazonq/rules",
            ".trae/rules",
        ] {
            assert!(
                !is_frontmatter_target(&format!("{dir}/x.md")),
                "{dir} rules must be treated as plain markdown"
            );
        }
        // Frontmatter-bearing shapes must stay on the splice path.
        assert!(is_frontmatter_target("skills/x/SKILL.md"));
        assert!(is_frontmatter_target(".codex/skills/x/SKILL.md"));
        assert!(is_frontmatter_target(".cursor/rules/x.mdc"));
    }

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
        let intent = auto_intent(&profile, &[], None, None).unwrap();
        assert_eq!(
            intent.invocation_command.as_deref(),
            Some(stem.as_str()),
            "renamed bin must be documented as its real name"
        );

        // `go run .` — a bare runtime name is NOT a resolvable file → package
        // name wins.
        let mut go = profile.clone();
        go.cli_command = Some(vec!["go".into(), "run".into(), ".".into()]);
        let intent = auto_intent(&go, &[], None, None).unwrap();
        assert_eq!(intent.invocation_command.as_deref(), Some("fd-find"));
    }

    // Regression: `init --auto --description` must work in a README-less repo
    // (the old error told users to "Pass --description" but `--auto` never
    // honored the flag — the fallback now fills the description hint).
    #[test]
    fn auto_intent_honors_description_flag_without_readme_hint() {
        let profile = types::ProjectProfile {
            name: "nodocs".into(),
            language: types::Language::Rust,
            secondary_languages: Vec::new(),
            has_cli: true,
            cli_command: Some(vec!["node".into(), "cli.js".into()]),
            cli_help_output: Some("usage".into()),
            cli_subcommand_tree: Vec::new(),
            repo_url: None,
            license: Some("MIT".into()),
            version: None,
            authors: None,
            // No README hint / manifest description → the flag must save it.
            description_hint: None,
            diag: types::DiagTrace::default(),
        };
        let intent = auto_intent(&profile, &[], None, Some("A docs-less tool")).unwrap();
        assert_eq!(intent.one_line_description, "A docs-less tool");
        // The flag also becomes the trigger fallback.
        assert_eq!(intent.when_to_use_phrases, vec!["A docs-less tool"]);

        // Without the flag (and no hint), the old error still fires.
        assert!(auto_intent(&profile, &[], None, None).is_err());
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
        assert_eq!(targets.len(), 19, "fallback must be the full target set");
        assert!(targets.contains(&Target::Claude));
        assert!(targets.contains(&Target::Goose));
        assert!(targets.contains(&Target::Qoder));
        assert!(targets.contains(&Target::AmazonQ));
        assert!(targets.contains(&Target::Trae));
        let _ = std::fs::remove_dir_all(&root);
    }
}
