#!/usr/bin/env python3
"""Check that the kernel writes only where the layout says.

hum's rule: every path under a hum root comes from hum-paths, and every
write there goes through `hum_paths::write_atomic`, which stages in the
root's `tmp/`. While humd runs, its guard moves whatever the layout does
not declare into that `tmp/`; this check stops the kernel from writing
such things in the first place. Outside hum-paths, kernel Rust may not:

  - stage a write beside its target (`with_extension("json.tmp")`, a
    pushed ".tmp"), which leaves an undeclared name under a root;
  - read an XDG_* variable, which builds a root by hand;
  - spell a hum directory as a string (".local/state", ".config/hum");
  - join onto a root (`state_dir().join(...)`), which invents an entry
    the layout does not declare.

The hives and the clients are exempt, as they are from speech.py: each
is its own workspace, and their writes land in their own sandbox.

Usage: python3 tools/paths.py

Exits 1 when a violation is found.
"""

import re
import subprocess
import sys

EXEMPT_PREFIXES = ("hum-paths/", "hives/", "thrum-clients/")

RULES = [
    (
        re.compile(r'with_extension\("[^"]*tmp"\)|\.push\("\.tmp"\)|"\{[^"}]*\}\.tmp"'),
        "stages a write beside its target; use hum_paths::write_atomic",
    ),
    (
        re.compile(r'env::var(?:_os)?\(\s*"XDG_'),
        "reads an XDG variable; ask hum_paths for the directory",
    ),
    (
        re.compile(r'"[^"]*(?:\.local/state|\.local/share|\.config/hum|\.cache/hum)'),
        "spells a hum directory; ask hum_paths for it",
    ),
    (
        re.compile(r"\b(?:state_dir|config_dir|data_dir|cache_dir|runtime_dir)\(\)\s*\.join\("),
        "joins onto a hum root; declare the path in hum_paths",
    ),
]


def code_lines(src):
    for lineno, line in enumerate(src.splitlines(), start=1):
        stripped = line.lstrip()
        if stripped.startswith("//"):
            continue
        yield lineno, line


def main():
    files = subprocess.run(
        ["git", "ls-files", "*.rs"], capture_output=True, text=True
    ).stdout.split()
    found = 0
    for path in files:
        if path.startswith(EXEMPT_PREFIXES):
            continue
        src = open(path, errors="ignore").read()
        for lineno, line in code_lines(src):
            for pattern, why in RULES:
                if pattern.search(line):
                    print(f"{path}:{lineno}: {why}\n    {line.strip()[:110]}")
                    found += 1
    if found:
        print(f"\n{found} write(s) outside the layout. The kernel writes where hum-paths says.")
        return 1
    print("the kernel writes where hum-paths says")
    return 0


if __name__ == "__main__":
    sys.exit(main())
