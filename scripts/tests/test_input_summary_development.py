import importlib.util
import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import MagicMock, patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('input_summary_development', ROOT / 'scripts/input_summary_development.py')
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


def row(identity='a' * 64, **changes):
    value = {'sha256': identity, 'ext': 'mp4', 'size': 123, 'key': 'real/a.mp4', 'set': 'real', 'holdout': False}
    value.update(changes)
    return value


class DevelopmentAdmission(unittest.TestCase):
    def test_preparation_failure_replaces_prior_complete_evidence(self):
        marker = 'private-preparation-marker'
        for diagnostics, manifests, stage, reason in (
            ([marker], [[row()], []], 'oracle-pin', 'verification'),
            ([], SystemExit(marker), 'preparation', 'validation'),
            ([], [[], []], 'selection', 'empty'),
        ):
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                candidate = root / 'coreav'
                candidate.write_bytes(b'generated-test-binary')
                evidence = root / 'evidence'
                evidence.mkdir()
                (evidence / 'summary.json').write_text('{"complete": true}')
                (evidence / 'details.p7m').write_bytes(b'old-ciphertext')
                arguments = ['checker', '--coreav', str(candidate), '--oracle-dir', str(root),
                             '--directory', str(evidence), '--evidence-cert', str(root / 'recipient.pem')]
                captured = {}
                def encrypt(command, **options):
                    captured.update(json.loads(Path(command[command.index('-in') + 1]).read_text()))
                    self.assertFalse((evidence / 'summary.json').exists())
                    self.assertFalse((evidence / 'details.p7m').exists())
                    Path(command[command.index('-out') + 1]).write_bytes(b'new-ciphertext')
                    return subprocess.CompletedProcess(command, 0, b'', b'')
                stdout, stderr = io.StringIO(), io.StringIO()
                with patch.object(CHECK.sys, 'argv', arguments), patch.object(CHECK.sys, 'platform', 'linux'), \
                        patch.object(CHECK.oracle_pin, 'verify_bin', return_value=diagnostics), \
                        patch.object(CHECK.round_files, 'manifest', side_effect=manifests), \
                        patch.object(CHECK.round_files, 'pool') as pool, \
                        patch.object(CHECK.subprocess, 'run', side_effect=encrypt), \
                        contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    self.assertEqual(CHECK.main(), 2)
                report = json.loads((evidence / 'summary.json').read_text())
                self.assertFalse(report['complete'])
                self.assertTrue(report['evidence_encrypted'])
                self.assertEqual(report['failures'], [{'stage': stage, 'reason': reason, 'files': 1}])
                self.assertEqual((evidence / 'details.p7m').read_bytes(), b'new-ciphertext')
                self.assertEqual(captured['details'][0]['failure'], {'stage': stage, 'reason': reason})
                self.assertNotIn(marker, json.dumps(report) + json.dumps(captured) + stdout.getvalue() + stderr.getvalue())
                pool.assert_not_called()

    def test_cancellation_after_reference_retains_the_current_half_pair(self):
        prefix = b"Input #0, matroska,webm, from 'file':\n"
        marker = b'private-reference-stdout'
        for pair, (tool, arguments) in enumerate((('ffprobe', []), ('ffprobe', ['-i']), ('ffmpeg', ['-i']))):
            with self.subTest(pair=pair):
                stop = threading.Event()
                count = 0
                def execute(command, **options):
                    nonlocal count
                    count += 1
                    current = count == 2 * pair + 1
                    if current:
                        stop.set()
                    return subprocess.CompletedProcess(command, int(count > 4), marker if current else b'', prefix)
                with patch.object(CHECK, 'execute', side_effect=execute) as child, self.assertRaises(CHECK.CaptureFailure) as caught:
                    CHECK.compare(Path('native'), Path('oracle'), Path('media'), stop=stop)
                self.assertEqual(child.call_count, 2 * pair + 1)
                error = caught.exception
                self.assertEqual((error.stage, error.reason), ('capture', 'cancelled'))
                self.assertEqual(len(error.details), pair + 1)
                self.assertTrue(all(item['outcome'] == 'identical' for item in error.details[:-1]))
                current = error.details[-1]
                self.assertEqual((current['tool'], current['arguments']), (tool, arguments))
                self.assertEqual(current['reference_status'], int(pair == 2))
                self.assertEqual(current['reference_stdout'], marker.decode())
                self.assertEqual(current['reference_stderr'], prefix.decode())
                self.assertNotIn('candidate_status', current)

    def test_candidate_failure_retains_completed_reference_stdout(self):
        results = [subprocess.CompletedProcess([], 0, b'private-stdout', b'private-stderr'), OSError('private-error')]
        with patch.object(CHECK, 'execute', side_effect=results), self.assertRaises(CHECK.CaptureFailure) as caught:
            CHECK.compare(Path('native'), Path('oracle'), Path('media'))
        current = caught.exception.details[-1]
        self.assertEqual(current['reference_stdout'], 'private-stdout')
        self.assertEqual(current['reference_stderr'], 'private-stderr')
        self.assertEqual(current['failure'], {'stage': 'candidate', 'reason': 'io-error'})
        self.assertNotIn('private-error', str(caught.exception) + json.dumps(current))

    def run_worker_case(self, fetch, rows=None, encryption_error=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / 'coreav'
            candidate.write_bytes(b'generated-test-binary')
            evidence = root / 'evidence'
            arguments = ['checker', '--coreav', str(candidate), '--oracle-dir', str(root),
                         '--directory', str(evidence), '--evidence-cert', str(root / 'recipient.pem')]
            captured = {}
            def encrypt(command, **options):
                captured.update(json.loads(Path(command[command.index('-in') + 1]).read_text()))
                Path(command[command.index('-out') + 1]).write_bytes(b'partial-ciphertext')
                if encryption_error is not None:
                    raise encryption_error
                return subprocess.CompletedProcess(command, 0, b'', b'')
            stdout, stderr = io.StringIO(), io.StringIO()
            with patch.object(CHECK.sys, 'argv', arguments), patch.object(CHECK.sys, 'platform', 'linux'), \
                    patch.object(CHECK.oracle_pin, 'verify_bin', return_value=[]), \
                    patch.object(CHECK.round_files, 'manifest', side_effect=[rows or [row()], []]), \
                    patch.object(CHECK.round_files, 'pool', return_value=MagicMock()), \
                    patch.object(CHECK.round_files, 'fetch_one', side_effect=fetch), \
                    patch.object(CHECK, 'compare', return_value=('identical', [])), \
                    patch.object(CHECK.subprocess, 'run', side_effect=encrypt), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = CHECK.main()
            return (code, json.loads((evidence / 'summary.json').read_text()), captured,
                    stdout.getvalue() + stderr.getvalue(), (evidence / 'details.p7m').exists())

    def test_validation_exit_is_retained_without_printing_its_message(self):
        marker = 'private-fetch-validation-marker'
        code, report, private, public, encrypted = self.run_worker_case(SystemExit(marker))
        self.assertEqual(code, 2)
        self.assertFalse(report['complete'])
        self.assertEqual(report['failures'], [{'stage': 'fetch', 'reason': 'validation', 'files': 1}])
        self.assertEqual(private['details'][0]['identity'], 'a' * 64)
        self.assertTrue(encrypted)
        self.assertNotIn(marker, public + json.dumps(private))

    def test_complete_requires_every_input_and_encrypted_evidence(self):
        code, report, _, _, encrypted = self.run_worker_case(
            lambda _, entry, destination: destination / (entry['sha256'] + '.mp4'),
            [row(c * 64) for c in 'abc'])
        self.assertEqual(code, 0)
        self.assertTrue(report['complete'])
        self.assertTrue(report['evidence_encrypted'])
        self.assertTrue(encrypted)
        self.assertEqual(report['processed'], report['inputs'])
        self.assertEqual(report['failed'] + report['cancelled'] + report['unprocessed'], 0)
        self.assertEqual(report['counts'], [{'family': 'isobmff', 'outcome': 'identical', 'files': 3}])

    def test_encryption_timeout_never_publishes_partial_ciphertext_or_a_pass(self):
        marker = 'private-encryption-marker'
        error = subprocess.TimeoutExpired([marker], 30, stderr=marker.encode())
        code, report, _, public, encrypted = self.run_worker_case(
            lambda _, entry, destination: destination / (entry['sha256'] + '.mp4'),
            encryption_error=error)
        self.assertEqual(code, 2)
        self.assertFalse(report['complete'])
        self.assertFalse(report['evidence_encrypted'])
        self.assertFalse(encrypted)
        self.assertNotIn(marker, public)
        self.assertEqual(report['failures'], [{'stage': 'evidence-encryption', 'reason': 'timeout', 'files': 1}])

    def test_later_worker_failure_stops_bounded_submission_before_earlier_work_finishes(self):
        entered, release = threading.Event(), threading.Event()
        fetched = []
        executor = CHECK.concurrent.futures.ThreadPoolExecutor
        class ReleaseOnFailure(executor):
            def submit(self, *args, **kwargs):
                future = super().submit(*args, **kwargs)
                def finished(result):
                    if not result.cancelled() and result.result()[1] == 'failed':
                        release.set()
                future.add_done_callback(finished)
                return future
        def fetch(_, entry, destination):
            fetched.append(entry['sha256'])
            if entry['sha256'] == 'a' * 64:
                entered.set()
                self.assertTrue(release.wait(5))
            elif entry['sha256'] == 'b' * 64:
                self.assertTrue(entered.wait(5))
                raise OSError('private-transfer-marker')
            else:
                self.assertTrue(release.wait(5))
            return destination / (entry['sha256'] + '.mp4')
        with patch.object(CHECK.concurrent.futures, 'ThreadPoolExecutor', ReleaseOnFailure):
            code, report, _, public, encrypted = self.run_worker_case(
                fetch, [row(c * 64) for c in 'abcdefghij'])
        self.assertEqual(code, 2)
        self.assertLessEqual(len(fetched), 4)
        self.assertGreaterEqual(report['unprocessed'], 6)
        self.assertEqual(report['processed'] + report['failed'] + report['cancelled'] + report['unprocessed'], 10)
        self.assertEqual(report['failed'], 1)
        self.assertFalse(report['complete'])
        self.assertTrue(encrypted)
        self.assertNotIn('private-transfer-marker', public)

    def test_child_timeout_keeps_prior_calls_and_partial_output_private(self):
        marker = 'private-child-marker'
        prefix = b"Input #0, matroska,webm, from 'file':\n  Stream #0:0: Audio: pcm_s16le\n"
        results = [subprocess.CompletedProcess([], 0, b'', prefix)] * 4
        results.extend([subprocess.CompletedProcess([], 1, b'', prefix),
                        subprocess.TimeoutExpired([marker], 30, stderr=marker.encode())])
        with patch.object(CHECK, 'execute', side_effect=results), self.assertRaises(CHECK.CaptureFailure) as caught:
            CHECK.compare(Path('native'), Path('oracle'), Path('media'))
        error = caught.exception
        self.assertEqual((error.stage, error.reason), ('candidate', 'timeout'))
        self.assertEqual(len(error.details), 3)
        self.assertEqual(error.details[0]['outcome'], 'identical')
        self.assertEqual(error.details[-1]['candidate_stderr'], marker)
        self.assertNotIn(marker, str(error))

    def test_capture_abort_preserves_incomplete_counts_and_encrypted_details(self):
        marker = 'private-capture-marker'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / 'coreav'
            candidate.write_bytes(b'generated-test-binary')
            evidence = root / 'evidence'
            arguments = ['checker', '--coreav', str(candidate), '--oracle-dir', str(root),
                         '--directory', str(evidence), '--evidence-cert', str(root / 'recipient.pem')]
            captured = {}
            def encrypt(command, **options):
                captured.update(json.loads(Path(command[command.index('-in') + 1]).read_text()))
                Path(command[command.index('-out') + 1]).write_bytes(b'encrypted')
                return subprocess.CompletedProcess(command, 0, b'', b'')
            stdout, stderr = io.StringIO(), io.StringIO()
            fault = subprocess.TimeoutExpired([marker], 30, output=marker.encode(), stderr=marker.encode())
            with patch.object(CHECK.sys, 'argv', arguments), patch.object(CHECK.sys, 'platform', 'linux'), \
                    patch.object(CHECK.oracle_pin, 'verify_bin', return_value=[]), \
                    patch.object(CHECK.round_files, 'manifest', side_effect=[[row()], []]), \
                    patch.object(CHECK.round_files, 'pool', return_value=MagicMock()), \
                    patch.object(CHECK.round_files, 'fetch_one', side_effect=fault), \
                    patch.object(CHECK.subprocess, 'run', side_effect=encrypt), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                self.assertEqual(CHECK.main(), 2)
            self.assertTrue((evidence / 'details.p7m').is_file())
            public = json.loads((evidence / 'summary.json').read_text())
            self.assertFalse(public['complete'])
            self.assertEqual(public['inputs'], 1)
            self.assertEqual(public['processed'], 0)
            self.assertEqual(public['failed'], 1)
            self.assertEqual(public['counts'], [])
            self.assertEqual(public['failures'], [{'stage': 'fetch', 'reason': 'timeout', 'files': 1}])
            self.assertEqual(captured['details'][0]['identity'], 'a' * 64)
            self.assertNotIn(marker, json.dumps(public) + stdout.getvalue() + stderr.getvalue())
            self.assertIn('fetch timeout', stderr.getvalue())

    def test_encryption_failure_is_closed_without_public_details(self):
        marker = 'private-detail-marker'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / 'coreav'
            candidate.write_bytes(b'generated-test-binary')
            evidence = root / 'evidence'
            arguments = ['checker', '--coreav', str(candidate), '--oracle-dir', str(root),
                         '--directory', str(evidence), '--evidence-cert', str(root / 'recipient.pem')]
            captured = {}

            def fail_encryption(command, **options):
                captured['private'] = json.loads(Path(command[command.index('-in') + 1]).read_text())
                return subprocess.CompletedProcess(command, 1, b'', marker.encode())

            stdout, stderr = io.StringIO(), io.StringIO()
            with patch.object(CHECK.sys, 'argv', arguments), patch.object(CHECK.sys, 'platform', 'linux'), \
                    patch.object(CHECK.oracle_pin, 'verify_bin', return_value=[]), \
                    patch.object(CHECK.round_files, 'manifest', side_effect=[[row()], []]), \
                    patch.object(CHECK.round_files, 'pool', return_value=MagicMock()), \
                    patch.object(CHECK.round_files, 'fetch_one', return_value=root / 'not-opened.mp4'), \
                    patch.object(CHECK, 'compare', return_value=('mismatch', [{'stderr': marker}])), \
                    patch.object(CHECK.subprocess, 'run', side_effect=fail_encryption), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                self.assertEqual(CHECK.main(), 2)
            public = (evidence / 'summary.json').read_text()
            self.assertNotIn(marker, public + stdout.getvalue() + stderr.getvalue())
            self.assertNotIn('candidate_sha256', public)
            self.assertIn('candidate_sha256', captured['private'])
            self.assertIn(marker, json.dumps(captured['private']))
            self.assertEqual(stderr.getvalue(), 'input-summary-development: failed at evidence-encryption process-failed\n')
            self.assertFalse(json.loads(public)['complete'])
            self.assertFalse(json.loads(public)['evidence_encrypted'])
            self.assertFalse((evidence / 'details.p7m').exists())

    def test_only_explicit_false_real_inputs_are_selected(self):
        candidates = [row(), row('b' * 64, holdout=True), row('c' * 64, holdout=0),
                      row('d' * 64, set='edge'), row('e' * 64, holdout=None)]
        selected = CHECK.select({'v1': candidates, 'v1-ts-audio': []})
        self.assertEqual([item['sha256'] for item in selected], ['a' * 64])

    def test_families_and_manifests_cannot_select_other_inputs(self):
        selected = CHECK.select({'v1': [row(ext='m4b'), row('b' * 64, ext='webm'), row('c' * 64, ext='ts')],
                                 'v1-ts-audio': [row('d' * 64, ext='m2t'), row('e' * 64, ext='wav')]})
        self.assertEqual([(item['family'], item['ext']) for item in selected],
                         [('isobmff', 'm4b'), ('matroska', 'webm'), ('mpegts', 'm2t')])
        self.assertTrue(all(item['key'].startswith('corpus/') for item in selected))

    def test_duplicate_identities_fail_before_download(self):
        with self.assertRaisesRegex(ValueError, 'duplicate-development-identity'):
            CHECK.select({'v1': [row(), row(ext='m4a')], 'v1-ts-audio': []})

    def test_identical_means_all_three_spellings_match(self):
        prefix = b"Input #0, wav, from 'file':\n  Stream #0:0: Audio: pcm_s16le\n"
        terminal = b'At least one output file must be specified\n'
        results = []
        for status, text in [(0, prefix), (0, prefix), (1, prefix + terminal)]:
            results.extend([subprocess.CompletedProcess([], status, b'', text)] * 2)
        with patch.object(CHECK, 'execute', side_effect=results):
            self.assertEqual(CHECK.compare(Path('native'), Path('oracle'), Path('media'))[0], 'identical')
        results[-1] = subprocess.CompletedProcess([], 2, b'', prefix + terminal)
        with patch.object(CHECK, 'execute', side_effect=results):
            self.assertEqual(CHECK.compare(Path('native'), Path('oracle'), Path('media'))[0], 'mismatch')

    def test_refusal_cannot_leak_partial_summary_or_stdout(self):
        prefix = b"Input #0, wav, from 'file':\n  Stream #0:0: Audio: pcm_s16le\n"
        for stdout, stderr in [(b'leak', b'refused'), (b'', prefix)]:
            results = []
            for status in (0, 0, 1):
                reference = prefix + (b'At least one output file must be specified\n' if status else b'')
                results.extend([subprocess.CompletedProcess([], status, b'', reference),
                                subprocess.CompletedProcess([], 4, stdout, stderr)])
            with self.subTest(stdout=stdout, stderr=stderr), patch.object(CHECK, 'execute', side_effect=results):
                self.assertEqual(CHECK.compare(Path('native'), Path('oracle'), Path('media'))[0], 'mismatch')


if __name__ == '__main__':
    unittest.main()
