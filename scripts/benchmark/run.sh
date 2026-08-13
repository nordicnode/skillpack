#!/usr/bin/env bash
# skillpack x OpenCode A/B benchmark harness.
#
# Measures the delta skillpack's generated guidance makes for a coding agent.
# Two conditions on the SAME repo, SAME default agent, SAME model, SAME
# questions:
#
#   a-plain      — a fresh clone of the target repo (no guidance files)
#   b-skillpack  — the same clone + `skillpack init --auto --target all --force`
#
# The agent wrapper is held constant (no `--agent` flag in either condition):
# opencode loads AGENTS.md from the repo root automatically, so condition B's
# only advantage is the skillpack-generated guidance itself.
#
# Usage:
#   scripts/benchmark/run.sh [--suite fd|ripgrep|bat|<path>] [--runs N] [--model M]
#                            [--out DIR] [--work DIR] [--repo URL] [--target-bin BIN]
#                            [--branch B] [--timeout SECS] [--format FMT] [--html]
#                            [--dry-run] [--fresh] [--no-build]
#
# Env overrides (same names):
#   SKILLPACK_BENCH_SUITE, _RUNS, _MODEL, _OUT, _WORK, _QUESTIONS, _BIN,
#   _REPO, _TARGET_BIN, _BRANCH, _TIMEOUT, _FORMAT
#
# Output:
#   <out>/a-plain-r<N>.json/.logs/.wall
#   <out>/b-skillpack-r<N>.json/.logs/.wall
#   <out>/b-skillpack-r<N>.agents.md
#   <out>/meta.txt
#   <out>/report.html (when --html is passed or enabled)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
SUITES_DIR="$HERE/suites"

# Smart default for skillpack binary
default_skillpack_bin() {
  if [[ -n "${SKILLPACK_BENCH_BIN:-}" ]]; then
    echo "$SKILLPACK_BENCH_BIN"
  elif [[ -x "$ROOT/target/release/skillpack" ]]; then
    echo "$ROOT/target/release/skillpack"
  elif [[ -x "$ROOT/target/debug/skillpack" ]]; then
    echo "$ROOT/target/debug/skillpack"
  else
    echo "skillpack"
  fi
}

SUITE="${SKILLPACK_BENCH_SUITE:-fd}"
MODEL="${SKILLPACK_BENCH_MODEL:-}"
RUNS="${SKILLPACK_BENCH_RUNS:-1}"
QUESTIONS="${SKILLPACK_BENCH_QUESTIONS:-}"
OUT="${SKILLPACK_BENCH_OUT:-$HERE/results}"
WORK="${SKILLPACK_BENCH_WORK:-/tmp/skillpack-bench}"
BIN="$(default_skillpack_bin)"
REPO="${SKILLPACK_BENCH_REPO:-}"
TARGET_BIN="${SKILLPACK_BENCH_TARGET_BIN:-}"
BRANCH="${SKILLPACK_BENCH_BRANCH:-}"
TIMEOUT="${SKILLPACK_BENCH_TIMEOUT:-900}"
FORMAT="${SKILLPACK_BENCH_FORMAT:-table}"
GENERATE_HTML=0
FRESH=0
NO_BUILD=0
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --suite) SUITE="$2"; shift 2 ;;
    --runs) RUNS="$2"; shift 2 ;;
    --model) MODEL="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --work) WORK="$2"; shift 2 ;;
    --repo) REPO="$2"; shift 2 ;;
    --target-bin) TARGET_BIN="$2"; shift 2 ;;
    --branch) BRANCH="$2"; shift 2 ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    --format) FORMAT="$2"; shift 2 ;;
    --html) GENERATE_HTML=1; shift ;;
    --fresh) FRESH=1; shift ;;
    --no-build) NO_BUILD=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) sed -n '1,32p' "$0" | sed 's/^# \?//'; exit 0 ;;
    *) echo "unknown option: $1"; exit 2 ;;
  esac
done

# Resolve suite JSON if available
SUITE_JSON=""
if [[ -f "$SUITE" ]]; then
  SUITE_JSON="$SUITE"
