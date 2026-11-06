"""Acquire and replay the registered GitHub Shapes 200 measurement."""
import argparse
import collections
import fractions
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path[:0] = [str(HERE), str(ROOT / 'scripts')]
import decoded_output
import encode_breadth as breadth
import github_shapes_fixtures as fixtures
import github_shapes_inputs as inputs
import github_shapes_set as roster
import github_shapes_aac as aac
import oracle_pin
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

FEATURES = 'release-full'
BUILD = ['cargo', 'build', '--release', '--locked', '-p', 'coreav-cli', '--features', FEATURES]
MISSES = ('uninstantiable', 'reference_failed', 'timeout', 'refused', 'failed',
          'unexpected_program', 'no_oracle', 'different')
EXACT_CODECS = ('h264', 'vp9', 'aac', 'opus')
COMPATIBLE = 'compatible_avc1'
MAPPING = re.compile(r'^\s*Stream #\d+:\d+(?:\[[^]]*\])?(?: \([^)]*\))? -> #(\d+):(\d+) \((.*)\)\s*$', re.M)
METHOD = ('docs/design/github-shapes-200.md', 'scripts/oracle-ffmpeg.env',
          '.github/workflows/github-shapes.yml', 'Cargo.lock',
          'docs/design/aac-pcm-eligibility.md', 'docs/design/aac-pns-eligibility.md',
          'docs/compat/audio-decoder-eligibility.json', 'scripts/aac-pns-requirements.txt',
          'scripts/oracle-audio-args.txt')


class EvidenceError(ValueError):
    pass


def digest(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + '\n')


def git(*args):
    return subprocess.run(['git', '-C', str(ROOT), *args], capture_output=True,
                          text=True, timeout=30)


def merged_source(commit):
    if re.fullmatch('[0-9a-f]{40}', commit) is None:
        raise EvidenceError('candidate_sha')
    if os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch' or os.environ.get('GITHUB_REF') != 'refs/heads/main':
        raise EvidenceError('main_dispatch_required')
    head = git('rev-parse', 'HEAD')
    status = git('status', '--porcelain', '--untracked-files=normal')
    if head.returncode or head.stdout.strip() != commit or status.returncode or status.stdout:
        raise EvidenceError('source_not_clean_candidate')
    if git('merge-base', '--is-ancestor', commit, 'origin/main').returncode:
        raise EvidenceError('candidate_not_merged')


def method():
    paths = [ROOT / path for path in METHOD]
    paths.extend(HERE.glob('*.py'))
    paths.extend((ROOT / 'scripts').glob('*.py'))
    paths.extend((HERE / 'github-shapes-200').iterdir())
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(set(paths)) if path.is_file()}


def environment():
    return {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'LANG': 'C', 'LC_ALL': 'C',
            'GCONV_PATH': '/nonexistent/github-shapes-gconv'}


def execute(argv, cwd, env, seconds, tag, stdin=None):
    source = (cwd / stdin).open('rb') if stdin else open(os.devnull, 'rb')
    out, err = cwd / (tag + '.stdout'), cwd / (tag + '.stderr')
    limited = ['/bin/sh', '-c', 'ulimit -f 262144 && exec "$@"', 'limit', *map(str, argv)]
    timed_out = False
    try:
        with source, out.open('wb') as stdout, err.open('wb') as stderr:
            with subprocess.Popen(limited, cwd=cwd, env=env, stdin=source, stdout=stdout,
                                  stderr=stderr, start_new_session=True) as child:
                try:
                    child.wait(timeout=seconds)
                except subprocess.TimeoutExpired:
                    timed_out = True
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=10)
                code = child.returncode
    finally:
        source.close()
    return {'code': code, 'timed_out': timed_out, 'args': list(map(str, argv)), 'stdin': stdin,
            'stdout': out.name, 'stderr': err.name}


def strict_fallback(path):
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    if any(not isinstance(row, dict) or 'ran' not in row for row in rows):
        raise EvidenceError('fallback_record')
    if any(row['ran'] is not None for row in rows):
        raise EvidenceError('fallback_executed')
    return rows


