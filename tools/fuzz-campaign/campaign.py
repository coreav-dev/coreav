#!/usr/bin/env python3
"""Prepare and report a fuzz campaign (docs/design/fuzz-campaign.md).

  campaign.py prepare --out DIR --iso-fixtures DIR --mkv-fixtures DIR --hostile DIR
                      [--ts-fixtures DIR] [--corpus DIR ...] [--shards N] [--seconds S]
                      [--targets NAME,NAME]
      Builds every fuzz target of crates/*/fuzz (scripts/fuzz_plan.py --all),
      or the ones --targets names (a campaign for one bar),
      with `cargo +nightly fuzz build -O`, copies the binaries to DIR/bin,
      builds each target's seed set in DIR/seeds, and writes DIR/plan.json:
      the commit, the binaries' and seed sets' SHA-256, and the shards.
      --corpus (repeatable) names an earlier campaign's corpus directory:
      NAME.SHARD.tar.gz files as the launcher writes them, or NAME/
      directories (a minimized corpus, finding inputs); their units join
      each target's seeds, minimized together with libFuzzer's -merge=1.
      --shards and --seconds change the plan's shape (a short rehearsal).
  campaign.py report --plan DIR/plan.json --results RESULTS.json [--results MORE.json ...]
                     --out EVIDENCE.json
      Checks the shard results against the plan and writes the evidence
      totals: CPU-hours, executions, coverage and findings per target.
      `--results` may be given more than once: a shard the infrastructure
      lost is run again and its result joins the campaign, the later file
      winning where both hold the same shard.

  campaign.py pack --plan DIR/plan.json --results RESULTS.json --out ARCHIVE.tar.zst
      Minimizes each target's seeds and the units its shards added (the
      launcher's `corpus` directory next to RESULTS.json) with -merge=1, adds
      every finding's input, and packs them as corpus/NAME/ and
      findings/NAME/ for the next campaign's --corpus.

Run the shards between prepare and report with local_fuzz.py, or with the
cloud launcher kept with the project's operations, which writes the same
rows.
"""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
SHARDS = 5
SECONDS = 1800
MAX_SEED = 1 << 20
# AddressSanitizer's options for every run of a target, recorded in the plan
# for the launchers: a 64 MiB quarantine rather than the default 256 MiB
# (`docs/design/fuzz-campaign.md`, "AddressSanitizer options").
ASAN_OPTIONS = 'quarantine_size_mb=64'
TRIPLE = 'x86_64-unknown-linux-gnu'
ISO = ('.mp4', '.mov', '.m4a', '.m4v', '.3gp', '.3g2', '.m4b')
MKV = ('.mkv', '.webm', '.mka')
TS = ('.ts', '.m2ts', '.mts')


def asan_env(options):
    """The environment of a run of a target: `options` before any
    ASAN_OPTIONS already set, so that an operator's own options win."""
    own = os.environ.get('ASAN_OPTIONS')
    return {**os.environ, 'ASAN_OPTIONS': f'{options}:{own}' if own else options}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def targets(only=()):
    """Every fuzz target, or the named ones. A campaign for one bar runs
    the targets that bar is about (bar C4 of docs/design/h264.md, say);
    the release gate names none and runs them all."""
    out = subprocess.run([sys.executable, '-I', str(ROOT / 'scripts' / 'fuzz_plan.py'), '--all'],
                         capture_output=True, text=True, check=True).stdout
    all_targets = [tuple(line.split()) for line in out.splitlines() if line.strip()]
    if not only:
        return all_targets
    # A target name is not unique across projects (`decode` is a target of
    # coreav-audio and of coreav-h264), so a name may be qualified by its
    # crate: `coreav-h264/decode`.
    def names(project, target):
        return {target, f'{pathlib.Path(project).parent.name}/{target}'}

    chosen = [t for t in all_targets if names(*t) & set(only)]
    unknown = set(only) - {n for t in all_targets for n in names(*t)}
    if unknown:
        raise SystemExit(f'no such fuzz target: {", ".join(sorted(unknown))}')
    # A bare name that two projects answer to is refused rather than
    # expanded: a campaign must run the targets its bar names.
    for name in only:
        if '/' not in name:
            matches = [t for t in all_targets if t[1] == name]
            if len(matches) > 1:
                qualified = ', '.join(f'{pathlib.Path(p).parent.name}/{t}' for p, t in matches)
                raise SystemExit(f'the target {name} belongs to more than one project: {qualified}')
    return chosen


