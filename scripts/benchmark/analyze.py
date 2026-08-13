#!/usr/bin/env python3
"""Analyze skillpack x OpenCode benchmark transcripts.

Usage: analyze.py <results-dir>

Reads the JSONL event streams opencode `run --format json` writes
(`<condition>-r<N>.json`, or `<condition>.json` for a single run) plus
`<condition>.wall` (wall-clock seconds written by run.sh), and prints:

  * per-run detail (agent step rounds, text answers, tokens, cost, wall clock,
    evidence-scored correctness),
  * per-condition medians across runs (single-run runs degenerate to the run
    itself), and
  * a delta table of medians vs the baseline condition (label starting with
    "a").

Metrics come from the event stream itself (the demo's original method:
`step_finish` events = agent rounds, last cumulative `tokens.total`, summed
`cost`, first→last `timestamp` = agent wall clock). Wall clock is taken from
the `.wall` file when present (includes full process time). Correctness is
evidence-scored from the transcript: each question has command + output rules;
a question scores 1.0 when the agent ran a command matching the expected flag
usage AND the tool output/final text shows the expected result. This is a
proxy for human review — it measures "used the right flags and got the right
output", not prose quality.
"""

import glob
import json
import os
import re
import statistics
import sys

# Per-question evidence rules: (command_regex, output_or_text_regex)
# Commands are matched against every `bash` tool call; output/text against
# every tool output and the final assistant text. fd-specific (the default
# target repo), matching docs/demo methodology.
QUESTION_CHECKS = [
    # Q1: -e rs + exclude target (-E or --exclude); output shows src/*.rs
    (
        re.compile(r"fd.*(-e rs|--extension rs).*(-E target|--exclude target)", re.S),
        re.compile(r"(src/[A-Za-z0-9_./-]+\.rs)", re.S),
    ),
    # Q2: -s (case-sensitive); output shows README
    (
        re.compile(r"fd.*(-s |--case-sensitive)", re.S),
        re.compile(r"README", re.S),
    ),
    # Q3: ignore-disable flag (-u/--no-ignore, -I, or --no-ignore-vcs,
    # optionally with -H/--hidden); text explains the gitignore mechanism
    (
        re.compile(r"fd.*(-u |--no-ignore|-I |--no-ignore-vcs)(.*(--hidden|-H ))?", re.S),
        re.compile(r"(gitignore|\.git/info/exclude|ignored)", re.I),
    ),
    # Q4: exec-per-result wc -l (-x/--exec wc -l)
    (
        re.compile(r"fd.*(-x wc -l|--exec wc -l)", re.S),
        re.compile(r"\d+\s+\S+\.rs", re.S),
    ),
]


def load_events(path):
    events = []
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    events.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
    except FileNotFoundError:
        pass
    return events


def metrics(events, wall_file=None):
    steps = [e for e in events if e.get("type") == "step_finish"]
    texts = [e for e in events if e.get("type") == "text"]
    tokens, cost = 0, 0.0
    for s in steps:
        part = s.get("part", {})
        tokens = part.get("tokens", {}).get("total", tokens)  # cumulative
        cost = part.get("cost", cost)
    wall = 0.0
    if events:
        ts = [e.get("timestamp", 0) for e in events if e.get("timestamp")]
        if ts:
            wall = (max(ts) - min(ts)) / 1000.0
    if wall_file and os.path.exists(wall_file):
        with open(wall_file) as f:
            wall = float(f.read().strip())
    return {
        "rounds": len(steps),
        "texts": len(texts),
        "tokens": tokens,
        "cost": cost,
        "wall_s": wall,
    }


def score_questions(events):
    commands = []
    output_text = []
    for e in events:
        part = e.get("part", {}) or {}
        if e.get("type") == "tool_use" or part.get("type") == "tool":
            state = part.get("state", {}) or {}
            cmd = state.get("input", {}).get("command") if isinstance(state.get("input"), dict) else None
            if cmd:
                commands.append(cmd)
            if state.get("output"):
                output_text.append(str(state["output"]))
        elif part.get("type") == "text" and part.get("text"):
            output_text.append(str(part["text"]))
    joined = "\n".join(output_text)
    scores = []
    for cmd_re, out_re in QUESTION_CHECKS:
        cmd_ok = any(cmd_re.search(c) for c in commands)
        out_ok = bool(out_re.search(joined))
        # A question is only attempted if the agent ran something; evidence
        # absent entirely means it wasn't attempted (0.0, not "failed").
        scores.append(1.0 if cmd_ok and out_ok else 0.0)
    return scores


