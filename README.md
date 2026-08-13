<div align="center"><img src="docs/logo.png" width="512" alt="skillpack logo"></div>

# skillpack — make your tool easy for AI coding agents to find and use

[![CI](https://github.com/nordicnode/skillpack/actions/workflows/ci.yml/badge.svg)](https://github.com/nordicnode/skillpack/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/skillpack.svg)](https://crates.io/crates/skillpack)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

## What is this?

AI coding agents (Claude Code, Cursor, Codex, OpenCode, GitHub Copilot, and any tool that reads `AGENTS.md`) don't use your project the way a person does. A person reads the README and runs the install command. An agent looks for a `SKILL.md`, a plugin manifest, or `--help` output — and if those aren't there, it often can't find your tool at all, or it guesses the wrong command and gets stuck.

skillpack closes that gap with one command. It:

1. **Learns** your project — what it's called, what language it's in, whether it ships a CLI, and what that CLI can actually do.
2. **Generates** the small set of files agents read to discover, understand, and run your tool.
3. **Verifies** those files by simulating an agent's first visit — including running your real CLI and checking that every documented flag actually exists.

The whole loop is safe: `init` verifies its own output **before** writing a single file, so a broken pack can never silently ship.

## Quick start

```sh
# 1. Install
cargo install skillpack

# 2. From your project root — a few quick questions, then everything is generated
skillpack init

# 3. Want every agent ecosystem, not just Claude Code?
skillpack init --target all

# 4. Re-check the generated files anytime (and in CI)
skillpack verify
```

Your answers are saved to a `skillpack.toml` (commit it), so re-runs are instant and CI-friendly: `skillpack init --non-interactive` needs no prompts. Want the least interaction possible? `skillpack init --auto` needs **zero** flags and **zero** prompts — it derives the description from your README, the author from `git config`, the license from your LICENSE file, and the invocation from the detected CLI, then writes the whole pack and verifies it. (For a library, add `--import "..."`; for anything exotic, the explicit bootstrap flags — `--description`/`--trigger`/`--author`/`--invocation`/`--import` — are there too.)

## What you get

`init` writes a handful of small files and touches nothing you already have:

- **Claude Code** (the default) — a marketplace + plugin manifest (`.claude-plugin/`) and a `SKILL.md` that explains what your tool does and demonstrates a real invocation.
- **Cursor** — a project rule (`.cursor/rules/…mdc`) that auto-attaches when relevant files are open.
- **Codex** — the same skill, under Codex's `.codex/skills/` convention.
- **OpenCode** — an agent definition (`.opencode/agents/…md`).
- **GitHub Copilot** — repo instructions (`.github/copilot-instructions.md`).
- **AGENTS.md** — a plain instructions file that 60k+ projects' agents read natively, including Codex, Windsurf, Zed, JetBrains Junie, aider, and Freebuff.
- **CLAUDE.md** — the Claude Code–ecosystem memory file (read by Cline, Roo Code, and Claude Code itself).
- **GEMINI.md** — project instructions for Google's Gemini CLI.
- **Windsurf** — Cascade rules (`.windsurf/rules/…md`), same frontmatter as Cursor.
- **Aider** — repo conventions (`CONVENTIONS.md`).

`--target all` writes all ten. Have an agent harness that reads a different file? Its format is almost certainly one of the above — plain markdown, a rules file, or a skill directory — and you can point it at what skillpack generates.

CLI projects get a skill that documents the real command and its flags; pure libraries get install + import instructions instead. When your CLI's surface changes, `skillpack update` refreshes the generated files without re-answering the questions.

## Does it actually help? (measured)

Four `fd` search tasks, run with [OpenCode](https://opencode.ai) on a plain clone of `sharkdp/fd` versus the same clone + `skillpack init --target opencode --target claude --target cursor`. Same model, same questions:

| Metric | plain clone | clone + skillpack | delta |
|---|---|---|---|
| Agent step rounds | 20 | 5 | **-75%** |
| Token total | 38,134 | 22,248 | **-42%** |
| Wall clock | 130 s | 27 s | **-79%** |

Both conditions got all four answers right — the win is *efficiency*, not capability. The biggest one: the plain-clone agent hit fd's `--max-results`/`-x` incompatibility and retried four times; the generated agent had the verified flag mapping and answered in one step. Full methodology and honest limitations (including one spot where the skillpack agent was *less* accurate) are in [`docs/agent-demo.md`](docs/agent-demo.md).

That demo was a one-off. The repeatable harness lives in [`scripts/benchmark/`](scripts/benchmark/) — one command (`run.sh --runs 3`) runs the same fd A/B through OpenCode with medians and evidence scoring, and it fixes the demo's confound by holding the agent wrapper constant (no `--agent`; the skillpack condition differs only by its generated `AGENTS.md`). Methodology and how to run it: [`docs/benchmark.md`](docs/benchmark.md).

## Use it in CI

`verify` exits non-zero when something's broken, so it drops straight into CI as a pull-request gate. A reusable workflow ships in this repo — one line in your workflow:

```yaml
jobs:
  skillpack:
    uses: nordicnode/skillpack/.github/workflows/skillpack.yml@v0.11.2
```

Pin to a released tag (e.g. `@v0.11.2`) and bump it when you want new features. It installs `skillpack` from crates.io and runs `skillpack verify --format json` on your repo, on the same OS matrix skillpack itself is tested on (Ubuntu, macOS, Windows). Prefer your own workflow? `cargo install skillpack --locked && skillpack verify` is all you need.

## What it checks

In plain terms, `skillpack verify` asks three questions:

- **Is everything where it should be?** Are the files valid and well-formed — correct kebab-case names, real descriptions, parseable JSON, no Anthropic-reserved names, no malformed frontmatter?
- **Does the CLI match what's documented?** It runs your actual CLI: `--help` exits cleanly, every flag the skill mentions really exists, and the advertised version is real.
- **How discoverable is the pack?** Every run produces a 0–100 score; pass `--min-score <N>` to make CI enforce a floor (useful for catching drift over time).

The complete check list, every command-line flag, and the platform details live in [docs/reference.md](docs/reference.md).

## Supported languages

| Language | How the CLI is detected |
|----------|-------------------------|
| Rust     | built binary under `target/`, or on PATH |
| Node     | `node <script>` from a `package.json` bin |
| Python   | `python -m <pkg>` from `[project.scripts]` |
| Go       | `go run .` for a `package main` project |
| Ruby     | a `ruby exe/<name>` (or `bin/<name>`) binstub |
| PHP      | `php <script>` from a `composer.json` bin entry |
| JVM      | pre-built Gradle `installDist` script, or `java -jar` a Maven shaded / Gradle shadow jar (pure file reads — no build invoked) |
| C#       | `dotnet run --project <csproj>` (SDK-style, `OutputType=Exe`; GUI projects skipped) |

Works on macOS, Linux, and Windows.

## Requirements

Rust 1.85+. Install from [crates.io](https://crates.io/crates/skillpack) with `cargo install skillpack`, or build from source with `cargo install --path .`.

## Status

Actively developed, MIT-licensed. See [CHANGELOG.md](CHANGELOG.md) for the release history, and [CONTRIBUTING.md](CONTRIBUTING.md) if you'd like to help — editing the templates needs no Rust knowledge.
