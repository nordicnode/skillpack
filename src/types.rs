//! Shared data types passed between the pipeline stages.
//!
//! The data flow is:
//!   `introspect` → [`ProjectProfile`]
//!   `interview`  → [`Intent`]
//!   `generate`   consumes both → writes files
//!   `verify`     reads the files → [`CheckResult`](crate::verify::result::CheckResult) per check
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
    /// Detected ecosystem. One of `rust`, `node`, `python`, `go`, `ruby`,
    /// `php`, `csharp`, `jvm`, `zig`, `swift`, `c_cpp`, `elixir`, `deno`,
    /// `nix`, `dart`, `haskell`, `lua`, `julia`, `crystal`, `clojure`,
    /// `ocaml`, `erlang`, `r`, `perl`, `shell`, `powershell`, or
    /// `unknown`.
    pub language: Language,
    /// Any additional languages detected alongside the dominant one (a
    /// polyglot monorepo, e.g. a Rust CLI with a TypeScript frontend). Empty
    /// for single-language repos. Order mirrors [`detect_language`] priority.
    /// Surfaced by `doctor` and `--verbose`, and used by `init --auto` to
    /// emit one skill per detected language.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary_languages: Vec<Language>,
    /// True iff introspect detected an invokable CLI binary. The branching
    /// point for the pure-library path.
    pub has_cli: bool,
    /// The command an agent would run, if `has_cli`. e.g. `["chronicle",
    /// "--help"]`. `None` for pure libraries.
    pub cli_command: Option<Vec<String>>,
    /// Captured `--help` output, if a CLI was spawned. `None` for pure
    /// libraries or when the spawn failed/timed out.
    pub cli_help_output: Option<String>,
    /// Captured subcommand tree (`<cli> <path...> --help` per node), in
    /// declaration order. Recursive: a node's `children` are the
    /// sub-subcommands its own `--help` advertises (e.g. `git remote` →
    /// `add`, `remove`). A node's `help` is empty when its spawn failed or
    /// timed out. Empty for pure libraries and non-subcommand CLIs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cli_subcommand_tree: Vec<SubcommandNode>,
    /// Introspection decision trace for `skillpack doctor`. Empty when every
    /// detection branch succeeded (or when `doctor` wasn't run); each falsy
    /// branch in a candidate fn pushes one decision note here so `doctor` can
    /// explain why `has_cli = false` rather than silently reporting it.
    //
    // Always serialized (no `skip_serializing_if`): the doctor JSON contract
    // promises a stable top-level `diag` key — empty array on clean runs,
    // non-empty when notes were pushed. Consumers can rely on `profile["diag"]`
    // existing.
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

/// One node in the captured CLI subcommand tree. `name` is the subcommand as
/// `--help` advertises it; `help` is the captured `<cli> <path...> --help`
/// (empty when that spawn failed or timed out — the node still exists because
/// its name came from the parent's `--help`); `children` are the sub-subcommands
/// the node's own `--help` advertises, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubcommandNode {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SubcommandNode>,
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
    Php,
    CSharp,
    Jvm,
    Zig,
    Swift,
    #[serde(rename = "c_cpp")]
    CCpp,
    Elixir,
    Deno,
    Nix,
    Dart,
    Haskell,
    Lua,
    Julia,
    Crystal,
    Clojure,
    Ocaml,
    Erlang,
    R,
    Perl,
    Shell,
    Powershell,
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
            Self::Php => "php",
            Self::CSharp => "csharp",
            Self::Jvm => "jvm",
            Self::Zig => "zig",
            Self::Swift => "swift",
            Self::CCpp => "c_cpp",
            Self::Elixir => "elixir",
            Self::Deno => "deno",
            Self::Nix => "nix",
            Self::Dart => "dart",
            Self::Haskell => "haskell",
            Self::Lua => "lua",
            Self::Julia => "julia",
            Self::Crystal => "crystal",
            Self::Clojure => "clojure",
            Self::Ocaml => "ocaml",
            Self::Erlang => "erlang",
            Self::R => "r",
            Self::Perl => "perl",
            Self::Shell => "shell",
            Self::Powershell => "powershell",
            Self::Unknown => "unknown",
        }
    }
}

/// What `skillpack` learned from the interactive interview (or from
/// `skillpack.toml` when re-running non-interactively). The `generate` and
/// `verify` stages depend on these answers.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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
    /// Stdin bytes to feed the CLI during `verify` spawns. For interactive
    /// CLIs that block on stdin. `None` uses `/dev/null` (default, preserves
    /// all existing behavior).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_stdin: Option<String>,
    /// Optional project-specific footguns or gotchas to document for agents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footguns: Vec<String>,
    /// Override the language-derived `allowed-tools` frontmatter value (e.g.
    /// `Read, Bash`). `Some` replaces the hint entirely; an empty string
    /// suppresses the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    /// Override the language-derived Cursor/Windsurf auto-attach `globs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub globs: Option<Vec<String>>,
    /// Override the language-derived `category` prose (e.g. "the Rust tooling").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Override the language-derived OpenCode `mode` (`primary`/`subagent`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_mode: Option<String>,
    /// Override the derived marketplace `keywords` list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    /// Override the marketplace `category` field (default `developer-tools`).
    /// Distinct from [`Intent::category`], which is body prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace_category: Option<String>,
    /// Override the marketplace `owner.type` field (default `individual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_type: Option<String>,
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
