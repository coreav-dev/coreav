#!/usr/bin/env python3
"""Untimed, deliberately pixel-inexact instruction ceiling for the VP9 pixel round."""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import resource
import subprocess
import sys
import tarfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PLAN = ROOT / 'docs/measurements/vp9-pixel-round/plan.json'
INDEXES = (0, 2, 4)
PACKETS = 8
ARMS = ('baseline', 'ceiling')


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def read(path):
    return json.loads(pathlib.Path(path).read_text())


def write(path, value):
    pathlib.Path(path).write_text(json.dumps(value, indent=2) + '\n')


def digest(path):
    h = hashlib.sha256()
    with pathlib.Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def selection():
    plan = read(PLAN)
    roster = read(ROOT / 'docs/measurements/vp9-native/roster.json')
    inputs = plan['inputs']
    require(len(inputs) == 6 and len({r['sha256'] for r in inputs}) == 6,
            'incomplete or duplicate development roster')
    require(all(r['sha256'] in roster['development'] and r['sha256'] not in roster['held_out']
                for r in inputs), 'ceiling inputs must belong to development only')
    require([r['class'] for r in inputs] == ['720p', '720p', '1080p', '1080p', '4K', '4K'],
            'resolution order differs from the plan')
    return plan, [inputs[i] for i in INDEXES]


def replace_body(text, name, body):
    start = text.index(f'fn {name}')
    opening = text.index(' {\n', start) + 1
    depth = 1
    end = opening + 1
    while depth and end < len(text):
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    require(depth == 0, 'unclosed scratch function')
    return text[:opening + 1] + '\n' + body + '\n' + text[end - 1:]


def patch_source(source, ceiling):
    path = source / 'tools/decode-oracle/src/main.rs'
    path.write_text(replace_body(path.read_text(), 'drain', '''    while let Some(frame) = decoder.receive_frame().map_err(|e| e.to_string())? {
        let picture = frame.to_cpu().map_err(|e| e.to_string())?;
        frames.push(json!({"pts": frame.pts, "width": picture.width(), "height": picture.height()}));
    }
    Ok(())'''))
    if ceiling:
        for relative, name in [('crates/coreav-kernels-vp9/src/inter.rs', 'predict_inter'),
                               ('crates/coreav-kernels-vp9/src/filter.rs', 'wide')]:
            path = source / relative
            path.write_text(replace_body(path.read_text(), name, ''))
        path = source / 'crates/coreav-vp9/src/picture.rs'
        text = path.read_text()
        start = text.index('            samples.extend(')
        end = text.index('            Ok(P::samples(samples))', start)
        path.write_text(text[:start] + '            samples.resize(len, P::default());\n' + text[end:])


def environment():
    return {k: os.environ[k] for k in ('PATH', 'HOME', 'TMPDIR', 'CARGO_HOME', 'RUSTUP_HOME')
            if k in os.environ} | {'CARGO_BUILD_JOBS': '2'}


def prepare(args):
    plan, _ = selection()
    out = pathlib.Path(args.out).resolve()
    require(not out.exists(), 'scratch destination already exists')
    out.mkdir(parents=True)
    archive = out / 'source.tar'
    subprocess.run(['git', 'archive', '--format=tar', '-o', str(archive), plan['baseline_commit']],
                   cwd=ROOT, check=True, timeout=120, env=environment())
    provenance = dict(baseline_commit=plan['baseline_commit'], plan_sha256=digest(PLAN),
                      driver_sha256=digest(pathlib.Path(__file__)), archive_sha256=digest(archive),
                      tooling_commit=os.environ.get('GITHUB_SHA'),
                      build=dict(profile='dist', features=['native-vp9'], rust_toolchain=plan['rust_toolchain']),
                      packets=PACKETS, indexes=list(INDEXES), arms={})
    for arm in ARMS:
        source = out / arm
        source.mkdir()
        with tarfile.open(archive) as bundle:
            bundle.extractall(source, filter='data')
        patch_source(source, arm == 'ceiling')
        paths = ('tools/decode-oracle/src/main.rs', 'crates/coreav-kernels-vp9/src/inter.rs',
                 'crates/coreav-kernels-vp9/src/filter.rs', 'crates/coreav-vp9/src/picture.rs')
        provenance['arms'][arm] = {p: digest(source / p) for p in paths}
    write(out / 'sources.json', provenance)


def fetch(args, wanted=None):
    require(sys.platform == 'linux', 'media acquisition requires hosted Linux')
    if wanted is None:
        _, wanted = selection()
    sys.path.insert(0, str(ROOT / 'tools/compat-corpus'))
    import round_files
    import r2pool
    limits = r2pool.Limits(requests=5000, bytes_=(len(wanted) + 1) * (128 << 20))

    def pool():
        credentials = {k: os.environ[v] for k, v in (
            ('access', 'R2_ACCESS_KEY_ID'), ('secret', 'R2_SECRET_ACCESS_KEY'), ('endpoint', 'R2_ENDPOINT'))}
        return r2pool.Pool(credentials, 4, limits, timeout=300)

    original_pool = round_files.pool
    directory = pathlib.Path(args.media).resolve()
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    try:
        round_files.pool = pool
        manifest = round_files.manifest('v1', None)
        entries = []
        for row in wanted:
            matches = [e for e in manifest if e.get('sha256') == row['sha256']]
            require(len(matches) == 1, 'missing or duplicate development input')
            entry = matches[0]
            require(entry.get('set') == 'real' and entry.get('ext') in ('mkv', 'webm')
                    and type(entry.get('size')) is int and 0 < entry['size'] <= 128 << 20,
                    'invalid development manifest entry')
            key = entry['key']
            require(isinstance(key, str) and key and '..' not in key.split('/'), 'invalid object key')
            entries.append(dict(entry, family='matroska-webm',
                                key=key if key.startswith('corpus/') else f'corpus/v1/{key}'))
        round_files.fetch(entries, directory)
    finally:
        round_files.pool = original_pool
        write(directory / 'acquisition.json', dict(requests_reserved=limits.requests,
                                                   bytes_reserved=limits.bytes, request_cap=5000))


