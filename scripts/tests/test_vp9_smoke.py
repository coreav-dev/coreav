import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import urllib.error

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import vp9_smoke


class Smoke(unittest.TestCase):
    def test_selection_is_eight_distinct_existing_pinned_vectors(self):
        source, rows = vp9_smoke.registration()
        self.assertEqual(len(rows), 8)
        self.assertEqual(len({row['name'] for row in rows}), 8)
        self.assertEqual(tuple(row['name'] for row in rows), vp9_smoke.VECTORS)
        self.assertTrue(source.startswith('https://'))
        for row in rows:
            self.assertRegex(row['sha256'], '^[0-9a-f]{64}$')

    def test_unknown_duplicate_or_missing_registered_names_fail(self):
        manifest = vp9_smoke.vp9_corpus.read_json(vp9_smoke.ROOT / 'crates/coreav-vp9/tests/data/vectors.json')
        for rows in ([row for row in manifest['vectors'] if row['name'] != vp9_smoke.VECTORS[0]], manifest['vectors'] + [next(
                row for row in manifest['vectors'] if row['name'] == vp9_smoke.VECTORS[0])]):
            with mock.patch.object(vp9_smoke.vp9_corpus, 'read_json', return_value=dict(manifest, vectors=rows)):
                with self.assertRaises((StopIteration, ValueError)):
                    vp9_smoke.registration()
        with mock.patch.object(vp9_smoke, 'VECTORS', (*vp9_smoke.VECTORS[:-1], 'unknown.webm')):
            with self.assertRaises(StopIteration):
                vp9_smoke.registration()

    def fetch(self, data, expected=None, side_effect=None):
        row = {'name': 'sample.webm', 'sha256': hashlib.sha256(data if expected is None else expected).hexdigest()}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / row['name']
            response = mock.MagicMock()
            response.__enter__.return_value = io.BytesIO(data)
            with mock.patch.object(vp9_smoke.urllib.request, 'urlopen', return_value=response,
                                   side_effect=side_effect) as request, mock.patch.object(vp9_smoke.time, 'sleep'):
                vp9_smoke.fetch('https://example.invalid/', row, path)
                self.assertEqual(path.read_bytes(), data)
                return request.call_count

    def test_fetch_verifies_original_bytes_before_publishing_a_file(self):
        self.assertEqual(self.fetch(b'original'), 1)
        with self.assertRaises(ValueError):
            self.fetch(b'changed', expected=b'original')
        with mock.patch.object(vp9_smoke, 'LIMIT', 3):
            with self.assertRaises(ValueError):
                self.fetch(b'oversize')

    def test_fetch_retries_transient_failures_only_within_the_cap(self):
        response = mock.MagicMock()
        response.__enter__.return_value = io.BytesIO(b'original')
        self.assertEqual(self.fetch(b'original', side_effect=[ConnectionResetError(), TimeoutError(), response]), 3)
        with self.assertRaises(TimeoutError):
            self.fetch(b'original', side_effect=TimeoutError())
        with self.assertRaises(urllib.error.HTTPError):
            self.fetch(b'original', side_effect=urllib.error.HTTPError('https://example.invalid', 404, 'missing', {}, None))

    def exercise(self, comparison):
        with tempfile.TemporaryDirectory() as directory:
            args = SimpleNamespace(native=Path('native'), oracle_bin=Path('oracle'), out=Path(directory) / 'out')
            _, rows = vp9_smoke.registration()
            seen = []
            def fetch(source, row, path):
                path.write_bytes(b'original')
            def compare(entry, tools, b5, private):
                seen.append(Path(entry['path']).name)
                self.assertEqual(entry['sha256'], rows[len(seen) - 1]['sha256'])
                self.assertEqual(Path(entry['path']).read_bytes(), b'original')
                self.assertEqual(b5, set())
                self.assertEqual((tools.native, tools.ffmpeg, tools.ffprobe), ('native', 'ffmpeg', 'ffprobe'))
                return comparison
            with mock.patch.object(vp9_smoke.sys, 'platform', 'linux'), \
                 mock.patch.object(vp9_smoke.test_oracle, 'require', return_value=SimpleNamespace(ffmpeg='ffmpeg', ffprobe='ffprobe')), \
                 mock.patch.object(vp9_smoke, 'fetch', side_effect=fetch), \
                 mock.patch.object(vp9_smoke.vp9_corpus, 'compare', side_effect=compare):
                if comparison.get('status') == 'agree' and type(comparison.get('frames')) is int and comparison['frames'] > 0:
                    self.assertEqual(vp9_smoke.run(args), 0)
                else:
                    with self.assertRaises(ValueError):
                        vp9_smoke.run(args)
            self.assertEqual(list(args.out.glob('*.webm')), [])
            return seen, json.loads((args.out / 'smoke.json').read_text())

    def test_actual_entrypoint_runs_every_original_through_the_existing_comparator(self):
        seen, report = self.exercise({'status': 'agree', 'frames': 7})
        self.assertEqual(tuple(seen), vp9_smoke.VECTORS)
        self.assertTrue(report['complete'])
        self.assertEqual(len(report['vectors']), 8)
        self.assertTrue(all(row['frames'] == 7 for row in report['vectors']))

    def test_no_pixel_credit_for_refusal_zero_frames_or_false_agreement(self):
        for result in ({'status': 'outside_profile0', 'frames': 7}, {'status': 'failed', 'frames': 7},
                       {'status': 'agree', 'frames': 0}, {'status': 'agree', 'frames': True}):
            seen, report = self.exercise(result)
            self.assertEqual(len(seen), 1)
            self.assertFalse(report['complete'])
            self.assertEqual(report['vectors'], [])
            self.assertEqual(report['failed_stage'], 'comparison')


if __name__ == '__main__':
    unittest.main()
