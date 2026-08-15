#!/usr/bin/env python3
"""Regenerate the Homebrew formula's per-platform sha256 pins.

Downloads each release tarball referenced in homebrew/skillpack.rb and
rewrites the `sha256` line that follows each `url` to the current checksum.
The release must be published first (the binaries must exist). Normally this
runs automatically: the Release workflow's `re-pin-homebrew` job invokes it on
the `v*` tag push and commits the result to main. Run it by hand only for an
out-of-band fix:

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
    dry_run = "--dry-run" in sys.argv[1:]
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
        digest = sha256_of(m.group(2))
        if dry_run:
            print(f"{m.group(2)} -> {digest}")
        else:
            out.append(line)
            out.append(f'{m.group(1)}sha256 "{digest}"')
        updated += 1
        i += 1
        # Drop a pre-existing sha256 line (or the re-pin reminder comment the
        # release-plz sync left behind) so it is replaced, not duplicated.
        if i < len(lines) and (
            lines[i].lstrip().startswith("sha256 ") or "re-pin sha256" in lines[i]
        ):
            i += 1
    if not dry_run:
        FORMULA.write_text("\n".join(out) + "\n")
        print(f"updated {updated} sha256 pin(s) in {FORMULA}")
    else:
        print(f"dry run: {updated} sha256 pin(s) would be updated (formula untouched)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
