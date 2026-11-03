"""Synthetic characterization rosters and default-option capture behavior."""

import contextlib
import io
import json
from pathlib import Path
import struct
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import swr_characterize as swr


class Characterization(unittest.TestCase):
    def test_every_wave_is_unique_and_within_the_registered_call_cap(self):
        for wave, count in (('initial', 540), ('confirmation', 570), ('raw-confirmation', 714)):
            cases = list(swr.cases(wave))
            self.assertEqual(len(cases), count)
            self.assertEqual(len(set(cases)), count)
            self.assertLessEqual(count, 768)
            for _, _, source, _, channels, signal, frames, _, _ in cases:
                data = swr.pcm(swr.signal(signal, frames), channels, source)
                self.assertEqual(len(data), frames * channels * struct.calcsize(swr.FORMS[source][0]))

    def test_final_wave_exercises_signed_extrema_and_only_the_native_candidate(self):
        values = struct.unpack('<8h', swr.pcm(swr.signal('extrema', 8), 1, 's16le'))
        self.assertEqual(values, (-32768, 32767, -1, 0, 1, 32766, -32767, 16384))
        for _, _, source, target, channels, _, _, scalar, _ in swr.cases('raw-confirmation'):
            self.assertEqual((source, target), ('s16le', 's16le'))
            self.assertIn(channels, (1, 2))
            self.assertFalse(scalar)

    def test_final_capture_leaves_the_default_dither_option_unmodified(self):
        case = (44100, 48000, 's16le', 's16le', 1, 'lcg-full', 25, False, None)
        for wave in ('initial', 'confirmation', 'raw-confirmation'):
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                binary = root / 'ffmpeg'
                binary.write_bytes(b'stand-in')
                result = SimpleNamespace(returncode=0, stdout=b'', stderr=b'')
                with patch.object(swr, 'verify_bin', return_value=[]), \
                     patch.object(swr, 'cases', return_value=[case]), \
                     patch.object(swr.subprocess, 'check_output', side_effect=['a' * 40, 'stand-in']), \
                     patch.object(Path, 'read_text', return_value='model name: test\nflags: fma'), \
                     patch.object(swr.subprocess, 'run', return_value=result) as run, \
                     contextlib.redirect_stdout(io.StringIO()):
                    swr.capture(binary, root / 'capture', wave)
                args = run.call_args.args[0]
                self.assertEqual('-dither_method' in args, wave != 'raw-confirmation')
                self.assertEqual(args[args.index('-i') - 2:args.index('-i')], list(swr.AUDIO_DECODE_INPUT))
                self.assertTrue(json.loads((root / 'capture/provenance.json').read_text())['complete'])


if __name__ == '__main__':
    unittest.main()
