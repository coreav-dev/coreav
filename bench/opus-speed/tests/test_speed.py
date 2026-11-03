import copy
import importlib.util
import json
import pathlib
import random
import statistics
import sys
import tempfile
import types
import unittest
import wave
import os
import boto3
from botocore.stub import Stubber
from unittest.mock import patch

HERE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
import speed

spec = importlib.util.spec_from_file_location('opus_speed_report', HERE / 'report.py')
reporter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reporter)

SECRET = 'private-output-marker'


def fixture():
    plan, binding = speed.load_plan(HERE / 'round4.json')
    plan = copy.deepcopy(plan)
    plan['draw'].update(minimum_sources=4, target_sources=4)
    plan['minimums'] = {'celt': 4, 'celt_per_class': 2, 'silk': 2, 'hybrid': 2}
    sources = [f'{index:064x}' for index in range(1, 5)]
    expected = {'draw': 10, 'sources': sources, 'shards': {str(i): [s] for i, s in enumerate(sources)},
                'inputs_sha256': 'a' * 64, 'exclusions_sha256': plan['exclusions_sha256']}
    inputs = []
    for index, source in enumerate(sources):
        mode = ('celt', 'celt', 'silk', 'hybrid')[index]
        for cls in speed.CLASSES:
            inputs.append({'source_sha256': source, 'class': cls, 'set': 'derived', 'status': 'ok',
                'sha256': f'{100 + len(inputs):064x}', 'channels': 2, 'kbps': 64, 'frame_ms': 10,
                'packets': {mode + '/10': 100}, 'file': source + ('.mp4' if cls == 'mp4' else '.webm')})
    return plan, binding, expected, {'inputs': inputs}


def gate(name):
    if name == 'raw_identity':
        return {'passed': True, 'packets': 100, 'frames': 48000, 'channels': 2}
    if name == 'cli_identity':
        return {'passed': True, 'frames': 48000, 'channels': 2, 'sample_rate': 48000,
                'sample_width': 2, 'output_bytes': 192044}
    return {'schema': 'opus-oracle/1', 'status': 'ok', 'passed': True, 'error': None,
            'detail': {'samples': {'ours': 48000, 'ref': 48000, 'default': 48000},
                'length_ok': True, 'default_length_ok': True, 'compare_passed': True, 'quality': 100,
                'applied': False, 'model': 48000, 'ffmpeg_expected': 48000, 'full': 48000, 'entries_count': 0}}


def call(name, ns=100):
    value = {'ok': True, 'ns': ns}
    if name.endswith('_cli'):
        return value | {k: v for k, v in gate('cli_identity').items() if k != 'passed'}
    value['samples'] = 48000
    if name.endswith('_lib'):
        value['bytes'] = 192000
    if name in ('head_lib', 'base_lib'):
        value['warnings'] = 0
    if name.startswith('lav_'):
        value['errors'] = 0
    if name == 'lav_dec':
        value['packets'] = 100
    return value


def completed(record, plan):
    result = speed.blank(record)
    result.update(status='timed', stream=0, gates={name: {'status': 'complete', 'result': gate(name), 'error': None}
                                                 for name in speed.GATES})
    result['warmup'] = {name: call(name) for name in speed.ENGINES}
    result['rounds'] = [{'index': i, 'order': list(speed.ENGINES[i:] + speed.ENGINES[:i]),
                        'calls': {name: call(name, 90 if name.startswith('head_') else 100)
                                  for name in speed.ENGINES}} for i in range(plan['rounds'])]
    medians(result)
    return result


def medians(record):
    record['ns'] = {name: statistics.median(row['calls'][name]['ns'] for row in record['rounds'])
                    for name in speed.ENGINES}


