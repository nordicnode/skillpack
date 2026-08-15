# Reference

Everything you'd want to look up without cluttering the README.

## `verify` checks in full

`skillpack verify` simulates an agent's first read of your project and reports
one result per check (`pass` / `warn` / `fail` / `skip`). Any `fail` makes
`verify` exit non-zero (and blocks `init` from writing); warnings are advisory.

### Discovery — structural validation per ecosystem

Claude Code (the `.claude-plugin/` + `skills/` set) is checked against the
documented plugin schema:

- plugin / marketplace names are kebab-case and not reserved
- `description` is present and the combined description + `when_to_use` stays
  under the 1,536-character listing cap
- `when_to_use` carries trigger phrases an agent can match on
- marketplace `source` paths use the `./` prefix and forward slashes only
- `version` is present in `plugin.json` (warns on missing/empty)
- `author` is present in `plugin.json` (warns on missing or `"Unspecified"`)
- `version` in `plugin.json` matches the project manifest version (warns on
  drift)
- `homepage` / `repository` in `plugin.json` match the git origin URL (warns on
  drift; both fields are reported, and the check is skipped when no git origin
  is configured)
- `allowed-tools` in `SKILL.md` frontmatter matches the Anthropic grammar
  (comma-separated; each token a bare identifier like `Read` or a namespaced
  call like `Bash(npm test:*)`) — warns on malformed tokens. Applied to Codex
  `SKILL.md` too.
- the SKILL.md frontmatter block is closed by a `---` delimiter (an
  unterminated block would swallow the body — fails). Applied to cursor `.mdc`
  and OpenCode agent files too.
- a `skills/<name>/SKILL.md` (or `.codex/skills/<name>/SKILL.md`) directory
  name matches its frontmatter `name:` (warns on mismatch — agents load skills
  by directory)

Other ecosystems:

- **Cursor** (`.cursor/rules/<name>.mdc`) — frontmatter parsed and validated
  against cursor.com/docs/rules: `description` present, non-empty, under the
  listing cap; `alwaysApply` present and boolean (missing or non-boolean
  warns).
- **Codex** (`.codex/skills/<name>/SKILL.md`) — reuses the same `SKILL.md`
  frontmatter schema as Claude (fields, length caps, name validation),
  namespaced under `discovery.codex.skill.*`.
- **OpenCode** (`.opencode/agents/<name>.md`) — `---` frontmatter block:
  `description` present, non-empty, under the listing cap (hard fail); `mode`
  (if present) one of `primary|subagent|all` (warn).
- **GitHub Copilot** (`.github/copilot-instructions.md`) — plain markdown:
  non-empty, first non-blank line starts with a `#` heading.
- **AGENTS.md** — plain markdown at the repo root: non-empty, `#` heading.
- **Cline** (`.clinerules/<name>.md`), **Roo Code** (`.roo/rules/<name>.md`),
  and **Kilo Code** (`.kilocode/rules/<name>.md`) — plain-markdown workspace
  rules (an optional `---` frontmatter block is tolerated; a missing `#`
  heading warns).
- **Goose** (`.goose/instructions.md`) — plain markdown at the repo root:
  non-empty, `#` heading.

A single-ecosystem pack (e.g. `--target copilot` alone) passes `verify`
without false-positive failures from the other ecosystems.

### Invocation — actually runs the documented CLI

- `--help` executes cleanly under a hard timeout
- every flag documented in `SKILL.md` exists in the real `--help` output
  (catches drift)
- flags the CLI advertises in `--help` that `SKILL.md` doesn't document (a
  discoverability warning)
- for CLIs with subcommands (clap-style `Commands:` sections), `init` captures
  the subcommand tree recursively (each subcommand's `--help`, plus nested
  sub-subcommands like `git remote add`) and documents them in a `### Subcommands`
  block with two-space nesting; `verify` spawns `<cli> <path...> --help` per
  documented subcommand path and drift-checks its flags
- `<cli> --version` output contains the `plugin.json` version (advisory —
  warns on mismatch, skips silently if `--version` exits non-zero or prints
  nothing)

