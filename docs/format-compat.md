# Format compatibility matrix

What `skillpack init/update` emits for each target, which format family it
belongs to, and what a format change would break. Use this when adding a target
(which format family does it share?) or when a downstream agent spec changes
(which targets are coupled to it?).

## Format families

Every target renders through one of five format modules in
`src/generate/targets/`. Two targets sharing a family emit the *same file
shape* — a format change touches every member of the family at once.

| Family | Module | File shape | Consumed by |
|---|---|---|---|
| Claude/Codex skill | `claude.rs` | `SKILL.md` + `.claude-plugin/` + `.codex/skills/` + `marketplace.json` | Claude Code, Codex (OpenAI) |
| Cursor rule | `cursor.rs` | `.cursor/rules/<name>.mdc` (YAML frontmatter + body) | Cursor |
| OpenCode agent | `opencode.rs` | `.opencode/agent/<name>.md` (frontmatter + body) | OpenCode |
| Windsurf rule | `windsurf.rs` | `.windsurf/rules/<name>.md` | Windsurf |
| Rules directory | `rules_markdown.rs` | `<ecosystem>/rules/<name>.md` | Cline, Roo, Kilo, Qoder, Continue, Augment, Amazon Q, Trae |
| Root instructions | `instructions_markdown.rs` | `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `CONVENTIONS.md`, `copilot-instructions.md`, `goose/AGENTS.md` | Zed/Cody/Kimi/Devin/Junie (AGENTS.md), Gemini CLI, Aider, Copilot, Goose |

## Per-target matrix

| `--target` | Emitted files | Family | Notes / compatibility |
|---|---|---|---|
| `claude` | `SKILL.md`, `.claude-plugin/plugin.json`, `marketplace.json` (`.claude-plugin/marketplaces/`) | Claude/Codex skill | Claude Code ≥ 1.0 plugin + skills; `SKILL.md` frontmatter must stay within the 1,536-char listing cap and 64-char name (see `src/verify/schema.rs`) |
| `codex` | `.codex/skills/<name>/SKILL.md` + `scripts/` | Claude/Codex skill | Codex's SKILL.md + frontmatter format is the same shape as Claude Code's; verified by `discovery.codex.*` |
| `cursor` | `.cursor/rules/<name>.mdc` | Cursor rule | Cursor rule YAML frontmatter (`description`, `globs`, `alwaysApply`); globs are path-scoped to the skill's language |
| `opencode` | `.opencode/agent/<name>.md` | OpenCode agent | `mode: subagent`; frontmatter + body |
| `windsurf` | `.windsurf/rules/<name>.md` | Windsurf rule | Same frontmatter shape as Cursor |
| `cline` / `roo` / `kilo` / `qoder` | `<eco>/rules/<name>.md` | Rules directory | VS Code-extension agents reading markdown rules from their rules dir |
| `continue` | `.continue/rules/<name>.md` | Rules directory | |
| `augment` | `.augment/rules/<name>.md` | Rules directory | |
| `amazonq` | `.amazonq/rules/<name>.md` | Rules directory | Amazon Q Developer |
| `trae` | `.trae/rules/<name>.md` | Rules directory | Trae IDE |
| `agentsmd` | `AGENTS.md` (root) | Root instructions | The de-facto cross-agent standard; consumed by Zed, Cody (Sourcegraph), Kimi, Devin, Junie, and others that read `AGENTS.md` at the repo root |
| `claude-md` | `CLAUDE.md` (root) | Root instructions | Claude Code's root-instructions file |
| `gemini` | `GEMINI.md` (root) | Root instructions | Gemini CLI |
| `aider` | `CONVENTIONS.md` (root) | Root instructions | Aider |
| `copilot` | `copilot-instructions.md` (root) | Root instructions | GitHub Copilot |
| `goose` | `goose/AGENTS.md` | Root instructions | Block/goose agent |
| `freebuff` | `AGENTS.md` | alias | Alias for `agentsmd`; `agents.md`/`agents-md` are accepted spellings too |

## Version pinning

- **`skillpack` release ↔ emitted files**: the generated files are only
  versioned by the skillpack release that produced them. `verify` re-checks
  them against the *current* binary's schema, so `skillpack update` after an
  upgrade brings drifted files back in sync.
- **The version lives in five places that must move together**: `Cargo.toml`
  `[package].version`, the reusable workflow's `skillpack-version` default
  (`.github/workflows/skillpack.yml`), the committed `.claude-plugin/plugin.json`,
  the Homebrew formula's `version` + URLs (`homebrew/skillpack.rb`), and the
  README pin examples. The release-plz workflow's "Sync version files" step
  bumps all of them on the release branch (see `.github/workflows/release-plz.yml`);
  the CI `version-sync` job fails if they drift.
- **`verify-report.schema.json`**: the JSON report emitted by `verify` carries
  a `$schema` pointer and `REPORT_SCHEMA_VERSION` (currently 1). Bump
  `REPORT_SCHEMA_VERSION` only on breaking changes to the report shape.

## Adding a target

1. Pick the format family that fits the file shape (a markdown-rules directory
   → `rules_markdown.rs`; a root-instructions file → `instructions_markdown.rs`).
2. Add the `Target` enum variant + `ALL_TARGETS` entry in `src/cli.rs`.
3. Add one arm to `format_for()` in `src/generate/targets.rs`.
4. Add the discovery check for the emitted files in `src/verify/discovery.rs`
   (or a per-target module under it).
5. Add the target's file globs to the pre-commit hook (`.pre-commit-hooks.yaml`).
6. Regenerate the repo's own pack (`skillpack update --target all`) so
   self-dogfood `verify` covers the new target, and commit the emitted file.

## Format-change blast radius

| If this format changes | Affected targets |
|---|---|
| `SKILL.md` frontmatter / skill listing rules | `claude`, `codex` |
| Cursor `.mdc` frontmatter | `cursor`, `windsurf` (shared shape) |
| OpenCode agent frontmatter | `opencode` |
| Root-instructions filename conventions | `agentsmd`, `claude-md`, `gemini`, `aider`, `copilot`, `goose` |
| Rules-directory filename conventions | `cline`, `roo`, `kilo`, `qoder`, `continue`, `augment`, `amazonq`, `trae` |
