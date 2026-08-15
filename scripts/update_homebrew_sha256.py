#!/usr/bin/env python3
"""Regenerate the Homebrew formula's per-platform sha256 pins.

Downloads each release tarball referenced in homebrew/skillpack.rb and
rewrites the `sha256` line that follows each `url` to the current checksum.
Run after a release is published (the binaries must exist):

    python3 scripts/update_homebrew_sha256.py

The release-plz sync step strips the `sha256` lines when it bumps the version,
so a stale checksum can never ship; this script re-pins them once the release
is live.
"""

import hashlib
import re
import sys
import urllib.request
from pathlib import Path

FORMULA = Path(__file__).resolve().parent.parent / "homebrew" / "skillpack.rb"

URL_RE = re.compile(r'^(\s*)url "(https://[^"]+\.tar\.gz)"\s*$')


def sha256_of(url: str) -> str:
    with urllib.request.urlopen(url) as resp:
        return hashlib.sha256(resp.read()).hexdigest()


def main() -> int:
    lines = FORMULA.read_text().splitlines()
    out = []
    updated = 0
    i = 0
    while i < len(lines):
        line = lines[i]
        m = URL_RE.match(line)
        if not m:
            out.append(line)
            i += 1
            continue
        out.append(line)
        out.append(f'{m.group(1)}sha256 "{sha256_of(m.group(2))}"')
        updated += 1
        i += 1
        # Drop a pre-existing sha256 line (or the re-pin reminder comment the
        # release-plz sync left behind) so it is replaced, not duplicated.
        if i < len(lines) and (
            lines[i].lstrip().startswith("sha256 ") or "re-pin sha256" in lines[i]
        ):
            i += 1
    FORMULA.write_text("\n".join(out) + "\n")
    print(f"updated {updated} sha256 pin(s) in {FORMULA}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
