import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from unittest.mock import Mock

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import hevc_reference as reference
import test_oracle

ORACLE = test_oracle.Oracle(pathlib.Path('/oracle/ffmpeg'), pathlib.Path('/oracle/ffprobe'))


class Reference(unittest.TestCase):
    def test_result_acceptance_binds_the_expected_full_decoder_sha(self):
        expected = 'b4c5f4dcd9b30cc3f2e6c5f3d21fc4bfa747e82a'
        other = '4fd8132748d5910d5bd86e03a4d935f302255bca'
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / 'results.json'
            path.write_text(json.dumps(dict(candidate=expected, complete=True, files=[])))
            self.assertEqual(reference.accepted_result(path, expected)['candidate'], expected)
            for candidate, complete, requested, reason in [
                    (other, True, expected, 'result_candidate_mismatch'),
                    (expected, False, expected, 'result_incomplete'),
                    (expected, True, expected[:8], 'expected_candidate_requires_full_sha')]:
                path.write_text(json.dumps(dict(candidate=candidate, complete=complete, files=[])))
                with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                    reference.accepted_result(path, requested)

    def test_result_command_rejects_another_candidate_without_printing_numbers(self):
        expected = 'b4c5f4dcd9b30cc3f2e6c5f3d21fc4bfa747e82a'
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / 'results.json'
            path.write_text(json.dumps(dict(candidate='4fd8132748d5910d5bd86e03a4d935f302255bca', complete=True, files=[])))
            command = [sys.executable, '-I', str(SCRIPTS / 'hevc_reference.py'), 'check-result',
                       str(path), '--expected-candidate', expected]
            refused = subprocess.run(command, capture_output=True, timeout=10)
            self.assertNotEqual(refused.returncode, 0)
            self.assertEqual(refused.stdout, b'')
            self.assertIn(b'result_candidate_mismatch', refused.stderr)
            path.write_text(json.dumps(dict(candidate=expected, complete=True, files=[])))
            accepted = subprocess.run(command, capture_output=True, check=True, timeout=10)
            self.assertEqual(json.loads(accepted.stdout)['candidate'], expected)

    def test_candidate_must_be_on_a_branch_before_it_builds_or_sees_credentials(self):
        workflow = (SCRIPTS.parent / '.github/workflows/hevc-reference.yml').read_text()
        admission = workflow.index("Admit only commits on this repository's branches")
        self.assertLess(workflow.index('path: candidate'), admission)
        self.assertLess(admission, workflow.index('cargo build'))
        self.assertLess(admission, workflow.index('secrets.'))
        self.assertIn('test -n "$(git branch -r --contains "$CANDIDATE")"', workflow[admission:workflow.index('cargo build')])

    def test_diagnostic_mode_never_times_verified_files(self):
        args = type('Args', (), dict(native=pathlib.Path('/native'), oracle_bin=pathlib.Path('/oracle'), diagnose_only=True))()
        with tempfile.TemporaryDirectory() as temp:
            source = pathlib.Path(temp) / 'input'
            source.write_bytes(b'input')
            with patch.object(reference, 'verify', return_value=dict(depth=8, frames=2)), \
                    patch.object(reference, 'command') as child:
                row = reference.measure(args, ORACLE, source, 'd00')
            self.assertEqual(row['status'], 'verified')
            self.assertNotIn('calls', row)
            child.assert_not_called()

    def test_failed_checks_identify_native_and_oracle_without_raw_stderr(self):
        args = type('Args', (), dict(native=pathlib.Path('/native'), oracle_bin=pathlib.Path('/oracle')))()
        for binary, stderr, expected in [('/native', b'Unsupported("HEVC field sequence")\n', 'native'),
                                         ('/oracle/ffmpeg', b'pixel format private-marker', 'oracle')]:
            error = subprocess.CalledProcessError(1, [binary], stderr=stderr)
            with tempfile.TemporaryDirectory() as temp:
                source = pathlib.Path(temp) / 'input'
                source.write_bytes(b'input')
                with patch.object(reference, 'verify', side_effect=error):
                    row = reference.measure(args, ORACLE, source, 'd00')
                self.assertEqual(row['tool'], expected)
                self.assertNotIn('private-marker', json.dumps(row))
                if expected == 'native':
                    self.assertEqual(row['native_reason'], 'HEVC field sequence')
                else:
                    self.assertEqual(row['oracle_reason'], 'pixel_format')

    def test_acquisition_retries_transient_disconnect_with_fresh_pool(self):
        connections = [Mock(), Mock()]
        budget = object()
        with patch.object(reference.round_files, 'pool', side_effect=connections), \
                patch.object(reference.round_files, 'fetch_one', side_effect=[ConnectionResetError(), pathlib.Path('/input')]) as fetch, \
                patch.object(reference.time, 'sleep'):
            result = reference.fetch({}, pathlib.Path('/directory'), budget)
        self.assertEqual(result, pathlib.Path('/input'))
        self.assertEqual(fetch.call_count, 2)
        for connection in connections:
            self.assertIs(connection.limits, budget)
            connection.close.assert_called_once()

    def test_acquisition_does_not_retry_integrity_failure(self):
        connection = Mock()
        with patch.object(reference.round_files, 'pool', return_value=connection), \
                patch.object(reference.round_files, 'fetch_one', side_effect=ValueError('integrity')) as fetch:
            with self.assertRaisesRegex(ValueError, 'integrity'):
                reference.fetch({}, pathlib.Path('/directory'), object())
        self.assertEqual(fetch.call_count, 1)
        connection.close.assert_called_once()

    def test_media_children_do_not_inherit_credentials(self):
        with patch.dict(os.environ, {'R2_SECRET_ACCESS_KEY': 'test-secret-marker'}):
            stdout, receipt = reference.command(['/usr/bin/env'])
        self.assertNotIn(b'test-secret-marker', stdout)
        self.assertNotIn(b'R2_', stdout)
        self.assertGreater(receipt['wall'], 0)

    def test_profile_preserves_functions_below_ten_percent(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp)
            record = dict(id='c00', frames=2)
            args = type('Args', (), dict(native=pathlib.Path('/native')))()

            def child(command, **kwargs):
                if command[0] == 'valgrind':
                    (directory / 'c00.callgrind').write_text('events: Ir\nsummary: 10000\n')
                    return b'{"pictures":2,"decoded_pictures":2,"bins":42}', {}
                return b' 100 ( 1.00%) decoder.rs:coreav_hevc::decoder::read_nal\n', {}

            with patch.object(reference, 'command', side_effect=child):
                reference.profile(args, pathlib.Path('/input'), record, directory)
            self.assertEqual(record['instructions'], 10000)
            self.assertEqual(record['hotspots'][0]['percent'], 1.0)

    def test_summary_takes_per_file_medians_before_depth_median(self):
        rows = []
        for native, one, default in ((10, 2, 1), (6, 3, 2)):
            calls = [dict(arm=arm, wall=value, user=value / 2, system=value / 2)
                     for arm, value in zip(reference.ARMS, (native, one, default)) for _ in range(3)]
            rows.append(dict(status='timed', depth=8, calls=calls))
        rows.append(dict(status='not_timed', depth=10))
        summary = reference.summarize(rows)
        self.assertEqual(summary['8']['median_coreav_over_ffmpeg']['ffmpeg_one_wall'], 3.5)
        self.assertEqual(summary['8']['median_coreav_over_ffmpeg']['ffmpeg_default_cpu'], 6.5)
        self.assertIsNone(summary['10']['median_coreav_over_ffmpeg']['ffmpeg_one_wall'])

    def test_native_and_oracle_calls_must_do_expected_work(self):
        reference.check_work('coreav', b'{"pictures":3,"decoded_pictures":3,"bins":42}', 3)
        reference.check_work('ffmpeg_one', b'frame=1\nframe=3\nprogress=end\n', 3)
        for arm, data in [('coreav', b'{"pictures":2,"decoded_pictures":2,"bins":42}'),
                          ('coreav', b'{"pictures":3,"decoded_pictures":3,"bins":0}'),
                          ('ffmpeg_default', b'frame=2\nprogress=end\n')]:
            with self.subTest(arm=arm, data=data), self.assertRaises(ValueError):
                reference.check_work(arm, data, 3)

    def test_pixel_mismatch_is_not_timed(self):
        args = type('Args', (), dict(native=pathlib.Path('/native'), oracle_bin=pathlib.Path('/oracle')))()
        with tempfile.TemporaryDirectory() as temp:
            source = pathlib.Path(temp) / 'input'
            source.write_bytes(b'input')
            with patch.object(reference, 'verify', side_effect=ValueError('pixel_mismatch')), \
                    patch.object(reference, 'command') as child:
                row = reference.measure(args, ORACLE, source, 'd00')
            self.assertEqual(row['status'], 'not_timed')
            self.assertEqual(row['failure'], 'pixel_mismatch')
            self.assertNotIn('calls', row)
            child.assert_not_called()

    def test_verify_compares_bytes_hashes_and_complete_frame_count(self):
        picture = dict(type='picture', bit_depth=10, output_bytes=12, output_md5='abcd', width=2, height=2)
        summary = dict(type='summary', complete=True, output_md5='abcd')
        native = ('\n'.join(json.dumps(r) for r in (picture, summary))).encode()
        for oracle, passes in [(b'0, 0, 0, 1, 12, abcd\n', True),
                               (b'0, 0, 0, 1, 12, abce\n', False),
                               (b'0, 0, 0, 1, 10, abcd\n', False), (b'', False)]:
            oracle = b'#dimensions 0: 2x2\n' + oracle
            metadata = json.dumps(dict(streams=[dict(pix_fmt='yuv420p10le', width=2, height=2)])).encode()
            with self.subTest(oracle=oracle), patch.object(reference, 'command', side_effect=[(native, {}), (metadata, {}), (oracle, {})]):
                if passes:
                    self.assertEqual(reference.verify('/native', ORACLE, '/input')['frames'], 1)
                else:
                    with self.assertRaisesRegex(ValueError, 'pixel_mismatch'):
                        reference.verify('/native', ORACLE, '/input')

    def test_verification_forbids_oracle_pixel_conversion(self):
        picture = dict(type='picture', bit_depth=10, output_bytes=12, output_md5='abcd', width=2, height=2)
        native = ('\n'.join(json.dumps(r) for r in
            (picture, dict(type='summary', complete=True, output_md5='abcd')))).encode()
        with patch.object(reference, 'command', side_effect=[(native, {}),
                (b'{"streams":[{"pix_fmt":"yuv420p10le","width":2,"height":2}]}', {}),
                (b'#dimensions 0: 2x2\n0, 0, 0, 1, 12, abcd\n', {})]) as child:
            reference.verify('/native', ORACLE, '/input')
        self.assertIn('+yuv420p10le', child.call_args_list[2].args[0])

    def test_full_range_uses_unchanged_oracle_format_and_wrong_depth_is_refused(self):
        picture = dict(type='picture', bit_depth=8, output_bytes=6, output_md5='abcd', width=2, height=2)
        native = ('\n'.join(json.dumps(r) for r in
            (picture, dict(type='summary', complete=True, output_md5='abcd')))).encode()
        for pixel_format, passes in [('yuvj420p', True), ('yuv420p10le', False), ('yuv444p', False)]:
            metadata = json.dumps(dict(streams=[dict(pix_fmt=pixel_format, width=2, height=2)])).encode()
            with self.subTest(pixel_format=pixel_format), patch.object(reference, 'command', side_effect=[
                    (native, {}), (metadata, {}), (b'#dimensions 0: 2x2\n0, 0, 0, 1, 6, abcd\n', {})]) as child:
                if passes:
                    result = reference.verify('/native', ORACLE, '/input')
                    self.assertEqual(result['oracle_pixel_format'], 'yuvj420p')
                    self.assertIn('+yuvj420p', child.call_args_list[2].args[0])
                else:
                    with self.assertRaisesRegex(ValueError, 'oracle_pixel_format'):
                        reference.verify('/native', ORACLE, '/input')
                    self.assertEqual(child.call_count, 2)

    def test_oracle_arms_select_hevc_with_correct_thread_scope(self):
        one = reference.ffmpeg_command('ffmpeg', 'input', True, ['-f', 'null', '-'])
        default = reference.ffmpeg_command('ffmpeg', 'input', False, ['-f', 'null', '-'])
        self.assertLess(one.index('-threads'), one.index('-i'))
        self.assertEqual(one[one.index('-c:v') + 1], 'hevc')
        self.assertNotIn('-threads', default)
        self.assertIn('-noautoscale', one)
        self.assertIn('-noautorotate', one)


