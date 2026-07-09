# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
