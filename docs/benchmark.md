# skillpack benchmark (OpenCode)

A repeatable, honest A/B harness for measuring what skillpack's generated
guidance does to a real coding agent. One command:

```sh
scripts/benchmark/run.sh --runs 3        # then:
python3 scripts/benchmark/analyze.py scripts/benchmark/results
```

## The comparison

Two conditions on the **same repo** (`sharkdp/fd` by default), **same default
agent**, **same model**, **same questions**:

| condition | setup |
|---|---|
| `a-plain` | a fresh clone of the target repo — no guidance files |
| `b-skillpack` | the same clone + `skillpack init --auto --target all --force` |

The agent wrapper is held constant — **no `--agent` flag in either condition**.
opencode loads `AGENTS.md` from the repo root automatically, so condition B's
only advantage is the skillpack-generated guidance. This removes the confound
documented in the original [`docs/agent-demo.md`](agent-demo.md), which routed
condition B into a purpose-built subagent (`--agent fd-find`) while A ran as a
general agent.

`--auto` generates the pack from the repo itself (description from the README,
invocation from the built binary, author from git) — it exercises the tool's
primary user journey end-to-end, and the resulting `AGENTS.md` carries the
verified flag list (e.g. fd's `-x/--exec`, `-u/--no-ignore`, `-H/--hidden`
mappings, checked against the repo's own `--help`).

## What it measures

Per run (from the opencode JSONL event stream):

- **rounds** — `step_finish` events (agent reasoning/action steps)
- **tokens / cost** — cumulative token total and cost from `step_finish`
- **wall s** — process wall clock (`date +%s%N` around the run, not the
  event timestamps)
- **correct** — evidence-scored from the transcript: each question has a
  command rule (e.g. Q4 requires `fd ... -x wc -l`) and an output rule; a
  question scores 1.0 only when both match. This is a proxy for human
  review — it measures "used the right flags and got the right output".

With `--runs N`, `analyze.py` reports medians per condition and a delta table
vs the baseline. Single-run output degrades gracefully to plain per-run rows.

## The task suite (`scripts/benchmark/questions.txt`)

Four fd tasks, phrased to match the skill's triggers:

1. Find all `.rs` files excluding `target/` — the `-e rs` + `--exclude target` combination.
2. Case-sensitive search for "README" — the `-s` flag.
3. Disable gitignore-respecting behavior so ignored files show up — `-u`/`--no-ignore` + `-H`/`--hidden`, plus a why-explanation.
4. Run `wc -l` on one `.rs` file in a single fd command — the exec-per-result flag `-x wc -l` (not `--max-results`, which fd rejects with `-x`).

These are the exact tasks from the original demo — Q4 is the one that cost the
baseline agent four retries (fd's `--max-results`/`-x` incompatibility) while
the skillpack condition answered in one step.

## Running it

```sh
# prerequisites: git, cargo, opencode (authenticated), python3, and skillpack
# on PATH (or SKILLPACK_BENCH_BIN=/path/to/skillpack)

scripts/benchmark/run.sh --runs 2            # 2 runs per condition
scripts/benchmark/run.sh --model anthropic/claude-sonnet-4-5 --runs 3
scripts/benchmark/run.sh --fresh             # re-clone + rebuild the target
python3 scripts/benchmark/analyze.py scripts/benchmark/results
```

The target repo is cached under `<work>/fd-src` (default `/tmp/skillpack-bench`)
and built once with `cargo build --release` — that's the only expensive
setup step, and it's reused across runs. Each condition is a fresh clone of
the cache, so condition A never sees `target/` and condition B gets the built
binary copied in for skillpack to introspect. Everything lands in
`scripts/benchmark/results/`:

```
meta.txt                      model / opencode / skillpack versions, fd rev
a-plain-r1.json/.logs/.wall   condition A run 1
b-skillpack-r1.json/.logs/.wall + b-skillpack-r1.agents.md (the generated pack)
```

## Prerequisite: an LLM backend

`opencode run` needs a working model provider. The harness pins
`opencode/claude-haiku-4-5` by default (fast, cheap, repeatable); override
with `--model`. The opencode gateway requires a payment method on the
account, or you can add a provider key (e.g. `opencode auth login` with an
Anthropic/OpenAI/OpenRouter/Gemini key) and pass `--model provider/model`.

## Results

Pending a live run — the harness, analyzer, and questions are committed and
validated (the analyzer reproduces the original demo's numbers exactly:
**−75% rounds, −42% tokens** on the archived transcripts). Commit results as
`scripts/benchmark/results/` when a run completes.
