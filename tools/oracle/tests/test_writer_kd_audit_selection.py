"""The writer audits execute the oracle pair the shared owner returned."""
import contextlib
import importlib.util
import io
import json
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[3]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'tools' / 'oracle' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


audit = load('writer_kd_audit')
cli = load('writer_kd_audit_cli')
PAIR = audit.test_oracle.Oracle(pathlib.Path('/qualified/ffmpeg'), pathlib.Path('/qualified/ffprobe'))


class Pool:
    created = []

    def __init__(self, workers):
        Pool.created.append(self)
        self.jobs = []

    def __enter__(self):
        return self

    def __exit__(self, *error):
        return False

    def map(self, function, jobs):
        self.jobs.extend(jobs)
        return [[] for _ in self.jobs]


class WriterAudit(unittest.TestCase):
    def run_main(self, **require):
        Pool.created.clear()
        with tempfile.TemporaryDirectory() as directory:
            manifest = pathlib.Path(directory) / 'manifest.json'
            manifest.write_text(json.dumps([{'output': 'out.mp4'}]))
            argv = ['writer_kd_audit.py', '--writer', 'clip', '--manifest', str(manifest),
                    '--ffmpeg', 'chosen/ffmpeg', '--ffprobe', 'chosen/ffprobe']
            with patch.object(sys, 'argv', argv), patch.object(audit, 'ProcessPoolExecutor', Pool), \
                    patch.object(audit.test_oracle, 'require', **require) as owner, \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                try:
                    audit.main()
                finally:
                    owner.assert_called_once_with('chosen/ffmpeg', 'chosen/ffprobe')

    def test_workers_receive_the_owner_returned_pair(self):
        self.run_main(return_value=PAIR)
        self.assertEqual(Pool.created[0].jobs[0][2], ('/qualified/ffprobe', '/qualified/ffmpeg', None))

    def test_an_unqualified_pair_stops_before_any_worker_starts(self):
        error = audit.test_oracle.QualificationError('stage=oracle_qualification: fake')
        with self.assertRaisesRegex(SystemExit, 'stage=oracle_qualification'):
            self.run_main(side_effect=error)
        self.assertEqual(Pool.created, [])


class CliAudit(unittest.TestCase):
    def selected_argv(self, oracle):
        calls = []
        runner = 'run_text' if oracle == 'text' else 'run_policy'
        argv = ['writer_kd_audit_cli.py', '--oracle', oracle, '--coreav', 'coreav', '--media', 'media']
        with patch.object(sys, 'argv', argv), patch.object(cli.test_oracle, 'require', return_value=PAIR), \
                patch.object(cli, runner, side_effect=lambda value, kds: calls.append(value) or {}), \
                patch.object(cli, 'TEXT_MUTATIONS', []), patch.object(cli, 'POLICY_MUTATIONS', []), \
                patch.dict(cli.os.environ), contextlib.redirect_stdout(io.StringIO()), \
                contextlib.redirect_stderr(io.StringIO()):
            try:
                cli.main()
            except SystemExit:
                pass
        return calls[0]

    def test_text_oracle_receives_the_owner_returned_pair(self):
        argv = self.selected_argv('text')
        self.assertEqual(argv[argv.index('--ffmpeg') + 1], '/qualified/ffmpeg')
        self.assertEqual(argv[argv.index('--ffprobe') + 1], '/qualified/ffprobe')

    def test_policy_oracle_receives_the_owner_returned_probe(self):
        argv = self.selected_argv('policy')
        self.assertEqual(argv[argv.index('--ffprobe') + 1], '/qualified/ffprobe')
        self.assertNotIn('--ffmpeg', argv)


if __name__ == '__main__':
    unittest.main()
