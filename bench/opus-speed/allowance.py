import copy
import math
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'bench/audio-decode'))

import capacity
import run as capacity_run

LEAF_TOOLS = {'capacity.raw.base': 'base_abench', 'capacity.raw.head': 'head_abench',
    'capacity.cli.base': 'base_coreav', 'capacity.cli.head': 'head_coreav',
    'capacity.lib.base': 'base_abench', 'capacity.lib.head': 'head_abench', 'capacity.lav.library': 'lav',
    'capacity.decoder.base': 'base_abench', 'capacity.decoder.head': 'head_abench', 'capacity.lav.decoder': 'lav',
    'capacity.ffmpeg': 'ffmpeg', 'oracle.probe': 'ffprobe', 'oracle.reference_decode': 'ffmpeg',
    'oracle.default_decode': 'ffmpeg', 'oracle.coreav_decode': 'coreav', 'oracle.compare': 'opus_compare',
    'capacity.input': 'ffprobe', 'oracle.default_compare': 'opus_compare',
    'oracle.full_decode': 'ffmpeg', 'oracle.reader': 'ffprobe'}
REQUIRED_LEAVES = LEAF_TOOLS.keys() - {'oracle.full_decode', 'oracle.reader'}


def number(value):
    return type(value) in (int, float) and math.isfinite(value) and value > 0


def operation_seconds(document, stage, binding):
    fields = {'schema', 'purpose', 'binding_sha256', 'stage', 'events', 'failures'}
    if (not isinstance(document, dict) or set(document) != fields
            or document['schema'] != 'opus-capacity-call-observations/1'
            or document['purpose'] != 'allowance planning only; no speed verdict'
            or document['binding_sha256'] != capacity.canonical(binding)
            or document['stage'] != stage or document['failures'] != []
            or not isinstance(document['events'], list) or not 1 <= len(document['events']) <= 10000):
        raise ValueError('invalid allowance observations')
    identities = {(row['sha256'], row['class']) for row in binding['inputs']}
    result, leaves, leaf_seconds = {}, {identity: set() for identity in identities}, {identity: 0 for identity in identities}
    for event in document['events']:
        common = {'kind', 'sha256', 'class', 'operation', 'outcome', 'seconds'}
        if not isinstance(event, dict) or not number(event.get('seconds')):
            raise ValueError('invalid allowance event')
        if (event.get('sha256'), event.get('class')) not in identities:
            raise ValueError('unexpected allowance input')
        if event.get('kind') == 'operation':
            if (set(event) != common or event['operation'] not in capacity.OPERATIONS
                    or event['outcome'] != 'complete'):
                raise ValueError('incomplete allowance operation')
            key = event['sha256'], event['class'], event['operation']
            if key in result:
                raise ValueError('duplicate allowance operation')
            result[key] = event['seconds']
        elif (event.get('kind') != 'leaf' or set(event) != common | {'tool'}
              or event.get('operation') not in capacity.receipts.OPERATIONS
              or event.get('tool') not in capacity.receipts.TOOLS
              or event['tool'] != LEAF_TOOLS.get(event['operation'])
              or event.get('outcome') not in ('ok', 'disagree')):
            raise ValueError('invalid allowance leaf')
        else:
            identity = event['sha256'], event['class']
            leaves[identity].add(event['operation'])
            leaf_seconds[identity] += event['seconds']
    wanted = {(sha, cls, op) for sha, cls in identities for op in capacity.OPERATIONS}
    if set(result) != wanted:
        raise ValueError('missing allowance operation')
    for identity in identities:
        if (not REQUIRED_LEAVES <= leaves[identity]
                or leaf_seconds[identity] > sum(seconds for key, seconds in result.items() if key[:2] == identity)):
            raise ValueError('incomplete allowance leaf work')
    return result


