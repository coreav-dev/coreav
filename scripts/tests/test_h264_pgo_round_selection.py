from contextlib import ExitStack
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import h264_pgo_round as driver
import h264_pgo_acquire as acquisition
import h264_pgo_diagnostics as diagnostics


class Selection(unittest.TestCase):
    def test_run_executes_the_owner_returned_pair_outside_qualification(self):
        oracle = driver.test_oracle.Oracle(Path('/qualified/encoder'), Path('/qualified/probe'))
        row = {'label': 'h000', 'class': 'sd', 'width': 2, 'height': 2, 'sha256': 'digest'}
        metadata = {'codec_name': 'h264', 'pix_fmt': 'yuv420p', 'field_order': 'progressive',
                    'profile': 'Main', 'width': 2, 'height': 2}
        plan = {'frames': {'sd': 1}, 'rounds': 1, 'calibration_min': 0.9, 'calibration_max': 1.1}
        commands = []

        def execute(command, *args, **kwargs):
            commands.append(command)
            return {'stdout': json.dumps({'streams': [metadata]}), 'bytes': 6, 'sha256': 'same', 'wall': 1.0}

        with tempfile.TemporaryDirectory() as temporary, ExitStack() as stack:
            root = Path(temporary)
            files = {'reservation-receipt.json': {'commit': 'revision'},
                     'allocation-contract.json': {'revision': 'revision'}, 'instance.json': {},
                     'roster.json': [row], 'references.json': [row], 'exclusions.json': {}, 'collection.json': []}
            for name, value in files.items():
                (root / name).write_text(json.dumps(value))
            for name, value in {'merged': 'revision', 'reservation_receipt': None,
                                'reservation': files['reservation-receipt.json'], 'reserved_evidence': {},
                                'reserved_inputs': {}, 'evaluation_cohort': {'counts': {}},
                                'verify_allocation': [], 'host': {}, 'roster': None, 'selection': None,
                                'reference_evidence': None, 'build_evidence': None, 'evidence_inventory': [],
                                'digest': 'digest', 'schedule': [
                                    {'label': 'h000', 'arm': 'ffmpeg', 'threads': 1, 'round': 0}]}.items():
                stack.enter_context(mock.patch.object(driver, name, return_value=value))
            for name in ('verify_receipt', 'audit_collection', 'bind'):
                stack.enter_context(mock.patch.object(acquisition, name))
            stack.enter_context(mock.patch.object(acquisition, 'policy', return_value={'artifact_revision': 'revision'}))
            pcm = diagnostics.oracle_pcm(root, root / 'diagnostics/output.mp4')

            def diagnostic_run(root, plan, execute):
                execute(pcm, maximum=1, timeout=180)
                return 'complete'
            stack.enter_context(mock.patch.object(diagnostics, 'run', side_effect=diagnostic_run))
            stack.enter_context(mock.patch.object(driver.subprocess, 'run'))
            stack.enter_context(mock.patch.object(driver, 'execute', side_effect=execute))
            require = stack.enter_context(mock.patch.object(driver.test_oracle, 'require', return_value=oracle))
            driver.run(root, plan, 'allocation')
            require.assert_called_once_with(root / 'ffmpeg', root / 'ffprobe')
            self.assertEqual(commands.count([str(oracle.ffprobe), '-v', 'error', '-threads', '1', '-select_streams', 'v:0',
                                             '-show_entries', 'stream=codec_name,pix_fmt,width,height,field_order,profile',
                                             '-of', 'json', str(root / 'media/h000.h264')]), 1)
            self.assertIn([str(oracle.ffmpeg), *pcm[1:]], commands)
            oracle_calls = [c for c in commands if str(oracle.ffmpeg) in c and c[0] == 'taskset']
            self.assertEqual(len(oracle_calls), 5)
            self.assertTrue(all(c[3] == str(oracle.ffmpeg) for c in oracle_calls))
            self.assertFalse(any(str(root / 'ffmpeg') in c or str(root / 'ffprobe') in c for c in commands))
