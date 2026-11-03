#!/usr/bin/env python3
"""The rule rows moved out of AGENTS.md are unchanged.

A row is one line of a `| Rule | Enforced by |` table, not the header or the
separator. The multiset of those lines in `--before` must equal the multiset
in docs/agents/*.md, byte for byte. The whole line is the row.

    git show origin/main:AGENTS.md > /tmp/agents-before.md
    python3 scripts/agents_rows_check.py --before /tmp/agents-before.md

Python standard library only. Exit status 0 when the multisets match, 1
otherwise.
"""

import argparse
import collections
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def rows(text):
    """Table body lines, in order, header and separator excluded."""
    out = []
    for line in text.splitlines():
        if line.startswith("|") and not line.startswith("|---") and not line.startswith("| Rule"):
            out.append(line)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--before", required=True, help="the AGENTS.md the rows were taken from")
    ap.add_argument("--agents", default=str(ROOT / "docs" / "agents"), help="directory of area files")
    a = ap.parse_args()
    before = rows(pathlib.Path(a.before).read_text())
    after = []
    for path in sorted(pathlib.Path(a.agents).glob("*.md")):
        after.extend(rows(path.read_text()))
    if collections.Counter(before) == collections.Counter(after):
        print(f"agents-rows: {len(before)} rows, identical")
        return
    missing = collections.Counter(before) - collections.Counter(after)
    extra = collections.Counter(after) - collections.Counter(before)
    print(f"agents-rows: {len(missing)} row(s) missing from area files, {len(extra)} extra")
    for line, count in list(missing.items())[:5]:
        print(f"missing x{count}: {line[:120]}")
    for line, count in list(extra.items())[:5]:
        print(f"extra x{count}: {line[:120]}")
    sys.exit(1)


if __name__ == "__main__":
    main()
