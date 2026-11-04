"""Synthetic controls only. No registered command or judged fixture is executed."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import decoded_output
import github_shapes as run
import github_shapes_inputs as inputs
import github_shapes_set as roster


def shape(tool='ffprobe', options=()):
    return {'tool': tool, 'options': list(options),
            'inputs': [{'container': 'unknown', 'container_hint': 'unknown', 'options': []}],
            'outputs': [] if tool == 'ffprobe' else [
                {'container': 'unknown', 'container_hint': 'unknown', 'options': []}]}


def row(number=1, weight=1):
    return {'rank': number, 'repository_count': weight, 'shape_id': str(number),
            'shape': shape(options=[['-show_entries', 'format=control_sentinel']])}


class SetAndInputs(unittest.TestCase):
    def test_frozen_counts_only_roster(self):
        rows = roster.load()
        self.assertEqual(sum(r['repository_count'] for r in rows), 1470)
        self.assertEqual(set(rows[0]), {'rank', 'shape_id', 'repository_count', 'shape'})
        self.assertNotIn('https://', roster.encoded(rows).decode())
        with self.assertRaises(ValueError):
            roster.validate(rows[:-1])
        changed = json.loads(json.dumps(rows))
        changed[0]['repository_count'] += 1
        with self.assertRaises(ValueError):
            roster.validate(changed)

    def test_all_rows_have_pure_representative_or_closed_miss(self):
        results = []
        for entry in roster.load():
            try:
                command = inputs.instantiate(entry['shape'])
                self.assertNotIn('<value>', command.args)
                results.append('representative')
            except inputs.Uninstantiable as error:
                self.assertIn(str(error), ('lost_value', 'lost_filter', 'lost_construction',
                                          'external_input', 'missing_value'))
                results.append('uninstantiable')
        self.assertEqual(len(results), 200)

    def test_order_selectors_and_input_options_stay_in_their_groups(self):
        value = shape('ffmpeg')
        value['inputs'][0]['options'] = [['-ss', '<value>']]
        value['outputs'][0]['options'] = [['-map', 'N:v:N'], ['-map', 'N:a'],
                                         ['-c:v', 'copy'], ['-t', '<value>']]
        command = inputs.instantiate(value)
        self.assertEqual(command.args, ('-ss', '0.2', '-i', 'in0/source.mp4', '-map', '0:v:0',
                                        '-map', '0:a', '-c:v', 'copy', '-t', '1', 'output0.mp4'))

    def test_no_guess_for_wrapper_graph_unknown_value_or_device(self):
        for option in ('-wrapper-unresolved', '-filter_complex', '-lavfi'):
            with self.subTest(option=option), self.assertRaises(inputs.Uninstantiable):
                inputs.instantiate(shape(options=[(option, 'overlay')]))
        for value in ('<codec>', '<selector>', '<enum>'):
            with self.assertRaises(inputs.Uninstantiable):
                inputs.options([('-c:v', value)])
        value = shape()
        value['inputs'][0]['container'] = 'device'
        with self.assertRaises(inputs.Uninstantiable):
            inputs.instantiate(value)

    def test_pipe_raw_and_multiple_output_roster(self):
        value = shape('ffmpeg')
        value['inputs'][0].update(container='pipe', options=[['-f', 's16le'], ['-ar', '<value>']])
        value['outputs'].append({'container': 'pipe', 'container_hint': 'unknown', 'options': []})
        command = inputs.instantiate(value)
        self.assertEqual(command.stdin, 'in0/source.s16')
        self.assertEqual(command.outputs, ('output0.mp4', '-'))
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            for name in ('image-10.png', 'image-02.png'):
                (folder / name).touch()
            self.assertEqual([p.name for p in inputs.output_files(folder, 'image-%02d.png')],
                             ['image-02.png', 'image-10.png'])
            (folder / 'image-2.png').touch()
            with self.assertRaises(ValueError):
                inputs.output_files(folder, 'image-%02d.png')


class AcquisitionAndReplay(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bundle = self.root / 'bundle'
        self.bundle.mkdir()
        fixture = self.bundle / 'fixtures'
        fixture.mkdir()
        (fixture / 'source.mp4').write_bytes(b'synthetic control, not media')
        self.fake = self.root / 'fake'
        self.fake.write_text('#!' + sys.executable + '\nprint("control result")\n')
        self.fake.chmod(0o755)
        self.programs = {key: str(self.fake) for key in ('$COREAV', '$FFMPEG', '$FFPROBE', '$X264')}
        self.record = run.acquire_one(row(), self.bundle, self.programs, run.environment())

    def judge(self, record=None):
        return run.judge(row(), record or self.record, self.bundle, self.programs,
                         run.environment(), self.root)

    def test_actual_fake_probe_capture_and_rejudge(self):
        self.assertEqual(self.judge(), 'counted')
        self.record['outcome'] = 'counted'
        candidate = self.bundle / 'calls/1/candidate'
        (candidate / 'stdout').write_bytes(b'wrong value\n')
        (candidate / 'command.stdout').write_bytes(b'wrong value\n')
        self.assertEqual(self.judge(), 'different')

    def test_refusal_timeout_and_error_are_misses(self):
        candidate = self.record['arms']['candidate']
        candidate['code'] = 4
        self.assertEqual(self.judge(), 'refused')
        candidate['timed_out'] = True
        self.assertEqual(self.judge(), 'timeout')
        candidate.update(code=1, timed_out=False)
        self.assertEqual(self.judge(), 'failed')

    def test_missing_and_executed_fallback_void_capture(self):
        log = self.bundle / 'calls/1/candidate/fallback.jsonl'
        for text in ('not json', '{}', '{"ran":"ffmpeg"}'):
            log.write_text(text)
            with self.assertRaises((ValueError, json.JSONDecodeError)):
                self.judge()
        log.unlink()
        with self.assertRaises(OSError):
            self.judge()

    def test_substituted_command_input_or_stdout_is_rejected(self):
        self.record['arms']['candidate']['args'][0] = '/bin/true'
        with self.assertRaisesRegex(ValueError, 'invocation_binding'):
            self.judge()
        self.record['arms']['candidate']['args'][0] = '$COREAV'
        source = self.bundle / 'calls/1/candidate/in0/source.mp4'
        source.write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'input_binding'):
            self.judge()

    def test_unexpected_encoder_does_not_count(self):
        path = self.bundle / 'calls/1/candidate/starts.json'
        run.write(path, [{'program': 'x264', 'args': []}])
        self.assertEqual(self.judge(), 'unexpected_program')

    def test_reference_failure_keeps_the_row(self):
        ref = self.record['arms']['reference']
        ref['code'] = 1
        self.record['arms'].pop('candidate')
        self.assertEqual(self.judge(), 'reference_failed')

    def test_replay_checks_inventory_and_recomputes_instead_of_trusting_labels(self):
        opts = types.SimpleNamespace(bundle=self.bundle, scratch=self.root / 'scratch',
                                     summary=self.root / 'summary.json', **{
                                         k: str(self.fake) for k in ('coreav', 'ffmpeg', 'ffprobe', 'x264')})
        identity = {'coreav': {'sha256': run.digest(self.fake)}}
        build = {'commit': 'a' * 40, 'command': run.BUILD, 'coreav_sha256': run.digest(self.fake),
                 'lock_sha256': run.digest(run.ROOT / 'Cargo.lock')}
        self.record['outcome'] = 'different'
        rows = [row(i, 1271 if i == 2 else 1) for i in range(1, 201)]
        records = [self.record]
        for entry in rows[1:]:
            entry['shape']['options'].append(['-wrapper-unresolved', '<value>'])
            records.append({'rank': entry['rank'], 'uninstantiable': 'lost_construction'})
        run.write(self.bundle / 'records.json', records)
        manifest = {'method': {}, 'programs': identity, 'features': run.FEATURES,
                    'build_command': run.BUILD, 'build': build, 'commit': 'a' * 40,
                    'inventory': run.inventory(self.bundle)}
        run.write(self.bundle / 'manifest.json', manifest)
        with mock.patch.object(run, 'method', return_value={}), \
             mock.patch.object(run, 'verify_programs', return_value=identity), \
             mock.patch.object(roster, 'load', return_value=rows):
            self.assertEqual(run.replay(opts), 0)
            summary = json.loads(opts.summary.read_text())
            self.assertEqual(summary['counted'], 1)
            self.fake.write_text('changed executable')
            with mock.patch.object(run, 'verify_programs', return_value={'changed': {}}), \
                 self.assertRaisesRegex(ValueError, 'program_identity'):
                run.replay(opts)
            (self.bundle / 'calls/1/candidate/stdout').write_bytes(b'mutation')
            with self.assertRaisesRegex(ValueError, 'evidence_identity'):
                run.replay(opts)

    def test_manifest_rejects_symlinks(self):
        (self.bundle / 'escape').symlink_to(self.fake)
        with self.assertRaisesRegex(ValueError, 'evidence_file_type'):
            run.inventory(self.bundle)


class ExactPredicate(unittest.TestCase):
    def test_raw_output_description_uses_output_options_not_input_demuxer(self):
        value = shape('ffmpeg')
        value['inputs'][0]['options'] = [['-f', 's16le']]
        value['outputs'][0]['options'] = [['-f', 'mp3']]
        self.assertEqual(run.raw_description(inputs.instantiate(value), 0, None), [])
        value['outputs'][0]['options'] = [['-f', 'f32le'], ['-ar', '<value>'], ['-ac', '<value>']]
        self.assertEqual(run.raw_description(inputs.instantiate(value), 0, None),
                         ['-f', 'f32le', '-sample_rate', '48000', '-ch_layout', 'mono'])
        value['outputs'][0]['options'] = [['-f', 'f32le']]
        with self.assertRaises(ValueError):
            run.raw_description(inputs.instantiate(value), 0, None)

    def test_lossy_is_exact_not_quality_and_does_not_change_default_eligibility(self):
        stream = {'index': 0, 'codec_type': 'video', 'codec_name': 'h264', 'width': 1, 'height': 1,
                  'pix_fmt': 'yuv420p', 'time_base': '1/10', 'start_pts': 0, 'duration_ts': 1}
        frame = {'stream_index': 0, 'width': 1, 'height': 1, 'pix_fmt': 'yuv420p',
                 'best_effort_timestamp': 0, 'duration': 1}
        document = {'streams': [stream], 'frames': [frame]}
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(decoded_output, 'probe', return_value=document):
            folder = Path(tmp)
            def invoke(argv, env, output=None):
                Path(output).write_bytes(b'pixel')
                return 0, b'', b''
            with mock.patch.object(decoded_output, 'invoke', side_effect=invoke):
                args = ('candidate', 'reference', 'ffmpeg', 'ffprobe', {}, folder)
                self.assertEqual(decoded_output.compare(*args)[0], 'unverified')
                self.assertEqual(decoded_output.compare(*args, registered_codecs=run.EXACT_CODECS)[0], 'verified')
            def changed(argv, env, output=None):
                Path(output).write_bytes(b'one' if 'candidate' in argv else b'two')
                return 0, b'', b''
            with mock.patch.object(decoded_output, 'invoke', side_effect=changed):
                self.assertEqual(decoded_output.compare(*args, registered_codecs=run.EXACT_CODECS),
                                 ('failed', 'decoded pixels'))

    def test_copy_needs_packets_even_when_decoded_output_agrees(self):
        command = inputs.instantiate(shape('ffmpeg'))
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for arm in ('reference', 'candidate'):
                folder = base / arm
                folder.mkdir()
                (folder / 'output0.mp4').write_bytes(b'control')
                (folder / 'command.stderr').write_text('Stream #0:4 -> #0:0 (copy)\n')
            description = {'streams': [{'codec_name': 'h264'}]}
            programs = {'$FFMPEG': 'ffmpeg', '$FFPROBE': 'ffprobe'}
            with mock.patch.object(decoded_output, 'probe', return_value=description), \
                 mock.patch.object(decoded_output, 'compare', return_value=('verified', 'exact')), \
                 mock.patch.object(run, 'packet_rows', side_effect=[[(0, 'one')], [(0, 'two')]]):
                self.assertEqual(run.compare_outputs(command, base, programs, {}, base), 'different')

    def test_packet_timing_uses_output_stream_and_rational_clocks(self):
        doc = {'streams': [{'index': 1, 'time_base': '1/10'}],
               'packets': [{'stream_index': 1, 'data_hash': 'SHA256:test', 'pts': 3, 'dts': 2, 'duration': 1}]}
        with mock.patch.object(decoded_output, 'invoke', return_value=(0, json.dumps(doc).encode(), b'')):
            self.assertEqual(run.packet_rows('output', 'probe', {}, ()), [(1, 'SHA256:test', '3/10', '1/5', '1/10')])
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / 'command.stderr').write_text('Stream #0:7 -> #1:0 (copy)\nStream #0:1 -> #0:2 (copy)\n')
            self.assertEqual(run.jobs(folder), {(1, 0): 'copy', (0, 2): 'copy'})


class AccountingAndGate(unittest.TestCase):
    def test_all_200_stay_and_weighted_share_has_fixed_denominator(self):
        rows = [row(i, 1271 if i == 1 else 1) for i in range(1, 201)]
        outcomes = [{'rank': r['rank'], 'outcome': 'counted' if r['rank'] == 1 else 'no_oracle'} for r in rows]
        summary = run.summarize(rows, outcomes)
        self.assertEqual((summary['counted'], summary['counted_pairs']), (1, 1271))
        self.assertEqual(summary['top_200_pair_share'], 1271 / 1470)
        self.assertIsNone(summary['bar'])
        for bad in (outcomes[:-1], outcomes + outcomes[:1], list(reversed(outcomes))):
            with self.assertRaises(ValueError):
                run.summarize(rows, bad)

    def test_no_generation_before_main_membership_and_clean_source(self):
        with mock.patch.dict(os.environ, {'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_REF': 'refs/heads/main'}), \
             self.assertRaisesRegex(ValueError, 'main_dispatch_required'):
            run.merged_source('a' * 40)
        with mock.patch.dict(os.environ, {'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REF': 'refs/heads/main'}):
            good = subprocess.CompletedProcess([], 0, 'a' * 40 + '\n', '')
            clean = subprocess.CompletedProcess([], 0, '', '')
            missing = subprocess.CompletedProcess([], 1, '', '')
            with mock.patch.object(run, 'git', side_effect=[good, clean, missing]), \
                 self.assertRaisesRegex(ValueError, 'candidate_not_merged'):
                run.merged_source('a' * 40)
            with mock.patch.object(run, 'git', side_effect=[good, clean, clean]):
                run.merged_source('a' * 40)

    def test_finite_process_deadline_and_binary_stdout(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / 'input').write_bytes(b'\x00\xff')
            receipt = run.execute([sys.executable, '-c', 'import sys;sys.stdout.buffer.write(sys.stdin.buffer.read())'],
                                  folder, run.environment(), 10, 'control', 'input')
            self.assertEqual(receipt['code'], 0)
            self.assertEqual((folder / 'control.stdout').read_bytes(), b'\x00\xff')
            receipt = run.execute([sys.executable, '-c', 'import time;time.sleep(10)'],
                                  folder, run.environment(), 0.01, 'timeout')
            self.assertTrue(receipt['timed_out'])
            self.assertNotEqual(receipt['code'], 0)

    def test_workflow_is_manual_only_and_uses_fixed_build(self):
        text = (run.ROOT / '.github/workflows/github-shapes.yml').read_text()
        self.assertIn('workflow_dispatch:', text)
        self.assertNotIn('pull_request:', text)
        self.assertIn(' '.join(run.BUILD), text)
        self.assertLess(text.index('candidate_not_merged'), text.index('cargo build'))


if __name__ == '__main__':
    unittest.main()
