# skillpack demo: real-world agent delta (measured)

> How we showed, not asserted, the difference skillpack makes to a real AI agent.

On a four-task real-world fd CLI exercise driven through **Google Antigravity (`agy`)**
with **Gemini 3.7 Flash**, the skillpack-guided agent finished the same correct
task set in **46 rather than 66** agent steps, **53.5 s rather than 71.3 s**,
**−25% fewer tokens**, and **half the `--help` calls** of the plain clone.
This is one controlled run per condition (run 1 of the committed suite), not a
general benchmark; the guidance reduced detours and — notably — kept the agent
anchored in the right repository.

## Demo setup

- **Repo**: [fd](https://github.com/sharkdp/fd) (`fd-find` crate, `fd` binary). Real popular Rust CLI.
- **Agent runtime**: Google Antigravity CLI (`agy` 1.1.13), non-interactive via `agy -p --output-format stream-json`.
- **Model**: Gemini 3.7 Flash (agy's configured default; reasoning effort High).
- **A/B clones** (built by `scripts/benchmark/run.sh` from one cached clone):
  - **Condition A — without skillpack** at `/tmp/skillpack-bench/run1/a-plain`: fresh clone of fd, no guidance files, questions only.
  - **Condition B — with skillpack** at `/tmp/skillpack-bench/run1/b-skillpack`: same clone + `skillpack init --auto --target all --force`. The generated `AGENTS.md` is fed to the agent as a prompt preamble (agy print mode does not auto-discover AGENTS.md rules — see the methodology note in [`docs/benchmark.md`](benchmark.md)).
- **Task**: four questions phrased to match the skill's `when_to_use` ("find files by name / search for files matching a pattern / list files by extension"):
  > Q1. Find all `.rs` files excluding `target/`. Show the exact command, then run it and paste the first 5 lines.
  > Q2. Make the search case-sensitive. Find files matching "README". Run it; first 3 lines.
  > Q3. Disable fd's gitignore-respecting behavior so ignored files show up. Why does your command reveal them?
  > Q4. Find one file matching `*.rs` and run `wc -l` on it in a single fd command, using fd's exec-per-result flag.
- Same agent, same model, same questions in both runs. Transcripts committed under `scripts/benchmark/results/`.

## Measured delta

| Metric | A — no skillpack | B — with skillpack | delta |
|---|---|---|---|
| Agent step rounds (agy stream steps) | 66 | 46 | -30% |
| `--help` invocations | 4 | 2 | -50% |
| Final token total | 155,329 | 116,920 | -25% |
| Wall clock | 71.3 s | 53.5 s | -25% |

Both conditions reached 4/4 by the suite's evidence scoring. The delta is
**efficiency, fewer detours, and staying in the right repo** — not "B solved and
A didn't."

## Where the difference showed up

### Q1 + Q2 — the baseline searched the wrong machine. The guided agent stayed in the repo.
- **A**: after a `pwd && which fd` probe, the agent ran `fd -e rs /home/mikey`
  and `fd -s README /home/mikey/Desktop/dev` — searching the *whole home
  directory* — and eventually `cd`'d into **a different project on the machine**
  (`/home/mikey/Desktop/dev/ashen-ledger`) and ran the fd commands there. Its
  final answers quoted that project's files (`crates/ashen_cli/src/main.rs`,
  `DwarfMind/README.md`, …) as if they were fd's. The flag syntax was right;
  the repository was not.
- **B**: went straight to `fd -e rs -E target/` and `fd -s README` in the fd
  clone and pasted fd's real files (`tests/provider_matrix.rs`, `README.md`,
  `changelog/README.md`, `docs/README.md`).

### Q3 — disabling gitignore. One command vs. a detour.
- **A**: `fd -I target` — treated `target` as a search pattern and explained the
  flag correctly, but never demonstrated a genuinely ignored file.
