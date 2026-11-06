"""Classify exact invocation sites using independently bound source evidence."""
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
                discovery.HISTORICAL_RECORD, 'scripts/oracle_invocation_check.py',
                'scripts/oracle_invocation_classify.py')
VERDICTS = ('confirmed-bypass', 'unknown', 'qualified-via-owner', 'excluded')
CLASSES = ('reviewed-reference', 'reviewed-exclusion', 'reviewed-bypass',
           'registered-source', 'bootstrap', 'runtime-boundary')
CLASS_VERDICTS = dict(zip(CLASSES, ('qualified-via-owner', 'excluded', 'confirmed-bypass',
                                  'excluded', 'excluded', 'excluded')))
SUFFIXES = {'.py', '.rs', '.sh', '.bash', '.yml', '.yaml', '.js', '.mjs', '.cjs', '.ts', '.tsx'}
DATA = {'.md', '.txt', '.json', '.toml', '.lock', '.csv', '.svg', '.png', '.jpg',
        '.jpeg', '.gif', '.webp', '.mp4', '.mkv', '.webm', '.m4a', '.wav', '.flac',
        '.opus', '.aac', '.h264', '.ivf', '.framemd5', '.env', '.css', '.html',
        '.xml', '.ttf', '.woff', '.woff2', '.pdf', '.snap'}
HEX = re.compile('[0-9a-f]{64}')


def digest(path):
    if path.is_symlink():
        raise ValueError('evidence source is a symbolic link: ' + str(path))
    return hashlib.sha256(path.read_bytes()).hexdigest()


def repository_paths(root):
    if (root / '.git').exists():
        paths = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z', '--cached',
                                         '--others', '--exclude-standard'], text=True).split('\0')
        return sorted(set(filter(None, paths)))
    return sorted(path.relative_to(root).as_posix() for path in root.rglob('*')
                  if (path.is_file() or path.is_symlink()) and '.git' not in path.parts)


def file_kind(path, root=None):
    if path.is_symlink() or root is not None and not path.resolve().is_relative_to(root):
        return 'unscanned-symlink'
    if not path.exists():
        return 'unscanned-missing'
    if not path.is_file():
        return 'unscanned-nonregular'
    if path.suffix in SUFFIXES:
        return 'source'
    with path.open('rb') as source:
        first = source.read(256)
    if first.startswith(b'#!'):
        if b'python' in first.split(b'\n', 1)[0]:
            return 'python'
        if re.search(rb'\b(?:ba|da|k|z)?sh\b', first.split(b'\n', 1)[0]):
            return 'shell'
        return 'unscanned-script'
    if path.stat().st_mode & 0o111:
        return 'unscanned-executable'
    if path.suffix in DATA or path.name.startswith('.') or path.name in ('LICENSE', 'NOTICE'):
        return 'data'
    return 'unscanned-file'


def file_inventory(root, paths=None):
    return {path: file_kind(root / path, root) for path in (paths or repository_paths(root))
            if path != RECORD and not path.startswith(discovery.HISTORICAL_BOUNDARY + '/')}


def site_key(site):
    return (site['line'], site['kind'], site.get('scope', '<unspecified>'), site['program'])


