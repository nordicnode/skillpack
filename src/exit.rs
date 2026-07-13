//! Canonical exit codes.
//!
//! Per design §8.1 "Exit codes matter". Kept in one place so the CLI dispatch
//! and every subcommand agree on semantics.

/// `init` success: files generated (and verified) successfully.
pub const INIT_OK: i32 = 0;
/// `init` abort: the user chose not to proceed (answered "no" / declined
/// the pre-commit verification prompt).
pub const INIT_ABORTED: i32 = 1;
/// `init` fixable problem: verification failed but recovery is possible
/// (e.g. drift flags the user can fix and re-run).
pub const INIT_FIXABLE: i32 = 2;
/// `init` fatal error: unrecoverable (introspection cannot proceed, I/O
/// failure writing files, template render error).
pub const INIT_FATAL: i32 = 3;

/// `verify` success: every critical check passed (warnings allowed).
pub const VERIFY_OK: i32 = 0;
/// `verify` failure: at least one critical check failed. Blocks the PR.
pub const VERIFY_FAIL: i32 = 1;
/// `verify` score-below-min: every critical check passed, but the
/// discoverability score fell below the `--min-score` threshold the caller
/// opted into. Distinct from VERIFY_FAIL so a CI gate can tell "structure
/// broke" (1) from "drift/warnings degraded the score" (2) — the latter is
/// often actionable via `verify --fix` rather than a hand-edit.
pub const VERIFY_SCORE_BELOW_MIN: i32 = 2;
/// `verify` usage error: bad flag combination (e.g. `--watch` with a
/// non-human `--format`). Distinct from VERIFY_FAIL (a real check failed)
/// and VERIFY_SCORE_BELOW_MIN (structure is fine, score too low).
pub const VERIFY_USAGE: i32 = 4;

/// `diff` drift: at least one generated file differs from its on-disk
/// committed counterpart. Exit 1 (not 2) to match `git diff --exit-code`
/// and `differ` conventions. Fatal errors use `INIT_FATAL` (3).
pub const DIFF_DRIFT: i32 = 1;
