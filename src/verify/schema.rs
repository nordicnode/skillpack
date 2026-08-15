//! The Claude Code schema constants that `verify` enforces.
//!
//! Every literal here is grounded in the public docs (code.claude.com/docs/en/
//! plugins-reference, plugin-marketplaces, skills) as checked against the live
//! spec. Keep a comment citing the source for each rule so future-me can
//! re-verify against an updated spec without guessing where a number came from.

/// The combined `description` + `when_to_use` text in the SKILL.md skill
/// listing is capped at 1,536 characters.
///
/// Source: code.claude.com/docs/en/skills — "the combined `description` and
/// `when_to_use` text is truncated at 1,536 characters in the skill listing."
pub const SKILL_LISTING_CHAR_CAP: usize = 1_536;

/// A skill `name` is capped at 64 characters.
///
/// Source: skill-authoring docs — "name: Maximum 64 characters."
pub const SKILL_NAME_MAX_CHARS: usize = 64;

// `when_to_use` advertises trigger phrases; we flag if it's present but empty
// or only whitespace, since that defeats the whole point of the field.
// (We do not enforce a hard length — the listing cap covers the upper bound.)

/// Plugin / marketplace `name` must be kebab-case and contain no spaces.
/// Source: plugin-marketplaces — "kebab-case, no spaces." We use a permissive
/// regex that matches the AgentSkills.io open standard `^[a-z][a-z0-9-]*[a-z0-9]$`,
/// which allows no consecutive hyphens and a trailing alnum.
pub const NAME_KEBAB_REGEX: &str = r"^[a-z][a-z0-9-]*[a-z0-9]$";

// A name that is a single lowercase letter is a degenerate corner not covered
// by the regex above (which requires ≥2 chars); we accept it explicitly below
// by treating length-1 names as valid iff they're `[a-z]`.

// The relative-path form of a marketplace plugin `source` MUST start with
// `./`. We only flag structural problems; absolute and `../` are flagged.
// Source: plugin-marketplaces — "Relative path ... Must start with `./`.
// Resolved relative to the marketplace root."

/// Names we refuse outright. The design doc listed a candidate set; the live
/// docs did not confirm Anthropic publishes an authoritative reserved list, so
/// these are treated as WARNINGS (not hard failures), per the honest-verifier
/// posture in design §13 Mitigation ("verify is deliberately conservative").
/// A maintainer can ignore a warning; they cannot ignore the reputation cost of
/// clobbering an Anthropic-owned name.
pub const RESERVED_NAMES: &[&str] = &[
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "anthropic",
    "claude",
    // `agent-skills` is the AgentSkills.io open-standard namespace; flag it so
    // a plugin doesn't squat the standard's name. Generic words like
    // `skills`/`official` are deliberately NOT listed (too common to warn on).
    "agent-skills",
];

/// `plugin.json` MUST live at `.claude-plugin/plugin.json`. Source:
/// anthropics/claude-code manifest-reference.md — "Required path:
/// `.claude-plugin/plugin.json`."
pub const PLUGIN_JSON_PATH: &str = ".claude-plugin/plugin.json";
/// The `.claude-plugin/` directory houses the marketplace + plugin manifests.
/// Source: anthropics/claude-code plugin-reference.md. Verified July 2026.
pub const CLAUDE_PLUGIN_DIR: &str = ".claude-plugin";

/// `marketplace.json` lives at `.claude-plugin/marketplace.json`. Source:
/// plugin-marketplaces — "Create `.claude-plugin/marketplace.json`."
pub const MARKETPLACE_JSON_PATH: &str = ".claude-plugin/marketplace.json";
/// Cursor project rules live under `.cursor/rules/<name>.mdc`. Source:
/// cursor.com/docs/rules — "Project rules are stored as `.mdc` files in
/// `.cursor/rules/`." Verified July 2026.
pub const CURSOR_RULES_DIR: &str = ".cursor/rules";
/// Codex CLI skills live under `.codex/skills/<name>/SKILL.md`. Source:
/// the AgentSkills open standard (agskills.dev) and the Codex CLI skill
/// convention — same `SKILL.md` frontmatter shape as Claude, installed under
/// `.codex/skills/`. Verified July 2026.
pub const CODEX_SKILLS_DIR: &str = ".codex/skills";
/// Native Claude Code skills directory `.claude/skills/<name>/SKILL.md`.
/// Source: code.claude.com/docs/en/skills — Claude Code scans `.claude/skills/`
/// and auto-loads each skill with no plugin-install step. Same `SKILL.md`
/// frontmatter shape as the plugin `skills/<name>/SKILL.md` path, different
/// directory (native vs plugin-installed). Verified August 2026.
pub const CLAUDE_SKILLS_DIR: &str = ".claude/skills";
/// OpenCode agent definitions live under `.opencode/agents/<name>.md`. Source:
/// opencode.ai/docs/agents — "Place them in: Per-project:
/// `.opencode/agents/`". The markdown file name becomes the agent name.
/// Frontmatter: `description` (required), `mode`/`temperature`/`permissions`
/// (optional). Verified July 2026.
pub const OPENCODE_AGENTS_DIR: &str = ".opencode/agents";

/// GitHub Copilot custom instructions live at
/// `.github/copilot-instructions.md`. Source:
/// docs.github.com/copilot/how-tos/copilot-on-github/customize-copilot/
/// add-repository-instructions — "The path is always
/// `.github/copilot-instructions.md`." Plain markdown, no frontmatter.
/// Verified July 2026.
pub const COPILOT_INSTRUCTIONS_PATH: &str = ".github/copilot-instructions.md";

