"""Media-free checks of round-10 selection, capture, execution and reporting."""
import collections
import copy
import hashlib
import json
import math
import os
import pathlib
import signal
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import b4_round10 as b4


def sha(label):
    return hashlib.sha256(label.encode()).hexdigest()


def rosters():
    return {phase: [{'class': family, 'file': f'{phase}:{family}:{rank}', 'rank': rank,
                     'sha256': sha(f'{phase}:{family}:{rank}'), 'size': 1,
                     'ext': {'isobmff': 'mp4', 'matroska': 'mkv', 'mpegts': 'ts'}[family],
                     'key': f'objects/{phase}/{family}/{rank}'}
                    for family in b4.protocol.FAMILIES
                    for rank in range(8 if phase == 'calibration' else b4.PRODUCT_COUNTS[family])]
            for phase in ('calibration', 'product')}


def clean_log():
    return {'lines': 0, 'refused': 0, 'executed': 0, 'invalid': 0, 'void': False}


def child(value=1):
    return {name[3:]: value for name in b4.COUNTERS}


def unavailable():
    return {'status': 'unavailable', 'reason': 'not exposed'}


def sample(value=1, steal=10):
    return {'child': child(value),
            'steal_ticks': {'status': 'available', 'value': steal},
            'frequency_khz': {name: unavailable() for name in ('scaling_cur_freq', 'cpuinfo_cur_freq')}}


def successful_call(elapsed=1, code=0):
    return {'executed': True, 'start': 0, 'end': elapsed, 'elapsed': elapsed, 'exit': code,
            'telemetry': {'child': child(), 'steal_ticks': unavailable(),
                          'frequency_khz': {'before': unavailable(), 'after': unavailable()}}}


def calls_for(schedule):
    return [{**pair, 'side': side, 'position': position, **successful_call()}
            for pair in schedule for position, side in enumerate(pair['order'])]


def report_data(mode='calibration'):
    return {'method': b4.protocol.METHOD, 'mode': mode, 'aborted': None,
            'fallback_log': clean_log(), 'commands': b4.speed.native_commands(),
            'command_sha256': b4.digest(b4.speed.COMMANDS),
            'selection': {phase: [{key: row[key] for key in ('class', 'file', 'rank')}
                                  for row in entries] for phase, entries in rosters().items()},
            'receipts': calls_for(b4.protocol.calibration_schedule())}


class MediaFree(unittest.TestCase):
    def setUp(self):
        for target in ('subprocess.Popen', 'subprocess.run', 'subprocess.check_output',
                       'round_files.pool', 'round_files.fetch_one', 'speed.timed'):
            patcher = mock.patch('b4_round10.' + target,
                                 side_effect=AssertionError('unexpected child or payload access'))
            patcher.start()
            self.addCleanup(patcher.stop)


