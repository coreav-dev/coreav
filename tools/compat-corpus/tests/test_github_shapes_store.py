import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import github_shapes_store as store


class Client:
    def __init__(self):
        self.objects = {}
        self.corrupt = False

    def put_object(self, Bucket, Key, Body):
        self.objects[Key] = Body.read() if hasattr(Body, 'read') else Body

    def get_object(self, Bucket, Key):
        data = self.objects[Key]
        return {'Body': io.BytesIO(data + b'x' if self.corrupt else data)}


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.client = Client()

    def write(self, name, data):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def test_round_trip_and_receipt(self):
        self.write('completion.json', b'{"complete":true}\n')
        self.write('build.json', b'build')
        for name in store.REGISTRATIONS:
            self.write(f'{name}/summary.json', name.encode())
            self.write(f'{name}/evidence/data', b'evidence')
        self.assertTrue(store.store(self.root, 'run/one', self.client, 'bucket'))
        archive = self.client.objects['run/one/evidence.zip']
        receipt = json.loads(self.client.objects['run/one/retention.json'])
        self.assertEqual(receipt['archive'], {'size': len(archive), 'sha256': hashlib.sha256(archive).hexdigest()})
        self.assertTrue(receipt['complete'])
        with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
            self.assertEqual(set(zipped.namelist()), set(receipt['files']))
            for name, info in receipt['files'].items():
                data = zipped.read(name)
                self.assertEqual(info, {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
        self.assertEqual(receipt['summaries']['github-shapes-200/summary.json'],
                         receipt['files']['github-shapes-200/summary.json'])

    def test_incomplete_still_retained(self):
        self.write('build.json', b'build')
        self.write('github-shapes-200/evidence/partial', b'partial')
        self.assertFalse(store.store(self.root, 'run', self.client, 'bucket'))
        self.assertFalse(json.loads(self.client.objects['run/retention.json'])['complete'])
        with zipfile.ZipFile(io.BytesIO(self.client.objects['run/evidence.zip'])) as archive:
            self.assertEqual(archive.read('github-shapes-200/evidence/partial'), b'partial')

    def test_summaries_alone_cannot_claim_completed_replay(self):
        self.write('build.json', b'build')
        for name in store.REGISTRATIONS:
            self.write(f'{name}/summary.json', b'summary')
        self.assertFalse(store.store(self.root, 'run', self.client, 'bucket'))
        self.write('completion.json', b'{"complete":false}\n')
        self.assertFalse(store.store(self.root, 'other-run', self.client, 'bucket'))

    def test_corruption_prevents_receipt(self):
        self.write('build.json', b'build')
        self.client.corrupt = True
        with self.assertRaises(ValueError):
            store.store(self.root, 'run', self.client, 'bucket')
        self.assertNotIn('run/retention.json', self.client.objects)

    def test_empty_and_symlinks_rejected(self):
        with self.assertRaises(ValueError):
            store.store(self.root, 'run', self.client, 'bucket')
        self.write('file', b'data')
        (self.root / 'link').symlink_to(self.root / 'file')
        with self.assertRaises(ValueError):
            store.store(self.root, 'run', self.client, 'bucket')
        self.assertFalse(self.client.objects)

    def test_limits_and_prefix(self):
        self.write('file', b'data')
        with mock.patch.object(store, 'LIMIT', 1), self.assertRaises(ValueError):
            store.store(self.root, 'run', self.client, 'bucket')
        with self.assertRaises(ValueError):
            store.store(self.root, '../run', self.client, 'bucket')

    def test_missing_credentials_and_exception_are_redacted(self):
        stderr = io.StringIO()
        with mock.patch.dict(os.environ, {}, clear=True), contextlib.redirect_stderr(stderr):
            self.assertEqual(store.main(['--root', 'private-path', '--prefix', 'private-key']), 1)
        self.assertEqual(stderr.getvalue(), 'retention_failed\n')
        with mock.patch.object(store.Parser, 'parse_args', side_effect=ValueError('private secret')):
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                self.assertEqual(store.main([]), 1)
            self.assertEqual(stderr.getvalue(), 'retention_failed\n')


if __name__ == '__main__':
    unittest.main()
