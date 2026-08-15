pub fn extract_documented_invocation(skill_md: &str) -> Option<String> {
    // (1) Prefer an explicit `## Invocation` section.
    if let Some(block) = heading_block(skill_md, "invocation") {
        return Some(block);
    }

    // (2) Fallback: any fenced ``` block whose text contains a `--flag`.
    let mut in_fence = false;
    let mut block = String::new();
    for line in skill_md.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            if in_fence {
                // closing fence: does this block name a flag?
                if extract_flags(&block).iter().any(|f| !is_meta_flag(f)) {
                    return Some(block.clone());
                }
                block.clear();
            }
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            block.push_str(line);
            block.push('\n');
        }
    }
    None
}

/// Derive a `[<program>, "--help"]` argv from a skill's documented invocation.
/// Used to spawn a *secondary* skill's CLI in a multi-skill pack, where
/// introspection only resolved the primary binary. Returns `None` when no
/// program token can be parsed from the invocation block, so the caller can
/// surface an honest skip instead of guessing.
///
/// Only lines INSIDE a fenced code block are candidates — the surrounding
/// prose ("The exact command an agent should run to use this tool:") must
/// never be read as the program name (a latent bug that surfaced as "CLI
/// `The`" for a secondary skill).
pub fn command_from_documented(skill_md: &str) -> Option<Vec<String>> {
    let block = extract_documented_invocation(skill_md)?;
    let mut in_fence = false;
    for line in block.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence || t.is_empty() {
            continue;
        }
        let prog = t.split_whitespace().next()?.trim();
        // A program token is a bare command name or a path. Skip option flags,
        // angle-bracket placeholders, and markdown noise.
        if prog.is_empty()
            || prog.starts_with('#')
            || prog.starts_with('-')
            || prog.starts_with('<')
            || !prog
                .chars()
                .all(|c| c.is_alphanumeric() || "_-./".contains(c))
        {
            continue;
        }
        return Some(vec![prog.to_string(), "--help".to_string()]);
    }
    None
}

/// Collect the body under a `## <heading>` section up to the next `## ` heading.
/// Stops at any `### ` (deeper) heading too: a subsection under `## Invocation`
/// (the `### Subcommands` block the CLI template emits) owns its own flags and
/// is drift-checked separately — including it here would let per-subcommand
/// flags like `--root` read as top-level drift against `<cli> --help`.
pub(crate) fn heading_block(skill_md: &str, heading: &str) -> Option<String> {
    let want = format!("## {heading}");
    let mut in_block = false;
    let mut out = String::new();
    for line in skill_md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") || trimmed.starts_with("### ") {
            if trimmed.starts_with("## ") && trimmed.eq_ignore_ascii_case(&want) {
                in_block = true;
                continue;
            }
            if in_block {
                break;
            }
            continue;
        }
        if in_block {
            out.push_str(line);
            out.push('\n');
        }
    }
    if in_block && !out.trim().is_empty() {
        Some(out)
    } else {
        None
    }
}

/// Spawn `<cmd[0]> [cmd[1..]]` (e.g. `chronicle --help`) under a hard timeout,
/// push the outcome as a check, and return the captured stdout+stderr on
/// success. Spawn calls are emitted as `tracing::debug!` events by the shared
/// spawn core (design §8.2 --debug / --log-level debug).
pub fn is_meta_flag(flag: &str) -> bool {
    matches!(flag, "--help" | "-h" | "--version" | "-V" | "--help-all")
}

/// Parse the subcommand names advertised in a top-level `--help` body.
///
/// Handles the three common help shapes, in order:
/// 1. clap — `Commands:` / `Subcommands:` section header, indented names.
/// 2. cobra (Go) — `Available Commands:` section header, indented names.
/// 3. argparse (Python) — subcommands listed inline in the usage line as
///    `{cmd1,cmd2}` (no dedicated section).
///
/// clap/cobra's auto-added `help` subcommand is filtered. Lines that aren't
/// indented under the header (`Options:`, `Arguments:`, a blank gap, or the
/// usage line) end a header section.
///
/// Returns `[]` for CLIs with no subcommands — so a non-subcommand `--help`
/// (a flat `Usage: chronicle [OPTIONS] ...`) yields nothing, and the
/// subcommand-aware template/verify paths stay dormant (byte-identical
/// snapshots, no extra checks).
pub fn extract_subcommands(help_output: &str) -> Vec<String> {
    let out = extract_header_subcommands(help_output);
    if out.is_empty() {
        extract_usage_brace_subcommands(help_output)
    } else {
        out
    }
}