class MetadataSelection(MediaFree):
    def setUp(self):
        super().setUp()
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.metadata = {name: [] for name in b4.MANIFESTS}
        for family, (corpus, extensions) in b4.speed.CLASSES.items():
            ext = sorted(extensions)[0]
            for holdout, count in ((False, 9), (True, b4.PRODUCT_COUNTS[family])):
                for index in range(count):
                    self.metadata[corpus].append({'set': 'real', 'holdout': holdout,
                                                  'sha256': sha(f'{family}:{holdout}:{index}'),
                                                  'size': 1, 'ext': ext,
                                                  'key': f'{family}-{holdout}-{index}'})

    def write(self):
        for name, rows in self.metadata.items():
            path = self.root / 'corpus' / name / 'manifest.json'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps({'files': rows}))

    def select(self, extra=()):
        self.write()
        return b4.select_rosters(self.root, extra)[0]

    def dev(self):
        return [row for row in self.metadata['v1'] if row['holdout'] is False and row['ext'] == '3gp']

    def test_explicit_false_is_development_and_salted_selection_is_exact(self):
        actual = self.select()
        self.assertEqual(collections.Counter(row['class'] for row in actual['calibration']),
                         {'isobmff': 8, 'matroska': 8, 'mpegts': 8})
        expected = sorted(self.dev(), key=lambda row: sha('coreav-b4-calibration-v2:' + row['sha256']))[:8]
        observed = [row for row in actual['calibration'] if row['class'] == 'isobmff']
        self.assertEqual([row['sha256'] for row in observed], [row['sha256'] for row in expected])
        self.assertEqual([row['rank'] for row in observed], list(range(8)))
        self.assertEqual([row['key'] for row in observed], ['corpus/v1/' + row['key'] for row in expected])
        self.assertEqual(len(actual['product']), 57)

    def test_absent_zero_and_null_are_not_explicit_development(self):
        for value in ('absent', 0, None, ''):
            with self.subTest(value=value):
                original = copy.deepcopy(self.metadata)
                for row in self.dev()[:2]:
                    if value == 'absent':
                        del row['holdout']
                    else:
                        row['holdout'] = value
                with self.assertRaises(ValueError):
                    self.select()
                self.metadata = original

    def test_size_limit_is_inclusive_but_wrong_sizes_extensions_and_sets_are_excluded(self):
        self.dev()[0]['size'] = b4.speed.LIMIT
        actual = self.select()
        self.assertIn(self.dev()[0]['sha256'], [row['sha256'] for row in actual['calibration']])
        for key, value in (('size', 0), ('size', -1), ('size', b4.speed.LIMIT + 1),
                           ('size', True), ('size', 1.0), ('size', '1'),
                           ('ext', 'MP4'), ('ext', 'avi'), ('set', 'synthetic')):
            with self.subTest(key=key, value=value):
                original = copy.deepcopy(self.metadata)
                for row in self.dev()[:2]:
                    row[key] = value
                with self.assertRaises(ValueError):
                    self.select()
                self.metadata = original

    def test_heldout_digest_in_another_manifest_or_extra_roster_blocks_development(self):
        selected = self.select()['calibration'][0]['sha256']
        self.metadata['v2-heldout'].append({'sha256': selected, 'holdout': True})
        with self.assertRaises(ValueError):
            self.select()
        self.metadata['v2-heldout'] = []
        extra = self.root / 'extra.json'
        extra.write_text(json.dumps([{'sha256': selected}]))
        with self.assertRaises(ValueError):
            self.select([extra])

    def test_duplicate_development_and_product_digests_are_rejected(self):
        for phase in ('calibration', 'product'):
            with self.subTest(phase=phase):
                original = copy.deepcopy(self.metadata)
                chosen = self.select()[phase]
                a, b = chosen[0]['sha256'], chosen[1]['sha256']
                for rows in self.metadata.values():
                    for row in rows:
                        if row['sha256'] == b:
                            row['sha256'] = a
                with self.assertRaises(ValueError):
                    self.select()
                self.metadata = original

    def test_cross_container_duplicate_development_is_rejected(self):
        iso = self.dev()[0]['sha256']
        for row in self.metadata['v1']:
            if row['holdout'] is False and row['ext'] in {'mkv', 'webm'}:
                row['sha256'] = iso
        with self.assertRaises(ValueError):
            self.select()

    def test_every_registered_manifest_is_required_before_selection(self):
        self.write()
        (self.root / 'corpus/v2-heldout/manifest.json').unlink()
        with self.assertRaises(ValueError):
            b4.select_rosters(self.root)

    def test_transport_stream_product_roster_cannot_grow_or_shrink(self):
        for change in ('grow', 'shrink'):
            with self.subTest(change=change):
                original = copy.deepcopy(self.metadata)
                rows = self.metadata['v1-ts-audio']
                held = next(row for row in rows if row['holdout'] is True)
                if change == 'grow':
                    rows.append({**held, 'sha256': sha('additional transport stream')})
                else:
                    rows.remove(held)
                with self.assertRaises(ValueError):
                    self.select()
                self.metadata = original


class FrozenManifest(MediaFree):
    def setUp(self):
        super().setUp()
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = pathlib.Path(temporary.name) / 'freeze.json'
        selected = rosters()
        commands = b4.speed.native_commands()
        self.manifest = {'method': b4.protocol.METHOD, 'selection': selected, 'commands': commands,
                         'command_sha256': sha('tool'),
                         'tools': {name: sha('tool') for name in b4.TOOL_FILES},
                         'schedule': b4.protocol.calibration_schedule()
                         + b4.protocol.product_schedule(selected['product'], commands)}
        patcher = mock.patch.object(b4, 'digest', return_value=sha('tool'))
        patcher.start()
        self.addCleanup(patcher.stop)

    def load(self, manifest):
        self.path.write_text(json.dumps(manifest))
        return b4.load_manifest(self.path)

    def test_unchanged_frozen_manifest_loads_without_payload_access(self):
        self.assertEqual(self.load(self.manifest), self.manifest)

    def test_missing_duplicate_reordered_and_altered_schedule_are_rejected(self):
        for corruption in ('missing', 'duplicate', 'order', 'timed', 'block', 'side'):
            with self.subTest(corruption=corruption):
                data = copy.deepcopy(self.manifest)
                schedule = data['schedule']
                if corruption == 'missing':
                    schedule.pop()
                elif corruption == 'duplicate':
                    schedule.append(copy.deepcopy(schedule[0]))
                elif corruption == 'order':
                    schedule[0], schedule[1] = schedule[1], schedule[0]
                else:
                    schedule[0][corruption] = {'timed': True, 'block': 1, 'side': 'A'}[corruption]
                with self.assertRaises(ValueError):
                    self.load(data)

    def test_frozen_rosters_require_counts_ranks_aliases_extensions_sizes_and_disjointness(self):
        for phase in ('calibration', 'product'):
            for field, value in (('rank', 5), ('file', 'other'), ('ext', 'avi'), ('size', True),
                                 ('size', 0), ('sha256', 'invalid'), ('missing', None), ('duplicate', None)):
                with self.subTest(phase=phase, field=field):
                    data = copy.deepcopy(self.manifest)
                    rows = data['selection'][phase]
                    if field == 'missing':
                        rows.pop()
                    elif field == 'duplicate':
                        rows[0]['sha256'] = data['selection']['calibration'][1]['sha256']
                    else:
                        rows[0][field] = value
                    with self.assertRaises(ValueError):
                        self.load(data)

    def test_old_schema_command_changes_and_tool_changes_are_rejected(self):
        for field in ('method', 'commands', 'command_sha256', 'tools'):
            with self.subTest(field=field):
                data = copy.deepcopy(self.manifest)
                data[field] = {'method': 'b4-round9-v1', 'commands': [],
                               'command_sha256': sha('changed'), 'tools': {}}[field]
                with self.assertRaises(ValueError):
                    self.load(data)