def normalize_start(start, bundle):
    if set(start) != {'program', 'args'} or start['program'] not in ('ffmpeg', 'x264'):
        raise EvidenceError('program_record')
    if not isinstance(start['args'], list) or any(not isinstance(arg, str) for arg in start['args']):
        raise EvidenceError('program_arguments')
    args = [arg.replace(str(bundle), '$BUNDLE') for arg in start['args']]
    return {'program': start['program'], 'args': args}


def expected_invocation(command, candidate):
    if candidate:
        return ['$COREAV', command.tool, *command.args]
    args = []
    for value in command.args:
        if value == '-i':
            args.extend(AUDIO_DECODE_INPUT)
        args.append(value)
    return ['$FFMPEG' if command.tool == 'ffmpeg' else '$FFPROBE', '-v',
            'info' if command.tool == 'ffmpeg' else 'error', *args]


def probe_witness_invocation(command):
    args = expected_invocation(command, False)
    positions = [index + 1 for index, value in enumerate(args[:-1]) if value == '-of']
    for index in positions:
        args[index] = 'json'
    return args if positions else [args[0], '-of', 'json', *args[1:]]


def validate_receipt(receipt, invocation, stdin, tag):
    if receipt['args'] != invocation:
        raise EvidenceError('invocation_binding')
    if receipt['stdin'] != stdin or receipt['stdout'] != tag + '.stdout' or receipt['stderr'] != tag + '.stderr':
        raise EvidenceError('stdio_binding')
    if type(receipt['code']) is not int or type(receipt['timed_out']) is not bool:
        raise EvidenceError('process_receipt')


def output_present(command, folder):
    if command.tool == 'ffprobe':
        return True
    for name, options in zip(command.outputs, command.output_options):
        if dict(zip(options, options[1:])).get('-f') == 'null':
            continue
        paths = inputs.output_files(folder, name)
        if not paths or any(not path.stat().st_size for path in paths):
            return False
    return True


def acquire_one(row, bundle, programs, env):
    try:
        command = inputs.instantiate(row['shape'])
    except inputs.Uninstantiable as error:
        return {'rank': row['rank'], 'uninstantiable': str(error)}
    base = bundle / 'calls' / str(row['rank'])
    base.mkdir(parents=True)
    result = {'rank': row['rank'], 'command': command.args, 'tool': command.tool, 'arms': {}}
    for arm in ('reference', 'candidate'):
        folder = base / arm
        folder.mkdir()
        fixtures.stage(command, bundle / 'fixtures', folder)
        candidate = arm == 'candidate'
        invocation = expected_invocation(command, candidate)
        argv = [programs.get(arg, arg) for arg in invocation]
        fallback = folder / 'fallback.jsonl'
        fallback.touch()
        logger = breadth.ProgramLog(folder / 'programs', programs['$X264'], programs['$FFMPEG']) if candidate else None
        call_env = dict(env)
        if candidate:
            call_env.update(COREAV_FALLBACK=breadth.NO_FALLBACK, COREAV_FALLBACK_LOG=str(fallback),
                            COREAV_X264=str(logger.wrappers['x264']), COREAV_FFMPEG=str(logger.wrappers['ffmpeg']))
        try:
            receipt = execute(argv, folder, call_env, 300 if candidate else 120, 'command', command.stdin)
            starts = [normalize_start(start, bundle) for start in logger.drain()] if logger else []
        finally:
            if logger:
                logger.close()
        receipt['args'] = invocation
        write(folder / 'starts.json', starts)
        (folder / 'stdout').write_bytes((folder / 'command.stdout').read_bytes())
        strict_fallback(fallback)
        if logger:
            for path in (folder / 'programs').iterdir():
                path.unlink()
            (folder / 'programs').rmdir()
        result['arms'][arm] = receipt
        if not candidate and (receipt['code'] or receipt['timed_out'] or not output_present(command, folder)):
            break
        if not candidate and command.tool == 'ffprobe':
            invocation = probe_witness_invocation(command)
            argv = [programs.get(arg, arg) for arg in invocation]
            witness = execute(argv, folder, env, 120, 'eligibility', command.stdin)
            witness['args'] = invocation
            result['eligibility'] = witness
    return result