`verify` works on hand-written skill packs too, not just `init` output: it
derives whether a CLI is documented from the `SKILL.md` itself (a
`## Invocation` section, or a fenced block with `--flags`). If the skill
documents a CLI but no runnable binary is found on your machine, the
invocation check is reported as a **warning** (not silently skipped). The
invocation check runs against the **first** documented CLI; discovery checks
run against every `SKILL.md` (a plugin may ship several).

### Discoverability score

Every `verify` run computes a 0–100 score: each check contributes
Pass = 1.0, Warn = 0.5, Error = 0.0, divided over non-skipped checks. The JSON
report carries it as `discoverability_score` (integer); the human report
prints it in the summary line. Track it over time as a single health number —
it does not gate the exit code unless you pass `--min-score`.

### Drift repair (`--fix`)

`verify --fix` mechanically regenerates *only the file the drift lives in*
(never wholesale regen — that's `skillpack init`). Repairs:

- `plugin.json` drift — `version`, `homepage`/`repository` URL, missing
  `description` or `author` — by rewriting `.claude-plugin/plugin.json` from
  the current manifest + intent, leaving your `SKILL.md`/`marketplace.json`
  intact.
- `SKILL.md` frontmatter drift — `name`, `when_to_use`, `allowed-tools`, and
  the `name_drift` checks (Claude + Codex) — by regenerating ONLY the `---`
  frontmatter block from the current intent and splicing it onto your
  committed body, so hand-tailored body prose survives byte-for-byte.

No-op when there's no fixable drift.

## Flags (full reference)

| Flag                    | Purpose                                                                          |
|-------------------------|----------------------------------------------------------------------------------|
| `init --non-interactive` | skip prompts (for CI). Uses a committed `skillpack.toml` when present; otherwise bootstraps from the flags below — the FIRST init can run on a fresh checkout with no TTY |
| `init --auto` | zero flags, zero prompts: derives the intent entirely from the repo (description from the README hint, author from manifest/git config, license from LICENSE or `--license`, invocation from the detected CLI; `--import` required for libraries, `--trigger` optional). Implies non-interactive — on a critical verify failure it refuses to write (exit 2) rather than prompt |
| `init --description <TEXT>` | one-sentence task description (bootstrap) |
| `init --trigger <PHRASE>` | when_to_use trigger phrase; repeatable, and comma/semicolon-separated values split (bootstrap) |
| `init --author <NAME>` | author name for plugin.json; optional (bootstrap) |
| `init --invocation <CMD>` | exact CLI command for CLI projects — pass exactly one of `--invocation`/`--import` (bootstrap) |
| `init --import <PATTERN>` | import pattern for library projects — pass exactly one of `--invocation`/`--import` (bootstrap) |
| `init --accept-warnings` | write files even when `verify` flags warnings (critical still blocks). Without it, warnings prompt before writing in interactive mode |
| `init --license <SPDX>` | override the license for this run                              |
| `init --target <ecosystem>` | agent ecosystem(s) to generate for: `claude` (default), `cursor`, `codex`, `opencode`, `copilot`, `agentsmd`, `claude-md`, `gemini`, `windsurf`, `aider`, `cline`, `roo`, `kilo`, `goose`, `qoder`, `continue`, `augment`, `amazonq`, `freebuff`, or `all` (all 18). Repeatable; the special value `list` prints the canonical names. |
| `init --dry-run` | render + verify + preview without writing any files (or `skillpack.toml`); exits 0 |
| `init --format human\|json` | human summary (default) or a machine-readable JSON object (`written`/`skipped`/`would_write`) for CI |
| `init --force` | overwrite an existing `AGENTS.md` at repo root (skip+warn otherwise). Has no effect on other targets, which write to skillpack-owned paths. |
| `init --template-dir <DIR>` | overlay custom `.tera` templates from a dir; missing files fall back to embedded defaults |
| `update` | incrementally refresh distribution files from an existing `skillpack.toml` — no interview, no verify gate. Writes only changed files; preserves body prose by splicing fresh frontmatter. |
| `update --target <ecosystem>` | same target syntax as `init --target`. Default: every target whose files are already present in the repo (falls back to `all` when none are found). Pass `all` or specific names to override. |
| `update --format human\|json` | human summary (default) or a machine-readable JSON object |
| `update --force` | overwrite an existing `AGENTS.md` (same collision guard as `init --force`). |
| `update --template-dir <DIR>` | same template override semantics as `init --template-dir` |
| `diff` | check whether distribution files are stale; exit 1 if any differs, 0 if all clean (CI gate). Same body-preservation semantics as `update`. |
| `diff --target <ecosystem>` | same target syntax as `update --target`. Default: every target whose files are already present in the repo (falls back to `all` when none are found). |
| `diff --format human\|json` | human summary (default) or a machine-readable JSON object (`clean`/`drifted`/`missing` counts) |
| `diff --force` | check `AGENTS.md` too (same collision guard as `update --force`). |
| `diff --template-dir <DIR>` | same override semantics — use when checking a pack generated with custom templates (avoids spurious drift) |
| `add <name>` | append a new skill to an existing `skillpack.toml` pack (then regenerate). Same bootstrap flags as `init --non-interactive` (`--description`/`--trigger`/`--author`/`--invocation`/`--import`/`--license`); interactive interview by default. |
| `add --format human\|json` | human summary (default) or a machine-readable JSON object |
| `remove <name>` | drop a skill from the pack: edits `skillpack.toml`, deletes the orphaned per-skill files, and regenerates the remaining targets (the symmetric counterpart to `add`). |
| `remove --target <ecosystem>` | same target syntax as `update --target`; which ecosystems to regenerate after removal |
| `remove --format human\|json` | human summary (default) or a machine-readable JSON object |
| `config` | print a summary of the committed `skillpack.toml` (skills + defaults) |
| `config --validate` | validate `skillpack.toml` against the structural invariants; exit non-zero when invalid |
| `verify --format human\|json\|sarif\|github\|junit` | human report (default), machine-readable JSON for CI, SARIF 2.1.0 for GitHub Code Scanning upload-sarif, GitHub Actions `::error`/`::warning` annotations for inline PR-diff comments, or JUnit XML for xUnit-consuming CI (GitLab/Jenkins/CircleCI) |
| `verify --fix` | mechanically repair detected drift (rewrites only the file the drift lives in; surgical). No-op when nothing is fixable. |
| `verify --min-score <N>` | minimum discoverability score (0–100) the run must reach to exit zero; gate runs against the post-fix report. Omitted by default. Pairs with `--format json` for CI. |
| `verify --watch` | re-run verify on every file change (debounced); iterative feedback during SKILL.md / skillpack.toml edits. Only valid with `--format human` (Ctrl-C stops). |
| `verify --template-dir <DIR>` | use custom templates when `--fix` re-renders drifted files; pass the same dir used at `init` to avoid a drift loop |
| `doctor` | read-only diagnosis: print detected language, CLI, diag trace, and verify-category preview (exit 0) |
| `doctor --format human\|json` | read-only diagnosis as serialized `ProjectProfile` for CI (default: human); the JSON form adds a `verify_category_preview` field listing the target ecosystems and check categories |
| `--root <DIR>`           | project root to operate on (default: current dir); available on `init`, `verify`, `doctor`, `update`, `diff`      |
| `--verbose`             | print what `skillpack` detected in the repo (introspection)      |
| `--debug`             | print every subprocess call (alias for `--log-level debug`)       |
| `--log-level <LEVEL>` | structured-diagnostic verbosity: `off`/`error`/`warn`/`info`/`debug`/`trace` (default `warn`) |
| `--log-format <FMT>`  | structured-diagnostic shape: `human` (default) or `json` (one JSON object per event on stderr) |

Notes:

- `update` preserves body prose, so it can't add new `### Subcommands` entries
  or refresh CLI-surface flags — use `init --target all` when the CLI surface
  changed.
- `--fix` requires a committed `skillpack.toml` (it recovers the intent from
  it); a hand-written pack with no config should run `skillpack init` instead.

## Multi-skill packs

A marketplace repo can bundle several skills — Claude Code loads every
`skills/<name>/SKILL.md`, not just one. To grow a pack beyond the single skill
`init` scaffolds:

1. Add a `[[skills]]` entry to `skillpack.toml` (the existing `[skill]` table
   stays the **primary** skill — pack-level files like `plugin.json` render
   from it; `[[skills]]` entries append after it):

   ```toml
   [[skills]]
   name = "sidekick"
   one_line_description = "Handle auxiliary chores"
   when_to_use_phrases = ["aux task", "side errand"]
   invocation_command = "mytool side"   # or import_pattern = "..." for a library
   ```

2. Run `skillpack update --target all` — every skill renders its own
   per-skill file (`skills/<name>/SKILL.md`, `.codex/skills/<name>/SKILL.md`,
   `.cursor/rules/<name>.mdc`, `.opencode/agents/<name>.md`) under its own
   directory name, with its own frontmatter. The config normalizes to a
   `[[skills]]` array on first update.

3. `verify` checks every skill independently — the `name_drift` check accepts
   any configured skill name (not just the canonical project name), and
   `verify --fix` splices the right skill's frontmatter without touching the
   other skills' bodies.

The easier path is `skillpack add <name>` — it appends a skill (via the
interview, or the `--non-interactive` bootstrap flags) and re-renders in one
step, instead of hand-authoring the `[[skills]]` array.

A multi-skill pack's marketplace entry merges every skill's `keywords`
(deduped) so it is discoverable under all of them, not just the primary's.

Polyglot monorepos: `introspect` detects every language manifest (Rust, Node,
Python, Go, Ruby, PHP, JVM, C#, Zig, Swift, C/C++, Elixir, Deno, Nix,
Dart/Flutter, Haskell, Lua, Julia, Crystal, Clojure, OCaml, Erlang, R, Perl)
and records all but the primary as
`secondary_languages`. `init --auto` on a monorepo with no committed config
emits one skill per detected language — the primary keeps the project name,
each secondary becomes `{name}-{lang}` with a library-style intent you can
refine via `skillpack update` / `skillpack add`.

Limitation: the invocation drift checks (`invocation.*`, which spawn the CLI
`--help` and diff flags) run against the FIRST skill file only — give each
skill its own CLI but know that the flag-drift gate checks one of them.

## Config overrides

The language-derived fields in generated files can be pinned per skill in
`skillpack.toml` (each optional; the language hint is the fallback):

```toml
[skill]
name = "mytool"
one_line_description = "..."
when_to_use_phrases = ["..."]
allowed_tools = "Read, Bash(npm test:*)"   # override the allowed-tools frontmatter
category = "the data tooling"              # override the category prose
globs = ["src/**", "*.md"]                 # override Cursor/Windsurf auto-attach globs
opencode_mode = "subagent"                 # override the OpenCode mode
keywords = ["journal", "log"]              # override the marketplace keywords
marketplace_category = "database"          # override the marketplace `category` field
owner_type = "organization"                # override the marketplace `owner.type` field
```

Editor autocomplete/validation for `skillpack.toml` is available via the JSON
Schema at `skillpack.schema.json` (point your TOML editor's `#:schema` at it,
or run `skillpack config --validate` to check the file from the CLI).

## Platform notes

- Cross-platform: CI runs on Ubuntu, macOS, and Windows.
- CLI detection probes `PATH` with `PATHEXT` enumeration on Windows (so a bare
  `node` lookup resolves `node.exe`), and `cargo build` artifacts carry the
  `.exe` suffix.
- Paths are normalized to forward slashes in the verify report; UTF-8 BOMs and
  CRLF line endings are handled at every read boundary (a Windows-edited
  SKILL.md won't false-fail).
- Workspace roots: a Cargo `[workspace]`-only or npm `workspaces` root is
  walked to find the member that ships the CLI; `skillpack doctor` explains
  the decision when detection comes up empty.
