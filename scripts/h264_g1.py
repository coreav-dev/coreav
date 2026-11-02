#!/usr/bin/env python3
"""Hash-bound G1 measurement driver. A merged freeze is mandatory before encode."""
import argparse
import hashlib
import json
import math
import os
import pathlib
import re
import statistics
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from h264_g1_conformance import capture, compare, digest, require, same_planes, inspect_anchor
import oracle_pin

QPS = (16, 22, 28, 34, 40)
METRICS = ('ssimulacra2', 'xpsnr_y')
ROOT = pathlib.Path(__file__).resolve().parents[1]


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


def publish_json(path, value):
    import fcntl
    directory = os.open(path.parent, os.O_RDONLY)
    temporary = None
    try:
        fcntl.flock(directory, fcntl.LOCK_EX)
        if path.exists():
            raise FileExistsError(f'completed publication already exists: {path}')
        descriptor, name = tempfile.mkstemp(prefix=f'.{path.name}.', suffix='.tmp', dir=path.parent)
        temporary = pathlib.Path(name)
        with os.fdopen(descriptor, 'wb') as output:
            output.write(canonical(value))
            output.flush()
            os.fsync(output.fileno())
        os.rename(temporary, path)
        os.fsync(directory)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
        os.close(directory)


def sha(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def git(*args, root=ROOT):
    return subprocess.run(['git', '-C', str(root), *args], check=True,
                          capture_output=True, timeout=30).stdout


def hexhash(value):
    if not isinstance(value, str) or not re.fullmatch('[0-9a-f]{64}', value):
        raise ValueError('missing or invalid artifact SHA-256')


def load_freeze(commit, path, expected, root=ROOT):
    if not re.fullmatch('[0-9a-f]{40}', commit):
        raise ValueError('freeze must name a full immutable commit')
    hexhash(expected)
    if pathlib.PurePosixPath(path).is_absolute() or '..' in pathlib.PurePosixPath(path).parts:
        raise ValueError('manifest path must be repository relative')
    git('merge-base', '--is-ancestor', commit, 'origin/main', root=root)
    data = git('show', f'{commit}:{path}', root=root)
    require(hashlib.sha256(data).hexdigest(), expected, 'merged manifest digest')
    manifest = json.loads(data)
    validate(manifest)
    return manifest


def validate(m):
    require(m['protocol'], 'coreav-h264-g1-v1', 'protocol')
    require(m['qps'], list(QPS), 'QP ladder')
    require(m['frames'], 60, 'frame count')
    require(m['timing'], [30, 1], 'nominal timing')
    require(m['colour'], [False, 1, 1, 1, 0], 'measurement preparation signalling')
    clips = m['clips']
    require(len(clips), 24, 'complete roster')
    require([c['label'] for c in clips], [f'G1-{n:02}' for n in range(1, 25)], 'ordered labels')
    if len({c['parent'] for c in clips}) != 24 or len({c['sha256'] for c in clips}) != 24:
        raise ValueError('duplicate parent or prepared bytes')
    slots = ((1, 2, 3), (2, 3, 4), (3, 4, 1), (4, 1, 2),
             (1, 2, 3), (2, 3, 4), (3, 4, 1), (4, 1, 2))
    for c in clips:
        w, h = c['width'], c['height']
        if not (16 <= w <= 1920 and 16 <= h <= 1080 and w % 2 == h % 2 == 0):
            raise ValueError('prepared geometry outside G1 envelope')
        require(c['frames'], 60, 'prepared frames')
        require(c['bytes'], w * h * 3 // 2 * 60, 'prepared byte count')
        hexhash(c['sha256'])
        hexhash(c['original_sha256'])
        if not c['licence_approved'] or c['licence'] not in ('public-domain', 'CC0', 'CC-BY', 'internal-rd-permission'):
            raise ValueError('missing permitted licence decision')
        for key in ('preparation', 'source_declarations', 'original_geometry', 'original_timing', 'original_bytes', 'first_frame', 'selected_stream'):
            if key not in c:
                raise ValueError(f'missing frozen {key}')
        require(c['first_frame'], 0, 'first presented frame')
        if c['original_bytes'] <= 0:
            raise ValueError('missing original byte count')
        group = 1 if h <= 360 else 2 if h <= 720 else 3 if w <= 1440 else 4
        require(c['resolution_group'], group, 'resolution group')
    for cls, required in enumerate(slots, 1):
        group = clips[(cls - 1) * 3:cls * 3]
        require([c['class'] for c in group], [cls] * 3, 'class quota')
        require([c['resolution_group'] for c in group], list(required), 'class slots')
        selected = min(group, key=lambda c: hashlib.sha256(('coreav-h264-g1-v1:' + c['sha256']).encode()).digest())
        for c in group:
            require(c['split'], 'training' if c is selected else 'held-out', 'deterministic split')
    require(m['ordered_planes_sha256'], sha([c['sha256'] for c in clips]), 'ordered plane hashes')
    for name in ('ffmpeg', 'ffprobe', 'jm', 'x264', 'metric_ffmpeg', 'ssimulacra2'):
        tool = m['tools'][name]
        hexhash(tool['sha256'])
        if not all(tool.get(k) for k in ('version', 'build', 'compiler')):
            raise ValueError(f'incomplete tool pin {name}')
    for name in ('jm', 'metric_ffmpeg', 'ssimulacra2'):
        hexhash(m['tools'][name]['archive_sha256'])
    for name in ('convert', 'ssimulacra2', 'xpsnr'):
        command = m['commands'][name]
        if not isinstance(command, list) or not command or not all(isinstance(v, str) for v in command):
            raise ValueError(f'invalid frozen command {name}')
    fixture = m['metric_fixture']
    require(fixture['frames'], 60, 'metric fixture frames')
    require(fixture['finite'], True, 'metric fixture finite scores')
    hexhash(fixture['reference_sha256'])
    hexhash(fixture['reconstruction_sha256'])
    if not (16 <= fixture['width'] <= 1920 and 16 <= fixture['height'] <= 1080):
        raise ValueError('invalid metric fixture dimensions')
    if len(fixture['png_sha256']) != 60:
        raise ValueError('incomplete metric conversion fixture')
    for value in fixture['png_sha256']:
        hexhash(value)


def curve(points):
    if len(points) != 5:
        raise ValueError('RD curve needs all five registered QPs')
    xy = sorted(points)
    x, rate = zip(*xy)
    if not all(math.isfinite(q) and math.isfinite(r) and r > 0 for q, r in xy):
        raise ValueError('nonfinite quality or nonpositive rate')
    if any(x[i] >= x[i + 1] or rate[i] >= rate[i + 1] for i in range(4)):
        raise ValueError('duplicate or nonmonotone RD curve')
    y = [math.log(r) for r in rate]
    h = [x[i + 1] - x[i] for i in range(4)]
    d = [(y[i + 1] - y[i]) / h[i] for i in range(4)]
    slopes = [0.] * 5
    for i in range(1, 4):
        w1, w2 = 2 * h[i] + h[i - 1], h[i] + 2 * h[i - 1]
        slopes[i] = (w1 + w2) / (w1 / d[i - 1] + w2 / d[i])
    for at, a, b in ((0, 0, 1), (4, 3, 2)):
        value = ((2 * h[a] + h[b]) * d[a] - h[a] * d[b]) / (h[a] + h[b])
        slopes[at] = max(0., value)
    return x, y, slopes


def integrate(c, low, high):
    x, y, slopes = c
    result = 0.
    for i in range(4):
        a, b = max(low, x[i]), min(high, x[i + 1])
        if a >= b:
            continue
        h = x[i + 1] - x[i]
        delta = (y[i + 1] - y[i]) / h
        quadratic = (3 * delta - 2 * slopes[i] - slopes[i + 1]) / h
        cubic = (slopes[i] + slopes[i + 1] - 2 * delta) / (h * h)

        def primitive(v):
            t = v - x[i]
            return y[i] * t + slopes[i] * t*t / 2 + quadratic * t**3 / 3 + cubic * t**4 / 4
        result += primitive(b) - primitive(a)
    return result


def bd_rate(ours, anchor):
    a, b = curve(ours), curve(anchor)
    low, high = max(a[0][0], b[0][0]), min(a[0][-1], b[0][-1])
    if high <= low or high - low < min(a[0][-1] - a[0][0], b[0][-1] - b[0][0]) / 2:
        raise ValueError('insufficient common quality span')
    return 100 * math.expm1((integrate(a, low, high) - integrate(b, low, high)) / (high - low))


def report(manifest, cells):
    expected = {(c['label'], arm, qp) for c in manifest['clips'] for arm in ('coreav', 'x264') for qp in QPS}
    indexed = {(c['label'], c['arm'], c['qp']): c for c in cells}
    if len(indexed) != len(cells) or set(indexed) != expected:
        raise ValueError('missing, duplicate or extra measurement cells')
    for cell in cells:
        validate_scores(cell)
    results = []
    for clip in manifest['clips']:
        item = {k: clip[k] for k in ('label', 'split', 'class', 'resolution_group')}
        for metric in METRICS:
            curves = []
            for arm in ('coreav', 'x264'):
                points = []
                for qp in QPS:
                    cell = indexed[(clip['label'], arm, qp)]
                    require(cell['conforming'], True, 'conformance before scoring')
                    require(cell['frames'], 60, 'scored frames')
                    points.append((cell[metric], cell['bytes'] * 8 / 60))
                curves.append(points)
            item[metric] = bd_rate(*curves)
        results.append(item)
    heldout = [c for c in results if c['split'] == 'held-out']
    training = [c for c in results if c['split'] == 'training']
    require((len(heldout), len(training)), (16, 8), 'split denominators')
    medians = {m: statistics.median(c[m] for c in heldout) for m in METRICS}
    return {'complete': True, 'cells': len(cells), 'per_clip': results,
            'held_out_medians': medians,
            'training_medians': {m: statistics.median(c[m] for c in training) for m in METRICS},
            'maximum': {m: max(c[m] for c in heldout) for m in METRICS},
            'group_medians': {str(g): {m: statistics.median(c[m] for c in heldout if c['resolution_group'] == g) for m in METRICS} for g in range(1, 5) if any(c['resolution_group'] == g for c in heldout)},
            'pass': all(v <= 30 for v in medians.values()),
            'reconsider': any(v > 60 for v in medians.values())}


def command(argv, **values):
    result = []
    for part in argv:
        result.append(part.format(**values))
    return result


def child(argv, directory, record=None):
    name = record or f'command-{len(list(directory.glob("command-*.json"))):03}.json'
    result = capture(argv, directory, 300, record=name)
    return (result.stdout + result.stderr).decode()


def strip_identifying_sei(data):
    markers = list(re.finditer(b'\x00\x00\x00\x01|\x00\x00\x01', data))
    if not markers or markers[0].start() != 0:
        raise ValueError('invalid Annex B')
    out = bytearray()
    for i, marker in enumerate(markers):
        end = markers[i + 1].start() if i + 1 < len(markers) else len(data)
        nal = data[marker.end():end]
        if not nal:
            raise ValueError('empty NAL')
        if nal[0] & 31 == 6:
            if len(nal) < 2 or nal[1] != 5:
                raise ValueError('unexpected non-identifying SEI')
        else:
            out.extend(data[marker.start():end])
    return bytes(out)


def validate_scores(cell):
    scores = cell['ssimulacra2_frames']
    require(len(scores), 60, 'all SSIMULACRA2 frame scores')
    if not all(math.isfinite(v) for v in scores):
        raise ValueError('nonfinite SSIMULACRA2 frame score')
    require(cell['ssimulacra2'], statistics.mean(scores), 'SSIMULACRA2 arithmetic mean')
    if not all(math.isfinite(cell[k]) for k in ('xpsnr_y', 'xpsnr_cb', 'xpsnr_cr')):
        raise ValueError('missing or nonfinite XPSNR plane score')


def replay_scores(directory):
    def text(path):
        record = json.loads(path.read_text())
        require(record['exit_code'], 0, 'successful metric process')
        return record['stdout'] + record['stderr']
    files = sorted(directory.glob('ssimulacra2-*.json'))
    require([p.name for p in files], [f'ssimulacra2-{i:03}.json' for i in range(60)], 'metric process completeness')
    scores = [float(text(path).strip()) for path in files]
    summaries = re.findall(r'XPSNR\s+y:\s*(\S+)\s+u:\s*(\S+)\s+v:\s*(\S+)', text(directory / 'xpsnr.json'))
    require(len(summaries), 1, 'sequence XPSNR summary')
    y, cb, cr = map(float, summaries[0])
    result = {'ssimulacra2': statistics.mean(scores), 'ssimulacra2_frames': scores,
              'xpsnr_y': y, 'xpsnr_cb': cb, 'xpsnr_cr': cr}
    validate_scores(result)
    return result


def verify_cell(cell, directory, binding, clip):
    require(cell['binding'], sha(binding), 'cell binding')
    files = {str(p.relative_to(directory)) for p in directory.rglob('*') if p.is_file() and p != directory / 'cell.json'}
    require(set(cell['artifacts']), files, 'complete checkpoint artifacts')
    for path, expected in cell['artifacts'].items():
        require(digest(directory / path), expected, 'checkpoint artifact')
    for key, value in replay_scores(directory).items():
        require(cell[key], value, 'metric summary derived from raw process output')
    require(cell['frames'], 60, 'checkpoint frame count')
    require(cell['conforming'], True, 'checkpoint conformance')
    stream = directory / 'output.h264'
    require(cell['bytes'], stream.stat().st_size, 'checkpoint complete stream byte count')
    if cell['arm'] == 'coreav':
        from h264_g1_conformance import inspect
        inspect(stream.read_bytes(), clip['width'], clip['height'], cell['qp'], 60)
        same_planes(directory / 'encoder.yuv', directory / 'ffmpeg.yuv', clip['bytes'])
    else:
        inspect_anchor(stream.read_bytes(), clip['width'], clip['height'], cell['qp'], 60)
        require((directory / 'coreav.log').read_bytes(), b'', 'CoreAV oracle diagnostics')
    for name in ('coreav.yuv', 'jm.yuv'):
        same_planes(directory / 'ffmpeg.yuv', directory / name, clip['bytes'])


def load_cells(manifest, output, freeze, manifest_digest):
    binding = json.loads((output / 'identity.json').read_text())
    require(binding['freeze'], freeze, 'report freeze identity')
    require(binding['manifest'], manifest_digest, 'report manifest identity')
    require(binding['tools'], {k: v['sha256'] for k, v in manifest['tools'].items()}, 'report tool identities')
    cells = []
    for clip in manifest['clips']:
        for arm in ('coreav', 'x264'):
            for qp in QPS:
                directory = output / f'{clip["label"]}-{arm}-q{qp}'
                cell = json.loads((directory / 'cell.json').read_text())
                require((cell['label'], cell['arm'], cell['qp']), (clip['label'], arm, qp), 'checkpoint coordinates')
                verify_cell(cell, directory, binding, clip)
                cells.append(cell)
    return cells


def decode_anchor(decoder, stream, directory):
    independent = directory / 'coreav.yuv'
    with independent.open('wb') as output:
        process = subprocess.run([str(decoder.resolve()), '--kernels', 'scalar', '--threads', '1', str(stream.resolve())], stdout=output, stderr=subprocess.PIPE, timeout=300, check=True)
    (directory / 'coreav.log').write_bytes(process.stderr)
    require(process.stderr, b'', 'CoreAV oracle diagnostics')
    return independent


def resume_cell(directory, binding, clip, arm, qp, resume_fault):
    checkpoint = directory / 'cell.json'
    if checkpoint.exists():
        try:
            cell = json.loads(checkpoint.read_text())
        except json.JSONDecodeError:
            if not resume_fault:
                raise ValueError(f'truncated checkpoint retained at {directory}; a recorded infrastructure fault is required') from None
        else:
            require((cell['label'], cell['arm'], cell['qp']), (clip['label'], arm, qp), 'checkpoint coordinates')
            verify_cell(cell, directory, binding, clip)
            return cell
    if directory.exists():
        if not resume_fault:
            raise ValueError(f'failed attempt retained at {directory}; an infrastructure-fault disposition is required')
        fault = json.loads(resume_fault.read_text())
        require(fault['binding'], sha(binding), 'infrastructure-fault identity')
        if fault['reason'] not in ('preemption', 'runner-loss', 'input-integrity', 'host-gate') or directory.name not in fault['cells']:
            raise ValueError('infrastructure fault does not cover this incomplete cell')
        failed = directory.parent / 'failed-attempts'
        failed.mkdir(exist_ok=True)
        number = len(list(failed.iterdir()))
        publish_json(failed / f'fault-{number:03}.json', fault)
        directory.rename(failed / f'{directory.name}-{number:03}')
    return None


def measure(args, manifest):
    if os.environ.get('GITHUB_ACTIONS') != 'true' or sys.platform != 'linux':
        raise ValueError('G1 measurement is enabled only in authorized Linux Actions jobs')
    require(args.authorize, args.candidate, 'explicit candidate authorization')
    if not re.fullmatch('[0-9a-f]{40}', args.candidate):
        raise ValueError('candidate must be a full immutable commit')
    git('merge-base', '--is-ancestor', args.candidate, 'origin/main')
    identity = json.loads(git('show', f'{args.candidate}:{args.identity}'))
    hexhash(identity['driver_sha256'])
    require(digest(args.driver), identity['driver_sha256'], 'committed driver binary')
    require(digest(args.decoder), identity['decoder_sha256'], 'committed independent decoder binary')
    tools = json.loads(args.tools.read_text())
    require(set(tools), set(manifest['tools']), 'complete frozen tool set')
    tools = {k: str(pathlib.Path(v).resolve()) for k, v in tools.items()}
    require(str(pathlib.Path(tools['ffmpeg']).with_name('ffprobe')), tools['ffprobe'], 'paired pinned FFmpeg tools')
    for name, pin in manifest['tools'].items():
        require(digest(tools[name]), pin['sha256'], f'{name} executable')
    problems = list(oracle_pin.verify_bin(pathlib.Path(tools['ffmpeg']).parent))
    if problems:
        raise ValueError('; '.join(problems))
    for clip in manifest['clips']:
        path = args.inputs / (clip['label'] + '.yuv')
        require((path.stat().st_size, digest(path)), (clip['bytes'], clip['sha256']), 'prepared input integrity')
    args.output.mkdir(parents=True, exist_ok=True)
    binding = {'freeze': args.freeze, 'manifest': args.sha256, 'candidate': args.candidate,
               'identity': identity, 'tools': {k: digest(v) for k, v in tools.items()}}
    stamp = args.output / 'identity.json'
    if stamp.exists():
        require(json.loads(stamp.read_text()), binding, 'checkpoint identity')
    else:
        publish_json(stamp, binding)
    fixture = manifest['metric_fixture']
    for name in ('reference', 'reconstruction'):
        path = args.metric_fixture / f'{name}.yuv'
        require(digest(path), fixture[f'{name}_sha256'], 'metric fixture bytes')
        require(path.stat().st_size, fixture['width'] * fixture['height'] * 3 // 2 * 60, 'metric fixture size')
    preflight = args.output / f'metric-preflight-{len(list(args.output.glob("metric-preflight-*"))):03}'
    preflight.mkdir()
    score(manifest, tools, (args.metric_fixture / 'reference.yuv').resolve(), (args.metric_fixture / 'reconstruction.yuv').resolve(), preflight, fixture)
    converted = sorted((preflight / 'reference').glob('*.png'))
    require([digest(p) for p in converted], fixture['png_sha256'], 'frozen metric conversion fixture')
    cells = []
    for clip in manifest['clips']:
        raw = (args.inputs / (clip['label'] + '.yuv')).resolve()
        for qp in QPS:
            for arm in ('coreav', 'x264'):
                directory = args.output / f'{clip["label"]}-{arm}-q{qp}'
                checkpoint = directory / 'cell.json'
                cell = resume_cell(directory, binding, clip, arm, qp, args.resume_fault)
                if cell is not None:
                    cells.append(cell)
                    continue
                if arm == 'coreav':
                    cell = compare(args.driver.resolve(), pathlib.Path(tools['ffmpeg']), pathlib.Path(tools['jm']), raw, directory,
                                   clip['width'], clip['height'], qp, 60)
                    decoded = directory / 'encoder.yuv'
                else:
                    directory.mkdir(parents=True)
                    stream = directory / 'output.h264'
                    argv = [tools['x264'], '--preset', 'medium', '--profile', 'baseline', '--level', '5.1', '--keyint', '1', '--min-keyint', '1', '--scenecut', '0', '--bframes', '0', '--ref', '1', '--no-cabac', '--partitions', 'i4x4', '--no-8x8dct', '--trellis', '0', '--no-psy', '--aq-mode', '0', '--no-mbtree', '--ipratio', '1', '--pbratio', '1', '--chroma-qp-offset', '0', '--no-deblock', '--threads', '1', '--lookahead-threads', '1', '--sync-lookahead', '0', '--rc-lookahead', '0', '--slices', '1', '--qp', str(qp), '--fps', '30/1', '--input-csp', 'i420', '--input-depth', '8', '--input-res', f'{clip["width"]}x{clip["height"]}', '--frames', '60', '--range', 'tv', '--colorprim', 'bt709', '--transfer', 'bt709', '--colormatrix', 'bt709', '--chromaloc', '0', '--output', str(stream.resolve()), str(raw)]
                    child(argv, directory)
                    stream.write_bytes(strip_identifying_sei(stream.read_bytes()))
                    inspect_anchor(stream.read_bytes(), clip['width'], clip['height'], qp, 60)
                    decoded = directory / 'ffmpeg.yuv'
                    child([tools['ffmpeg'], '-nostdin', '-v', 'error', '-xerror', '-err_detect', 'explode', '-hwaccel', 'none', '-threads', '1', '-c:v', 'h264', '-i', stream.resolve(), '-an', '-c:v', 'rawvideo', '-pix_fmt', '+yuv420p', '-fps_mode', 'passthrough', '-f', 'rawvideo', decoded.resolve()], directory)
                    reference = directory / 'jm.yuv'
                    child([tools['jm'], '-i', stream.resolve(), '-o', reference.resolve()], directory)
                    same_planes(decoded, reference, clip['bytes'])
                    independent = decode_anchor(args.decoder, stream, directory)
                    same_planes(decoded, independent, clip['bytes'])
                    cell = {'frames': 60, 'bytes': stream.stat().st_size, 'conforming': True}
                scores = score(manifest, tools, raw, decoded.resolve(), directory, clip)
                require(digest(raw), clip['sha256'], 'input unchanged through scoring')
                for name, pin in manifest['tools'].items():
                    require(digest(tools[name]), pin['sha256'], f'{name} unchanged through scoring')
                cell.update(scores)
                cell.update({'label': clip['label'], 'arm': arm, 'qp': qp, 'binding': sha(binding),
                             'artifacts': {str(p.relative_to(directory)): digest(p) for p in directory.rglob('*') if p.is_file()}})
                publish_json(checkpoint, cell)
                cells.append(cell)
    result = report(manifest, cells)
    summary = args.output / 'report.json'
    if summary.exists():
        require(json.loads(summary.read_text()), result, 'completed report')
    else:
        publish_json(summary, result)
    return 0 if result['pass'] else 1


def score(manifest, tools, raw, decoded, directory, clip):
    values = dict(tools, width=clip['width'], height=clip['height'], frames=60, fps='30/1')
    images = []
    for name, source in (('reference', raw), ('reconstruction', decoded)):
        folder = directory / name
        folder.mkdir()
        child(command(manifest['commands']['convert'], **values, input=str(source), output=str((folder / '%06d.png').resolve())), directory, record=f'convert-{name}.json')
        files = sorted(folder.glob('*.png'))
        require(len(files), 60, 'converted frame count')
        images.append(files)
    for i, (reference, reconstruction) in enumerate(zip(*images)):
        child(command(manifest['commands']['ssimulacra2'], **values, reference=str(reference.resolve()), reconstruction=str(reconstruction.resolve())), directory, record=f'ssimulacra2-{i:03}.json')
    child(command(manifest['commands']['xpsnr'], **values, reference=str(raw), reconstruction=str(decoded)), directory, record='xpsnr.json')
    return replay_scores(directory)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=('verify-freeze', 'run', 'report'))
    p.add_argument('--freeze', required=True)
    p.add_argument('--manifest', required=True)
    p.add_argument('--sha256', required=True)
    p.add_argument('--candidate')
    p.add_argument('--identity', help='committed repository-relative binary identity JSON')
    p.add_argument('--authorize', help='full candidate commit explicitly authorized for this run')
    p.add_argument('--tools', type=pathlib.Path)
    p.add_argument('--driver', type=pathlib.Path)
    p.add_argument('--decoder', type=pathlib.Path)
    p.add_argument('--resume-fault', type=pathlib.Path, help='recorded infrastructure fault, never an RD miss')
    p.add_argument('--inputs', type=pathlib.Path)
    p.add_argument('--metric-fixture', type=pathlib.Path)
    p.add_argument('--output', type=pathlib.Path)
    args = p.parse_args()
    manifest = load_freeze(args.freeze, args.manifest, args.sha256)
    if args.action == 'verify-freeze':
        print('merged freeze and protocol are complete')
        return 0
    if args.action == 'report':
        if not args.output:
            raise ValueError('report needs the retained output directory')
        result = report(manifest, load_cells(manifest, args.output, args.freeze, args.sha256))
        print(json.dumps(result, indent=2, allow_nan=False))
        return 0 if result['pass'] else 1
    if not all((args.candidate, args.identity, args.authorize, args.tools, args.driver, args.decoder, args.inputs, args.metric_fixture, args.output)):
        raise ValueError('run needs committed candidate identity, authorization, tools, input directory and output')
    return measure(args, manifest)


if __name__ == '__main__':
    raise SystemExit(main())