class QualifiedPair(unittest.TestCase):
    OWNER = test_oracle.Oracle(pathlib.Path('/owner/ffmpeg'), pathlib.Path('/probe-owner/ffprobe'))
    CANDIDATE = 'b4c5f4dcd9b30cc3f2e6c5f3d21fc4bfa747e82a'

    def test_reference_qualifies_its_pair_before_fetching_inputs(self):
        argv = ['hevc_reference.py', '--native', '/native', '--oracle-bin', '/requested', '--out', '/unused',
                '--candidate', self.CANDIDATE]
        with patch.object(sys, 'argv', argv), \
                patch.object(reference.test_oracle, 'require',
                             side_effect=test_oracle.QualificationError('pin mismatch')) as require, \
                patch.object(reference.round_files, 'pool') as pool, patch.object(reference, 'command') as child:
            with self.assertRaisesRegex(test_oracle.QualificationError, 'pin mismatch'):
                reference.main()
        require.assert_called_once_with(pathlib.Path('/requested/ffmpeg'), pathlib.Path('/requested/ffprobe'))
        pool.assert_not_called()
        child.assert_not_called()

    def test_verification_probes_and_decodes_with_the_returned_pair(self):
        picture = dict(type='picture', bit_depth=8, output_bytes=6, output_md5='abcd', width=2, height=2)
        native = ('\n'.join(json.dumps(r) for r in
            (picture, dict(type='summary', complete=True, output_md5='abcd')))).encode()
        with patch.object(reference, 'command', side_effect=[(native, {}),
                (b'{"streams":[{"pix_fmt":"yuv420p","width":2,"height":2}]}', {}),
                (b'#dimensions 0: 2x2\n0, 0, 0, 1, 6, abcd\n', {})]) as child:
            reference.verify('/native', self.OWNER, '/input')
        self.assertEqual(child.call_args_list[1].args[0][0], self.OWNER.ffprobe)
        self.assertEqual(child.call_args_list[2].args[0][0], self.OWNER.ffmpeg)

    def test_timed_arms_run_the_returned_ffmpeg(self):
        args = type('Args', (), dict(native=pathlib.Path('/native'), repeats=3))()
        oracle_calls = []

        def child(command, **kwargs):
            if command[0] == args.native:
                return b'{"pictures":1,"decoded_pictures":1,"bins":1}', dict(wall=1, user=0, system=0)
            oracle_calls.append(command[0])
            return b'frame=1\nprogress=end\n', dict(wall=1, user=0, system=0)

        with tempfile.TemporaryDirectory() as temp:
            source = pathlib.Path(temp) / 'input'
            source.write_bytes(b'input')
            with patch.object(reference, 'verify', return_value=dict(depth=8, frames=1)) as verify, \
                    patch.object(reference, 'command', side_effect=child):
                row = reference.measure(args, self.OWNER, source, 'd00')
        verify.assert_called_once_with(args.native, self.OWNER, source)
        self.assertEqual(row['status'], 'timed')
        self.assertEqual(len(oracle_calls), 8)
        self.assertEqual(set(oracle_calls), {self.OWNER.ffmpeg})

    def test_input_preparation_runs_the_returned_ffmpeg(self):
        item = dict(partition='development', sha256='aa', size=5, ext='mp4')
        pool = Mock()
        pool.head.return_value = (10, 'etag')
        pool.get.return_value = json.dumps(dict(files=[dict(item, key='input.mp4')])).encode()
        args = type('Args', (), dict(only={'d00'}, frames=4))()
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp)
            with patch.object(reference.conformance, 'roster', return_value=[item]), \
                    patch.object(reference.conformance, 'registration', return_value=dict(archives=[])), \
                    patch.object(reference, 'fetch', return_value=directory / 'original'), \
                    patch.object(reference, 'command') as child:
                prepared = list(reference.inputs(args, self.OWNER, directory, pool))
        self.assertEqual([identity for identity, _, _ in prepared], ['d00'])
        child.assert_called_once()
        self.assertEqual(child.call_args.args[0][0], self.OWNER.ffmpeg)


if __name__ == '__main__':
    unittest.main()
