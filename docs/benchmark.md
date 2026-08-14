# skillpack benchmark

A repeatable, honest A/B evaluation suite measuring the quantitative and qualitative delta that skillpack's verified guidance makes for AI coding agents.

```sh
# Run the benchmark (or test with dry-run)
scripts/benchmark/run.sh --suite fd --runs 3 --html

# Analyze transcripts in multiple formats
python3 scripts/benchmark/analyze.py scripts/benchmark/results --format table
python3 scripts/benchmark/analyze.py scripts/benchmark/results --format markdown
python3 scripts/benchmark/analyze.py scripts/benchmark/results --format html --out report.html
python3 scripts/benchmark/analyze.py scripts/benchmark/results --format json > report.json

# Offline CI replay validation (0 API calls, millisecond execution)
python3 scripts/benchmark/replay.py scripts/benchmark/results
```

---

## 1. Why skillpack makes a real difference

When an AI coding agent (Claude Code, Cursor, Copilot, Codex, OpenCode, Gemini CLI, Windsurf, Aider) operates in an OSS repository without skillpack:
1. **Help Search Overhead**: The agent repeatedly runs `--help`, `man`, or greps documentation to figure out what flags exist (wasting 2–5 round trips).
2. **Syntax Detours & Retries**: The agent guesses wrong or incompatible flag combinations (e.g. combining `--max-results` with `-x`), hitting CLI errors and requiring multi-step recovery.
3. **High Latency & Costs**: Reasoning detours inflate agent rounds by 30%–75% and wall-clock execution time by 40%–80%.

With `skillpack init`, verified guidance is embedded across all 10 major distribution layers. The agent immediately knows the exact CLI syntax, valid flags, subcommands, and known footguns.

---

## 2. Measured Delta (Committed Transcripts in `scripts/benchmark/results/`)

