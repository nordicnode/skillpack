# Security Policy

skillpack runs arbitrary subcommands against your project's CLI binaries during
`init` and `verify`, so we take security reports seriously.

## Reporting a vulnerability

**Do not open a public issue** for security problems. Report privately instead:

- GitHub private vulnerability reporting:
  https://github.com/nordicnode/skillpack/security/advisories/new
- Or email the maintainers directly if private reporting is unavailable.

Please include:

- The affected version(s)
- A minimal reproduction (project shape + commands run)
- Impact assessment if you have one

## Response expectations

- We acknowledge reports within **3 business days**.
- We aim to ship a fix and advisory within **14 days** of triage, faster for
  critical issues.
- We'll credit reporters in the advisory (unless you prefer anonymity).
