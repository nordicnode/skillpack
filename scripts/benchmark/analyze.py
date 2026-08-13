#!/usr/bin/env python3
"""Analyze skillpack x OpenCode benchmark transcripts.

Usage:
  analyze.py <results-dir> [--format {table,markdown,json,csv,html}]
                           [--suite <suite.json|name>] [--out <file>]

Reads the JSONL event streams opencode `run --format json` writes
(`<condition>-r<N>.json`, or `<condition>.json` for a single run) plus
`<condition>.wall` (wall-clock seconds written by run.sh), and evaluates:

  * Quantitative efficiency: agent rounds, total tokens, cost, wall-clock time
  * Behavioral overhead: help commands run, failed tool attempts, first-shot accuracy
  * Evidence-scored correctness: verifiable flag usage and matched outputs
  * Side-by-side trajectory comparison: diffing baseline vs guided actions

Supported output formats:
  - table    : Formatted ASCII terminal tables (default)
  - markdown : GitHub-flavored Markdown tables for docs / PR comments
  - html     : Rich interactive single-file HTML report
  - json     : Machine-readable JSON summary for CI integration
  - csv      : Comma-separated values for spreadsheet export
"""

import argparse
import glob
import html
import json
import os
import re
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SUITES_DIR = os.path.join(HERE, "suites")

# Default per-question evidence rules (fallback if no suite file is loaded)
DEFAULT_CHECKS = [
    {
        "id": "Q1",
        "name": "Extension filter + Exclude",
        "command_regex": r"fd.*(-e rs|--extension rs).*(-E target|--exclude target)",
        "output_regex": r"(src/[A-Za-z0-9_./-]+\.rs)",
    },
    {
        "id": "Q2",
        "name": "Case-sensitive search",
        "command_regex": r"fd.*(-s |--case-sensitive)",
        "output_regex": r"README",
    },
    {
        "id": "Q3",
        "name": "Disable gitignore filter",
        "command_regex": r"fd.*(-u |--no-ignore|-I |--no-ignore-vcs)(.*(--hidden|-H ))?",
        "output_regex": r"(gitignore|\.git/info/exclude|ignored)",
    },
    {
        "id": "Q4",
        "name": "Exec per result (-x)",
        "command_regex": r"fd.*(-x wc -l|--exec wc -l)",
        "output_regex": r"\d+\s+\S+\.rs",
    },
]


def load_suite(suite_arg=None):
    if not suite_arg:
        # Check if fd.json exists in suites dir
        default_suite = os.path.join(SUITES_DIR, "fd.json")
        if os.path.exists(default_suite):
            suite_arg = default_suite
        else:
            return {"name": "default", "checks": DEFAULT_CHECKS}

    # If argument is just a name like 'fd' or 'ripgrep'
    if not os.path.exists(suite_arg):
        candidate = os.path.join(SUITES_DIR, f"{suite_arg}.json")
        if os.path.exists(candidate):
            suite_arg = candidate
        else:
            print(f"warning: suite file not found: {suite_arg}, using default rules", file=sys.stderr)
            return {"name": "default", "checks": DEFAULT_CHECKS}

    try:
        with open(suite_arg, encoding="utf-8") as f:
            data = json.load(f)
            if "checks" in data:
                return data
            elif isinstance(data, list):
                return {"name": os.path.splitext(os.path.basename(suite_arg))[0], "checks": data}
    except Exception as e:
        print(f"warning: failed to load suite {suite_arg}: {e}", file=sys.stderr)

    return {"name": "default", "checks": DEFAULT_CHECKS}


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


def load_meta(meta_path):
    if not os.path.exists(meta_path):
        return {}
    meta = {}
    try:
        with open(meta_path, encoding="utf-8") as f:
            for line in f:
                if ":" in line:
                    k, v = line.split(":", 1)
                    meta[k.strip()] = v.strip()
    except Exception:
        pass
    return meta


HELP_PATTERN = re.compile(r"(--help|-h\b|\bman\s|grep.*help|help.*grep)", re.I)