- **B**: `fd -I` in one pass, plus `fd -I Cargo.lock` to show an ignored entry.
  (Slight inaccuracy: B claimed `Cargo.lock` is in fd's `.gitignore` — it is
  not; `fd Cargo.lock` finds it even without `-I`. The flag list is not a
  complete model of a repo's ignore rules.)

### Q4 — the `-x` exec flag. Fewer retries with guidance.
- **A**: probed `fd --max-results 1 -e rs -x wc -l` (an incompatible
  combination), recovered, tried `fd -g 'main.rs' -x wc -l` and
  `fd -g 'app.rs' -x wc -l`, and finally settled on `fd -g '*.rs' -x wc -l`.
- **B**: reached the working `-x` form quickly and targeted a single file with
  `fd -g 'provider_matrix.rs' -x wc -l`, naming `-x`/`--exec` as the
  exec-per-result flag.

### Help overhead — quartered.
- **A**: paged through `fd --help` four times (`fd --help | head -n 45`,
  `| head -n 90 | tail -n 50`, `| head -n 140 | tail -n 50`, …).
- **B**: consulted `fd --help` once.

## What the guidance actually provided the agent

Condition B's prompt carried the skillpack-generated `AGENTS.md` for fd-find
(the `--target all` run produces it plus Claude/Cursor/Codex/Copilot/Gemini
files). The body contains:

- An invocation template (`fd <pattern>`).
- A list of **verified** flags (`skillpack verify` checks them against the real
  `fd --help`) — not prose examples from the help text.
- A footguns section that explicitly tells the agent "if you're unsure, run
  skillpack verify."

The agent in Condition B started Q1–Q4 already knowing `fd` is the binary to
invoke (not `fd-find`, the crate name), which flags are confirmed-current for
this build, and the `-x`/`--exec` mapping — which plausibly removed the
help-search detour and the wrong-repo wandering observed in the baseline. A
single run cannot prove per-flag causality, but the baseline's detours are
visible in the transcript.

## What it cost to produce

```sh
# The harness does all of this per condition:
skillpack init --auto --target all --force   # in the fd clone, <1s
skillpack verify                              # 100/100
```

Output: `AGENTS.md`, `.claude-plugin/`, `skills/fd-find/SKILL.md`,
`.cursor/rules/fd-find.mdc`, `.opencode/agents/fd-find.md`,
`.github/copilot-instructions.md`, `GEMINI.md`, `.windsurf/`, `CONVENTIONS.md`.
No hand-written docs.

## What this demo did NOT show (honest limitations)

1. **The baseline's Q1/Q2 answers quoted the wrong repository.** The evidence
   scoring (flag syntax + output shape) passes both conditions, but A presented
   another project's files as the answer. This is the strongest argument for the
   guidance — and the most honest one: without it, the agent didn't even stay in
   the repo.
2. **Condition B was inaccurate on one detail** — it claimed `Cargo.lock` is
   gitignored in fd, which it is not. The guidance carries the flag list, not a
   complete model of a repo's ignore rules.
3. **Both conditions reached a 4/4 score.** Gemini 3.7 Flash is strong;
   skillpack removes detours rather than adding knowledge the model can't
   eventually reach.
4. **Methodology caveat**: agy print mode (v1.1.12) does not auto-discover
   `AGENTS.md` rules, so the harness feeds the generated guidance to the agent
   as a prompt preamble — the only difference between conditions is still the
   guidance content. See the methodology note in `docs/benchmark.md`.

## Reproducing

Captured evidence is committed in `scripts/benchmark/results/`:

- Condition A: `a-plain-r1.json` (+ `.logs`, `.prompt`, `.wall`)
- Condition B: `b-skillpack-r1.json` (+ `.logs`, `.prompt`, `.wall`,
  `b-skillpack-r1.agents.md`)

Re-run the comparison with the benchmark harness:

```sh
# From your skillpack clone root:
scripts/benchmark/run.sh --suite fd --runs 1 --out /tmp/demo-results
# Or the full 2-run suite committed in this repo:
scripts/benchmark/run.sh --suite fd --runs 2
```
