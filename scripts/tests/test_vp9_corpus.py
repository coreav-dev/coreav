import copy
from fnmatch import fnmatchcase
from fractions import Fraction
import importlib.util
import json
import os
import pathlib
from types import SimpleNamespace
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

PATH = pathlib.Path(__file__).resolve().parents[1] / 'vp9_corpus.py'
SPEC = importlib.util.spec_from_file_location('vp9_corpus', PATH)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class Comparisons(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.meta = dict(kind='stream', stream_index=0, profile=0, bit_depth=8,
                         subsampling=[1, 1], time_base=[1000000, 1000000000])
        self.footer = dict(kind='complete', scope='first120', packets=2, fed=2, frames=2,
                           cutoff=None, coverage=dict(coded=2, sharpness=0, segment_filter=0))
        self.mine = [self.frame(0, -4), self.frame(1, 7)]
        self.theirs = [{k: r[k] for k in ('pts', 'size', 'md5')} for r in self.mine]

    @staticmethod
    def frame(number, pts, packet=None):
        return dict(kind='frame', packet=number if packet is None else packet, frame=number,
                    pts=pts, width=3, height=3, size=17, md5=f'{number:032x}')

    def capture(self, rows=None):
        path = self.root / 'native.jsonl'
        rows = [self.meta, *self.mine, self.footer] if rows is None else rows
        path.write_text(''.join(json.dumps(r) + '\n' for r in rows))
        return path

    def judge(self, mine=None, theirs=None, clock=Fraction(1, 1000)):
        return runner.judge(self.meta, self.footer, self.mine if mine is None else mine, clock,
                            self.theirs if theirs is None else theirs)

    def test_the_control_checks_negative_timestamps_and_odd_frame_sizes(self):
        meta, footer, frames = runner.native_capture(self.capture(), 'first120')
        self.assertEqual((meta, footer, frames), (self.meta, self.footer, self.mine))
        self.assertEqual(self.judge(), 2)

    def test_missing_tail_extra_frame_wrong_size_hash_and_timestamp_fail(self):
        variants = {'missing tail': self.mine[:-1], 'extra frame': self.mine + [self.frame(2, 9)]}
        for field, value in [('pts', 8), ('size', 18), ('md5', 'f' * 32)]:
            rows = copy.deepcopy(self.mine)
            rows[-1][field] = value
            variants[field] = rows
        for name, rows in variants.items():
            with self.subTest(name), self.assertRaisesRegex(ValueError, 'differs'):
                self.judge(mine=rows)

    def test_clock_mismatch_and_empty_expected_prefix_fail(self):
        with self.assertRaisesRegex(ValueError, 'clock'):
            self.judge(clock=Fraction(1001, 30000))
        with self.assertRaisesRegex(ValueError, 'independently expected'):
            self.judge(mine=[], theirs=[])

    def test_order_and_duplicate_timestamps_are_not_normalized(self):
        self.mine = [self.frame(i, pts) for i, pts in enumerate([2, -3, 100, 2])]
        self.theirs = [{k: r[k] for k in ('pts', 'size', 'md5')} for r in self.mine]
        self.footer['cutoff'] = 90
        self.assertEqual(self.judge(), 3)
        with self.assertRaises(ValueError):
            self.judge(mine=sorted(self.mine, key=lambda r: r['pts']))

    def test_the_cutoff_is_exclusive_even_with_duplicate_times(self):
        self.footer['cutoff'] = 7
        self.assertEqual(self.judge(), 1)
        changed = copy.deepcopy(self.mine)
        changed[-1]['md5'] = 'f' * 32
        self.assertEqual(self.judge(mine=changed), 1)

    def test_eight_shown_members_per_packet_are_counted_but_nine_are_refused(self):
        self.mine = [self.frame(i, i, packet=0) for i in range(8)]
        self.footer.update(packets=1, fed=1, frames=8)
        runner.native_capture(self.capture(), 'first120')
        self.mine.append(self.frame(8, 8, packet=0))
        self.footer['frames'] = 9
        with self.assertRaisesRegex(ValueError, 'too many pictures'):
            runner.native_capture(self.capture(), 'first120')

    def test_hidden_packets_do_not_require_one_output_each(self):
        self.mine = [self.frame(0, 7, packet=119)]
        self.footer.update(packets=120, fed=120, frames=1, coverage=dict(coded=120, sharpness=1, segment_filter=1))
        runner.native_capture(self.capture(), 'first120')

    def test_missing_footer_partial_packet_work_and_false_counts_fail(self):
        variants = [[self.meta, *self.mine], [self.meta, *self.mine, dict(self.footer, fed=1)],
                    [self.meta, *self.mine, dict(self.footer, frames=1)],
                    [self.meta, *self.mine, dict(self.footer, fed=True)],
                    [self.meta, *self.mine, dict(self.footer, packets=121, fed=2, cutoff=9)]]
        for rows in variants:
            with self.subTest(rows=rows[-1]), self.assertRaises(ValueError):
                runner.native_capture(self.capture(rows), 'first120')

    def test_unknown_or_boolean_stream_configuration_is_refused(self):
        for field, value in [('profile', False), ('profile', None), ('bit_depth', None),
                             ('subsampling', [True, True]), ('time_base', [True, 1000])]:
            meta = dict(self.meta, **{field: value})
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                runner.native_capture(self.capture([meta, *self.mine, self.footer]), 'first120')

    def test_the_one_file_condition_is_sha_bound_scope_bound_and_fails_when_stale(self):
        condition = runner.read_json(runner.CONDITION)
        sha = condition['sha256']
        self.footer.update(packets=121, fed=120, cutoff=20, file_condition=condition['id'])
        runner.native_capture(self.capture(), 'first120', sha)
        wrong_clock = dict(self.meta, time_base=[2_000_000, 1_000_000_000])
        with self.assertRaisesRegex(ValueError, 'clock'):
            runner.native_capture(self.capture([wrong_clock, *self.mine, self.footer]), 'first120', sha)
        with self.assertRaisesRegex(ValueError, 'unregistered'):
            runner.native_capture(self.capture(), 'first120', sha[:-1] + ('0' if sha[-1] != '0' else '1'))
        for changed in (dict(self.footer, file_condition=None),
                        dict(self.footer, file_condition='unregistered'),
                        dict(self.footer, cutoff=condition['partial_pts'] + 1),
                        dict(self.footer, fed=119)):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                runner.native_capture(self.capture([self.meta, *self.mine, changed]), 'first120', sha)
        self.footer.update(scope='all', fed=121, cutoff=None)
        with self.assertRaises(ValueError):
            runner.native_capture(self.capture(), 'all', sha)

    def test_duplicate_json_keys_are_refused(self):
        path = self.capture()
        path.write_text(path.read_text().replace('"profile": 0', '"profile": 1, "profile": 0'))
        with self.assertRaisesRegex(ValueError, 'duplicate JSON key'):
            runner.native_capture(path, 'first120')

    def test_the_independent_oracle_bound_and_sentinel_are_checked(self):
        rows = [dict(pts=i, size=17, md5='0' * 32) for i in range(runner.ORACLE_FRAMES)]
        with self.assertRaisesRegex(ValueError, 'sentinel'):
            self.judge(theirs=rows)
        self.footer['cutoff'] = 2
        actual = [self.frame(i, i) | {'md5': '0' * 32} for i in range(2)]
        self.assertEqual(self.judge(mine=actual, theirs=rows), 2)
        rows[-1]['pts'] = 0
        with self.assertRaisesRegex(ValueError, 'sentinel'):
            self.judge(mine=actual, theirs=rows)
        with self.assertRaisesRegex(ValueError, 'frame bound'):
            self.judge(theirs=rows + [rows[-1]])

    def test_b5_has_no_oracle_frame_limit_and_requires_full_output(self):
        self.footer['scope'] = 'all'
        self.assertEqual(self.judge(), 2)
        with self.assertRaises(ValueError):
            self.judge(mine=self.mine[:-1])
        command = runner.ffmpeg_command('ffmpeg', 'input.mkv', 'all')
        self.assertNotIn('-frames:v', command)
        prefix = runner.ffmpeg_command('ffmpeg', 'input.mkv', 'first120')
        self.assertEqual(prefix[prefix.index('-frames:v') + 1], '961')
        self.assertLess(prefix.index('-copyts'), prefix.index('-i'))
        self.assertEqual(prefix[prefix.index('-enc_time_base') + 1], 'demux')
        self.assertEqual(prefix[prefix.index('-c:v') + 1], 'vp9')
        for selected in (command, prefix):
            self.assertEqual(selected.count('-map'), 1)
            self.assertEqual(selected[selected.index('-map') + 1], '0:v:0')

    def test_oracle_capture_requires_its_clock_and_md5_headers(self):
        path = self.root / 'oracle'
        control = '#hash: MD5\n#tb 0: 1/1000\n0, -4, -4, 1, 17, ' + '0' * 32 + '\n'
        path.write_text(control)
        clock, frames = runner.oracle_capture(path)
        self.assertEqual(clock, Fraction(1, 1000))
        self.assertEqual(frames, [dict(pts=-4, size=17, md5='0' * 32)])
        for text in [control.replace('#hash: MD5\n', ''), control.replace('#tb 0: 1/1000\n', ''),
                     control + '#tb 0: 1/1000\n', control.replace('17, ', '0, '),
                     control.replace('0' * 32, 'garbage')]:
            path.write_text(text)
            with self.assertRaises(ValueError):
                runner.oracle_capture(path)


class InputsAndChildren(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)

    def test_the_registered_roster_has_no_loss_or_overlap(self):
        names, b5 = runner.roster()
        self.assertEqual((len(names), len(set(names)), len(b5)), (74, 74, 10))
        control = runner.read_json(runner.ROSTER)
        for mode in ('missing', 'duplicate', 'overlap', 'unknown b5'):
            data = copy.deepcopy(control)
            if mode == 'missing':
                data['held_out'].pop()
            elif mode == 'duplicate':
                data['development'][-1] = data['development'][0]
            elif mode == 'overlap':
                data['held_out'][-1] = data['development'][0]
            else:
                data['b5'][-1] = 'f' * 64
            path = self.root / 'roster.json'
            runner.write_json(path, data)
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                runner.roster(path)

    def test_shards_partition_the_exact_registered_order(self):
        names, _ = runner.roster()
        indices = [runner.shard(f'{i}/8') for i in range(8)]
        parts = [names[i::n] for i, n in indices]
        self.assertEqual(sorted(v for part in parts for v in part), sorted(names))
        self.assertEqual(sum(map(len, parts)), 74)
        for value in ('', '1/1', '-1/2', '0/0', '0/75', '0/1/2'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                runner.shard(value)

    def test_eligibility_needs_independent_agreement_and_never_a_decode_result(self):
        for profile, depth, subsampling, pixel_format, expected in (
                (0, 8, [1, 1], 'yuv420p', True), (1, 8, [0, 0], 'yuv444p', False),
                (2, 10, [1, 1], 'yuv420p10le', False), (3, 12, [1, 0], 'yuv422p12le', False)):
            meta = dict(profile=profile, bit_depth=depth, subsampling=subsampling)
            with self.subTest(meta=meta):
                self.assertEqual(runner.eligible(meta, pixel_format), expected)
                with self.assertRaisesRegex(ValueError, 'disagree'):
                    runner.eligible(meta, 'unknown')
        for meta in (dict(profile=0, bit_depth=8, subsampling=[0, 0]),
                     dict(profile=0, bit_depth=10, subsampling=[1, 1]),
                     dict(profile=1, bit_depth=8, subsampling=[1, 1])):
            with self.subTest(meta=meta), self.assertRaisesRegex(ValueError, 'configuration'):
                runner.eligible(meta, 'yuv420p')

    def test_acquisition_requires_the_entire_roster_before_fetching_a_shard(self):
        names, _ = runner.roster()
        control = [dict(sha256=name, size=1, ext='webm', set='real', key=f'{name}.webm')
                   for name in names]
        fake = SimpleNamespace(manifest=mock.Mock(return_value=control), fetch=mock.Mock())
        args = SimpleNamespace(shard='3/8', manifests=None, dest=self.root / 'media')
        with mock.patch.dict(sys.modules, round_files=fake), mock.patch.object(runner.sys, 'platform', 'linux'):
            runner.fetch(args)
            selected = fake.fetch.call_args.args[0]
            self.assertEqual([e['sha256'] for e in selected], names[3::8])
            self.assertTrue(all(e['key'].startswith('corpus/v1/') for e in selected))
            for mode in ('missing', 'duplicate', 'extension', 'size', 'key'):
                entries = copy.deepcopy(control)
                if mode == 'missing':
                    entries.pop()
                elif mode == 'duplicate':
                    entries.append(entries[0])
                else:
                    field, value = {'extension': ('ext', 'mp4'), 'size': ('size', True),
                                    'key': ('key', '../escape')}[mode]
                    entries[0][field] = value
                fake.manifest.return_value = entries
                fake.fetch.reset_mock()
                with self.subTest(mode=mode), self.assertRaises(ValueError):
                    runner.fetch(args)
                fake.fetch.assert_not_called()

    def test_input_bytes_are_bound_before_and_after_the_decoder_calls(self):
        path = self.root / 'synthetic.bin'
        path.write_bytes(b'synthetic test bytes')
        entry = dict(sha256=runner.digest(path), path=str(path))
        args = SimpleNamespace(ffprobe='ffprobe')
        with mock.patch.object(runner, 'child') as child:
            path.write_bytes(b'changed synthetic test bytes')
            with self.assertRaisesRegex(ValueError, 'digest mismatch'):
                runner.compare(entry, args, set(), self.root)
            child.assert_not_called()
        path.write_bytes(b'synthetic test bytes')

        def replace_bytes(*_):
            path.write_bytes(b'changed during test child')
            raise ValueError('synthetic child failure')

        with mock.patch.object(runner, 'child', side_effect=replace_bytes):
            with self.assertRaisesRegex(ValueError, 'bytes changed'):
                runner.compare(entry, args, set(), self.root)

    @unittest.skipUnless(sys.platform == 'linux', 'C2 subprocess limits are Linux-only')
    def test_the_decoder_child_inherits_no_corpus_or_cloud_credentials(self):
        marker = dict(R2_ACCESS_KEY_ID='unit-only', R2_SECRET_ACCESS_KEY='unit-only',
                      COREAV_GCP_SA_KEY='unit-only', CAPY_API_KEY='unit-only')
        command = [sys.executable, '-c',
                   'import os; assert not any(k in os.environ for k in ' + repr(list(marker)) + ')']
        with mock.patch.dict(os.environ, marker):
            runner.child(command, self.root, 'environment', 5)

    @unittest.skipUnless(sys.platform == 'linux', 'C2 subprocess limits are Linux-only')
    def test_failed_and_timed_out_children_never_return_a_pass_capture(self):
        with self.assertRaisesRegex(ValueError, 'failed'):
            runner.child([sys.executable, '-c', 'raise SystemExit(2)'], self.root, 'failure', 5)
        with self.assertRaises(subprocess.TimeoutExpired):
            runner.child([sys.executable, '-c', 'import time; time.sleep(60)'], self.root, 'timeout', 1)


class Summaries(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.args = SimpleNamespace(summaries=self.root, shards=8, out=self.root / 'aggregate.json')
        names, b5 = runner.roster()
        condition_sha = runner.read_json(runner.CONDITION)['sha256']
        self.reports = []
        for index in range(8):
            count = len(names[index::8])
            whole = sum(name in b5 for name in names[index::8])
            self.reports.append(dict(complete=True, expected=count, processed=count, agree=count,
                                     outside_profile0=0, failed=0, frames=count, shard=[index, 8],
                                     truncated_tail=int(condition_sha in names[index::8]),
                                     roster_sha256=runner.digest(runner.ROSTER),
                                     b5=dict(agree=whole, outside_profile0=0, failed=0),
                                     coverage=dict(coded=count, sharpness=0, segment_filter=0)))

    def save(self, reports=None):
        for index, report in enumerate(self.reports if reports is None else reports):
            runner.write_json(self.root / f'summary-{index}.json', report)

    def test_complete_roster_and_b5_outcomes_are_aggregated_without_private_fields(self):
        self.reports[0]['private_record'] = 'must-not-escape'
        self.save()
        self.assertEqual(runner.summarize(self.args), 0)
        value = runner.read_json(self.args.out)
        self.assertEqual((value['expected'], value['processed'], value['agree'], value['frames']),
                         (74, 74, 74, 74))
        self.assertEqual(value['b5'], dict(agree=10, outside_profile0=0, failed=0))
        self.assertTrue(value['pass'])
        self.assertEqual(value['truncated_tail'], 1)
        self.assertNotIn('must-not-escape', self.args.out.read_text())

    def test_missing_duplicate_extra_stale_or_malformed_summaries_fail(self):
        control = copy.deepcopy(self.reports)
        for mode in ('missing', 'duplicate', 'extra', 'stale', 'bool count', 'false complete',
                     'missing b5', 'empty frames', 'bad coverage', 'bad expected', 'missing condition',
                     'extra condition'):
            self.reports = copy.deepcopy(control)
            for path in self.root.glob('summary-*.json'):
                path.unlink()
            if mode == 'missing':
                self.reports.pop()
            elif mode == 'duplicate':
                self.reports[-1] = self.reports[0]
            elif mode == 'extra':
                self.reports.append(self.reports[0])
            elif mode == 'stale':
                self.reports[0]['roster_sha256'] = '0' * 64
            elif mode == 'bool count':
                self.reports[0]['processed'] = True
            elif mode == 'false complete':
                self.reports[0]['processed'] -= 1
                self.reports[0]['agree'] -= 1
            elif mode == 'missing b5':
                next(r for r in self.reports if r['b5']['agree'])['b5']['agree'] -= 1
            elif mode == 'empty frames':
                self.reports[0]['frames'] = 0
            elif mode == 'bad coverage':
                self.reports[0]['coverage']['sharpness'] = 1000
            elif mode == 'missing condition':
                next(r for r in self.reports if r['truncated_tail'])['truncated_tail'] = 0
            elif mode == 'extra condition':
                next(r for r in self.reports if not r['truncated_tail'])['truncated_tail'] = 1
            else:
                self.reports[0]['expected'] += 1
            self.save()
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                runner.summarize(self.args)

    def test_aborted_and_failed_runs_keep_a_nonpassing_aggregate(self):
        self.reports[0].update(complete=False, processed=0, agree=0, frames=0)
        self.reports[0]['b5'] = dict(agree=0, outside_profile0=0, failed=0)
        self.save()
        self.assertEqual(runner.summarize(self.args), 1)
        self.assertFalse(runner.read_json(self.args.out)['pass'])
        self.reports[1]['agree'] -= 1
        self.reports[1]['failed'] += 1
        self.save()
        self.assertEqual(runner.summarize(self.args), 1)
        self.assertEqual(runner.read_json(self.args.out)['failed'], 1)


class Workflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from test_registered_candidate import load_workflow
        cls.workflow = load_workflow('vp9-corpus.yml')

    def test_every_corpus_build_enables_native_boundary_comparisons(self):
        commands = '\n'.join(step.get('run', '') for step in self.workflow['jobs']['contract']['steps'])
        for command in ('test', 'clippy', 'build'):
            lines = [line for line in commands.splitlines()
                     if line.strip().startswith(f'cargo {command} ') and '-p coreav-vp9-corpus-oracle' in line]
            self.assertEqual(len(lines), 1, command)
            self.assertIn('--features native-vp9', lines[0],
                          'enable native-vp9 so the corpus checks both full and keyframe-only boundary output')

    def test_boundary_changes_start_corpus_and_vector_comparisons(self):
        from test_registered_candidate import load_workflow
        for workflow in (self.workflow, load_workflow('vp9-conformance.yml')):
            triggers = workflow.get('on', workflow.get('true'))
            for event in ('push', 'pull_request'):
                for path in ('crates/coreav-backend-vp9/src/lib.rs', 'crates/coreav-decode/src/lib.rs',
                             'crates/coreav-frame/src/picture.rs'):
                    self.assertTrue(any(fnmatchcase(path, pattern) for pattern in triggers[event]['paths']), path)

    def test_hosted_vectors_exercise_the_boundary_without_skipping_ignored_tests(self):
        from test_registered_candidate import load_workflow
        workflow = load_workflow('vp9-conformance.yml')
        steps = workflow['jobs']['vectors']['steps']
        boundary = [step for step in steps if '-p coreav-backend-vp9 --test vectors' in step.get('run', '')]
        self.assertEqual(len(boundary), 1)
        self.assertIn('--include-ignored', boundary[0]['run'])
        self.assertIn('COREAV_VP9_VECTORS', boundary[0]['env'])

    def test_preflight_gates_acquisition_and_credentials_exist_only_in_fetch(self):
        jobs = self.workflow['jobs']
        self.assertEqual(jobs['corpus']['needs'], 'contract')
        steps = jobs['contract']['steps']
        preflight = next(i for i, s in enumerate(steps) if s.get('name', '').startswith('Preflight'))
        artifact = next(i for i, s in enumerate(steps) if 'upload-artifact@' in s.get('uses', ''))
        self.assertLess(preflight, artifact)
        self.assertIn('scripts/vp9_corpus.py preflight', steps[preflight]['run'])
        self.assertNotIn('continue-on-error', steps[preflight])
        for job in jobs.values():
            self.assertNotIn('env', job)
            for step in job['steps']:
                if 'checkout@' in step.get('uses', ''):
                    self.assertFalse(step['with']['persist-credentials'])
                if any('secrets.' in str(v) for v in step.get('env', {}).values()):
                    self.assertEqual(step['name'], 'Fetch the registered shard')
        self.assertEqual(jobs['corpus']['strategy']['matrix']['shard'], list(range(8)))

    def test_the_compiled_file_condition_starts_both_workflow_events(self):
        triggers = self.workflow.get('on', self.workflow.get('true'))
        for event in ('push', 'pull_request'):
            for path in ('docs/measurements/vp9-native/file-condition.json',
                         'tools/vp9-corpus-oracle/src/known_tail.rs'):
                with self.subTest(event=event, path=path):
                    self.assertTrue(any(fnmatchcase(path, pattern) for pattern in triggers[event]['paths']))

    def test_oracle_tool_helpers_start_both_workflow_events(self):
        triggers = self.workflow.get('on', self.workflow.get('true'))
        for event in ('push', 'pull_request'):
            for path in ('scripts/oracle_pin.py', 'scripts/oracle_from_r2.py'):
                with self.subTest(event=event, path=path):
                    self.assertTrue(any(fnmatchcase(path, pattern) for pattern in triggers[event]['paths']),
                                    f'add {path} to the {event} paths of vp9-corpus.yml')

    def test_only_sealed_evidence_and_closed_summary_paths_are_uploaded(self):
        steps = self.workflow['jobs']['corpus']['steps']
        uploads = [s['with']['path'] for s in steps if 'upload-artifact@' in s.get('uses', '')]
        self.assertEqual(uploads, ['${{ runner.temp }}/sealed',
                                  '${{ runner.temp }}/public/summary-${{ matrix.shard }}.json'])
        seal = next(s for s in steps if s.get('name') == 'Seal private records and remove media')
        self.assertEqual(seal['if'], 'always()')
        self.assertLess(seal['run'].index('rm -rf media'), seal['run'].index('tar -czf'))
        self.assertIn('rsa_padding_mode:oaep', seal['run'])
        self.assertIn('rm -rf private', seal['run'])

    def test_failed_sealing_or_a_skipped_shard_cannot_report_success(self):
        step = next(s for s in self.workflow['jobs']['corpus-summary']['steps'] if 'run' in s)
        script = step['run']
        self.assertIn('scripts/vp9_corpus.py summarize', script)
        self.assertIn('--shards 8', script)
        self.assertEqual(step['env']['CORPUS_RESULT'], '${{ needs.corpus.result }}')
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            fake = root / 'bin'
            fake.mkdir()
            executable = fake / 'python3'
            executable.write_text('#!/bin/sh\nexit 0\n')
            executable.chmod(0o755)
            for result in ('success', 'failure', 'cancelled', 'skipped', ''):
                done = subprocess.run(['bash', '-c', script], capture_output=True, timeout=10,
                                      env={**os.environ, 'PATH': str(fake) + os.pathsep + os.environ['PATH'],
                                           'CORPUS_RESULT': result, 'RUNNER_TEMP': tmp,
                                           'GITHUB_STEP_SUMMARY': str(root / 'job-summary')})
                with self.subTest(result=result):
                    self.assertEqual(done.returncode == 0, result == 'success', done.stderr)


if __name__ == '__main__':
    unittest.main()
