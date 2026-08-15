//! CLI probe: workspace walking + `--help` capture. Owns the full
//! "is there an invokable CLI here, and if so what does `--help` say?"
//! pipeline — the guarded `--help` spawn and the workspace member walks
//! that find a binary when the root manifest is workspace-only.
//!
//! Sibling to [`super::cli_candidates`]: that module *resolves* a candidate
//! argv from pure filesystem reads + PATH probes (no subprocess); this
//! module *spawns* the resolved candidate under a hard timeout and maps the
//! outcome to [`DetectCli`]. Split out of `introspect.rs` (v0.9.3).
//!
//! Every non-clean branch pushes a `DiagNote` so `skillpack doctor` can
//! explain why `has_cli=false` (or `help_output=None`) happened — never a
//! silent gap.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::spawn::{self, SpawnOutcome, HELP_TIMEOUT};
use crate::types::{DiagTrace, Language, SubcommandNode};

use super::cli_candidates::{primary_cli_candidate, CliCandidate, DetectCli};
use super::workspace::{
    expand_workspace_members, is_cargo_workspace_only, is_npm_workspace_only, pyproject_has_tool,
};

/// Walk a Cargo workspace's members looking for a crate with a CLI binary.
/// Parses `Cargo.toml` `[workspace].members` (including glob patterns like
/// `crates/*`), then for each member probes `primary_cli_candidate` against
/// the member's `[package].name`. Pushes a diag note per member tried so
/// doctor explains the walk; returns `Some` on the first member that yields
/// a runnable CLI, `None` if none do.
pub(crate) fn walk_cargo_workspace(
    root: &Path,
    _name: &str,
    diag: &mut DiagTrace,
) -> Option<DetectCli> {
    let raw = fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let v = toml::from_str::<toml::Value>(&raw).ok()?;
    let members = v.get("workspace")?.get("members")?.as_array()?;
    let raw_list: Vec<String> = members
        .iter()
        .filter_map(|m| m.as_str().map(String::from))
        .collect();
    let expanded = expand_workspace_members(root, &raw_list);
    diag.push(
        "detect_cli.rust.workspace",
        format!(
            "Cargo workspace root: {} member(s) to probe",
            expanded.len()
        ),
    );
    for member_rel in expanded {
        let member_root = root.join(&member_rel);
        if !member_root.join("Cargo.toml").is_file() {
            diag.push(
                "detect_cli.rust.workspace",
                format!("member `{member_rel}` has no Cargo.toml, skipped"),
            );
            continue;
        }
        // Prefer the member's own [package].name; fall back to the dir tail.
        let manifest_name = fs::read_to_string(member_root.join("Cargo.toml"))
            .ok()
            .and_then(|r| toml::from_str::<toml::Value>(&r).ok())
            .and_then(|v| {
                v.get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .map(String::from)
            });
        let Some(member_name) = manifest_name.or_else(|| {
            member_root
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        }) else {
            diag.push(
                "detect_cli.rust.workspace",
                format!("member `{member_rel}` has no name in manifest, skipping"),
            );
            continue;
        };
        match primary_cli_candidate(&member_root, Language::Rust, &member_name) {
            Some(candidate) => {
                diag.push(
                    "detect_cli.rust.workspace",
                    format!(
                        "member `{member_rel}` yielded candidate `{}`",
                        candidate.argv.join(" ")
                    ),
                );
                return Some(spawn_candidate(&candidate, diag));
            }
            None => diag.push(
                "detect_cli.rust.workspace",
                format!("member `{member_rel}` (`{member_name}`): no built/installed artifact"),
            ),
        }
    }
    diag.push(
        "detect_cli.rust.workspace",
        "no workspace member yielded a runnable CLI; has_cli=false \
         (run `skillpack init` inside the member crate that ships the binary)"
            .to_string(),
    );
    None
}

