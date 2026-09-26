#!/usr/bin/env python3
"""Catch documentation drifting toward another language's syntax.

Reads docs/consistency-rules.toml and greps every tracked Markdown file (and the
EBNF grammar) for spellings that are not Paco. Deliberately dumb and syntactic:
it is grep with an exception list, not a prose linter.

Exit 0 when clean, 1 with file:line for every violation.
"""
import re
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    print("needs Python 3.11+ (tomllib)", file=sys.stderr)
    sys.exit(2)

ROOT = Path(__file__).resolve().parent.parent
RULES = ROOT / "docs" / "consistency-rules.toml"
SCANNED_SUFFIXES = {".md", ".ebnf"}


def tracked_files():
    out = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.split()
    for rel in out:
        if Path(rel).suffix in SCANNED_SUFFIXES and not rel.startswith("openspec/changes/archive/"):
            yield rel


def main() -> int:
    rules = tomllib.loads(RULES.read_text())["rule"]
    compiled = [
        (r["id"], re.compile(r["forbidden"]), r["canonical"], r["message"],
         set(r.get("except_in", [])))
        for r in rules
    ]

    violations = []
    for rel in tracked_files():
        # The rules file names the forbidden spellings; scanning it finds itself.
        if rel in ("docs/consistency-rules.toml", "scripts/check_docs_consistency.py"):
            continue
        text = (ROOT / rel).read_text(encoding="utf-8", errors="replace")
        lines = text.splitlines()
        for lineno, line in enumerate(lines, 1):
            # A line may opt out, for prose that must SHOW the forbidden spelling
            # in order to say it is forbidden:
            #   <!-- consistency-ignore: selective-import -->
            prev = lines[lineno - 2] if lineno >= 2 else ""
            ignored = set()
            for src in (line, prev):
                if "consistency-ignore:" in src:
                    tail = src.split("consistency-ignore:", 1)[1]
                    tail = tail.replace("-->", "").replace("*)", "")
                    ignored.update(t.strip() for t in tail.split(","))
            for rid, pattern, canonical, message, excepted in compiled:
                if rel in excepted or rid in ignored:
                    continue
                m = pattern.search(line)
                if m:
                    violations.append((rel, lineno, rid, m.group(0), canonical, message))

    if not violations:
        n = sum(1 for _ in tracked_files())
        print(f"docs-consistency: clean ({n} files, {len(compiled)} rules)")
        return 0

    print(f"docs-consistency: {len(violations)} violation(s)\n")
    for rel, lineno, rid, found, canonical, message in violations:
        print(f"{rel}:{lineno}: [{rid}] found {found!r}")
        print(f"    canonical: {canonical}")
        print(f"    {message}\n")
    return 1


if __name__ == "__main__":
    sys.exit(main())