def packet_rows(path, ffprobe, env, raw):
    code, data, err = decoded_output.invoke([ffprobe, '-v', 'error', *raw,
        '-i', str(path), '-show_streams', '-show_packets', '-show_data_hash', 'sha256', '-of', 'json'], env)
    if code or err:
        return None
    doc = json.loads(data)
    clocks = {s['index']: fractions.Fraction(s['time_base']) for s in doc['streams']}
    return [(p['stream_index'], p['data_hash'], *[
        str(int(p[key]) * clocks[p['stream_index']]) if p.get(key) not in (None, 'N/A') else None
        for key in ('pts', 'dts', 'duration')]) for p in doc['packets']]


def jobs(reference):
    stderr = (reference / 'command.stderr').read_text(errors='replace')
    return {(int(output), int(stream)): 'copy' if text == 'copy' else breadth.encoder(text)
            for output, stream, text in MAPPING.findall(stderr)}


def raw_description(command, index, source):
    options = command.output_options[index]
    values = dict(zip(options, options[1:]))
    form = values.get('-f')
    if form in ('s16le', 'f32le'):
        audio = next((s for s in (source or {}).get('streams', []) if s.get('codec_type') == 'audio'), {})
        rate = int(values.get('-ar', audio.get('sample_rate', 0)))
        channels = int(values.get('-ac', audio.get('channels', 0)))
        if rate <= 0 or channels not in (1, 2):
            raise EvidenceError('raw_audio_description')
        return ['-f', form, '-sample_rate', str(rate), '-ch_layout', 'mono' if channels == 1 else 'stereo']
    if form == 'rawvideo':
        if source is None or '-vf' in options:
            raise EvidenceError('raw_video_description')
        return decoded_output.raw_inputs([*AUDIO_DECODE_INPUT, '-i', 'source', *options], source)
    return []


def probe_available(command, json_output):
    requested = {}
    if '-show_entries' in command.args:
        for entry in command.args[command.args.index('-show_entries') + 1].split(':'):
            section, _, fields = entry.partition('=')
            requested[section] = fields.split(',') if fields else []
    for option, section in (('-show_streams', 'stream'), ('-show_format', 'format')):
        if option in command.args:
            requested.setdefault(section, [])
    if not requested or set(requested) - {'stream', 'format'}:
        return False
    def available(value):
        if isinstance(value, dict):
            return any(available(v) for v in value.values())
        if isinstance(value, list):
            return bool(value) and all(available(v) for v in value)
        if isinstance(value, float):
            return math.isfinite(value)
        return value is not None and str(value).strip().casefold() not in ('', 'n/a', 'unknown', 'nan', 'inf', '-inf')
    try:
        document = json.loads(json_output)
        for section, fields in requested.items():
            value = document.get('streams' if section == 'stream' else section)
            records = value if isinstance(value, list) else [value]
            if not records or any(not isinstance(record, dict) or not available(record) or
                any(field not in record or not available(record[field]) for field in fields) for record in records):
                return False
        return True
    except (ValueError, TypeError, AttributeError, UnicodeError):
        return False


