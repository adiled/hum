#!/usr/bin/env python3
"""Check that the kernel speaks in code.

hum's rule: `//` and `///` are forbidden in the kernel's own Rust. `//!`
is the module header and is the only comment form allowed. The name of
every thing should carry its meaning, so a reader never has to scroll
past a wall of prose to find out what a function does.

The hives are exempt. Each one is a separate workspace and a kind any
third party may write in any language, so they are held to their own
community's conventions rather than ours.

Usage: python3 tools/speech.py [--fix-list]

Exits 1 when a forbidden comment is found.
"""

import re
import subprocess
import sys

EXEMPT_PREFIXES = ("hives/", "thrum-clients/")


def strip_literals(src):
    out = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]
        m = re.match(r'r(#*)"', src[i:])
        if m:
            hashes = m.group(1)
            end = src.find('"' + hashes, i + len(m.group(0)))
            end = n if end == -1 else end
            out.append(" " * (end - i))
            i = end + 1 + len(hashes)
            continue
        if src.startswith('b"', i) or src.startswith('br"', i):
            out.append(" ")
            i += 1
            continue
        if c == '"':
            out.append(" ")
            i += 1
            while i < n and src[i] != '"':
                if src[i] == "\\":
                    out.append(" ")
                    i += 1
                    if i < n:
                        out.append(" ")
                        i += 1
                    continue
                out.append("\n" if src[i] == "\n" else " ")
                i += 1
            if i < n:
                out.append(" ")
                i += 1
            continue
        if c == "'":
            j = i + 1
            j = j + 2 if (j < n and src[j] == "\\") else j + 1
            if j < n and src[j] == "'":
                out.append("''")
                i = j + 1
                continue
            out.append("'")
            i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def scan(src):
    found = []
    i = 0
    n = len(src)
    line = 1
    while i < n:
        c = src[i]
        if c == "\n":
            line += 1
            i += 1
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            depth = 1
            i += 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth += 1
                    i += 2
                    continue
                if src.startswith("*/", i):
                    depth -= 1
                    i += 2
                    continue
                if src[i] == "\n":
                    line += 1
                i += 1
            found.append((line, "/* block comment */"))
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = i
            while j < n and src[j] != "\n":
                j += 1
            text = src[i:j]
            if not text.startswith("//!"):
                found.append((line, text.strip()))
            i = j
            continue
        i += 1
    return found

def main():
    files = subprocess.run(
        ["git", "ls-files", "*.rs"], capture_output=True, text=True
    ).stdout.split()
    total = 0
    for path in files:
        if path.startswith(EXEMPT_PREFIXES):
            continue
        src = open(path, errors="ignore").read()
        found = scan(strip_literals(src))
        for lineno, text in found:
            print(f"{path}:{lineno}: {text[:100]}")
        total += len(found)
    if total:
        print(f"\n{total} comment(s) in the kernel. The kernel speaks in code.")
        return 1
    print("the kernel speaks in code — no comments")
    return 0

if __name__ == "__main__":
    sys.exit(main())
