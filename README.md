<div align="center">
  <img src="docs/logo.png" width="480" alt="skillpack logo">
  <h1>skillpack</h1>
  <p><strong>The agent-distribution layer for modern OSS tools and libraries.</strong></p>
  <p>Generate, verify, and maintain agent instructions across Claude Code, Cursor, Codex, OpenCode, GitHub Copilot, Freebuff, and 10+ AI coding ecosystems.</p>

  <p>
    <a href="https://github.com/nordicnode/skillpack/actions/workflows/ci.yml"><img src="https://github.com/nordicnode/skillpack/actions/workflows/ci.yml/badge.svg" alt="CI Status"></a>
    <a href="https://crates.io/crates/skillpack"><img src="https://img.shields.io/crates/v/skillpack.svg" alt="crates.io version"></a>
    <a href="https://crates.io/crates/skillpack"><img src="https://img.shields.io/crates/d/skillpack.svg" alt="crates.io downloads"></a>
    <a href="https://docs.rs/skillpack"><img src="https://docs.rs/skillpack/badge.svg" alt="docs.rs"></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
    <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/rust-1.85%2B-orange.svg" alt="Rust 1.85+"></a>
  </p>

  <p>
    <a href="#quick-start">Quick Start</a> ·
    <a href="#key-features">Features</a> ·
    <a href="#what-skillpack-generates">What You Get</a> ·
    <a href="#command-line-reference">CLI Reference</a> ·
    <a href="docs/reference.md">Docs</a> ·
    <a href="CHANGELOG.md">Changelog</a> ·
    <a href="CONTRIBUTING.md">Contributing</a>
  </p>
</div>

---

## Built with Freebuff

<div align="center">
  <p><strong>This project was made possible by <a href="https://freebuff.com">Freebuff</a> — the free AI coding agent.</strong><br/>
  No subscription, credits, or API key required.</p>
  <p>
    <a href="https://freebuff.com"><img src="https://img.shields.io/badge/Try%20Freebuff-ff5c1c?style=for-the-badge" alt="Try Freebuff — the free AI coding agent"></a>
    <a href="https://github.com/CodebuffAI/freebuff"><img src="https://img.shields.io/badge/Freebuff%20on%20GitHub-2f353b?style=for-the-badge" alt="Freebuff on GitHub"></a>
  </p>
</div>

---

## Why skillpack?

AI coding assistants (Claude Code, Cursor, Codex, OpenCode, GitHub Copilot, Freebuff) interact with software differently than humans:

* **Humans** read high-level READMEs, tutorials, and manual pages to learn syntax.
* **AI Agents** look for machine-readable skill files, plugin manifests, rule definitions, and structured invocation syntax.

Without an agent guidance layer, AI coding agents frequently:
1. **Miss installed tools** and attempt to write redundant custom scripts from scratch.
2. **Hallucinate invalid CLI flags**, resulting in execution failures and broken scripts.
3. **Waste context tokens and time** repeatedly grepping `--help` outputs across multiple exploratory reasoning steps.

**`skillpack` solves this with a single command.** It introspects your project repository, discovers your CLI binaries or library interfaces, extracts verified argument signatures, and generates tailored distribution files for all major agent ecosystems.

---

## Key Features

* **Universal Multi-Ecosystem Generation**: Generates native guidance files for **10 agent formats** simultaneously (`AGENTS.md`, Claude Code, Cursor `.mdc`, OpenCode, Copilot, Codex, Windsurf, Freebuff, Gemini, and Aider).
* **Zero-Drift Verification**: Simulates agent invocations against your live CLI to verify that every documented flag and subcommand actually exists.
* **Pre-Commit Safe**: `skillpack init` validates the complete surface before writing a single file to disk.
* **Non-Destructive Updates**: `skillpack update` refreshes flags and versions while preserving your hand-written descriptions and notes.
* **Turnkey CI Integration**: Includes GitHub Actions workflows and `.pre-commit` hooks to prevent documentation drift in pull requests.

---

## Quick Start

### 1. Installation

```bash
# Prebuilt binaries — fastest, no Rust toolchain needed
cargo install cargo-binstall && cargo binstall skillpack

# Or build from source
cargo install skillpack --locked
```

### 2. Scaffold Agent Guidance

Run interactive setup in your project root:

```bash
# Interactive setup (asks a few quick questions and persists answers to skillpack.toml)
skillpack init

# Or generate for all supported ecosystems at once:
skillpack init --target all
```

### 3. Zero-Interaction Automated Setup (`--auto`)

For instant, unattended generation in CI or local scripts:

```bash
# Introspects README, git config, LICENSE, and compiled CLI binaries automatically
skillpack init --auto --target all
```

### 4. Verify Accuracy

Validate your repository's agent guidance against your real CLI:

```bash
skillpack verify
```

---

## What skillpack Generates

Running `skillpack init --target all` generates a clean, non-intrusive distribution layer:

