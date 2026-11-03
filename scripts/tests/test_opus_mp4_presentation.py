import importlib.util
from pathlib import Path
import struct
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / 'opus_mp4_presentation.py'
SPEC = importlib.util.spec_from_file_location('opus_mp4_presentation', SCRIPT)
presentation = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(presentation)


def box(kind, data):
    return struct.pack('>I4s', len(data) + 8, kind) + data


def source():
    packet = b'\xf8\xff\xfe'
    media = box(b'mdat', packet * 6)
    edit = box(b'elst', b'\0' * 4 + struct.pack('>IIihh', 1, 4800, 312, 1, 0))
    config = box(b'dOps', b'\0\1' + struct.pack('>HIhB', 312, 48000, 0, 0))
    description = box(b'stsd', b'\0' * 4 + struct.pack('>I', 1)
                      + box(b'Opus', b'\0' * 28 + config))
    tables = description + box(b'stsz', b'\0' * 4 + struct.pack('>II', len(packet), 6)) \
        + box(b'stco', b'\0' * 4 + struct.pack('>II', 1, 8)) \
        + box(b'stsc', b'\0' * 4 + struct.pack('>4I', 1, 1, 6, 1))
    track = box(b'trak', box(b'edts', edit) + box(b'mdia',
                box(b'mdhd', b'\0' * 12 + struct.pack('>I', 48000))
                + box(b'minf', box(b'stbl', tables))))
    return media + box(b'moov', box(b'mvhd', b'\0' * 12 + struct.pack('>I', 48000)) + track)


class OpusMp4Presentation(unittest.TestCase):
    def test_metadata_driver_runs_in_audio_conformance_ci(self):
        workflow = (SCRIPT.parent.parent / '.github/workflows/audio-conformance.yml').read_text()
        self.assertEqual(workflow.count('"scripts/opus_mp4_presentation.py"'), 2)
        self.assertIn('python3 -I scripts/opus_mp4_presentation.py --coreav target/debug/coreav', workflow)
        self.assertIn('${{ runner.temp }}/opus-mp4-presentation.json', workflow)

    def test_source_metadata_owns_expected_length_independently_of_preskip(self):
        data = bytearray(source())
        info = presentation.metadata(data)
        self.assertEqual((info['start'], info['skip'], info['packet_samples']), (312, 312, 5760))
        struct.pack_into('>H', data, info['config'] + 2, 3840)
        changed = presentation.metadata(data)
        self.assertEqual(changed['packets'], info['packets'])
        self.assertEqual(changed['start'], info['start'])
        self.assertEqual(changed['skip'], 3840)
        self.assertEqual(struct.unpack_from('>I', data, changed['edit'] + 8)[0], 4800)

    def test_unsupported_generated_edit_and_packet_shapes_are_refused(self):
        data = bytearray(source())
        info = presentation.metadata(data)
        for offset, value in [(info['edit'] + 8, 4799), (info['edit'] + 12, 0xFFFFFFFF)]:
            changed = bytearray(data)
            struct.pack_into('>I', changed, offset, value)
            with self.assertRaises(ValueError):
                presentation.metadata(changed)
        data[8] = 0xE0
        with self.assertRaisesRegex(ValueError, '20 ms'):
            presentation.metadata(data)

    def test_wav_comparison_counts_complete_payload_not_header_bytes(self):
        data = b'RIFF' + struct.pack('<I', 16) + b'WAVEdata' + struct.pack('<I', 4) + b'1234'
        self.assertEqual(presentation.wav_samples(data), b'1234')
        with self.assertRaisesRegex(ValueError, 'incomplete'):
            presentation.wav_samples(data[:-1])


if __name__ == '__main__':
    unittest.main()