def extract_telemetry(events, wall_file=None):
    steps = [e for e in events if e.get("type") == "step_finish"]
    texts = [e for e in events if e.get("type") == "text"]
    tokens, cost = 0, 0.0
    input_tokens, output_tokens, cache_read = 0, 0, 0

    for s in steps:
        part = s.get("part", {}) or {}
        tok_obj = part.get("tokens", {}) or {}
        tokens = tok_obj.get("total", tokens)
        input_tokens = tok_obj.get("input", input_tokens)
        output_tokens = tok_obj.get("output", output_tokens)
        cache_obj = tok_obj.get("cache", {}) or {}
        cache_read = cache_obj.get("read", cache_read)
        cost = part.get("cost", cost)

    wall = 0.0
    if events:
        ts = [e.get("timestamp", 0) for e in events if e.get("timestamp")]
        if ts:
            wall = (max(ts) - min(ts)) / 1000.0
    if wall_file and os.path.exists(wall_file):
        try:
            with open(wall_file) as f:
                wall = float(f.read().strip())
        except Exception:
            pass

    # Command & tool call telemetry
    commands = []
    tool_failures = 0
    help_calls = 0
    trajectories = []

    for e in events:
        part = e.get("part", {}) or {}
        if e.get("type") == "tool_use" or part.get("type") == "tool":
            state = part.get("state", {}) or {}
            meta_data = state.get("metadata", {}) or {}
            cmd = state.get("input", {}).get("command") if isinstance(state.get("input"), dict) else None
            exit_code = meta_data.get("exit", 0) if isinstance(meta_data, dict) else 0
            out_str = str(state.get("output", ""))

            if cmd:
                commands.append(cmd)
                is_help = bool(HELP_PATTERN.search(cmd))
                if is_help:
                    help_calls += 1
                is_fail = exit_code != 0 or "error:" in out_str.lower()
                if is_fail:
                    tool_failures += 1
                trajectories.append({
                    "type": "command",
                    "command": cmd,
                    "exit": exit_code,
                    "output_preview": out_str.strip()[:200] if out_str else "",
                    "is_help": is_help,
                    "is_error": is_fail
                })
        elif part.get("type") == "text" and part.get("text"):
            trajectories.append({
                "type": "text",
                "text": part.get("text")[:300]
            })

    return {
        "rounds": len(steps),
        "texts": len(texts),
        "tokens": tokens,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_read": cache_read,
        "cost": round(cost, 4),
        "wall_s": round(wall, 1),
        "commands_count": len(commands),
        "help_invocations": help_calls,
        "tool_failures": tool_failures,
        "commands": commands,
        "trajectories": trajectories,
    }


def score_questions(events, checks):
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
    for item in checks:
        cmd_re = re.compile(item["command_regex"], re.S)
        out_re = re.compile(item["output_regex"], re.S | re.I)
        cmd_ok = any(cmd_re.search(c) for c in commands)
        out_ok = bool(out_re.search(joined))
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


def fmt_delta(a, b, invert_good=False):
    if a == 0:
        return "—" if b == 0 else ("+100%" if b > 0 else "-100%")
    pct = (b - a) / a * 100.0
    return f"{pct:+.0f}%"


def render_table(header, rows):
    widths = [max(len(str(r[i])) for r in [header] + rows) for i in range(len(header))]
    lines = ["  ".join(h.ljust(widths[i]) for i, h in enumerate(header))]
    for r in rows:
        lines.append("  ".join(str(c).ljust(widths[i]) for i, c in enumerate(r)))
    return "\n".join(lines)


def render_markdown_table(header, rows):
    widths = [max(len(str(r[i])) for r in [header] + rows) for i in range(len(header))]
    header_line = "| " + " | ".join(h.ljust(widths[i]) for i, h in enumerate(header)) + " |"
    sep_line = "|-" + "-|-".join("-" * widths[i] for i in range(len(header))) + "-|"
    data_lines = ["| " + " | ".join(str(c).ljust(widths[i]) for i, c in enumerate(r)) + " |" for r in rows]
    return "\n".join([header_line, sep_line] + data_lines)


