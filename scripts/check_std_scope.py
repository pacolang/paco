#!/usr/bin/env python3
"""Check that `stdlib`'s scope matches what docs/ecosystem.md documents.

Two checks, both mechanical and deliberately dumb (grep with structure, not a
parser):

1. Every `.paco` module directly under `stdlib/` (plus `core`, for the whole
   `stdlib/core/` directory, which is the prelude) has a row in the module table
   of `docs/ecosystem.md`, and the table has no row for a module that isn't
   really there.
2. No `.paco` file under `stdlib/` imports anything other than another `stdlib`
   module — `stdlib` imports only `stdlib` (ADR 0030).

Exit 0 when clean, 1 with a description of every violation.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STD = ROOT / "stdlib"
ECOSYSTEM = ROOT / "docs" / "ecosystem.md"

USE_RE = re.compile(r"^\s*use\s+([^\s;]+)")
TABLE_MODULE_RE = re.compile(r"^\|\s*`([A-Za-z0-9_]+)`\s*\|", re.MULTILINE)


def real_modules() -> set[str]:
    modules = set()
    for entry in STD.iterdir():
        if entry.name == "core" and entry.is_dir():
            modules.add("core")
        elif entry.is_file() and entry.suffix == ".paco":
            modules.add(entry.stem)
    return modules


def documented_modules() -> set[str]:
    text = ECOSYSTEM.read_text(encoding="utf-8")
    return set(TABLE_MODULE_RE.findall(text))


def check_scope() -> list[str]:
    real = real_modules()
    documented = documented_modules()
    problems = []
    for missing in sorted(real - documented):
        problems.append(f"stdlib/{missing} exists but has no row in docs/ecosystem.md's module table")
    for stale in sorted(documented - real):
        problems.append(f"docs/ecosystem.md documents `{stale}`, but stdlib/{stale} does not exist")
    return problems


def check_imports() -> list[str]:
    problems = []
    for path in sorted(STD.rglob("*.paco")):
        for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            stripped = line.strip()
            if stripped.startswith("//"):
                continue
            match = USE_RE.match(line)
            if not match:
                continue
            target = match.group(1)
            if not (target == "stdlib" or target.startswith("stdlib::")):
                rel = path.relative_to(ROOT)
                problems.append(f"{rel}:{lineno}: `use {target}` — stdlib imports only stdlib")
    return problems


def main() -> int:
    problems = check_scope() + check_imports()
    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1
    print(f"check_std_scope: clean ({len(real_modules())} stdlib modules)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
