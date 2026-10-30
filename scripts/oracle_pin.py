#!/usr/bin/env python3
"""The oracle FFmpeg pin (scripts/oracle-ffmpeg.env), for Python readers,
and the check that it stays the only one.

`load()` returns the pin's NAME=value lines as a dict. With `--check`, fails,
naming the file and line, when a tracked file pins an FFmpeg of its own.
Records are left out: docs/ (past rounds and the prose that cites them),
Markdown, and `evidence/` and `results/` directories. A second pin is:

- a download URL of an FFmpeg build or release archive;
- an FFmpeg archive name with a version (`ffmpeg-7.0.2-amd64-static`);
- a cache key or a `VERSION` assignment naming an FFmpeg version;
- an x264 commit in a path, a cache key or a `COMMIT` assignment;
- one of the pin's own values (its build, archive, SHA-256, the LGPL
  build's SHA-256 or x264 commit), which only scripts/oracle-ffmpeg.env
  may hold.

The fix it names: read the value from scripts/oracle-ffmpeg.env
(`source` in shell, `oracle_pin.load()` in Python, `hashFiles` of the file
in a workflow's cache key).

    oracle_pin.py --check [--root DIR]
"""
import argparse
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PIN = 'scripts/oracle-ffmpeg.env'
OWN = ('scripts/oracle_pin.py', 'scripts/tests/test_oracle_pin.py')
LINE = re.compile(r'\A([A-Z][A-Z0-9_]*)=(\S+)\Z')
VERSION = r'n?\d+\.\d+(?:\.\d+)?'
PATTERNS = (
    ('an FFmpeg download URL', re.compile(
        r'johnvansickle\.com/ffmpeg|BtbN/FFmpeg-Builds|ffmpeg\.org/releases/', re.I)),
    ('an FFmpeg version in a cache key', re.compile(rf'\bkey:.*\bffmpeg-{VERSION}\b', re.I)),
    ('an FFmpeg archive name', re.compile(rf'\bffmpeg-{VERSION}[-.]', re.I)),
    ('an FFmpeg version assignment', re.compile(
        r'''\bVERSION\s*=\s*['"]?n?\d+\.\d+\.\d+\b|\bFFMPEG_VERSION\s*=\s*['"]?n?\d+\.\d+\b''')),
    ('an x264 commit', re.compile(
        r'''\bx264-[0-9a-f]{7,40}\b|\A\s*(?:X264_)?COMMIT\s*=\s*['"]?[0-9a-f]{7,40}\b''')),
)


def load(root=ROOT):
    pin = {}
    for line in (root / PIN).read_text().splitlines():
        if line and not line.startswith('#'):
            m = LINE.match(line)
            if not m:
                raise SystemExit(f'{PIN}: not NAME=value: {line!r}')
            pin[m.group(1)] = m.group(2)
    return pin


def tracked(root):
    out = subprocess.run(['git', '-C', str(root), 'ls-files', '-z'], check=True,
                         capture_output=True).stdout.decode()
    for rel in filter(None, out.split('\0')):
        parts = rel.split('/')
        if rel != PIN and rel not in OWN and parts[0] != 'docs' and not rel.endswith('.md') \
                and 'evidence' not in parts and 'results' not in parts:
            yield rel


def problems(root):
    pin = load(root)
    own = [re.compile(re.escape(pin[name])) for name in ('BUILD', 'ARCHIVE', 'SHA256', 'LAV_SHA256', 'X264_COMMIT')]
    own.append(re.compile(rf'\b{re.escape(pin["X264_COMMIT"][:7])}\b'))
    for rel in tracked(root):
        try:
            text = (root / rel).read_text()
        except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
            continue
        for number, line in enumerate(text.splitlines(), 1):
            what = next((w for w, p in PATTERNS if p.search(line)), None)
            if what is None and any(p.search(line) for p in own):
                what = 'a value of the pin'
            if what:
                yield f'{rel}:{number}: {what}; read it from {PIN} instead: {line.strip()[:120]}'


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--check', action='store_true', required=True)
    parser.add_argument('--root', type=pathlib.Path, default=ROOT)
    args = parser.parse_args()
    found = list(problems(args.root))
    for problem in found:
        print(problem, file=sys.stderr)
    if found:
        print(f'oracle_pin.py: {len(found)} second pin(s) of the oracle FFmpeg', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