def generate_html_report(out_dir, meta, groups, base, suite, report_data):
    b_med = report_data["conditions"][base]
    other_groups = [g for g in report_data["conditions"] if g != base]
    other_g = other_groups[0] if other_groups else base
    o_med = report_data["conditions"][other_g]
    deltas = report_data["deltas"].get(other_g, {})

    suite_name = suite.get("name", "CLI Suite").upper()
    total_q = suite.get("checks", [])

    html_content = f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>skillpack Benchmark Report — {html.escape(suite_name)}</title>
  <style>
    :root {{
      --bg: #090d16;
      --card-bg: #111726;
      --card-border: #1e293b;
      --text: #f1f5f9;
      --text-muted: #94a3b8;
      --accent: #38bdf8;
      --success: #34d399;
      --warning: #fbbf24;
      --danger: #f87171;
      --indigo: #818cf8;
      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
      --font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    }}
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      background: var(--bg);
      color: var(--text);
      font-family: var(--font-sans);
      line-height: 1.6;
      padding: 2rem 1rem;
      min-height: 100vh;
    }}
    .container {{
      max-width: 1100px;
      margin: 0 auto;
    }}
    header {{
      margin-bottom: 2.5rem;
      border-bottom: 1px solid var(--card-border);
      padding-bottom: 1.5rem;
      display: flex;
      justify-content: space-between;
      align-items: flex-end;
      flex-wrap: wrap;
      gap: 1rem;
    }}
    h1 {{
      font-size: 2rem;
      font-weight: 700;
      letter-spacing: -0.02em;
      color: #fff;
    }}
    .badge {{
      display: inline-block;
      padding: 0.25rem 0.6rem;
      font-size: 0.75rem;
      font-weight: 600;
      border-radius: 9999px;
      background: rgba(56, 189, 248, 0.15);
      color: var(--accent);
      border: 1px solid rgba(56, 189, 248, 0.3);
      text-transform: uppercase;
      margin-left: 0.5rem;
    }}
    .meta-bar {{
      color: var(--text-muted);
      font-size: 0.875rem;
      margin-top: 0.5rem;
    }}
    /* Hero KPI Cards */
    .grid-kpi {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      gap: 1.25rem;
      margin-bottom: 2.5rem;
    }}
    .kpi-card {{
      background: var(--card-bg);
      border: 1px solid var(--card-border);
      border-radius: 12px;
      padding: 1.25rem;
      position: relative;
      overflow: hidden;
    }}
    .kpi-title {{
      font-size: 0.8rem;
      text-transform: uppercase;
      letter-spacing: 0.05em;
      color: var(--text-muted);
      font-weight: 600;
      margin-bottom: 0.5rem;
    }}
    .kpi-value {{
      font-size: 2rem;
      font-weight: 800;
      color: #fff;
      display: flex;
      align-items: baseline;
      gap: 0.5rem;
    }}
    .kpi-delta {{
      font-size: 1rem;
      font-weight: 700;
    }}
    .delta-positive {{ color: var(--success); }}
    .delta-negative {{ color: var(--success); }} /* negative steps is good */
    .kpi-sub {{
      font-size: 0.8rem;
      color: var(--text-muted);
      margin-top: 0.35rem;
    }}
    /* Sections & Tables */
    .section-title {{
      font-size: 1.25rem;
      font-weight: 600;
      margin: 2rem 0 1rem;
      color: #fff;
    }}
    .card {{
      background: var(--card-bg);
      border: 1px solid var(--card-border);
      border-radius: 12px;
      overflow: hidden;
      margin-bottom: 2rem;
    }}
    table {{
      width: 100%;
      border-collapse: collapse;
      text-align: left;
      font-size: 0.9rem;
    }}
    th {{
      background: rgba(255, 255, 255, 0.03);
      color: var(--text-muted);
      font-weight: 600;
      padding: 0.85rem 1.25rem;
      border-bottom: 1px solid var(--card-border);
    }}
    td {{
      padding: 0.85rem 1.25rem;
      border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    }}
    tr:last-child td {{ border-bottom: none; }}
    .check-mark {{ color: var(--success); font-weight: bold; }}
    .cross-mark {{ color: var(--danger); font-weight: bold; }}
    .mono {{ font-family: var(--font-mono); }}
    /* Side by Side Trajectory */
    .diff-grid {{
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 1rem;
      padding: 1.25rem;
    }}
    .diff-col h4 {{
      font-size: 0.9rem;
      padding-bottom: 0.5rem;
      margin-bottom: 0.75rem;
      border-bottom: 1px solid var(--card-border);
      color: var(--accent);
    }}
    .diff-step {{
      background: rgba(0, 0, 0, 0.25);
      border: 1px solid rgba(255, 255, 255, 0.07);
      border-radius: 6px;
      padding: 0.6rem 0.8rem;
      margin-bottom: 0.5rem;
      font-family: var(--font-mono);
      font-size: 0.8rem;
    }}
    .diff-step.error {{ border-color: rgba(248, 113, 113, 0.4); background: rgba(248, 113, 113, 0.05); }}
    .diff-step.help {{ border-color: rgba(251, 191, 36, 0.4); background: rgba(251, 191, 36, 0.05); }}
    .step-tag {{
      display: inline-block;
      font-size: 0.65rem;
      padding: 0.1rem 0.4rem;
      border-radius: 4px;
      font-weight: 600;
      margin-right: 0.4rem;
    }}
    .tag-cmd {{ background: rgba(56, 189, 248, 0.2); color: var(--accent); }}
    .tag-help {{ background: rgba(251, 191, 36, 0.2); color: var(--warning); }}
    .tag-err {{ background: rgba(248, 113, 113, 0.2); color: var(--danger); }}
    footer {{
      margin-top: 3rem;
      text-align: center;
      color: var(--text-muted);
      font-size: 0.8rem;
      border-top: 1px solid var(--card-border);
      padding-top: 1.5rem;
    }}
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div>
        <h1>skillpack Benchmark Report <span class="badge">{html.escape(suite_name)}</span></h1>
        <div class="meta-bar">
          Target: <strong>{html.escape(meta.get('target repo', 'unknown'))}</strong> |
          Model: <strong>{html.escape(meta.get('model', 'opencode default'))}</strong> |
          Skillpack: <strong>{html.escape(meta.get('skillpack', '0.11.3'))}</strong>
        </div>
      </div>
      <div>
        <span style="font-size: 0.85rem; color: var(--text-muted);">Total Runs: {len(report_data['runs'])}</span>
      </div>
    </header>

    <!-- KPI Metric Cards -->
    <div class="grid-kpi">
      <div class="kpi-card">
        <div class="kpi-title">Agent Steps / Rounds</div>
        <div class="kpi-value">
          {o_med['rounds_median']}
          <span class="kpi-delta delta-negative">{deltas.get('rounds', '—')}</span>
        </div>
        <div class="kpi-sub">Baseline: {b_med['rounds_median']} steps</div>
      </div>
      <div class="kpi-card">
        <div class="kpi-title">Wall Clock Time</div>
        <div class="kpi-value">
          {o_med['wall_s_median']}s
          <span class="kpi-delta delta-negative">{deltas.get('wall_s', '—')}</span>
        </div>
        <div class="kpi-sub">Baseline: {b_med['wall_s_median']}s</div>
      </div>
      <div class="kpi-card">
        <div class="kpi-title">Help Searches Run</div>
        <div class="kpi-value">
          {o_med.get('help_median', 0)}
          <span class="kpi-delta delta-negative">{deltas.get('help', '0')}</span>
        </div>
        <div class="kpi-sub">Baseline: {b_med.get('help_median', 0)} calls</div>
      </div>
      <div class="kpi-card">
        <div class="kpi-title">Evidence Accuracy</div>
        <div class="kpi-value">
          {o_med['correct_median']}/{len(total_q)}
          <span class="kpi-delta delta-positive">{deltas.get('correct', '—')}</span>
        </div>
        <div class="kpi-sub">Baseline: {b_med['correct_median']}/{len(total_q)}</div>
      </div>
    </div>

    <!-- Comparative Table -->
    <h3 class="section-title">Medians by Condition</h3>
    <div class="card">
      <table>
        <thead>
          <tr>
            <th>Condition</th>
            <th>Runs</th>
            <th>Rounds</th>
            <th>Wall Clock</th>
            <th>Help Searches</th>
            <th>Errors</th>
            <th>Tokens</th>
            <th>Correctness</th>
          </tr>
        </thead>
        <tbody>
