# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [0.9.3] - 2026-07-12


### Fixed — name_drift WARN no longer shadows name_length FAIL

- `discovery.skill.name_drift` (a WARN) was placed in `check_one_skill_md`
  BEFORE `discovery.skill.name_length` (a FAIL). The 0.9.2 fix that moved
  name_drift after the first batch of FAIL-severity checks (description,
  description_empty, description_length) missed this fourth FAIL further down
  the function — name_drift's early `return Ok(r)` shadowed it. A SKILL.md
  with BOTH a drifted name AND one >64 chars reported only the warn → verify
  exited 0 / "OK" instead of exit 1 / "FAILED", silently passing a structurally
  broken skill that should have hard-failed. Both `name_length` (FAIL) and its
  sibling `name_reserved` (WARN) now run BEFORE name_drift — restoring the
  fail-severity-first invariant the 0.9.2 patch locked down for description.
  Regression test: `name_drift_warn_does_not_shadow_name_length_fail`
  (asserts a 65-char drifted name exits 1 with `discovery.skill.name_length`
  FAIL in the report, not exit 0 / warn-only).

### Fixed — BOM-prefixed (U+FEFF) SKILL.md / .mdc / OpenCode / Copilot files no longer false-fail

- Four text-entry points (`parse_skill_frontmatter`, `parse_cursor_mdc_frontmatter`,
  `parse_opencode_agent_frontmatter`, `check_copilot_instructions`) had no
  handling for a leading UTF-8 BOM (U+FEFF, bytes EF BB BF). `fs::read_to_string`
  preserves BOM (valid UTF-8); Rust's `char::is_whitespace` returns false for
  U+FEFF (Unicode 3.2+), so `str::trim()` and `trim_start()` do NOT strip it →
  a `\u{feff}---` opening delimiter failed the `== "---"` guard → empty
  SkillFrontmatter → false "frontmatter missing description" FAIL on a
  structurally valid file, and a BOM-prefixed `#` heading in Copilot
  instructions false-warned "not a markdown heading". Windows editors
  (Notepad, VS Code "UTF-8 with BOM" save) emit this prefix, so any hand-edited
  distribution file from a Windows user was exposed. Added a `strip_bom`
  helper (`discovery.rs`) applied at the raw-text read boundary in all 4 entry
  points plus `verify --fix`'s committed-SKILL.md read (defensive — the
  latent `split_frontmatter` body-deletion path was unreachable today but is
  now closed). Regression test: `bom_prefixed_skill_md_validates_clean`
  (asserts a BOM-prefixed SKILL.md with valid frontmatter exits 0 with no
  `discovery.skill.description` fail).

### Fixed — invocation multi-CLI counter no longer silently drops unreadable skills

- `verify::run`'s multi-CLI counter (`mod.rs`) swallowed `read_to_string` Err
  to `None` via `.ok()`, dropping unreadable skills (non-UTF8, permission
  denied, EBUSY) from the count — while the parallel discovery loader
  propagated the same error class via `.with_context(...)?` to abort verify.
  A corrupt `skills/<x>/SKILL.md` would FATAL-abort via discovery but silently
  no-op in the invocation counter — inconsistent. Now surfaces an
  `invocation.read_failed` WARN per unreadable skill (in addition to discovery's
  existing abort behavior) so the maintainer sees the read failure on both
  paths rather than the multi-CLI counter silently under-firing.

### Fixed — misleading "description is empty" message on malformed nested-YAML `description:`

- A multi-line YAML map under `description:` (e.g. `description:\n  broken:
  yaml`) parses without panic but flushes the key with an empty value — verify
  hard-failed with "description is empty", which hid the real cause (the user
  wrote a nested map instead of a same-line scalar). The message now reads
  "is empty (it may be missing, OR the line `description:` has no value on the
  same line — e.g. a nested map)" and the suggestion points at writing the
  value on the SAME line as `description:` to avoid indented YAML blocks.

### Docs — README flags table now covers `--root` and `--min-score`

- The Flags table in README.md was missing two shipped flags: `--root <DIR>`
  (the project-root override available on init / verify / doctor) and
  `--min-score <N>` (shipped in 0.9.2, the opt-in CI discoverability-score
  gate). Both are now in the table.

### Internal — dropped `let _ = &…` anti-idiom in `verify::render`

- `render()` destructured `counts()` as `(pass, warn, fail, skip)` then emitted
  three `let _ = &pass; let _ = &skip; let _ = &warn;` lines to bypass
  `unused_variable` (only `fail` was actually consumed by the summary line +
  `pass`/`warn` were used in the format string). Replaced with a clean
  `(pass, warn, fail, _skip)` discard.
### Changed — internal: decompose `introspect.rs` into `cli_probe.rs` + `repo.rs`

- Pure internal refactor; no user-facing behavior or API change. `src/introspect.rs`
  shrank from 1091 to 260 lines (76% reduction) — it is now a thin top-level
  orchestrator that calls `detect_language` then delegates to five sibling
  submodules:
  - `cli_probe.rs` (517, new) — the guarded `--help` spawn pipeline
    (`detect_cli`, `spawn_candidate`, `capture_subcommand_help`) plus the
    Cargo/npm workspace member walks (`walk_cargo_workspace`,
    `walk_npm_workspace`) and the root-manifest probes (`has_gemspec`,
    `has_csproj`).
  - `repo.rs` (136, new) — repo-metadata reads: `git remote get-url origin`
    (`detect_repo_url`, inline `spawn::run` so it shares no helper with the
    CLI probe pipeline), the LICENSE SPDX heuristic (`detect_license`), the
    README description hint (`read_readme_hint`), and `repo_url_name`.
  - `cli_candidates.rs` — gained the `candidate_tests` module (moved from the
    parent; tests the candidate *resolution* fns that already lived here),
    and its module doc was updated to point at `super::cli_probe` (the
    orchestrator no longer "stays in the parent").
  - `manifest.rs`, `workspace.rs` — unchanged.
