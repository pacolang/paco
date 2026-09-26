#!/usr/bin/env python3
"""Defense-in-depth: the compiler and runtime name no library type.

The real guarantee is structural (RFC 0030 reasons 1-3, checked by
`check_std_scope.py` and the static-link CI job: no `#[builtin]`, no prelude
entry, no `extern`/`paco_rt_*` symbol names a library type). This script
catches the lexical case those miss: a library's public item name leaking into
a compiler diagnostic string or comment, which would tie the compiler to that
name without the compiler actually depending on the type.

The name list is generated from the public top-level items (`pub struct`,
`pub enum`, `pub trait`, `pub fn`) of every stdlib module `docs/ecosystem.md`
marks "marked for extraction" — the modules that are about to become official
libraries, and so the only ones with names a library will actually export.
Once a real library repository exists, its own pinned checkout replaces this
local source as the generator; nothing else here changes.

Deliberately dumb and syntactic, like check_docs_consistency.py: line-based
`//` and `/* */` comment stripping, then a whole-word grep. False positives are
suppressed with a reviewed entry (`path:line`) in
scripts/compiler_names_allowlist.txt, not by narrowing the check.

Exit 0 when clean, 1 with file:line for every unallowed hit.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ECOSYSTEM = ROOT / "docs" / "ecosystem.md"
ALLOWLIST = ROOT / "scripts" / "compiler_names_allowlist.txt"

EXTRACTED_MODULE_RE = re.compile(r"^\|\s*`([a-z_]+)`\s*\|.*marked for extraction", re.MULTILINE | re.IGNORECASE)
PUBLIC_ITEM_RE = re.compile(r"^pub (?:struct|enum|trait|fn) ([A-Za-z_][A-Za-z0-9_]*)", re.MULTILINE)
SOURCE_DIRS = ["compiler/*/src", "runtime/*/src"]


def extracted_names() -> set[str]:
    text = ECOSYSTEM.read_text(encoding="utf-8")
    names = set()
    for module in EXTRACTED_MODULE_RE.findall(text):
        path = ROOT / "stdlib" / f"{module}.paco"
        if path.is_file():
            names.update(PUBLIC_ITEM_RE.findall(path.read_text(encoding="utf-8")))
    return names


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return "\n".join(re.sub(r"//.*", "", line) for line in text.splitlines())


def source_files():
    for pattern in SOURCE_DIRS:
        for src_dir in sorted(ROOT.glob(pattern)):
            yield from sorted(src_dir.rglob("*.rs"))


def load_allowlist() -> set[str]:
    if not ALLOWLIST.is_file():
        return set()
    allowed = set()
    for line in ALLOWLIST.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            allowed.add(line)
    return allowed


def main() -> int:
    names = extracted_names()
    if not names:
        print("check_compiler_names: no names generated (no module marked for extraction?)", file=sys.stderr)
        return 1
    pattern = re.compile(r"\b(" + "|".join(re.escape(name) for name in sorted(names)) + r")\b")
    allowed = load_allowlist()

    problems = []
    for path in source_files():
        stripped = strip_comments(path.read_text(encoding="utf-8", errors="replace"))
        rel = path.relative_to(ROOT)
        for lineno, line in enumerate(stripped.splitlines(), 1):
            match = pattern.search(line)
            if match and f"{rel}:{lineno}" not in allowed:
                problems.append(f"{rel}:{lineno}: `{match.group(1)}` — a library's public item name in compiler/runtime source")

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} unallowed hit(s). If each is a real leak, fix it; "
              f"if it is a false positive, add `path:line` to {ALLOWLIST.relative_to(ROOT)} with a reason.",
              file=sys.stderr)
        return 1
    print(f"check_compiler_names: clean ({len(names)} names checked)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