"""
    for cond_name, c_data in report_data["conditions"].items():
        is_b = cond_name == base
        prefix = "⚡ " if not is_b else "⚪ "
        html_content += f"""          <tr>
            <td><strong>{prefix}{html.escape(cond_name)}</strong></td>
            <td>{c_data['runs']}</td>
            <td>{c_data['rounds_median']}</td>
            <td>{c_data['wall_s_median']}s</td>
            <td>{c_data.get('help_median', 0)}</td>
            <td>{c_data.get('failures_median', 0)}</td>
            <td>{c_data['tokens_median']}</td>
            <td><strong>{c_data['correct_median']}/{c_data['total_questions']}</strong></td>
          </tr>
"""
    html_content += """        </tbody>
      </table>
    </div>

    <!-- Per Run Breakdown -->
    <h3 class="section-title">Per-Run Detailed Audit</h3>
    <div class="card">
      <table>
        <thead>
          <tr>
            <th>Run ID</th>
            <th>Condition</th>
            <th>Rounds</th>
            <th>Time</th>
            <th>Help Calls</th>
            <th>Tool Errors</th>
            <th>Tokens</th>
            <th>Correct</th>
            <th>Q Breakdown</th>
          </tr>
        </thead>
        <tbody>
"""
    for r in report_data["runs"]:
        sc_icons = "".join(f"<span class='check-mark'>✓</span>" if s == 1.0 else f"<span class='cross-mark'>✗</span>" for s in r["scores"])
        html_content += f"""          <tr>
            <td class="mono">{html.escape(r['run'])}</td>
            <td>{html.escape(r['condition'])}</td>
            <td>{r['rounds']}</td>
            <td>{r['wall_s']}s</td>
            <td>{r['help_invocations']}</td>
            <td>{r['tool_failures']}</td>
            <td>{r['tokens']}</td>
            <td>{r['correct']}/{r['total_questions']}</td>
            <td>{sc_icons}</td>
          </tr>
