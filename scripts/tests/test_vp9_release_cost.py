import copy
import json
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import vp9_release_cost as runner


class ReleaseCostTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        (self.root / 'mimalloc-arena-reserve-kib.txt').write_text('32768\n')
        prefixes = sorted(runner.LARGE) + [f'{n:012x}' for n in range(7)]
        self.files = [{'sha256': name + '0' * 52, 'ext': 'webm'} for name in prefixes]
        self.stack = mock.patch.multiple(runner, ROOT=self.root, OUT=self.root, FILES=self.files)
        self.stack.start()
        self.addCleanup(self.stack.stop)
        self.rows = []
        for file in self.files:
            name = file['sha256'][:12]
            for arm in runner.ARMS:
                for round in range(5):
                    for repetition in range(2):
                        self.rows.append(dict(kind='timed', file=name, arm=arm, round=round,
                                              repetition=repetition, unit=2, rc=0, frames=3,
                                              cpu=1.2 if arm == 'rel' else 1.0, wall=1.0, maxrss_kb=100,
                                              allocator_arena_reserve_kib=32768 if arm == 'alloc' else None))
            for arm in ['ff', 'rel-hash', 'alloc-hash']:
                self.rows.append(dict(kind='hash-result', file=name, arm=arm, frames=3))
            self.rows.append(dict(kind='pixel-identity', file=name, frames=3, matches=True))
            if name in runner.LARGE:
                for arm in ['stock', 'alloc']:
                    self.rows.append(dict(kind='transcode-hash', file=name, arm=arm, sha256='a' * 64))
        (self.root / 'completed.json').write_text(json.dumps(dict(files=10, arms=runner.ARMS, rounds=5)))
        (self.root / 'gate.json').write_text(json.dumps(dict(before=1.0, after=1.0, ratio=1.0)))

    def write_rows(self, rows=None):
        (self.root / 'calls.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in (self.rows if rows is None else rows)))

    def test_complete_report_and_missed_bars(self):
        self.write_rows()
        result = runner.analyse()
        self.assertTrue(result['passed'])
        self.assertEqual(result['summary']['alloc/deb']['median'], 1.0)
        self.assertEqual(result['large_median'], 1.0)
        for row in self.rows:
            if row['kind'] == 'timed' and row['arm'] == 'alloc':
                row['cpu'] = 1.1
        self.write_rows()
        result = runner.analyse()
        self.assertFalse(result['passed'])
        self.assertTrue(result['host_valid'])

    def test_missing_duplicate_and_failed_calls_are_refused(self):
        variants = [self.rows[1:], self.rows + [self.rows[0]],
                    self.rows + [dict(kind='hash', rc=1)]]
        for rows in variants:
            with self.subTest(rows=len(rows)):
                self.write_rows(rows)
                with self.assertRaises(RuntimeError):
                    runner.analyse()

    def test_hash_counts_and_pixel_identity_are_required(self):
        for kind, field, value in [('hash-result', 'frames', 4), ('pixel-identity', 'matches', False)]:
            rows = copy.deepcopy(self.rows)
            next(row for row in rows if row['kind'] == kind)[field] = value
            self.write_rows(rows)
            with self.assertRaises(RuntimeError):
                runner.analyse()

    def test_the_worker_allocator_configuration_is_required(self):
        next(row for row in self.rows if row['kind'] == 'timed' and row['arm'] == 'alloc').pop('allocator_arena_reserve_kib')
        self.write_rows()
        with self.assertRaises(RuntimeError):
            runner.analyse()

    def test_the_hash_oracle_selects_only_the_registered_video_stream(self):
        commands = []
        file = dict(self.files[0], maps=dict(video=7))

        def measured(cmd, label):
            commands.append(cmd)
            text = '0, 0, 0, 1, 1, abc\n' if label['arm'] == 'ff' else json.dumps(dict(frames=[dict(md5='abc')]))
            (self.root / 'last.stdout').write_text(text)

        with mock.patch.object(runner, 'measured', side_effect=measured):
            self.assertEqual(runner.hashes(file), 1)
        oracle = commands[-1]
        self.assertEqual(oracle[oracle.index('-map') + 1], '0:7')
        self.assertEqual(oracle.count('-map'), 1)
        self.assertEqual(oracle[oracle.index('-f') + 1], 'framemd5')

    def test_transcode_roster_and_bytes_are_required(self):
        for mutate in ['missing', 'changed']:
            rows = copy.deepcopy(self.rows)
            row = next(row for row in rows if row['kind'] == 'transcode-hash')
            if mutate == 'missing':
                rows.remove(row)
            else:
                row['sha256'] = 'b' * 64
            self.write_rows(rows)
            with self.assertRaises(RuntimeError):
                runner.analyse()

    def test_aborted_and_incomplete_runs_are_refused(self):
        self.write_rows()
        (self.root / 'aborted.json').write_text('{}')
        with self.assertRaises(RuntimeError):
            runner.analyse()
        (self.root / 'aborted.json').unlink()
        (self.root / 'completed.json').unlink()
        with self.assertRaises(FileNotFoundError):
            runner.analyse()

    def test_host_controls_make_the_run_invalid(self):
        self.write_rows()
        (self.root / 'gate.json').write_text(json.dumps(dict(before=1.0, after=1.06, ratio=1.06)))
        self.assertFalse(runner.analyse()['host_valid'])
        (self.root / 'gate.json').write_text(json.dumps(dict(before=1.0, after=1.0, ratio=1.0)))
        for row in self.rows:
            if row['kind'] == 'timed' and row['arm'] == 'ff_aa':
                row['cpu'] = 1.03
        self.write_rows()
        self.assertFalse(runner.analyse()['host_valid'])

    def test_failed_exec_and_malformed_output_are_retained(self):
        with self.assertRaises(RuntimeError):
            runner.measured(['/nonexistent/vp9-test'], dict(kind='failed-exec'))
        last = json.loads((self.root / 'calls.jsonl').read_text().splitlines()[-1])
        self.assertEqual(last['rc'], 127)
        for text in ['invalid', '{"frames": []}', '{"frames": [{}]}']:
            with mock.patch.object(runner, 'command', return_value=[sys.executable, '-c', 'print(' + repr(text) + ')']):
                with self.assertRaises((ValueError, RuntimeError)):
                    runner.decode_call('rel', self.files[0], dict(kind='bad-output'), expected=3)
            last = json.loads((self.root / 'calls.jsonl').read_text().splitlines()[-1])
            self.assertIn('error', last)
            self.assertEqual((self.root / 'last.stdout').read_text().strip(), text)

    def test_candidate_decoder_uses_the_worker_arena_configuration(self):
        row = runner.measured([sys.executable, '-c', 'import os; print(os.environ["MIMALLOC_ARENA_RESERVE"])'],
                              dict(kind='timed', arm='alloc'))
        self.assertEqual(row['allocator_arena_reserve_kib'], 32768)
        self.assertEqual((self.root / 'last.stdout').read_text().strip(), '32768')


if __name__ == '__main__':
    unittest.main()
