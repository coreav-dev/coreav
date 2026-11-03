import copy
import math
import pathlib
import sys
import unittest
import tempfile
import types
import time
from unittest.mock import patch

HERE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
import speed


def observations(binding, stage, seconds):
    events = []
    leaves = ('capacity.raw.base', 'capacity.raw.head', 'capacity.cli.base', 'capacity.cli.head',
              'capacity.lib.base', 'capacity.lib.head', 'capacity.lav.library', 'capacity.decoder.base',
              'capacity.decoder.head', 'capacity.lav.decoder', 'capacity.ffmpeg', 'oracle.probe',
              'oracle.reference_decode', 'oracle.default_decode', 'oracle.coreav_decode', 'oracle.compare',
              'capacity.input', 'oracle.default_compare')
    for row in binding['inputs']:
        identity = {'sha256': row['sha256'], 'class': row['class']}
        events += [{'kind': 'leaf', **identity, 'operation': op, 'tool': speed.allowance.LEAF_TOOLS[op],
                    'outcome': 'ok', 'seconds': seconds / 100} for op in leaves]
        events += [{'kind': 'operation', **identity, 'operation': op, 'outcome': 'complete', 'seconds': seconds}
                   for op in speed.capacity.OPERATIONS]
    return {'schema': 'opus-capacity-call-observations/1', 'purpose': 'allowance planning only; no speed verdict',
            'binding_sha256': speed.capacity.canonical(binding), 'stage': stage, 'events': events, 'failures': []}


def fixture():
    receipt = speed.capacity.load(speed.ROOT / 'bench/audio-decode/evidence/opus-round4-capacity.json')
    binding = copy.deepcopy(receipt['binding'])
    smoke = copy.deepcopy(receipt)
    smoke.update(purpose='smoke', passed=False)
    smoke['inputs'] = smoke['inputs'][:1]
    smoke['binding']['inputs'] = smoke['binding']['inputs'][:1]
    smoke['binding']['limits'].update(per_call_seconds=60, job_seconds=600)
    control = {'schema': 'opus-capacity-driver-scheduling/1', 'purpose': 'deadline planning only; no speed verdict',
        'observation_resolution_seconds': 0.5, 'invocation_seconds': 3, 'observer_errors': [],
        'observations': [{'input': 0, 'operation': 'raw_identity', 'status': 'complete', 'observed_seconds': 1},
                         {'input': 0, 'operation': 'cli_identity', 'status': 'running', 'observed_seconds': 2}]}
    document = {'schema': 'opus-speed-allowance/1', 'full': observations(binding, 'full', 1),
        'smoke': observations(smoke['binding'], 'smoke', 0.01), 'smoke_receipt': smoke,
        'control': {'smoke': control, 'full': copy.deepcopy(control)},
        'control_binding_sha256': speed.capacity.canonical(binding),
        'acquisition': {'seconds': 1, 'bytes': 1000000, 'files': 7},
        'lifecycle': {'vm_to_archive_seconds': 171, 'full_invocation_seconds': 65, 'smoke_invocation_seconds': 5},
        'headroom': 2, 'vm_margin_seconds': 300}
    return document, binding, receipt


