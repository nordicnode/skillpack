# skillpack

> Turn any OSS library or CLI into an agent-discoverable skill pack for Claude Code.

Coding agents (Claude Code, Cursor, Codex) now discover and invoke tools by reading
marketplace manifests, skill files, and CLIs — not by `npm install`-ing on instinct. An
OSS project's code quality no longer matters if the agent can't find, understand, and
autonomously invoke the tool. That wiring around a library or CLI is the **distribution
layer**, and `skillpack` generates it for you — then verifies that a coding agent coming
in cold could actually use what you shipped.

`skillpack` takes any OSS project and generates the three Claude Code distribution files,
then runs a verification suite that simulates an agent's first read and actually invokes
the documented CLI to catch drift before it reaches a user.

## What it generates

From your repo, `skillpack init` writes (purely additive — nothing existing is touched):

- `.claude-plugin/marketplace.json` — a single-plugin marketplace entry pointing at your project root
- `.claude-plugin/plugin.json` — the plugin manifest (name, version, author, repo URL)
- `skills/<tool-name>/SKILL.md` — the operational knowledge file an agent reads (frontmatter + body)

A `skillpack.toml` at your project root captures your answers so re-runs are deterministic
and CI-friendly.

## Supported ecosystems

| Language | CLI detection                             |
|----------|-------------------------------------------|
| Rust     | built binary under `target/`, or on PATH  |
| Node     | `node <script>` from a `package.json` bin |
| Python   | `python -m <pkg>` from `[project.scripts]` |
| Go       | `go run .` for a `package main` project   |
| Ruby     | a `ruby exe/<name>` binstub                |

Projects without a CLI take the pure-library path: `SKILL.md` documents the install +
import pattern instead, and the invocation test is a no-op. The `has_cli` flag is the
single branching point.

## Install

```sh
cargo install --path .    # from source; or `cargo install skillpack` once published
```

Requires Rust 1.74+.

## Quick start

```sh
# In your OSS project root:
skillpack init            # introspect → interview → generate → pre-commit verify

# Re-run anywhere / in CI (deterministic, non-interactive):
skillpack init --non-interactive --accept-warnings

# Check a generated (or hand-written) skill pack:
skillpack verify
```

`init` runs the full `verify` suite against its own output **before** writing files, so
the worst case — a broken skill pack that looks fine until an agent tries to use it — is
caught up front.

## What `verify` checks

**Discovery** — structural validation against the documented Claude Code schema:

- plugin / marketplace names are kebab-case and not reserved
- `description` is present and the combined description + `when_to_use` stays under the
  1,536-character listing cap
- `when_to_use` carries trigger phrases an agent can match on
- marketplace `source` paths use the `./` prefix and forward slashes only

**Invocation** — actually runs the documented CLI:

- `--help` executes cleanly under a hard timeout
- every flag documented in `SKILL.md` exists in the real `--help` output (catches drift)

Exits non-zero on any critical failure, so it drops straight into CI as a PR gate.

## Flags

| Flag                  | Purpose                                                          |
|-----------------------|------------------------------------------------------------------|
| `init --non-interactive` | skip prompts; requires a `skillpack.toml` (for CI)           |
| `init --accept-warnings` | write files even when `verify` flags warnings (critical still blocks) |
| `init --license <SPDX>`  | override the license for this run                              |
| `--verbose`           | print what `skillpack` detected in the repo (introspection)      |
| `--debug`             | print every subprocess call                                       |

## Status

V1: `init` + `verify`, Claude Code only, across the five ecosystems above. MIT-licensed.
Multi-ecosystem targets (Cursor, Codex) and a bundled skill-pack marketplace are later.

## Funding

`skillpack` is MIT-licensed and free forever. If it saves you time wiring an
agent-distribution layer, [sponsor on GitHub Sponsors](https://github.com/sponsors/nordicnode).
Curated, pre-verified skill packs (Polar.sh) come later — see the design spec.

## License

MIT. See [LICENSE](LICENSE). See [CHANGELOG.md](CHANGELOG.md) for release history.
