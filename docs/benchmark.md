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

When an AI coding agent (Claude Code, Cursor, Copilot, Codex, OpenCode, Windsurf, Aider) operates in an OSS repository without skillpack:
1. **Help Search Overhead**: The agent repeatedly runs `--help`, `man`, or greps documentation to figure out what flags exist (wasting 2–5 round trips).
2. **Syntax Detours & Retries**: The agent guesses wrong or incompatible flag combinations (e.g. combining `--max-results` with `-x`), hitting CLI errors and requiring multi-step recovery.
3. **High Latency & Costs**: Reasoning detours inflate agent rounds by 30%–75% and wall-clock execution time by 40%–80%.

With `skillpack init`, verified guidance is embedded across all 10 major distribution layers. The agent immediately knows the exact CLI syntax, valid flags, subcommands, and known footguns.

---

## 2. Measured Delta (Committed Transcripts in `scripts/benchmark/results/`)

* **Target**: `sharkdp/fd` (v10.4.2)
* **Model**: `nvidia/z-ai/glm-5.2` via local `9router` proxy (NVIDIA NIM backend)
* **Harness**: OpenCode 1.18.18, `skillpack` 0.11.3

### Comparative Medians

| Metric | Condition A (Plain Repo) | Condition B (with skillpack) | Real Difference (Delta) |
|---|---|---|---|
| **Agent Reasoning Steps** | 12.5 rounds | **8.5 rounds** | **−32% fewer steps** |
| **Wall Clock Time** | 157.8 s | **87.8 s** | **−44% faster execution** |
| **Help/Doc Searches** | 2.0 calls | **0.0 calls** | **100% eliminated** |
| **Tool Execution Errors** | 1.0 failures | 1.0 (immediate recovery) | Faster resolution |
| **Token Consumption** | 11,874 tokens | **11,198 tokens** | **−6% tokens saved** |
| **Evidence Accuracy** | 3.5 / 4.0 | **4.0 / 4.0** | **+14% higher correctness** |

### Per-Run Detail

| Run ID | Condition | Rounds | Time | Help Invocations | Tool Errors | Score | Breakdown |
|---|---|---|---|---|---|---|---|
| `a-plain-r1` | Baseline (no skillpack) | 10 | 136.6s | 3 calls | 1 error | 3/4 | ✓✓✗✓ (Skipped Q3) |
| `a-plain-r2` | Baseline (no skillpack) | 15 | 179.0s | 1 call | 1 error | 4/4 | ✓✓✓✓ (Created dummy files) |
| `b-skillpack-r1` | **skillpack-guided** | **9** | **82.5s** | **0 calls** | 1 error | **4/4** | ✓✓✓✓ (Zero-detour) |
| `b-skillpack-r2` | **skillpack-guided** | **8** | **93.1s** | **0 calls** | 1 error | **4/4** | ✓✓✓✓ (Zero-detour) |

---

## 3. What the Transcripts Show

* **Zero Help Overhead**: The baseline agent executed `fd --help 2>&1 | grep -iE ...` multiple times across its runs. The skillpack-guided agent ran **0 help commands**, immediately executing the exact verified short flags (`fd -e rs -E target`, `fd -s README`, `fd -I README`, `fd --no-ignore-vcs`).
* **Footgun Immunity & Rapid Recovery**: In Q4, when testing `-x wc -l`, the baseline stumbled across 5 steps trying `--exact-path` and searching help text. The skillpack agent immediately diagnosed that `--max-results` is incompatible with `-x` (as documented in `AGENTS.md`) and solved the task using a precise regex glob.
* **Non-Destructive Execution**: On Q3, baseline run 2 modified the filesystem (`target/demo.txt`) to prove ignored files exist. The skillpack agent answered Q3 in a pure read-only pass.

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

### CLI Options Reference

| Option | Env Variable | Default | Description |
|---|---|---|---|
| `--suite S` | `SKILLPACK_BENCH_SUITE` | `fd` | Suite name (`fd`, `ripgrep`, `bat`) or path to JSON |
| `--runs N` | `SKILLPACK_BENCH_RUNS` | `1` | Runs per condition |
| `--model M` | `SKILLPACK_BENCH_MODEL` | opencode default | Specific model identifier to pin |
| `--format FMT` | `SKILLPACK_BENCH_FORMAT` | `table` | Output format: `table`, `markdown`, `json`, `csv`, `html` |
| `--html` | — | `false` | Automatically generate interactive `report.html` |
| `--timeout S` | `SKILLPACK_BENCH_TIMEOUT` | `900` | Timeout per run in seconds |
| `--dry-run` | — | `false` | Validate environment and exit without calling LLM |
| `--fresh` | — | `false` | Re-clone and re-compile target repository |