class Bindings:
    def __init__(self, root):
        self.root = root
        self.paths = repository_paths(root)
        self.path_set = set(self.paths)
        self.hashes = {}
        self.sources = {}
        self.imports = {}
        self.names = defaultdict(set)
        self.startup = set()
        for path in self.paths:
            file = Path(path)
            if file.suffix in ('.py', '.pyi', '.pyc', '.so', '.pyd'):
                self.names[file.name.split('.')[0]].add(path)
            for part in file.parts[:-1]:
                self.names[part].add(path)
            if file.suffix == '.pth' or file.stem in ('sitecustomize', 'usercustomize'):
                self.startup.add(path)

    def hash(self, path):
        if path not in self.hashes:
            self.hashes[path] = hashlib.sha256(self.source(path)).hexdigest()
        return self.hashes[path]

    def source(self, path):
        if path not in self.sources:
            selected = self.root / path
            if selected.is_symlink() or not selected.resolve().is_relative_to(self.root):
                raise ValueError('evidence source is a symbolic link')
            self.sources[path] = selected.read_bytes()
        return self.sources[path]

    def stems(self, path):
        if path not in self.imports:
            file = self.root / path
            values = set()
            if not file.is_symlink() and (file.suffix == '.py' or file_kind(file, self.root) == 'python'):
                for node in ast.walk(ast.parse(self.source(path).decode())):
                    if isinstance(node, ast.Import):
                        for item in node.names:
                            values.update(item.name.split('.'))
                    elif isinstance(node, ast.ImportFrom):
                        values.update((node.module or '').split('.'))
                        values.update(item.name for item in node.names if item.name != '*')
            self.imports[path] = values - {''}
        return self.imports[path]

    def context(self, path, requires=(), anchors=()):
        pending = [path, *requires, *anchors, *self.startup]
        reached, stems = set(), set()
        rust_witnesses = {}
        while pending:
            current = pending.pop()
            if current in reached:
                continue
            if current not in self.path_set:
                raise ValueError('missing evidence dependency: ' + current)
            reached.add(current)
            for stem in self.stems(current):
                stems.add(stem)
                pending.extend(self.names.get(stem, ()))
            file = self.root / current
            if file.suffix == '.rs':
                text = self.source(current).decode()
                for relative in re.findall(r'(?:include|include_str|include_bytes)!\(\s*"([^"]+)"\s*\)', text):
                    selected = (file.parent / relative).resolve()
                    if not selected.is_relative_to(self.root):
                        raise ValueError('Rust evidence dependency leaves the checkout')
                    pending.append(selected.relative_to(self.root).as_posix())
                for relative in re.findall(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]', text):
                    selected = (file.parent / relative).resolve()
                    if not selected.is_relative_to(self.root):
                        raise ValueError('Rust evidence dependency leaves the checkout')
                    pending.append(selected.relative_to(self.root).as_posix())
                for name in re.findall(r'\bmod\s+(\w+)\s*;', text):
                    candidates = [base / relative for base in (file.parent, file.parent / file.stem)
                                  for relative in (name + '.rs', name + '/mod.rs')]
                    candidates = sorted({candidate.relative_to(self.root).as_posix() for candidate in candidates})
                    present = [candidate for candidate in candidates if candidate in self.path_set]
                    pending.extend(present)
                    pairs = [(candidate, self.hash(candidate) if candidate in present else None)
                             for candidate in candidates]
                    rust_witnesses['rust:' + current + ':' + name] = hashlib.sha256(
                        json.dumps(pairs, separators=(',', ':')).encode()).hexdigest()
        witnesses = {}
        for stem in sorted(stems):
            candidates = sorted(set(self.names.get(stem, ())) | self.startup)
            pairs = [(candidate, self.hash(candidate)) for candidate in candidates]
            witnesses[stem] = hashlib.sha256(json.dumps(pairs, separators=(',', ':')).encode()).hexdigest()
        witnesses.update(rust_witnesses)
        return dict(sha256=self.hash(path), requires=sorted(reached - {path}),
                    imports=witnesses)


def safe_path(path):
    return (isinstance(path, str) and bool(Path(path).parts) and not Path(path).is_absolute()
            and '..' not in Path(path).parts and Path(path).as_posix() == path)


