//! C#/.NET: `*.csproj` selection + field extraction.

use std::fs;
use std::path::{Path, PathBuf};

use super::{extract_xml_tag, LanguageSpec};
use crate::introspect::cli_candidates::{which_on_path, CliCandidate};

pub(crate) struct CSharp;

impl LanguageSpec for CSharp {
    fn present(&self, dir: &Path) -> bool {
        crate::introspect::cli_probe::has_csproj(dir)
    }

    fn name(&self, root: &Path) -> Option<String> {
        if let Some(csproj) = select_csproj(root) {
            if let Ok(raw) = fs::read_to_string(&csproj) {
                if let Some(n) = extract_xml_tag(&raw, "AssemblyName") {
                    return Some(n);
                }
                if let Some(n) = extract_xml_tag(&raw, "RootNamespace") {
                    return Some(n);
                }
            }
        }
        None
    }

    fn version(&self, root: &Path) -> Option<String> {
        select_csproj(root)
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|raw| extract_xml_tag(&raw, "Version"))
    }

    fn authors(&self, root: &Path) -> Option<String> {
        select_csproj(root)
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|raw| extract_xml_tag(&raw, "Authors"))
            .and_then(|a| a.split(',').next().map(|s| s.trim().to_string()))
    }

    fn license(&self, root: &Path) -> Option<String> {
        select_csproj(root)
            .and_then(|p| fs::read_to_string(&p).ok())
            .and_then(|raw| extract_xml_tag(&raw, "PackageLicenseExpression"))
    }

    fn category_hint(&self) -> &'static str {
        "the .NET/C# tooling"
    }

    fn cursor_globs(&self) -> Vec<String> {
        vec!["*.cs".into(), "*.csproj".into(), "*.sln".into()]
    }

    fn import_pattern(&self, name: &str) -> String {
        format!("using {ns};", ns = name.replace('-', ""))
    }
    fn cli_candidate(&self, root: &Path, _name: &str) -> Option<CliCandidate> {
        // `dotnet run --project <csproj>` from the project root (the
        // canonical uninstalled invocation — mirrors `go run .`). Requires
        // `dotnet` on PATH (honest `None` otherwise). `select_csproj` skips
        // `WinExe` projects (GUI — no stdout) for deterministic, cross-platform
        // CLI invocation. The trailing `--` separates `dotnet run`'s own flags
        // from the app's argv so an appended `--help` reaches the app, not
        // dotnet (dotnet would print its own help and never invoke the program).
        which_on_path("dotnet")?;
        let csproj = select_csproj(root)?;
        let csproj_arg = csproj.to_string_lossy().to_string();
        Some(CliCandidate {
            argv: vec![
                "dotnet".to_string(),
                "run".to_string(),
                "--project".to_string(),
                csproj_arg,
                "--".to_string(),
            ],
            spawn_cwd: root.to_path_buf(),
        })
    }
}

/// Select the best csproj at root for CLI invocation. Prefers one with
/// `<OutputType>Exe</OutputType>`, skipping `WinExe` (GUI — no stdout).
/// Ties broken lexicographically by filename for cross-platform determinism.
/// Returns the path to the csproj, or `None` if none are suitable.
/// Re-exported from the `manifest` module root for `csharp_cli_candidate`.
pub(crate) fn select_csproj(root: &Path) -> Option<PathBuf> {
    let mut csprojs: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("csproj"))
        .collect();
    // Deterministic order for cross-platform parity (dir iteration order
    // varies by OS/filesystem).
    csprojs.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    // Pass 1: a csproj explicitly declaring Exe/Console.
    for p in &csprojs {
        if let Ok(raw) = fs::read_to_string(p) {
            match extract_xml_tag(&raw, "OutputType").as_deref() {
                Some("WinExe") => continue,
                Some("Exe") | Some("Console") => return Some(p.clone()),
                _ => {}
            }
        }
    }
    // Pass 2: no explicit OutputType — assume first non-WinExe csproj is a CLI.
    for p in &csprojs {
        if let Ok(raw) = fs::read_to_string(p) {
            if extract_xml_tag(&raw, "OutputType").as_deref() == Some("WinExe") {
                continue;
            }
        }
        return Some(p.clone());
    }
    None
}
