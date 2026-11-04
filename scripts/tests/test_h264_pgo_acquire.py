"""Synthetic-only acquisition and amendment boundary tests."""
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import h264_pgo_acquire as collect
import h264_pgo_round as driver


class Acquisition(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        (self.root / 'preparation').mkdir()
        driver.write(self.root / 'preparation/collection.json', [])
        driver.write(self.root / 'preparation/state.json', {'captures': []})
        self.plan = json.loads(driver.PLAN.read_text())
        self.state = {'captures': [], 'items': {}, 'decisions': [], 'rows': [], 'downloads': {},
                      'deadline': 1000, 'byte_cap': collect.TOTAL_MAX}
        self.counts = {c: 0 for c in driver.bars.CLASSES}
        self.candidate = {'identity': 'https://example.test/new', 'groups': ['new']}
        self.original = self.root / 'operator/originals/o00000.source'
        self.original.parent.mkdir(parents=True)
        self.original.write_bytes(b'synthetic-container')
        self.metadata = {'codec_name': 'h264', 'pix_fmt': 'yuv420p', 'field_order': 'progressive',
                         'profile': 'High', 'width': 640, 'height': 480, 'frames': 3}
        self.used = {key: set() for key in ('hashes', 'identities', 'groups')}

    def runner(self, argv, **kwargs):
        if argv[0].endswith('ffmpeg'):
            Path(argv[-1]).write_bytes(b'synthetic-packet-copy')
        else:
            stream = {k: v for k, v in self.metadata.items() if k != 'frames'}
            stream.update(index=0, codec_type='video', nb_read_frames=str(self.metadata['frames']))
            kwargs['stdout'].write(json.dumps({'streams': [stream]}).encode())
        return subprocess.CompletedProcess(argv, 0)

    def prepare(self, runner=None, replay=False, rows=None):
        return collect.prepare(self.root, 0, self.candidate, self.original, collect.digest(self.original),
            self.state, lambda: None, self.used, rows or [], self.counts, self.plan,
            runner or self.runner, lambda: 0, replay)

    def test_success_replays_without_any_child_or_changed_bytes(self):
        decision, row = self.prepare()
        self.assertEqual(decision['status'], 'selected')
        self.assertEqual(row['class'], 'sd')
        with mock.patch.object(collect, '_capture', wraps=collect._capture):
            replayed = self.prepare(runner=mock.Mock(side_effect=AssertionError('child rerun')), replay=True)
        self.assertEqual(replayed, (decision, row))
        (self.root / 'media/h000.h264').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'selected elementary'):
            self.prepare(replay=True)

    def test_all_registered_capture_outcomes_are_recorded_and_not_retried(self):
        cases = ('stderr', 'nonzero-exit', 'timeout', 'output-limit', 'invalid-metadata')
        for why in cases:
            with self.subTest(reason=why):
                self.state['captures'] = []
                self.state['items'] = {}
                def failed(argv, **kwargs):
                    if why == 'timeout':
                        raise subprocess.TimeoutExpired(argv, 10)
                    if why == 'stderr':
                        kwargs['stderr'].write(b'Invalid NAL unit size')
                        kwargs['stdout'].write(b'{"streams":[]}')
                    elif why == 'output-limit':
                        kwargs['stdout'].write(b'x' * ((1 << 20) + 1))
                    elif why == 'invalid-metadata':
                        kwargs['stdout'].write(b'{"streams": [null]}')
                    return subprocess.CompletedProcess(argv, 9 if why == 'nonzero-exit' else 0)
                decision, selected = self.prepare(runner=failed)
                self.assertIsNone(selected)
                self.assertEqual(decision['reason'], why)
                self.assertIsNone(decision['metadata'])
                self.assertEqual(decision['failure_classes'], list(driver.bars.CLASSES))
                self.assertEqual(self.prepare(runner=mock.Mock(side_effect=AssertionError('rerun')), replay=True),
                                 (decision, selected))

    def test_extraction_failures_charge_only_the_successful_probe_class(self):
        for why in ('stderr', 'empty-extraction', 'invalid-extraction', 'invalid-metadata'):
            with self.subTest(reason=why):
                self.state['captures'] = []
                self.state['items'] = {}
                calls = []
                def failed(argv, **kwargs):
                    calls.append(argv)
                    result = self.runner(argv, **kwargs)
                    if argv[0].endswith('ffmpeg'):
                        if why == 'stderr':
                            kwargs['stderr'].write(b'packet-copy diagnostic')
                        if why == 'empty-extraction':
                            Path(argv[-1]).write_bytes(b'')
                    elif len(calls) == 3:
                        kwargs['stdout'].seek(0)
                        kwargs['stdout'].truncate()
                        if why == 'invalid-metadata':
                            kwargs['stdout'].write(b'[]')
                        else:
                            stream = {'index': 0, 'codec_type': 'video', 'nb_read_frames': '0'}
                            kwargs['stdout'].write(json.dumps({'streams': [stream]}).encode())
                    return result
                decision, row = self.prepare(runner=failed)
                self.assertIsNone(row)
                self.assertEqual(decision['reason'], why)
                self.assertEqual(decision['failure_classes'], ['sd'])
                self.assertEqual(decision['metadata'], self.metadata)
                self.assertEqual(self.prepare(replay=True), (decision, row))

    def test_fatal_errors_never_become_source_exclusions(self):
        for exception in (OSError('host error'), RuntimeError('checkpoint error'), ValueError('unknown')):
            with self.subTest(exception=type(exception).__name__):
                self.state['captures'] = []
                def failed(*args, **kwargs):
                    kwargs['stderr'].write(b'diagnostic')
                    raise exception
                with self.assertRaises(type(exception)):
                    self.prepare(runner=failed)
                with self.assertRaises((ValueError, KeyError)):
                    self.prepare(replay=True)
        for message in (b"Protocol 'https' not on whitelist 'file,pipe'!", b'Protocol not found'):
            self.state['captures'] = []
            def protocol(argv, **kwargs):
                kwargs['stderr'].write(message)
                return subprocess.CompletedProcess(argv, 1)
            with self.assertRaisesRegex(ValueError, 'external protocol'):
                self.prepare(runner=protocol)

    def test_signals_and_operator_deadlines_are_fatal(self):
        def signal(argv, **kwargs):
            return subprocess.CompletedProcess(argv, -25)
        with self.assertRaisesRegex(ValueError, 'signal or resource'):
            self.prepare(runner=signal)
        self.state['captures'] = []
        self.state['deadline'] = 0
        with self.assertRaises(collect.Incomplete):
            self.prepare()

    def test_started_duplicate_changed_and_missing_receipts_are_fatal(self):
        self.prepare()
        record = self.state['captures'][0]
        record['status'] = 'started'
        with self.assertRaisesRegex(ValueError, 'terminal'):
            self.prepare()
        record['status'] = 'complete'
        self.state['captures'].append(dict(record))
        with self.assertRaisesRegex(ValueError, 'duplicate capture'):
            self.prepare()
        self.state['captures'].pop()
        (self.root / record['stdout']).write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'bytes changed'):
            self.prepare()
        (self.root / record['stdout']).unlink()
        with self.assertRaises(OSError):
            self.prepare()

    def test_relocated_terminal_capture_matches_but_keeps_its_exact_command(self):
        decision, row = self.prepare()
        for record in self.state['captures']:
            record['argv'] = [arg.replace(str(self.root), '/original/root') for arg in record['argv']]
            record['root'] = '/original/root'
        self.assertEqual(self.prepare(replay=True), (decision, row))
        self.assertTrue(self.state['captures'][0]['argv'][0].startswith('/original/root/'))

    def test_unknown_class_charges_only_unfilled_classes_and_stops_at_limit(self):
        self.counts['sd'] = 16
        self.assertEqual(collect.charges(None, self.counts, self.plan), ['720p', '1080p', '4k'])
        decision = {'capture_status': 'failed', 'status': 'excluded', 'reason': 'stderr',
                    'metadata': None, 'failure_classes': list(driver.bars.CLASSES)}
        counts, failures = collect.failure_counts([decision] * 16, self.plan)
        self.assertEqual(failures, {c: 16 for c in driver.bars.CLASSES})
        self.assertEqual(counts, {c: 0 for c in driver.bars.CLASSES})
        with self.assertRaisesRegex(ValueError, 'replacement limit'):
            collect.failure_counts([decision] * 17, self.plan)
        with self.assertRaisesRegex(ValueError, 'class charge'):
            collect.failure_counts([decision | {'failure_classes': ['sd']}], self.plan)
        with self.assertRaisesRegex(ValueError, 'unregistered'):
            collect.failure_counts([decision | {'reason': 'unknown'}], self.plan)

    def test_order_and_same_class_quota_remain_binding(self):
        plan = self.plan | {'count_per_class': 1}
        candidates = [{'identity': f'https://example.test/{i}', 'groups': [str(i)]} for i in range(5)]
        registry = {owner: {'commitment': 'a' * 64, 'hashes': ['b' * 64], 'identities': ['old'], 'groups': ['old']}
                    for owner in driver.COVERAGE}
        failed = {'identity': candidates[0]['identity'], 'capture_status': 'failed', 'metadata': None,
                  'status': 'excluded', 'reason': 'stderr', 'failure_classes': list(driver.bars.CLASSES)}
        decisions, rows = [failed], []
        for i, (cls, w, h) in enumerate((('720p', 1280, 720), ('sd', 640, 480),
                                        ('4k', 3840, 2160), ('1080p', 1920, 1080)), 1):
            value = self.metadata | {'width': w, 'height': h}
            sha = f'{i:064x}'
            decisions.append({'identity': candidates[i]['identity'], 'capture_status': 'ok', 'metadata': value,
                              'status': 'selected', 'source_sha256': sha})
            rows.append({'identity': candidates[i]['identity'], 'groups': candidates[i]['groups'],
                         'source_sha256': sha, 'class': cls, 'width': w, 'height': h})
        driver.selection(rows, candidates, decisions, registry, plan)
        with self.assertRaises(ValueError):
            driver.selection(rows, candidates, [decisions[1], decisions[0], *decisions[2:]], registry, plan)
        with self.assertRaises(ValueError):
            driver.selection(rows[1:], candidates, decisions, registry, plan)
        with self.assertRaises(ValueError):
            driver.selection([rows[0] | {'class': 'sd'}, *rows[1:]], candidates, decisions, registry, plan)

    def test_original_reuse_does_not_fetch_or_refund_prior_charge(self):
        row = {'identity': self.candidate['identity'], 'size': len(self.original.read_bytes()), 'checksums': {}}
        sha = collect.digest(self.original)
        self.state['downloads'] = {'0': [{'status': 'complete', 'charged_bytes': row['size'] + 1,
                                         'sha256': sha, 'received_bytes': row['size']}]}
        before = json.loads(json.dumps(self.state['downloads']))
        source, value = collect._original(self.root, row, 0, self.state, lambda: None,
            mock.Mock(side_effect=AssertionError('refetch')), lambda: 0, mock.Mock())
        self.assertEqual((source, value), (self.original, sha))
        self.assertEqual(self.state['downloads'], before)
        self.original.unlink()
        with self.assertRaisesRegex(ValueError, 'checkpointed original'):
            collect._original(self.root, row, 0, self.state, lambda: None, mock.Mock(), lambda: 0, mock.Mock())

    def test_byte_cap_is_precharged_before_network_and_survives_interruption(self):
        self.original.unlink()
        row = {'identity': self.candidate['identity'], 'size': 3, 'checksums': {}}
        self.state['byte_cap'] = 3
        opener = mock.Mock()
        with self.assertRaises(collect.Incomplete):
            collect._original(self.root, row, 0, self.state, lambda: None, opener, lambda: 0, mock.Mock())
        opener.assert_not_called()
        self.state['byte_cap'] = 8
        def failed(*args, **kwargs):
            self.assertEqual(self.state['downloads']['0'][-1]['charged_bytes'], 4)
            raise OSError('network lost')
        for _ in range(2):
            with self.assertRaises(OSError):
                collect._original(self.root, row, 0, self.state, lambda: None, failed, lambda: 0, mock.Mock())
        with self.assertRaises(collect.Incomplete):
            collect._original(self.root, row, 0, self.state, lambda: None, opener, lambda: 0, mock.Mock())
        self.assertEqual(sum(a['charged_bytes'] for a in self.state['downloads']['0']), 8)
        opener.assert_not_called()

    def test_inherited_credentials_are_not_passed_to_children(self):
        with mock.patch.dict(os.environ, {'R2_SECRET_ACCESS_KEY': 'synthetic-secret', 'CARGO_HOME': 'unsafe'}), \
             mock.patch.object(collect.subprocess, 'run') as run:
            collect.bounded_runner(['oracle'], timeout=3, check=False)
        self.assertNotIn('R2_SECRET_ACCESS_KEY', run.call_args.kwargs['env'])
        self.assertNotIn('CARGO_HOME', run.call_args.kwargs['env'])
        self.assertIn('preexec_fn', run.call_args.kwargs)

    def test_premerge_receipt_freeze_stops_before_allocation_or_media(self):
        with mock.patch.object(collect.sys, 'platform', 'linux'), \
             mock.patch.object(driver, 'merged', side_effect=ValueError('not merged')), \
             mock.patch.object(driver, 'verify_allocation') as allocation:
            with self.assertRaises(ValueError):
                collect.freeze(self.root)
        allocation.assert_not_called()
        self.assertFalse((self.root / 'acquisition-receipt.json').exists())

    def test_isolated_cli_imports_without_running_acquisition(self):
        result = subprocess.run([sys.executable, '-I', collect.__file__, '--help'],
                                capture_output=True, text=True, timeout=10, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('freeze', result.stdout)

    def test_missing_external_acquisition_digest_is_not_inferred_from_nearby_receipt(self):
        driver.write(self.root / 'acquisition-receipt.json', {'anything': 'not registered'})
        with self.assertRaisesRegex(ValueError, 'separately recorded'):
            collect.verify_receipt(self.root, None)

    def test_no_replacement_after_evaluation_started(self):
        (self.root / 'events.jsonl').write_text('')
        with mock.patch.object(collect.sys, 'platform', 'linux'), \
             mock.patch.object(collect, 'verify_receipt') as receipt:
            with self.assertRaisesRegex(ValueError, 'replacement forbidden'):
                collect.acquire(self.root, 'a' * 64, 'b' * 64, opener=mock.Mock(), checkpoint=mock.Mock())
        receipt.assert_not_called()



    def integrated(self, failures=1, checkpoint=None, finish=True):
        self.original.unlink(missing_ok=True)
        geometry = [(640, 480), (1280, 720), (1920, 1080), (3840, 2160)]
        count = failures + 4
        candidates = [{'identity': f'https://example.test/{i}', 'groups': [str(i)]} for i in range(count)]
        inventory = {'population_sha256': None, 'files': {}}
        bodies = {}
        for i, candidate in enumerate(candidates):
            bodies[candidate['identity']] = f'synthetic-container-{i}'.encode()
            inventory['files'][candidate['identity']] = candidate | {
                'size': len(bodies[candidate['identity']]), 'checksums': {}}
        driver.write(self.root / 'population.json', {'synthetic': True})
        inventory['population_sha256'] = collect.digest(self.root / 'population.json')
        driver.write(self.root / 'operator/acquisition.json', inventory)
        registry = {owner: {'commitment': 'a' * 64, 'hashes': ['b' * 64], 'identities': ['old'], 'groups': ['old']}
                    for owner in driver.COVERAGE}
        driver.write(self.root / 'exclusions.json', registry)
        initial = self.state | {'notifications': [], 'status': 'incomplete'}
        driver.write(self.root / 'preparation/state.json', initial)
        driver.write(self.root / 'preparation/roster.json', [])
        config = {'allocation_sha256': 'a' * 64, 'artifact_revision': 'c' * 40}
        receipt = {'deadline': 1000, 'tooling_revision': 'd' * 40}
        plan = self.plan | {'count_per_class': 1}
        plan_path = self.root / 'synthetic-plan.json'
        driver.write(plan_path, plan)
        child_calls, fetches = [], []
        def runner(argv, **kwargs):
            child_calls.append(argv)
            source = Path(argv[-1])
            if argv[0].endswith('ffmpeg'):
                original = Path(argv[argv.index('-i') + 1])
                source.write_bytes(b'packets-' + original.stem.encode())
            else:
                index = int(source.stem[1:])
                if index < failures:
                    kwargs['stderr'].write(b'Invalid NAL unit size')
                else:
                    width, height = geometry[index - failures]
                    stream = self.metadata | {'index': 0, 'codec_type': 'video', 'width': width,
                                              'height': height, 'nb_read_frames': '3'}
                    kwargs['stdout'].write(json.dumps({'streams': [stream]}).encode())
            return subprocess.CompletedProcess(argv, 0)
        def opener(identity, **kwargs):
            fetches.append(identity)
            result = io.BytesIO(bodies[identity])
            result.status = 200
            return result
        import oracle_pin
        patches = [mock.patch.object(driver, 'PLAN', plan_path), mock.patch.object(driver, 'merged', return_value='d' * 40),
                   mock.patch.object(driver, 'verify_allocation', return_value=candidates),
                   mock.patch.object(collect, 'verify_receipt', return_value=receipt),
                   mock.patch.object(collect, 'policy', return_value=config),
                   mock.patch.object(collect, 'bounded_runner', side_effect=runner),
                   mock.patch.object(oracle_pin, 'verify_bin', return_value=[]),
                   mock.patch.object(collect.sys, 'platform', 'linux')]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        def run(callback=None, clock=lambda: 1):
            return collect.acquire(self.root, 'a' * 64, 'e' * 64, opener=opener,
                                   checkpoint=callback or (lambda *_: None), clock=clock)
        if finish:
            run(checkpoint)
        return run, candidates, plan, child_calls, fetches

    def test_full_continuation_and_offline_replay_are_idempotent(self):
        run, candidates, plan, calls, fetches = self.integrated()
        state = json.loads((self.root / 'operator/amended-state.json').read_text())
        self.assertEqual(len(state['rows']), 4)
        self.assertEqual(len(state['decisions']), 5)
        self.assertEqual(state['decisions'][0]['failure_classes'], list(driver.bars.CLASSES))
        before = (len(calls), len(fetches), state['completed_at'])
        state = run(clock=lambda: 2000)
        self.assertEqual((len(calls), len(fetches), state['completed_at']), before)
        collect.audit_collection(self.root, candidates, plan, 'e' * 64)
        driver.selection(state['rows'], candidates, state['decisions'],
                         json.loads((self.root / 'exclusions.json').read_text()), plan)
        state['downloads']['0'][0]['charged_bytes'] = 0
        driver.write(self.root / 'operator/amended-state.json', state)
        with self.assertRaisesRegex(ValueError, 'accounting differs'):
            collect.audit_collection(self.root, candidates, plan, 'e' * 64)

    def test_lost_decision_ack_retries_checkpoint_not_capture_or_download(self):
        run, candidates, plan, calls, fetches = self.integrated(finish=False)
        keys = []
        def checkpoint(event, paths):
            keys.append(event['key'])
            if event['key'] == 'amended:decision:0' and keys.count(event['key']) == 1:
                raise OSError('lost acknowledgement')
        with self.assertRaises(OSError):
            run(checkpoint)
        first = len(calls), len(fetches)
        self.assertEqual(first, (1, 1))
        state = run(checkpoint)
        self.assertEqual(len(calls), 13)
        self.assertEqual(len(fetches), 5)
        self.assertEqual(keys.count('amended:decision:0'), 2)
        self.assertEqual(state['status'], 'complete')
        collect.audit_collection(self.root, candidates, plan, 'e' * 64)

    def test_replacement_limit_keeps_the_final_exclusion_and_never_fetches_successor(self):
        run, candidates, plan, calls, fetches = self.integrated(failures=16, finish=False)
        with self.assertRaisesRegex(collect.Incomplete, 'replacement limit'):
            run()
        state = json.loads((self.root / 'operator/amended-state.json').read_text())
        self.assertEqual(len(state['decisions']), 16)
        self.assertEqual(len(fetches), 16)
        self.assertEqual(state['status'], 'incomplete')
        with self.assertRaisesRegex(collect.Incomplete, 'replacement limit'):
            run()
        self.assertEqual(len(calls), 16)
        self.assertEqual(len(fetches), 16)

    def test_receipt_paths_cannot_escape_the_acquisition_directory(self):
        self.prepare()
        self.state['captures'][0]['stdout'] = '../private-key'
        with self.assertRaisesRegex(ValueError, 'path escapes'):
            self.prepare(replay=True)



    def test_receipt_binds_stopped_preparation_policy_tooling_and_external_digest(self):
        config = json.loads(collect.POLICY.read_text())
        driver.write(self.root / 'preparation/roster.json', [])
        driver.write(self.root / 'operator/acquisition.json', {'synthetic': True})
        config['preparation'] = {name: collect.digest(self.root / 'preparation' / name)
                                 for name in ('state.json', 'collection.json', 'roster.json')}
        config['source_inventory_sha256'] = collect.digest(self.root / 'operator/acquisition.json')
        receipt = {'schema': 1, 'policy': collect.digest(collect.POLICY), 'allocation': config['allocation_sha256'],
                   'tooling_revision': 'a' * 40, 'deadline': 1000}
        driver.write(self.root / 'acquisition-receipt.json', receipt)
        external = collect.digest(self.root / 'acquisition-receipt.json')
        with mock.patch.object(collect, 'policy', return_value=config):
            self.assertEqual(collect.verify_receipt(self.root, external, 'a' * 40), receipt)
            with self.assertRaisesRegex(ValueError, 'tooling changed'):
                collect.verify_receipt(self.root, external, 'b' * 40)
            driver.write(self.root / 'acquisition-receipt.json', receipt | {'deadline': 2000})
            with self.assertRaisesRegex(ValueError, 'separately recorded'):
                collect.verify_receipt(self.root, external)
            driver.write(self.root / 'acquisition-receipt.json', receipt)
            (self.root / 'preparation/state.json').write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'stopped preparation changed'):
                collect.verify_receipt(self.root, external)

    def test_complete_collection_binding_refuses_receipt_or_tool_mutation(self):
        _, candidates, plan, _, _ = self.integrated()
        driver.write(self.root / 'acquisition-receipt.json', {'synthetic': True})
        config = collect.policy() | {'preparation': {'state.json': 'synthetic', 'collection.json': 'synthetic',
                                                    'roster.json': 'synthetic'}}
        with mock.patch.object(collect, 'policy', return_value=config):
            collect.bind(self.root, 'e' * 64)
            collect.audit_bound(self.root, plan, 'a' * 64, 'e' * 64)
            binding = json.loads((self.root / 'acquisition-binding.json').read_text())
            binding['tools']['scripts/h264_pgo_acquire.py'] = '0' * 64
            driver.write(self.root / 'acquisition-binding.json', binding)
            with self.assertRaisesRegex(ValueError, 'binding changed'):
                collect.audit_bound(self.root, plan, 'a' * 64, 'e' * 64)

    def test_old_zero_exit_diagnostic_capture_is_excluded_without_retry_after_relocation(self):
        raw = self.root / 'operator/captures'
        raw.mkdir()
        stdout, stderr = raw / 'c00000.stdout', raw / 'c00000.stderr'
        stdout.write_bytes(b'{"streams": []}')
        stderr.write_bytes(b'Invalid NAL unit size')
        argv = collect.probe_command(Path('/old/evaluation'), Path('/old/evaluation/operator/originals/o00000.source'))
        record = {'kind': 'source-probe', 'argv': argv, 'status': 'failed', 'returncode': 0, 'error': 'ValueError',
                  'stdout': 'operator/captures/c00000.stdout', 'stderr': 'operator/captures/c00000.stderr',
                  'files': {'stdout': collect.digest(stdout), 'stderr': collect.digest(stderr)}}
        self.state['captures'] = [record]
        driver.write(self.root / 'preparation/state.json', self.state)
        decision, selected = self.prepare(runner=mock.Mock(side_effect=AssertionError('legacy retry')))
        self.assertIsNone(selected)
        self.assertEqual(decision['reason'], 'stderr')
        self.assertEqual(decision['failure_classes'], list(driver.bars.CLASSES))
        self.assertEqual(self.state['captures'], [record])


if __name__ == '__main__':
    unittest.main()