def validate_record(record):
    if record.get('schema') != 2:
        raise ValueError('rebuild the exact-site evidence catalogue with schema 2')
    if any(key in record for key in ('source_snapshot_sha256', 'sources', 'rules')):
        raise ValueError('replace global snapshots and regex rules with exact per-file evidence')
    for field in ('dependencies', 'frozen'):
        if not isinstance(record.get(field), dict):
            raise ValueError('restore evidence ' + field)
        for path, value in record[field].items():
            if not safe_path(path) or not isinstance(value, str) or not HEX.fullmatch(value):
                raise ValueError('restore an evidence path and SHA-256 digest')
    if not isinstance(record.get('inventory'), dict) or not isinstance(record.get('files'), dict):
        raise ValueError('restore the closed file inventory and per-file evidence')
    if not all(safe_path(path) and isinstance(kind, str) for path, kind in record['inventory'].items()):
        raise ValueError('restore the inventory paths and file kinds')
    for path, proof in record['files'].items():
        if not safe_path(path) or not HEX.fullmatch(proof.get('sha256', '')):
            raise ValueError('bind each evidence file to its source digest')
        if not isinstance(proof.get('requires'), list) or not all(safe_path(p) for p in proof['requires']):
            raise ValueError('restore explicit evidence dependencies')
        if not isinstance(proof.get('imports'), dict) or not all(
                isinstance(k, str) and isinstance(v, str) and HEX.fullmatch(v)
                for k, v in proof['imports'].items()):
            raise ValueError('restore the import candidate and absence witnesses')
        if any(p not in record['dependencies'] for p in proof['requires']):
            raise ValueError('record a digest for every required evidence source')
        seen = set()
        for entry in proof.get('sites', []):
            key = entry.get('site')
            if (not isinstance(key, list) or len(key) != 4 or not isinstance(key[0], int)
                    or key[0] < 1 or not all(isinstance(s, str) for s in key[1:])):
                raise ValueError('bind evidence to an exact line, kind, scope and program')
            identity = (*key, entry.get('occurrence', 0))
            if not isinstance(identity[-1], int) or identity[-1] < 0 or identity in seen:
                raise ValueError('remove duplicate exact-site evidence entries')
            seen.add(identity)
            if (entry.get('verdict') not in VERDICTS or entry['verdict'] == 'unknown'
                    or entry.get('class') not in CLASSES or not entry.get('evidence')):
                raise ValueError('restore a closed verdict, evidence class and explanation')
            if CLASS_VERDICTS[entry['class']] != entry['verdict']:
                raise ValueError('use the verdict belonging to the evidence class')
            if 'pattern' in entry or 'lines' in entry:
                raise ValueError('replace regex evidence with exact-site entries')
            if not isinstance(entry.get('fold_to', key[0]), int):
                raise ValueError('use a catalogued same-file fold origin')
        for entry in proof.get('sites', []):
            target = entry.get('fold_to', entry['site'][0])
            origins = [e for e in proof['sites'] if e['site'][0] == target]
            if not origins or any(VERDICTS.index(e['verdict']) > VERDICTS.index(entry['verdict'])
                                  for e in origins):
                raise ValueError('fold only to a same-file site with an equally severe verdict')


