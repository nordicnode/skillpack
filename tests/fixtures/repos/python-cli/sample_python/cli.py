"""Entry point for the sample_python CLI.

Mirrors the `[project.scripts]` target in pyproject.toml
(`sample-python = "sample_python.cli:main"`). Prints a `--help` usage line so
skillpack's invocation check can capture it.
"""

import sys


def main() -> None:
    argv = sys.argv[1:]
    if "--help" in argv or "-h" in argv:
        print("Usage: sample-python [--lint] [--fix]")
        return
    print("sample-python")


if __name__ == "__main__":
    main()