def compare_outputs(command, base, programs, env, scratch):
    ref, mine = base / 'reference', base / 'candidate'
    if command.tool == 'ffprobe':
        if not all((folder / 'stdout').read_bytes().strip() for folder in (ref, mine)):
            return 'no_oracle'
        if not probe_available(command, (ref / 'eligibility.stdout').read_bytes()):
            return 'no_oracle'
        return 'counted' if decoded_output.same_files(ref / 'stdout', mine / 'stdout') else 'different'
    if any(dict(zip(opts, opts[1:])).get('-f') in ('hls', 'null') for opts in command.output_options) or any(
            name.endswith('.m3u8') for name in command.outputs):
        return 'no_oracle'
    source = decoded_output.probe(ref / command.sources[0][0], programs['$FFPROBE'], env)
    mapping = jobs(ref)
    if not mapping:
        return 'no_oracle'
    aac_decoded = aac.decodes_aac(ref)
    outcome, unavailable = 'counted', 'no_oracle'
    timeline_checks = []
    controls = {'command.stdout', 'command.stderr', 'stdout', 'fallback.jsonl', 'starts.json'}
    for folder, failure in ((ref, 'no_oracle'), (mine, 'different')):
        declared = {path.name for name in command.outputs for path in inputs.output_files(folder, name)}
        if any(path.is_file() and path.name not in controls | declared for path in folder.iterdir()):
            return failure
    for output_index, name in enumerate(command.outputs):
        try:
            raw = raw_description(command, output_index, source)
        except (ValueError, TypeError):
            return unavailable
        copied = [stream for (output, stream), kind in mapping.items() if output == output_index and kind == 'copy']
        left, right = inputs.output_files(mine, name), inputs.output_files(ref, name)
        if not right:
            return unavailable
        if [p.name for p in left] != [p.name for p in right]:
            return 'different'
        for actual, expected in zip(left, right):
            carriage = False
            description = decoded_output.probe(expected, programs['$FFPROBE'], env, raw)
            if description is None:
                return unavailable
            if any((output_index, stream) not in mapping for stream in range(len(description['streams']))):
                return unavailable
            if any(s.get('codec_name') in ('png', 'bmp') for s in description['streams']):
                if source is None or any(s.get('codec_type') == 'video' and
                    not s.get('pix_fmt', '').startswith(('rgb', 'bgr', 'gbr')) for s in source['streams']):
                    return unavailable
            verdict, reason = decoded_output.compare(actual, expected, programs['$FFMPEG'], programs['$FFPROBE'],
                                                env, scratch, inputs=raw, registered_codecs=EXACT_CODECS)
            aac_pcm = aac_decoded and len(command.sources) == len(command.outputs) == len(left) == 1 and (
                mapping.get((0, 0)) in aac.PCM and len(mapping) == 1)
            if aac_pcm and (verdict == 'verified' or (verdict, reason) == ('failed', 'decoded samples')):
                result = aac.compare(command, base, source, mapping, (verdict, reason), programs, scratch)
                if result == aac.COMPATIBLE:
                    checked = decoded_output.compare(actual, expected, programs['$FFMPEG'], programs['$FFPROBE'],
                        env, scratch, inputs=raw, registered_codecs=EXACT_CODECS, exact_frame_timeline=True)
                    if checked not in (('failed', 'decoded samples'), ('verified', 'exact decoded output')):
                        return 'different'
                return result
            if aac_decoded and verdict == 'verified' and not aac.exact_without_pns(
                    command, base, source, programs, scratch):
                return 'different'
            if verdict != 'verified':
                return unavailable if verdict == 'unverified' else 'different'
            if copied:
                a = packet_rows(actual, programs['$FFPROBE'], env, raw)
                b = packet_rows(expected, programs['$FFPROBE'], env, raw)
                if a is None or b is None:
                    return unavailable
                if [p for p in a if p[0] in copied] != [p for p in b if p[0] in copied]:
                    if not decoded_output.avc1_carriage(actual, expected, programs['$FFMPEG'], programs['$FFPROBE'],
                            env, scratch, copied, inputs=raw, registered_codecs=EXACT_CODECS):
                        return 'different'
                    outcome, unavailable = COMPATIBLE, 'different'
                    carriage = True
            if not carriage:
                timeline_checks.append((actual, expected, raw, copied))
    if outcome == COMPATIBLE:
        for actual, expected, raw, copied in timeline_checks:
            verdict, _ = decoded_output.compare(actual, expected, programs['$FFMPEG'], programs['$FFPROBE'],
                env, scratch, inputs=raw, copied_streams=copied, registered_codecs=EXACT_CODECS,
                exact_frame_timeline=True)
            if verdict != 'verified':
                return 'different'
    return outcome if command.outputs else unavailable