def classify(root, sites, record=None):
    if record is None:
        path = root / RECORD
        record = json.loads(path.read_text()) if path.is_file() else None
    if record is not None:
        validate_record(record)
    bindings = Bindings(root)
    inventory = file_inventory(root, bindings.paths)
    problems = []
    current_sites = list(sites)
    observed = {site['path'] for site in sites}
    for path in sorted(inventory):
        expected = record and record['inventory'].get(path)
        if expected != inventory[path] or inventory[path].startswith('unscanned'):
            if expected != inventory[path]:
                problems.append(dict(path=path, reason='unreviewed-inventory-file'))
            if path not in observed or inventory[path].startswith('unscanned'):
                current_sites.append(dict(path=path, line=1, kind='inventory', scope='<inventory>',
                                          program=inventory[path], status='unclassified'))
    if record:
        for path in record['inventory'].keys() - inventory.keys():
            problems.append(dict(path=path, reason='removed-inventory-file'))
    anchors_valid = bool(record)
    if record:
        if (root / '.git').exists() and not set(DEPENDENCIES) <= record['dependencies'].keys():
            anchors_valid = False
            problems.append(dict(path=RECORD, reason='missing-evidence-anchor'))
        for path, value in record['dependencies'].items():
            if path in DEPENDENCIES:
                try:
                    matches = bindings.hash(path) == value
                except (OSError, ValueError):
                    matches = False
                if not matches:
                    anchors_valid = False
                    problems.append(dict(path=path, reason='stale-evidence-anchor'))
        if (root / 'scripts/compat_sources.py').is_file():
            roster = discovery.frozen_files(root, bindings.paths)
            if set(record['frozen']) != roster:
                anchors_valid = False
                problems.append(dict(path=RECORD, reason='frozen-roster-mismatch'))
    proofs, valid = {}, {}
    for path, proof in (record or {}).get('files', {}).items():
        try:
            context = bindings.context(path, proof['requires'],
                                       [p for p in record['dependencies'] if p in DEPENDENCIES])
            matches = (anchors_valid and context == {key: proof[key] for key in context}
                       and all(bindings.hash(p) == record['dependencies'][p] for p in proof['requires']))
            if path in record['frozen']:
                matches = matches and proof['sha256'] == record['frozen'][path]
        except (OSError, ValueError, SyntaxError):
            matches = False
        valid[path] = matches
        if not matches:
            problems.append(dict(path=path, reason='stale-file-evidence'))
        proofs[path] = {(*entry['site'], entry.get('occurrence', 0)): entry
                        for entry in proof['sites']}
    occurrences = Counter()
    matched = defaultdict(set)
    docstrings = {}
    groups = {}
    for row, site in enumerate(current_sites):
        path, line = site['path'], site['line']
        key = site_key(site)
        occurrence = occurrences[(path, key)]
        occurrences[(path, key)] += 1
        identity = (*key, occurrence)
        entry = proofs.get(path, {}).get(identity)
        verdict, evidence_class, evidence = 'unknown', 'unmatched-site', 'no current exact-site evidence'
        folded_line = line
        if entry:
            matched[path].add(identity)
        required = set(site.get('requires', [])) - {path}
        if (entry and valid.get(path) and required <= set(record['files'][path]['requires'])):
            verdict, evidence_class, evidence = entry['verdict'], entry['class'], entry['evidence']
            folded_line = entry.get('fold_to', line)
            boundary = {'registered-source': 'frozen', 'bootstrap': 'bootstrap',
                        'runtime-boundary': 'runtime-boundary'}.get(evidence_class)
            if boundary and (site.get('status') != boundary or
                             boundary == 'frozen' and path not in record['frozen']):
                verdict, evidence_class = 'unknown', 'boundary-class-mismatch'
            if verdict == 'qualified-via-owner' and re.fullmatch(r"""['"](?:ffmpeg|ffprobe)['"]""", site['program']):
                verdict, evidence_class = 'unknown', 'literal-reference-is-not-owner'
            if site.get('program_span'):
                span = site['program_span']
                raw = bindings.source(path)
                if (entry.get('program_span') != span or
                        raw[span['start']:span['end']].decode() != site['program']):
                    verdict, evidence_class = 'unknown', 'source-span-mismatch'
        elif site['kind'] == 'embedded-source' and path.endswith('.py'):
            if path not in docstrings:
                tree = ast.parse((root / path).read_text())
                docstrings[path] = {node.body[0].value.lineno for node in ast.walk(tree)
                                   if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef))
                                   and node.body and isinstance(node.body[0], ast.Expr)
                                   and isinstance(node.body[0].value, ast.Constant)
                                   and isinstance(node.body[0].value.value, str)}
            if line in docstrings[path]:
                verdict, evidence_class, evidence = 'excluded', 'python-docstring', 'AST docstring data'
        origin = (path, folded_line)
        if origin not in groups:
            groups[origin] = dict(path=path, line=folded_line, verdict=verdict, rule=evidence_class,
                                  evidence=evidence, rows=[], patterns=set())
        group = groups[origin]
        group['rows'].append(row)
        group['patterns'].add(evidence_class)
        if VERDICTS.index(verdict) < VERDICTS.index(group['verdict']):
            group.update(verdict=verdict, rule=evidence_class, evidence=evidence)
    for path, entries in proofs.items():
        if entries.keys() - matched[path]:
            problems.append(dict(path=path, reason='dead-site-evidence'))
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
    return dict(schema=2, evidence_valid=bool(record) and not problems, counts=counts,
                problems=problems,
                top_patterns=[dict(rule=rule, locations=count,
                              files=len({item['path'] for item in locations if item['rule'] == rule}))
                              for rule, count in patterns.most_common()],
                locations=locations,
                reconciliation=dict(input_rows=len(current_sites), discovered_rows=len(sites),
                                    closure_rows=len(current_sites) - len(sites), mapped_rows=len(row_ids),
                                    exact=row_ids == list(range(len(current_sites)))),
                proof='exact source-bound evidence; unknown, stale evidence and bypass fail')


