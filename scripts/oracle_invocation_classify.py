"""Classify source-bound invocation patterns; unmatched evidence remains failed."""
import argparse
import ast
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_invocation_check as discovery

RECORD = 'docs/design/oracle-invocation-dispositions.json'
DEPENDENCIES = ('scripts/test_oracle.py', 'test-support/oracle.rs',
                'scripts/oracle_pin.py', 'scripts/oracle_pin_macos.py',
                'scripts/oracle-ffmpeg.env', 'scripts/oracle-ffmpeg-macos.env',
                discovery.HISTORICAL_RECORD)
VERDICTS = ('confirmed-bypass', 'unknown', 'qualified-via-owner', 'excluded')


def digest(path):
    if path.is_symlink():
        raise ValueError('invocation proof source must not be a symbolic link: ' + str(path))
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_snapshot(root):
    if (root / '.git').exists():
        paths = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z', '--cached',
                                         '--others', '--exclude-standard'], text=True).split('\0')
    else:
        paths = [path.relative_to(root).as_posix() for path in root.rglob('*') if path.is_file()]
    selected = {path for path in paths if path and path != RECORD
                and not path.startswith(discovery.HISTORICAL_BOUNDARY + '/')}
    hashes = {}
    for path in sorted(selected):
        file = root / path
        if file.is_symlink():
            target = file.resolve(strict=True)
            if not target.is_relative_to(root) or not target.is_file():
                raise ValueError('invocation input link must name a repository file: ' + path)
            value = 'link:' + str(file.readlink()) + ':' + digest(target)
            hashes[path] = hashlib.sha256(value.encode()).hexdigest()
        else:
            hashes[path] = digest(file)
    return hashlib.sha256(json.dumps(hashes, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def validate_record(record):
    if record.get('schema') != 1 or not re.fullmatch('[0-9a-f]{64}', record.get('source_snapshot_sha256', '')):
        raise ValueError('restore the invocation disposition schema and source snapshot')
    for field in ('sources', 'dependencies', 'frozen'):
        values = record.get(field)
        if not isinstance(values, dict):
            raise ValueError('restore invocation ' + field)
        for path, value in values.items():
            if Path(path).is_absolute() or '..' in Path(path).parts or not re.fullmatch('[0-9a-f]{64}', value):
                raise ValueError('invalid invocation proof path or digest: ' + path)
    for rule in record.get('rules', []):
        if rule.get('verdict') not in ('confirmed-bypass', 'qualified-via-owner', 'excluded'):
            raise ValueError('restore a closed invocation verdict')
        if rule.get('path') not in record['sources'] or not rule.get('evidence'):
            raise ValueError('bind every invocation rule to source bytes and an explanation')
        if not rule.get('lines') or not all(isinstance(line, int) and line > 0 for line in rule['lines']):
            raise ValueError('bind every invocation rule to concrete source locations')
        if not rule.get('kinds') or not isinstance(rule.get('pattern'), str):
            raise ValueError('bind every invocation rule to its call pattern')
        re.compile(rule['pattern'])


def classify(root, sites, record=None):
    if record is None:
        path = root / RECORD
        record = json.loads(path.read_text()) if path.is_file() else None
    if record is not None:
        validate_record(record)
    source_hashes = {}
    def current(path):
        if path not in source_hashes:
            source_hashes[path] = digest(root / path)
        return source_hashes[path]
    trusted = bool(record) and source_snapshot(root) == record['source_snapshot_sha256']
    if trusted:
        trusted = all(current(path) == value for path, value in record['dependencies'].items())
    rules = defaultdict(list)
    if record:
        for rule in record['rules']:
            rules[rule['path']].append(rule)
    docstrings = {}
    groups = {}
    for row, site in enumerate(sites):
        path, line = site['path'], site['line']
        verdict, pattern, evidence = 'unknown', 'unmatched-call-pattern', 'no source-bound disposition'
        folded_line = line
        if site['status'] == 'frozen' and record and record['frozen'].get(path) == current(path):
            verdict, pattern, evidence = 'excluded', 'registered-source', 'unchanged registered source bytes'
        elif site['status'] in ('bootstrap', 'runtime-boundary') and trusted:
            verdict, pattern, evidence = 'excluded', site['status'], 'explicit non-reference execution boundary'
        elif site['kind'] == 'embedded-source' and path.endswith('.py'):
            if path not in docstrings:
                tree = ast.parse((root / path).read_text())
                docstrings[path] = {node.body[0].value.lineno for node in ast.walk(tree)
                                   if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef))
                                   and node.body and isinstance(node.body[0], ast.Expr)
                                   and isinstance(node.body[0].value, ast.Constant)
                                   and isinstance(node.body[0].value.value, str)}
            if line in docstrings[path]:
                verdict, pattern, evidence = 'excluded', 'python-docstring', 'AST docstring data, not a process launch'
        if verdict == 'unknown' and trusted and record['sources'].get(path) == current(path):
            matches = [rule for rule in rules[path] if line in rule['lines'] and site['kind'] in rule['kinds']
                       and re.fullmatch(rule['pattern'], site['program'])]
            if matches and len({rule['verdict'] for rule in matches}) == 1:
                verdict = matches[0]['verdict']
                pattern = matches[0]['id']
                evidence = matches[0]['evidence']
                folded_line = matches[0].get('fold_to', {}).get(str(line), line)
        key = (path, folded_line)
        if key not in groups:
            groups[key] = dict(path=path, line=folded_line, verdict=verdict, rule=pattern,
                               evidence=evidence, rows=[], patterns=set())
        group = groups[key]
        group['rows'].append(row)
        group['patterns'].add(pattern)
        if VERDICTS.index(verdict) < VERDICTS.index(group['verdict']):
            group.update(verdict=verdict, rule=pattern, evidence=evidence)
    locations = []
    for value in groups.values():
        value['patterns'] = sorted(value['patterns'])
        locations.append(value)
    counts = {}
    for verdict in VERDICTS:
        selected = [item for item in locations if item['verdict'] == verdict]
        counts[verdict] = dict(locations=len(selected), files=len({item['path'] for item in selected}),
                               rows=sum(len(item['rows']) for item in selected))
    patterns = Counter(item['rule'] for item in locations if item['verdict'] == 'confirmed-bypass')
    row_ids = sorted(row for item in locations for row in item['rows'])
    return dict(schema=1, source_snapshot_valid=trusted, counts=counts,
                top_patterns=[dict(rule=rule, locations=count,
                              files=len({item['path'] for item in locations if item['rule'] == rule}))
                              for rule, count in patterns.most_common()],
                locations=locations,
                reconciliation=dict(input_rows=len(sites), mapped_rows=len(row_ids),
                                    exact=row_ids == list(range(len(sites)))),
                proof='source-bound patterns only; unknown and bypass fail; the bypass count is a lower bound while unknown remains')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=discovery.ROOT)
    parser.add_argument('--json', type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve()
    if args.json.resolve().is_relative_to(root):
        parser.error('write the report outside the checkout so it cannot change the input snapshot')
    report = classify(root, discovery.inventory(root))
    args.json.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ('source_snapshot_valid', 'counts', 'top_patterns', 'reconciliation')}, sort_keys=True))
    if not report['reconciliation']['exact']:
        return 2
    return int(bool(report['counts']['confirmed-bypass']['rows'] or report['counts']['unknown']['rows']))


if __name__ == '__main__':
    raise SystemExit(main())