def files(directory, exts):
    if directory is None:
        return []
    return sorted(p for p in pathlib.Path(directory).rglob('*')
                  if p.is_file() and p.suffix.lower() in exts and p.stat().st_size <= MAX_SEED)


def hostile(directory):
    """The hostile suite's files (any name), except its manifest."""
    if directory is None:
        return []
    return sorted(p for p in pathlib.Path(directory).iterdir()
                  if p.is_file() and p.name != 'manifest.json' and p.stat().st_size <= MAX_SEED)


def text_samples():
    """3GPP timed text samples (3GPP TS 26.245 5.17): a 16-bit length, the
    UTF-8 text, then modifier boxes."""
    def sample(text, boxes=b''):
        data = text.encode()
        return len(data).to_bytes(2, 'big') + data + boxes
    # `styl` with one style record (start, end, font ID, face, size, RGBA).
    styl = (22).to_bytes(4, 'big') + b'styl' + (1).to_bytes(2, 'big') + bytes(
        [0, 0, 0, 5, 0, 1, 1, 18, 255, 255, 255, 255])
    hlit = (12).to_bytes(4, 'big') + b'hlit' + bytes([0, 0, 0, 3])
    return [('empty', sample('')), ('plain', sample('hello')),
            ('lines', sample('line one\nline two\r\n')), ('markup', sample('<b>a & b</b>')),
            ('styled', sample('hello', styl)), ('highlight', sample('hello', styl + hlit)),
            ('utf16', b'\0\x06\xfe\xff\0h\0i')]


def hls_indexes(iso, work):
    """HLS indexes (docs/design/hls.md) of the ISO base media fixtures, built
    with `coreav hls index`, as seeds for the index parser; a fixture the
    packager refuses gives none."""
    subprocess.run(['cargo', 'build', '--release', '-p', 'coreav-cli'], cwd=ROOT, check=True)
    coreav = ROOT / 'target' / 'release' / 'coreav'
    out = []
    # A fresh directory: `coreav` does not overwrite an index an earlier,
    # interrupted prepare left.
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    for path in iso:
        index = work / f'{path.name}.index'
        done = subprocess.run([str(coreav), 'hls', 'index', str(path), str(index)], capture_output=True)
        if done.returncode == 0 and index.exists() and index.stat().st_size <= MAX_SEED:
            out.append((index.name, index.read_bytes()))
    return out


def seeds_for(project, target, iso, mkv, bad, indexes=(), ts=()):
    """(name, bytes) seeds for one target: the files of its container with
    the target's option bytes in front (docs/design/fuzz-campaign.md)."""
    crate = pathlib.Path(project).parent.name
    if crate == 'coreav-text' and target == 'sample':
        return text_samples()
    if target == 'hls_index':
        return list(indexes)
    if crate == 'coreav-mpegts':
        sources = list(ts)
    elif crate == 'coreav-mkv' or target == 'mkv_remux':
        sources = mkv
    elif target == 'check' or crate == 'coreav-wasm':
        # The policy check reads Matroska and WebM too, and the WebAssembly
        # operations take either container.
        sources = iso + bad + mkv
    else:
        sources = iso + bad
    prefixes = {
        'clip': [b'\0\0\0'],
        'extract_audio': [b'\0'],
        'remux': [b'\0'],
        'frame_sizes': [b'\0'],
        'frame_access': [b'\0'],
        'compressed_movie': [b'\0'],
        # The first audio and 2 s segments, or no audio and 1 s segments.
        'hls': [b'\0\x14', b'\x01\x0a'],
        'block': [b'\0'],
        'mkv_remux': [b'\0'],
        'check': [b'\0', b'\1'],
        # Keep the default metadata, or nothing.
        'sanitize': [b'\0', b'\x80'],
        # The first audio; cuts every half second, or at 0.2 and 0.5 s.
        'split': [b'\0\x05\0', b'\0\x02\x03'],
        # The first text track.
        'subtitles': [b'\0'],
        # probe, extractAudio and clip with the first fixed options and no
        # injected failure, and probe with its fifth read failing.
        'ops': [b'\0\0', b'\1\0', b'\2\0', b'\0\x05'],
    }.get(target, [b''])
    out = []
    for path in sources:
        data = path.read_bytes()
        for prefix in prefixes:
            out.append((f'{path.name}.{prefix.hex() or "raw"}', prefix + data))
    if target == 'mkv_remux':
        # The fixed-header mode (bit 0x80) reads the input after a fixed
        # Segment header: the whole file there is a start the mutator can cut.
        for path in sources:
            out.append((f'{path.name}.80', b'\x80' + path.read_bytes()))
    return out