"""
    html_content += """        </tbody>
      </table>
    </div>

    <!-- Sample Trajectory Diff -->
    <h3 class="section-title">Sample Action Trajectory: Baseline vs Guided</h3>
    <div class="card">
      <div class="diff-grid">
"""
    # Sample run 1 from base and condition B
    b_run1 = next((r for r in report_data["runs"] if r["condition"] == base), None)
    o_run1 = next((r for r in report_data["runs"] if r["condition"] == other_g), None)

    for title, target_run in [(f"Baseline ({base})", b_run1), (f"Guided ({other_g})", o_run1)]:
        html_content += f"""        <div class="diff-col">
          <h4>{html.escape(title)}</h4>
"""
        if target_run and "trajectories" in target_run:
            for step in target_run["trajectories"][:8]:
                if step["type"] == "command":
                    cls = "error" if step["is_error"] else ("help" if step["is_help"] else "")
                    tag = "ERR" if step["is_error"] else ("HELP" if step["is_help"] else "CMD")
                    tag_cls = "tag-err" if step["is_error"] else ("tag-help" if step["is_help"] else "tag-cmd")
                    html_content += f"""          <div class="diff-step {cls}">
            <span class="step-tag {tag_cls}">{tag}</span> {html.escape(step['command'])}
          </div>
"""
                elif step["type"] == "text":
                    preview = step["text"].replace("\n", " ")[:100]
                    html_content += f"""          <div class="diff-step" style="opacity: 0.75; font-style: italic;">
            💬 {html.escape(preview)}...
          </div>
"""
        html_content += "        </div>\n"

    html_content += f"""      </div>
    </div>

    <footer>
      Generated by <code>skillpack analyze</code> — Verification & Distribution Layer for OSS
    </footer>
  </div>