elif [[ -f "$SUITES_DIR/$SUITE.json" ]]; then
  SUITE_JSON="$SUITES_DIR/$SUITE.json"
fi

# Extract defaults from suite JSON if not explicitly provided
if [[ -n "$SUITE_JSON" && -f "$SUITE_JSON" ]]; then
  if [[ -z "$REPO" ]]; then
    REPO="$(python3 -c "import json; print(json.load(open('$SUITE_JSON')).get('repo', ''))" 2>/dev/null || true)"
  fi
  if [[ -z "$BRANCH" ]]; then
    BRANCH="$(python3 -c "import json; print(json.load(open('$SUITE_JSON')).get('branch', 'master'))" 2>/dev/null || true)"
  fi
  if [[ -z "$TARGET_BIN" ]]; then
    TARGET_BIN="$(python3 -c "import json; print(json.load(open('$SUITE_JSON')).get('target_bin', ''))" 2>/dev/null || true)"
  fi
fi

REPO="${REPO:-https://github.com/sharkdp/fd.git}"
BRANCH="${BRANCH:-master}"
if [[ -z "$TARGET_BIN" ]]; then
  TARGET_BIN="$(basename "$REPO" .git)"
  TARGET_BIN="${TARGET_BIN#rust-}"
  TARGET_BIN="${TARGET_BIN#cli-}"
fi

# Questions resolution
if [[ -z "$QUESTIONS" ]]; then
  if [[ -n "$SUITE_JSON" && -f "$SUITE_JSON" ]]; then
    mkdir -p "$WORK"
    QUESTIONS="$WORK/questions_${SUITE}.txt"
    python3 -c "
import json
data = json.load(open('$SUITE_JSON'))
qs = data.get('questions', [])
prompt = 'Four independent tasks. Do all four, one at a time, in order. For each: show the exact command, run it, and paste the relevant output.\n\n'
prompt += '\n\n'.join(qs)
open('$QUESTIONS', 'w').write(prompt)
"
  else
    QUESTIONS="$HERE/questions.txt"
  fi
fi

for tool in git cargo opencode python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: missing required tool: $tool"; exit 1; }
done
if ! command -v "$BIN" >/dev/null 2>&1 && [[ ! -x "$BIN" ]]; then
  echo "error: skillpack binary not found: $BIN (build via \`cargo build --release\` or set SKILLPACK_BENCH_BIN)"; exit 1
fi
[[ -r "$QUESTIONS" ]] || { echo "error: questions file not readable: $QUESTIONS"; exit 1; }
[[ "$RUNS" =~ ^[0-9]+$ ]] && [[ "$RUNS" -ge 1 ]] || { echo "error: --runs must be a positive integer"; exit 1; }

if [[ "$DRY_RUN" == 1 ]]; then
  echo "=== skillpack benchmark [DRY RUN] ==="
  echo "Suite:            $SUITE (${SUITE_JSON:-custom})"
  echo "Skillpack binary: $BIN ($("$BIN" --version 2>/dev/null || echo 'unknown'))"
  echo "Opencode version: $(opencode --version 2>/dev/null | head -1 || echo 'unknown')"
  echo "Target repo:      $REPO (branch: $BRANCH, binary: $TARGET_BIN)"
  echo "Model:            ${MODEL:-<opencode default>}"
  echo "Runs:             $RUNS"
  echo "Timeout per run:  ${TIMEOUT}s"
  echo "Questions file:   $QUESTIONS"
  echo "Output dir:       $OUT"
  echo "Working dir:      $WORK"
  echo "Dry run validation succeeded."
  exit 0
fi

mkdir -p "$WORK" "$OUT"

# --- target repo cache: clone once, build once -----------------------------
REPO_SLUG="$(basename "$REPO" .git)"
SRC="$WORK/$REPO_SLUG-src"
BUILT_BIN="$SRC/target/release/$TARGET_BIN"

if [[ "$FRESH" == 1 ]]; then
  rm -rf "$SRC"
fi
if [[ ! -d "$SRC/.git" ]]; then
  echo "[setup] cloning $REPO -> $SRC"
  git clone -q --depth=1 --branch "$BRANCH" "$REPO" "$SRC"
