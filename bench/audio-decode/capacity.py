#!/usr/bin/env python3
"""Validate untimed Opus capacity evidence before a new registration."""
import argparse
import hashlib
import importlib.metadata
import json
import os
import pathlib
import re
import subprocess
import sys
import tarfile
import tempfile
from dataclasses import dataclass

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import oracle
import receipts
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / 'scripts'))
import oracle_pin

INPUT_RATE = 48000
MIN_FULL_FRAMES = 900 * INPUT_RATE
OPERATIONS = ('raw_identity', 'cli_identity', 'oracle', 'head_lib', 'base_lib',
              'lav_lib', 'head_dec', 'base_dec', 'lav_dec', 'ffmpeg_cli')
TOOLS = ('base_abench', 'head_abench', 'base_coreav', 'head_coreav',
         'lav', 'ffmpeg', 'ffprobe', 'opus_compare')
SCRIPT_INPUTS = ('scripts/opus_identity.py', 'scripts/oracle_audio.py',
                 'scripts/oracle-audio-args.txt', 'scripts/decode_audio_check.py',
                 'scripts/oracle_kd.py', 'scripts/oracle_kd_ts.py',
                 'scripts/oracle-ffmpeg.env', 'scripts/oracle_pin.py',
                 'docs/known-divergences-decode-audio.json')
JUDGING_INPUTS = (*SCRIPT_INPUTS, *(f'bench/audio-decode/{name}.py'
                  for name in ('capacity', 'run', 'receipts', 'oracle', 'agreement')))


class Invalid(Exception):
    """A closed validation failure whose code contains no supplied text."""

    def __init__(self, code):
        super().__init__(code)
        self.code = code


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise Invalid('invalid_capacity_arguments')


@dataclass(frozen=True)
class ValidatedCapacity:
    """The identity of evidence accepted by the one capacity validator."""

    binding_sha256: str


def canonical(value):
    try:
        raw = json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()
    except (TypeError, ValueError):
        raise Invalid('invalid_json_value') from None
    return hashlib.sha256(raw).hexdigest()


def digest(path):
    value = hashlib.sha256()
    try:
        with pathlib.Path(path).open('rb') as source:
            while block := source.read(4 << 20):
                value.update(block)
    except OSError:
        raise Invalid('unavailable_bound_file') from None
    return value.hexdigest()


def verify_tools(repo, tools):
    if os.environ.get('LD_PRELOAD') or os.environ.get('LD_AUDIT'):
        raise Invalid('injected_baseline_loader')
    pin = oracle_pin.load(pathlib.Path(repo))
    binaries = [pathlib.Path(tools[name]).resolve() for name in ('ffmpeg', 'ffprobe')]
    if (any(path.name != name for path, name in zip(binaries, ('ffmpeg', 'ffprobe')))
            or binaries[0].parent != binaries[1].parent
            or list(oracle_pin.verify_bin(binaries[0].parent, pathlib.Path(repo)))):
        raise Invalid('oracle_pin_mismatch')
    archive = pathlib.Path(tools['lav_archive'])
    if digest(archive) != pin['LAV_SHA256']:
        raise Invalid('baseline_archive_pin_mismatch')
    try:
        done = subprocess.run(['ldd', str(pathlib.Path(tools['lav']).resolve())],
                              capture_output=True, check=True, timeout=10)
        libraries = {}
        for line in done.stdout.decode().splitlines():
            match = re.fullmatch(r'\s*(\S+) => (/.+) \(0x[0-9a-f]+\)\s*', line)
            direct = re.fullmatch(r'\s*(/.+) \(0x[0-9a-f]+\)\s*', line)
            if direct:
                path = direct[1]
                name = pathlib.Path(path).name
            if match:
                name, path = match.groups()
            if match or direct:
                if name in libraries:
                    raise Invalid('ambiguous_baseline_library')
                libraries[name] = digest(path)
        required = ('libavcodec', 'libavformat', 'libavutil', 'libswresample')
        if any(not any(name.startswith(prefix + '.so.') for name in libraries) for prefix in required):
            raise Invalid('missing_baseline_library')
        with tarfile.open(archive, 'r:xz') as source:
            members = source.getmembers()
            for name, actual in libraries.items():
                if not re.fullmatch(r'lib(?:avcodec|avformat|avutil|avfilter|avdevice|swresample|swscale)\.so\.\d+', name):
                    continue
                matches = [member for member in members if member.isfile()
                           and member.name.startswith(pin['LAV_ARCHIVE'] + '/lib/' + name)
                           and re.fullmatch(re.escape(name) + r'(?:\.\d+)*', pathlib.PurePosixPath(member.name).name)]
                if len(matches) != 1:
                    raise Invalid('missing_pinned_baseline_library')
                value = hashlib.sha256()
                with source.extractfile(matches[0]) as incoming:
                    while block := incoming.read(4 << 20):
                        value.update(block)
                if value.hexdigest() != actual:
                    raise Invalid('baseline_library_pin_mismatch')
    except (OSError, ValueError, UnicodeError, tarfile.TarError, subprocess.SubprocessError):
        raise Invalid('unverifiable_baseline_libraries') from None
    return {'pin_sha256': digest(pathlib.Path(repo) / 'scripts/oracle-ffmpeg.env'),
            'lav_archive_sha256': digest(archive), 'libraries': libraries}