def carry_over(corpora, name, binary, seed_dir, work):
    """Adds earlier campaigns' corpus units for target `name` to its seeds:
    from each directory of `corpora`, every shard's tar file (NAME.SHARD.tar.gz)
    and the files of a NAME/ directory (a minimized corpus or finding
    inputs), minimized together with the seeds by libFuzzer's -merge=1
    (only units that add coverage are kept). Returns how many units it
    added."""
    if not corpora:
        return 0
    extra = work / name
    if extra.exists():
        shutil.rmtree(extra)
    extra.mkdir(parents=True)
    for number, corpus in enumerate(map(pathlib.Path, corpora)):
        for tar_path in sorted(corpus.glob(f'{name}.*.tar.gz')):
            with tarfile.open(tar_path) as tar:
                for member in tar.getmembers():
                    # Plain files with plain names only.
                    if not member.isfile() or '/' in member.name or member.name.startswith('.'):
                        continue
                    data = tar.extractfile(member).read()
                    (extra / f'{number}-{tar_path.name.split(".")[1]}-{member.name}').write_bytes(data)
        units = corpus / name
        if units.is_dir():
            for f in sorted(units.iterdir()):
                if f.is_file():
                    (extra / f'{number}-{f.name}').write_bytes(f.read_bytes())
    if not any(extra.iterdir()):
        return 0
    before = {sha256(p.read_bytes()) for p in seed_dir.iterdir()}
    merged = work / f'{name}.merged'
    if merged.exists():
        shutil.rmtree(merged)
    merged.mkdir()
    # The seeds and the new units together; every seed stays in the seed
    # directory whatever the merge keeps.
    subprocess.run([str(binary), '-merge=1', f'-max_len={MAX_SEED}', '-timeout=10',
                    '-rss_limit_mb=2048', str(merged), str(seed_dir), str(extra)], env=asan_env(ASAN_OPTIONS),
                   check=True, capture_output=True)
    # -merge=1 names its output by content; a unit is new when no seed has
    # its bytes.
    added = 0
    for f in sorted(merged.iterdir()):
        data = f.read_bytes()
        if sha256(data) not in before:
            (seed_dir / f'carried-{f.name}').write_bytes(data)
            added += 1
    return added