/// Section-header subcommand parsing (clap `Commands:`/`Subcommands:` and
/// cobra `Available Commands:`). Reads each following indented line's first
/// token as the name; a blank line or an un-indented line ends the section.
fn extract_header_subcommands(help_output: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in help_output.lines() {
        let trimmed = line.trim();
        let is_header = matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "commands:" | "subcommands:" | "available commands:"
        );
        if is_header {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        // A blank line or a new un-indented section header ends the block.
        if trimmed.is_empty() {
            // clap separates the last subcommand line from `Options:` with a
            // blank line — end here so `Options:` entries aren't swept in.
            break;
        }
        // Only count lines that are indented under the header (clap/cobra use
        // 2 spaces). An un-indented line is the start of the next section.
        if line == trimmed {
            break;
        }
        let Some(name) = trimmed.split_whitespace().next() else {
            continue;
        };
        if name == "help" {
            continue;
        }
        if !out.contains(&name.to_string()) {
            out.push(name.to_string());
        }
    }
    out
}

/// argparse-style subcommand parsing: the top-level usage line lists
/// subcommands inline as `{cmd1,cmd2}` (e.g. `usage: prog [-h] {foo,bar} ...`).
/// argparse has no dedicated `Commands:` section, so this is the fallback when
/// no section header matched. Only the FIRST `{...}` group on the usage line is
/// read (argparse emits exactly one, the subparser group).
fn extract_usage_brace_subcommands(help_output: &str) -> Vec<String> {
    for line in help_output.lines() {
        let t = line.trim();
        if !(t.starts_with("usage:") || t.starts_with("Usage:")) {
            continue;
        }
        let Some(inner) = t
            .split_once('{')
            .and_then(|(_, after)| after.split_once('}'))
            .map(|(inner, _)| inner)
        else {
            continue;
        };
        let mut out = Vec::new();
        for name in inner.split(',') {
            let name = name.trim();
            if name.is_empty() || name == "help" || out.contains(&name.to_string()) {
                continue;
            }
            out.push(name.to_string());
        }
        return out;
    }
    Vec::new()
}

/// Pull the subcommand *paths* a SKILL.md documents (the `### Subcommands`
/// bullets), so verify checks drift against exactly what the skill advertises
/// — the published surface, not the introspected one. Nested bullets become
/// multi-segment paths (`git remote add` → `["remote", "add"]`). Mirrors
/// [`extract_documented_invocation`]: a template section is the signal.
/// `### Subcommands` is an h3 (deliberately a subsection under the `## Invocation`
/// h2), so this is its own scan rather than reusing the h2-only `heading_block`.
pub fn extract_documented_subcommands(skill_md: &str) -> Vec<Vec<String>> {
    documented_subcommand_bullets(skill_md)
        .into_iter()
        .map(|(path, _bullet)| path)
        .collect()
}

/// Parse the `### Subcommands` block into `(path, bullet_line)` pairs,
/// indentation-aware. A bullet's nesting level is its leading-space count
/// divided by two (the template emits two spaces per depth), and each bullet's
/// path is its ancestor chain plus its own name. The bullet line is returned
/// verbatim so the per-subcommand flag diff reads exactly the flags that
/// bullet advertises.
pub fn documented_subcommand_bullets(skill_md: &str) -> Vec<(Vec<String>, String)> {
    let want = "### Subcommands";
    let mut in_block = false;
    let mut block = String::new();
    for line in skill_md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("### ") || trimmed.starts_with("## ") {
            if in_block {
                break;
            }
            if trimmed.eq_ignore_ascii_case(want) {
                in_block = true;
            }
            continue;
        }
        if in_block {
            block.push_str(line);
            block.push('\n');
        }
    }

    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new(); // (level, name)
    for line in block.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("- ") {
            continue;
        }
        // Each bullet is `- `name` ...`; the first backticked token is the name.
        let after = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        let Some(name) = after
            .split('`')
            .nth(1)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        // Two-space indent per depth (the template's contract).
        let indent = line.len() - line.trim_start_matches(' ').len();
        let level = indent / 2;
        while stack.len() > level {
            stack.pop();
        }
        let mut path: Vec<String> = stack.iter().map(|(_, n)| n.clone()).collect();
        path.push(name.to_string());
        out.push((path, line.to_string()));
        stack.push((level, name.to_string()));
    }
    out
}

