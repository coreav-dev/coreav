import importlib.util
from pathlib import Path
import random
import struct
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'audio_source_diagnostic.py'
SPEC = importlib.util.spec_from_file_location('audio_source_diagnostic', SCRIPT)
diagnostic = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(diagnostic)


def pcm(values):
    return struct.pack('<' + 'f' * len(values), *values)


class AudioSourceDiagnostic(unittest.TestCase):
    def test_equal_pcm_and_counts(self):
        raw = pcm(range(64))
        result = diagnostic.compare_pcm(raw, raw, max_lag=4, window=16)
        self.assertTrue(result['equal'])
        self.assertEqual(result['coreav_samples'], 64)
        self.assertEqual(result['ffmpeg_samples'], 64)
        self.assertEqual(result['max_absolute_difference'], 0)
        self.assertIsNone(result['zero_lag_snr_db'])
        self.assertEqual(result['lag_peaks'][0]['lag'], 0)

    def test_lag_direction_and_unequal_lengths_are_reported(self):
        rng = random.Random(7)
        reference = [rng.uniform(-1, 1) for _ in range(80)]
        result = diagnostic.compare_pcm(pcm([0] * 3 + reference), pcm(reference), max_lag=4, window=32)
        self.assertFalse(result['equal'])
        self.assertEqual(result['coreav_samples'], 83)
        self.assertEqual(result['ffmpeg_samples'], 80)
        self.assertEqual(result['lag_peaks'][0]['lag'], 3)

    def test_silence_and_nonfinite_pcm_are_not_false_agreement(self):
        result = diagnostic.compare_pcm(pcm([0] * 64), pcm([0] * 64), max_lag=4, window=16)
        self.assertTrue(result['reference_silent'])
        self.assertEqual(result['lag_peaks'], [])
        with self.assertRaises(ValueError):
            diagnostic.compare_pcm(pcm([float('nan')] * 64), pcm([0] * 64), max_lag=4, window=16)

    def test_owned_f32_wav_data_is_not_redecoded(self):
        raw = pcm([0.25, -0.5])
        fmt = struct.pack('<HHIIHH', 3, 1, 48_000, 192_000, 4, 32)
        chunks = b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(raw)) + raw
        wav = b'RIFF' + struct.pack('<I', len(chunks) + 4) + b'WAVE' + chunks
        self.assertEqual(diagnostic.wav_pcm(wav, 48_000), raw)
        fmt = (struct.pack('<HHIIHHHHI', 0xfffe, 1, 48_000, 192_000, 4, 32, 22, 32, 4)
               + bytes.fromhex('0300000000001000800000aa00389b71'))
        extended = b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(raw)) + raw
        extended = b'RIFF' + struct.pack('<I', len(extended) + 4) + b'WAVE' + extended
        self.assertEqual(diagnostic.wav_pcm(extended, 48_000), raw)
        with self.assertRaises(ValueError):
            diagnostic.wav_pcm(wav, 44_100)
        with self.assertRaises(ValueError):
            diagnostic.wav_pcm(wav[:-1], 48_000)


if __name__ == '__main__':
    unittest.main()
