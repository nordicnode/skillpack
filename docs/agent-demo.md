# skillpack demo: real-world agent delta (measured)

> How we showed, not asserted, the difference skillpack makes to a real AI agent.

On a four-task real-world fd CLI exercise, a skillpack-generated OpenCode agent finished the same correct task set in **5 rather than 20** agent steps, **27 rather than 130** seconds, and **42% fewer** final tokens. This is one controlled run, not a general benchmark; the skill reduced detours but did not eliminate semantic mistakes.

## Demo setup

- **Repo**: [fd](https://github.com/sharkdp/fd) (`fd-find` crate, `fd` binary). Real popular Rust CLI.
- **Agent runtime**: [OpenCode](https://opencode.ai) 1.17.15, non-interactive via `opencode run --format json`.
- **A/B clones**:
  - **Condition A — without skillpack** at `/tmp/fd-no-skillpack`: plain `git clone --depth=1` of fd. No `.opencode/`, no `skills/`, no `CLAUDE.md`/`AGENTS.md`.
  - **Condition B — with skillpack** at `/tmp/fd-with-skillpack`: same clone + skillpack output generated via `skillpack init --non-interactive --target opencode --target claude --target cursor`. Produces `.opencode/agents/fd-find.md` containing the skillpack-verified invocation + flag list.
- **Task**: four questions phrased to match the skill's `when_to_use` ("find files by name / search for files matching a pattern / list files by extension"):
  > Q1. Find all `.rs` files excluding `target/`. Show the exact command, then run it and paste the first 5 lines.
  > Q2. Make the search case-sensitive. Find files matching "README". Run it; first 3 lines.
  > Q3. Disable fd's gitignore-respecting behavior so ignored files show up. Why does your command reveal them?
  > Q4. Find one file matching `*.rs` and run `wc -l` on it in a single fd command, using fd's exec-per-result flag.
- Same model, same questions, same `--print-logs` capture in both runs.

## Measured delta

| Metric | A — no skillpack | B — with skillpack | delta |
|---|---|---|---|
| Agent step rounds (`step_finish` events) | 20 | 5 | -75% |
| Text answers emitted | 20 | 5 | -75% |
| Final token total | 38,134 | 22,248 | -42% |
| Wall clock | 130 s | 27 s | -79% |

All four final answers are correct in both conditions (verified against the README and `fd --help`). The delta is **efficiency and fewer detours**, not "B solved and A didn't."

## Where the difference showed up

### Q4 — the `-x` exec flag. No retries vs. retries.
- **A**: agent tried `--max-results 1` together with `-x wc -l`, hit fd's incompatible error, re-reasoned, narrowed the glob to a unique filename (`cli.rs`), re-ran twice. **4 steps spent on Q4 alone.**
- **B**: agent invoked `fd -e rs --exclude target -x wc -l` directly and noted proactively: "`--max-results` cannot combine with `-x`/`-X` (fd errors). Pipe instead: `... | head -1`." One step.

### Q3 — disabling gitignore. Less debugging, more direct.
- **A**: agent walked through repo observation, re-checked `.gitignore`, re-checked `.git/info/exclude`, reasoned twice about whether `.codegraph` is hidden vs. ignored, eventually produced `fd -u -t f` plus a long explanation. **>8 steps on Q3.**
- **B**: `fd --no-ignore --hidden --exclude target --exclude .git` in one pass, with a short evidence-style output sample. One step. (Slight inaccuracy: B claimed `Cargo.lock` is gitignored — it is not in fd's repo; A correctly identified `.codegraph/` as ignored via `.git/info/exclude`.)

### Q1 + Q2 — straightforward search. Same behavior.
- Both conditions produced correct commands on the first try (`fd -e rs --exclude target` / `fd -s README`), ran them, and pasted matching output. **No delta.**
- The skillpack SKILL.md's flag list ("verified flags only") did not cost the agent anything here; the skill surfaced `-e`, `-E`, `-s`, `--exclude` immediately without a `--help` run.

## What the skill actually provided the agent

OpenCode discovers `.opencode/agents/fd-find.md` (one of skillpack's 5 distribution targets). When invoked via `opencode run --agent fd-find`, that file becomes the **agent's identity system prompt** — its description, mode, and full SKILL.md body. The body contains:
- An invocation template (`fd <pattern>`).
- A list of 82 flags **verified** by `skillpack verify` against the real `fd --help` — not compiled from prose examples in the help text.
- A footguns section that explicitly tells the agent "if you're unsure, run skillpack verify."

This means the agent in Condition B started Q1-Q4 already knowing:
1. `fd` is the binary it can run (not `fd-find`, which is the crate name).
2. The set of flags that are confirmed-current in this repository's build, not clap help-text prose.
3. That this file is *authoritative* and *self-verifying*.
The generated skill surfaced the verified `-x` / `--exec` mapping directly, which plausibly removed the help-search and recovery detour observed in the baseline run. The file doesn't waste the agent's attention on `-tf` or `-mount` (prose examples our v0.8.1 fix now filters out). A single run cannot prove per-flag causality — the agent may have known fd already — but the baseline recovery steps are visible in the transcript.

## What it cost to produce

```sh
# In the fd clone:
cargo build --release                # skillpack probes target/release/<bin>
cat > skillpack.toml <<'TOML'
[skill]
name = "fd-find"
one_line_description = "A simple, fast and user-friendly alternative to find"
when_to_use_phrases = ["find files by name", "search for files matching a pattern", "list files by extension"]
invocation_command = "fd <pattern>"
license = "MIT OR Apache-2.0"
TOML
skillpack init --non-interactive --target opencode --target claude --target cursor   # <1s
skillpack verify                                                                      # 6/6 ✓
```

Output: `.opencode/agents/fd-find.md`, `skills/fd-find/SKILL.md`, `.cursor/rules/fd-find.mdc`, `.claude-plugin/{marketplace,plugin}.json`. No hand-written docs.

## What this demo did NOT show (honest limitations)

1. **Condition B was less accurate on Q3** — it claimed `Cargo.lock` is gitignored in fd, which it is not. Condition A correctly identified `.codegraph/` as ignored via `.git/info/exclude`. The SKILL.md doesn't carry a complete ignore-behavior model; it carries the flag list. Agents that need runtime semantics still benefit from running the tool itself.
2. **OpenCode's `--agent fd-find` requires the user to know the agent name.** A "natural" user prompt ("use fd to find X") does not auto-invoke the skill — the user must `--agent fd-find` or mention the agent appropriately. This is an OpenCode affordance, not a skillpack limitation; Claude Code's `--plugin-dir` has the same property.
3. **Both conditions reached the correct final answers.** skillpack does not add knowledge the agent couldn't eventually reach; it removes detours. Demonstrating questions where Condition A would have produced a *wrong* answer requires more adversarial prompting (Q4 from the earlier completion-based round 2 did this — the README-only agent asserted `-tf` is reliable, which is wrong).
4. **Experiment confound: B also changed the agent wrapper, not just the skill body.** Condition B is invoked via `opencode run --agent fd-find`, giving it a specialized agent identity and system-prompt context; Condition A runs as a general agent in the same repo. This is fair for demonstrating the OpenCode artifact skillpack generates, but the conclusion should be phrased precisely: *when the generated OpenCode agent is explicitly selected, it reduced execution overhead on this task suite.* A stronger follow-up would hold the agent wrapper constant in both conditions and vary only whether it contains the generated/verified skill body — that would isolate the informational value of skillpack's output from the benefit of routing into a purpose-built subagent.

## Reproducing

Captured evidence in this repo under `docs/demo/transcripts/`:
- Condition A: [condition-a-no-skillpack.json](demo/transcripts/condition-a-no-skillpack.json) (20 step events), [logs](demo/transcripts/condition-a-logs.txt)
- Condition B: [condition-b-with-skillpack.json](demo/transcripts/condition-b-with-skillpack.json) (5 step events), [logs](demo/transcripts/condition-b-logs.txt)
- Questions: [questions.txt](demo/transcripts/questions.txt)

The transcripts were written then committed; you can re-run the comparison from a clean checkout with:

```sh
# From your skillpack clone root:
QUESTIONS="$(pwd)/docs/demo/transcripts/questions.txt"

# Condition A (no skillpack)
cd /tmp && git clone --depth=1 https://github.com/sharkdp/fd.git fd-no-skillpack
cd fd-no-skillpack
opencode run "$(cat "$QUESTIONS")" --format json --print-logs 2>a-logs.txt | tee a.json

# Condition B (with skillpack)
cd /tmp && git clone --depth=1 https://github.com/sharkdp/fd.git fd-with-skillpack
cd fd-with-skillpack && cargo build --release
# ... seed skillpack.toml + skillpack init --target opencode (see above)
cd /tmp/fd-with-skillpack
opencode run --agent fd-find "$(cat "$QUESTIONS")" --format json --print-logs 2>b-logs.txt | tee b.json
```
