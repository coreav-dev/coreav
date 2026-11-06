"""Metadata capture selection contracts without programs or media."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('videotoolbox_identity_capture', ROOT / 'tools/videotoolbox-identities/capture.py')
CAPTURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CAPTURE)
from evidence import Invalid


class IdentityCapture(unittest.TestCase):
    def test_metadata_executes_the_owner_returned_pair(self):
        oracle = CAPTURE.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'output'
            with patch.object(CAPTURE.sys, 'platform', 'darwin'), \
                 patch.object(CAPTURE.test_oracle, 'require', return_value=oracle) as require, \
                 patch.object(CAPTURE, 'command', return_value=b'configuration: --synthetic\n') as command, \
                 patch.object(CAPTURE, 'file_hash', return_value='a' * 64), \
                 patch.object(CAPTURE.registration, 'baseline_build'), \
                 patch.object(CAPTURE, 'observer_binary'), \
                 patch.object(CAPTURE.cache_residency, 'preflight'):
                CAPTURE.capture(root, output)
            require.assert_called_once_with(root.resolve() / 'ffmpeg', root.resolve() / 'ffprobe')
            self.assertEqual([call.args[0][0] for call in command.call_args_list],
                             [str(oracle.ffmpeg), str(oracle.ffprobe), str(oracle.ffmpeg)])
            self.assertTrue((output / 'capture.json').is_file())

    def test_pin_failure_precedes_commands_and_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'output'
            with patch.object(CAPTURE.sys, 'platform', 'darwin'), \
                 patch.object(CAPTURE.test_oracle, 'require', side_effect=CAPTURE.test_oracle.QualificationError('private-marker')), \
                 patch.object(CAPTURE, 'command') as command, \
                 patch.object(CAPTURE, 'observer_binary') as observer:
                with self.assertRaisesRegex(Invalid, '^baseline_pin$'):
                    CAPTURE.capture(root, output)
            command.assert_not_called()
            observer.assert_not_called()
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
