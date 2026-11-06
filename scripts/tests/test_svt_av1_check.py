"""Exercise the small AV1 correctness check with synthetic decoder results."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import shlex
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
SPEC = importlib.util.spec_from_file_location('svt_av1_check', ROOT / 'scripts/svt_av1_check.py')
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class Correctness(unittest.TestCase):
    def test_documented_checks_include_feature_gated_process_targets(self):
        manifest = tomllib.loads((ROOT / 'crates/coreav-transcode/Cargo.toml').read_text())
        required = {entry['name'] for entry in manifest['test']
                    if 'svt-av1' in entry.get('required-features', [])}
        doc = (ROOT / 'docs/design/encoders/svt-av1.md').read_text()
        commands = [shlex.split(line) for line in doc.splitlines()
                    if line.startswith('cargo test -p coreav-transcode ')]
        self.assertEqual(len(commands), 1)
        command = commands[0]
        targets = {command[index + 1] for index, value in enumerate(command) if value == '--test'}
        self.assertTrue(required <= targets, f'Add --test for missing targets: {required - targets}')

    def fixture(self, mutation=None, moving=False):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            work = root / 'work'
            width, height, frames = (256, 256, 48) if moving else (128, 96, CHECK.FRAMES)
            size = frames * width * height * 3 // 2
            oracle = CHECK.test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified-probe/ffprobe'))
            commands = []

            def run(command):
                commands.append(command)
                if 'transcode' in command:
                    return json.dumps({'schema': 'coreav.transcode/2',
                                       'video': {'encoding': {'segments': 1 if mutation == 'segments' else 2}}}).encode()
                if Path(command[0]).name == 'ffprobe':
                    stream = {'codec_name': 'av1', 'width': width, 'height': height,
                              'pix_fmt': 'yuv420p', 'nb_read_frames': str(frames)}
                    if mutation == 'count':
                        stream['nb_read_frames'] = str(frames - 1)
                    return json.dumps({'streams': [stream]}).encode()
                output = Path(command[-1])
                if output.suffix == '.yuv':
                    value = 255 if mutation == 'quality' and output.name != 'reference.yuv' else 128
                    length = size - 1 if mutation == 'truncated' and output.name == 'oracle.yuv' else size
                    pixels = bytearray([value]) * length
                    if mutation == 'decoder' and output.name == 'coreav.yuv':
                        pixels[0] ^= 1
                    output.write_bytes(pixels)
                return b''

            with patch.object(CHECK.test_oracle, 'require', return_value=oracle), \
                    patch.object(CHECK, 'run', side_effect=run), \
                    contextlib.redirect_stdout(io.StringIO()):
                if mutation:
                    with self.assertRaises(ValueError):
                        CHECK.check(root / 'coreav', root, work, moving)
                else:
                    CHECK.check(root / 'coreav', root, work, moving)
                    self.assertEqual([command[0] for command in commands],
                                     [oracle.ffmpeg, oracle.ffmpeg, root / 'coreav',
                                      oracle.ffprobe, oracle.ffmpeg, root / 'coreav'])

    def test_moving_preset_zero_fixture(self):
        self.fixture(moving=True)

    def test_shared_input_uses_av1_without_h264_encoder_options(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(CHECK.test_oracle, 'require', return_value=CHECK.test_oracle.Oracle(root / 'ffmpeg', root / 'ffprobe')), \
                    patch.object(CHECK, 'run', side_effect=RuntimeError('generated')) as run:
                with self.assertRaisesRegex(RuntimeError, 'generated'):
                    CHECK.check(root / 'coreav', root, root / 'work', shared_input=True)
                command = run.call_args.args[0]
                self.assertEqual(command[command.index('-c:v') + 1], 'libaom-av1')
                self.assertNotIn('-x264-params', command)
                self.assertNotIn('-preset', command)

    def test_positive_decodes_and_refusal_controls(self):
        self.fixture()
        for mutation in ('segments', 'count', 'truncated', 'decoder', 'quality'):
            with self.subTest(mutation=mutation):
                self.fixture(mutation)

    def test_bad_oracle_is_refused_before_fixture_creation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(CHECK.test_oracle, 'require', side_effect=CHECK.test_oracle.QualificationError('wrong oracle')), \
                    patch.object(CHECK, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'wrong oracle'):
                    CHECK.check(root / 'coreav', root, root / 'work')
                run.assert_not_called()
                self.assertFalse((root / 'work').exists())


if __name__ == '__main__':
    unittest.main()
