"""Check native media binding with synthetic files, never encoded AV1 evidence."""

from dataclasses import replace
import hashlib
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import svt_av1_cases as cases
import svt_av1_media as media


@unittest.skipUnless(hasattr(os, 'O_NOFOLLOW'), 'native media is supported on Unix targets only')
class MediaBinding(unittest.TestCase):
    def fixture(self, directory):
        case = next(case for case in cases.load() if case.capacity == 3 and case.pictures == 10)
        names = ['wire.bin', 'combined.obu', 'samples.ndjson', 'configuration.av1c',
                 'complete.json', 'layout.json']
        names += [f'segment-{number}.obu' for number in range(4)]
        for name in names:
            (directory / name).write_bytes(b'synthetic media, not encoded AV1: ' + name.encode())
        identity = {'revision': 'a' * 40, 'workflow_run': '1', 'workflow_attempt': '2',
                    'target': 'x86_64-unknown-linux-musl', 'roster_sha256': 'b' * 64,
                    'native_manifest_sha256': 'c' * 64, 'binary_sha256': 'd' * 64}
        return case, identity

    def test_complete_inventory_binds_every_file_case_and_producing_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            case, identity = self.fixture(root)
            record = media.capture(root, case, identity, time.monotonic() + 30)
            self.assertEqual(record['identity'], identity)
            self.assertEqual(len(record['files']), 10)
            self.assertEqual(record['case']['pictures'], 10)
            self.assertEqual(record['case']['dimensions'], list(case.dimensions))
            self.assertEqual(record['total_bytes'], sum(path.stat().st_size for path in root.iterdir()))
            for name, value in record['files'].items():
                self.assertEqual(value, {'bytes': (root / name).stat().st_size,
                                        'sha256': hashlib.sha256((root / name).read_bytes()).hexdigest()})
            self.assertEqual(media.verify(root, case, identity, record, time.monotonic() + 30),
                             record['total_bytes'])

    def test_relabelled_build_case_and_file_records_are_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            case, identity = self.fixture(root)
            record = media.capture(root, case, identity, time.monotonic() + 30)
            for field in identity:
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'identity changed'):
                    media.verify(root, case, {**identity, field: 'another'}, record, time.monotonic() + 30)
            with self.assertRaisesRegex(ValueError, 'identity changed'):
                media.verify(root, replace(case, crf=(case.crf + 1) % 64), identity, record,
                             time.monotonic() + 30)
            value = record['files']['wire.bin']['bytes']
            for invalid in (float(value), float('nan')):
                with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                    record['files']['wire.bin']['bytes'] = invalid
                    media.verify(root, case, identity, record, time.monotonic() + 30)
            record['files']['wire.bin']['bytes'] = value
            record['files']['wire.bin']['bytes'] += 1
            with self.assertRaisesRegex(ValueError, 'identity changed'):
                media.verify(root, case, identity, record, time.monotonic() + 30)

    def test_changed_missing_extra_empty_and_nonregular_media_are_refused(self):
        for mutation in ('changed', 'missing', 'extra', 'unregistered', 'unknown', 'empty', 'link', 'directory', 'fifo'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                case, identity = self.fixture(root)
                record = media.capture(root, case, identity, time.monotonic() + 30)
                path = root / 'wire.bin'
                if mutation == 'changed':
                    path.write_bytes(b'changed synthetic bytes')
                elif mutation == 'extra':
                    (root / 'segment-4.obu').write_bytes(b'unregistered segment')
                elif mutation == 'unregistered':
                    (root / 'unregistered.obu').write_bytes(b'unregistered coded file')
                elif mutation == 'unknown':
                    (root / 'hidden.bin').write_bytes(b'unregistered output')
                elif mutation == 'empty':
                    path.write_bytes(b'')
                else:
                    path.unlink()
                    if mutation == 'link':
                        path.symlink_to(root / 'combined.obu')
                    elif mutation == 'directory':
                        path.mkdir()
                    elif mutation == 'fifo':
                        os.mkfifo(path)
                with self.assertRaises((OSError, ValueError)):
                    media.verify(root, case, identity, record, time.monotonic() + 30)

    def test_file_target_and_deadline_limits_are_checked_before_unbounded_reads(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            case, identity = self.fixture(root)
            record = media.capture(root, case, identity, time.monotonic() + 30)
            self.assertEqual(media.capture(root, case, identity, time.monotonic() + 30,
                                           record['total_bytes']), record)
            with self.assertRaisesRegex(ValueError, 'budget'):
                media.capture(root, case, identity, time.monotonic() + 30, record['total_bytes'] - 1)
            with self.assertRaisesRegex(ValueError, 'deadline'):
                media.capture(root, case, identity, time.monotonic() - 1)
            with (root / 'wire.bin').open('wb') as output:
                output.truncate(media.MAX_FILE_BYTES + 1)
            with self.assertRaisesRegex(ValueError, 'budget'):
                media.capture(root, case, identity, time.monotonic() + 30)

    def test_expiration_during_final_io_or_record_comparison_prevents_success(self):
        for phase in ('last-file', 'comparison'):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                case, identity = self.fixture(root)
                record = media.capture(root, case, identity, time.monotonic() + 30)
                clock = [0.0]
                original_lstat = Path.lstat
                original_dumps = media.json.dumps

                def lstat(path, *args, **kwargs):
                    result = original_lstat(path, *args, **kwargs)
                    if phase == 'last-file' and path.name == 'wire.bin':
                        clock[0] = 11.0
                    return result

                def dumps(value, *args, **kwargs):
                    result = original_dumps(value, *args, **kwargs)
                    if phase == 'comparison' and value is record:
                        clock[0] = 11.0
                    return result

                with patch.object(media.time, 'monotonic', side_effect=lambda: clock[0]), \
                        patch.object(Path, 'lstat', lstat), \
                        patch.object(media.json, 'dumps', side_effect=dumps), \
                        self.assertRaisesRegex(ValueError, 'deadline'):
                    if phase == 'last-file':
                        media.capture(root, case, identity, 10.0)
                    else:
                        media.verify(root, case, identity, record, 10.0)


if __name__ == '__main__':
    unittest.main()