def group_key(label):
    """'b-skillpack-r3' -> ('b-skillpack', 3); 'a-plain' -> ('a-plain', 1)."""
    m = re.match(r"^(.*?)-r(\d+)$", label)
    if m:
        return m.group(1), int(m.group(2))
    return label, 1


def med(vals):
    if not vals:
        return 0.0
    return statistics.median(vals)


def fmt_delta(a, b):
    if a == 0:
        return "—"
    pct = (b - a) / a * 100.0
    return f"{pct:+.0f}%"


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    out_dir = sys.argv[1]

    # Collect per-run results, grouped by condition.
    groups = {}  # label -> {run_idx: {metrics, scores}}
    for jf in sorted(glob.glob(os.path.join(out_dir, "*.json"))):
        label = os.path.splitext(os.path.basename(jf))[0]
        if label.endswith(".logs"):
            continue
        group, idx = group_key(label)
        events = load_events(jf)
        wall = os.path.join(out_dir, f"{label}.wall")
        groups.setdefault(group, {})[idx] = {
            "metrics": metrics(events, wall),
            "scores": score_questions(events),
        }

    if not groups:
        print(f"no transcripts found in {out_dir}")
        sys.exit(1)

    base = None
    for g in sorted(groups):
        if g == "a" or g.startswith("a-"):
            base = g
            break
    if base is None:
        base = sorted(groups)[0]

    # Detail table: every run.
    print(f"\nbenchmark results: {out_dir}\n")
    header = ["run", "rounds", "texts", "tokens", "cost $", "wall s", "correct"]
    rows = []
    for group in sorted(groups):
        for idx in sorted(groups[group]):
            m = groups[group][idx]["metrics"]
            sc = groups[group][idx]["scores"]
            rows.append(
                [f"{group}-r{idx}", str(m["rounds"]), str(m["texts"]), str(m["tokens"]),
                 str(round(m["cost"], 4)), str(round(m["wall_s"], 1)), f"{sum(sc)}/{len(sc)}"]
            )
    widths = [max(len(r[i]) for r in [header] + rows) for i in range(len(header))]
    print("  ".join(h.ljust(widths[i]) for i, h in enumerate(header)))
    for r in rows:
        print("  ".join(str(c).ljust(widths[i]) for i, c in enumerate(r)))

    # Median table per condition + deltas vs baseline.
    print("\nmedians by condition (delta vs baseline):\n")
    mheader = ["condition", "runs", "rounds", "tokens", "cost $", "wall s", "correct"]
    mrows = []
    for group in sorted(groups):
        runs = groups[group]
        ms = [r["metrics"] for r in runs.values()]
        ss = [sum(r["scores"]) for r in runs.values()]
        n = len(next(iter(runs.values()))["scores"]) if runs else 0
        mrows.append(
            [group, str(len(runs)), str(round(med([m["rounds"] for m in ms]), 1)),
             str(round(med([m["tokens"] for m in ms]))),
             str(round(med([m["cost"] for m in ms]), 4)),
             str(round(med([m["wall_s"] for m in ms]), 1)),
             f"{med(ss):g}/{n}" + (f" {ss}" if len(ss) > 1 else "")]
        )
    widths = [max(len(r[i]) for r in [mheader] + mrows) for i in range(len(mheader))]
    print("  ".join(h.ljust(widths[i]) for i, h in enumerate(mheader)))
    for r in mrows:
        print("  ".join(str(c).ljust(widths[i]) for i, c in enumerate(r)))

    b = groups[base]
    bm = [r["metrics"] for r in b.values()]
    bs = [sum(r["scores"]) for r in b.values()]
    n = len(next(iter(b.values()))["scores"]) if b else 0
    print(f"\ndelta vs baseline ({base} median):\n")
    print(f"{'metric':<10} {'baseline':>10} {'condition':>12} {'delta':>8}")
    for group in sorted(groups):
        if group == base:
            continue
        runs = groups[group]
        ms = [r["metrics"] for r in runs.values()]
        ss = [sum(r["scores"]) for r in runs.values()]
        for metric in ["rounds", "wall_s", "tokens", "cost"]:
            a = med([m[metric] for m in bm])
            c = med([m[metric] for m in ms])
            print(f"{metric:<10} {a:>10} {c:>12} {fmt_delta(a, c):>8}")
        a, c = med(bs), med(ss)
        print(f"{'correct':<10} {f'{a:g}/{n}':>10} {f'{c:g}/{n}':>12} {fmt_delta(a, c):>8}")
    print()


if __name__ == "__main__":
    main()
