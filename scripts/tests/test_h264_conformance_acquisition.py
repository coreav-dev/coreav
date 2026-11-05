import contextlib
import errno
import hashlib
import http.client
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
import urllib.error
from unittest.mock import Mock, patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import h264_conformance as conformance


class Acquisition(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.zips = pathlib.Path(self.temp.name)
        self.payload = b"synthetic archive bytes"
        self.stream = dict(name="synthetic", zip="synthetic.zip",
                           zip_sha256=hashlib.sha256(self.payload).hexdigest(), in_scope=True)
        self.archive = self.zips / self.stream['zip']
        self.log = io.StringIO()
        redirect = contextlib.redirect_stdout(self.log)
        redirect.__enter__()
        self.addCleanup(redirect.__exit__, None, None, None)
        self.open = self.enterContext(patch.object(conformance.urllib.request, 'urlopen'))
        self.sleep = self.enterContext(patch.object(conformance.time, 'sleep'))

    def fetch(self):
        return conformance.fetch(str(self.zips), self.stream)

    def assert_no_downloads(self):
        self.assertEqual(list(self.zips.iterdir()), [])

    def test_cache_hit_is_hashed_without_network(self):
        self.archive.write_bytes(self.payload)
        self.assertEqual(self.fetch(), str(self.archive))
        self.open.assert_not_called()
        self.assertIn('cache SHA-256 verified', self.log.getvalue())

    def test_corrupt_cache_gets_one_verified_replacement(self):
        self.archive.write_bytes(b'corrupt cache')
        self.open.return_value = io.BytesIO(self.payload)
        self.fetch()
        self.assertEqual(self.archive.read_bytes(), self.payload)
        self.assertIn('cache SHA-256 mismatch', self.log.getvalue())
        self.assertEqual(self.open.call_count, 1)

    def test_corrupt_cache_and_bad_replacement_fail(self):
        self.archive.write_bytes(b'corrupt cache')
        self.open.return_value = io.BytesIO(b'bad replacement')
        with self.assertRaisesRegex(SystemExit, 'repeated digest mismatch'):
            self.fetch()
        self.assertEqual(self.open.call_count, 1)
        self.assert_no_downloads()

    def test_transient_errors_recover_with_backoff_and_source_logs(self):
        for error in (ConnectionRefusedError(errno.ECONNREFUSED, 'refused'),
                      OSError(errno.ENETUNREACH, 'unreachable'),
                      OSError(errno.EHOSTUNREACH, 'host unreachable'),
                      TimeoutError('timeout'),
                      http.client.RemoteDisconnected('closed'),
                      urllib.error.URLError(http.client.RemoteDisconnected('closed')),
                      urllib.error.URLError(TimeoutError('timeout')),
                      http.client.IncompleteRead(b'partial'),
                      urllib.error.HTTPError('url', 503, 'unavailable', {}, None)):
            with self.subTest(error=type(error).__name__):
                self.archive.unlink(missing_ok=True)
                self.open.reset_mock()
                self.sleep.reset_mock()
                self.open.side_effect = [error, io.BytesIO(self.payload)]
                self.fetch()
                self.assertEqual(self.archive.read_bytes(), self.payload)
                self.assertEqual(self.open.call_count, 2)
                self.sleep.assert_called_once_with(5)
        self.assertIn(f'GET {conformance.BASE}/AVCv1/synthetic.zip attempt 1/3', self.log.getvalue())
        self.assertIn('HTTP 503; transient failure', self.log.getvalue())
        self.assertIn('SHA-256 verified', self.log.getvalue())

    def test_transient_exhaustion_is_bounded_for_each_existing_url(self):
        self.open.side_effect = urllib.error.URLError(OSError(errno.ENETUNREACH, 'unreachable'))
        with self.assertRaisesRegex(SystemExit, 'could not fetch'):
            self.fetch()
        self.assertEqual(self.open.call_count, 6)
        self.assertEqual([c.args[0] for c in self.sleep.call_args_list], [5, 10, 5, 10])
        for sub in conformance.SUBDIRS:
            self.assertIn(f'{conformance.BASE}/{sub}/synthetic.zip', self.log.getvalue())
        self.assert_no_downloads()

    def test_permanent_http_error_falls_back_without_retry(self):
        self.open.side_effect = [urllib.error.HTTPError('url', 404, 'missing', {}, None),
                                 io.BytesIO(self.payload)]
        self.fetch()
        self.assertEqual([c.args[0] for c in self.open.call_args_list],
                         [f'{conformance.BASE}/{sub}/synthetic.zip' for sub in conformance.SUBDIRS])
        self.sleep.assert_not_called()
        self.assertIn('HTTP 404; permanent failure', self.log.getvalue())

    def test_digest_mismatch_is_redownloaded_once(self):
        self.open.side_effect = [io.BytesIO(b'error page'), io.BytesIO(self.payload)]
        self.fetch()
        self.assertEqual(self.open.call_count, 2)
        self.assertEqual(self.archive.read_bytes(), self.payload)
        self.assertIn('expected ' + self.stream['zip_sha256'], self.log.getvalue())

    def test_second_digest_mismatch_never_tries_another_url(self):
        self.open.side_effect = [io.BytesIO(b'error page'), io.BytesIO(b'other error page')]
        with self.assertRaisesRegex(SystemExit, 'repeated digest mismatch'):
            self.fetch()
        self.assertEqual(self.open.call_count, 2)
        self.assertNotIn('/FRExt/', self.log.getvalue())
        self.assert_no_downloads()

    def test_partial_transfer_is_removed_before_retry(self):
        response = io.BytesIO()
        response.read = Mock(side_effect=[b'partial', TimeoutError('timeout')])
        self.open.side_effect = [response, io.BytesIO(self.payload)]
        self.fetch()
        self.assertEqual(list(self.zips.iterdir()), [self.archive])
        self.assertEqual(self.archive.read_bytes(), self.payload)

    def test_filesystem_failure_is_not_retried_as_network(self):
        self.open.return_value = io.BytesIO(self.payload)
        with patch.object(conformance.os, 'replace', side_effect=OSError(errno.ENOSPC, 'disk full')):
            with self.assertRaises(OSError):
                self.fetch()
        self.assertEqual(self.open.call_count, 1)
        self.sleep.assert_not_called()
        self.assert_no_downloads()

    def test_download_only_verifies_cached_bytes_without_cargo_or_extraction(self):
        self.archive.write_bytes(self.payload)
        manifest = self.zips / 'manifest.json'
        manifest.write_text(json.dumps(dict(streams=[self.stream])))
        script = pathlib.Path(conformance.__file__).resolve()
        result = subprocess.run([sys.executable, '-I', str(script), '--download-only',
                                 '--manifest', str(manifest), '--zips', str(self.zips)],
                                capture_output=True, text=True, timeout=10, env={'PATH': ''})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('cache SHA-256 verified', result.stdout)
        self.assertEqual(set(self.zips.iterdir()), {manifest, self.archive})


class Workflow(unittest.TestCase):
    def test_both_jobs_restore_acquire_and_save_before_decoding(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/ci.yml').read_text()
        for job in ('conformance', 'conformance-arm64'):
            body = workflow.split(f'\n  {job}:\n', 1)[1].split('\n  #', 1)[0]
            self.assertLess(body.index('actions/cache/restore@'), body.index('--download-only'))
            self.assertLess(body.index('--download-only'), body.index('actions/cache/save@'))
            self.assertLess(body.index('actions/cache/save@'), body.index('name: Decode every in-scope stream'))
            self.assertIn("hashFiles('crates/coreav-h264/tests/data/conformance-streams.json')", body)
            self.assertIn("steps.conformance-acquisition.outcome == 'failure'", body)
            self.assertIn("format('-partial-{0}-{1}-{2}'", body)
            self.assertIn("!cancelled()", body)
            for action in ('restore', 'save'):
                self.assertIn(f'actions/cache/{action}@5a3ec84eff668545956fd18022155c47e93e2684', body)


if __name__ == '__main__':
    unittest.main()