def judge(row, record, bundle, programs, env, scratch):
    try:
        command = inputs.instantiate(row['shape'])
    except inputs.Uninstantiable as error:
        if record != {'rank': row['rank'], 'uninstantiable': str(error)}:
            raise EvidenceError('uninstantiable_receipt')
        return 'uninstantiable'
    if record['rank'] != row['rank'] or record['tool'] != command.tool or list(record['command']) != list(command.args):
        raise EvidenceError('command_binding')
    base = bundle / 'calls' / str(row['rank'])
    arms = record['arms']
    for name, receipt in arms.items():
        if name not in ('reference', 'candidate'):
            raise EvidenceError('invocation_binding')
        validate_receipt(receipt, expected_invocation(command, name == 'candidate'), command.stdin, 'command')
        folder = base / name
        if (folder / 'stdout').read_bytes() != (folder / 'command.stdout').read_bytes():
            raise EvidenceError('stdout_binding')
        strict_fallback(folder / 'fallback.jsonl')
        for source, _ in command.sources:
            staged = folder / source.split('/')[0]
            expected = {p.name: digest(p) for p in (bundle / 'fixtures').iterdir()
                        if p.name.startswith(('source.', 'frame-')) or p.name == 'concat.txt'}
            actual = {p.name: digest(p) for p in staged.iterdir() if p.is_file()}
            if actual != expected:
                raise EvidenceError('input_binding')
    ref = arms['reference']
    if ref['code'] or ref['timed_out'] or not output_present(command, base / 'reference'):
        if set(arms) != {'reference'}:
            raise EvidenceError('failed_reference_roster')
        return 'reference_failed'
    if set(arms) != {'reference', 'candidate'}:
        raise EvidenceError('candidate_receipt_missing')
    if command.tool == 'ffprobe':
        if 'eligibility' not in record:
            raise EvidenceError('probe_witness_missing')
        validate_receipt(record['eligibility'], probe_witness_invocation(command), command.stdin, 'eligibility')
    candidate = arms['candidate']
    if candidate['timed_out']:
        return 'timeout'
    if candidate['code'] == 4:
        return 'refused'
    if candidate['code'] or not output_present(command, base / 'candidate'):
        return 'failed'
    if command.tool == 'ffprobe' and (record['eligibility']['code'] or record['eligibility']['timed_out']):
        return 'no_oracle'
    starts = json.loads((base / 'candidate' / 'starts.json').read_text())
    encoders = set(jobs(base / 'reference').values()) if command.tool == 'ffmpeg' else set()
    for start in starts:
        normalize_start(start, bundle)
        if not breadth.encoder_start(start['program'], start['args']):
            return 'unexpected_program'
        if start['program'] == 'x264' and 'libx264' not in encoders:
            return 'unexpected_program'
        if start['program'] == 'ffmpeg' and not (
                'aac' in encoders and breadth.aac_encoder_start(start['args']) or
                'libmp3lame' in encoders and breadth.mp3_encoder_start(start['args'])):
            return 'unexpected_program'
    if 'libx264' in encoders and not any(start['program'] == 'x264' for start in starts):
        return 'no_oracle'
    for codec, predicate in (('aac', breadth.aac_encoder_start), ('libmp3lame', breadth.mp3_encoder_start)):
        if codec in encoders and not any(start['program'] == 'ffmpeg' and predicate(start['args']) for start in starts):
            return 'no_oracle'
    return compare_outputs(command, base, programs, env, scratch)


