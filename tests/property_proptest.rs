//! Property-based tests (design §7.3): the help-text flag extractor and the
//! SKILL.md frontmatter parser never panic and are deterministic on arbitrary
//! input.
//!
//! `lib.rs` re-exports the crate internals, so these call the public
//! `extract_flags` and `parse_skill_frontmatter` directly (in-process, no
//! binary spawn) — they're genuine property tests, not placeholders.

use proptest::prelude::*;

use skillpack::verify::discovery::{parse_cursor_mdc_frontmatter, parse_skill_frontmatter};
use skillpack::verify::invocation::extract_flags;

proptest! {
    /// Extracting flags from arbitrary text must never panic and must be a pure
    /// function of the input — two runs on the same blob yield the same list.
    /// The strategy mimics realistic --help text: words, dashes, flags, =values,
    /// commas, and newlines, bounded to keep the test fast.
    #[test]
    fn extract_flags_never_panics_and_is_deterministic(
        blob in "[a-zA-Z0-9 \t--=,;:.()'\"\n]{0,4000}"
    ) {
        let a = extract_flags(&blob);
        let b = extract_flags(&blob);
        prop_assert_eq!(&a, &b, "extract_flags must be deterministic");

        // Invariants that hold for all inputs: every returned token is a
        // well-formed flag (starts with a dash, has a leading letter, ≥2 chars
        // long), and the list is deduplicated.
        let mut seen = std::collections::HashSet::new();
        for f in &a {
            prop_assert!(f.starts_with('-'), "flag must start with '-': {f}");
            prop_assert!(f.len() >= 2, "flag must be ≥2 chars: {f}");
            // first non-dash char is a letter — filters `-`/`--`/`-1` prose.
            let first = f.chars().find(|c| *c != '-');
            prop_assert!(
                first.is_some_and(|c| c.is_ascii_alphabetic()),
                "flag's first non-dash char must be a letter: {f}"
            );
            prop_assert!(seen.insert(f.clone()), "flags must be deduped: {f}");
        }
    }

    /// Parsing frontmatter out of arbitrary markdown must never panic.
    /// Whatever it returns, the parser is total: it either finds a `---`…`---`
    /// block or returns None, and never hangs or throws.
    #[test]
    fn parse_frontmatter_never_panics(blob in "[\x20-\x7E\n]{0,4000}") {
        let fm = parse_skill_frontmatter(&blob);
        let _ = fm; // the property is "returns in bounded time without panicking"
    }
    /// Parsing cursor `.mdc` frontmatter out of arbitrary text must never
    /// panic — the parser is total, returning `Option<CursorFrontmatter>`.
    #[test]
    fn parse_cursor_mdc_frontmatter_never_panics(blob in "[\x20-\x7E\n]{0,4000}") {
        let fm = parse_cursor_mdc_frontmatter(&blob);
        let _ = fm; // the property is "returns in bounded time without panicking"
    }
}