def reports(plan, binding, expected, document):
    host = {'cpu': plan['host']['cpu'], 'smt': False, 'outputs_tmpfs': True,
        'memory_as_bytes': binding['limits']['memory_bytes'], 'tmpfs_budget_bytes': binding['limits']['tmpfs_bytes'],
        'tmpfs_capacity_bytes': binding['limits']['tmpfs_bytes'], 'machine': 'c4-standard-4', 'provisioning': 'STANDARD',
        'affinity_cpu': plan['host']['affinity_cpu']}
    return [{'schema': 'opus-speed/1', 'round': 4, 'plan_sha256': 'b' * 64,
        'provenance_commit': plan['provenance_commit'], 'binding_sha256': speed.capacity.canonical(binding),
        'roster_sha256': 'c' * 64, 'shard': shard, 'host': host, 'errors': [], 'complete': True,
        'results': [completed(row, plan) for row in document['inputs'] if row['source_sha256'] in assigned]}
        for shard, assigned in expected['shards'].items()]


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.plan, self.binding, self.expected, self.document = fixture()
        self.record = completed(self.document['inputs'][0], self.plan)

    def test_registration_retains_the_exact_qualified_capacity(self):
        plan, binding = speed.load_plan(HERE / 'round4.json')
        self.assertEqual(plan['call_limits']['seconds'], 900)
        self.assertEqual(binding['limits']['memory_bytes'], 8 << 30)
        changed = copy.deepcopy(plan)
        changed['capacity']['receipt_sha256'] = '0' * 64
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'plan.json'
            path.write_text(json.dumps(changed))
            with self.assertRaises(speed.Failure):
                speed.load_plan(path)

    def test_roster_and_exclusions_cover_both_containers_even_for_failed_derivation(self):
        excluded = {'schema': 'opus-speed-exclusions/1', 'through_draw': 9, 'hashes': []}
        self.document['inputs'][0]['status'] = 'encode_failed'
        indexed = speed.input_records(self.document, self.expected, excluded, self.plan)
        self.assertEqual(len(indexed), 8)
        self.assertTrue(speed.record_valid(speed.blank(self.document['inputs'][0]), self.plan))
        attacks = [self.document['inputs'][:-1], self.document['inputs'] + [self.document['inputs'][0]]]
        for inputs in attacks:
            with self.subTest(inputs=len(inputs)), self.assertRaises(speed.Failure):
                speed.input_records({'inputs': inputs}, self.expected, excluded, self.plan)
        for digest in (self.expected['sources'][0], self.document['inputs'][1]['sha256']):
            with self.subTest(digest=digest), self.assertRaises(speed.Failure):
                speed.input_records(self.document, self.expected, excluded | {'hashes': [digest]}, self.plan)
        bad = copy.deepcopy(self.expected)
        bad['shards']['1'] = bad['shards']['0']
        with self.assertRaises(speed.Failure):
            speed.roster(bad, self.plan)

    def test_every_engine_requires_complete_typed_work_and_all_repetitions(self):
        self.assertTrue(speed.record_valid(self.record, self.plan))
        for name in speed.ENGINES:
            for phase in ('warmup', 'measured'):
                for field, value in (('ok', False), ('ns', True), ('ns', 0), ('ns', float('nan')),
                                     ('samples', 1), ('unexpected', SECRET)):
                    bad = copy.deepcopy(self.record)
                    calls = bad['warmup'] if phase == 'warmup' else bad['rounds'][0]['calls']
                    calls[name][field] = value
                    with self.subTest(name=name, phase=phase, field=field, value=value):
                        self.assertFalse(speed.record_valid(bad, self.plan))
                bad = copy.deepcopy(self.record)
                calls = bad['warmup'] if phase == 'warmup' else bad['rounds'][0]['calls']
                del calls[name]
                self.assertFalse(speed.record_valid(bad, self.plan))
        for change in ('round', 'order', 'median', 'gate', 'warnings', 'stream', 'bytes', 'raw_packets'):
            bad = copy.deepcopy(self.record)
            if change == 'round':
                bad['rounds'].pop()
            elif change == 'order':
                bad['rounds'][1]['order'].reverse()
            elif change == 'median':
                bad['ns']['head_dec'] = 1
            elif change == 'gate':
                bad['gates']['oracle']['status'] = 'not_run'
            elif change == 'warnings':
                bad['warmup']['head_lib']['warnings'] = -1
            elif change == 'stream':
                bad['stream'] = True
            elif change == 'bytes':
                bad['gates']['cli_identity']['result']['output_bytes'] = 1
            else:
                bad['gates']['raw_identity']['result']['packets'] = 1
            with self.subTest(change=change):
                self.assertFalse(speed.record_valid(bad, self.plan))

    def test_default_disagreement_does_not_change_the_frozen_primary_predicate(self):
        detail = self.record['gates']['oracle']['result']['detail']
        detail['samples']['default'] -= 1
        detail['default_length_ok'] = False
        self.assertTrue(speed.record_valid(self.record, self.plan))
        detail['default_length_ok'] = True
        self.assertFalse(speed.record_valid(self.record, self.plan))
        detail['default_length_ok'] = False
        self.record.update(status='oracle_disagreement', warmup={}, rounds=[], ns={})
        self.record['gates']['oracle']['result']['passed'] = False
        detail.update(compare_passed=False, quality=None)
        self.assertTrue(speed.record_valid(self.record, self.plan))
        del detail['model']
        self.assertFalse(speed.record_valid(self.record, self.plan))

    def test_public_projection_cannot_copy_unknown_labels_or_diagnostics(self):
        self.record.update(status=SECRET, failure={'stage': 'measured', 'reason': SECRET,
            'error': speed.receipts.receipt('capacity.stage', 'harness', 'protocol_error')}, private=SECRET)
        self.record['gates']['raw_identity']['status'] = SECRET
        projected = speed.public_untimed(self.record)
        self.assertNotIn(SECRET, json.dumps(projected))
        self.assertEqual(speed.gate_status(self.record, 'raw_identity'), 'invalid')
        self.record['failure']['error']['operation'] = [SECRET]
        self.assertIsNone(speed.public_untimed(self.record)['failure'])