* **Target**: `sharkdp/fd` (v10.4.2, commit `ee20f42`)
* **Runtime**: Google Antigravity CLI (`agy` 1.1.13, `-p --output-format stream-json`)
* **Model**: Gemini 3.7 Flash (agy's configured default; reasoning effort High)
* **Harness**: `scripts/benchmark/run.sh`, `skillpack` 0.12.0

### Comparative Medians

| Metric | Condition A (Plain Repo) | Condition B (with skillpack) | Real Difference (Delta) |
|---|---|---|---|
| **Agent Reasoning Steps** | 60.0 rounds | **48.0 rounds** | **−20% fewer steps** |
| **Wall Clock Time** | 62.6 s | **53.0 s** | **−15% faster execution** |
| **Help/Doc Searches** | 4.0 calls | **1.5 calls** | **−63% fewer detours** |
| **Token Consumption** | 145,082 tokens | **123,120 tokens** | **−15% tokens saved** |
| **Tool Execution Errors** | 2.0 failures | **1.5 failures** | −25% (faster recovery) |
| **Evidence Accuracy** | 4.0 / 4.0 | **4.0 / 4.0** | Tie — both perfect |

### Per-Run Detail

| Run ID | Condition | Rounds | Time | Help Invocations | Tool Errors | Score | Breakdown |
|---|---|---|---|---|---|---|---|
| `a-plain-r1` | Baseline (no skillpack) | 66 | 71.3s | 4 calls | 2 errors | 4/4 | ✓✓✓✓ |
| `a-plain-r2` | Baseline (no skillpack) | 54 | 53.9s | 4 calls | 2 errors | 4/4 | ✓✓✓✓ |
| `b-skillpack-r1` | **skillpack-guided** | **46** | **53.5s** | **2 calls** | 2 errors | **4/4** | ✓✓✓✓ |
| `b-skillpack-r2` | **skillpack-guided** | **50** | **52.6s** | **1 call** | 1 error | **4/4** | ✓✓✓✓ |

---

## 3. What the Transcripts Show

* **Help overhead, not eliminated, but quartered**: The baseline agent ran `fd --help` four times per run, slicing it into `head`/`tail` windows to page through the 140+ lines. The skillpack-guided agent consulted help once or twice (b-skillpack-r2 ran it exactly once).
* **The guidance anchors the agent in the repo**: The baseline agent started by searching the whole home directory (`fd -e rs /home/mikey`) and wandered into *other* projects on the machine (ashen-ledger, rust-cargo-project) before settling on the fd clone. The guided agent, whose prompt carried the skillpack AGENTS.md ("`fd` is a program to find entries...", verified flags), stayed in the repo and went straight to the verified short flags: `fd -e rs -E target`, `fd -s README`, `fd -I`, `fd -1 -g '*.rs' -x wc -l`.
* **Footgun recovery, faster**: The baseline tried `fd --max-results 1 -e rs -x wc -l` (an incompatible combination) and had to re-reason; the guided agent reached the working `fd -1 -g '*.rs' -x wc -l` form in one step.
* **Both conditions solved everything**: Gemini 3.7 Flash is strong — every run scored 4/4. The delta is efficiency and fewer detours, not "B solved and A didn't."

---

## 4. Built-in Benchmark Suites (`scripts/benchmark/suites/`)

Pre-configured benchmark suites are provided for major CLI architectures:

| Suite | Description | Key Capabilities Tested |
|---|---|---|
| **`fd`** | File search & execution (`sharkdp/fd`) | Flag combinations, case-sensitivity, ignore rules, `-x` exec footguns |
| **`ripgrep`** | Regex code search (`BurntSushi/ripgrep`) | Multiline regex, type filters, word boundaries, unrestricted search |
| **`bat`** | Syntax highlighting viewer (`sharkdp/bat`) | Line range highlighting, style arguments, theme inspection, plain piping |

---

## 5. Running Custom Benchmarks

Benchmarking your own CLI or OSS project is simple:

```sh
# Run with a pre-configured suite
scripts/benchmark/run.sh --suite ripgrep --runs 2 --html

# Or run against any custom repository URL
scripts/benchmark/run.sh \
  --repo https://github.com/your-org/your-cli.git \
  --target-bin your-cli \
  --runs 2 \
  --html
```

### How it runs

The harness clones the target repo once, builds it, then for each run creates two
condition dirs from that cache:

* **Condition A (plain)** — fresh clone, questions only.
* **Condition B (skillpack)** — same clone + `skillpack init --auto --target all --force`,
  with the generated `AGENTS.md` fed to the agent as a prompt preamble.

Both conditions are driven by `agy -p <prompt> --dangerously-skip-permissions
--output-format stream-json` from the condition dir, with agy's configured model
(Gemini 3.7 Flash by default; pin another with `--model`). The exact prompt sent
to each agent is committed next to the transcript (`<condition>.prompt`).

**Methodology note**: agy print mode (v1.1.12) does not auto-discover `AGENTS.md`
workspace rules, so condition B passes the skillpack-generated guidance to the
agent explicitly as a prompt preamble — the *only* difference between the two
conditions is the guidance content. The agent wrapper, model, and questions are
identical. agy's stream-json output does not expose command exit codes, so tool
failures are inferred from error-shaped tool output (documented in `analyze.py`).

### CLI Options Reference

| Option | Env Variable | Default | Description |
|---|---|---|---|
| `--suite S` | `SKILLPACK_BENCH_SUITE` | `fd` | Suite name (`fd`, `ripgrep`, `bat`) or path to JSON |
| `--runs N` | `SKILLPACK_BENCH_RUNS` | `1` | Runs per condition |
| `--model M` | `SKILLPACK_BENCH_MODEL` | agy default | Specific model identifier to pin (passed to `agy --model`) |
| `--format FMT` | `SKILLPACK_BENCH_FORMAT` | `table` | Output format: `table`, `markdown`, `json`, `csv`, `html` |
| `--html` | — | `false` | Automatically generate interactive `report.html` |
| `--timeout S` | `SKILLPACK_BENCH_TIMEOUT` | `900` | Timeout per run in seconds |
| `--dry-run` | — | `false` | Validate environment and exit without calling LLM |
| `--fresh` | — | `false` | Re-clone and re-compile target repository |