def refresh(root, sites, record, selected):
    validate_record(record)
    bindings = Bindings(root)
    rows = defaultdict(list)
    for site in sites:
        if site['path'] in selected:
            rows[site['path']].append(list(site_key(site)))
    changes = {}
    for path in selected:
        if not safe_path(path):
            raise ValueError('refresh only checkout-relative evidence paths')
        previous = record['files'].get(path, {})
        old = [entry['site'] for entry in previous.get('sites', [])]
        current = rows[path]
        current_hash = bindings.hash(path) if path in bindings.paths else None
        unchanged = current_hash == previous.get('sha256')
        if unchanged and previous:
            try:
                context = bindings.context(path, previous['requires'],
                    [value for value in record['dependencies'] if value in DEPENDENCIES])
                unchanged = (context == {key: previous[key] for key in context} and all(
                    bindings.hash(value) == record['dependencies'][value] for value in previous['requires']))
            except (OSError, ValueError, SyntaxError):
                unchanged = False
        changes[path] = dict(previous_sha256=previous.get('sha256'),
                             current_sha256=current_hash,
                             requires_review=True, current_sites=current,
                             carried_sites=[site for site in current if unchanged and site in old],
                             added_sites=[site for site in current if site not in old],
                             changed_sites=[site for site in current if not unchanged and site in old],
                             removed_sites=[site for site in old if site not in current])
    affected = {path: sorted(record['files']) if path in DEPENDENCIES else sorted(
        file for file, proof in record['files'].items() if path in proof['requires']) for path in selected}
    return dict(kind='unreviewed-evidence-proposal', files=changes, dependents=affected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=discovery.ROOT)
    parser.add_argument('--json', type=Path, required=True)
    parser.add_argument('--refresh', nargs='+', metavar='PATH')
    args = parser.parse_args()
    root = args.root.resolve()
    if args.json.resolve().is_relative_to(root):
        parser.error('write reports and proposals outside the checkout')
    sites = discovery.inventory(root)
    if args.refresh:
        report = refresh(root, sites, json.loads((root / RECORD).read_text()), args.refresh)
    else:
        report = classify(root, sites)
    args.json.write_text(json.dumps(report, indent=2) + '\n')
    if args.refresh:
        print('unreviewed proposal written; the catalogue is unchanged')
        return 0
    print(json.dumps({key: report[key] for key in ('evidence_valid', 'counts', 'problems',
                                                  'top_patterns', 'reconciliation')}, sort_keys=True))
    if not report['reconciliation']['exact'] or not report['evidence_valid']:
        return 2
    return int(bool(report['counts']['confirmed-bypass']['rows'] or report['counts']['unknown']['rows']))


if __name__ == '__main__':
    raise SystemExit(main())
