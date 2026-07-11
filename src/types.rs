//! Shared data types passed between the pipeline stages.
//!
//! The data flow is:
//!   `introspect` → [`ProjectProfile`]
//!   `interview`  → [`Intent`]
//!   `generate`   consumes both → writes files
//!   `verify`     reads the files → [`CheckResult`] per check
//!
//! `has_cli` is the single branching point for the pure-library path
//! (design §5.1 "Pure-library path"). Everything downstream keys off it.

/// What `skillpack` learned by reading the repo. Pure filesystem reads; the
/// only side-effectful piece is a guarded `--help` spawn, and only when a CLI
/// binary is detected (`has_cli = true`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectProfile {
    /// Best-effort tool name, derived from the project manifest or repo dir.
    /// Always coerced to kebab-case before it reaches a generated file.
    pub name: String,
    /// One of: `rust`, `node`, `python`, `go`, `ruby`, `unknown`.
    pub language: Language,
    /// True iff introspect detected an invokable CLI binary. The branching
    /// point for the pure-library path.
    pub has_cli: bool,
    /// The command an agent would run, if `has_cli`. e.g. `["chronicle",
    /// "--help"]`. `None` for pure libraries.
    pub cli_command: Option<Vec<String>>,
    /// Captured `--help` output, if a CLI was spawned. `None` for pure
    /// libraries or when the spawn failed/timed out.
    pub cli_help_output: Option<String>,
    /// Captured `<cli> <sub> --help` per subcommand, in declaration order.
    /// `Vec` (not a map) so clap's declaration order survives into
    /// deterministic snapshots. Empty for pure libraries, non-subcommand
    /// CLIs, or when every per-sub spawn failed/timed out.
    #[serde(default)]
    pub cli_subcommand_help: Vec<(String, String)>,
    /// Introspection decision trace for `skillpack doctor`. Empty when every
    /// detection branch succeeded (or when `doctor` wasn't run); each falsy
    /// branch in a candidate fn pushes one decision note here so `doctor` can
    /// explain why `has_cli = false` rather than silently reporting it.
    #[serde(default, skip_serializing_if = "DiagTrace::is_empty")]
    pub diag: DiagTrace,
    /// `git remote get-url origin`, best-effort.
    pub repo_url: Option<String>,
    /// SPDX identifier guessed from LICENSE file or manifest, e.g. `MIT`.
    pub license: Option<String>,
    /// Project version parsed from the language manifest (`Cargo.toml`
    /// `[package].version`, `package.json` `"version"`, etc.). `None` when
    /// the manifest has no version or the language has no version-bearing
    /// manifest (e.g. Go `go.mod`).
    pub version: Option<String>,
    /// Author(s) parsed from the language manifest (`Cargo.toml`
    /// `[package].authors`, `package.json` `"author"`, `pyproject.toml`
    /// `[project].authors`, `*.gemspec spec.authors`). `None` when the
    /// manifest has no author field. Used as a fallback when the interview
    /// / `skillpack.toml` doesn't supply one.
    pub authors: Option<String>,
    /// First paragraph of README, used as a description hint. May be empty.
    pub description_hint: Option<String>,
}

/// One decision point recorded during introspection for `skillpack doctor`.
/// `stage` is the function/phase that recorded the note; `note` is a short
/// human-readable string doctor prints.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiagNote {
    pub stage: &'static str,
    pub note: String,
}

/// A chronological trace of introspection decisions, surfaced by
/// `skillpack doctor` to explain why `has_cli` came out the way it did
/// (e.g. "python candidate: scripts entry `foo` points at `foo.cli:main`
/// but no importable dir `foo/` at root — try src-layout or `pip install -e .`").
/// The trace is best-effort: every falsy branch in a candidate fn pushes one
/// note before returning `None`; happy paths push nothing (doctor's signal
/// is the negative branches, the success is reflected in `has_cli` itself).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DiagTrace(pub Vec<DiagNote>);

impl DiagTrace {
    pub fn push(&mut self, stage: &'static str, note: impl Into<String>) {
        self.0.push(DiagNote {
            stage,
            note: note.into(),
        });
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Node,
    Python,
    Go,
    Ruby,
    Unknown,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
            Self::Ruby => "ruby",
            Self::Unknown => "unknown",
        }
    }
}

/// What `skillpack` learned from the interactive interview (or from
/// `skillpack.toml` when re-running non-interactively). The `generate` and
/// `verify` stages depend on these answers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Intent {
    /// One sentence describing the *task* the tool accomplishes, not the tool
    /// itself (design §5.1 Q1). Leads the `SKILL.md` description.
    pub one_line_description: String,
    /// Trigger phrases / verbs that tell an agent *when* to reach for this
    /// tool. Becomes `when_to_use` (design §5.1 Q2).
    pub when_to_use_phrases: Vec<String>,
    /// The exact invocation for a CLI project, e.g. `chronicle --new "entry"`.
    /// `None` for pure-library projects.
    pub invocation_command: Option<String>,
    /// The import pattern for a pure-library project, e.g.
    /// `import { foo } from 'yourpkg'`. `None` for CLI projects.
    pub import_pattern: Option<String>,
    /// Author display name, written to `plugin.json` and `skillpack.toml`.
    pub author: Option<String>,
    /// SPDX license id, written to `plugin.json` and `skillpack.toml`.
    pub license: Option<String>,
}

impl Intent {
    /// Whether this intent describes a pure library (no CLI). Convenience
    /// wrapper; exercised by integration tests (kept even though the staged
    /// modules inline the same check, so it stays a stable API surface).
    #[allow(dead_code)]
    pub fn is_pure_library(&self) -> bool {
        self.invocation_command.is_none()
    }
}