def summarize(rows, outcomes):
    if [row['rank'] for row in rows] != list(range(1, 201)) or sum(row['repository_count'] for row in rows) != 1470:
        raise EvidenceError('summary_population')
    if [o['rank'] for o in outcomes] != [r['rank'] for r in rows]:
        raise EvidenceError('result_roster')
    counts, weights, tools = collections.Counter(), collections.Counter(), {}
    for row, outcome in zip(rows, outcomes):
        code = outcome['outcome']
        if code not in (*MISSES, COMPATIBLE, aac.COMPATIBLE, 'counted'):
            raise EvidenceError('result_class')
        counts[code] += 1
        weights[code] += row['repository_count']
        tool = tools.setdefault(row['shape']['tool'], {'shapes': collections.Counter(), 'pairs': collections.Counter()})
        tool['shapes'][code] += 1
        tool['pairs'][code] += row['repository_count']
    return {'set': 'github-shapes-200', 'set_sha256': roster.DIGEST, 'shapes': 200,
            'counted': counts['counted'], 'repository_pairs': 1470, 'counted_pairs': weights['counted'],
            COMPATIBLE: counts[COMPATIBLE], COMPATIBLE + '_pairs': weights[COMPATIBLE],
            aac.COMPATIBLE: counts[aac.COMPATIBLE], aac.COMPATIBLE + '_pairs': weights[aac.COMPATIBLE],
            'top_200_pair_share': weights['counted'] / 1470,
            'whole_sample_presence_lower_bound': weights['counted'] / 3338,
            'outcomes': {key: counts[key] for key in (*MISSES, COMPATIBLE, aac.COMPATIBLE, 'counted')},
            'outcome_pairs': {key: weights[key] for key in (*MISSES, COMPATIBLE, aac.COMPATIBLE, 'counted')}, 'tools': tools, 'bar': None}


def inventory(bundle):
    found = {}
    for path in sorted(bundle.rglob('*')):
        if path.is_symlink() or (not path.is_file() and not path.is_dir()):
            raise EvidenceError('evidence_file_type')
        if path.is_file() and path != bundle / 'manifest.json':
            found[str(path.relative_to(bundle))] = digest(path)
    return found


def verify_programs(opts):
    import numpy as np
    required = (ROOT / 'scripts/aac-pns-requirements.txt').read_text().splitlines()[0].split()[0].removeprefix('numpy==')
    if np.__version__ != required:
        raise EvidenceError('pns_dependency_version')
    identities = breadth.oracle_programs(opts)
    identities['pns_diagnostic'] = {'sha256': digest(Path(opts.pns_diagnostic)), 'command': aac.DIAGNOSTIC_BUILD}
    identities['numpy'] = {'version': np.__version__}
    pin = oracle_pin.load()
    version = subprocess.run([opts.x264, '--version'], capture_output=True, text=True, timeout=10, check=True).stdout
    if pin['X264_COMMIT'][:7] not in version:
        raise EvidenceError('x264_pin')
    identities['x264'] = {'sha256': digest(Path(opts.x264)), 'commit': pin['X264_COMMIT']}
    identities['coreav'] = {'sha256': digest(Path(opts.coreav))}
    linked = subprocess.run(['ldd', opts.coreav], capture_output=True, text=True, timeout=10, check=True,
                            env=environment()).stdout
    libraries = {}
    for line in linked.splitlines():
        match = re.search(r'=> (/\S+) \(', line)
        if match:
            path = Path(match[1])
            libraries[path.name] = digest(path)
    versions = subprocess.run(['pkg-config', '--modversion', 'dav1d', 'vpx', 'opus'],
                              capture_output=True, text=True, timeout=10, check=True).stdout.splitlines()
    if len(versions) != 3:
        raise EvidenceError('library_versions')
    identities['libraries'] = {'sha256': libraries, 'versions': dict(zip(('dav1d', 'vpx', 'opus'), versions))}
    return identities