def load(path):
    def pairs(items):
        out = {}
        for key, value in items:
            if key in out:
                raise Invalid('duplicate_json_key')
            out[key] = value
        return out

    def constant(_):
        raise Invalid('nonfinite_json_value')

    try:
        with pathlib.Path(path).open('rb') as source:
            raw = source.read((4 << 20) + 1)
        if len(raw) > 4 << 20:
            raise Invalid('oversized_json_document')
        return json.loads(raw, object_pairs_hook=pairs, parse_constant=constant)
    except (OSError, ValueError, UnicodeError):
        raise Invalid('unreadable_json_document') from None


def atomic(path, value):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as target:
            temporary = pathlib.Path(target.name)
            json.dump(value, target, sort_keys=True, indent=1, allow_nan=False)
            target.write('\n')
            target.flush()
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def sha(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def positive(value):
    return type(value) is int and 0 < value < 1 << 63


def result_valid(operation, result):
    if not isinstance(result, dict):
        return False
    if operation == 'oracle':
        return oracle.validate(result) and result['status'] == 'ok'
    fields = {
        'raw_identity': {'passed', 'packets', 'frames', 'channels'},
        'cli_identity': {'passed', 'frames', 'channels', 'sample_rate', 'sample_width', 'output_bytes'},
        'ffmpeg_cli': {'frames', 'channels', 'sample_rate', 'sample_width', 'output_bytes'},
        'head_lib': {'samples', 'bytes'}, 'base_lib': {'samples', 'bytes'},
        'lav_lib': {'samples', 'bytes', 'errors'},
        'head_dec': {'samples'}, 'base_dec': {'samples'},
        'lav_dec': {'samples', 'packets', 'errors'},
    }[operation]
    if set(result) != fields:
        return False
    if 'passed' in fields and type(result['passed']) is not bool:
        return False
    if 'errors' in fields and (type(result['errors']) is not int or result['errors'] != 0):
        return False
    if not all(type(result[key]) is int and 0 < result[key] <= receipts.COUNT_LIMIT
               for key in fields - {'passed', 'errors'}):
        return False
    return ('sample_rate' not in fields or (result['sample_rate'] == INPUT_RATE
                                           and result['sample_width'] == 2))


def records(inputs):
    if not isinstance(inputs, dict) or inputs.get('scope') != 'spent':
        raise Invalid('spent_inputs_required')
    values = inputs.get('inputs')
    if not isinstance(values, list) or not values:
        raise Invalid('empty_input_roster')
    out, seen = [], set()
    frames = {'celt': (2.5, 5, 10, 20), 'hybrid': (10, 20), 'silk': (10, 20, 40, 60)}
    for row in values:
        if (not isinstance(row, dict) or not sha(row.get('sha256'))
                or row.get('class') not in ('mp4', 'matroska')
                or type(row.get('channels')) is not int or row['channels'] not in (1, 2)
                or not isinstance(row.get('file'), str) or not row['file']
                or not isinstance(row.get('packets'), dict) or not row['packets']):
            raise Invalid('invalid_input_record')
        modes = {}
        for label, count in row['packets'].items():
            try:
                mode, frame = label.split('/')
                valid = mode in frames and float(frame) in frames[mode] and positive(count)
            except (ValueError, TypeError, AttributeError):
                valid = False
            if not valid:
                raise Invalid('invalid_packet_roster')
            modes[mode] = modes.get(mode, 0) + count
        key = row['sha256'], row['class']
        if key in seen:
            raise Invalid('duplicate_input')
        seen.add(key)
        out.append({name: row[name] for name in ('file', 'sha256', 'class', 'channels', 'packets')}
                   | {'mode': min(modes, key=lambda mode: (-modes[mode], mode))})
    return sorted(out, key=lambda row: (row['sha256'], row['class']))


def normalized_limits(limits):
    fields = {'per_call_seconds', 'job_seconds', 'concurrency', 'memory_bytes', 'tmpfs_bytes'}
    if (not isinstance(limits, dict) or set(limits) != fields
            or any(not positive(limits[key]) for key in fields)
            or limits['concurrency'] != 1 or limits['per_call_seconds'] > 7200
            or limits['job_seconds'] > 21600 or limits['per_call_seconds'] > limits['job_seconds']):
        raise Invalid('invalid_capacity_limits')
    return dict(limits)


def judging(repo):
    repo = pathlib.Path(repo).resolve()
    names = set(SCRIPT_INPUTS)
    names.update(str(path.relative_to(repo)) for path in (repo / 'bench/audio-decode').glob('*.py'))
    if not set(JUDGING_INPUTS) <= names:
        raise Invalid('missing_judging_source')
    return {name: digest(repo / name) for name in sorted(names)}


def make_binding(repo, inputs, media, tools, limits):
    if not isinstance(tools, dict) or set(tools) != set(TOOLS) | {'lav_archive'}:
        raise Invalid('invalid_tool_roster')
    provenance = verify_tools(repo, tools)
    media = pathlib.Path(media).resolve()
    bound = []
    for row in records(inputs):
        path = (media / row['file']).resolve()
        suffix = '.mp4' if row['class'] == 'mp4' else '.webm'
        if media not in path.parents or path.suffix != suffix or digest(path) != row['sha256']:
            raise Invalid('changed_or_invalid_input_bytes')
        bound.append({key: value for key, value in row.items() if key != 'file'})
    try:
        numpy = importlib.metadata.version('numpy')
    except importlib.metadata.PackageNotFoundError:
        raise Invalid('numpy_unavailable') from None
    if re.fullmatch(r'\d+\.\d+\.\d+(?:[a-z0-9.+-]*)', numpy) is None:
        raise Invalid('invalid_runtime_version')
    return {'schema': 'opus-capacity-binding/1', 'scope': 'spent', 'inputs': bound,
            'tools': {role: digest(tools[role]) for role in TOOLS}, 'oracle': provenance, 'judging': judging(repo),
            'runtime': {'python': f'{sys.version_info.major}.{sys.version_info.minor}', 'numpy': numpy},
            'operations': list(OPERATIONS), 'minimum_full_frames': MIN_FULL_FRAMES,
            'limits': normalized_limits(limits)}


def validate_binding(binding):
    if (not isinstance(binding, dict) or set(binding) != {'schema', 'scope', 'inputs', 'tools',
            'judging', 'oracle', 'runtime', 'operations', 'minimum_full_frames', 'limits'}
            or binding['schema'] != 'opus-capacity-binding/1' or binding['scope'] != 'spent'
            or binding['operations'] != list(OPERATIONS) or binding['minimum_full_frames'] != MIN_FULL_FRAMES
            or not isinstance(binding['tools'], dict) or set(binding['tools']) != set(TOOLS)
            or not all(sha(value) for value in binding['tools'].values())
            or not isinstance(binding['judging'], dict)
            or not all(isinstance(key, str) and sha(value) for key, value in binding['judging'].items())
            or not set(JUDGING_INPUTS) <= set(binding['judging'])
            or not isinstance(binding['runtime'], dict) or set(binding['runtime']) != {'python', 'numpy'}
            or not all(isinstance(value, str) and re.fullmatch(r'[0-9]+(?:\.[0-9]+)+(?:[a-z0-9.+-]*)', value)
                       for value in binding['runtime'].values())):
        raise Invalid('invalid_binding_manifest')
    provenance = binding['oracle']
    if (not isinstance(provenance, dict) or set(provenance) != {'pin_sha256', 'lav_archive_sha256', 'libraries'}
            or provenance['pin_sha256'] != binding['judging']['scripts/oracle-ffmpeg.env']
            or provenance['lav_archive_sha256'] != oracle_pin.load()['LAV_SHA256']
            or not isinstance(provenance['libraries'], dict)
            or not all(isinstance(name, str) and re.fullmatch(r'[A-Za-z0-9_.+-]+', name) and sha(value)
                       for name, value in provenance['libraries'].items())
            or any(not any(name.startswith(prefix + '.so.') for name in provenance['libraries'])
                   for prefix in ('libavcodec', 'libavformat', 'libavutil', 'libswresample'))):
        raise Invalid('invalid_oracle_provenance')
    normalized_limits(binding['limits'])
    inputs = {'scope': 'spent', 'inputs': [dict(row, file='bound') for row in binding['inputs']]}
    expected = [{key: value for key, value in row.items() if key != 'file'} for row in records(inputs)]
    if binding['inputs'] != expected:
        raise Invalid('invalid_bound_input_roster')
    return binding


def validate_capacity(receipt, expected_binding):
    binding = validate_binding(expected_binding)
    if (not isinstance(receipt, dict) or receipt.get('schema') != 'opus-capacity/1'
            or set(receipt) != {'schema', 'purpose', 'binding', 'host', 'inputs', 'complete', 'passed'}
            or receipt.get('purpose') != 'full' or receipt.get('binding') != binding
            or receipt.get('complete') is not True or type(receipt.get('passed')) is not bool):
        raise Invalid('capacity_incomplete_or_unbound')
    host = receipt.get('host', {})
    if (not isinstance(host, dict) or set(host) != {'cpu', 'smt', 'outputs_tmpfs',
                                                  'memory_as_bytes', 'tmpfs_budget_bytes', 'tmpfs_capacity_bytes'}):
        raise Invalid('invalid_capacity_host')
    cpu = re.sub(r'\((?:r|tm)\)', '', str(host.get('cpu', '')).lower())
    if (not 'intel xeon platinum 8581c' in ' '.join(cpu.split()) or host.get('smt') is not False
            or host.get('outputs_tmpfs') is not True
            or host.get('memory_as_bytes') != binding['limits']['memory_bytes']
            or host.get('tmpfs_budget_bytes') != binding['limits']['tmpfs_bytes']
            or host.get('tmpfs_capacity_bytes') != binding['limits']['tmpfs_bytes']):
        raise Invalid('unqualified_capacity_host')
    rows = receipt.get('inputs')
    if not isinstance(rows, list) or len(rows) != len(binding['inputs']):
        raise Invalid('incomplete_capacity_roster')
    seen, cells = set(), set()
    by_key = {(row['sha256'], row['class']): row for row in binding['inputs']}
    for row in rows:
        if not isinstance(row, dict) or set(row) != {'sha256', 'class', 'mode', 'operations'}:
            raise Invalid('invalid_capacity_input')
        key = row.get('sha256'), row.get('class')
        if key not in by_key or key in seen or row.get('mode') != by_key[key]['mode']:
            raise Invalid('unexpected_capacity_input')
        seen.add(key)
        cells.add((row['mode'], row['class']))
        ops = row.get('operations', {})
        if not isinstance(ops, dict) or set(ops) != set(OPERATIONS):
            raise Invalid('incomplete_operation_roster')
        results = {}
        for name, op in ops.items():
            if (not isinstance(op, dict) or op.get('status') != 'complete' or op.get('error') is not None
                    or set(op) != {'status', 'result', 'error'}
                    or not result_valid(name, op.get('result'))):
                raise Invalid('failed_or_incomplete_operation')
            results[name] = op['result']
        raw, cli, oracle = (results[name] for name in ('raw_identity', 'cli_identity', 'oracle'))
        channels, packets = by_key[key]['channels'], sum(by_key[key]['packets'].values())
        if (raw.get('passed') is not True or raw.get('packets') != packets
                or raw.get('channels') != channels or not positive(raw.get('frames'))
                or raw['frames'] < MIN_FULL_FRAMES):
            raise Invalid('missing_full_length_raw_work')
        if (cli.get('passed') is not True or not positive(cli.get('frames'))
                or cli.get('channels') != channels or cli.get('sample_rate') != INPUT_RATE
                or cli.get('sample_width') != 2 or not positive(cli.get('output_bytes'))
                or cli['output_bytes'] < cli['frames'] * channels * 2 + 44):
            raise Invalid('incomplete_command_identity')
        if (oracle.get('schema') != 'opus-oracle/1' or oracle.get('status') != 'ok'
                or type(oracle.get('passed')) is not bool or oracle.get('error') is not None
                or not isinstance(oracle.get('detail'), dict)):
            raise Invalid('failed_oracle_operation')
        samples = oracle['detail'].get('samples', {})
        if (not isinstance(samples, dict) or set(samples) != {'ours', 'ref', 'default'}
                or any(not positive(value) or value < MIN_FULL_FRAMES - 5760 for value in samples.values())
                or samples['ours'] != cli['frames']):
            raise Invalid('incomplete_oracle_sample_work')
        detail = oracle['detail']
        if row['class'] == 'mp4':
            if (not {'model', 'full', 'ffmpeg_expected'} <= set(detail)
                    or detail['model'] != cli['frames']
                    or abs(detail['full'] - raw['frames']) > 5760):
                raise Invalid('incomplete_oracle_model_work')
            expected = detail['ffmpeg_expected']
        else:
            if abs(cli['frames'] - raw['frames']) > 5760:
                raise Invalid('incomplete_oracle_presentation_work')
            expected = cli['frames']
        if samples['ref'] != expected or samples['default'] != expected:
            raise Invalid('incomplete_oracle_sample_work')
        raw_bytes = 2 * (8 + packets * 32 + raw['frames'] * channels * 4)
        if max(raw_bytes, cli['output_bytes'] * 2) > binding['limits']['tmpfs_bytes']:
            raise Invalid('insufficient_temporary_output_budget')
        for name in ('head_dec', 'base_dec', 'lav_dec', 'head_lib', 'base_lib', 'lav_lib'):
            work = results[name]
            if not positive(work.get('samples')) or work.get('errors', 0) != 0:
                raise Invalid('missing_engine_work')
            target = raw['frames'] if name.endswith('_dec') else cli['frames']
            allowance = 0 if name in ('head_dec', 'base_dec') else 5760
            if abs(work['samples'] - target) > allowance:
                raise Invalid('incomplete_engine_sample_count')
            if name.endswith('_lib') and work.get('bytes') != work['samples'] * channels * 2:
                raise Invalid('incomplete_library_bytes')
        if results['lav_dec'].get('packets') != packets or results['lav_dec'].get('errors') != 0:
            raise Invalid('incomplete_lav_packet_work')
        ff = results['ffmpeg_cli']
        if (not positive(ff.get('frames')) or abs(ff['frames'] - cli['frames']) > 5760
                or ff.get('channels') != channels or ff.get('sample_rate') != INPUT_RATE
                or ff.get('sample_width') != 2 or not positive(ff.get('output_bytes'))
                or ff['output_bytes'] < ff['frames'] * channels * 2 + 44):
            raise Invalid('incomplete_ffmpeg_command_work')
    if cells != {(mode, cls) for mode in ('celt', 'hybrid', 'silk') for cls in ('mp4', 'matroska')}:
        raise Invalid('missing_mode_container_cell')
    return ValidatedCapacity(canonical(binding))


def qualify(receipt, expected_binding):
    try:
        validate_capacity(receipt, expected_binding)
        return True
    except (Invalid, KeyError, TypeError, ValueError, OverflowError):
        return False


def check_registrations(repo):
    from markdown_it import MarkdownIt

    repo = pathlib.Path(repo).resolve()
    document = repo / 'docs/design/audio-decode.md'
    tokens = MarkdownIt().parse(document.read_text())
    sections = []
    for index, token in enumerate(tokens):
        if token.type == 'heading_open' and index + 1 < len(tokens):
            title = tokens[index + 1].content
            match = re.match(r'Opus speed,\s*round\s+(\d+)\b', title, re.I)
            if match:
                sections.append((int(match.group(1)), token.map[0], int(token.tag[1:])))
    lines, seen = document.read_text().splitlines(), set()
    pattern = re.compile(r'^Capacity preflight: `([^`]+)` sha256:`([0-9a-f]{64})`; binding: `([^`]+)` sha256:`([0-9a-f]{64})`\.$', re.M)
    for number, first, level in sections:
        if number in seen:
            raise Invalid('duplicate_opus_registration')
        seen.add(number)
        if number <= 3:
            continue
        end = len(lines)
        for token in tokens:
            if token.type == 'heading_open' and token.map[0] > first and int(token.tag[1:]) <= level:
                end = token.map[0]
                break
        refs = pattern.findall('\n'.join(lines[first:end]))
        if len(refs) != 1:
            raise Invalid('new_opus_registration_needs_capacity_preflight')
        receipt_name, receipt_hash, binding_name, binding_hash = refs[0]
        paths = [(repo / name).resolve() for name in (receipt_name, binding_name)]
        if any(repo not in path.parents for path in paths):
            raise Invalid('capacity_reference_outside_repository')
        if [digest(path) for path in paths] != [receipt_hash, binding_hash]:
            raise Invalid('changed_capacity_reference')
        binding = load(paths[1])
        if binding.get('judging') != judging(repo):
            raise Invalid('capacity_preflight_uses_changed_judging_code')
        validate_capacity(load(paths[0]), binding)


def main():
    parser = Parser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    validate = sub.add_parser('validate')
    validate.add_argument('--receipt', type=pathlib.Path, required=True)
    validate.add_argument('--binding', type=pathlib.Path, required=True)
    registration = sub.add_parser('check-registrations')
    registration.add_argument('--repo', type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[2])
    try:
        args = parser.parse_args()
        if args.command == 'validate':
            result = validate_capacity(load(args.receipt), load(args.binding))
            print(json.dumps({'qualified': True, 'binding_sha256': result.binding_sha256}))
        else:
            check_registrations(args.repo)
            print('Opus registrations satisfy the capacity prerequisite')
        return 0
    except Invalid as error:
        print(json.dumps({'qualified': False, 'reason': error.code}))
        return 1
    except Exception:
        print(json.dumps({'qualified': False, 'reason': 'capacity_validation_failure'}))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
