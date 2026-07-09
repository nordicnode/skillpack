# Contributing to skillpack

Thanks for considering a contribution. The project is small on purpose —
prefer a focused change over a broad one.

## Before you write code

1. **Open an issue first** for anything beyond a typo or obvious bug fix.
   A 30-second comment saves a 3-hour PR.
2. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
   `cargo test -- --include-ignored` must all pass. CI enforces this, but
   running locally saves a round trip. (`--include-ignored` runs the Go +
   Ruby round trips if those runtimes are installed; they self-skip
   otherwise.)
3. The toolchain is pinned in `rust-toolchain.toml` (1.95.0). If you use
   rakup/rustup, that version is used automatically — this is what keeps
   `cargo fmt` identical between your machine and CI.

## The two ways to contribute

**Rust changes** (`src/`) touch detection, verification, or generation
logic. Add or adjust a test in `tests/` or the relevant `#[cfg(test)]` module.
Non-trivial parser/detection logic gets a check that fails if the logic
breaks — the smallest thing that reproduces the bug, not a full fixture suite.

**Template changes** (`templates/*.tera`) need no Rust knowledge. These define
what the generated `SKILL.md`, `plugin.json`, and `marketplace.json` look like.
After editing a template, regenerate the snapshots and review the diff:

```sh
INSTA_UPDATE=always cargo test --test snapshots
cargo insta review    # or accept the .snap.new files manually
```

The snapshot tests in `tests/snapshots.rs` pin the byte-identical output for a
CLI profile and a pure-library profile. Any template edit surfaces there as a
diff you must deliberately accept — this is the guard against a silent change
to what `skillpack` ships.

## Adding a new ecosystem

Detection lives in `src/introspect.rs`. The pattern: add a `Language` variant
in `src/types.rs`, a candidate resolver returning `Option<CliCandidate>` with
an honest `None` when the runtime is missing, a fixture in
`tests/fixtures/repos/<lang>-cli`, and a `#[ignore]`-gated round-trip test in
`tests/integration_end_to_end.rs` with a runtime self-skip probe. See the Go
or Ruby entries as the template.