class Observation(MediaFree):
    def test_snapshots_and_telemetry_surround_the_timer_without_another_child(self):
        events = []
        before, after = sample(2), sample(5, 13)

        def snapshot(cpu):
            events.append(('snapshot', cpu))
            return before if len(events) == 1 else after

        def timed(argv, env, **kwargs):
            events.append(('timed', argv, env, kwargs))
            return 3.0, 0

        def telemetry(first, last):
            events.append(('telemetry', first, last))
            return {'child': child(3)}

        with mock.patch.object(b4, 'snapshot', side_effect=snapshot), \
                mock.patch.object(b4.speed, 'timed', side_effect=timed), \
                mock.patch.object(b4, 'telemetry', side_effect=telemetry), \
                mock.patch.object(b4.time, 'perf_counter', side_effect=[100.0, 103.0]):
            actual = b4.observe(['program', 'arg'], {'ENV': 'fixed'}, 2, 90)
        self.assertEqual(events, [('snapshot', 2),
                                 ('timed', ['program', 'arg'], {'ENV': 'fixed'}, {'raise_oserror': True}),
                                 ('snapshot', 2), ('telemetry', before, after)])
        self.assertEqual((actual['start'], actual['end'], actual['elapsed']), (10, 13, 3))
        self.assertTrue(actual['executed'])

    def test_mandatory_capture_failure_before_launch_does_not_execute(self):
        for error in (OSError('capture'), ValueError('capture'), AttributeError('capture')):
            with self.subTest(error=type(error).__name__), \
                    mock.patch.object(b4, 'snapshot', side_effect=error), \
                    mock.patch.object(b4.speed, 'timed') as timed:
                actual = b4.observe(['program'], {}, 2, 0)
                timed.assert_not_called()
                self.assertFalse(actual['executed'])
                self.assertIn('error', actual)
                self.assertIsNone(actual['exit'])

    def test_mandatory_capture_failure_after_launch_keeps_exit_and_fails_call(self):
        with mock.patch.object(b4, 'snapshot', side_effect=[sample(), ValueError('capture')]), \
                mock.patch.object(b4.speed, 'timed', return_value=(2.0, 0)):
            actual = b4.observe(['program'], {}, 2, 0)
        self.assertTrue(actual['executed'])
        self.assertEqual(actual['exit'], 0)
        self.assertIn('error', actual)
        self.assertFalse(b4.valid_call(actual))

    def test_launch_timeout_and_invalid_elapsed_cannot_be_valid_calls(self):
        for outcome in (OSError('launch'), TimeoutError('deadline'), (1, None),
                        (0, 0), (-1, 0), (math.nan, 0), (math.inf, 0)):
            with self.subTest(outcome=outcome), \
                    mock.patch.object(b4, 'snapshot', side_effect=[sample(1), sample(2)]), \
                    mock.patch.object(b4.speed, 'timed') as timed:
                if isinstance(outcome, Exception):
                    timed.side_effect = outcome
                else:
                    timed.return_value = outcome
                self.assertFalse(b4.valid_call(b4.observe(['program'], {}, 2, 0)))

    def test_snapshot_requires_every_mandatory_finite_nonnegative_child_counter(self):
        for name in b4.COUNTERS:
            for value in ('missing', -1, math.nan, math.inf):
                with self.subTest(name=name, value=value):
                    usage = dict.fromkeys(b4.COUNTERS, 1)
                    if value == 'missing':
                        del usage[name]
                    else:
                        usage[name] = value
                    with mock.patch.object(b4.resource, 'getrusage', return_value=types.SimpleNamespace(**usage)), \
                            mock.patch.object(pathlib.Path, 'read_text', side_effect=FileNotFoundError):
                        with self.assertRaises((ValueError, AttributeError)):
                            b4.snapshot(2)

    def test_missing_optional_host_counters_are_explicit_and_do_not_fail_child_capture(self):
        with mock.patch.object(b4.resource, 'getrusage', return_value=types.SimpleNamespace(**dict.fromkeys(b4.COUNTERS, 1))), \
                mock.patch.object(pathlib.Path, 'read_text', side_effect=FileNotFoundError):
            captured = b4.snapshot(2)
        self.assertEqual(captured['steal_ticks'], unavailable())
        self.assertTrue(all(value == unavailable() for value in captured['frequency_khz'].values()))
        usage = b4.telemetry(captured, captured)
        self.assertEqual(usage['child'], child(0))
        self.assertEqual(usage['steal_ticks']['status'], 'unavailable')

    def test_malformed_host_counters_are_invalid_rather_than_zero(self):
        usage = types.SimpleNamespace(**dict.fromkeys(b4.COUNTERS, 1))
        for text in ('', 'cpu2 1 2', 'cpu2 1 2 3 4 5 6 7 -1'):
            with self.subTest(text=text), \
                    mock.patch.object(b4.resource, 'getrusage', return_value=usage), \
                    mock.patch.object(pathlib.Path, 'read_text', return_value=text):
                captured = b4.snapshot(2)
            self.assertEqual(captured['steal_ticks']['status'], 'invalid')
            self.assertNotIn('value', captured['steal_ticks'])
            self.assertTrue(all(value['status'] == 'invalid' for value in captured['frequency_khz'].values()))

    def test_child_counter_reset_is_invalid_but_host_reset_is_only_a_diagnostic(self):
        with self.assertRaises(ValueError):
            b4.telemetry(sample(2), sample(1))
        usage = b4.telemetry(sample(1, 20), sample(2, 10))
        self.assertEqual(usage['steal_ticks']['status'], 'invalid')
        self.assertIsNone(usage['steal_ticks']['delta'])
        call = successful_call()
        call['telemetry'] = usage
        self.assertTrue(b4.valid_call(call))

    def test_missing_or_invalid_mandatory_delta_is_not_valid(self):
        for name in child():
            for value in ('missing', -1, math.nan, math.inf, True):
                with self.subTest(name=name, value=value):
                    call = successful_call()
                    if value == 'missing':
                        del call['telemetry']['child'][name]
                    else:
                        call['telemetry']['child'][name] = value
                    self.assertFalse(b4.valid_call(call))

    def test_success_receipt_requires_actual_execution_and_finite_monotonic_bounds(self):
        for field, value in (('executed', False), ('executed', 1), ('start', -1),
                             ('start', math.nan), ('end', 0), ('end', math.inf),
                             ('elapsed', True), ('exit', False)):
            with self.subTest(field=field, value=value):
                call = successful_call()
                call[field] = value
                self.assertFalse(b4.valid_call(call))


