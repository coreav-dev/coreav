"""Project the saved presence sample into the registered counts-only set."""
import argparse
import csv
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SET = HERE / 'github-shapes-200' / 'shapes.jsonl'
DIGEST = '0ca6bcb9b983ade0625f795bd5098cffa12ee638d5ddf94a37c1cefb782d9045'
SOURCE = '700cfe814c1442a88a85a69f9513636ecd9f2adb70d86621cb58c0ff51b7483f'


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'))


def encoded(rows):
    return ''.join(canonical(row) + '\n' for row in rows).encode()


def validate(rows):
    if len(rows) != 200 or [r['rank'] for r in rows] != list(range(1, 201)):
        raise ValueError('set_roster')
    if len({r['shape_id'] for r in rows}) != 200:
        raise ValueError('set_duplicates')
    if sum(r['repository_count'] for r in rows) != 1470:
        raise ValueError('set_weights')
    if hashlib.sha256(encoded(rows)).hexdigest() != DIGEST:
        raise ValueError('set_digest')
    return rows


def load():
    data = SET.read_bytes()
    if hashlib.sha256(data).hexdigest() != DIGEST:
        raise ValueError('set_bytes')
    return validate([json.loads(line) for line in data.splitlines()])


def build(source, top):
    data = source.read_bytes()
    if hashlib.sha256(data).hexdigest() != SOURCE:
        raise ValueError('source_digest')
    original = [json.loads(line) for line in data.splitlines()][:200]
    rows = [{k: row[k] for k in ('rank', 'shape_id', 'repository_count', 'shape')} for row in original]
    with top.open() as handle:
        ranking = list(csv.DictReader(handle))
    if [(r['shape_id'], int(r['repository_count'])) for r in ranking] != [
            (r['shape_id'], r['repository_count']) for r in rows]:
        raise ValueError('ranking_differs')
    return validate(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path)
    parser.add_argument('--top', type=Path)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    if args.source:
        if not args.top:
            parser.error('--source requires --top')
        data = encoded(build(args.source, args.top))
        if args.check:
            if SET.read_bytes() != data:
                raise ValueError('set_differs')
        else:
            SET.parent.mkdir(exist_ok=True)
            SET.write_bytes(data)
            (SET.parent / 'SHA256SUMS').write_text(DIGEST + '  shapes.jsonl\n')
    else:
        load()
    print('github-shapes-200: 200 shapes, 1470 repository-shape pairs')


if __name__ == '__main__':
    main()
