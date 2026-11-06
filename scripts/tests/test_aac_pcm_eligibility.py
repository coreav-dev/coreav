import array
import copy
import importlib.util
import struct
from pathlib import Path
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / 'aac_pcm_eligibility.py'
SPEC = importlib.util.spec_from_file_location('aac_pcm_eligibility', SCRIPT)
qualification = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qualification)


class PcmQualification(unittest.TestCase):
    def test_whole_wav_comparison_covers_framing_and_both_qualified_payloads(self):
        def wave(pcm, info=b''):
            body = b'WAVEfmt ' + struct.pack('<IHHIIHH', 16, 1, 1, 48000, 96000, 2, 16)
            body += info + b'data' + struct.pack('<I', len(pcm)) + pcm
            return b'RIFF' + struct.pack('<I', len(body)) + body

        a, b = b'\x01\x00\x02\x00', b'\x02\x00\x03\x00'
        info = b'LIST' + struct.pack('<I', 4) + b'INFO'
        qualification.compare_wav(wave(a, info), wave(b, info), a, b, 1, 48000)
        with self.assertRaisesRegex(ValueError, 'framing or metadata'):
            qualification.compare_wav(wave(a), wave(b, info), a, b, 1, 48000)
        for actual, reference in [(wave(b, info), wave(b, info)),
                                  (wave(a, info), wave(a, info)),
                                  (wave(a[:-2], info), wave(b, info))]:
            with self.assertRaisesRegex(ValueError, 'qualified PCM'):
                qualification.compare_wav(actual, reference, a, b, 1, 48000)
        with self.assertRaisesRegex(ValueError, 'RIFF extent'):
            qualification.compare_wav(wave(a, info) + b'X', wave(b, info), a, b, 1, 48000)

    def test_pns_timeline_uses_explicit_priming_and_checked_terminal_padding(self):
        metadata = {'packets': 2, 'frames': [{'offset': 100}, {'offset': 200}],
                    'no_edits': False, 'media_duration': 2048, 'gapless_end': None}
        listing = {'streams': [{'duration_ts': 1000}], 'packets': [
            {'pos': '100', 'pts': -1024, 'dts': -1024, 'duration': 1024,
             'side_data_list': [{'side_data_type': 'Skip Samples', 'skip_samples': 1024}]},
            {'pos': '200', 'pts': 0, 'dts': 0, 'duration': 1000,
             'side_data_list': [{'side_data_type': 'Skip Samples', 'discard_padding': 24}]}]}
        self.assertEqual(qualification.presentation_interval(metadata, listing), (1024, 1000))
        changed = copy.deepcopy(listing)
        changed['packets'][-1]['side_data_list'][0]['discard_padding'] = 25
        with self.assertRaisesRegex(ValueError, 'independently predicted endpoint'):
            qualification.presentation_interval(metadata, changed)
        for packet_list in (listing['packets'][1:], list(reversed(listing['packets']))):
            changed = {**listing, 'packets': packet_list}
            with self.assertRaisesRegex(ValueError, 'complete stored packet sequence'):
                qualification.presentation_interval(metadata, changed)

    def test_pns_no_edit_endpoint_uses_media_duration_and_gapless_metadata(self):
        metadata = {'packets': 2, 'frames': [{'offset': 100}, {'offset': 200}],
                    'no_edits': True, 'media_duration': 2048, 'gapless_end': 1800}
        listing = {'streams': [{'duration_ts': 9999}], 'packets': [
            {'pos': '100', 'pts': 0, 'dts': 0, 'duration': 1024},
            {'pos': '200', 'pts': 1024, 'dts': 1024, 'duration': 1024}]}
        self.assertEqual(qualification.presentation_interval(metadata, listing), (0, 1800))
        metadata['gapless_end'] = None
        self.assertEqual(qualification.presentation_interval(metadata, listing), (0, 2048))

    def test_native_input_threads_inherit_the_shared_audio_policy(self):
        with mock.patch.object(qualification, 'AUDIO_DECODE_INPUT', ('-threads:a', '2')):
            self.assertEqual(qualification.native('coreav', 'input', []),
                             ['coreav', '--strict-safe', 'ffmpeg', '-threads', '2', '-i', 'input'])

    def test_native_commands_stay_distinct_from_the_pinned_oracle(self):
        self.assertEqual(qualification.native('coreav', 'input', ['-f', 's16le']),
                         ['coreav', '--strict-safe', 'ffmpeg', '-threads', '1', '-i', 'input', '-f', 's16le'])
        self.assertEqual(qualification.native('coreav', 'input', [], strict=False),
                         ['coreav', 'ffmpeg', '-threads', '1', '-i', 'input'])
        import oracle_audio
        self.assertEqual(list(oracle_audio.python_problems(SCRIPT.read_text(),
                                                         'scripts/aac_pcm_eligibility.py')), [])

    def pcm(self, code, values):
        values = array.array(code, values)
        import sys
        if sys.byteorder != 'little':
            values.byteswap()
        return values.tobytes()

    def test_each_s16_sample_must_be_within_one_lsb(self):
        reference = self.pcm('h', [-32768, 0, 32767])
        qualification.compare(self.pcm('h', [-32767, 1, 32766]), reference, 's16', 1)
        with self.assertRaisesRegex(ValueError, 'maximum error'):
            qualification.compare(self.pcm('h', [-32768, 2, 32767]), reference, 's16', 1)

    def test_f32_bound_is_absolute_and_inclusive(self):
        reference = self.pcm('f', [0, 0, 0])
        qualification.compare(self.pcm('f', [0, 2 ** -16, -(2 ** -16)]), reference, 'f32', 1)
        with self.assertRaisesRegex(ValueError, 'maximum error'):
            qualification.compare(self.pcm('f', [0, 2 ** -15, 0]), reference, 'f32', 1)

    def test_nonfinite_samples_and_any_count_drift_fail(self):
        reference = self.pcm('f', [0, 0])
        for values in ([float('nan'), 0], [0, float('inf')], [0], [0, 0, 0], []):
            with self.subTest(values=values), self.assertRaises(ValueError):
                qualification.compare(self.pcm('f', values), reference, 'f32', 1)
        with self.assertRaises(ValueError):
            qualification.compare(reference, self.pcm('f', [0, float('nan')]), 'f32', 1)

    def test_channel_order_and_final_sample_are_compared(self):
        reference = self.pcm('h', [0, 100, 200, 300])
        for values in ([100, 0, 300, 200], [0, 100, 200, 302]):
            with self.subTest(values=values), self.assertRaises(ValueError):
                qualification.compare(self.pcm('h', values), reference, 's16', 2)