class FakeExecutor:
    def __init__(self, media):
        self.media = media
        self.tools = {name: name for name in speed.capacity.TOOLS}
        self.calls = []

    def selected_stream(self, record):
        return 0

    def __call__(self, name, record):
        self.calls.append(name)
        return gate(name)

    def invoke(self, operation, role, command):
        self.calls.append(operation)
        if role in ('head_coreav', 'ffmpeg'):
            with wave.open(str(command[-1]), 'wb') as output:
                output.setnchannels(2)
                output.setsampwidth(2)
                output.setframerate(48000)
                output.writeframes(b'\0' * 192000)
            return types.SimpleNamespace(stdout=b'')
        name = ('lav_' + ('lib' if command[1] == 'decode' else 'dec') if role == 'lav'
                else role.split('_')[0] + ('_lib' if command[1] == 'lib' else '_dec'))
        return types.SimpleNamespace(stdout=json.dumps(call(name)).encode())


class ExecutionTests(unittest.TestCase):
    def setUp(self):
        self.plan, self.binding, self.expected, document = fixture()
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.record = document['inputs'][0]
        source = self.root / self.record['file']
        source.write_bytes(b'synthetic fixture, not encoded media')
        self.record['sha256'] = speed.capacity.digest(source)
        self.execute = FakeExecutor(self.root)

    def one(self, execute=None):
        result = speed.blank(self.record)
        with patch.object(speed.time, 'perf_counter_ns', side_effect=range(100, 10000, 100)):
            speed.one(execute or self.execute, self.record, result, self.root, self.plan)
        return result

    def test_real_adapter_runs_gates_one_warmup_and_five_rotated_rounds(self):
        result = self.one()
        self.assertEqual(result['status'], 'timed')
        self.assertTrue(speed.record_valid(result, self.plan))
        self.assertEqual(self.execute.calls[:3], list(speed.GATES))
        self.assertEqual(len(self.execute.calls), 3 + 6 * len(speed.ENGINES))
        self.assertEqual([row['order'] for row in result['rounds']],
                         [list(speed.ENGINES[i:] + speed.ENGINES[:i]) for i in range(5)])
        self.assertFalse(list(self.root.glob('*.wav')))

    def test_all_call_failures_are_terminal_and_keep_closed_errors(self):
        for index in range(3 + 6 * len(speed.ENGINES)):
            executor = FakeExecutor(self.root)
            count = 0
            original_gate, original_invoke = executor.__call__, executor.invoke

            def fail(function, *args):
                nonlocal count
                count += 1
                if count == index + 1:
                    raise speed.receipts.CommandFailure(speed.receipts.receipt(
                        'capacity.stage', 'harness', 'timeout'))
                return function(*args)

            executor.invoke = lambda *args: fail(original_invoke, *args)
            class Gates:
                media, tools = executor.media, executor.tools
                selected_stream = executor.selected_stream
                invoke = staticmethod(executor.invoke)

                def __call__(self, *args):
                    return fail(original_gate, *args)

            result = self.one(Gates())
            with self.subTest(index=index):
                self.assertEqual(count, index + 1)
                self.assertEqual(result['status'], 'failed')
                self.assertEqual(result['failure']['error']['outcome'], 'timeout')
                self.assertEqual(result['ns'], {})
                self.assertFalse(speed.record_valid(result, self.plan))

    def test_valid_disagreement_never_starts_timing_but_oracle_errors_fail(self):
        executor = self.execute
        class Gates:
            media, tools = executor.media, executor.tools
            selected_stream = executor.selected_stream

            def __call__(self, name, record):
                value = executor(name, record)
                if name == 'oracle':
                    value['passed'] = False
                    value['detail'].update(compare_passed=False, quality=None)
                return value
        result = self.one(Gates())
        self.assertEqual(result['status'], 'oracle_disagreement')
        self.assertTrue(speed.record_valid(result, self.plan))
        self.assertEqual(executor.calls, list(speed.GATES))
        class Broken(Gates):
            def __call__(self, name, record):
                if name == 'oracle':
                    return speed.oracle.failure(speed.receipts.receipt('oracle.compare', 'opus_compare', 'timeout'))
                return super().__call__(name, record)
        result = self.one(Broken())
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['failure']['error']['operation'], 'oracle.compare')

    def test_store_claim_is_create_once_and_checkpoints_round_trip(self):
        class Client:
            objects = {}
            def put_object(self, **args):
                if args.get('IfNoneMatch') == '*' and args['Key'] in self.objects:
                    raise FileExistsError
                self.objects[args['Key']] = bytes(args['Body'])
        client = Client()
        store = speed.Store(self.root / 'first', self.plan, '0', {'registration': 'fixed'}, client)
        value = {'complete': False, 'results': [speed.blank(self.record)]}
        store.write(value)
        self.assertEqual(speed.capacity.load(self.root / 'first/results.json'), value)
        self.assertEqual(json.loads(client.objects[store.prefix + 'results.json']), value)
        with self.assertRaises(FileExistsError):
            speed.Store(self.root / 'second', self.plan, '0', {'registration': 'changed'}, client)

    def test_pinned_sdk_supports_the_conditional_start_claim(self):
        client = boto3.client('s3', region_name='auto', endpoint_url='https://checkpoint.invalid',
                              aws_access_key_id='synthetic', aws_secret_access_key='synthetic')
        with Stubber(client) as stub, patch.dict(os.environ, {'R2_BUCKET': 'synthetic'}):
            stub.add_response('put_object', {}, {'Bucket': 'synthetic',
                'Key': 'measurements/opus-speed/round-4/shard-0/STARTED.json',
                'Body': b'{"registered": true}', 'IfNoneMatch': '*'})
            speed.Store(self.root / 'sdk', self.plan, '0', {'registered': True}, client)
            stub.assert_no_pending_responses()

    def test_shard_seeds_all_slots_and_stops_at_the_first_failure(self):
        indexed = {speed.key(self.record): self.record}
        second = copy.deepcopy(self.record)
        second.update({'class': 'matroska', 'file': self.record['file'].replace('.mp4', '.webm')})
        indexed[speed.key(second)] = second
        args = types.SimpleNamespace(plan=HERE / 'round4.json', roster=self.root / 'roster.json',
                                     shard='0', work=self.root, media=self.root)
        args.roster.write_text(json.dumps(self.expected))
        writes = []
        store = types.SimpleNamespace(write=lambda value: writes.append(copy.deepcopy(value)))
        self.execute.invoke = lambda *args: (_ for _ in ()).throw(OSError(SECRET))
        with patch.object(speed, 'host_check', return_value={}), patch.object(speed, 'runtime_matches'), \
                patch.object(speed.capacity_run, 'Executor', return_value=self.execute) as factory:
            result = speed.run(args, self.plan, self.binding, self.expected, indexed, {}, store)
        self.assertEqual([row['status'] for row in writes[0]['results']], ['pending', 'pending'])
        self.assertEqual([row['status'] for row in result['results']], ['failed', 'pending'])
        self.assertFalse(result['complete'])
        self.assertEqual(result['results'][0]['failure']['error']['outcome'], 'io_error')
        self.assertNotIn(SECRET, json.dumps(result))
        self.assertGreater(factory.call_args.args[-1] - speed.time.monotonic(), 16000)

    def test_host_refuses_spot_or_an_unregistered_machine(self):
        for preemptible, machine in (('true', 'c4-standard-4'), ('false', 'c4-standard-8')):
            with patch.object(speed.capacity_run, 'host_setup', return_value={}), \
                    patch.object(speed, 'metadata', side_effect=[machine, preemptible]), \
                    self.assertRaises(speed.Failure):
                speed.host_check(self.root, self.binding, self.plan)
        with patch.object(speed.capacity_run, 'host_setup', return_value={}), \
                patch.object(speed, 'metadata', side_effect=['c4-standard-4', 'false']), \
                patch.object(speed.os, 'sched_setaffinity') as pin, \
                patch.object(speed.os, 'sched_getaffinity', return_value={1}):
            host = speed.host_check(self.root, self.binding, self.plan)
        pin.assert_called_once_with(0, {1})
        self.assertEqual(host['affinity_cpu'], 1)

    def test_changed_source_and_malformed_native_output_never_get_a_median(self):
        with patch.object(self.execute, 'invoke', return_value=types.SimpleNamespace(stdout=b'{')):
            result = self.one()
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['failure']['error']['outcome'], 'protocol_error')
        self.assertEqual(result['ns'], {})
        (self.root / self.record['file']).write_bytes(b'changed synthetic fixture')
        result = self.one()
        self.assertEqual(result['failure']['reason'], 'changed_input_bytes')
        self.assertEqual(self.execute.calls, list(speed.GATES))


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.plan, self.binding, self.expected, self.document = fixture()
        self.reports = reports(self.plan, self.binding, self.expected, self.document)

    def summarize(self, reports=None):
        indexed = {speed.key(row): row for row in self.document['inputs']}
        return reporter.summarize(self.plan, self.binding, self.expected, indexed,
                                  self.reports if reports is None else reports, 'b' * 64, 'c' * 64)

    def test_complete_reports_recompute_bootstrap_shares_and_the_exact_bars(self):
        celt = sorted((row for shard in self.reports for row in shard['results'] if row['mode'] == 'celt'),
                      key=speed.key)
        for row, ns in zip(celt, (80, 90, 95, 99)):
            for round_ in row['rounds']:
                round_['calls']['head_dec']['ns'] = ns
            medians(row)
        result = self.summarize()
        self.assertTrue(result['passed'], result['errors'])
        self.assertEqual(result['timed'], 8)
        first = result['stats'][0]
        rng = random.Random(206)
        values = [0.8, 0.9, 0.95, 0.99]
        independent = sorted(statistics.median(rng.choices(values, k=4)) for _ in range(10000))
        self.assertEqual(first['ci95'], [independent[249], independent[9749]])
        self.assertEqual(first['share_faster'], 1)
        for shard in self.reports:
            for row in shard['results']:
                for round_ in row['rounds']:
                    round_['calls']['head_dec']['ns'] = 100 if row['mode'] == 'celt' else 102
                    round_['calls']['head_lib']['ns'] = 100
                medians(row)
        result = self.summarize()
        self.assertFalse(result['passed'])
        misses = [error for error in result['errors'] if error['reason'] == 'bar_missed']
        self.assertEqual({error['pair'] for error in misses}, {'decoder_vs_lav'})
        self.assertEqual(len(misses), 3)

    def test_missing_duplicate_partial_and_wrong_binding_never_produce_statistics(self):
        missing = self.summarize(self.reports[:-1])
        self.assertEqual(len(missing['untimed']), 2)
        self.assertEqual({row['status'] for row in missing['untimed']}, {'pending'})
        attacks = [self.reports[:-1], self.reports + [self.reports[0]]]
        for field, value in (('complete', False), ('host', {}), ('plan_sha256', 'd' * 64),
                             ('binding_sha256', 'd' * 64), ('errors', [{'message': SECRET}])):
            bad = copy.deepcopy(self.reports)
            bad[0][field] = value
            attacks.append(bad)
        bad = copy.deepcopy(self.reports)
        bad[0]['results'].pop()
        attacks.append(bad)
        bad = copy.deepcopy(self.reports)
        bad[0]['results'].append(bad[0]['results'][0])
        attacks.append(bad)
        bad = copy.deepcopy(self.reports)
        bad[0]['results'][0]['rounds'].pop()
        attacks.append(bad)
        for field, value in (('sha256', 'd' * 64), ('kbps', 128), ('channels', 1)):
            bad = copy.deepcopy(self.reports)
            bad[0]['results'][0][field] = value
            attacks.append(bad)
        for index, attack in enumerate(attacks):
            result = self.summarize(attack)
            with self.subTest(index=index):
                self.assertFalse(result['passed'])
                self.assertEqual(result['timed'], 0)
                self.assertEqual(result['stats'], [])
                self.assertNotIn(SECRET, json.dumps(result))

    def test_all_untimed_outcomes_remain_visible_and_minima_fail_without_inventing_timings(self):
        for shard in self.reports:
            for index, row in enumerate(shard['results']):
                original = self.document['inputs'][int(shard['shard']) * 2 + index]
                original['status'] = 'encode_failed'
                shard['results'][index] = speed.blank(original)
        result = self.summarize()
        self.assertFalse(result['passed'])
        self.assertEqual(len(result['untimed']), 8)
        self.assertEqual(result['gates']['oracle'], {'not_run': 8})
        self.assertEqual(result['stats'], [])
        self.assertTrue(any(error['reason'] == 'minimum_not_met' for error in result['errors']))


if __name__ == '__main__':
    unittest.main()
