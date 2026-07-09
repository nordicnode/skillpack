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
    "agent-skills",
    "skills",
    "official",
];

/// `plugin.json` MUST live at `.claude-plugin/plugin.json`. Source:
/// anthropics/claude-code manifest-reference.md — "Required path:
/// `.claude-plugin/plugin.json`."
pub const PLUGIN_JSON_PATH: &str = ".claude-plugin/plugin.json";

/// `marketplace.json` lives at `.claude-plugin/marketplace.json`. Source:
/// plugin-marketplaces — "Create `.claude-plugin/marketplace.json`."
pub const MARKETPLACE_JSON_PATH: &str = ".claude-plugin/marketplace.json";

// Action-verb heuristic: the first word of a good skill description is an
// action verb (e.g. "Lint", "Generate", "Format"). We don't enforce grammar —
// we only flag descriptions that don't begin with an alphabetic word, a
// strong signal the description was written as a name/title. Source: skill
// best-practices — open with "one sentence describing what the skill does";
// the listing places the key use case first.