class HostGate(MediaFree):
    def test_non_linux_run_is_rejected_before_any_preparation(self):
        with mock.patch.object(b4.platform, 'system', return_value='Darwin'), \
                mock.patch.object(b4, 'load_manifest') as load:
            with self.assertRaises(ValueError):
                b4.run(types.SimpleNamespace())
            load.assert_not_called()

    def test_linux_without_exit_descriptors_is_rejected(self):
        with mock.patch.object(b4.platform, 'system', return_value='Linux'), \
                mock.patch.object(b4, 'os', types.SimpleNamespace()), \
                mock.patch.object(b4, 'load_manifest') as load:
            with self.assertRaises(ValueError):
                b4.run(types.SimpleNamespace())
            load.assert_not_called()

    def test_host_requires_registered_machine_provisioning_and_topology(self):
        registered = {'machine_type': 'c4-standard-8', 'provisioning': 'STANDARD',
                      'advanced_machine_features': {'threadsPerCore': 1}}
        for field, value in (('machine_type', 'other'), ('provisioning', 'SPOT'),
                             ('advanced_machine_features', {'threadsPerCore': 2})):
            with self.subTest(field=field), \
                    mock.patch.object(b4.platform, 'system', return_value='Linux'), \
                    mock.patch.object(b4.os, 'cpu_count', return_value=4), \
                    mock.patch.object(pathlib.Path, 'exists', return_value=False):
                with self.assertRaises(ValueError):
                    b4.host_gate({**registered, field: value}, 2)

    def test_valid_linux_host_pins_exactly_one_physical_core(self):
        instance = {'machine_type': 'c4-standard-8', 'provisioning': 'STANDARD',
                    'advanced_machine_features': {'threadsPerCore': 1}}
        contents = {'active': '0', 'physical_package_id': '0', 'core_id': '2',
                    'thread_siblings_list': '2', 'cpuinfo': 'model name : synthetic CPU'}

        def read(path, *args, **kwargs):
            if path.name in contents:
                return contents[path.name]
            raise FileNotFoundError

        with mock.patch.object(b4.platform, 'system', return_value='Linux'), \
                mock.patch.object(b4.platform, 'platform', return_value='synthetic Linux'), \
                mock.patch.object(b4.os, 'cpu_count', return_value=4), \
                mock.patch.object(b4.os, 'sched_getaffinity', side_effect=[{0, 1, 2, 3}, {2}], create=True), \
                mock.patch.object(b4.os, 'sched_setaffinity', create=True) as pin, \
                mock.patch.object(pathlib.Path, 'exists', return_value=True), \
                mock.patch.object(pathlib.Path, 'read_text', autospec=True, side_effect=read):
            result = b4.host_gate(instance, 2)
        pin.assert_called_once_with(0, {2})
        self.assertEqual(result['cpu_model'], 'synthetic CPU')
        self.assertFalse(result['smt'])
        self.assertTrue(all(value['status'] == 'unavailable' for value in result['clock_policy'].values()))