fi
if [[ "$NO_BUILD" != 1 && ! -x "$BUILT_BIN" ]]; then
  echo "[setup] cargo build --release in $SRC (first run only)"
  ( cd "$SRC" && cargo build --release )
fi

# Fall back to debug build if release not found
if [[ ! -x "$BUILT_BIN" && -x "$SRC/target/debug/$TARGET_BIN" ]]; then
  BUILT_BIN="$SRC/target/debug/$TARGET_BIN"
fi

[[ -x "$BUILT_BIN" ]] || { echo "error: built binary not found: $BUILT_BIN (use --no-build? build first?)"; exit 1; }

# --- metadata ---------------------------------------------------------------
{
  echo "created: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "suite: $SUITE"
  echo "model: $MODEL"
  echo "runs: $RUNS"
  echo "opencode: $(opencode --version 2>/dev/null | head -1 || true)"
  echo "skillpack: $("$BIN" --version 2>/dev/null | head -1 || true)"
  echo "target repo: $REPO @ $(git -C "$SRC" rev-parse --short HEAD 2>/dev/null || true)"
  echo "target build: $(test -x "$BUILT_BIN" && "$BUILT_BIN" --version 2>/dev/null || echo none)"
  echo "questions: $QUESTIONS"
  echo "confound control: same default agent in both conditions (no --agent)"
} | tee "$OUT/meta.txt"

run_condition() {
  local stem="$1" dir="$2" skillpack_on="$3"
  rm -rf "$dir"
  echo "[run] $stem: cloning from cache -> $dir"
  git clone -q "$SRC" "$dir"
  if [[ "$skillpack_on" == 1 ]]; then
    mkdir -p "$dir/target/release"
    cp "$BUILT_BIN" "$dir/target/release/$TARGET_BIN"
    echo "[run] $stem: skillpack init --auto --target all --force"
    ( cd "$dir" && "$BIN" init --auto --target all --force )
    if [[ -f "$dir/AGENTS.md" ]]; then
      cp "$dir/AGENTS.md" "$OUT/$stem.agents.md"
    fi
  fi
  echo "[run] $stem: opencode run (model=${MODEL:-<config default>}) [timeout=${TIMEOUT}s]"
  local start end
  start=$(date +%s%N)
  local model_args=()
  [[ -n "$MODEL" ]] && model_args=(-m "$MODEL")
  
  local runner=()
  if command -v timeout >/dev/null 2>&1; then
    runner=(timeout "$TIMEOUT")
  fi

  if ( cd "$dir" && "${runner[@]}" opencode run "$(cat "$QUESTIONS")" "${model_args[@]}" --pure --format json --print-logs \
        >"$OUT/$stem.json" 2>"$OUT/$stem.logs" ); then
    end=$(date +%s%N)
    awk -v d="$((end - start))" 'BEGIN { printf "%.2f\n", d / 1e9 }' > "$OUT/$stem.wall"
  else
    echo "  !! opencode run failed (or timed out) for $stem — see $OUT/$stem.logs"
    return 1
  fi
}

ok=1
for i in $(seq 1 "$RUNS"); do
  run_condition "a-plain-r$i" "$WORK/run$i/a-plain" 0 || ok=0
  run_condition "b-skillpack-r$i" "$WORK/run$i/b-skillpack" 1 || ok=0
done

echo
ANALYZE_ARGS=("$OUT" "--format" "$FORMAT" "--suite" "$SUITE")

if [[ "$ok" == 1 ]]; then
  echo "[done] all runs complete. Results in $OUT"
  python3 "$HERE/analyze.py" "${ANALYZE_ARGS[@]}"
else
  echo "[done] some runs failed. Results in $OUT"
  python3 "$HERE/analyze.py" "${ANALYZE_ARGS[@]}" || true
fi

if [[ "$GENERATE_HTML" == 1 || "$FORMAT" == "html" ]]; then
  python3 "$HERE/analyze.py" "$OUT" --format html --suite "$SUITE" --out "$OUT/report.html"
  echo "Interactive HTML report generated: $OUT/report.html"
fi

if [[ "$ok" != 1 ]]; then
  exit 1
fi
