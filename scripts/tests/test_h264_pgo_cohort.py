"""Media-free checks of the exact registered evaluation cohort."""
from collections import Counter
import copy
import hashlib
import json
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import h264_pgo_acquire as acquisition
import h264_pgo_round as driver


class EvaluationCohort(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'operator').mkdir()
        (self.root / 'media').mkdir()
        self.plan = json.loads(driver.PLAN.read_text())
        self.registration = json.loads(driver.EVALUATION.read_text())
        self.rows, self.candidates, self.decisions = [], [], []
        for index, entry in enumerate(self.registration['inputs']):
            body = f'synthetic-stream-{index}'.encode()
            entry['sha256'] = hashlib.sha256(body).hexdigest()
            entry['source_sha256'] = hashlib.sha256(f'synthetic-source-{index}'.encode()).hexdigest()
            (self.root / 'media' / (entry['label'] + '.h264')).write_bytes(body)
            candidate = {'identity': f'https://example.test/{index}', 'groups': [str(index)]}
            self.candidates.append(candidate)
            self.rows.append(entry | candidate | {'stream_index': 0})
            self.decisions.append({'identity': candidate['identity'], 'source_sha256': entry['source_sha256'],
                'status': 'selected', 'reason': None, 'capture_status': 'ok',
                'metadata': {'codec_name': 'h264', 'pix_fmt': 'yuv420p', 'field_order': 'progressive',
                             'profile': 'High', 'width': entry['width'], 'height': entry['height'],
                             'frames': entry['source_frames']}})
        driver.write(self.root / 'roster.json', self.rows)
        driver.write(self.root / 'collection.json', self.decisions)
        for name in ('operator/amended-state.json', 'acquisition-report.json',
                     'allocation-contract.json', 'acquisition-receipt.json'):
            driver.write(self.root / name, {'synthetic': name})
        for key, name in (('allocation_sha256', 'allocation-contract.json'),
                          ('acquisition_sha256', 'acquisition-receipt.json')):
            self.registration[key] = driver.digest(self.root / name)
        self.refresh_snapshot()
        self.manifest = self.root / 'evaluation.json'
        driver.write(self.manifest, self.registration)
        patch = mock.patch.object(driver, 'EVALUATION', self.manifest)
        patch.start()
        self.addCleanup(patch.stop)
        self.registry = {owner: {'commitment': 'a' * 64, 'hashes': ['b' * 64],
                                'identities': ['old'], 'groups': ['old']}
                         for owner in driver.COVERAGE}

    def refresh_snapshot(self):
        self.registration['snapshot'] = {name: driver.digest(self.root / name)
                                         for name in self.registration['snapshot']}

    def admit(self):
        return driver.evaluation_cohort(self.root, self.plan, self.registration['allocation_sha256'])

    def test_exact_cohort_admits_without_relaxing_original_acquisition_quotas(self):
        cohort = self.admit()
        self.assertEqual(cohort['counts'], {'sd': 16, '720p': 16, '1080p': 16, '4k': 9})
        self.assertEqual(self.plan['count_per_class'], 16)
        self.assertEqual(len(driver.roster(self.rows, self.registry, self.plan, counts=cohort['counts'])), 57)
        driver.selection(self.rows, self.candidates, self.decisions, self.registry, self.plan,
                         expected_counts=cohort['counts'])
        with self.assertRaises(ValueError):
            driver.roster(self.rows, self.registry, self.plan)
        with self.assertRaises(ValueError):
            driver.selection(self.rows, self.candidates, self.decisions, self.registry, self.plan)

    def test_missing_extra_reordered_relabeled_and_substituted_rosters_are_refused(self):
        variants = [self.rows[:-1], self.rows + [self.rows[-1]],
                    [self.rows[1], self.rows[0], *self.rows[2:]],
                    [self.rows[0] | {'label': 'h999'}, *self.rows[1:]],
                    [self.rows[0] | {'sha256': 'f' * 64}, *self.rows[1:]]]
        for rows in variants:
            with self.subTest(rows=len(rows)):
                driver.write(self.root / 'roster.json', rows)
                self.refresh_snapshot()
                driver.write(self.manifest, self.registration)
                with self.assertRaises(ValueError):
                    self.admit()

    def test_every_frozen_record_and_both_external_receipts_remain_bound(self):
        names = [*self.registration['snapshot'], 'allocation-contract.json', 'acquisition-receipt.json']
        for name in names:
            with self.subTest(name=name):
                path = self.root / name
                before = path.read_bytes()
                path.write_bytes(before + b'\n')
                with self.assertRaises(ValueError):
                    self.admit()
                path.write_bytes(before)

    def test_changed_or_missing_elementary_bytes_are_refused(self):
        path = self.root / 'media' / (self.rows[0]['label'] + '.h264')
        path.write_bytes(b'substituted')
        with self.assertRaises(ValueError):
            self.admit()
        path.unlink()
        with self.assertRaises(ValueError):
            self.admit()

    def test_extra_elementary_file_is_not_an_unregistered_backfill(self):
        (self.root / 'media/h057.h264').write_bytes(b'extra')
        with self.assertRaises(ValueError):
            self.admit()

    def test_changed_counts_cannot_authorize_a_different_cohort(self):
        self.registration['counts']['4k'] = 16
        driver.write(self.manifest, self.registration)
        with self.assertRaises(ValueError):
            self.admit()

    def test_run_and_audit_refuse_changed_cohort_before_media_or_statistics(self):
        driver.write(self.root / 'roster.json', list(reversed(self.rows)))
        receipt = {'commit': 'a' * 40}
        driver.write(self.root / 'reservation-receipt.json', receipt)
        with mock.patch.object(driver, 'merged', return_value='a' * 40), \
             mock.patch.object(driver, 'reservation_receipt'), \
             mock.patch.object(driver, 'reservation', return_value=receipt), \
             mock.patch.object(driver, 'reserved_evidence'), \
             mock.patch.object(driver, 'reserved_inputs'), \
             mock.patch.object(driver, 'execute') as execute, \
             mock.patch.object(driver.bars, 'paired_round_summary') as summarize:
            with self.assertRaises(ValueError):
                driver.run(self.root, self.plan, self.registration['allocation_sha256'],
                           self.registration['acquisition_sha256'])
            with self.assertRaises(ValueError):
                driver.audit(self.root, self.plan, self.registration['allocation_sha256'])
        execute.assert_not_called()
        summarize.assert_not_called()

    def test_closed_receipt_cannot_fetch_backfill_or_checkpoint_again(self):
        opener, checkpoint = mock.Mock(), mock.Mock()
        with self.assertRaisesRegex(ValueError, 'closed'):
            acquisition.acquire(self.root, self.registration['allocation_sha256'],
                                self.registration['acquisition_sha256'], opener=opener, checkpoint=checkpoint)
        opener.assert_not_called()
        checkpoint.assert_not_called()

    def test_exact_closed_snapshot_replays_all_captures_without_completing_acquisition(self):
        (self.root / 'preparation').mkdir()
        (self.root / 'operator/originals').mkdir()
        initial = {'decisions': [], 'rows': [], 'captures': [], 'downloads': {}}
        driver.write(self.root / 'preparation/state.json', initial)
        driver.write(self.root / 'preparation/collection.json', [])
        driver.write(self.root / 'preparation/roster.json', [])
        driver.write(self.root / 'exclusions.json', self.registry)
        state = initial | {'items': {}, 'notifications': [], 'byte_cap': acquisition.TOTAL_MAX,
                           'deadline': 1000, 'status': 'incomplete',
                           'acquisition_sha256': self.registration['acquisition_sha256']}
        inventory = {'files': {}}
        used = driver.exclusions(self.registry)
        def runner(argv, **kwargs):
            path = Path(argv[-1])
            if argv[0].endswith('ffmpeg'):
                index = int(Path(argv[argv.index('-i') + 1]).stem[1:])
                path.write_bytes(f'synthetic-stream-{index}'.encode())
            else:
                index = int(path.stem[1:])
                row = self.rows[index]
                stream = {'index': 0, 'codec_type': 'video', 'codec_name': 'h264', 'pix_fmt': 'yuv420p',
                          'profile': 'High', 'field_order': 'progressive', 'width': row['width'],
                          'height': row['height'], 'nb_read_frames': str(row['frames'] if path.suffix == '.h264'
                                                                       else row['source_frames'])}
                kwargs['stdout'].write(json.dumps({'streams': [stream]}).encode())
            return subprocess.CompletedProcess(argv, 0)
        for index, candidate in enumerate(self.candidates):
            original = self.root / 'operator/originals' / f'o{index:05}.source'
            original.write_bytes(f'synthetic-source-{index}'.encode())
            sha = driver.digest(original)
            inventory['files'][candidate['identity']] = {'size': original.stat().st_size, 'checksums': {'sha256': sha}}
            state['downloads'][str(index)] = [{'status': 'complete', 'charged_bytes': original.stat().st_size + 1,
                                               'sha256': sha}]
            counts, _ = acquisition.failure_counts(state['decisions'], self.plan)
            decision, row = acquisition.prepare(self.root, index, candidate, original, sha, state, lambda: None,
                                                 used, state['rows'], counts, self.plan, runner, lambda: 0)
            state['decisions'].append(decision)
            state['rows'].append(row)
            state['notifications'].append(f'amended:decision:{index}')
        state['downloads']['57'] = []
        driver.write(self.root / 'operator/acquisition.json', inventory)
        driver.write(self.root / 'operator/amended-state.json', state)
        driver.write(self.root / 'collection.json', state['decisions'])
        driver.write(self.root / 'roster.json', state['rows'])
        driver.write(self.root / 'acquisition-report.json', acquisition.public_report(state, self.plan))
        self.refresh_snapshot()
        driver.write(self.manifest, self.registration)
        with mock.patch.object(acquisition, 'verify_receipt', return_value={'deadline': 1000}), \
             mock.patch.object(acquisition, 'bounded_runner', side_effect=AssertionError('oracle rerun')):
            audited = acquisition.audit_collection(self.root, self.candidates, self.plan,
                                                    self.registration['acquisition_sha256'])
            self.assertEqual((audited['status'], len(audited['rows']), len(audited['captures'])),
                             ('incomplete', 57, 171))
            self.assertEqual(audited['downloads']['57'], [])
            with self.assertRaises(ValueError):
                acquisition.audit_collection(self.root, self.candidates, self.plan, 'f' * 64)

    def test_bootstrap_uses_nine_files_and_keeps_the_four_k_verdict(self):
        classes = {r['label']: r['class'] for r in self.rows}
        times = {(label, arm): {n: (n + 1) * (0.95 if arm == 'pgo' else 1)
                              for n in range(self.plan['rounds'])}
                 for label in classes for arm in ('base', 'pgo')}
        rng = mock.Mock(wraps=random.Random(self.plan['seed']))
        with mock.patch.object(driver.bars.random, 'Random', return_value=rng):
            summary = driver.bars.paired_round_summary(times, classes, 'pgo', 'base',
                                                       self.plan['rounds'], self.plan['seed'], 100)
        self.assertEqual(Counter(len(call.args[0]) for call in rng.choice.call_args_list), {16: 4800, 9: 900})
        self.assertEqual({c: r['n'] for c, r in summary['groups'].items()},
                         self.registration['counts'] | {'overall': 57})
        for cell in summary['groups'].values():
            self.assertAlmostEqual(cell['median'], 0.95)
            for bound in cell['ci95']:
                self.assertAlmostEqual(bound, 0.95)
        eight = copy.deepcopy(summary['groups'])
        eight['4k']['ci95'][1] = 1.010001
        self.assertFalse(driver.verdict(summary['groups']['overall'], eight, self.plan))


if __name__ == '__main__':
    unittest.main()