class PhaseExecution(MediaFree):
    def setUp(self):
        super().setUp()
        self.entry = rosters()['product'][0]
        self.commands = [['-show_format', 'INPUT']]
        self.schedule = b4.protocol.product_schedule([self.entry], self.commands)
        self.manifest = {'selection': {'product': [self.entry]}, 'commands': self.commands,
                         'schedule': self.schedule}
        self.receipts = []

    def execute(self, calls, phase='product', log=None):
        with mock.patch.object(b4, 'observe', side_effect=calls) as observed, \
                mock.patch.object(b4, 'fallback', return_value=log or clean_log()), \
                mock.patch.object(b4.time, 'perf_counter', return_value=1):
            result = b4.execute_phase(phase, self.manifest, pathlib.Path('/synthetic'),
                                      'candidate', 'oracle', {'ENV': 'fixed'}, pathlib.Path('/log'),
                                      2, 0, 100, self.receipts)
        return result, observed

    def test_ba_preserves_logical_side_status_and_actual_invocation(self):
        self.manifest['schedule'] = [{**self.schedule[0], 'order': 'BA'}]
        result, observed = self.execute([successful_call(5, 0), successful_call(2, 4)])
        self.assertEqual(result, {('product', self.entry['file'], 0): 'refused'})
        self.assertEqual([(row['side'], row['position'], row['exit']) for row in self.receipts],
                         [('B', 0, 0), ('A', 1, 4)])
        self.assertEqual([call.args[0][0] for call in observed.call_args_list], ['oracle', 'candidate'])
        self.assertEqual(observed.call_args_list[1].args[0][1], 'ffprobe')

    def test_product_has_four_warmup_pairs_and_five_timed_pairs(self):
        _, observed = self.execute([successful_call()] * 18)
        self.assertEqual(observed.call_count, 18)
        self.assertEqual(sum(not row['timed'] for row in self.receipts), 8)
        self.assertEqual(sum(row['timed'] for row in self.receipts), 10)
        self.assertEqual([row['side'] for row in self.receipts],
                         list('ABBAABBAABBAABBAAB'))

    def test_full_calibration_executes_both_arms_with_identical_argv_and_registered_counts(self):
        self.manifest = {'selection': {'calibration': rosters()['calibration']},
                         'schedule': b4.protocol.calibration_schedule()}
        with mock.patch.object(b4, 'observe', side_effect=lambda *args: successful_call()) as observed, \
                mock.patch.object(b4, 'fallback', return_value=clean_log()), \
                mock.patch.object(b4.time, 'perf_counter', return_value=1):
            result = b4.execute_phase('calibration', self.manifest, pathlib.Path('/synthetic'),
                                      'candidate', 'oracle', {}, pathlib.Path('/log'),
                                      2, 0, 100, self.receipts)
        self.assertEqual(result, {})
        self.assertEqual(observed.call_count, 4992)
        self.assertEqual(collections.Counter(row['arm'] for row in self.receipts), {'aa': 2496, 'null': 2496})
        self.assertEqual(sum(row['timed'] for row in self.receipts), 3072)
        paired_argv = collections.defaultdict(list)
        for receipt, invocation in zip(self.receipts, observed.call_args_list):
            key = tuple(receipt[name] for name in ('arm', 'file', 'command', 'block', 'timed', 'pair'))
            paired_argv[key].append(invocation.args[0])
        self.assertTrue(all(a == b for a, b in paired_argv.values()))
        self.assertEqual({argv[0][0] for key, argv in paired_argv.items() if key[0] == 'aa'}, {'oracle'})
        self.assertEqual({tuple(argv[0][:2]) for key, argv in paired_argv.items() if key[0] == 'null'},
                         {('candidate', 'ffprobe')})

    def test_safety_deadline_stops_before_another_call(self):
        with mock.patch.object(b4, 'observe') as observed, \
                mock.patch.object(b4.time, 'perf_counter', return_value=100):
            with self.assertRaises(b4.Incomplete):
                b4.execute_phase('product', self.manifest, pathlib.Path('/synthetic'),
                                 'candidate', 'oracle', {}, pathlib.Path('/log'), 2, 0, 100, self.receipts)
        observed.assert_not_called()
        self.assertEqual(self.receipts, [])

    def test_only_first_successfully_captured_warmup_refusal_skips_the_case(self):
        _, observed = self.execute([successful_call(code=4), successful_call()])
        self.assertEqual(observed.call_count, 2)
        self.assertEqual(len(self.receipts), 2)

    def test_warmup_failure_timed_failure_and_timeout_keep_both_statuses(self):
        for pair_index, a, b in ((0, 1, 0), (0, 4, 1), (4, 4, 0), (4, 0, None), (4, 0, 1)):
            with self.subTest(pair_index=pair_index, a=a, b=b):
                self.receipts = []
                pair = self.schedule[pair_index]
                calls = [successful_call(code={'A': a, 'B': b}[side]) for side in pair['order']]
                self.manifest['schedule'] = [pair]
                with self.assertRaises(b4.Incomplete):
                    self.execute(calls)
                self.assertEqual({row['side']: row['exit'] for row in self.receipts}, {'A': a, 'B': b})

    def test_capture_failure_is_retained_and_stops_before_the_next_child(self):
        call = {'executed': False, 'error': 'capture unavailable', 'exit': None}
        with self.assertRaises(b4.Incomplete):
            self.execute([call])
        self.assertEqual(len(self.receipts), 1)
        self.assertFalse(self.receipts[0]['executed'])

    def test_executed_or_unreadable_fallback_invalidates_product(self):
        for counts in ({'lines': 1, 'executed': 1}, {'invalid': 1}):
            with self.subTest(counts=counts):
                self.receipts = []
                with self.assertRaises(b4.Incomplete):
                    self.execute([successful_call()] * 2, log={**clean_log(), **counts, 'void': True})
                self.assertEqual(len(self.receipts), 2)

    def test_refusal_log_is_allowed_only_as_product_coverage_not_calibration(self):
        log = {**clean_log(), 'lines': 1, 'refused': 1}
        self.execute([successful_call(code=4), successful_call()], log=log)
        pair = b4.protocol.calibration_schedule()[0]
        entry = rosters()['calibration'][0]
        self.manifest = {'selection': {'calibration': [entry]}, 'schedule': [pair]}
        self.receipts = []
        with self.assertRaises(b4.Incomplete):
            self.execute([successful_call()] * 2, phase='calibration', log=log)

    def test_unreadable_fallback_capture_is_explicitly_void(self):
        with mock.patch.object(b4.speed, 'b4_fallback', side_effect=OSError('unreadable')):
            result = b4.fallback(pathlib.Path('/missing'))
        self.assertTrue(result['void'])
        self.assertEqual(result['invalid'], 1)


