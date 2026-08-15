#!/usr/bin/env python3
"""Validate the committed JSON Schemas and a representative report instance.

Guards against two drift classes:
1. A schema that is not a valid draft-07 schema (typos, bad refs).
2. `verify --format json` output drifting from `verify-report.schema.json`
   (the report's `$schema` / `schemaVersion` self-pointer must match, and a
   representative instance must validate).

Run from the repo root. Exits non-zero on any failure so CI fails loudly.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from jsonschema import Draft7Validator

REPO_ROOT = Path(__file__).resolve().parent.parent

SCHEMA_FILES = [
    REPO_ROOT / "skillpack.schema.json",
    REPO_ROOT / "verify-report.schema.json",
]

# A representative `verify --format json` report. Mirrors the fields
# `src/verify/mod.rs::render_json` emits, including the `$schema` and
# `schemaVersion` self-pointer. Keep this in sync with render_json.
SAMPLE_REPORT = {
    "$schema": "https://raw.githubusercontent.com/nordicnode/skillpack/main/verify-report.schema.json",
    "schemaVersion": 1,
    "ok": True,
    "discoverability_score": 100,
    "counts": {"pass": 3, "warn": 0, "fail": 0, "skip": 0},
    "results": [
        {
            "check_id": "discovery.plugin.present",
            "check_name": "Plugin manifest present",
            "severity": "pass",
            "message": "plugin.json found",
        },
        {
            "check_id": "discovery.description_length",
            "check_name": "Description length",
            "severity": "warn",
            "message": "description is short",
            "suggestion": "expand the one-line description",
            "location": {"file": ".claude-plugin/plugin.json", "line": 3},
        },
    ],
}


def main() -> int:
    failures = []

    for schema_file in SCHEMA_FILES:
        if not schema_file.is_file():
            failures.append(f"{schema_file.name}: file missing")
            continue
        try:
            schema = json.loads(schema_file.read_text())
        except json.JSONDecodeError as exc:
            failures.append(f"{schema_file.name}: not valid JSON: {exc}")
            continue
        try:
            # `check_schema` raises on an invalid draft-07 schema.
            Draft7Validator.check_schema(schema)
            print(f"OK   {schema_file.name}: valid draft-07 schema")
        except Exception as exc:  # jsonschema.SchemaError
            failures.append(f"{schema_file.name}: invalid schema: {exc}")

    # Validate the representative report against the committed report schema,
    # including the self-pointer (`const` on `$schema` and `schemaVersion`).
    report_schema_file = REPO_ROOT / "verify-report.schema.json"
    if report_schema_file.is_file():
        try:
            schema = json.loads(report_schema_file.read_text())
            errors = sorted(
                Draft7Validator(schema).iter_errors(SAMPLE_REPORT),
                key=lambda e: list(e.path),
            )
            if errors:
                for err in errors:
                    failures.append(
                        f"verify-report.schema.json: sample report fails at "
                        f"{'/'.join(map(str, err.path)) or '<root>'}: {err.message}"
                    )
            else:
                print("OK   verify-report.schema.json: sample report validates")
        except json.JSONDecodeError:
            # Already reported above; don't double-count.
            pass

    if failures:
        print("\nSchema validation FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print("\nAll JSON schemas valid.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
