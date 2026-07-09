//! The interactive interview. Three questions max (design §5.1 step 2),
//! producing an [`Intent`]. Bypassable when `skillpack.toml` already holds the
//! answers.
//!
//! For pure-library projects the third question becomes "what's the import
//! pattern?" instead of "what's the exact command to run?" (design §5.1
//! "Pure-library path").
//!
//! All prompting goes through the [`Prompter`] trait so unit tests can inject a
//! canned-answer stub instead of driving a real TTY.

use anyhow::Result;

use crate::types::{Intent, ProjectProfile};

/// Abstraction over the prompt backend. The real implementation uses
/// `dialoguer`; tests inject a deterministic stub.
pub trait Prompter {
    fn text(&self, prompt: &str, default: &str) -> Result<String>;
}

/// A `dialoguer::Input`-backed prompter. Constructed once per interview.
pub struct DialoguerPrompter;

impl Prompter for DialoguerPrompter {
    fn text(&self, prompt: &str, default: &str) -> Result<String> {
        // dialoguer 0.11 builder methods consume `self`; chain them. `theme()`
        // gives the modern look; `default()` seeds the editable suggestion.
        let v: String = dialoguer::Input::new()
            .with_prompt(prompt)
            .default(default.to_string())
            .interact_text()?;
        Ok(v)
    }
}

/// Run the interview and return an [`Intent`], seeded from the profile's
/// introspection hints so the maintainer isn't typing things we already know.
pub fn run(profile: &ProjectProfile, prompter: &dyn Prompter) -> Result<Intent> {
    let desc_default = profile
        .description_hint
        .clone()
        .unwrap_or_else(|| "No description yet".to_string());
    let q1 = prompter.text(
        "What does your tool do? (one sentence — describe the task, not the tool)",
        &desc_default,
    )?;
    let one_line_description = q1.trim().to_string();

    let q2 = prompter.text(
        "When should an agent use this? (trigger verbs or scenarios, comma-separated)",
        "",
    )?;
    let when_to_use_phrases: Vec<String> = q2
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let author = prompter.text("Author name (for plugin.json)", "")?;
    let author = author.trim();
    let author = if author.is_empty() {
        None
    } else {
        Some(author.to_string())
    };

    let license = prompter.text("License SPDX id (enter for MIT)", "MIT")?;
    let license = license.trim();
    let license = if license.is_empty() || license.eq_ignore_ascii_case("MIT") {
        Some("MIT".to_string())
    } else {
        Some(license.to_string())
    };

    if profile.has_cli {
        // The suggested default is the bare tool *name* — the thing an agent
        // would actually type (e.g. `sample-node`), not the internal spawn
        // argv, whose first element is the runtime (`node`, `go`) or an
        // absolute build path (design §5.1 Q3). `profile.name` is already the
        // best-effort human invocation; the maintainer edits from there.
        let suggest = profile.name.clone();
        let q3 = prompter.text(
            "What's the exact command an agent should run to use it?",
            &suggest,
        )?;
        let invocation_command = if q3.trim().is_empty() {
            Some(suggest)
        } else {
            Some(q3.trim().to_string())
        };
        Ok(Intent {
            one_line_description,
            when_to_use_phrases,
            invocation_command,
            import_pattern: None,
            author,
            license,
        })
    } else {
        let q3 = prompter.text(
            "What's the import pattern an agent should use? (e.g. import { foo } from 'yourpkg')",
            "",
        )?;
        Ok(Intent {
            one_line_description,
            when_to_use_phrases,
            invocation_command: None,
            import_pattern: if q3.trim().is_empty() {
                None
            } else {
                Some(q3.trim().to_string())
            },
            author,
            license,
        })
    }
}

#[cfg(test)]
pub mod stub {
    //! A canned-answer `Prompter` for tests.

    use super::Prompter;
    use anyhow::Result;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Returns the next pre-loaded answer for each `text` call, falling back to
    /// the supplied default once the queue is empty. De-queues FIFO so the
    /// answers read in the same order the interview asks them.
    pub struct StubPrompter {
        answers: RefCell<VecDeque<String>>,
    }

    impl StubPrompter {
        pub fn new(answers: Vec<String>) -> Self {
            Self {
                answers: RefCell::new(VecDeque::from(answers)),
            }
        }
    }

    impl Prompter for StubPrompter {
        fn text(&self, _prompt: &str, default: &str) -> Result<String> {
            let mut q = self.answers.borrow_mut();
            Ok(q.pop_front().unwrap_or_else(|| default.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProjectProfile;

    #[test]
    fn cli_interview_builds_intent_with_invocation() {
        let mut profile = ProjectProfile::test_default();
        profile.has_cli = true;
        // The interview asks: description, when, author, license, invocation.
        let stub = stub::StubPrompter::new(vec![
            "serve my notes".to_string(),
            "write journals, log entries".to_string(),
            String::new(), // author
            String::new(), // license -> MIT
            "chronicle --new 'entry'".to_string(),
        ]);
        let intent = run(&profile, &stub).unwrap();
        assert_eq!(intent.one_line_description, "serve my notes");
        assert_eq!(
            intent.when_to_use_phrases,
            vec!["write journals".to_string(), "log entries".to_string()]
        );
        assert_eq!(
            intent.invocation_command.as_deref(),
            Some("chronicle --new 'entry'")
        );
        assert!(intent.import_pattern.is_none());
        assert_eq!(intent.license.as_deref(), Some("MIT"));
    }

    #[test]
    fn pure_library_interview_builds_intent_with_import() {
        let mut profile = ProjectProfile::test_default();
        profile.has_cli = false;
        let stub = stub::StubPrompter::new(vec![
            "parse CSVs".to_string(),
            "ingest, convert".to_string(),
            "Jane".to_string(),
            "Apache-2.0".to_string(),
            "import { parse } from 'fastcsv'".to_string(),
        ]);
        let intent = run(&profile, &stub).unwrap();
        assert!(intent.invocation_command.is_none());
        assert_eq!(
            intent.import_pattern.as_deref(),
            Some("import { parse } from 'fastcsv'")
        );
        assert_eq!(intent.author.as_deref(), Some("Jane"));
        assert_eq!(intent.license.as_deref(), Some("Apache-2.0"));
    }
}