class Reporter(MediaFree):
    def reject_or_fail(self, data):
        try:
            result = b4.summary(data)
        except (ValueError, KeyError, TypeError):
            return
        self.assertFalse(result['passes'])

    def test_old_method_is_rejected_without_rescoring(self):
        for method in (None, 'b4-round9-v1', 'b4-counterbalanced-v1'):
            with self.subTest(method=method), mock.patch.object(b4, 'calibration_summary') as summarize:
                with self.assertRaises(ValueError):
                    b4.summary({'method': method})
                summarize.assert_not_called()

    def test_complete_calibration_reports_both_confidences_and_all_planned_calls(self):
        result = b4.summary(report_data())
        self.assertTrue(result['passes'])
        self.assertIsNone(result['product'])
        self.assertIsNone(result['b4_passes'])
        self.assertEqual(result['b4_verdict'], 'not executed')
        calibration = result['calibration']
        self.assertEqual(calibration['gate_confidence'], 0.995)
        self.assertEqual(calibration['diagnostic_confidence'], 0.95)
        self.assertEqual(calibration['coverage'], {'planned': 4992, 'executed': 4992,
                                                 'missing': 0, 'timed': 3072, 'failed': 0})
        self.assertEqual(set(calibration['arms']), {'aa', 'null'})

    def test_missing_warmup_timed_call_or_mandatory_capture_cannot_pass(self):
        for corruption in ('warmup', 'timed', 'capture', 'before_launch'):
            with self.subTest(corruption=corruption):
                data = report_data()
                if corruption in ('warmup', 'timed'):
                    index = next(i for i, row in enumerate(data['receipts']) if row['timed'] == (corruption == 'timed'))
                    data['receipts'].pop(index)
                elif corruption == 'capture':
                    del data['receipts'][0]['telemetry']['child']['utime']
                else:
                    data['receipts'][-1] = {**data['receipts'][-1], 'executed': False,
                                            'error': 'capture unavailable', 'exit': None}
                self.reject_or_fail(data)

    def test_prelaunch_failure_is_not_counted_as_an_executed_child(self):
        data = report_data()
        data['receipts'][-1] = {**data['receipts'][-1], 'executed': False,
                                'error': 'capture unavailable', 'exit': None}
        result = b4.calibration_summary(data)
        self.assertEqual(result['coverage']['executed'], 4991)
        self.assertEqual(result['coverage']['missing'], 1)
        self.assertFalse(result['passes'])

    def test_calibration_ratio_is_logical_a_over_b_even_in_ba_pairs(self):
        data = report_data()
        for row in data['receipts']:
            row['elapsed'] = 1.04 if row['side'] == 'A' else 1.0
        result = b4.calibration_summary(data)
        for arm in result['arms'].values():
            self.assertAlmostEqual(arm['estimate'], 1.04)
            self.assertFalse(arm['point_in_band'])
            self.assertFalse(arm['passes'])

    def test_optional_counter_unavailability_and_resets_do_not_create_a_gate(self):
        data = report_data()
        for row in data['receipts']:
            row['telemetry']['steal_ticks'] = {'status': 'invalid', 'reason': 'counter moved backwards', 'delta': None}
        self.assertTrue(b4.summary(data)['passes'])

    def test_missing_or_corrupt_fallback_capture_cannot_pass(self):
        for capture in ('missing', {}, {'lines': 0}, {**clean_log(), 'invalid': 1},
                        {**clean_log(), 'lines': 1, 'refused': 1}, {**clean_log(), 'void': True}):
            with self.subTest(capture=capture):
                data = report_data()
                if capture == 'missing':
                    del data['fallback_log']
                else:
                    data['fallback_log'] = capture
                self.reject_or_fail(data)

    def test_missing_or_altered_public_roster_cannot_pass(self):
        for corruption in ('missing', 'count', 'rank', 'alias', 'class', 'duplicate'):
            with self.subTest(corruption=corruption):
                data = report_data()
                if corruption == 'missing':
                    del data['selection']
                else:
                    rows = data['selection']['calibration']
                    if corruption == 'count':
                        rows.pop()
                    elif corruption == 'duplicate':
                        rows[1] = copy.deepcopy(rows[0])
                    else:
                        rows[0][{'rank': 'rank', 'alias': 'file', 'class': 'class'}[corruption]] = 'altered'
                self.reject_or_fail(data)

    def test_calibration_only_report_rejects_any_product_receipt(self):
        data = report_data()
        pair = b4.protocol.product_schedule(data['selection']['product'][:1], data['commands'][:1])[0]
        data['receipts'] += calls_for([pair])
        with self.assertRaises(ValueError):
            b4.summary(data)

    def test_refusal_requires_both_mandatory_resource_captures(self):
        data = report_data('product')
        pair = b4.protocol.product_schedule(data['selection']['product'], data['commands'])[0]
        calls = calls_for([pair])
        calls[0]['exit'] = 4
        del calls[0]['telemetry']['child']['utime']
        data['receipts'] += calls
        with mock.patch.object(b4.speed, 'interval', return_value=None):
            result = b4.summary(data)
        coverage = result['product']['coverage']
        self.assertEqual(coverage['refused'], 0)
        self.assertEqual(coverage['failed'], 1)
        self.assertFalse(result['passes'])

    def test_product_refusal_log_does_not_retroactively_fail_clean_calibration(self):
        data = report_data('product')
        data['calibration_fallback_log'] = clean_log()
        data['fallback_log'] = {**clean_log(), 'lines': 1, 'refused': 1}
        pair = b4.protocol.product_schedule(data['selection']['product'], data['commands'])[0]
        calls = calls_for([pair])
        calls[0]['exit'] = 4
        data['receipts'] += calls
        with mock.patch.object(b4.speed, 'interval', return_value=None):
            result = b4.summary(data)
        self.assertTrue(result['calibration']['passes'])
        self.assertEqual(result['product']['coverage']['refused'], 1)
        self.assertFalse(result['passes'])

    def test_product_counts_are_exclusive_and_five_side_medians_ignore_warmup(self):
        data = report_data('product')
        entries = data['selection']['product'][:4]
        schedules = [b4.protocol.product_schedule([entry], data['commands'][:1]) for entry in entries]
        success = calls_for(schedules[0])
        durations = {'A': [1, 2, 3, 4, 5], 'B': [10, 1, 10, 1, 10]}
        for row in success:
            row['elapsed'] = durations[row['side']][row['pair']] if row['timed'] else 100
        refused = calls_for(schedules[1][:1])
        refused[0]['exit'] = 4
        failed = calls_for(schedules[2][:5])
        failed[-1]['exit'] = None
        data['receipts'] += success + refused + failed
        with mock.patch.object(b4.speed, 'interval', return_value=[0.3, 0.3]):
            result = b4.summary(data)
        product = result['product']
        coverage = product['coverage']
        self.assertEqual((coverage['timed'], coverage['refused'], coverage['failed']), (1, 1, 1))
        self.assertEqual(coverage['total'], 93 * 57)
        self.assertEqual(coverage['total'], sum(coverage[key] for key in ('timed', 'refused', 'failed', 'missing', 'ineligible')))
        overall = next(row for row in product['rows'] if row['group'] == 'all')
        self.assertEqual((overall['median'], overall['faster_share']), (0.3, 1))
        self.assertFalse(result['passes'])

    def test_incomplete_calibration_blocks_even_a_complete_fast_product(self):
        data = report_data('product')
        data['receipts'].pop()
        product_calls = calls_for(b4.protocol.product_schedule(data['selection']['product'], data['commands']))
        for row in product_calls:
            row['elapsed'] = 1 if row['side'] == 'A' else 2
        data['receipts'] += product_calls
        with mock.patch.object(b4.speed, 'interval', return_value=[0.5, 0.5]):
            result = b4.summary(data)
        self.assertTrue(result['product']['passes'])
        self.assertFalse(result['calibration']['passes'])
        self.assertFalse(result['passes'])


