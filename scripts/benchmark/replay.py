#!/usr/bin/env python3
"""Offline benchmark transcript replay and evaluation validator.

Usage:
  python3 scripts/benchmark/replay.py [results-dir] [--suite fd|ripgrep|bat|<path>]

Runs in offline CI environments without requiring live LLM API keys. Replays
committed JSONL event streams, verifies schema integrity, runs evidence scoring,
and ensures benchmark metrics match expected tolerance boundaries.
"""

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from analyze import load_events, load_suite, extract_telemetry, score_questions, group_key, med


def main():
    results_dir = sys.argv[1] if len(sys.argv) > 1 and not sys.argv[1].startswith("-") else os.path.join(HERE, "results")
    suite_arg = "fd"
    if "--suite" in sys.argv:
        idx = sys.argv.index("--suite")
        if idx + 1 < len(sys.argv):
            suite_arg = sys.argv[idx + 1]

    suite = load_suite(suite_arg)
    checks = suite.get("checks", [])

    print(f"=== Replaying Benchmark Suite '{suite.get('name', 'unknown')}' from {results_dir} ===")

    json_files = sorted([f for f in os.listdir(results_dir) if f.endswith(".json") and not f.endswith(".logs")])
    if not json_files:
        print(f"error: no transcript json files found in {results_dir}")
        sys.exit(1)

    print(f"Found {len(json_files)} transcript(s) to validate.")

    groups = {}
    total_events = 0

    for jf in json_files:
        full_path = os.path.join(results_dir, jf)
        label = os.path.splitext(jf)[0]
        group, idx = group_key(label)
        events = load_events(full_path)
        total_events += len(events)
        wall_file = os.path.join(results_dir, f"{label}.wall")

        telemetry = extract_telemetry(events, wall_file)
        scores = score_questions(events, checks)

        groups.setdefault(group, {})[idx] = {
            "telemetry": telemetry,
            "scores": scores,
        }

        print(f"  ✓ {label:<16}: {len(events)} events | {telemetry['rounds']} rounds | {telemetry['wall_s']}s | score: {sum(scores)}/{len(scores)} | help calls: {telemetry['help_invocations']}")

    print(f"\nTotal events verified: {total_events}")

    # Assertion check: verify condition B has fewer help calls and equal or higher score than condition A
    base_group = "a-plain" if "a-plain" in groups else sorted(groups.keys())[0]
    guided_group = "b-skillpack" if "b-skillpack" in groups else [g for g in groups if g != base_group][0]

    b_help = med([r["telemetry"]["help_invocations"] for r in groups[base_group].values()])
    g_help = med([r["telemetry"]["help_invocations"] for r in groups[guided_group].values()])

    b_score = med([sum(r["scores"]) for r in groups[base_group].values()])
    g_score = med([sum(r["scores"]) for r in groups[guided_group].values()])

    b_rounds = med([r["telemetry"]["rounds"] for r in groups[base_group].values()])
    g_rounds = med([r["telemetry"]["rounds"] for r in groups[guided_group].values()])

    print("\n--- Integrity Assertions ---")
    assert g_help <= b_help, f"Guided group ran more help calls ({g_help}) than baseline ({b_help})!"
    print(f"  ✓ Help calls assertion passed: {g_help} <= {b_help}")

    assert g_score >= b_score, f"Guided group score ({g_score}) lower than baseline ({b_score})!"
    print(f"  ✓ Score assertion passed: {g_score} >= {b_score}")

    assert g_rounds <= b_rounds, f"Guided group rounds ({g_rounds}) higher than baseline ({b_rounds})!"
    print(f"  ✓ Rounds efficiency assertion passed: {g_rounds} <= {b_rounds}")

    print("\n[OK] All benchmark transcripts successfully validated!")


if __name__ == "__main__":
    main()