| Target / Ecosystem | Generated File(s) | Description |
|---|---|---|
| **AGENTS.md** | `AGENTS.md` | Standard instructions file read natively by **Freebuff**, Zed, Cursor, Windsurf, and 60k+ repos. |
| **Claude Code** | `.claude-plugin/` & `skills/<tool>/SKILL.md` | Plugin manifest and skill specification for Claude Code and Codex. |
| **Cursor** | `.cursor/rules/<tool>.mdc` | Context-aware rule file with automated file-glob matching. |
| **OpenCode** | `.opencode/agents/<tool>.md` | Agent definition for OpenCode AI coding environments. |
| **GitHub Copilot** | `.github/copilot-instructions.md` | Custom repository instructions for GitHub Copilot. |
| **Gemini CLI** | `GEMINI.md` | Native repository instruction layer for Google Gemini CLI. |
| **Windsurf** | `.windsurf/rules/<tool>.md` | Cascade IDE rule file. |
| **Aider** | `CONVENTIONS.md` | Codebase convention guidelines for Aider. |
| **Deterministic Config** | `skillpack.toml` | Committed configuration making future updates and CI checks deterministic. |

---

## Measured Benchmark: The skillpack Impact

We benchmarked autonomous coding agents performing complex search and execution tasks on a plain repository clone versus a `skillpack`-guided clone:

```
Baseline (Plain Clone):     [Tool Error] ──> [Help Grep 1] ──> [Help Grep 2] ──> [Retry 1] ──> [Success in 6 Rounds] (179s)
Guided (With skillpack):    [Direct Execution with Verified Flags] ─────────────> [Success in 1 Round] (82s)
```

### Performance Metrics (OpenCode / `sharkdp/fd` Benchmark Suite)

| Metric | Plain Clone (Baseline) | Clone + skillpack | Improvement |
|---|---|---|---|
| **Wall Clock Latency** | 158 s | **88 s** | **44% Faster** |
| **Reasoning Rounds** | 12.5 steps | **8.5 steps** | **32% Fewer Steps** |
| **Help Query Detours** | 4 – 5 queries | **0 queries** | **100% Elimination** |
| **Task Accuracy** | 3.5 / 4 (87.5%) | **4.0 / 4 (100%)** | **+14% Accuracy** |

> *Full benchmark methodology, repeatable test suites (`fd`, `ripgrep`, `bat`), and replay harness documentation are available in [`docs/benchmark.md`](docs/benchmark.md). Want to see it happen? Read the step-by-step [`docs/agent-demo.md`](docs/agent-demo.md) — or reproduce the A/B run yourself from the committed transcripts.*

---

## Command-Line Reference

* **`skillpack init`**: Scaffold agent distribution files (interactive, `--auto`, or `--non-interactive`).
* **`skillpack verify`**: Check guidance files against agent schemas and live `--help` flag surfaces (supports `--min-score <N>`, `--fix`, and `--format {human,json,sarif}`).
* **`skillpack doctor`**: Diagnose language and CLI candidate discovery decision traces.
* **`skillpack update`**: Incrementally regenerate files from `skillpack.toml` after CLI changes.
* **`skillpack diff`**: Preview pending guidance updates without modifying disk.

---

## CI/CD & Pre-Commit Integration

### GitHub Actions Workflow

Add `.github/workflows/skillpack.yml` to prevent documentation drift in PRs:

```yaml
name: Verify Agent Guidance

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install skillpack
        run: cargo install skillpack --locked
      - name: Verify Guidance Layer
        run: skillpack verify --min-score 100
```

### Pre-Commit Hook

Add to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/nordicnode/skillpack
    rev: v0.12.0
    hooks:
      - id: skillpack-verify
```

---

## Supported Language Ecosystems

`skillpack` automatically detects CLI binaries and library entrypoints across:

* **Rust**: Built artifacts in `target/{release,debug}` (including Cargo workspaces) and PATH binaries.
* **Node.js**: `package.json` `bin` fields and `./bin/<name>.js` scripts.
* **Python**: `pyproject.toml` `[project.scripts]` and Poetry/Flit entries.
* **Go**: `go run .` for `package main` modules and built binaries.
* **Ruby**: Gem binstubs in `exe/` and `bin/`.
* **PHP**: Composer package binaries in `vendor/bin`.
* **JVM**: Gradle `installDist` and Maven shaded JAR entrypoints.
* **.NET / C#**: SDK-style executable `.csproj` targets.
* **Zig**: `build.zig` / `build.zig.zon` and artifacts in `zig-out/bin/`.
* **Swift**: `Package.swift` and `.build/{debug,release}/` artifacts or `swift run`.
* **C / C++**: `CMakeLists.txt`, `meson.build`, `Makefile` and `build/` / `bin/` binaries.
* **Elixir**: `mix.exs` and `_build/{dev,prod}/rel/` releases or `mix escript`.
* **Deno**: `deno.json` / `deno.jsonc` and `deno run` script entrypoints.

---

## License & Community

`skillpack` is open-source software licensed under the **[MIT License](LICENSE)**.

* **Documentation**: See [`docs/reference.md`](docs/reference.md) for full check catalogs and schemas.
* **Changelog**: See [`CHANGELOG.md`](CHANGELOG.md) for version release notes.
* **Contributing**: Contributions are welcome! Review [`CONTRIBUTING.md`](CONTRIBUTING.md) to get started.
