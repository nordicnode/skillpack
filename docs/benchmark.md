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

`opencode run` needs a working model provider. The harness uses opencode's
**configured default model** unless you pin one with `--model` — so it works
with any backend: the opencode gateway, a provider key (e.g. `opencode auth
login` with Anthropic/OpenAI/OpenRouter/Gemini), or a local proxy (the runs
below used a `9router` proxy serving an NVIDIA NIM `glm-5.2` model). Pin a
specific model for cross-machine reproducibility:

```sh
scripts/benchmark/run.sh --model 9router/nvidia/z-ai/glm-5.2 --runs 3
```

## Results (2 runs per condition, committed in `scripts/benchmark/results/`)

- **Model**: `nvidia/z-ai/glm-5.2` via a local `9router` proxy (NVIDIA NIM backend)
- **opencode** 1.18.18, **fd** @ `ee20f42` (v10.4.2), **skillpack** 0.11.3

| metric (median) | a-plain | b-skillpack | delta |
|---|---|---|---|
| agent step rounds | 12.5 | 8.5 | **−32%** |
| wall clock (s) | 157.8 | 87.8 | **−44%** |
| tokens | 11,875 | 11,198 | −6% |
| correct (evidence-scored) | 3.5/4 | **4/4** | +14% |

Per-run detail:

| run | rounds | wall s | correct |
|---|---|---|---|
| a-plain-r1 | 10 | 136.6 | 3/4 (skipped Q3) |
| a-plain-r2 | 15 | 179.0 | 4/4 |
| b-skillpack-r1 | 9 | 82.5 | 4/4 |
| b-skillpack-r2 | 8 | 93.1 | 4/4 |

**Efficiency and correctness both improved.** The one question missed across
eight runs was the baseline's r1 *skipping Q3* (it never ran an
ignore-disable command at all — it went straight to Q4's retries). Both
skillpack runs answered all four correctly.

### What the transcript shows

- The skillpack agent used the skill's short flags **first try** (`fd -e rs -E
target`, `fd -s README`, `fd -I README`, `fd --no-ignore-vcs ...`) and
**never ran `fd --help`** across either run. The baseline agent used long
flag names and ran `fd --help`/flag greps **five times** across its two runs
— including the Q4 `--max-results`-with-`-x` incompatibility, which fd
rejects; the skillpack agent's Q4 was a direct `-x wc -l` call.
- The baseline's r2 *modified the repo* to answer Q3 (it created
`target/demo.txt` to prove ignored files exist). The skillpack agent answered
Q3 read-only in both runs.
- Was the guidance actually loaded? opencode auto-loads `AGENTS.md` from the
project root (documented behavior), and two fingerprints confirm it here:
(1) **token fingerprint** — condition B's system prompt is consistently
~840–1,100 tokens larger than A's (8,728 / 8,989 vs 7,886 first-step input
+cache), matching the ~2.7 KB generated `AGENTS.md`; (2) **behavioral
fingerprint** — B's first action was `fd --version` (the skill says to
verify the tool is installed) and it used the skill's exact flag spellings.
A direct in-context probe was attempted but opencode's local server wedged
after the runs completed (environment issue, unrelated to skillpack).

### Honest caveats

- **N=2 per condition.** LLM runs are stochastic; treat the delta as
indicative, not statistically significant. The efficiency win is consistent
directionally across both runs (A: 10/15 steps, B: 9/8).
- **One model, one repo, one task suite.** GLM-5.2 is a strong model; a
weaker model or a more adversarial suite would likely show a larger delta.
- **Evidence scoring is a proxy** for human review — it rewards the right
flags and output, not prose quality.
- The **old demo's numbers differ** (−75% rounds) because it used a
different model and the `--agent` confound; the harness fixes the confound
and pins the model, so future runs are comparable to these.

### Reproducing

```sh
SKILLPACK_BENCH_BIN=/path/to/skillpack scripts/benchmark/run.sh --runs 2
python3 scripts/benchmark/analyze.py scripts/benchmark/results
```

The harness clones + builds the fd cache once, then reuses it; `--fresh`
re-clones and rebuilds. Every run's transcript, wall clock, logs, and the
generated `AGENTS.md` are written to `scripts/benchmark/results/` for audit.