- Refactor coverage preserved verbatim (62 lib + 49 integration + 3 property + 13
  snapshot, all green); the walk_* tests live in `cli_probe::tests`, the
  `read_readme_hint` HTML-skip regression lives in `repo::tests`, and the
  dir-tail canonicalization (Bug #3) + `which_on_path` real-exercise tests
  stay in the parent `parse_tests`. The bug fixes below add 2 integration tests
  (51 integration total, 132 tests overall), each documented inline.
- All release gates green: `cargo fmt -- --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test -- --include-ignored`, self-dogfood
  `skillpack verify --root .` → 12 pass / 0 warn / 0 fail / score 100, and
  `skillpack doctor --root .` (human + json) clean.

## [0.9.2] - 2026-07-12

### Added — verify catches SKILL.md name drift + opt-in score gate

- `verify` now warns when a generated `SKILL.md` frontmatter `name:` field
  no longer matches the canonical project name (`coerce_kebab(profile.name)`,
  the exact value the template renders). Fires under two check IDs —
  `discovery.skill.name_drift` (Claude) and `discovery.codex.skill.name_drift`
  (Codex) — both mapped to the new `FixAction::RegenSkillMdFrontmatter`.
  Warning (not failure): drift signals a hand-edited skill file or a renamed
  project that wasn't regenerated. Skips silently when either side is absent
  (e.g. no detectable project name).
- `verify --fix` regenerates ONLY the frontmatter block of the drifted
  `SKILL.md`, splicing the fresh frontmatter onto the preserved body prose.
  The applicator derives the target (Claude vs Codex) and which skill file
  from the warning's `location` rel-path, re-renders that single target, and
  reads the committed file with CRLF→LF normalization before splicing — so
  maintainer body prose (gotchas, examples, hand-tailored sections) survives
  the fix byte-for-byte on Windows autocrlf checkouts. Wholesale `init` regen
  remains the path for body drift; `--fix` is the surgical tool for
  frontmatter-only drift.
- `verify --min-score <N>` (0–100) is an opt-in CI gate: when passed, verify
  exits **2** if the discoverability score is below `N` and no critical check
  failed. Exit 2 (`VERIFY_SCORE_BELOW_MIN`) is distinct from exit 1
  (`VERIFY_FAIL`) so CI can distinguish a structurally broken pack from a
  score below an opt-in threshold — the latter is often `verify --fix`-
  actionable. A human-readable stderr line names the threshold and actual
  score when the gate fires; the JSON body still lands on stdout under
  `--format json`. Exit precedence is now load-bearing: **critical failure
  (1) > score below min (2) > ok (0)** — a broken pack surfaces the
  structure error first, the score gate fires only when structure passed.
  Omitted by default; the score continues not to gate exits unless asked.

### Internal — verify plumbing threads profile_name + location to discovery & fix

- `VerifyInput` gains a `profile_name: Option<String>` field (the
  `coerce_kebab`'d canonical name, threaded from both callers). `discovery::run`
  forwards it to `check_one_skill_md`, where the name_drift check compares it
  against the frontmatter `name:`. Threading the pre-coerced value (rather
  than re-coercing inside discovery) keeps the comparison a raw string match
  with no duplicated normalization, and keeps discovery a pure function of
  its inputs.
- `fix::apply` now threads `location: Option<&(String, Option<usize>)>` so the
  `RegenSkillMdFrontmatter` applicator can locate the drifted file without
  re-scanning. `RegenPluginJson` ignores `location` (fixed path); the verify
  dispatcher retains `r.location.clone()` so the applicator isn't dead. The
  distillation is one new enum variant + one applicator + one `action_for`
  arm — the exhaustive match makes forgetting the arm a compile failure.
- `exit::VERIFY_SCORE_BELOW_MIN = 2` joins `VERIFY_OK = 0` and `VERIFY_FAIL = 1`.
  Documented for consumers scripting `verify` — exit 2 is recoverable via
  `--fix` or by raising the pack's score, exit 1 is structural.

## [0.9.1] - 2026-07-12

### Added — verify catches plugin.json URL drift (`discovery.plugin.url_drift`)

- `verify` now warns when `.claude-plugin/plugin.json`'s `homepage` or
  `repository` field no longer matches the `git remote get-url origin` URL.
  `init` writes both fields from the detected origin, so drift means a
  hand-edited plugin.json or a renamed/stale remote that wasn't regenerated.
  Warning (not failure): a maintainer may intentionally host the plugin
  elsewhere.
- The check stays pure — no subprocess spawn inside `verify::discovery`. The
  git origin URL is detected once at introspection (`introspect::detect_repo_url`,
  already cached on `ProjectProfile.repo_url`) and threaded into the
  discovery stage via a new `VerifyInput.repo_url` field, preserving the
  module's "pure functions over their inputs" contract.
- Extends `verify --fix`: `discovery.plugin.url_drift` is now mapped to
  `RegenPluginJson` (same mechanical fix as `version_drift` — regenerate ONLY
  plugin.json, leave SKILL.md/marketplace.json intact). `action_for` is the
  only registration point: an exhaustive match makes forgetting the arm a
  compile failure.
- Skips silently on a repo with no git origin (no canonical URL to drift
  against) — `profile.repo_url = None` short-circuits the check.

### Internal — verify plumbing surfaces repo_url to discovery

- `VerifyInput` gains a `repo_url: Option<String>` field. Both callers
  (`verify_rendered` in `init`'s pre-commit gate, `run_verify_inner` in the
  `verify` subcommand) populate it from `profile.repo_url`. `discovery::run`
  and `check_plugin_json` take it as a parameter — no new spawn, no global
  state, no breaking change to the public `verify::run(input)` signature
  beyond the added struct field.


## [0.9.0] - 2026-07-12

### Added — reusable GitHub Action workflow for CI drift gating

- `.github/workflows/skillpack.yml`: a reusable workflow consumers pin via
  `uses: nordicnode/skillpack/.github/workflows/skillpack.yml@v0.9.0`. Installs
  skillpack from crates.io (`cargo install skillpack --version <input> --locked`)
  and runs `skillpack verify --format json` on a multi-OS + multi-runtime matrix
  mirroring the project's own `ci.yml`. Inputs: `skillpack-version` (default
  `0.9.0`). README documents the drop-in usage between Quick start and the
  dogfood section.

### Added — `doctor --format json` (stable machine-readable diagnosis)

- `doctor --format json` emits the serialized `ProjectProfile` (introspection
  result + decision trace) as a stable JSON object for CI/scripts. Mirrors
  `verify --format json`'s contract. The `diag` decision trace is ALWAYS
  present as an array (empty `[]` on clean runs, non-empty when candidate
  fns pushed falsy-branch notes) — removed the `skip_serializing_if =
  DiagTrace::is_empty` so `profile["diag"]` never KeyErrors. Read-only,
  non-gating — `doctor` always exits 0 (human + JSON form alike). Test
  `doctor_format_json_is_machine_readable` pins the contract: scalar field
  types + always-present `diag` array + per-entry `{ stage, note }` shape.

### Added — `discovery.skill.allowed_tools` grammar check

- `verify` now checks `allowed-tools` frontmatter against the Anthropic
  GRAMMAR (not an enumerated allowlist): each comma-separated token must be a
  bare identifier (`Read`) or a namespaced call (`Bash(npm test:*)`). Validates
  unbalanced parens, non-alpha identifiers, and empty tokens; reports
  `discovery.skill.allowed_tools` at WARN severity (doesn't gate). Applied to
  both Claude (`skills/<name>/SKILL.md`) and Codex (`.codex/skills/<name>/SKILL.md`)
  SKILL.md fronts. Membership-validation rejected as a brittle false-fail the
  moment Anthropic ships new tools. The `allowed_tools_hint` emit also moved
  from space-separated `"Read Bash"` to comma-separated `"Read, Bash"` to
  match the grammar — freshly-init'd packs no longer self-warned. Tests:
  `verify_warns_on_malformed_allowed_tools_grammar` (control: `Read`, `Bash(*)`
  pass; `Bash(`, `4R3ad` flagged) + `verify_passes_on_valid_allowed_tools_grammar`.

### Added — `verify --fix` (surgical drift repair)

- `verify --fix` mechanically repairs detected drift. Scope is deliberately
  narrow: only the file the drift lives in is rewritten — never wholesale regen
  (that's `skillpack init`). One fixable drift supported at launch:
  `discovery.plugin.version_drift` — regenerates ONLY `.claude-plugin/plugin.json`
  from the current manifest + intent, leaving `SKILL.md`/`marketplace.json`
  intact. Pre-fix report is suppressed; the post-fix verify re-runs and prints
  the post-fix report. An `✓ applied N fix(es), wrote: <files>` summary line
  precedes the report. Empty when no fixable drift — `--fix` is a no-op.
  The `FixAction` enum uses exhaustive `apply` match so adding a variant +
  forgetting an applier arm is a compile error, not a silent skip. Tests:
  `verify_fix_repairs_version_drift_surgically` (asserts surgical) +
  `verify_fix_is_noop_when_no_fixable_drift`.

### Added — self-dogfood drift gate (byte-identical regen test)

- `self_dogfood_regenerated_artifacts_match_committed_byte_identical`
  integration test: copies the repo's minimum files to a temp dir, runs
  `skillpack init --target <all 5> --non-interactive`, and asserts the five
  body files (`skills/skillpack/SKILL.md`, `.codex/skills/skillpack/SKILL.md`,
  `.cursor/rules/skillpack.mdc`, `.opencode/agents/skillpack.md`,
  `.github/copilot-instructions.md`) are byte-identical to the committed
  versions. Catches the F1-class drift that `verify`-passing hides:
  missing `globs:`, wrong opencode `mode`, stale `allowed-tools` grammar.
  Skips `plugin.json` / `marketplace.json` (their `url` / `repository`
  fields carry the git origin URL, which differs between the temp-dir copy
  and the GitHub-hosted source) — `verify`-passes covers those two.

### Fixed

- `DiagTrace` serialization no longer uses `skip_serializing_if = "is_empty"`:
  the `diag` field now ALWAYS serializes (empty `[]` on clean runs) so
  consumers of `doctor --format json` can rely on `profile["diag"]` existing.

### Internal

- The 0.8.8-class committed-artifact drift (cursor `globs:` missing, opencode
  `mode: subagent` when should be `primary`, body-stale subcommand flag lists
  after adding `--fix` / `doctor --format`) is now caught at test time by
  the new byte-identical self-dogfood gate, not only by ad-hoc dogfood runs.

## [0.8.8] - 2026-07-12

### Fixed — doctor `desc_hint` surfaces raw HTML when README leads with markup

- `doctor --verbose`'s `desc_hint` (the surfaced README preview) returned raw
  `<div align="center"><img src="docs/logo.png" ...></div>` markup on READMEs
  that begin with an HTML block (logos, banners). The `read_readme_hint`
  `skip_while` predicate only skipped markdown headings (`#`) and image lines
  (`!`); it didn't skip raw HTML tags. Extended the predicate to also skip
  lines starting with `<`, so the hint lands on the first real prose line.
  Affects the `--verbose` diagnostic display only — generated distribution
  files (`SKILL.md`, `plugin.json`, etc.) never read `desc_hint`; their
  description comes from `intent.one_line_description` in `skillpack.toml`,
  so no shipped artifacts changed.
  Regression test `read_readme_hint_skips_leading_html_div` in
  `src/introspect.rs` asserts the predicate drops HTML and lands on prose;
  reproduces the skillpack self-dogfood gap (this repo's own README leading
  with a div-wrapped logo).

### Fixed — committed distribution artifacts regenerated after introspection drift

- Re-running `skillpack init` on this repo (self-dogfood) surfaced two
  committed-artifact drifts caused by hand-edits + a changed default heuristic:
  - `.cursor/rules/skillpack.mdc` was missing `globs: ["*.rs"]` (the cursor
    auto-attach frontmatter line derived from `Language::Rust` via
    `cursor_globs_hint`).
  - `.opencode/agents/skillpack.md` carried `mode: subagent`; for a detected
    Rust CLI `opencode_mode_hint` correctly returns `"primary"` (Unknown →
    `"subagent"`). The demo doc (`docs/agent-demo.md` condition-B) confirms
    `primary` is the right mode for a Rust CLI agent invoked via
    `opencode run --agent <name>`.
  Neither was a code bug — the binary was correct; the committed files were
  stale. Regenerated all 5 targets so the committed artifacts match what a
  fresh `skillpack init` produces on this repo. Also picks up the new
  transparent-background `docs/logo.png`.

## [0.8.7] - 2026-07-12

### Changed — internal: introspect workspace helpers split

- Extracted the workspace-root heuristics from `src/introspect.rs` into a new
  sibling module `src/introspect/workspace.rs` (123 lines): `is_cargo_workspace_only`,
  `is_npm_workspace_only`, `pyproject_has_tool`, `first_cargo_member_name`, and
  `first_npm_member_name`. These are pure manifest-structure reads (no spawn)
  shared by `detect_language` and `detect_cli`; kept separate from
  `manifest.rs` because `manifest`'s contract is "pull a scalar from one
  manifest", whereas these walk `[workspace].members` / `workspaces` arrays
  and reason about structure. `introspect.rs` shrank from 1153 to 1052 lines
  (-9 %). Pure refactor — no public-API or behavior change: a `pub(crate) use
  workspace::{…}` re-export at the parent root preserves the flat
  `crate::introspect::is_*_workspace_only` / `first_*_member_name` import paths,
  so `detect_language`, `introspect`, and `detect_cli` bare call sites are
  unchanged. `walk_cargo_workspace` + `walk_npm_workspace` stay in the parent
  (they call parent-side `spawn_candidate`); `has_gemspec` / `has_csproj` stay
  parent-side (pure presence checks used only by `detect_language`).
  Self-dogfood: `skillpack verify` on this repo still reports 12 passed,
  discoverability score 100/100.

### Fixed — empty invocation block on config replay

- `init --non-interactive` re-runs from a `skillpack.toml` that omits
  `invocation_command` (the field is elided on save by
  `skip_serializing_if = "Option::is_none"`) produced a SKILL.md with an
  empty fenced invocation block for a CLI project — the template rendered
  `{{ invocation_command }}` as the empty string because `intent.invocation_command`
  yielded `None`. The interactive interview path already had a `suggest`
  default for blank answers; the config replay path had no such recovery.
  Fixed in `templates/skill_body.md.tera` by falling back to `cli_binary`
  (already derived from `cli_command` in `build_context`) via Tera's
  `default(value=cli_binary)` filter. Self-dogfood surfaced this on a fresh
  Rust CLI fixture: the generated SKILL.md now shows `frob-cli` in the
  invocation block instead of empty ticks. Regression test
  `invocation_block_falls_back_to_cli_binary_when_intent_omits_command`
  in `src/generate.rs` asserts the fallback fires; verified it fails when
  the template fix is reverted. Existing snapshots unchanged (fixtures
  supply `invocation_command: Some("chronicle --new \"entry\"")`).

## [0.8.6] - 2026-07-12

### Changed — internal: discovery module split

- Extracted the per-ecosystem discovery checks from `src/verify/discovery.rs`
  into three sibling modules under `src/verify/discovery/`:
  `cursor.rs` (`CursorFrontmatter` + `parse_cursor_mdc_frontmatter` +
  `check_one_mdc` + `find_cursor_mdc_files`, 172 lines),
  `opencode.rs` (`OpenCodeFrontmatter` + `parse_opencode_agent_frontmatter` +
  `check_one_opencode_agent` + `find_opencode_agent_files`, 147 lines), and
  `copilot.rs` (`find_copilot_instructions` + `check_copilot_instructions`,
  70 lines). `discovery.rs` shrank from 1041 to 689 lines (-34 %); the three
  new leaves total 389 lines. Pure refactor — no public-API or behavior
  change: `pub use` re-exports preserve the `crate::verify::discovery::`
  import paths for `parse_cursor_mdc_frontmatter`, `CursorFrontmatter`,
  `parse_opencode_agent_frontmatter`, and `OpenCodeFrontmatter`, so the
  property test (`tests/property_proptest.rs` L11) and the `run()` orchestrator
  are unchanged. Shared helpers `rel_unix` + `find_kv_colon` were lifted to
  `pub(crate)`; `is_valid_kebab`, `validate_relative_source`, `read_optional`,
  and the Claude/Codex/`store` core (marketplace.json + plugin.json +
  SKILL.md frontmatter) stay in the parent. Self-dogfood: `skillpack verify`
  on this repo still reports 12 passed, discoverability score 100/100.

## [0.8.5] - 2026-07-11

### Changed — internal: introspect module split

- Extracted the per-language CLI candidate resolvers (rust, node, go, python,
  ruby, php, jvm, csharp) plus their backing structs (`CliCandidate`,
  `DetectCli`), the language dispatch (`primary_cli_candidate`), the private
  helpers (`cargo_bin_names`, `has_go_main`, `python_script_package`,
  `canonicalize_for_argv`), and the Windows-aware `which_on_path` PATH resolver
  from `src/introspect.rs` into a new sibling module
  `src/introspect/cli_candidates.rs`. `introspect.rs` shrank from 1578 to 1153
  lines (-27 %); the new `cli_candidates.rs` holds 451 lines. Pure refactor —
  no public-API or behavior change: the `pub(crate)` re-exports
  (`CliCandidate`, `DetectCli`, `primary_cli_candidate`, `which_on_path`) keep
  the flat import paths unchanged, so `detect_cli`/`spawn_candidate`/
  `capture_subcommand_help` and the candidate/parse test modules need no edits.
  The `which_on_path` re-export and the `std::path::PathBuf` import are
  `#[cfg(test)]`-gated since only the test modules consume them.

## [0.8.4] - 2026-07-11

### Changed — internal: introspect module split

- Extracted the per-language manifest field parsers (name, version, authors,
  license) plus their helpers (`extract_xml_tag`, `extract_gradle_string`,
  `extract_ruby_string_value`, `select_csproj`, `strip_author_email`) from
  `src/introspect.rs` into a new sibling module `src/introspect/manifest.rs`.
  `introspect.rs` shrank from 2055 to 1578 lines (-23 %); the new
  `manifest.rs` holds 535 lines, including the 5 manifest-field regression
  tests that moved with the code. Pure refactor — no public-API or
  behavior change: the public `introspect()` entry and the `pub(crate)`
  re-exports (`project_manifest_version`, `select_csproj`) keep the old
  flat import paths unchanged, so `verify::discovery` and
  `csharp_cli_candidate` call sites need no edits. `strip_author_email`
  now strips inside `project_manifest_authors` (was at the call site).


## [0.8.3] - 2026-07-11

### Added — discoverability score in verify output

- `verify` now computes a 0-100 `discoverability_score`: each check
  contributes Pass = 1.0, Warn = 0.5, Error = 0.0, divided over non-skipped
  checks (Skipped excluded from the denominator; all-skipped or empty → 0,
  an honest "nothing verified" rather than a misleading 100). Exposed as
  `discoverability_score` (integer) in the `--format json` report and in
  the human summary line. The score does not gate the exit code — only
  critical failures do — so it's a tracking signal, not a pass/fail gate.
- New `VerifyReport::discoverability_score` method + 7 unit tests covering
  edge cases (all-pass=100, warn=partial, error lowers, all-skipped=0,
  empty=0, skipped-excluded, all-errors=0).
- Integration test `verify_format_json_is_machine_readable` extended to
  assert the score field is present, numeric, and matches the expected
  value (90 — the rust-cli fixture emits one `discovery.plugin.author`
  warning, so 4 pass + 1 warn = 4.5/5 = 90).

## [0.8.2] - 2026-07-11

### Added — verify catches plugin.json version drift

- New `discovery.plugin.version_drift` check: `verify` now compares the
  `version` in `.claude-plugin/plugin.json` against the project manifest
  version (`Cargo.toml [package].version`, `package.json "version"`,
  `pyproject.toml [project].version`, etc.) and warns on mismatch. Drift
  is the canonical stale-dogfood / bumped-manifest-without-regeneration bug;
  warns (not fails) so maintainers may intentionally pin a plugin version.
  Previously drift was invisible — `verify` only checked plugin.json had *a*
  version field, never compared it to the manifest.
- `detect_language` and `project_manifest_version` in `src/introspect.rs`
  bumped to `pub(crate)` so verify's discovery checks reuse the canonical
  per-language version parser (no duplicate logic, no new public API).
- Self-dogfood surfaced real drift: committed `.claude-plugin/plugin.json`
  was `0.6.4` while `Cargo.toml` was `0.8.1` (stale from a 0.6.4-era dogfood
  run, never refreshed). Fixed committed `plugin.json` to `0.8.2`.
- New integration test `verify_warns_on_plugin_json_version_drift`
  exercises both control (fresh `init` → no drift) and drift (mutate to
  `9.9.9-fake` → warning fires naming both versions, `verify` exits 0).
- README "What verify checks" adds a bullet for `version_drift`. Plugin.json
  version updated 0.6.4 → 0.8.2 to match Cargo.toml.

## [0.8.1] - 2026-07-11

### Fixed — introspection + flag extraction (self-dogfood on fd-find)

- `rust_cli_candidate` now parses `[[bin]].name` from `Cargo.toml` and probes
  those artifact paths before the package-name fallback. Crates that rename
  their binary (e.g. `fd-find` → `fd`, `ripgrep` → `rg`) were misdetected as
  libraries — `has_cli=false`, template emitted "library not CLI" branch.
  PATH fallback also extended to probe `[[bin]].name` candidates first.
- `extract_flags` now strips clap-style `[=<value>]` optional-arg suffix
  *before* punctuation trim. Previously raw `--help` tokens kept `[` (interior
  → survived `trim_matches`) while backtick-wrapped SKILL.md tokens stripped
  `[` (edge → punct) → same flag extracted differently → false flag-drift
  failure on verify. Now produces consistent `--flag` both sides.
- `extract_flags` now rejects prose tokens from rich clap help: multi-char
  short flags (`-tf` from `fd -tf` examples), example patterns (`-foo`),
  short/long pair separators (`-x'/'--exec`), and find(1) comparisons
  (`-mount`, `-xdev`). Filters: reject tokens containing `/` or `'`; reject
  single-dash flags with >1 char after dash (real clap short flags are single
  letter).
- `cli_binary` in generated SKILL.md now derives from the actual CLI command
  argv (`Path::file_stem()`) rather than the package name. SKILL.md prose
  now reads "Ensure `fd` is installed" (binary) instead of "Ensure `fd-find`
  is installed" (crate) — correct binary name for agent invocation.
- 4 new tests pin these fixes: `rust_candidate_probes_bin_name_not_package_name`,
  `strips_clap_optional_arg_suffix_consistently`, `ignores_prose_examples_in_help_text`.

## [0.8.0] - 2026-07-11

### Added — C# / .NET ecosystem support

- New **language** ecosystem: C# / .NET (`*.csproj` detection via
  `has_csproj` glob, SDK-style projects). `select_csproj` picks
  deterministically: sort by filename, prefer `OutputType=Exe`, skip
  `WinExe` (GUI, no stdout), lexicographic tiebreak. `csharp_cli_candidate`
  emits `dotnet run --project <csproj> --` (trailing `--` separator so
  appended `--help` reaches the app, not dotnet). Cursor globs:
  `["*.cs", "*.csproj", "*.sln"]`. CI adds `setup-dotnet@v4` (net8.0).
  Fixture `tests/fixtures/repos/csharp-cli/` with SDK-style csproj +
  `Program.cs` `--help` handler. Integration test `#[ignore]`-gated
  (self-skips without dotnet, pre-builds with `dotnet build -v q`).

## [0.7.0] - 2026-07-11

### Added — multi-ecosystem depth + language coverage

- Two new **language** ecosystems: PHP (`composer.json`) and JVM (Maven
  `pom.xml` + Gradle `build.gradle`/`.kts`). Introspection detects manifests,
  extracts name/version/authors, and resolves CLI candidates (`php_cli_candidate`
  mirrors `node_cli_candidate` with `php` runtime + `bin` script resolution;
  `jvm_cli_candidate` probes pre-built Gradle `installDist` scripts + Maven/
  Gradle shaded jars — no build invocation, pure filesystem reads per design
  §6.3). Cursor globs derive from language: PHP `["*.php", "composer.json"]`,
  JVM `["*.java", "*.kt", "*.scala", "pom.xml", "build.gradle",
  "build.gradle.kts"]`.
- **Cursor `globs` frontmatter** now derived from detected language for
  auto-attach (was missing entirely — generated Cursor rules wouldn't
  trigger on real work).
- **OpenCode `mode` frontmatter** now derived from project shape (`primary`
  for CLI tools, `subagent` for libraries); previously hardcoded `subagent`.
- **Template refactor**: 4 near-identical ecosystem templates → shared
  `skill_body.md.tera` partial + per-target frontmatter-only wrappers.
  Adding a 6th `Target` is now ~10 lines instead of ~60.
- New insta snapshots for Cursor, OpenCode, Copilot, PHP-Cursor, JVM-Cursor
  lock rendered output from regression.
- CI matrix adds `setup-php` and `setup-java` for cross-OS runtime coverage
  on `--include-ignored` round-trip tests.

### Changed

- `Language::Php`, `Language::Jvm` variants extend the `Language` enum and
  every match site (`category_hint`, `cursor_globs_hint`, introspect
  `detect_language`, manifest name/version/authors, `primary_cli_candidate`).
- `cli.rs` `about` string updated to name all 5 agent ecosystems (was
  leaking "Claude Code skill packs" into `--help`).

## [0.6.4] - 2026-07-11

### Fixed — Windows spawn + report normalization

- **`\\?\` UNC prefix from `std::fs::canonicalize` broke node module loading
  on Windows**. `canonicalize` returns `\\?\C:\path\bin\cli.js` on Windows; the
  kernel's `CreateProcess` accepts that for native exes, but Node's V8 module
  loader rejects `\\?\` paths — `node \\?\C:\foo\cli.js --help` exits non-zero
  → `verify`'s `invocation.help_present` fired `RanNonZero` → init's critical
  gate failed → init refused to write on Windows. New `canonicalize_for_argv`
  helper strips the `\\?\` prefix post-canonicalize, applied at the rust,
  node, and ruby CLI candidate sites. Unix is a no-op.
- **`HELP_TIMEOUT` raised 8s → 15s**. Windows CI's cold-cache `go run .
  --help` first-compile cost (GOCACHE build + AV scan) exceeded 8s, false-
  timing-out the go round-trip test. 15s covers CI cold-cache compile
  while still bounding hung CLIs (a CLI that can't print `--help` in 15s is
  not one an agent should invoke anyway, so the cap doubles as fail-safe).
- **`verify` report paths normalized to forward slashes**. `discovery.rs`
  rendered relative paths via `path.strip_prefix(root).to_string_lossy()`,
  which on Windows emits `skills\skillpack\SKILL.md` — but the marketplace
  schema requires forward-slash-only paths, snapshot tests pin forward
  slashes, and downstream tools grep the report on `/`. New `rel_unix`
  helper (replace `\` with `/`) applied at the SKILL.md, .mdc, OpenCode
  agent, and Copilot instructions check sites. Unix unchanged.

## [0.6.3] - 2026-07-11

### Fixed — Windows rust CLI detection

- **`rust_cli_candidate` missed built artifacts on Windows**. The fn joined
  `target/{release,debug}/<name>` with the bare name, but `cargo build` on
  Windows writes `<name>.exe`. `p.exists()` returned false → `has_cli=false`
  on a real built CLI → `doctor` falsely reported no CLI and `verify`'s
  invocation checks descended into the `not_runnable_here` warning path +
  critical gate failure, refusing to write init output on Windows. Now
  appends `.exe` on Windows (`cfg!`), leaves Unix untouched. The PATH
  fallback already handles PATHEXT via `which_on_path`.

## [0.6.2] - 2026-07-11

### Fixed — Windows build correctness

- **Three lib tests failed on Windows** (introduced/exposed by the 0.6.1
  matrix; the code was unix-only before). All three now cross-OS:
  - `python_candidate_uses_m_module_when_importable` asserted
    `argv[0].ends_with("python")`; the PATHEXT fix correctly resolves
    `python.exe`, so the old assert rejected a valid Windows path. Replaced
    with a `Path::file_stem().eq_ignore_ascii_case("python")` check that
    accepts `python`, `python.exe`, `python3.exe`, etc.
  - `node_cli_detected_via_bin_absolute_argv` had the same `ends_with("node")`
    bug plus a string-suffix `script.ends_with("bin/cli.js")` that misses on
    `\`-separated Windows paths. Now uses `Path::ends_with` (component-aware,
    cross-OS) for the script tail + `file_stem` for the node binary stem.
  - `skill_md_has_description_and_when_to_use_in_frontmatter` failed because
    the repo had no `.gitattributes`: a Windows checkout with the default
    `core.autocrlf=true` converted the `.tera` template sources to CRLF,
    `include_str!` pulled CRLF bytes, Tera preserved them, and the generated
    SKILL.md started with `---\r\n` instead of `---\n`. The real fix is not a
    test-string patch — a generated SKILL.md shipping with CRLF on Windows
    and LF on Unix violates byte-identical output. Added `.gitattributes`
    pinning `*.tera` (and the rest of the source) to `eol=lf`, so templates
    keep LF on every checkout regardless of the host's autocrlf setting.

## [0.6.1] - 2026-07-11

### Fixed — CI

- **Windows CI hard-failed on `unused_imports`**: `src/spawn.rs` test module
  imported `use super::*` but every test fn inside is `#[cfg(unix)]`, so Windows
  compiled the import with zero usages → `clippy -D warnings` turned the
  `unused_imports` warning into a hard error. The new windows-latest matrix
  entry surfaced this pre-existing bug (the 0.6.0 PATHEXT work was unrelated).
  The import is now `#[cfg(unix)] use super::*` to match the tests it serves.

## [0.6.0] - 2026-07-11

### Added — Cross-platform CI matrix

- **CI now runs on ubuntu-latest, macos-latest, and windows-latest**, not
  just ubuntu. The matrix uses the official `setup-node`, `setup-python`,
  `setup-go`, and `ruby/setup-ruby` actions so every OS gets working runtimes
  without hand-rolled apt/choco/brew logic. A previously-unchecked Windows
  build regression would have shipped undetected.

### Fixed — Windows PATH resolution (PATHEXT)

- **`which_on_path` now enumerates `PATHEXT` on Windows**. `cmd.exe` appends
  `PATHEXT` (`.EXE;.CMD;.BAT;…`) to a bare name; Rust's `Command::new` does
  not, so probing `"node"` missed `node.exe` and `has_cli` silently reported
  `false`. The probe now checks `name` plus `name{ext}` for each PATHEXT
  entry on Windows; Unix path lookup is unchanged.
- **Three CLI candidates discarded the resolved binary path and re-spawned
  the bare name**, which defeats PATHEXT resolution even when `which_on_path`
  finds the binary. `rust_cli_candidate`, `python_cli_candidate`, and
  `ruby_cli_candidate` now use the resolved `PathBuf` as `argv[0]` (the
  `node_cli_candidate` pattern), so the spawn finds the real
  `node.exe`/`python.exe`/`ruby.exe` on Windows instead of failing with
  `NotFound`. The README "Platform" caveat is retired — Windows now works.

### Tests

- `which_on_path_returns_existing_file` sanity-checks that a PATH lookup
  returns a real file on every CI OS. The PATHEXT-specific branch is
  exercised end-to-end by the new windows-latest CI matrix entry against
  real `cmd.exe` / `node.exe` lookups, not a racy synthetic env mutation.

## [0.5.1] - 2026-07-11

### Fixed — Template correctness (BLOCKER)

- **Double-escaped `one_line_description` in generated output**: the YAML-escaped
  value was reused in markdown bodies and JSON templates, producing literal `\"`
  in skill files and marketplace/plugin JSON. The render context now exposes two
  keys: `one_line_description` (YAML-escaped, frontmatter only) and
  `one_line_description_raw` (raw, used in markdown bodies + JSON). Both JSON
  templates pipe through `json_encode | safe`; the markdown templates emit raw.
- **Dangling sentences from `{%-` trim markers**: all four markdown templates
  (`SKILL.md`, `cursor-rule.mdc`, `opencode-agent.md`, `copilot-instructions.md`)
  used Tera `{%-`/`-%}` whitespace-trim markers that over-trimmed and glued
  headings to preceding content. Rewritten to bare `{% if %}` tags on own lines
  (no trim markers); the tag emits nothing but its source-line `\n` survives as
  exactly one blank line. Inline tags (`{% if cond %}content{% endif %}`) on the
  same line as content emit no extra newlines.
- **Empty sections rendered when phrases/flags absent**: the "When to use"
  section now wraps in `{% if when_to_use_phrases %}...{% endif %}` and the
  flags intro+loop wraps in `{% if documented_flags %}...{% endif %}` across
  all four markdown templates. A library with no `when_to_use_phrases` no longer
  emits a "When to use" heading with an empty bullet list.

### Fixed — Introspection bugs

- **`HELP_TIMEOUT` divergence**: `introspect.rs` used 5s while
  `verify::invocation` used 8s — a CLI that printed help in 6s passed verify but
  had its flags dropped during `init` introspection (silent false-negative).
  Unified to a single `pub const HELP_TIMEOUT: Duration = Duration::from_secs(8)`
  in `spawn.rs`; both call sites import it.
- **Go module line trailing comment bleed**: `project_manifest_name` for Go
  trimmed outer whitespace only, so `module github.com/foo/bar // bar tool`
  produced a module name of "tool" (or worse) by taking the last `/`-segment
  of the comment-bleeded path. Now takes the first whitespace-delimited token
  before splitting, correctly yielding "bar". New test:
  `go_module_name_strips_trailing_line_comment`.
- **Workspace walk early-return on nameless member**: `walk_cargo_workspace`
  and `walk_npm_workspace` used `?` on the `member_name` resolution, so a
  single member manifest with no resolvable name aborted the entire walk —
  hiding CLI detection for every *other* member and silently reporting
  `has_cli=false`. Replaced with `let ... else { diag.push(...); continue; }`
  so the walk skips the pathological member and continues. New tests:
  `walk_cargo_workspace_continues_past_no_artifact_member`,
  `walk_npm_workspace_continues_past_no_cli_member`.
- **`detect_repo_url` git hang**: spawned `git remote get-url origin` with no
  timeout — a credential-prompt stall (`git@github.com` with no SSH agent on a
  fresh host) blocked introspection indefinitely. Routed through
  `spawn_with_timeout(&mut cmd, Duration::from_secs(3))`; `RanClean` returns
  the trimmed URL, all other `SpawnOutcome` variants return `None`.

### Fixed — Verify discovery sentinels

- **Empty ecosystem directories passed silently**: a `.codex/skills/` directory
  that existed but contained no `SKILL.md` (e.g. files removed after init)
  passed verify with no check emitted — a silent false-negative. Added
  `.missing` fail sentinels for empty `.codex/skills/`, `.cursor/rules/`, and
  `.opencode/agents/` directories. New check ids:
  `discovery.codex.skill.missing`, `discovery.cursor.mdc.missing`,
  `discovery.opencode.agent.missing`. New test:
  `codex_empty_skills_dir_fails_verify`.
- **Copilot frontmatter passed verify**: a `.github/copilot-instructions.md`
  starting with a `---` frontmatter block passed verify (the existing heading
  check fired on the YAML content). The Copilot spec says "plain markdown, no
  frontmatter" — now a `---` prefix triggers
  `discovery.copilot.instructions` at fail severity before the heading check.
  New test: `copilot_frontmatter_fails_verify`.

### Added — Test coverage

- **All-5-targets round trip**: `all_five_targets_init_then_verify_round_trip`
  asserts `init --target claude --target cursor --target codex --target opencode --target copilot`
  emits all 5 files and `verify` passes with all 5 `discovery.*` check ids at
  `pass` severity.
- **Discovery.empty on bare repo**: `verify_on_empty_repo_fails_with_discovery_empty`
  asserts a repo with no ecosystem files emits `discovery.empty` at fail severity.
- **Self-dogfood all ecosystems**: `self_dogfood_verify_asserts_all_ecosystems`
  runs `verify` against the skillpack repo itself and asserts all 6 check ids
  (marketplace, skill, cursor.mdc, codex.skill, opencode.agent, copilot.instructions) pass.
- **Doctor on plain Rust CLI**: `doctor_on_plain_rust_cli_reports_has_cli_true`
  builds the `rust-cli` fixture, runs `doctor`, and asserts `has_cli:  true`.

## [0.5.0] - 2026-07-10

### Added — `doctor` subcommand

- **`skillpack doctor`** — diagnose why introspection chose `has_cli` / language
  as it did. Prints the detected profile + a chronological decision trace
  (every falsy branch in `detect_cli` / `detect_language` pushes a `DiagNote`
  explaining why a candidate was rejected). Read-only, exits 0. Answers the
  user's "what happens with monorepos / workspaces / uv / poetry?" question
  directly: the trace explains the workspace walk and the uv/poetry gap.

### Added — Workspace member walking

- **Cargo workspace support**: a workspace-only `Cargo.toml` (no `[package]`)
  no longer silently reports `has_cli=false`. Introspection now walks
  `[workspace].members`, probes each member crate's built/installed binary,
  and derives the profile name from the member that ships the CLI.
- **npm workspace support**: a root `package.json` with `workspaces` but no
  `bin` walks member packages, probing each for a `bin` entry.
- **uv / poetry**: explicitly diagnosed as "member walking not yet
  implemented — run `skillpack init` in the member dir" (honest gap, no
  false promise).

### Added — OpenCode + GitHub Copilot targets

- **`--target opencode`**: emits `.opencode/agents/<name>.md` with
  `description` (required) + `mode` frontmatter per opencode.ai/docs/agents.
  `verify` validates the frontmatter (`discovery.opencode.agent.*` check ids).
- **`--target copilot`**: emits `.github/copilot-instructions.md` (plain
  markdown, no frontmatter) per docs.github.com/copilot. `verify` checks
  the file is non-empty and starts with a `#` heading
  (`discovery.copilot.instructions` check id).
- The repo's own committed distribution now dogfoods all 5 ecosystems.


## [0.4.0] - 2026-07-10

### Added — Multi-ecosystem verify (design §3 Phase 4)

- **`verify` now checks the Cursor and Codex distribution files**, not just
  Claude's. Previously `init --target cursor --target codex` emitted those
  files but `verify` silently ignored them — a broken `.mdc` or Codex
  `SKILL.md` would pass `verify` and ship undetected. The discovery suite
  now runs per-ecosystem:
  - **Cursor** (`.cursor/rules/<name>.mdc`): new `check_one_mdc` parses the
    YAML frontmatter and fails on a missing/empty `description` or a
    description over the 1,536-char listing cap, warns on a missing or
    non-boolean `alwaysApply`. Schema sourced from cursor.com/docs/rules
    (verified July 2026).
  - **Codex** (`.codex/skills/<name>/SKILL.md`): reuses the existing
    `check_one_skill_md` validator with a distinct `discovery.codex.skill`
    check_id prefix — same `SKILL.md` frontmatter shape as Claude, different
    output path.
  - Check ids are namespaced per ecosystem (`discovery.skill.*`,
    `discovery.codex.skill.*`, `discovery.cursor.mdc.*`) so a CI report
    names *which* ecosystem file drifted.

### Fixed

- **`verify` no longer hard-fails on a missing `.claude-plugin/`** — the
  `marketplace.json` / `plugin.json` checks now run only when the Claude
  distribution directory is present. A `--target cursor`-only pack
  (legitimately without `.claude-plugin/`) was previously blocked by a
  false positive `discovery.marketplace.missing` failure; `verify` now
  skips the Claude checks entirely. An empty repo (no ecosystem files at
  all) still emits a single `discovery.empty` failure so a typo'd `verify`
  doesn't silently pass.

### Notes

- The human `verify` report now prints the per-file message for passing
  checks (previously suppressed), so multi-ecosystem passes are visually
  distinguishable (e.g. `skills/skillpack/SKILL.md validates` vs
  `.codex/skills/skillpack/SKILL.md validates`). JSON output was already
  per-file.
- Schema constants live in `src/verify/schema.rs` with cited sources:
  `CLAUDE_PLUGIN_DIR`, `CODEX_SKILLS_DIR`, `CURSOR_RULES_DIR` join the
  existing `PLUGIN_JSON_PATH` / `MARKETPLACE_JSON_PATH`.
- Invocation drift still runs against the first documented CLI's
  SKILL.md only (design §5.2 step 3, unchanged from 0.2.1). Cursor/Codex
  bodies are identical to the Claude SKILL.md body (same template), so a
  Claude invocation-drift pass transitively validates the parallel
  ecosystem files — per-ecosystem invocation drift is deferred to when a
  pack legitimately ships divergent CLI surfaces per ecosystem.

## [0.3.0] - 2026-07-10

### Added — Multi-ecosystem emitter (design §3 Phase 4)

- **`--target` flag on `init`** — generate distribution files for multiple
  agent ecosystems in a single run. Repeatable: `--target claude --target
  cursor --target codex`. Defaults to `claude` only (backward compatible).
  New `Target` enum (`Claude`, `Cursor`, `Codex`) drives `render_targets`,
  which dispatches per ecosystem:
  - **Claude** (unchanged): `.claude-plugin/marketplace.json` +
    `plugin.json` + `skills/<name>/SKILL.md`.
  - **Cursor**: `.cursor/rules/<name>.mdc` — YAML frontmatter
    (`description`, `alwaysApply: false`) + rule body. Matches the Cursor
    `.mdc` format documented at cursor.com/docs/rules.
  - **Codex**: `.codex/skills/<name>/SKILL.md` — same `SKILL.md` frontmatter
    as Claude (cross-agent-compatible), different output path per Codex's
    `.codex/skills/` convention.
- **Self-dogfood** now generates all three: the repo is installable as a
  Cursor rule (`@skillpack`) and a Codex skill (`.codex/skills/skillpack/`)
  in addition to `claude plugin marketplace add nordicnode/skillpack`.

### Notes

- `verify` remains Claude-only for V1; cursor/codex files are emitted but
  not yet checked by the discovery/invocation suite. Multi-ecosystem verify
  is a follow-up.
- `--target` is CLI-only (not persisted in `skillpack.toml`); targets are a
  per-run choice, not project metadata.

## [0.2.5] - 2026-07-10

### Fixed

- **`plugin.json` author defaulted to `Unspecified`** — same drift class as
  the version fix in 0.2.4: `generate` fell back to the template's
  `"Unspecified"` sentinel whenever the interview / `skillpack.toml` didn't
  supply an author, ignoring the manifest entirely. `introspect` now reads
  the first author from `Cargo.toml [package].authors`, `package.json
  "author"` (string or `{ name }` object), `pyproject.toml [project].authors`,
  and `*.gemspec spec.authors`. `generate` resolves `intent.author` with
  `profile.authors` as fallback, so a non-interactive `init` (the CI path,
  where no interview happens) still emits a real author. The Cargo
  `"Name <email>"` format is stripped to the display name only — `author.name`
  in the plugin schema is a display name, not a contact record.

### Added

- **`discovery.plugin.author` verify check** — warns when `plugin.json` has
  no author or defaults to `"Unspecified"`, with a suggestion pointing at
  the manifest key to set. Parallel to `discovery.plugin.version` (0.2.4).

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