</body>
</html>
"""
    return html_content


def main():
    parser = argparse.ArgumentParser(description="Analyze skillpack x OpenCode benchmark transcripts.")
    parser.add_argument("results_dir", help="Directory containing benchmark results (*.json, *.wall, meta.txt)")
    parser.add_argument(
        "--format", "-f",
        choices=["table", "markdown", "md", "json", "csv", "html"],
        default="table",
        help="Output format: table (default), markdown, json, csv, or html"
    )
    parser.add_argument("--suite", "-s", help="Suite name or path to suite JSON file (e.g. fd, ripgrep, bat)")
    parser.add_argument("--out", "-o", help="Write output directly to a file")
    args = parser.parse_args()

    out_dir = args.results_dir
    suite = load_suite(args.suite)
    checks = suite.get("checks", DEFAULT_CHECKS)

    groups = {}  # label -> {run_idx: {telemetry, scores}}
    for jf in sorted(glob.glob(os.path.join(out_dir, "*.json"))):
        label = os.path.splitext(os.path.basename(jf))[0]
        if label.endswith(".logs"):
            continue
        group, idx = group_key(label)
        events = load_events(jf)
        wall = os.path.join(out_dir, f"{label}.wall")
        groups.setdefault(group, {})[idx] = {
            "telemetry": extract_telemetry(events, wall),
            "scores": score_questions(events, checks),
        }

    if not groups:
        print(f"no transcripts found in {out_dir}", file=sys.stderr)
        sys.exit(1)

    base = None
    for g in sorted(groups):
        if g == "a" or g.startswith("a-"):
            base = g
            break
    if base is None:
        base = sorted(groups)[0]

    num_questions = len(checks)
    meta = load_meta(os.path.join(out_dir, "meta.txt"))

    # Build comprehensive report data structure
    report_data = {
        "results_dir": out_dir,
        "suite": suite.get("name", "default"),
        "metadata": meta,
        "baseline": base,
        "conditions": {},
        "runs": [],
        "deltas": {}
    }

    for group in sorted(groups):
        runs = groups[group]
        tels = [r["telemetry"] for r in runs.values()]
        ss = [sum(r["scores"]) for r in runs.values()]
        report_data["conditions"][group] = {
            "runs": len(runs),
            "rounds_median": med([t["rounds"] for t in tels]),
            "tokens_median": med([t["tokens"] for t in tels]),
            "cost_median": med([t["cost"] for t in tels]),
            "wall_s_median": med([t["wall_s"] for t in tels]),
            "help_median": med([t["help_invocations"] for t in tels]),
            "failures_median": med([t["tool_failures"] for t in tels]),
            "correct_median": med(ss),
            "total_questions": num_questions,
        }
        for idx in sorted(runs):
            t = runs[idx]["telemetry"]
            sc = runs[idx]["scores"]
            report_data["runs"].append({
                "run": f"{group}-r{idx}",
                "condition": group,
                "iteration": idx,
                "rounds": t["rounds"],
                "texts": t["texts"],
                "tokens": t["tokens"],
                "cost": t["cost"],
                "wall_s": t["wall_s"],
                "help_invocations": t["help_invocations"],
                "tool_failures": t["tool_failures"],
                "correct": sum(sc),
                "total_questions": len(sc),
                "scores": sc,
                "trajectories": t["trajectories"],
            })

    b_cond = report_data["conditions"][base]
    for group, data in report_data["conditions"].items():
        if group == base:
            continue
        report_data["deltas"][group] = {
            "rounds": fmt_delta(b_cond["rounds_median"], data["rounds_median"]),
            "tokens": fmt_delta(b_cond["tokens_median"], data["tokens_median"]),
            "wall_s": fmt_delta(b_cond["wall_s_median"], data["wall_s_median"]),
            "help": f"{data['help_median'] - b_cond['help_median']:+g}" if b_cond['help_median'] > 0 else f"{data['help_median']}",
            "cost": fmt_delta(b_cond["cost_median"], data["cost_median"]),
            "correct": fmt_delta(b_cond["correct_median"], data["correct_median"]),
        }

    # Format dispatch
    if args.format == "html":
        output_str = generate_html_report(out_dir, meta, groups, base, suite, report_data)
    elif args.format == "json":
        output_str = json.dumps(report_data, indent=2) + "\n"
    elif args.format == "csv":
        import csv
        import io
        buf = io.StringIO()
        writer = csv.writer(buf)
        writer.writerow(["run", "condition", "iteration", "rounds", "tokens", "cost", "wall_s", "help_calls", "errors", "correct", "total_questions"])
        for r in report_data["runs"]:
            writer.writerow([r["run"], r["condition"], r["iteration"], r["rounds"], r["tokens"], r["cost"], r["wall_s"], r["help_invocations"], r["tool_failures"], r["correct"], r["total_questions"]])
        output_str = buf.getvalue()
    elif args.format in ("markdown", "md"):
        header = ["run", "rounds", "tokens", "cost $", "wall s", "help calls", "errors", "correct", "breakdown"]
        rows = []
        for r in report_data["runs"]:
            breakdown = "".join("✓" if s == 1.0 else "✗" for s in r["scores"])
            rows.append([r["run"], str(r["rounds"]), str(r["tokens"]), f"{r['cost']:.4f}", f"{r['wall_s']:.1f}", str(r["help_invocations"]), str(r["tool_failures"]), f"{r['correct']:g}/{r['total_questions']}", breakdown])

        mheader = ["condition", "runs", "rounds", "tokens", "cost $", "wall s", "help calls", "errors", "correct"]
        mrows = []
        for group, c in report_data["conditions"].items():
            mrows.append([group, str(c["runs"]), str(round(c["rounds_median"], 1)), str(round(c["tokens_median"])), f"{c['cost_median']:.4f}", f"{c['wall_s_median']:.1f}", str(c["help_median"]), str(c["failures_median"]), f"{c['correct_median']:g}/{c['total_questions']}"])

        dheader = ["metric", f"baseline ({base})", "condition", "delta"]
        drows = []
        for group in sorted(groups):
            if group == base:
                continue
            c = report_data["conditions"][group]
            d = report_data["deltas"][group]
            drows.append(["rounds", str(b_cond["rounds_median"]), f"{group}: {c['rounds_median']}", d["rounds"]])
            drows.append(["wall_s", f"{b_cond['wall_s_median']:.1f}s", f"{group}: {c['wall_s_median']:.1f}s", d["wall_s"]])
            drows.append(["help_calls", str(b_cond["help_median"]), f"{group}: {c['help_median']}", d["help"]])
            drows.append(["tokens", str(b_cond["tokens_median"]), f"{group}: {c['tokens_median']}", d["tokens"]])
            drows.append(["correct", f"{b_cond['correct_median']:g}/{num_questions}", f"{group}: {c['correct_median']:g}/{num_questions}", d["correct"]])

        lines = [
            f"### Benchmark Results: `{out_dir}` (Suite: `{suite.get('name', 'default')}`)\n",
            "#### Per-Run Detail\n",
            render_markdown_table(header, rows),
            "\n#### Medians by Condition\n",
            render_markdown_table(mheader, mrows),
            f"\n#### Delta vs Baseline (`{base}`)\n",
            render_markdown_table(dheader, drows),
            ""
        ]
        output_str = "\n".join(lines)
    else:  # Table terminal output
        header = ["run", "rounds", "tokens", "cost $", "wall s", "help", "errs", "correct", "breakdown"]
        rows = []
        for r in report_data["runs"]:
            breakdown = "".join("✓" if s == 1.0 else "✗" for s in r["scores"])
            rows.append([r["run"], str(r["rounds"]), str(r["tokens"]), f"{r['cost']:.4f}", f"{r['wall_s']:.1f}", str(r["help_invocations"]), str(r["tool_failures"]), f"{r['correct']:g}/{r['total_questions']}", breakdown])

        mheader = ["condition", "runs", "rounds", "tokens", "cost $", "wall s", "help", "errs", "correct"]
        mrows = []
        for group, c in report_data["conditions"].items():
            mrows.append([group, str(c["runs"]), str(round(c["rounds_median"], 1)), str(round(c["tokens_median"])), f"{c['cost_median']:.4f}", f"{c['wall_s_median']:.1f}", str(c["help_median"]), str(c["failures_median"]), f"{c['correct_median']:g}/{c['total_questions']}"])

        dheader = ["metric", f"baseline ({base})", "condition", "delta"]
        drows = []
        for group in sorted(groups):
            if group == base:
                continue
            c = report_data["conditions"][group]
            d = report_data["deltas"][group]
            drows.append(["rounds", str(b_cond["rounds_median"]), f"{group}: {c['rounds_median']}", d["rounds"]])
            drows.append(["wall_s", f"{b_cond['wall_s_median']:.1f}s", f"{group}: {c['wall_s_median']:.1f}s", d["wall_s"]])
            drows.append(["help_calls", str(b_cond["help_median"]), f"{group}: {c['help_median']}", d["help"]])
            drows.append(["tokens", str(b_cond["tokens_median"]), f"{group}: {c['tokens_median']}", d["tokens"]])
            drows.append(["correct", f"{b_cond['correct_median']:g}/{num_questions}", f"{group}: {c['correct_median']:g}/{num_questions}", d["correct"]])

        lines = [
            f"\nbenchmark results: {out_dir} (Suite: {suite.get('name', 'default')})\n",
            render_table(header, rows),
            "\nmedians by condition (delta vs baseline):\n",
            render_table(mheader, mrows),
            f"\ndelta vs baseline ({base} median):\n",
            render_table(dheader, drows),
            ""
        ]
        output_str = "\n".join(lines)

    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(output_str)
        print(f"Report written to {args.out}")
    else:
        print(output_str)


if __name__ == "__main__":
    main()