def cmd_prepare(args):
    out = pathlib.Path(args.out)
    (out / 'bin').mkdir(parents=True, exist_ok=True)
    commit = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True, text=True,
                            check=True).stdout.strip()
    dirty = subprocess.run(['git', 'status', '--porcelain', '--', 'crates', 'Cargo.lock', 'Cargo.toml'],
                           cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    if dirty and not args.allow_dirty:
        raise SystemExit('the crates have uncommitted changes; commit them first')
    # A name that matches no target fails here, before the seed sets are
    # built and the binaries compiled.
    chosen = targets(args.targets.split(',') if args.targets else ())
    iso = files(args.iso_fixtures, ISO)
    mkv = files(args.mkv_fixtures, MKV) + files(args.iso_fixtures, MKV)
    bad = hostile(args.hostile)
    ts = files(args.ts_fixtures, TS)
    shards, seconds = args.shards, args.seconds
    indexes = hls_indexes(iso, out / 'work' / 'hls')
    # The release resets the repository's history, so the commit will not
    # exist publicly: the tree, and the `crates` subtree (every crate and
    # fuzz project, their lock files included) with the root manifest and
    # lock file, which later documentation changes leave alone, identify
    # the fuzzed code.
    # The toolchain that built the binaries, to rebuild them later.
    def run(*argv):
        return subprocess.run(list(argv), cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    source = {'commit': commit, 'tree': run('git', 'rev-parse', 'HEAD^{tree}'),
              'crates_tree': run('git', 'rev-parse', 'HEAD:crates'),
              'cargo_toml_sha256': sha256((ROOT / 'Cargo.toml').read_bytes()),
              'cargo_lock_sha256': sha256((ROOT / 'Cargo.lock').read_bytes()),
              'rustc': run('rustc', '+nightly', '-V'), 'cargo_fuzz': run('cargo', 'fuzz', '--version')}
    plan = {**source, 'source': source,
            'shards_per_target': shards, 'seconds_per_shard': seconds,
            'libfuzzer': ['-timeout=10', '-rss_limit_mb=2048', f'-max_len={MAX_SEED}',
                          '-print_final_stats=1'],
            'asan_options': ASAN_OPTIONS,
            'seed_sources': {'iso_fixtures': len(iso), 'mkv_fixtures': len(mkv), 'ts_fixtures': len(ts),
                             'hostile': len(bad),
                             'hls_indexes': len(indexes)},
            'targets': []}
    built = set()
    for project, target in chosen:
        if project not in built:
            subprocess.run(['cargo', '+nightly', 'fuzz', 'build', '-O', '--target', TRIPLE],
                           cwd=ROOT / project, check=True)
            built.add(project)
        name = f'{pathlib.Path(project).parent.name}__{target}'
        binary = ROOT / project / 'target' / TRIPLE / 'release' / target
        shutil.copy2(binary, out / 'bin' / name)
        seeds = seeds_for(project, target, iso, mkv, bad, indexes, ts)
        seed_dir = out / 'seeds' / name
        if seed_dir.exists():
            shutil.rmtree(seed_dir)
        seed_dir.mkdir(parents=True)
        if not seeds:
            raise SystemExit(f'{name}: no seeds; a target that starts from nothing is not the planned campaign')
        for seed_name, data in sorted(seeds):
            (seed_dir / seed_name).write_bytes(data)
        carried = carry_over(args.corpus, name, out / 'bin' / name, seed_dir, out / 'work')
        digest = hashlib.sha256()
        seeds = sorted(seed_dir.iterdir())
        for path in seeds:
            digest.update(path.name.encode() + b'\0' + sha256(path.read_bytes()).encode())
        plan['targets'].append({
            'name': name, 'project': project, 'target': target,
            'binary_sha256': sha256((out / 'bin' / name).read_bytes()),
            'seeds': len(seeds), 'seeds_carried': carried, 'seeds_sha256': digest.hexdigest(),
            'shards': [{'shard': i, 'seed': 1000 + i, 'seconds': seconds} for i in range(shards)],
        })
    plan['cpu_hours'] = sum(s['seconds'] for t in plan['targets'] for s in t['shards']) / 3600
    (out / 'plan.json').write_text(json.dumps(plan, indent=1) + '\n')
    print(json.dumps({k: v for k, v in plan.items() if k != 'targets'}, indent=1))
    print(len(plan['targets']), 'targets')
    return 0


def shard_results(paths):
    """The shard results the launchers wrote: a list, or (from the gate
    tooling on) an object with the source's identity and `shards`.

    More than one file may be given: a shard the infrastructure lost is
    run again (`docs/design/fuzz-campaign.md`, "Findings and failures"),
    and its result joins the campaign rather than replacing it. The last
    result for a (target, shard) wins, so a re-run supersedes what it
    repeats, except over a finding: libFuzzer's runs are not repeatable,
    so a clean re-run does not show that a crash or hang was not there,
    and the row that found it stands."""
    if isinstance(paths, (str, pathlib.Path)):
        paths = [paths]
    by_shard, order = {}, []
    for path in paths:
        data = json.loads(pathlib.Path(path).read_text())
        for index, row in enumerate(data['shards'] if isinstance(data, dict) else data):
            # A row with no name (a launcher's error) keeps a key of its own.
            key = (row.get('name'), row.get('shard')) if 'name' in row else (path, index)
            if key not in by_shard:
                order.append(key)
            elif by_shard[key].get('finding'):
                continue
            by_shard[key] = row
    return [by_shard[k] for k in order]


def cmd_report(args):
    plan = json.loads(pathlib.Path(args.plan).read_text())
    # Every results file must come from the plan's build: shards of
    # another commit measure other code, and would join its hours.
    for path in args.results:
        data = json.loads(pathlib.Path(path).read_text())
        commit = data.get('commit') if isinstance(data, dict) else None
        if commit != plan['commit']:
            raise SystemExit(f'{path}: results of commit {commit}, '
                             f'but the plan is of {plan["commit"]}')
    results = shard_results(args.results)
    by_target, unnamed = {}, []
    for r in results:
        if 'name' in r:
            by_target.setdefault(r['name'], []).append(r)
        else:
            unnamed.append(r)
    rows, findings, total = [], [], 0.0
    for t in plan['targets']:
        shards = sorted(by_target.get(t['name'], []), key=lambda r: r['shard'])
        done = [s for s in shards if s.get('completed')]
        # A shard's hours are the CPU time the kernel charged it where the
        # launcher recorded that (`local_fuzz.py`), and its wall-clock
        # otherwise (a cloud container with a core to itself).
        hours = sum(s.get('cpu_seconds', s['seconds']) for s in done) / 3600
        total += hours
        for s in shards:
            if s.get('finding'):
                findings.append({'target': t['name'], 'shard': s['shard'], 'finding': s['finding'],
                                 'artifacts': [a['name'] for a in s.get('artifacts', [])]})
        rows.append({
            'target': t['name'], 'shards_completed': len(done), 'shards_planned': len(t['shards']),
            'cpu_hours': round(hours, 3),
            'wall_hours': round(sum(s['seconds'] for s in done) / 3600, 3),
            'executions': sum(s.get('executions') or 0 for s in done),
            'max_cov': max((s.get('cov') or 0 for s in done), default=0),
            'max_ft': max((s.get('ft') or 0 for s in done), default=0),
            'max_corpus': max((s.get('corpus') or 0 for s in done), default=0),
            'max_rss_mb': max((s.get('peak_rss_mb') or 0 for s in done), default=0),
            'findings': sum(1 for s in shards if s.get('finding')),
        })
    # The evidence names each finding's inputs; the inputs themselves (and
    # the log tails) stay out of the repository, with the corpus.
    shards = [{k: v for k, v in r.items() if k not in ('artifacts', 'log_tail')}
              | {'artifacts': [{'name': a['name'], 'size': a['size']} for a in r.get('artifacts', [])]}
              for r in results]
    launcher = {}
    for path in args.results:
        data = json.loads(pathlib.Path(path).read_text())
        if isinstance(data, dict):
            launcher.update({k: v for k, v in data.items()
                             if k != 'shards' and k not in plan.get('source', {})})
    evidence = {**plan.get('source', {'commit': plan['commit']}), **launcher,
                'plan': plan, 'shards': shards, 'targets': rows,
                'cpu_hours': round(total, 3), 'findings': findings,
                # Shards lost to the infrastructure (not findings; run again).
                'failed_shards': [{'target': r.get('name'), 'shard': r.get('shard'), 'error': r.get('error')}
                                  for r in results if r.get('error') and 'name' in r]
                                 + [{'target': None, 'shard': None, 'error': r.get('error')} for r in unnamed],
                'gate': {'cpu_hours_required': 24,
                         # Every planned shard of every target, not merely
                         # one: a shard with no result is a shard that may
                         # have been the one to find something.
                         'every_target': all(r['shards_completed'] == r['shards_planned'] for r in rows),
                         'passed': total >= 24 and not findings
                         and all(r['shards_completed'] == r['shards_planned'] for r in rows)}}
    pathlib.Path(args.out).write_text(json.dumps(evidence, indent=1) + '\n')
    print(json.dumps({'cpu_hours': evidence['cpu_hours'], 'findings': findings, 'gate': evidence['gate'],
                      'targets': rows}, indent=1))
    return 0


def cmd_pack(args):
    plan_path = pathlib.Path(args.plan)
    plan = json.loads(plan_path.read_text())
    campaign = plan_path.parent
    corpus = pathlib.Path(args.results).parent / 'corpus'
    results = shard_results([args.results])
    names = {t['name'] for t in plan['targets']}
    work = pathlib.Path(tempfile.mkdtemp(prefix='fuzz-pack-'))
    try:
        return pack(plan, campaign, corpus, results, names, work, pathlib.Path(args.out))
    finally:
        shutil.rmtree(work, ignore_errors=True)


def pack(plan, campaign, corpus, results, names, work, archive):
    root = work / 'fuzz-corpus'
    units = 0
    for t in plan['targets']:
        name = t['name']
        extra = work / 'extra' / name
        extra.mkdir(parents=True)
        for tar_path in sorted(corpus.glob(f'{name}.*.tar.gz')):
            shard = tar_path.name.split('.')[1]
            with tarfile.open(tar_path) as tar:
                for member in tar.getmembers():
                    if member.isfile() and '/' not in member.name and not member.name.startswith('.'):
                        (extra / f'{shard}-{member.name}').write_bytes(tar.extractfile(member).read())
        # A local run writes its units as files under corpus/NAME/ rather
        # than a tar per shard (`local_fuzz.py`), which `carry_over` already
        # reads; this reads it too, so a campaign can be packed whichever
        # launcher ran it.
        loose = corpus / name
        if loose.is_dir():
            for unit in sorted(loose.iterdir()):
                if unit.is_file() and not unit.name.startswith('.'):
                    (extra / f'local-{unit.name}').write_bytes(unit.read_bytes())
        dest = root / 'corpus' / name
        dest.mkdir(parents=True)
        subprocess.run([str(campaign / 'bin' / name), '-merge=1', f'-max_len={MAX_SEED}', '-timeout=10',
                        '-rss_limit_mb=2048', str(dest), str(campaign / 'seeds' / name), str(extra)],
                       env=asan_env(plan.get('asan_options', '')),
                       check=True, capture_output=True)
        units += sum(1 for _ in dest.iterdir())
    findings = 0
    for r in results:
        # Names from the plan, and plain file names only: the results come
        # from the containers.
        if r.get('name') not in names or not isinstance(r.get('shard'), int):
            continue
        for a in r.get('artifacts', []):
            dest = root / 'findings' / r['name']
            dest.mkdir(parents=True, exist_ok=True)
            (dest / f"{r['shard']}-{pathlib.PurePath(str(a['name'])).name}").write_bytes(
                base64.b64decode(a['base64']))
            findings += 1
    subprocess.run(['tar', '-I', 'zstd -19 -T0', '-cf', str(archive.resolve()), 'fuzz-corpus'], cwd=work, check=True)
    print(json.dumps({'archive': str(archive), 'bytes': archive.stat().st_size,
                      'sha256': sha256(archive.read_bytes()), 'units': units, 'finding_inputs': findings}))
    return 0


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    prepare = sub.add_parser('prepare')
    prepare.add_argument('--out', required=True)
    prepare.add_argument('--iso-fixtures')
    prepare.add_argument('--mkv-fixtures')
    prepare.add_argument('--ts-fixtures')
    prepare.add_argument('--hostile')
    prepare.add_argument('--corpus', action='append',
                         help="an earlier campaign's corpus directory (repeatable)")
    prepare.add_argument('--allow-dirty', action='store_true')
    prepare.add_argument('--targets', default='',
                         help='run only these targets, by name, comma separated '
                              '(a campaign for one bar rather than the release gate)')
    prepare.add_argument('--shards', type=int, default=SHARDS)
    prepare.add_argument('--seconds', type=int, default=SECONDS)
    report = sub.add_parser('report')
    report.add_argument('--plan', required=True)
    report.add_argument('--results', required=True, action='append',
                        help='a launcher\'s results; repeatable, so a re-run of '
                             'lost shards joins the campaign')
    report.add_argument('--out', required=True)
    pack = sub.add_parser('pack')
    pack.add_argument('--plan', required=True)
    pack.add_argument('--results', required=True)
    pack.add_argument('--out', required=True)
    args = parser.parse_args()
    return {'prepare': cmd_prepare, 'report': cmd_report, 'pack': cmd_pack}[args.command](args)


if __name__ == '__main__':
    sys.exit(main())
