# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.4] - 2026-07-10

### Fixed

- **`plugin.json` version hardcoded `0.1.0`** — `generate::build_context`
  emitted a literal `"0.1.0"` regardless of the project's real version. An
  agent installing via a marketplace would see the wrong version signal for
  every skill pack `skillpack` produced — the exact class of distribution-
  layer drift `verify` exists to catch, yet `verify` itself never checked the
  field. `introspect` now reads the version from the language manifest
  (`Cargo.toml [package].version`, `package.json "version"`,
  `pyproject.toml [project].version`, `*.gemspec spec.version`), stores it
  on `ProjectProfile`, and `generate` passes it through to the template. Go
  (`go.mod` has no version field) and manifests lacking a version key yield
  an empty `version` rather than a fake sentinel — the honest signal.

### Added

- **`discovery.plugin.version` verify check** — warns on a missing or empty
  `version` field in `plugin.json`, with a suggestion pointing at the
  manifest key to set. Surfaced by the self-dogfood below: the old code
  shipped `0.1.0` for a `0.2.3` crate and `verify` never flagged it.
- **Self-dogfood** — `skillpack` now generates and verifies its own skill
  pack (`skills/skillpack/SKILL.md`, `.claude-plugin/marketplace.json`,
  `.claude-plugin/plugin.json`) via a committed `skillpack.toml`. Closes the
  Phase-1 §10 "use it on yourself" gap; the repo is now installable as
  `claude plugin marketplace add nordicnode/skillpack`.

## [0.2.3] - 2026-07-10

### Fixed

- **Pipe-deadlock on >64KB `--help`** — subprocess spawns in `introspect`
  (`spawn_with_timeout`), `verify::invocation` (`run_help` + `spawn_capture`)
  piped stdout/stderr but only drained pipes *after* the child exited. A CLI
  whose help output exceeded the ~64KB pipe buffer would block on the write,
  `try_wait` would keep returning `Ok(None)`, and the deadline fired → false
  `TimedOut`. Extracted a shared `spawn::run` helper that drains pipes on
  reader threads while polling, killing + reaping on timeout. The new boundary
  replaces the hand-rolled poll loops in all three call sites.
- **Byte-slice panic on multibyte README hints** — `print_profile` (`--verbose`)
  truncated the description hint with `&hint[..120]`, slicing by byte index.
  A multibyte UTF-8 char across byte 120 (emoji, CJK, accented — common in
  real OSS) panicked with "byte index 120 is not a char boundary" →
  `catch_unwind` → false `INIT_FATAL` exit for a display-only path. Fixed to
  truncate by chars: `hint.chars().take(120).collect::<String>()`.
- **`coerce_kebab` leading-digit names** — names starting with digits (e.g.
  `"123foo"`) passed through unchanged, but the schema regex
  `^[a-z][a-z0-9-]*[a-z0-9]$` requires a letter first. The generated
  `marketplace.json` / `plugin.json` name would then fail `verify`'s own
  `is_valid_kebab` check. Now strips leading digits + re-trims hyphens,
  falling back to `"tool"` if the result is empty.

### Added

- **Subcommand-drift e2e coverage** — the `capture_subcommand_help` (introspect)
  and `check_subcommand_drift` (verify) code paths — real spawn reassembly of
  `<base> <sub> --help` — were reasoned about but never exercised against a
  compiled CLI (every fixture set `cli_subcommand_help: Vec::new()`). Added a
  zero-dep `subcommand-cli` fixture with a hand-rolled clap-shaped `Commands:`
  section + per-subcommand `--help`, and an integration test asserting the
  generated `SKILL.md` contains `### Subcommands` with real sub names/flags and
  `verify --format json` emits `invocation.subcommand_drift` pass results.

## [0.2.2] - 2026-07-09

### Fixed

