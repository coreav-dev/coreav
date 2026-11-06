import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / 'opus_matroska_padding.py'
SPEC = importlib.util.spec_from_file_location('opus_matroska_padding', SCRIPT)
padding = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(padding)
HEAD = b'OpusHead\x01\x01\0\0' + struct.pack('<I', 48000) + b'\0\0\0'


def page(body, lengths, flags=0, granule=0):
    header = bytearray(27)
    header[:4] = b'OggS'
    header[5] = flags
    struct.pack_into('<Q', header, 6, granule)
    header[26] = len(lengths)
    return bytes(header) + bytes(lengths) + body


class OpusMatroskaPadding(unittest.TestCase):
    def test_oracle_diagnostics_are_consumed_before_work_is_created(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory) / 'cases'
            with mock.patch.object(padding.test_oracle, 'require',
                    side_effect=padding.test_oracle.QualificationError('wrong oracle version')), \
                    mock.patch.object(padding, 'run', side_effect=AssertionError('unexpected process')) as launch:
                with self.assertRaisesRegex(ValueError, 'unverified oracle'):
                    padding.check(Path('native'), Path('oracle'), work)
            launch.assert_not_called()
            self.assertFalse(work.exists())

    def test_ogg_packet_continuation_and_eos_supply_known_presentation(self):
        packet = b'\xf8' + b'\0' * 255
        data = page(HEAD, [len(HEAD)], flags=2) + page(b'OpusTags', [8]) \
            + page(packet[:255], [255]) + page(packet[255:], [1], flags=5, granule=960)
        head, packets, eos = padding.ogg_packets(data)
        self.assertEqual((head, packets, eos), (HEAD, [packet], 960))
        self.assertEqual(padding.packet_samples(packet), 960)
        for invalid in [data[:-1], data[:27], b'not an Ogg page']:
            with self.assertRaises(ValueError):
                padding.ogg_packets(invalid)

    def test_both_discard_padding_signs_are_serialized_without_changing_packets(self):
        packet = b'\xf8\xff\xfe'
        for ns in [-1000000, 1000000]:
            data = padding.matroska(HEAD, [packet], 0, {0: ns})
            self.assertIn(packet, data)
            self.assertIn(b'\x75\xa2\x88' + ns.to_bytes(8, 'big', signed=True), data)

    def test_duration_model_covers_each_mode_and_frame_count_code(self):
        for packet in [b'\x48', b'\x78', b'\xf8', b'\xf1', b'\xe3\x08']:
            self.assertEqual(padding.packet_samples(packet), 960)

    def test_script_and_report_are_in_conformance_ci(self):
        workflow = (SCRIPT.parent.parent / '.github/workflows/audio-conformance.yml').read_text()
        self.assertEqual(workflow.count('"scripts/opus_matroska_padding.py"'), 2)
        self.assertIn('python3 -I scripts/opus_matroska_padding.py --coreav target/debug/coreav', workflow)
        self.assertIn('${{ runner.temp }}/opus-matroska-padding.json', workflow)



class OracleSelection(unittest.TestCase):
    def test_first_execution_uses_the_owner_returned_program(self):
        import test_oracle
        pair = test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        stopped = RuntimeError('stop before media execution')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with mock.patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                    mock.patch.object(padding, 'run', side_effect=stopped) as run:
                with self.assertRaises(RuntimeError) as error:
                    padding.check(Path('native'), Path('/requested'), root / 'work')
            self.assertIs(error.exception, stopped)
            qualify.assert_called_once_with(Path('/requested/ffmpeg'), Path('/requested/ffprobe'))
            self.assertEqual(run.call_args.args[0][0], pair.ffmpeg)


if __name__ == '__main__':
    unittest.main()