def child_limits():
    resource.setrlimit(resource.RLIMIT_AS, (6 << 30, 6 << 30))
    resource.setrlimit(resource.RLIMIT_FSIZE, (128 << 20, 128 << 20))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def instructions(path):
    text = pathlib.Path(path).read_text()
    require(re.findall(r'^events: (.+)$', text, re.M) == ['Ir'], 'unexpected counter events')
    counts = re.findall(r'^summary: ([0-9]+)$', text, re.M)
    require(len(counts) == 1 and int(counts[0]) > 0, 'missing instruction count')
    return int(counts[0])


def capture(args):
    require(sys.platform == 'linux' and platform.machine() == 'x86_64', 'counter host must be Linux x86-64')
    plan, wanted = selection()
    scratch = pathlib.Path(args.out).resolve()
    media = pathlib.Path(args.media).resolve()
    private = pathlib.Path(args.private).resolve()
    private.mkdir(mode=0o700, parents=True, exist_ok=False)
    provenance = read(scratch / 'sources.json')
    require(provenance['plan_sha256'] == digest(PLAN) and provenance['baseline_commit'] == plan['baseline_commit']
            and provenance['indexes'] == list(INDEXES) and provenance['packets'] == PACKETS
            and provenance['driver_sha256'] == digest(pathlib.Path(__file__)),
            'scratch provenance differs')
    listing = read(media / 'files.json')
    require([r['sha256'] for r in listing] == [r['sha256'] for r in wanted], 'media roster differs')
    summary = dict(complete=False, diagnostic_only=True, pixel_exact=False, counter='callgrind:Ir',
                   plan_sha256=digest(PLAN), packets_per_input=PACKETS, inputs=len(wanted), classes=[])
    write(args.summary, summary)
    binaries = {}
    for arm in ARMS:
        for relative, sha in provenance['arms'][arm].items():
            require(digest(scratch / arm / relative) == sha, 'scratch source changed')
        binaries[arm] = scratch / arm / 'target/dist/decode-oracle'
    with tarfile.open(private / 'scratch-sources.tgz', 'w:gz') as bundle:
        for arm in ARMS:
            for relative in provenance['arms'][arm]:
                bundle.add(scratch / arm / relative, arcname=f'{arm}/{relative}')
    write(private / 'provenance.json', dict(provenance, binaries={a: digest(p) for a, p in binaries.items()},
          cpu=[line.strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines()
               if line.startswith(('model name', 'flags'))][:2],
          counter_version=subprocess.run(['valgrind', '--version'], capture_output=True, check=True,
                                         timeout=30, env=environment()).stdout.decode().strip()))
    for index, (row, entry) in enumerate(zip(wanted, listing)):
        path = pathlib.Path(entry['path']).resolve()
        require(path.parent == media and digest(path) == row['sha256'], 'input path or bytes differ')
        counts = {}
        work = []
        for arm in ARMS:
            prefix = private / f'{index}-{arm}'
            command = ['valgrind', '--tool=callgrind', '--cache-sim=no', '--branch-sim=no',
                       '--dump-instr=no', f'--callgrind-out-file={prefix}.counts',
                       f'--log-file={prefix}.log', str(binaries[arm]), str(path),
                       '--backend', 'native', '--packets', str(PACKETS)]
            with prefix.with_suffix('.stdout').open('wb') as stdout, prefix.with_suffix('.stderr').open('wb') as stderr:
                subprocess.run(command, stdout=stdout, stderr=stderr, timeout=600, check=True,
                               preexec_fn=child_limits, env=environment())
            require(prefix.with_suffix('.stderr').stat().st_size == 0, 'decoder emitted diagnostics')
            document = read(prefix.with_suffix('.stdout'))
            frames = document['frames']
            require(document['codec'] == 'vp9' and document['decoder'] == 'coreav vp9, scalar profile 0'
                    and document['fed'] == PACKETS and frames
                    and all(f['width'] == row['width'] and f['height'] == row['height'] for f in frames),
                    'decoder did not perform the expected packet and frame work')
            work.append(document)
            counts[arm] = instructions(str(prefix) + '.counts')
            write(str(prefix) + '.command.json', command)
        require(work[0] == work[1], 'ceiling work differs from baseline')
        summary['classes'].append(dict(resolution=row['class'], frames=len(work[0]['frames']), instructions=counts))
        write(args.summary, summary)
    summary['complete'] = True
    write(args.summary, summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    p = commands.add_parser('prepare')
    p.add_argument('--out', required=True)
    p = commands.add_parser('fetch')
    p.add_argument('--media', required=True)
    p = commands.add_parser('capture')
    for name in ('out', 'media', 'private', 'summary'):
        p.add_argument('--' + name, required=True)
    args = parser.parse_args()
    try:
        globals()[args.command](args)
    except (Exception, SystemExit) as error:
        print(f'VP9 instruction ceiling incomplete: {type(error).__name__}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