- Generated `SKILL.md` "When NOT to use" prose no longer reads as broken
  grammar ("outside the systems programming it was built for") — `category_hint`
  now yields a noun per language ("the Rust tooling", "the JavaScript/Node
  tooling", etc.).
- `verify --format json` now reports per-check `severity: "pass"` for passes,
  matching the `counts.pass` key. Previously it emitted `"ok"` for a passed
  check while the aggregate `counts` object used `pass` — two words for one
  concept in the same payload.
- Removed the dead `has_cli` field from `VerifyInput` / `InvocationInput`.
  CLI *presence* is derived from the SKILL.md itself, not the introspected
  binary, so the field was threaded in but never read.

## [0.2.1] - 2026-07-09

### Fixed

- `init` now distinguishes its exit codes per design §8.1: a fixable
  verify critical that the user declines to keep exits `INIT_FIXABLE` (2),
  not the clean-abort `INIT_ABORTED` (1); fatal errors (introspect / render /
  I/O) still exit `INIT_FATAL` (3). Previously the interactive decline path
  returned 1, collapsing the fixable/fatal contract.
- Removed a redundant second `wait` in both spawn helpers
  (`introspect::spawn_with_timeout`, `verify::invocation::run_help`): the
  piped stdout/stderr are drained by a single `wait_with_output`, whose exit
  status is used rather than re-reading the one `try_wait` already probed.

### Changed

- The invocation check is documented (README + design §5.2) as running
  against the first documented CLI; discovery checks still cover every
  `SKILL.md` in a multi-skill plugin. `init` only ever emits one skill, so
  per-skill spawn plumbing is deferred until a CLI-backed multi-skill plugin
  is a real case.

## [0.2.0] — 2026-07-09

### Fixed

- `init` no longer emits a `when_to_use: "(unspecified)"` placeholder when no
  trigger phrases are given — it emits an empty `when_to_use`, so `verify`'s own
  emptiness warning now fires honestly. Previously a generated skill with no
  triggers would pass `verify` silently (the "looks fine until an agent tries
  it" failure mode).
- `verify`'s invocation/flag-drift check now derives CLI presence from the
  `SKILL.md` itself (a `## Invocation` section, or a fenced block with
  `--flags`), not from whether introspect found a built binary on the local
  machine. Hand-written packs that document a CLI but ship no source tree now
  get a visible warning instead of a silent "pure-library" skip.
- `--debug` now prints every subprocess spawn in `verify` (it was previously a
  no-op for the `verify` subcommand).
- `--accept-warnings` now matches its docs: in interactive mode, `verify`
  warnings prompt before writing; `--accept-warnings` skips the prompt.
  `--non-interactive` warnings never block (CI gates on criticals only).
- The `property_proptest.rs` placeholder is now a real property test exercising
  `extract_flags` and `parse_skill_frontmatter` directly (the lib re-exports it
  already exposed).

### Added

- `verify --format json`: machine-readable report with per-check ids, counts,
  and an `ok` flag, for CI gating / scripting.
- Reverse flag-drift warning: `--help` flags the skill doesn't document are
  reported as a warning (discoverability gap for an agent).
- Multi-skill verification: `verify` checks every SKILL.md under `skills/`
  (sorted, deterministic) instead of an arbitrary first one.
- `--format`, scoped flag-drift extraction (the documented invocation area,
  not the whole body), and a testable confirmation path for the pre-commit gate.

### Changed

- `tempfile` is now a runtime dependency; the hand-rolled `mod tempfile` in
  `main.rs` is removed.

## [0.1.0] — 2026-07-09

### Added

- `skillpack init` — introspect a repo (Rust, npm, Python, Go, Ruby), interview
  the maintainer (or run non-interactive from `skillpack.toml`), and generate
  the three Claude Code distribution files (`marketplace.json`, `plugin.json`,
  `SKILL.md`). Pure-library path when no CLI is detected (documents the install
  + import pattern instead of an invocation).
- `skillpack verify` — discovery checks (kebab-case + reserved-name validation,
  1,536-char listing cap, `when_to_use` trigger phrases, `./`-prefixed paths)
  and invocation checks (`--help` runs under timeout; every documented flag
  exists in real `--help` output). Exits non-zero on critical failure for use
  as a CI PR gate.
- Pre-commit verification: `init` runs the full `verify` suite against its own
  output before writing files.
- `--non-interactive`, `--accept-warnings`, `--license <SPDX>`, `--verbose`,
  `--debug` flags.
- GitHub Actions CI: `cargo fmt --check`, `cargo clippy --all-targets -D
  warnings`, and `cargo test -- --include-ignored` (runs the Go + Ruby
  `#[ignore]`-gated round trips where those runtimes are present).
- Insta snapshot tests pinning byte-identical output of all three generated
  files for CLI and pure-library profiles.
- Test fixtures for rust, node, python, go, ruby CLIs and a node library;
  a `broken-cli` fixture for drift detection.

### Toolchain

- Pin Rust 1.95.0 via `rust-toolchain.toml` so local and CI fmt/clippy agree.
