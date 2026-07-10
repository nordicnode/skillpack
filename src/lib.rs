//! skillpack library surface.
//!
//! The binary target (`main.rs`) implements the interactive CLI; this lib
//! target re-exports the pure modules so integration tests and property tests
//! in `tests/` can exercise the parsers (`extract_flags`,
//! `parse_skill_frontmatter`, `coerce_kebab`, the discovery checks) directly
//! without going through the compiled binary.

pub mod cli;
pub mod config;
pub mod exit;
pub mod generate;
pub mod interview;
pub mod introspect;
pub mod spawn;
pub mod types;
pub mod verify;