def control_seconds(document):
    if not isinstance(document, dict) or set(document) != {'smoke', 'full'}:
        raise ValueError('invalid control evidence')
    gaps = []
    for stage, count in (('smoke', 1), ('full', 6)):
        value = document[stage]
        if (set(value) != {'schema', 'purpose', 'observation_resolution_seconds', 'invocation_seconds',
                          'observations', 'observer_errors'}
                or value['schema'] != 'opus-capacity-driver-scheduling/1'
                or value['purpose'] != 'deadline planning only; no speed verdict'
                or value['observer_errors'] != [] or not number(value['observation_resolution_seconds'])
                or not number(value['invocation_seconds'])):
            raise ValueError('invalid control observations')
        previous, last_complete = 0, None
        for event in value['observations']:
            if (set(event) != {'input', 'operation', 'status', 'observed_seconds'}
                    or type(event['input']) is not int or not 0 <= event['input'] < count
                    or event['operation'] not in capacity.OPERATIONS
                    or event['status'] not in ('pending', 'running', 'complete', 'interrupted')
                    or not number(event['observed_seconds'])
                    or not previous <= event['observed_seconds'] <= value['invocation_seconds']):
                raise ValueError('invalid control transition')
            previous = event['observed_seconds']
            if event['status'] == 'complete':
                last_complete = previous
            elif event['status'] == 'running' and last_complete is not None:
                gaps.append(previous - last_complete + 2 * value['observation_resolution_seconds'])
                last_complete = None
    if not gaps or min(gaps) <= 0:
        raise ValueError('missing control transitions')
    return math.ceil(3 * max(gaps))


def prepare(document, binding, receipt, control_binding):
    if (set(document) != {'schema', 'full', 'smoke', 'smoke_receipt', 'control', 'control_binding_sha256',
                         'acquisition', 'lifecycle', 'headroom', 'vm_margin_seconds'}
            or document['schema'] != 'opus-speed-allowance/1' or document['headroom'] != 2
            or document['vm_margin_seconds'] != 300
            or document['control_binding_sha256'] != capacity.canonical(control_binding)
            or any(binding[field] != control_binding[field]
                   for field in ('inputs', 'tools', 'oracle', 'runtime', 'limits'))):
        raise ValueError('invalid allowance evidence')
    smoke = document['smoke_receipt']
    small = capacity.validate_binding(smoke['binding'])
    if (len(small['inputs']) != 1 or small['limits'] != {**binding['limits'], 'per_call_seconds': 60, 'job_seconds': 600}
            or any(small[field] != binding[field] for field in ('tools', 'oracle', 'runtime', 'judging', 'scope'))
            or smoke['host'] != receipt['host'] or smoke['complete'] is not True or smoke['passed'] is not False):
        raise ValueError('changed startup evidence')
    expected = capacity_run.new_receipt(small, small['inputs'], smoke['host'], True)
    capacity_run.validate_checkpoint(smoke, expected)
    checked = copy.deepcopy(smoke['inputs'][0])
    capacity_run.reconcile(checked, small['inputs'][0])
    if (checked != smoke['inputs'][0]
            or any(slot['status'] != 'complete' or slot['error'] is not None for slot in checked['operations'].values())):
        raise ValueError('incomplete startup work')
    full_times = operation_seconds(document['full'], 'full', binding)
    small_times = operation_seconds(document['smoke'], 'smoke', small)
    startup = {op: max(seconds for key, seconds in small_times.items() if key[2] == op)
               for op in capacity.OPERATIONS}
    acquisition, lifecycle = document['acquisition'], document['lifecycle']
    if (set(acquisition) != {'seconds', 'bytes', 'files'} or not number(acquisition['seconds'])
            or not capacity.positive(acquisition['bytes']) or acquisition['files'] != 7
            or set(lifecycle) != {'vm_to_archive_seconds', 'full_invocation_seconds', 'smoke_invocation_seconds'}
            or not all(number(value) for value in lifecycle.values())):
        raise ValueError('invalid setup evidence')
    setup = (lifecycle['vm_to_archive_seconds'] - lifecycle['full_invocation_seconds']
             - lifecycle['smoke_invocation_seconds'] - acquisition['seconds'])
    if (setup <= 0 or sum(full_times.values()) > lifecycle['full_invocation_seconds']
            or sum(small_times.values()) > lifecycle['smoke_invocation_seconds']):
        raise ValueError('inconsistent setup clocks')
    cells = {}
    for source, row in zip(binding['inputs'], receipt['inputs'], strict=True):
        identity = source['sha256'], source['class']
        cells[source['mode'], source['class']] = {
            'samples': row['operations']['raw_identity']['result']['frames'] * source['channels'],
            'packets': sum(source['packets'].values()),
            'seconds': {op: full_times[identity[0], identity[1], op] for op in capacity.OPERATIONS}}
    return {'cells': cells, 'startup': startup, 'control_seconds': control_seconds(document['control']),
            'setup_seconds': math.ceil(document['headroom'] * setup), 'acquisition': acquisition,
            'headroom': document['headroom'], 'vm_margin_seconds': document['vm_margin_seconds']}


