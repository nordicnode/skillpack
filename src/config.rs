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
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

impl Default for Defaults {
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

    /// Validate structural invariants that `verify` cannot catch later.
    /// Called by `load` right after parse. Only checks `skill.name` —
    /// a non-kebab name corrupts every generated artifact at the source
    /// and there is no verify-side warning for it. Description and trigger
    /// phrases are left to `verify`'s soft-checks (load stays lossless;
    /// empty triggers surface as a verify warning, not a load-time error).
    /// An absent `[skill]` table is fine (fresh project before interview).
    fn validate(&self) -> Result<()> {
        let s = match &self.skill {
            None => return Ok(()),
            Some(s) => s,
        };
        if !crate::verify::discovery::is_valid_kebab(&s.name) {
            bail!(
                "skill.name must be non-empty kebab-case (a-z, 0-9, single \
                 hyphens), got {:?}",
                s.name
            );
        }
        Ok(())
    }

    /// Build an [`Intent`] from this config, if a skill block is present.
    /// Used by `init` to skip the interactive interview on re-runs.
    pub fn to_intent(&self) -> Option<Intent> {
        let s = self.skill.as_ref()?;
        Some(Intent {
            one_line_description: s.one_line_description.clone(),
            when_to_use_phrases: s.when_to_use_phrases.clone(),
            invocation_command: s.invocation_command.clone(),
            import_pattern: s.import_pattern.clone(),
            author: s.author.clone().or_else(|| self.defaults.author.clone()),
            license: s.license.clone().or_else(|| self.defaults.license.clone()),
        })
    }

    /// Construct a config from an [`Intent`] + name, for the first-run save.
    pub fn from_intent(name: &str, intent: &Intent) -> Self {
        let skill = SkillConfig {
            name: name.to_string(),
            one_line_description: intent.one_line_description.clone(),
            when_to_use_phrases: intent.when_to_use_phrases.clone(),
            invocation_command: intent.invocation_command.clone(),
            import_pattern: intent.import_pattern.clone(),
            author: intent.author.clone(),
            license: intent.license.clone(),
        };
        Self {
            skill: Some(skill),
            defaults: Defaults {
                author: intent.author.clone(),
                license: intent.license.clone().or(Some("MIT".to_string())),
            },
        }
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
            }),
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
            }),
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
            }),
            defaults: Defaults::default(),
        };
        assert!(cfg.validate().is_ok());
    }
}