/// AGENTS.md lives at the repository root. Source: agents.md (Linux Foundation
/// stewarded, aaif.io/projects/agents-md) — "AGENTS.md is just standard Markdown.
/// Use any headings you like; the agent simply parses the text you provide."
/// Plain markdown, no frontmatter, no required fields. Read natively by 20+
/// coding agents (Codex, Cursor, Windsurf, Copilot, Aider, Zed, Warp, JetBrains
/// Junie, Freebuff, etc.). Verified July 2026.
pub const AGENTS_MD_PATH: &str = "AGENTS.md";

/// `CLAUDE.md` lives at the repository root. Source: the Claude Code
/// ecosystem convention — Claude Code, Cline, Roo Code, and their forks read
/// a root `CLAUDE.md` for project instructions. Plain markdown, no
/// frontmatter (same structural contract as AGENTS.md). Verified July 2026.
pub const CLAUDE_MD_PATH: &str = "CLAUDE.md";

/// `GEMINI.md` lives at the repository root. Source:
/// google-gemini.github.io/gemini-cli/docs/cli/gemini-md.html — "You can use
/// these files to give project-specific instructions ... to make the AI's
/// responses more accurate." Plain markdown, no frontmatter. Verified July 2026.
pub const GEMINI_MD_PATH: &str = "GEMINI.md";

/// Windsurf (Cascade) project rules live under `.windsurf/rules/<name>.md`.
/// Source: the Windsurf docs and the cursor↔windsurf converter ecosystem —
/// the same `description`/`globs`/`alwaysApply` frontmatter shape as Cursor
/// rules, `.md` files under `.windsurf/rules/`. Verified July 2026.
pub const WINDSURF_RULES_DIR: &str = ".windsurf/rules";

/// Aider reads a root-level `CONVENTIONS.md` for repo conventions. Plain
/// markdown, no frontmatter. Verified July 2026 against the aider docs.
pub const CONVENTIONS_MD_PATH: &str = "CONVENTIONS.md";

/// Cline workspace rules live in `.clinerules/` (`.md`/`.txt` files, optional
/// `paths:` YAML frontmatter for conditional rules). Source:
/// docs.cline.bot/customization/cline-rules — "Workspace rules go in
/// `.clinerules/` at your project root." Verified August 2026.
pub const CLINE_RULES_DIR: &str = ".clinerules";

/// Roo Code workspace rules live in `.roo/rules/` (markdown; mode-specific
/// rules live in `.roo/rules-{modeSlug}/`). Source: docs.roocode.com custom
/// modes / rules — "Instructions can also live in `.roo/rules/`
/// (workspace-wide)." Verified August 2026.
pub const ROO_RULES_DIR: &str = ".roo/rules";

/// Kilo Code project rules. The current format references `.kilo/rules/*.md`
/// via `kilo.jsonc`'s `instructions` array, but `.kilocode/rules/` is
/// auto-included for backward compatibility (kilo.ai/docs/customize/custom-rules
/// — "The extension is backward compatible with `.kilocode/rules/` directories").
/// Emitting into the backward-compatible directory loads with zero config edit.
/// Verified August 2026.
pub const KILOCODE_RULES_DIR: &str = ".kilocode/rules";

/// Qoder workspace rules live in `.qoder/rules/` (markdown). Source:
/// docs.qoder.com/user-guide/rules — "By creating configuration files in the
/// .qoder/rules directory, you can instruct Qoder on your project's specific
/// conventions." Verified August 2026.
pub const QODER_RULES_DIR: &str = ".qoder/rules";

/// Continue.dev workspace rules live in `.continue/rules/` (markdown). Source:
/// docs.continue.dev/customize/rules — "Create files in .continue/rules."
/// Verified August 2026.
pub const CONTINUE_RULES_DIR: &str = ".continue/rules";

/// Augment Code rules live in `.augment/rules/` (markdown instruction files).
/// Source: docs.augmentcode.com "Introducing Augment Rules" — "add instruction
/// files to .augment/rules/". Verified August 2026.
pub const AUGMENT_RULES_DIR: &str = ".augment/rules";

/// Amazon Q Developer project rules live in `.amazonq/rules/` (markdown).
/// Source: AWS Q Developer docs — "Rules must be written in Markdown format
/// (.md files); They should be placed in the .amazonq/rules directory."
/// Verified August 2026.
pub const AMAZONQ_RULES_DIR: &str = ".amazonq/rules";

/// Trae IDE workspace rules live in `.trae/rules/` (markdown). Source:
/// docs.trae.ai/ide/rules — "In IDE mode, add the rule's content using
/// Markdown syntax ... in the .trae/rules directory." Verified August 2026.
pub const TRAE_RULES_DIR: &str = ".trae/rules";

/// Goose (Block's open-source agent) reads a project-wide
/// `.goose/instructions.md`. Plain markdown, no frontmatter. Verified August
/// 2026 against the Goose docs + the ctxlint context-file registry.
pub const GOOSE_INSTRUCTIONS_PATH: &str = ".goose/instructions.md";

// Action-verb heuristic: the first word of a good skill description is an
// action verb (e.g. "Lint", "Generate", "Format"). We don't enforce grammar —
// we only flag descriptions that don't begin with an alphabetic word, a
// strong signal the description was written as a name/title. Source: skill
// best-practices — open with "one sentence describing what the skill does";
// the listing places the key use case first.