class AllowanceTests(unittest.TestCase):
    def setUp(self):
        self.document, self.binding, self.receipt = fixture()

    def prepare(self, document=None):
        return speed.allowance.prepare(self.document if document is None else document,
                                        self.binding, self.receipt, self.binding)

    def test_every_complete_call_and_control_transition_is_replayed(self):
        data = self.prepare()
        self.assertEqual(len(data['cells']), 6)
        self.assertEqual(data['startup'], {name: 0.01 for name in speed.capacity.OPERATIONS})
        self.assertEqual(data['control_seconds'], 6)
        self.assertEqual(data['setup_seconds'], 200)
        for stage in ('full', 'smoke'):
            for mutation in ('missing', 'duplicate', 'failed', 'bad_clock', 'changed_binding', 'wrong_tool', 'missing_leaf'):
                bad = copy.deepcopy(self.document)
                events = bad[stage]['events']
                if mutation == 'missing':
                    events.pop()
                elif mutation == 'duplicate':
                    events.append(copy.deepcopy(events[-1]))
                elif mutation == 'failed':
                    events[-1]['outcome'] = 'exception'
                elif mutation == 'bad_clock':
                    events[-1]['seconds'] = True
                elif mutation == 'changed_binding':
                    bad[stage]['binding_sha256'] = '0' * 64
                elif mutation == 'wrong_tool':
                    events[0]['tool'] = 'head_abench'
                else:
                    events[:] = [event for event in events if event['kind'] != 'leaf']
                with self.subTest(stage=stage, mutation=mutation), self.assertRaises(ValueError):
                    self.prepare(bad)

    def test_startup_work_and_lifecycle_cannot_be_incomplete_or_inconsistent(self):
        for mutation in ('incomplete_smoke', 'bad_model', 'bad_setup', 'bad_control', 'extra_control'):
            bad = copy.deepcopy(self.document)
            if mutation == 'incomplete_smoke':
                bad['smoke_receipt']['complete'] = False
            elif mutation == 'bad_model':
                bad['smoke_receipt']['inputs'][0]['operations']['raw_identity']['result']['frames'] = 1
            elif mutation == 'bad_setup':
                bad['lifecycle']['vm_to_archive_seconds'] = 1
            elif mutation == 'bad_control':
                bad['control']['full']['observations'][1]['observed_seconds'] = 0.5
            else:
                bad['control']['unexpected'] = 'not published'
            with self.subTest(mutation=mutation), self.assertRaises((ValueError, speed.capacity.Invalid)):
                self.prepare(bad)

    def test_forecast_counts_three_gates_six_engine_passes_and_control_headroom(self):
        data = self.prepare()
        source = self.binding['inputs'][0]
        row = {'class': source['class'], 'channels': source['channels'], 'packets': source['packets'],
               'raw_frames': self.receipt['inputs'][0]['operations']['raw_identity']['result']['frames']}
        plan = {'_allowance': data, 'warmups': 1, 'rounds': 5}
        scale = max(max(row['raw_frames'] * row['channels'] / data['cells'][mode, row['class']]['samples'],
                        sum(row['packets'].values()) / data['cells'][mode, row['class']]['packets'])
                    for mode in {label.split('/')[0] for label in row['packets']})
        expected = math.ceil(2 * ((3 + 6 * 8) * (0.01 + scale) + 6))
        self.assertEqual(speed.allowance.forecast(row, plan), expected)
        row['raw_frames'] *= 2
        self.assertGreater(speed.allowance.forecast(row, plan), expected)

    def test_acquisition_and_work_are_both_admission_gates(self):
        data = self.prepare()
        source = self.binding['inputs'][0]
        row = {'status': 'ok', 'class': source['class'], 'channels': source['channels'], 'packets': source['packets'],
               'raw_frames': self.receipt['inputs'][0]['operations']['raw_identity']['result']['frames'], 'file_bytes': 100}
        other = copy.deepcopy(row)
        other['class'] = 'matroska'
        indexed = {('a', row['class']): row, ('a', other['class']): other}
        plan = {'_allowance': data, 'warmups': 1, 'rounds': 5, 'shard_seconds': 1000, 'vm_seconds': 1600}
        expected = {'shards': {'0': ['a']}}
        self.assertLess(speed.allowance.admit(indexed, expected, plan)['0']['work_seconds'], 1000)
        plan['shard_seconds'] = 1
        with self.assertRaises(ValueError):
            speed.allowance.admit(indexed, expected, plan)
        plan['shard_seconds'] = 1000
        row['file_bytes'] = 10**12
        with self.assertRaises(ValueError):
            speed.allowance.admit(indexed, expected, plan)

    def test_mixed_packets_charge_the_slowest_present_mode_and_both_work_counters(self):
        data = self.prepare()
        source = self.binding['inputs'][0]
        row = {'class': source['class'], 'channels': source['channels'],
               'raw_frames': self.receipt['inputs'][0]['operations']['raw_identity']['result']['frames'],
               'packets': {'celt/10': 99, 'silk/10': 1}}
        cell = data['cells']['silk', row['class']]
        cell['seconds'] = {op: 100 for op in speed.capacity.OPERATIONS}
        plan = {'_allowance': data, 'warmups': 1, 'rounds': 5}
        scale = max(row['raw_frames'] * row['channels'] / cell['samples'], 100 / cell['packets'])
        expected = math.ceil(2 * ((3 + 6 * 8) * (0.01 + 100 * scale) + 6))
        self.assertEqual(speed.allowance.forecast(row, plan), expected)
        row['packets']['silk/10'] = 10 * cell['packets']
        self.assertGreater(speed.allowance.forecast(row, plan), expected)


class ShardDeadlineTests(unittest.TestCase):
    def test_slow_non_worker_work_cannot_complete_after_the_shard_deadline(self):
        plan = speed.capacity.load(HERE / 'round4.json')
        plan['shard_seconds'] = 0.02
        source = 'a' * 64
        records = {(source, cls): {'source_sha256': source, 'class': cls, 'status': 'ok',
            'sha256': 'b' * 64, 'channels': 2, 'kbps': 64, 'frame_ms': 10, 'packets': {'celt/10': 100},
            'raw_frames': 48000, 'file_bytes': 100} for cls in speed.CLASSES}
        expected = {'shards': {'0': [source]}}
        def slow(execute, record, result, work, plan):
            time.sleep(0.06)
            result['status'] = 'timed'
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            roster = root / 'roster.json'
            roster.write_text('{}')
            args = types.SimpleNamespace(plan=HERE / 'round4.json', roster=roster, shard='0', work=root, media=root)
            store = types.SimpleNamespace(write=lambda value: None)
            with patch.object(speed.allowance, 'admit', return_value={'0': {}}), \
                    patch.object(speed, 'host_check', return_value={}), patch.object(speed, 'runtime_matches'), \
                    patch.object(speed.capacity_run, 'Executor'), patch.object(speed, 'one', side_effect=slow), \
                    patch.object(speed, 'record_valid', return_value=True):
                result = speed.run(args, plan, {'limits': {}}, expected, records, {}, store)
        self.assertFalse(result['complete'])
        self.assertTrue(any(error['error']['outcome'] == 'job_deadline' for error in result['errors']))


if __name__ == '__main__':
    unittest.main()