/// Pull the first standalone version-looking token from `--version` output.
/// Handles `prog 0.13.0`, `v0.13.0`, `0.13.0 (build abc)`, and semver
/// pre-release/build suffixes (`1.0.0-rc.1+build2`). Returns `None` when no
/// digit-leading token is present (e.g. a CLI that prints only a git SHA or
/// human prose), so the caller can fall back to substring containment.
pub(crate) fn extract_version_token(stdout: &str) -> Option<String> {
    for tok in stdout.split_whitespace() {
        // Strip surrounding punctuation (parens, quotes, trailing comma) but
        // keep interior version chars.
        let t =
            tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != '+');
        if t.is_empty() || !t.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        // A single leading `v`/`V` is conventional; strip it for the compare.
        let core = t
            .strip_prefix('v')
            .or_else(|| t.strip_prefix('V'))
            .unwrap_or(t);
        let first = core.chars().next()?;
        if !first.is_ascii_digit() {
            continue;
        }
        // Plausible version: digits, dots, hyphens, plus, and (for semver
        // pre-release) ASCII letters. Anything else (e.g. a URL or sentence)
        // is not a version token.
        if core
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+'))
        {
            continue;
        }
        return Some(core.to_string());
    }
    None
}

/// Extract the set of `--double-dash` and `-single-dash` flags from a blob.
/// Only flags (a token whose first non-whitespace char is `-` followed by a
/// letter) count; `--` alone and bare `-` do not. Short flags require a letter
/// so we don't sweep up hyphenated prose like `two-step`.
pub fn extract_flags(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in text.split_whitespace() {
        // Strip clap-style optional-arg suffix early so `--flag[=<when>]`
        // normalizes consistently regardless of surrounding punctuation
        // (raw --help token has `[` interior → survives trim; backtick-wrapped
        // SKILL.md token has `[` at edge → stripped by trim → asymmetry bug).
        let t = if let Some(i) = tok.find("[=") {
            tok[..i].to_string()
        } else {
            tok.to_string()
        };
        // Strip surrounding punctuation we don't care about (commas, quotes),
        // but keep interior/leading dashes since flags begin with them.
        let t = t
            .trim_matches(|c: char| c.is_ascii_punctuation() && c != '-')
            .to_string();
        if !t.starts_with('-') || t.len() < 2 {
            continue;
        }
        // Find the first non-dash char; it must be a letter (filters `-` alone,
        // `--` alone, numeric `-1`, and `-2 step` prose).
        let first_letter = match t.chars().find(|c| *c != '-') {
            Some(c) => c,
            None => continue,
        };
        if !first_letter.is_ascii_alphabetic() {
            continue;
        }
        // Reject prose tokens: `/` and `'` are never inside a real flag
        // (e.g. `-x'/'--exec` is prose explaining the short/long pair, not a
        // flag). Catches separators in examples that single-letter rule misses.
        if t.contains('/') || t.contains('\'') {
            continue;
        }
        // Short flags (`-x`) are exactly one letter after the dash; multi-char
        // (`-tf`, `-foo`, `-mount`) are prose examples like `fd -tf` documenting
        // `--type f` shorthand, not real flags.
        let dash_count = t.chars().take_while(|c| *c == '-').count();
        if dash_count == 1 && t.len() > 2 {
            continue;
        }
        // Strip a trailing `=value` (`--foo=bar` -> `--foo`) and trailing
        // punctuation glued to the flag.
        let flag: String = t
            .split('=')
            .next()
            .unwrap_or(&t)
            .trim_end_matches([',', '.', ';', ':', ')', ']', '\''])
            .to_string();
        if flag.len() >= 2 && !out.contains(&flag) {
            out.push(flag);
        }
    }
    out
}