class OracleSelection(unittest.TestCase):
    def test_generation_receives_the_owner_returned_pair(self):
        import sys
        import tempfile
        import test_oracle
        pair = test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        stopped = RuntimeError('stop before generating media')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            argv = ['aac_pcm_eligibility', '--coreav', 'coreav', '--oracle-dir', '/requested',
                    '--work', str(root / 'work'), '--out', str(root / 'report.json'),
                    '--pns-diagnostic', 'diagnostic']
            with mock.patch.object(sys, 'argv', argv), \
                    mock.patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                    mock.patch.object(qualification, 'generate', side_effect=stopped) as generate:
                with self.assertRaises(RuntimeError) as error:
                    qualification.main()
            self.assertIs(error.exception, stopped)
            qualify.assert_called_once_with(Path('/requested/ffmpeg'), Path('/requested/ffprobe'))
            self.assertIs(generate.call_args.args[0], pair)

    def test_pin_failure_removes_stale_report_without_creating_work(self):
        import sys
        import tempfile
        import test_oracle
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            out, work = root / 'report.json', root / 'work'
            out.write_text('stale')
            argv = ['aac_pcm_eligibility', '--coreav', 'coreav', '--oracle-dir', '/requested',
                    '--work', str(work), '--out', str(out), '--pns-diagnostic', 'diagnostic']
            with mock.patch.object(sys, 'argv', argv), \
                    mock.patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('private pin detail')), \
                    mock.patch.object(qualification, 'generate') as generate, \
                    mock.patch.object(qualification, 'run') as run:
                with self.assertRaisesRegex(ValueError, '^unverified pinned oracle$'):
                    qualification.main()
            generate.assert_not_called()
            run.assert_not_called()
            self.assertFalse(out.exists())
            self.assertFalse(work.exists())


if __name__ == '__main__':
    unittest.main()
