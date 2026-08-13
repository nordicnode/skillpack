//! Repo metadata: git origin URL, LICENSE SPDX hint, README description hint.
//! Sibling to [`super`] — pure reads (git origin spawn, no candidate spawn).
//!
//! Extracted from `introspect.rs` (v0.9.3): file-repo probes were ~10% of
//! that file and form a self-contained concern with zero calls into the
//! CLI candidate machinery. The git origin uses an inline [`crate::spawn`]
//! guard (`spawn::run` direct) so this module owns no spawn helper shared
//! with the CLI probe pipeline (`cli_probe.rs`).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::spawn::SpawnOutcome;

/// `git remote get-url origin`, best-effort. Never errors the caller.
pub(crate) fn detect_repo_url(root: &Path) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["remote", "get-url", "origin"]).current_dir(root);
    match crate::spawn::run(&mut cmd, Duration::from_secs(3)) {
        SpawnOutcome::RanClean(out) => Some(out.trim().to_string()),
        _ => None,
    }
}

/// `git config user.name`, best-effort. Fills the plugin.json `author` for
/// `init --auto` and `doctor` when the manifest declares no authors — the
/// maintainer's identity is one `git` spawn away, so a zero-interaction init
/// shouldn't prompt for it.
pub(crate) fn detect_author(root: &Path) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["config", "user.name"]).current_dir(root);
    match crate::spawn::run(&mut cmd, Duration::from_secs(3)) {
        SpawnOutcome::RanClean(out) => {
            let s = out.trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
        _ => None,
    }
}

/// Heuristic: read LICENSE, look for the SPDX id text.
pub(crate) fn detect_license(root: &Path) -> Option<String> {
    for filename in &["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"] {
        let p = root.join(filename);
        if let Ok(raw) = std::fs::read_to_string(&p) {
            let head = raw.split('\n').take(3).collect::<Vec<_>>().join("\n");
            let lower = head.to_lowercase();
            if lower.contains("mit license") || lower.contains("permission is hereby granted") {
                return Some("MIT".to_string());
            }
            if lower.contains("apache license") {
                return Some("Apache-2.0".to_string());
            }
            if lower.contains("bsd 3-clause") || lower.contains("neither the name") {
                return Some("BSD-3-Clause".to_string());
            }
            if lower.contains("gnu general public license") {
                return Some("GPL-3.0".to_string());
            }
        }
    }
    None
}

/// We only read the first slice of the README to bound cost.
const README_HEAD_LINES: usize = 500;

/// True if the line is badge/markup only — markdown images and links, inline
/// HTML, and the `[![alt](img)](url)` wrapper form — with no prose left
/// after stripping. Badge rows (shields.io / CI status / language flags)
/// commonly lead READMEs; they must never leak into the description hint.
fn is_markup_only(line: &str) -> bool {
    static RE: std::sync::OnceLock<[regex::Regex; 3]> = std::sync::OnceLock::new();
    let [img, link, html] = RE.get_or_init(|| {
        [
            // Images first: `![alt](url)` (inner of the badge wrapper).
            regex::Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap(),
            // Then links: `[text](url)` — after the image strip, the outer
            // `[![alt](img)](url)` wrapper collapses to `[](url)` and is
            // consumed here too.
            regex::Regex::new(r"\[[^\]]*\]\([^)]*\)").unwrap(),
            // Inline HTML tags last.
            regex::Regex::new(r"<[^>]*>").unwrap(),
        ]
    });
    let mut s = line.to_string();
    s = img.replace_all(&s, "").into_owned();
    s = link.replace_all(&s, "").into_owned();
    s = html.replace_all(&s, "").into_owned();
    // Nothing letter-like left ⇒ the line carried no prose (URLs, alt text,
    // and tag names are all stripped with their surrounding markup).
    !s.chars().any(|c| c.is_ascii_alphabetic())
}

/// First paragraph(s) of the README, capped for cost. Used only as a *hint*
/// surfaced under `--verbose`; the interview is the source of truth.
pub(crate) fn read_readme_hint(root: &Path) -> Option<String> {
    for filename in &["README.md", "README", "readme.md"] {
        let p = root.join(filename);
        if let Ok(raw) = std::fs::read_to_string(&p) {
            let head: String = raw
                .lines()
                .take(README_HEAD_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            // Find the first non-heading, non-empty prose paragraph. Skip
            //   raw HTML tags (READMEs often lead with `<div`, `<p`, `<a`),
            //   markdown headings + image lines, and badge/markup-only rows
            //   (shields.io links, CI status) so the surfaced hint is prose
            //   a maintainer would actually want in a description.
            let paragraph = head
                .lines()
                .skip_while(|l| {
                    let t = l.trim();
                    t.is_empty()
                        || t.starts_with('#')
                        || t.starts_with('!')
                        || t.starts_with('<')
                        || is_markup_only(t)
                })
                .take_while(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let trimmed = paragraph.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub(crate) fn repo_url_name(repo_url: &Option<String>) -> Option<String> {
    let url = repo_url.as_ref()?;
    let last = url.rsplit('/').next()?.trim_end();
    let stem = last.strip_suffix(".git").unwrap_or(last);
    Some(stem.to_string())
}

#[cfg(test)]
mod tests {
    //! README-hint tests that assert the `skip_while` predicate drops raw
    //! HTML and lands on first prose.

    use super::read_readme_hint;

    #[test]
    fn read_readme_hint_skips_leading_html_div() {
        // Reproduces the skillpack self-dogfood gap (README leading with a
        // `<div align="center"><img ...></div>` logo block): the surfaced
        // `desc_hint` was raw HTML markup, not prose. After the fix the
        // `skip_while` predicate also skips lines starting with `<`, so the
        // hint lands on the first real prose line.
        let dir = std::env::temp_dir().join(format!(
            "skillpack-readme-html-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("README.md"),
            "<div align=\"center\"><img src=\"logo.png\" alt=\"logo\"></div>\n\n\
             # mytool\n\n\
             A sample tool that frobs widgets.\n",
        )
        .unwrap();
        let hint = read_readme_hint(&dir).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            !hint.contains('<'),
            "desc_hint must drop raw HTML, got: {hint:?}"
        );
        assert!(
            hint.contains("frobs widgets"),
            "desc_hint must land on first prose line, got: {hint:?}"
        );
    }

    #[test]
    fn read_readme_hint_skips_badge_rows() {
        // Reproduces the fd README shape: a heading, then a row of shields.io
        // / CI badge links, then the real tagline. The badge row must not
        // leak into the hint (it would otherwise become the --auto
        // description and fail verify's "leads with an action" check).
        let dir = std::env::temp_dir().join(format!(
            "skillpack-readme-badges-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("README.md"),
            "# fd\n\n\
             [![CICD](https://github.com/sharkdp/fd/actions/workflows/CICD.yml/badge.svg)](https://github.com/sharkdp/fd/actions/workflows/CICD.yml) \
             [![Version info](https://img.shields.io/crates/v/fd-find.svg)](https://crates.io/crates/fd-find) \
             [[中文](https://github.com/cha0ran/fd-zh)] [[한국어](https://github.com/spearkkk/fd-kor)]\n\n\
             fd is a simple, fast and user-friendly alternative to find.\n",
        )
        .unwrap();
        let hint = read_readme_hint(&dir).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            hint, "fd is a simple, fast and user-friendly alternative to find.",
            "desc_hint must skip badge rows and land on the tagline, got: {hint:?}"
        );
    }
}