class RunMode(MediaFree):
    def test_calibration_only_never_prepares_or_executes_product_even_when_gate_passes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            instance = root / 'instance.json'
            instance.write_text('{}')
            opts = types.SimpleNamespace(max_seconds=10, cap_usd=1, usd_per_hour=1, reserve_usd=0.1,
                                         manifest='freeze', coreav='candidate', oracle_bin='oracle',
                                         instance=instance, cpu=2, out=root / 'result.json',
                                         dir=root / 'synthetic', mode='calibration')
            manifest = {'sources': dict.fromkeys(('plan', 'tooling', 'candidate', 'baseline'), 'a' * 40),
                        'binaries': dict.fromkeys(('candidate', 'ffprobe', 'ffmpeg'), sha('binary')),
                        'command_sha256': sha('commands'), 'commands': [],
                        'selection': {'calibration': [], 'product': []}}
            with mock.patch.object(b4.platform, 'system', return_value='Linux'), \
                    mock.patch.object(b4.os, 'pidfd_open', create=True), \
                    mock.patch.object(b4, 'load_manifest', return_value=manifest), \
                    mock.patch.object(b4, 'digest', return_value=sha('binary')), \
                    mock.patch.object(b4.subprocess, 'run'), \
                    mock.patch.object(b4, 'host_gate', return_value={}), \
                    mock.patch.object(b4, 'snapshot', return_value=sample()), \
                    mock.patch.object(b4.release_round, 'environment', return_value={}), \
                    mock.patch.object(b4, 'payload_pool', return_value=mock.Mock()) as pool, \
                    mock.patch.object(b4, 'execute_phase') as execute, \
                    mock.patch.object(b4, 'fallback', return_value=clean_log()), \
                    mock.patch.object(b4, 'calibration_summary', return_value={'passes': True}), \
                    mock.patch.object(b4, 'summary', return_value={'passes': True}), \
                    mock.patch.object(b4.signal, 'signal'), \
                    mock.patch.object(b4.signal, 'setitimer'):
                self.assertEqual(b4.run(opts), 0)
            self.assertEqual([call.args[0] for call in execute.call_args_list], ['calibration'])
            self.assertEqual(pool.call_count, 1)
            self.assertEqual(json.loads(opts.out.read_text())['mode'], 'calibration')


@unittest.skipUnless(hasattr(os, 'pidfd_open'), 'real-child checks run on Linux Actions')
class LinuxDeadline(unittest.TestCase):
    def test_total_deadline_kills_and_reaps_the_timed_child(self):
        def deadline(signum, frame):
            raise TimeoutError('synthetic total deadline')
        previous = signal.signal(signal.SIGALRM, deadline)
        try:
            with mock.patch.object(b4.speed.os, 'pidfd_open', wraps=os.pidfd_open) as opened, \
                    mock.patch.object(b4.speed.time, 'sleep', side_effect=AssertionError('no polling')):
                signal.setitimer(signal.ITIMER_REAL, 0.2)
                row = b4.observe([sys.executable, '-c', 'import time; time.sleep(10)'], {}, 0, 0)
            self.assertFalse(b4.valid_call(row))
            self.assertIn('infrastructure', row['error'])
            with self.assertRaises(ChildProcessError):
                os.waitpid(opened.call_args.args[0], os.WNOHANG)
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, previous)


if __name__ == '__main__':
    unittest.main()
