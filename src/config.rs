//! `skillpack.toml` — the committed config that makes re-runs deterministic.
//!
//! Per design §4.3, this file lives at the OSS project root, is committed to
//! git, and stores the interview answers plus user prefs so `init` can run
//! non-interactively in CI. `Config` round-trips losslessly through TOML.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::types::Intent;

/// The fixed filename committed at the project root.
pub const FILENAME: &str = "skillpack.toml";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Config {
    /// The persisted interview answers. When present, `init` skips the
    /// interactive prompts entirely (design §5.1 step 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillConfig>,
    /// Multi-skill packs: one `[[skills]]` entry per skill, each with its own
    /// `name` (skill directory + frontmatter) and description/triggers/
    /// invocation. Mutually exclusive with `[skill]` in practice — a config
    /// uses either the single-skill form (written by `init`; backward
    /// compatible) or the array form (author multi-skill packs by hand-editing
    /// this array, then run `update` to render all skills). When both are
    /// present, `to_intents()` treats `[skill]` as the primary and appends the
    /// array entries after it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillConfig>,
    /// Persistent user prefs, independent of any single skill. Filled in
    /// once and reused across re-runs.
    #[serde(default)]
    pub defaults: Defaults,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SkillConfig {
    /// Kebab-case plugin name.
    pub name: String,
    /// One-sentence task description.
    pub one_line_description: String,
    /// Trigger phrases for `when_to_use`.
    pub when_to_use_phrases: Vec<String>,
    /// Exact CLI invocation. `None` for pure libraries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_command: Option<String>,
    /// Import pattern for pure libraries. `None` for CLI projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_pattern: Option<String>,
    /// SPDX license id (e.g. `MIT`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Stdin bytes to feed the CLI during `verify` spawns (e.g. `--help`,
    /// `--version`). For interactive CLIs that block on stdin — without this,
    /// `verify` times out and false-flags drift. Raw bytes are written then
    /// stdin closes; `None` (default) uses `/dev/null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_stdin: Option<String>,
    /// Optional project-specific footguns or gotchas to document for agents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footguns: Vec<String>,
    /// Override the language-derived `allowed-tools` frontmatter value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    /// Override the language-derived Cursor/Windsurf auto-attach `globs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub globs: Option<Vec<String>>,
    /// Override the language-derived `category` prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Override the language-derived OpenCode `mode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_mode: Option<String>,
    /// Override the derived marketplace `keywords` list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    /// Override the marketplace `category` field (default `developer-tools`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace_category: Option<String>,
    /// Override the marketplace `owner.type` field (default `individual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_type: Option<String>,
}

impl SkillConfig {
    /// Build an [`Intent`] from this skill's persisted fields, filling
    /// author/license from the shared defaults when the skill omits them.
    pub fn to_intent(&self, defaults: &Defaults) -> Intent {
        Intent {
            one_line_description: self.one_line_description.clone(),
            when_to_use_phrases: self.when_to_use_phrases.clone(),
            invocation_command: self.invocation_command.clone(),
            import_pattern: self.import_pattern.clone(),
            author: self.author.clone().or_else(|| defaults.author.clone()),
            license: self.license.clone().or_else(|| defaults.license.clone()),
            verify_stdin: self.verify_stdin.clone(),
            footguns: self.footguns.clone(),
            allowed_tools: self.allowed_tools.clone(),
            globs: self.globs.clone(),
            category: self.category.clone(),
            opencode_mode: self.opencode_mode.clone(),
            keywords: self.keywords.clone(),
            marketplace_category: self.marketplace_category.clone(),
            owner_type: self.owner_type.clone(),
        }
    }

    /// Construct a skill entry from a name + intent.
    pub fn from_intent(name: &str, intent: &Intent) -> Self {
        Self {
            name: name.to_string(),
            one_line_description: intent.one_line_description.clone(),
            when_to_use_phrases: intent.when_to_use_phrases.clone(),
            invocation_command: intent.invocation_command.clone(),
            import_pattern: intent.import_pattern.clone(),
            author: intent.author.clone(),
            license: intent.license.clone(),
            verify_stdin: intent.verify_stdin.clone(),
            footguns: intent.footguns.clone(),
            allowed_tools: intent.allowed_tools.clone(),
            globs: intent.globs.clone(),
            category: intent.category.clone(),
            opencode_mode: intent.opencode_mode.clone(),
            keywords: intent.keywords.clone(),
            marketplace_category: intent.marketplace_category.clone(),
            owner_type: intent.owner_type.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

impl Default for Defaults {
    // `license` deliberately defaults to MIT (not `None`): the generated
    // plugin.json/marketplace.json both fall back to MIT when the intent
    // carries no license, and the interview's license prompt defaults to MIT
    // too — so an absent `[defaults]` table round-trips as the same MIT that
    // every other path would produce. Note this makes "no [defaults] table"
    // indistinguishable from an explicit `license = "MIT"`; that is the
    // intent (MIT is the lowest-surprise default for OSS), just documented
    // so the losslessness claim in the module doc stays honest.
    fn default() -> Self {
        Self {
            author: None,
            license: Some("MIT".to_string()),
        }
    }
}

impl Config {
    /// Read `skillpack.toml` from `root`. Returns `None` if the file does
    /// not exist (the caller decides whether that means "fresh project, run
    /// the interview" — it is not an error here).
    pub fn load(root: &Path) -> Result<Option<Config>> {
        let path = Self::path(root);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
        cfg.validate()
            .with_context(|| format!("invalid {}", path.display()))?;
        Ok(Some(cfg))
    }

    /// Absolute path to the config file under `root`.
    pub fn path(root: &Path) -> PathBuf {
        root.join(FILENAME)
    }

    /// Write the config back to disk, creating parent dirs as needed.
    /// Output is stable: field order matches the struct declaration so
    /// re-saves produce a minimal diff.
    pub fn save(&self, root: &Path) -> Result<PathBuf> {
        let path = Self::path(root);
        let serialized =
            toml::to_string_pretty(self).context("failed to serialize skillpack.toml")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, serialized)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    /// Write the config only when its serialized form differs from what is
    /// currently on disk. Returns `true` when a write happened. Keeps
    /// `update` from churning `skillpack.toml`'s mtime (and rewriting a
    /// user's hand-formatted file) when nothing actually changed.
    pub fn save_if_changed(&self, root: &Path) -> Result<bool> {
        let serialized =
            toml::to_string_pretty(self).context("failed to serialize skillpack.toml")?;
        let current = fs::read_to_string(Self::path(root)).unwrap_or_default();
        if current == serialized {
            return Ok(false);
        }
        self.save(root)?;
        Ok(true)
    }

    /// Validate structural invariants that `verify` cannot catch later.
    /// Called by `load` right after parse. An absent skill block is fine
    /// (fresh project before interview). Each skill's name is validated —
    /// a non-kebab name corrupts every generated artifact at the source
    /// and there is no verify-side warning for it. Description and trigger
    /// phrases are left to `verify`'s soft-checks (load stays lossless;
    /// empty triggers surface as a verify warning, not a load-time error).
    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for s in self.skill.iter().chain(self.skills.iter()) {
            if !crate::verify::discovery::is_valid_kebab(&s.name) {
                bail!(
                    "skill name must be non-empty kebab-case (a-z, 0-9, single \
                     hyphens), got {:?}",
                    s.name
                );
            }
            // A duplicate name would make `render_all` emit the same output
            // path twice and `write_files`/`update` silently overwrite the
            // first copy with the second (last-wins). Reject it at load time
            // so a hand-edited config can't corrupt the pack this way.
            if !seen.insert(s.name.clone()) {
                bail!(
                    "duplicate skill name {:?} in skillpack.toml (every skill \
                     needs a unique name)",
                    s.name
                );
            }
            // The marketplace `owner.type` field is restricted to two values
            // by the JSON Schema, but the renderer would previously emit any
            // string the user typed. Reject invalid values at load time so a
            // hand-edited config can't produce a marketplace entry consumers
            // reject.
            if let Some(ot) = &s.owner_type {
                if ot != "individual" && ot != "organization" {
                    bail!(
                        "skill {:?} has owner_type {:?}; expected \"individual\" \
                         or \"organization\"",
                        s.name,
                        ot
                    );
                }
            }
        }
        Ok(())
    }

    /// Build an [`Intent`] from this config's single `[skill]` block, if
    /// present. Used by `init` to skip the interactive interview on re-runs
    /// and by the interactive path. Prefers the `[[skills]]` array's first
    /// entry when no `[skill]` table exists.
    pub fn to_intent(&self) -> Option<Intent> {
        if let Some(s) = &self.skill {
            return Some(s.to_intent(&self.defaults));
        }
        self.skills.first().map(|s| s.to_intent(&self.defaults))
    }

    /// All skills as `(name, Intent)` pairs. A `[skill]` table is the PRIMARY
    /// skill (pack-level files render from it); `[[skills]]` entries append
    /// after it — so hand-editing "add a `[[skills]]` entry" keeps the
    /// existing skill primary. Empty when the config has no skill block.
    pub fn to_intents(&self) -> Vec<(String, Intent)> {
        let mut out = Vec::new();
        if let Some(s) = &self.skill {
            out.push((s.name.clone(), s.to_intent(&self.defaults)));
        }
        for s in &self.skills {
            out.push((s.name.clone(), s.to_intent(&self.defaults)));
        }
        out
    }

    /// Construct a config from one or more `(name, Intent)` pairs. A single
    /// skill serializes as the `[skill]` form (byte-compatible with what
    /// `init` has always written); multiple skills serialize as `[[skills]]`.
    pub fn from_intents(intents: &[(String, Intent)]) -> Self {
        let skills: Vec<SkillConfig> = intents
            .iter()
            .map(|(name, i)| SkillConfig::from_intent(name, i))
            .collect();
        let defaults = Defaults {
            author: intents.first().and_then(|(_, i)| i.author.clone()),
            license: intents
                .first()
                .and_then(|(_, i)| i.license.clone())
                .or(Some("MIT".to_string())),
        };
        if skills.len() == 1 {
            Self {
                skill: Some(skills.into_iter().next().unwrap()),
                skills: Vec::new(),
                defaults,
            }
        } else {
            Self {
                skill: None,
                skills,
                defaults,
            }
        }
    }

    /// Construct a config from a single [`Intent`] + name, for `init`'s
    /// first-run save. Delegates to [`Config::from_intents`].
    pub fn from_intent(name: &str, intent: &Intent) -> Self {
        Self::from_intents(&[(name.to_string(), intent.clone())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_non_kebab_name() {
        let mut cfg = Config {
            skill: Some(SkillConfig {
                name: "".into(),
                one_line_description: "desc".into(),
                when_to_use_phrases: vec!["test".into()],
                invocation_command: Some("cmd".into()),
                import_pattern: None,
                author: None,
                license: None,
                verify_stdin: None,
                footguns: Vec::new(),
                ..Default::default()
            }),
            skills: Vec::new(),
            defaults: Defaults::default(),
        };
        assert!(cfg.validate().is_err());

        // Non-kebab name (uppercase + spaces)
        cfg.skill.as_mut().unwrap().name = "My Tool".into();
        assert!(cfg.validate().is_err());

        // Double hyphen
        cfg.skill.as_mut().unwrap().name = "foo--bar".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_kebab_name() {
        let cfg = Config {
            skill: Some(SkillConfig {
                name: "sample-rust".into(),
                one_line_description: "desc".into(),
                when_to_use_phrases: vec!["test".into()],
                invocation_command: Some("cmd".into()),
                import_pattern: None,
                author: None,
                license: None,
                verify_stdin: None,
                footguns: Vec::new(),
                ..Default::default()
            }),
            skills: Vec::new(),
            defaults: Defaults::default(),
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_passes_when_skill_absent() {
        let cfg = Config::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_allows_empty_triggers_soft_check() {
        // Empty when_to_use_phrases is a verify warning, not a load-time
        // rejection — load stays lossless (the contract in config.rs:1-5).
        let cfg = Config {
            skill: Some(SkillConfig {
                name: "sample-rust".into(),
                one_line_description: "desc".into(),
                when_to_use_phrases: vec![],
                invocation_command: Some("cmd".into()),
                import_pattern: None,
                author: None,
                license: None,
                verify_stdin: None,
                footguns: Vec::new(),
                ..Default::default()
            }),
            skills: Vec::new(),
            defaults: Defaults::default(),
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_non_kebab_name_in_skills_array() {
        let cfg = Config {
            skill: None,
            skills: vec![SkillConfig {
                name: "My Tool".into(),
                one_line_description: "desc".into(),
                when_to_use_phrases: vec!["test".into()],
                invocation_command: Some("cmd".into()),
                import_pattern: None,
                author: None,
                license: None,
                verify_stdin: None,
                footguns: Vec::new(),
                ..Default::default()
            }],
            defaults: Defaults::default(),
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_skill_names() {
        // Two [[skills]] entries with the same name would make render_all emit
        // the same rel_path twice (last-wins on write). Reject at load time.
        let s = || SkillConfig {
            name: "dupe".into(),
            one_line_description: "d".into(),
            when_to_use_phrases: vec!["x".into()],
            invocation_command: None,
            import_pattern: Some("import d".into()),
            author: None,
            license: None,
            verify_stdin: None,
            footguns: Vec::new(),
            ..Default::default()
        };
        let cfg = Config {
            skill: None,
            skills: vec![s(), s()],
            defaults: Defaults::default(),
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate skill name"), "got: {err}");

        // A [skill] table colliding with a [[skills]] entry is the same defect.
        let mut primary = s();
        primary.name = "primary".into();
        let mut dup = s();
        dup.name = "primary".into();
        let cfg = Config {
            skill: Some(primary),
            skills: vec![dup],
            defaults: Defaults::default(),
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_owner_type() {
        let mut cfg = Config {
            skill: Some(SkillConfig {
                name: "mytool".into(),
                one_line_description: "d".into(),
                when_to_use_phrases: vec!["x".into()],
                invocation_command: None,
                import_pattern: Some("import d".into()),
                author: None,
                license: None,
                verify_stdin: None,
                footguns: Vec::new(),
                ..Default::default()
            }),
            skills: Vec::new(),
            defaults: Defaults::default(),
        };
        cfg.skill.as_mut().unwrap().owner_type = Some("collective".into());
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("owner_type"), "got: {err}");

        for ok in ["individual", "organization"] {
            cfg.skill.as_mut().unwrap().owner_type = Some(ok.into());
            assert!(cfg.validate().is_ok(), "{ok} must be accepted");
        }
    }

    #[test]
    fn to_intents_keeps_skill_table_as_primary_then_appends_array() {
        let cfg = Config {
            skill: Some(SkillConfig {
                name: "primary".into(),
                one_line_description: "p".into(),
                when_to_use_phrases: vec!["x".into()],
                invocation_command: None,
                import_pattern: Some("import p".into()),
                author: None,
                license: None,
                verify_stdin: None,
                footguns: Vec::new(),
                ..Default::default()
            }),
            skills: vec![
                SkillConfig {
                    name: "alpha".into(),
                    one_line_description: "a".into(),
                    when_to_use_phrases: vec!["a-trigger".into()],
                    invocation_command: None,
                    import_pattern: Some("import a".into()),
                    author: None,
                    license: None,
                    verify_stdin: None,
                    footguns: Vec::new(),
                    ..Default::default()
                },
                SkillConfig {
                    name: "beta".into(),
                    one_line_description: "b".into(),
                    when_to_use_phrases: vec!["b-trigger".into()],
                    invocation_command: Some("beta run".into()),
                    import_pattern: None,
                    author: None,
                    license: None,
                    verify_stdin: None,
                    footguns: Vec::new(),
                    ..Default::default()
                },
            ],
            defaults: Defaults::default(),
        };
        let intents = cfg.to_intents();
        assert_eq!(intents.len(), 3);
        assert_eq!(intents[0].0, "primary");
        assert_eq!(intents[1].0, "alpha");
        assert_eq!(intents[2].0, "beta");
        assert_eq!(intents[0].1.import_pattern.as_deref(), Some("import p"));
        assert_eq!(
            intents[1].1.when_to_use_phrases,
            vec!["a-trigger".to_string()]
        );
        assert_eq!(intents[2].1.invocation_command.as_deref(), Some("beta run"));
    }

    #[test]
    fn from_intents_round_trips_multi_skill_config() {
        let intents = vec![
            (
                "alpha".to_string(),
                Intent {
                    one_line_description: "a".into(),
                    when_to_use_phrases: vec!["a-trigger".into()],
                    invocation_command: None,
                    import_pattern: Some("import a".into()),
                    author: Some("Jane".into()),
                    license: Some("MIT".into()),
                    verify_stdin: None,
                    footguns: Vec::new(),
                    ..Default::default()
                },
            ),
            (
                "beta".to_string(),
                Intent {
                    one_line_description: "b".into(),
                    when_to_use_phrases: vec!["b-trigger".into()],
                    invocation_command: Some("beta run".into()),
                    import_pattern: None,
                    author: None,
                    license: None,
                    verify_stdin: None,
                    footguns: Vec::new(),
                    ..Default::default()
                },
            ),
        ];
        let cfg = Config::from_intents(&intents);
        assert!(
            cfg.skill.is_none(),
            "multi-skill must not serialize as [skill]"
        );
        assert_eq!(cfg.skills.len(), 2);
        let back = cfg.to_intents();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].0, "alpha");
        // beta omits author/license; the pack defaults (from the primary) cascade.
        assert_eq!(back[1].1.author.as_deref(), Some("Jane"));
        assert_eq!(back[1].1.license.as_deref(), Some("MIT"));
        // Single-skill still serializes as the legacy [skill] form.
        let single = Config::from_intents(&intents[..1]);
        assert!(single.skill.is_some());
        assert!(single.skills.is_empty());
    }

    #[test]
    fn verify_stdin_round_trips_through_toml() {
        let cfg = Config {
            skill: Some(SkillConfig {
                name: "sample-rust".into(),
                one_line_description: "desc".into(),
                when_to_use_phrases: vec!["trigger".into()],
                invocation_command: Some("cmd".into()),
                import_pattern: None,
                author: None,
                license: None,
                verify_stdin: Some("\n".into()),
                footguns: Vec::new(),
                ..Default::default()
            }),
            skills: Vec::new(),
            defaults: Defaults::default(),
        };
        let intent = cfg.to_intent().expect("intent from config");
        assert_eq!(intent.verify_stdin.as_deref(), Some("\n"));
        let back = Config::from_intent("sample-rust", &intent);
        assert_eq!(back.skill.unwrap().verify_stdin, Some("\n".into()));
    }
}
