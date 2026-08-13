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
# only advantage is the skillpack-generated guidance itself. This removes the
# confound documented in the original docs/agent-demo.md (which routed B into
# a purpose-built subagent).
#
# Usage:
#   scripts/benchmark/run.sh [--runs N] [--model M] [--out DIR] [--work DIR]
#                            [--fresh] [--no-build]
#
# Env overrides (same names): SKILLPACK_BENCH_RUNS, _MODEL, _OUT, _WORK,
# _QUESTIONS, _BIN (skillpack binary), _REPO (target repo URL).
#
# Output (one transcript set per run):
#   <out>/a-plain-r<N>.json     opencode event stream (JSONL)
#   <out>/a-plain-r<N>.logs     opencode stderr logs
#   <out>/a-plain-r<N>.wall     wall-clock seconds for the run
#   <out>/b-skillpack-r<N>.json/.logs/.wall
#   <out>/b-skillpack-r<N>.agents.md   the generated AGENTS.md (audit trail)
#   <out>/meta.txt              model / versions / repo rev / questions
#
# Then: python3 scripts/benchmark/analyze.py <out>
#
# The target repo is cached in <work>/fd-src and built once with
# `cargo build --release`; each condition is a fresh clone of that cache so
# condition A stays pristine (no target/) and condition B gets the built
# binary copied in for skillpack to introspect. Pass --fresh to re-clone and
# rebuild, --no-build to skip the build check entirely.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${SKILLPACK_BENCH_MODEL:-opencode/claude-haiku-4-5}"
RUNS="${SKILLPACK_BENCH_RUNS:-1}"
QUESTIONS="${SKILLPACK_BENCH_QUESTIONS:-$HERE/questions.txt}"
OUT="${SKILLPACK_BENCH_OUT:-$HERE/results}"
WORK="${SKILLPACK_BENCH_WORK:-/tmp/skillpack-bench}"
BIN="${SKILLPACK_BENCH_BIN:-skillpack}"
REPO="${SKILLPACK_BENCH_REPO:-https://github.com/sharkdp/fd.git}"
BRANCH="${SKILLPACK_BENCH_BRANCH:-master}"
FRESH=0
NO_BUILD=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runs) RUNS="$2"; shift 2 ;;
    --model) MODEL="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --work) WORK="$2"; shift 2 ;;
    --fresh) FRESH=1; shift ;;
    --no-build) NO_BUILD=1; shift ;;
    -h|--help) sed -n '1,30p' "$0" | sed 's/^# \?//'; exit 0 ;;
    *) echo "unknown option: $1"; exit 2 ;;
  esac
done

for tool in git cargo opencode python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: missing required tool: $tool"; exit 1; }
done
if ! command -v "$BIN" >/dev/null 2>&1 && [[ ! -x "$BIN" ]]; then
  echo "error: skillpack binary not found: $BIN (set SKILLPACK_BENCH_BIN)"; exit 1
fi
[[ -r "$QUESTIONS" ]] || { echo "error: questions file not readable: $QUESTIONS"; exit 1; }
[[ "$RUNS" =~ ^[0-9]+$ ]] && [[ "$RUNS" -ge 1 ]] || { echo "error: --runs must be a positive integer"; exit 1; }

mkdir -p "$WORK" "$OUT"

# --- target repo cache: clone once, build once -----------------------------
SRC="$WORK/fd-src"
FD_BIN="$SRC/target/release/fd"
if [[ "$FRESH" == 1 ]]; then
  rm -rf "$SRC"
fi
if [[ ! -d "$SRC/.git" ]]; then
  echo "[setup] cloning $REPO -> $SRC"
  git clone -q --depth=1 --branch "$BRANCH" "$REPO" "$SRC"
fi
if [[ "$NO_BUILD" != 1 && ! -x "$FD_BIN" ]]; then
  echo "[setup] cargo build --release in $SRC (first run only; takes a few minutes)"
  ( cd "$SRC" && cargo build --release )
fi
[[ -x "$FD_BIN" ]] || { echo "error: built binary not found: $FD_BIN (use --no-build? build first?)"; exit 1; }

# --- metadata ---------------------------------------------------------------
{
  echo "created: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "model: $MODEL"
  echo "runs: $RUNS"
  echo "opencode: $(opencode --version 2>/dev/null | head -1 || true)"
  echo "skillpack: $("$BIN" --version 2>/dev/null | head -1 || true)"
  echo "target repo: $REPO @ $(git -C "$SRC" rev-parse --short HEAD 2>/dev/null || true)"
  echo "target build: $(test -x "$FD_BIN" && "$FD_BIN" --version 2>/dev/null || echo none)"
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
    cp "$FD_BIN" "$dir/target/release/fd"
    echo "[run] $stem: skillpack init --auto --target all --force"
    ( cd "$dir" && "$BIN" init --auto --target all --force )
    cp "$dir/AGENTS.md" "$OUT/$stem.agents.md"
  fi
  echo "[run] $stem: opencode run (model=$MODEL) — this is the slow step"
  local start end wall
  start=$(date +%s%N)
  if ( cd "$dir" && opencode run "$(cat "$QUESTIONS")" -m "$MODEL" --pure --format json --print-logs \
        >"$OUT/$stem.json" 2>"$OUT/$stem.logs" ); then
    end=$(date +%s%N)
    awk -v d="$((end - start))" 'BEGIN { printf "%.2f\n", d / 1e9 }' > "$OUT/$stem.wall"
  else
    echo "  !! opencode run failed for $stem — see $OUT/$stem.logs"
    return 1
  fi
}

ok=1
for i in $(seq 1 "$RUNS"); do
  run_condition "a-plain-r$i" "$WORK/run$i/a-plain" 0 || ok=0
  run_condition "b-skillpack-r$i" "$WORK/run$i/b-skillpack" 1 || ok=0
done

echo
if [[ "$ok" == 1 ]]; then
  echo "[done] all runs complete. Results in $OUT"
  python3 "$HERE/analyze.py" "$OUT"
else
  echo "[done] some runs failed. Results in $OUT"
  exit 1
fi
