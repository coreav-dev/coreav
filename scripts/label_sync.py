#!/usr/bin/env python3
"""Makes a repository's labels match .github/labels.yml
(docs/design/ci-checks.md, "Labels").

Every label under `labels` is created, or updated to the file's colour and
description; every label under `retired` is deleted where it exists. A
label in neither list is left alone and named in the output, so the labels
a workflow creates for itself survive.

    label_sync.py --check [--file FILE]
    label_sync.py --repo OWNER/NAME [--file FILE] [--dry-run]

`--check` reads and validates the file only, with no network: names unique
(GitHub compares them without case), a colour of six hex digits, a
description of at most 100 characters, no name in both lists. The sync runs
`gh`, which reads its token from GH_TOKEN. Exit status 0 on success, 1 on
a file the parser refuses or a failed `gh` call. Reads the file as text in
the narrow shape it is written in; anything else is an error rather than a
silent pass. Python 3.11 standard library only.
"""
import argparse
import json
import pathlib
import re
import subprocess
import sys

SECTION = re.compile(r'\A(labels|retired):\s*\Z')
FIRST = re.compile(r'\A  - name: (\S.*?)\s*\Z')
MORE = re.compile(r'\A    (color|description): (\S.*?)\s*\Z')
RETIRED = re.compile(r'\A  - (\S.*?)\s*\Z')
COLOR = re.compile(r'\A[0-9a-f]{6}\Z')


def parse(text):
    """({'name', 'color', 'description'} for each label, [retired name]),
    or SystemExit naming the first line it cannot read."""
    labels, retired, section = [], [], None
    for n, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        match = SECTION.match(line)
        if match:
            section = match.group(1)
            continue
        if section == 'labels':
            match = FIRST.match(line)
            if match:
                labels.append({'name': match.group(1)})
                continue
            match = MORE.match(line)
            if match and labels and match.group(1) not in labels[-1]:
                labels[-1][match.group(1)] = match.group(2)
                continue
        elif section == 'retired':
            match = RETIRED.match(line)
            if match:
                retired.append(match.group(1))
                continue
        raise SystemExit(f'label-sync: line {n}: not in the shape this script reads')
    return labels, retired


def problems_of(labels, retired):
    out, seen = [], set()
    if not labels:
        out.append('no label under `labels`')
    for label in labels:
        name = label['name']
        if name.lower() in seen:
            out.append(f'`{name}` is listed twice')
        seen.add(name.lower())
        if not COLOR.match(label.get('color', '')):
            out.append(f'`{name}`: the color is not six lowercase hex digits')
        description = label.get('description', '')
        if not description or len(description) > 100:
            out.append(f'`{name}`: the description is missing or longer than 100 characters')
    for name in retired:
        if name.lower() in seen:
            out.append(f'`{name}` is both a label and retired')
        seen.add(name.lower())
    return out


def gh(*args):
    return subprocess.run(['gh', *args], check=True, capture_output=True, text=True).stdout


def sync(repo, labels, retired, dry_run):
    existing = {label['name'].lower(): label for label in json.loads(
        gh('label', 'list', '--repo', repo, '--limit', '1000', '--json', 'name,color,description'))}
    for label in labels:
        now = existing.get(label['name'].lower())
        if now and (now['name'], now['color'].lower(), now['description']) == (
                label['name'], label['color'], label['description']):
            continue
        print(f"{'update' if now else 'create'} {label['name']}")
        if not dry_run:
            gh('label', 'create', label['name'], '--repo', repo, '--force',
               '--color', label['color'], '--description', label['description'])
    for name in retired:
        if name.lower() in existing:
            print(f'delete {name}')
            if not dry_run:
                gh('label', 'delete', name, '--repo', repo, '--yes')
    known = {label['name'].lower() for label in labels} | {name.lower() for name in retired}
    for key, label in sorted(existing.items()):
        if key not in known:
            print(f"leave {label['name']} (not in the file)")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--file', type=pathlib.Path,
                        default=pathlib.Path(__file__).resolve().parents[1] / '.github' / 'labels.yml')
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--check', action='store_true', help='validate the file only')
    mode.add_argument('--repo', help='OWNER/NAME of the repository to sync')
    parser.add_argument('--dry-run', action='store_true', help='print what would change, change nothing')
    args = parser.parse_args()
    labels, retired = parse(args.file.read_text(encoding='utf-8'))
    problems = problems_of(labels, retired)
    for problem in problems:
        print(f'label-sync: {problem}', file=sys.stderr)
    if problems:
        return 1
    if args.check:
        print(f'label-sync: {len(labels)} labels and {len(retired)} retired, the file reads cleanly')
        return 0
    try:
        sync(args.repo, labels, retired, args.dry_run)
    except subprocess.CalledProcessError as error:
        print(f'label-sync: `gh {" ".join(error.cmd[1:3])}` failed: {error.stderr.strip()}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