def replay(opts):
    bundle = opts.bundle.resolve()
    if opts.summary.resolve().is_relative_to(bundle) or opts.scratch.resolve().is_relative_to(bundle):
        raise EvidenceError('replay_destinations')
    manifest = json.loads((bundle / 'manifest.json').read_text())
    if manifest['method'] != method() or manifest['inventory'] != inventory(bundle):
        raise EvidenceError('evidence_identity')
    if manifest['programs'] != verify_programs(opts) or manifest['features'] != FEATURES or manifest['build_command'] != BUILD:
        raise EvidenceError('program_identity')
    if manifest['build'] != {'commit': manifest['commit'], 'command': BUILD,
                            'coreav_sha256': manifest['programs']['coreav']['sha256'],
                            'lock_sha256': digest(ROOT / 'Cargo.lock')}:
        raise EvidenceError('build_binding')
    rows = roster.load()
    records = json.loads((bundle / 'records.json').read_text())
    if [record['rank'] for record in records] != [row['rank'] for row in rows]:
        raise EvidenceError('receipt_roster')
    programs = {'$COREAV': str(Path(opts.coreav).resolve()), '$FFMPEG': str(Path(opts.ffmpeg).resolve()),
                '$FFPROBE': str(Path(opts.ffprobe).resolve()), '$X264': str(Path(opts.x264).resolve()),
                '$AAC_DIAGNOSTIC': str(Path(opts.pns_diagnostic).resolve())}
    opts.scratch.mkdir(parents=True, exist_ok=False)
    outcomes = [{'rank': row['rank'], 'outcome': judge(row, record, bundle, programs, environment(), opts.scratch)}
                for row, record in zip(rows, records)]
    summary = summarize(rows, outcomes)
    summary['results'] = outcomes
    summary['commit'] = manifest['commit']
    write(opts.summary, summary)
    if manifest['inventory'] != inventory(bundle):
        raise EvidenceError('evidence_changed_during_replay')
    return 0


def run(opts):
    merged_source(opts.commit)
    if platform.system() != 'Linux' or platform.machine() != 'x86_64' or not os.environ.get('GITHUB_ACTIONS'):
        raise EvidenceError('actions_linux_required')
    rows = roster.load()
    identities = verify_programs(opts)
    build = json.loads(opts.build_record.read_text())
    if build != {'commit': opts.commit, 'command': BUILD, 'coreav_sha256': identities['coreav']['sha256'],
                 'lock_sha256': digest(ROOT / 'Cargo.lock')}:
        raise EvidenceError('build_record')
    programs = {'$COREAV': str(Path(opts.coreav).resolve()), '$FFMPEG': str(Path(opts.ffmpeg).resolve()),
                '$FFPROBE': str(Path(opts.ffprobe).resolve()), '$X264': str(Path(opts.x264).resolve()),
                '$AAC_DIAGNOSTIC': str(Path(opts.pns_diagnostic).resolve())}
    bundle = opts.bundle.resolve()
    bundle.mkdir(parents=True, exist_ok=False)
    env = environment()
    fixtures.generate(bundle / 'fixtures', programs['$FFMPEG'], env, execute)
    fixture_identity = inventory(bundle)
    initial_method = method()
    records = []
    for row in rows:
        if any(digest(bundle / path) != identity for path, identity in fixture_identity.items()):
            raise EvidenceError('fixture_changed')
        record = acquire_one(row, bundle, programs, env)
        records.append(record)
        write(bundle / 'records.json', records)
    write(bundle / 'host.json', {'system': platform.system(), 'machine': platform.machine(),
                               'kernel': platform.release(), 'image': os.environ.get('ImageVersion'),
                               'cpu': Path('/proc/cpuinfo').read_text().split('model name', 1)[-1].splitlines()[0],
                               'cores': os.cpu_count()})
    if identities != verify_programs(opts) or initial_method != method():
        raise EvidenceError('acquisition_identity_changed')
    if any(digest(bundle / path) != identity for path, identity in fixture_identity.items()):
        raise EvidenceError('fixture_changed')
    manifest = {'commit': opts.commit, 'method': initial_method, 'programs': identities, 'build': build,
                'features': FEATURES, 'build_command': BUILD, 'inventory': inventory(bundle)}
    write(bundle / 'manifest.json', manifest)
    return replay(opts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('run', 'replay'))
    for name in ('coreav', 'ffmpeg', 'ffprobe', 'x264', 'pns-diagnostic'):
        parser.add_argument('--' + name, required=True)
    for name in ('bundle', 'scratch', 'summary'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--commit')
    parser.add_argument('--build-record', type=Path)
    opts = parser.parse_args()
    if opts.mode == 'run' and (not opts.commit or not opts.build_record):
        parser.error('run requires --commit and --build-record')
    try:
        return run(opts) if opts.mode == 'run' else replay(opts)
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        reason = str(error) if isinstance(error, EvidenceError) else type(error).__name__
        print(f'github-shapes: stage={opts.mode} reason={reason}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