def forecast(record, plan):
    data = plan['_allowance']
    gates = ('raw_identity', 'cli_identity', 'oracle')
    timed = ('head_lib', 'base_lib', 'lav_lib', 'head_dec', 'base_dec', 'lav_dec', 'cli_identity', 'ffmpeg_cli')
    forecasts = []
    for mode in {label.split('/')[0] for label in record['packets']}:
        cell = data['cells'][mode, record['class']]
        scale = max(record['raw_frames'] * record['channels'] / cell['samples'],
                    sum(record['packets'].values()) / cell['packets'])
        duration = lambda op: data['startup'][op] + cell['seconds'][op] * scale
        work = sum(duration(op) for op in gates) + (plan['warmups'] + plan['rounds']) * sum(duration(op) for op in timed)
        forecasts.append(math.ceil(data['headroom'] * (work + data['control_seconds'])))
    return max(forecasts)


def admit(indexed, expected, plan):
    data = plan['_allowance']
    result = {}
    for shard, sources in expected['shards'].items():
        records = [indexed[source, cls] for source in sources for cls in ('mp4', 'matroska')]
        ready = [row for row in records if row['status'] == 'ok']
        work = sum(forecast(row, plan) for row in ready)
        acquisition = data['acquisition']
        scale = max(1, len(ready) / acquisition['files'], sum(row['file_bytes'] for row in ready) / acquisition['bytes'])
        transfer = math.ceil(data['headroom'] * acquisition['seconds'] * scale)
        vm = data['setup_seconds'] + transfer + plan['shard_seconds'] + data['vm_margin_seconds']
        if work > plan['shard_seconds'] or vm > plan['vm_seconds']:
            raise ValueError('roster exceeds registered allowance')
        result[shard] = {'work_seconds': work, 'acquisition_seconds': transfer,
                         'setup_seconds': data['setup_seconds'], 'vm_seconds': vm}
    return result


def main(argv=None):
    parser = capacity.Parser(description='Bind planning-only observations to complete spent-input capacity evidence')
    for name in ('full', 'smoke', 'smoke-receipt', 'control-full', 'control-smoke', 'acquisition',
                 'lifecycle', 'receipt', 'binding', 'out'):
        parser.add_argument('--' + name, type=pathlib.Path, required=True)
    try:
        args = parser.parse_args(argv)
        binding, receipt = capacity.load(args.binding), capacity.load(args.receipt)
        capacity.validate_capacity(receipt, binding)
        control = capacity.load(ROOT / 'bench/audio-decode/evidence/opus-round4-binding.json')
        acquisition = capacity.load(args.acquisition)
        if (set(acquisition) != {'schema', 'purpose', 'seconds', 'bytes'}
                or acquisition['schema'] != 'opus-capacity-acquisition/1'
                or acquisition['purpose'] != 'allowance planning only'):
            raise ValueError
        document = {'schema': 'opus-speed-allowance/1', 'full': capacity.load(args.full),
            'smoke': capacity.load(args.smoke), 'smoke_receipt': capacity.load(args.smoke_receipt),
            'control': {'full': capacity.load(args.control_full), 'smoke': capacity.load(args.control_smoke)},
            'control_binding_sha256': capacity.canonical(control),
            'acquisition': {'seconds': acquisition['seconds'], 'bytes': acquisition['bytes'], 'files': 7},
            'lifecycle': capacity.load(args.lifecycle), 'headroom': 2, 'vm_margin_seconds': 300}
        prepare(document, binding, receipt, control)
        capacity_run.receipts.atomic_write(args.out, document)
        print('Bound planning evidence written; no speed judgment')
        return 0
    except SystemExit as error:
        return 0 if error.code == 0 else 1
    except BaseException:
        print('Invalid or incomplete planning evidence')
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
