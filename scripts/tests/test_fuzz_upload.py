"""scripts/fuzz_upload.py: a failed upload loses no other finding, fails the
run, and prints nothing of the error but its class."""
import contextlib
import importlib.util
import io
import os
import pathlib
import sys
import tempfile
import types
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'fuzz_upload.py'
SPEC = importlib.util.spec_from_file_location('fuzz_upload', SCRIPT)
fuzz_upload = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fuzz_upload)

ENV = {'R2_ENDPOINT': 'https://r2.invalid', 'R2_ACCESS_KEY_ID': 'id', 'R2_SECRET_ACCESS_KEY': 'secret',
       'R2_BUCKET': 'bucket'}


class Bucket:
    def __init__(self, failing):
        self.failing, self.tried, self.kept = failing, [], []

    def put_object(self, Bucket, Key, Body):
        self.tried.append(Key)
        if Key in self.failing:
            raise ConnectionError(f'SECRET-DETAIL {Key}')
        self.kept.append(Key)


def upload(names, failing):
    bucket = Bucket(failing)
    boto3 = types.SimpleNamespace(client=lambda *a, **k: bucket)
    with tempfile.TemporaryDirectory() as d:
        for name in names:
            (pathlib.Path(d) / name).mkdir()
            (pathlib.Path(d) / name / 'crash-input').write_bytes(b'INPUT-BYTES')
        out = io.StringIO()
        with mock.patch.dict(os.environ, ENV), mock.patch.dict(sys.modules, {'boto3': boto3}), \
                mock.patch.object(sys, 'argv', ['fuzz_upload.py', d, 'prefix']), contextlib.redirect_stdout(out):
            code = fuzz_upload.main()
    return code, bucket, out.getvalue()


class Upload(unittest.TestCase):
    def test_every_finding_is_kept(self):
        code, bucket, out = upload(['a', 'b'], set())
        self.assertEqual(code, 0)
        self.assertEqual(bucket.kept, ['prefix/a.tar.gz', 'prefix/b.tar.gz'])

    def test_a_failed_upload_loses_no_other_finding_and_fails(self):
        code, bucket, out = upload(['a', 'b'], {'prefix/a.tar.gz'})
        self.assertEqual(code, 1)
        self.assertEqual(bucket.kept, ['prefix/b.tar.gz'])
        self.assertEqual(bucket.tried.count('prefix/a.tar.gz'), fuzz_upload.ATTEMPTS)
        self.assertIn('::error::a: not kept', out)
        self.assertIn('ConnectionError', out)
        self.assertNotIn('SECRET-DETAIL', out)
        self.assertNotIn('INPUT-BYTES', out)


if __name__ == '__main__':
    unittest.main()