/// Walk an npm workspace's members (including `packages/*` patterns)
/// looking for a package with a `bin`. Parses `package.json` `workspaces`
/// (string or array of strings). Returns `Some` on the first member that
/// yields a runnable CLI; `None` otherwise. Pushes a diag note per member.
pub(crate) fn walk_npm_workspace(
    root: &Path,
    _name: &str,
    diag: &mut DiagTrace,
) -> Option<DetectCli> {
    let raw = fs::read_to_string(root.join("package.json")).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let ws = v.get("workspaces")?;
    let paths: Vec<String> = match ws {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|e| e.as_str().map(String::from))
            .collect(),
        _ => return None,
    };
    let expanded = expand_workspace_members(root, &paths);
    diag.push(
        "detect_cli.node.workspace",
        format!("npm workspace root: {} member(s) to probe", expanded.len()),
    );
    for member_rel in expanded {
        let member_root = root.join(&member_rel);
        let pkg_json = member_root.join("package.json");
        if !pkg_json.is_file() {
            diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` has no package.json, skipped"),
            );
            continue;
        }
        let Ok(mraw) = fs::read_to_string(&pkg_json) else {
            continue;
        };
        let Ok(mv) = serde_json::from_str::<serde_json::Value>(&mraw) else {
            continue;
        };
        let Some(member_name) = mv
            .get("name")
            .and_then(|n| n.as_str())
            .map(String::from)
            .or_else(|| {
                member_root
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
        else {
            diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` has no name in manifest, skipping"),
            );
            continue;
        };
        if mv.get("bin").is_none() {
            diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` (`{member_name}`): no `bin` field, skipped"),
            );
            continue;
        }
        match primary_cli_candidate(&member_root, Language::Node, &member_name) {
            Some(candidate) => {
                diag.push(
                    "detect_cli.node.workspace",
                    format!(
                        "member `{member_rel}` yielded candidate `{}`",
                        candidate.argv.join(" ")
                    ),
                );
                return Some(spawn_candidate(&candidate, diag));
            }
            None => diag.push(
                "detect_cli.node.workspace",
                format!("member `{member_rel}` (`{member_name}`): candidate None (node missing?)"),
            ),
        }
    }
    diag.push(
        "detect_cli.node.workspace",
        "no workspace member yielded a runnable CLI; has_cli=false \
         (run `skillpack init` inside the member package that ships the bin)"
            .to_string(),
    );
    None
}

/// True if the root contains any `*.gemspec` file.
pub(crate) fn has_gemspec(root: &Path) -> bool {
    fs::read_dir(root).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("gemspec"))
    })
}

/// True if the root contains any `*.csproj` file. Solution-only repos (`.sln`
/// at root, csproj in subdirs) are not detected — same limitation class as
/// Cargo workspace-only roots. ponytail: add .sln directory walk when needed.
pub(crate) fn has_csproj(root: &Path) -> bool {
    fs::read_dir(root).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("csproj"))
    })
}

/// True if the root contains any `*.cabal` file (Haskell package manifest).
pub(crate) fn has_cabal_file(root: &Path) -> bool {
    has_file_with_ext(root, "cabal")
}

/// True if the root contains any file with the given extension (case-sensitive).
pub(crate) fn has_file_with_ext(root: &Path, ext: &str) -> bool {
    fs::read_dir(root).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some(ext))
    })
}

/// True if the root contains any file whose name ends with `suffix` (for
/// double-extension manifests like `.app.src`, which `has_file_with_ext`
/// can't match because `Path::extension` returns only the last segment).
pub(crate) fn has_file_ending_with(root: &Path, suffix: &str) -> bool {
    fs::read_dir(root).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
        })
    })
}

/// True if a top-level `name` file exists and contains `needle` as a
/// whole-line prefix (or anywhere on a line, for a loose `contains` match).
/// Used to distinguish R's `DESCRIPTION` (DCF `Package:` header) from an
/// unrelated file that happens to share the name.
pub(crate) fn root_file_contains(root: &Path, name: &str, needle: &str) -> bool {
    fs::read_to_string(root.join(name))
        .ok()
        .is_some_and(|raw| raw.lines().any(|l| l.contains(needle)))
}

/// Detect whether the project ships an invokable CLI, and if so capture its
/// `--help` output under a hard timeout. Returns
/// `(has_cli, command, output, subcommand_tree)`.
///
/// `command` is the full multi-token `--help` argv the verifier re-spawns (e.g.
/// `["node","/abs/bin/cli.js","--help"]`, `["go","run",".","--help"]`). The
/// bare human-facing invocation that SKILL.md publishes is derived separately
/// from the profile name + interview — this is the internal, machine-specific
/// spawn argv (design §5.1, §6.3).
///
/// `subcommand_tree` holds the recursive `<cli> <path...> --help` capture
/// (clap-style), in declaration order, so the generated SKILL.md can document
/// the real command surface and `verify` can drift-check it. Empty for
/// non-subcommand CLIs — a flat `--help` yields no `Commands:` section.
///
/// Every falsy branch (no name, no root candidate, spawn failure) pushes a
/// `DiagNote` so `skillpack doctor` explains why `has_cli=false` rather than
/// silently reporting it. Workspace-only roots (Cargo `[workspace]` only,
/// npm `workspaces` no `bin`) trigger a member walk before giving up.
pub(crate) fn detect_cli(
    root: &Path,
    language: Language,
    name: Option<String>,
    diag: &mut DiagTrace,
) -> DetectCli {
    let Some(name) = name else {
        diag.push(
            "detect_cli",
            "no tool name derivable from the manifest or repo; ".to_string()
                + "cannot probe for a CLI without a name",
        );
        return DetectCli::none();
    };

    let Some(candidate) = primary_cli_candidate(root, language, &name) else {
        // The root didn't yield a runnable CLI. For workspace roots the binary
        // lives in a member crate/package; walk members before reporting a
        // final `has_cli=false`. uv/poetry monorepos are NOT walked yet —
        // doctor notes the gap so the maintainer can run init in the member.
        if language == Language::Rust && is_cargo_workspace_only(root) {
            if let Some(d) = walk_cargo_workspace(root, &name, diag) {
                return d;
            }
        }
        if language == Language::Node && is_npm_workspace_only(root) {
            if let Some(d) = walk_npm_workspace(root, &name, diag) {
                return d;
            }
        }
        diag.push(
            "detect_cli",
            format!(
                "primary_cli_candidate for language `{}` returned None; \
                 runtime may be missing, no build artifact present, or no bin \
                 entry point. Run `skillpack doctor --verbose` to see the raw \
                 profile; if this is a monorepo member, try running \
                 `skillpack init` inside the member directory.",
                language.as_str()
            ),
        );
        // uv / poetry Python monorepo: explicitly NOT walked yet.
        if language == Language::Python
            && (root.join("uv.toml").exists()
                || pyproject_has_tool(root, "uv")
                || pyproject_has_tool(root, "poetry"))
        {
            diag.push(
                "detect_cli.python",
                "uv/poetry workspace detected; member walking not yet \
                 implemented; run `skillpack init` in the member package dir"
                    .to_string(),
            );
        }
        return DetectCli::none();
    };
    spawn_candidate(&candidate, diag)
}

/// Build the `--help` command from `candidate`, spawn it under the hard
/// timeout, and map the outcome to a `DetectCli`. Pushes a diag note on
/// every non-clean outcome so `doctor` explains timeouts/non-zero/missing.
/// Returns `DetectCli::none()` when the spawn can't run at all (NotFound /
/// SpawnFailed), `has_cli=true` with `help_output=None` on a RanNonZero or
/// TimedOut result (the binary exists and responded — it's a CLI — but the
/// help text wasn't captured).
fn spawn_candidate(candidate: &CliCandidate, diag: &mut DiagTrace) -> DetectCli {
    // Build the spawn command from the multi-token argv (program + args, minus
    // `--help`), then append `--help` for the help capture.
    let mut command = candidate.argv.clone();
    command.push("--help".to_string());

    let mut cmd = Command::new(&candidate.argv[0]);
    for arg in &candidate.argv[1..] {
        cmd.arg(arg);
    }
    cmd.arg("--help")
        .current_dir(&candidate.spawn_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match spawn::run(&mut cmd, HELP_TIMEOUT) {
        SpawnOutcome::RanClean(output) => {
            // A subcommand CLI advertises its subcommands in the top-level
            // `--help`; capture each one's `--help` so the generated SKILL.md
            // documents the real surface (init/verify + their flags, not the
            // global flags). Best-effort: a subcommand that fails/times out is
            // omitted here — `verify` surfaces the gap if the skill documents
            // a subcommand we couldn't capture.
            let subs = capture_subcommand_tree(candidate, &output, diag);
            DetectCli {
                has_cli: true,
                command: Some(command),
                help_output: Some(output),
                subcommand_tree: subs,
            }
        }
        SpawnOutcome::RanNonZero(_) => {
            diag.push(
                "detect_cli",
                format!(
                    "`{} --help` exited non-zero; help output not captured",
                    command.join(" ")
                ),
            );
            DetectCli {
                has_cli: true,
                command: Some(command),
                help_output: None,
                subcommand_tree: Vec::new(),
            }
        }
        SpawnOutcome::TimedOut => {
            diag.push(
                "detect_cli",
                format!(
                    "`{} --help` timed out after {HELP_TIMEOUT:?}",
                    command.join(" ")
                ),
            );
            DetectCli {
                has_cli: true,
                command: Some(command),
                help_output: None,
                subcommand_tree: Vec::new(),
            }
        }
        SpawnOutcome::NotFound => {
            diag.push(
                "detect_cli",
                format!(
                    "spawn failed: `{}` binary not found on PATH",
                    command.first().unwrap_or(&candidate.argv[0])
                ),
            );
            DetectCli::none()
        }
        // ponytail: permission-denied etc. are rare; mapping to `none()`
        // means `has_cli=false` (pure-library path) rather than crashing.
        // verify's spawn will then surface the gap downstream if the CLI IS
        // documented. The honest path for V1 — doesn't crash.
        SpawnOutcome::SpawnFailed(_) => {
            diag.push(
                "detect_cli",
                "spawn failed (permission-denied or OS error); treated as has_cli=false"
                    .to_string(),
            );
            DetectCli::none()
        }
    }
}

/// Hard cap on how deep [`capture_subcommand_tree`] recurses. Each level
/// multiplies the spawn count, so the cap bounds the probe (and the generated
/// SKILL.md's nesting) while still covering real-world CLIs like
/// `git remote add` and `aws ec2 describe-instances` (two nested levels).
/// Deeper nesting than this is out of scope, same posture as the old
/// one-level-deep limit — but now genuinely recursive.
const MAX_SUBCOMMAND_DEPTH: usize = 4;

/// Hard cap on the TOTAL number of `--help` probes across the whole tree.
/// Depth is bounded above, but breadth is not — a wide CLI (e.g. one with
/// hundreds of top-level subcommands) would otherwise fork a process per node
/// and, multiplied by recursion, could spawn thousands of probes. The budget
/// keeps introspect bounded and predictable; when it is exhausted we truncate
/// the tree and note it in the `doctor` trace rather than silently dropping
/// branches.
const MAX_SUBCOMMAND_SPAWNS: usize = 64;

/// A shared, decrementing spawn allowance threaded through the recursive
/// capture. `exhausted` records whether the budget ever hit zero.
struct SpawnBudget {
    remaining: usize,
    exhausted: bool,
}

impl SpawnBudget {
    fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            exhausted: false,
        }
    }

    /// Try to spend one spawn. Returns `true` when the spawn may proceed.
    fn spend(&mut self) -> bool {
        if self.remaining == 0 {
            self.exhausted = true;
            return false;
        }
        self.remaining -= 1;
        true
    }
}

/// For a subcommand CLI, recursively capture `<cli> <path...> --help` for every
/// subcommand advertised at each level, returning a `SubcommandNode` tree in
/// declaration order. Reuses the same guarded spawn + timeout as the top-level
/// capture. A spawn that fails/times out still contributes a node (its name came
/// from the parent's `--help`) with empty `help` and no `children`, so verify
/// can surface the gap if the skill documents that subcommand. When the total
/// spawn budget is exhausted, the tree is truncated and a `doctor` note records
/// it.
fn capture_subcommand_tree(
    candidate: &CliCandidate,
    top_level_help: &str,
    diag: &mut DiagTrace,
) -> Vec<SubcommandNode> {
    let mut budget = SpawnBudget::new(MAX_SUBCOMMAND_SPAWNS);
    let tree = capture_children(candidate, &[], top_level_help, 0, &mut budget);
    if budget.exhausted {
        diag.push(
            "detect_cli",
            format!(
                "subcommand tree truncated after {MAX_SUBCOMMAND_SPAWNS} `--help` probes \
                 (wide CLI); deeper branches are not documented"
            ),
        );
    }
    tree
}

/// Recursive core of [`capture_subcommand_tree`]. `prefix` is the subcommand
/// path already walked (empty at the top level); `help_output` is the `--help`
/// text of the node at `prefix`; `depth == prefix.len()`.
fn capture_children(
    candidate: &CliCandidate,
    prefix: &[String],
    help_output: &str,
    depth: usize,
    budget: &mut SpawnBudget,
) -> Vec<SubcommandNode> {
    if depth >= MAX_SUBCOMMAND_DEPTH {
        return Vec::new();
    }
    let subs = crate::verify::invocation::extract_subcommands(help_output);
    let mut out = Vec::with_capacity(subs.len());
    for sub in subs {
        // Honor the shared budget: once it is spent, stop probing (the rest of
        // this level's subcommands are silently omitted rather than documented
        // shallowly — verify will surface the gap if the skill documents them).
        if !budget.spend() {
            break;
        }
        let help = spawn_subcommand_help(candidate, prefix, &sub);
        let mut child_prefix = prefix.to_vec();
        child_prefix.push(sub.clone());
        let children = if help.is_empty() {
            Vec::new()
        } else {
            capture_children(candidate, &child_prefix, &help, depth + 1, budget)
        };
        out.push(SubcommandNode {
            name: sub,
            help,
            children,
        });
    }
    out
}

/// Spawn `<candidate.argv> <prefix...> <sub> --help` and return the captured
/// stdout+stderr, or `""` when the spawn failed/timed out. Best-effort:
/// introspect never fails on a subcommand it can't probe.
fn spawn_subcommand_help(candidate: &CliCandidate, prefix: &[String], sub: &str) -> String {
    let mut cmd = Command::new(&candidate.argv[0]);
    for arg in &candidate.argv[1..] {
        cmd.arg(arg);
    }
    for p in prefix {
        cmd.arg(p);
    }
    cmd.arg(sub)
        .arg("--help")
        .current_dir(&candidate.spawn_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match spawn::run(&mut cmd, HELP_TIMEOUT) {
        SpawnOutcome::RanClean(help) => help,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    //! Workspace-walk tests: assert the walk probes every member and reports
    //! each one in the diag trace, so doctor explains the hit/miss sequence.

    use super::*;

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    // ponytail: walk_*_workspace skip branch (member with no name in manifest
    // AND dir-tail file_name() None) is unreachable for non-root member paths —
    // the path-tail fallback always yields a name. These tests assert the
    // observable contract we DO hit: the walk continues past every member to the
    // end, not aborting on the first no-artifact member. Skip-and-continue vs
    // early-return-None is indistinguishable here only if a name resolution
    // failure occured; the `?`→`continue` fix guards that pathological case.
    #[test]
    fn walk_cargo_workspace_continues_past_no_artifact_member() {
        let root = std::env::temp_dir().join(format!(
            "skillpack-walk-cargo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("members/m1")).unwrap();
        std::fs::create_dir_all(root.join("members/m2")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"members/m1\", \"members/m2\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m1/Cargo.toml"),
            "[package]\nname = \"m1\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m2/Cargo.toml"),
            "[package]\nname = \"m2\"\n",
        )
        .unwrap();
        let mut diag = DiagTrace::default();
        let res = walk_cargo_workspace(&root, "ws", &mut diag);
        assert!(res.is_none(), "no member has a built artifact → None");
        let notes: Vec<&str> = diag.0.iter().map(|d| d.note.as_str()).collect();
        assert!(
            notes.iter().any(|n| n.contains("m1")),
            "m1 probed: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("m2")),
            "m2 probed: {notes:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn walk_npm_workspace_continues_past_no_cli_member() {
        let root = std::env::temp_dir().join(format!(
            "skillpack-walk-npm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("members/m1")).unwrap();
        std::fs::create_dir_all(root.join("members/m2")).unwrap();
        std::fs::write(
            root.join("package.json"),
            "{ \"workspaces\": [\"members/m1\", \"members/m2\"] }",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m1/package.json"),
            "{ \"name\": \"m1\", \"bin\": {} }",
        )
        .unwrap();
        std::fs::write(
            root.join("members/m2/package.json"),
            "{ \"name\": \"m2\", \"bin\": {} }",
        )
        .unwrap();
        let mut diag = DiagTrace::default();
        let res = walk_npm_workspace(&root, "ws", &mut diag);
        assert!(res.is_none(), "bin:{{}} → both candidate None → walk None");
        let notes: Vec<&str> = diag.0.iter().map(|d| d.note.as_str()).collect();
        assert!(
            notes.iter().any(|n| n.contains("m1")),
            "m1 probed: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("m2")),
            "m2 probed: {notes:?}"
        );
        cleanup(&root);
    }
}
