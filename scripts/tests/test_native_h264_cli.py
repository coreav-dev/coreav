import pathlib
from fractions import Fraction
import struct
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import native_h264_cli as cli


class NativeCli(unittest.TestCase):
    def test_terminal_duration_changes_only_the_last_generated_sample(self):
        def box(kind, body):
            return struct.pack('>I4s', len(body) + 8, kind) + body

        def movie(payload):
            for kind in (b'stbl', b'minf', b'mdia', b'trak', b'moov'):
                payload = box(kind, payload)
            return box(b'mdat', b'unchanged encoded pictures') + payload

        for count in (1, 4):
            original = movie(box(b'stts', struct.pack('>IIII', 0, 1, count, 512)))
            for duration in (0, 1024):
                pairs = (1, count, duration) if count == 1 else (2, count - 1, 512, 1, duration)
                expected = movie(box(b'stts', struct.pack('>' + 'I' * (len(pairs) + 1), 0, *pairs)))
                self.assertEqual(cli.terminal_duration(original, duration), expected)

    def test_explicit_sps_sample_aspect_rejects_zero_terms_with_a_label(self):
        def sps(width, height):
            bits = (f'{0x42c033:024b}' + '110110100111101111111111'
                    + f'{width:016b}{height:016b}' + '0110101'
                    + f'{0x010101:024b}' + '1111' + f'{1:032b}{60:032b}' + '100001')
            bits += '0' * (-len(bits) % 8)
            escaped = bytearray()
            for value in int(bits, 2).to_bytes(len(bits) // 8, 'big'):
                if escaped[-2:] == b'\0\0' and value <= 3:
                    escaped.append(3)
                escaped.append(value)
            return b'\x67' + escaped

        self.assertEqual(cli.sps_metadata(sps(17, 13)), (Fraction(30), Fraction(17, 13)))
        for width, height in ((0, 1), (1, 0), (0, 0)):
            with self.subTest(width=width, height=height):
                with self.assertRaisesRegex(ValueError, 'zero SPS sample aspect'):
                    cli.sps_metadata(sps(width, height))

    def test_sample_aspect_reader_uses_the_visual_entry_structure(self):
        def box(kind, body):
            return struct.pack('>I4s', len(body) + 8, kind) + body

        for ratio in (None, (16, 15), (65535, 65534)):
            body = bytes(78) + (box(b'pasp', struct.pack('>II', *ratio)) if ratio else b'')
            body = box(b'stsd', bytes(4) + struct.pack('>I', 1) + box(b'avc1', body))
            for kind in (b'stbl', b'minf', b'mdia', b'trak', b'moov'):
                body = box(kind, body)
            data = box(b'mdat', box(b'pasp', struct.pack('>II', 7, 9))) + body
            actual = cli.entry_aspect(data)
            self.assertEqual(struct.unpack_from('>II', data, actual[0]) if actual else None, ratio)
        with self.assertRaisesRegex(ValueError, 'missing generated video sample entry'):
            cli.entry_aspect(box(b'mdat', box(b'pasp', struct.pack('>II', 7, 9))))

    def test_oracle_annex_b_accepts_both_standard_delimiter_lengths(self):
        self.assertEqual(cli.annexb_units(b'\0\0\0\1\x67\x80\0\0\1\x68\x80\0\0\0\1\x65\x80'),
                         [b'\x67\x80', b'\x68\x80', b'\x65\x80'])
        for data in (b'', b'no delimiter', b'\0\0\1', b'\0\0\1\0\0\1\x65\x80'):
            with self.assertRaises(ValueError):
                cli.annexb_units(data)

    def test_byte_comparator_rejects_a_changed_output(self):
        cli.same(b'complete', b'complete', 'output')
        with self.assertRaisesRegex(ValueError, 'output differs'):
            cli.same(b'complete', b'changed', 'output')

    def test_workflow_builds_actual_base_and_candidate_modes(self):
        workflow = (ROOT / '.github/workflows/h264-encoder.yml').read_text()
        job = workflow.split('  native-h264-cli:', 1)[1].split('  h264-encoder-conformance:', 1)[0]
        for required in ('github.event.pull_request.base.sha || github.sha',
                         'cd ../baseline', 'default-base', 'external-base', 'default-candidate',
                         '--features native-h264-encoder,native-h264', '--features h264-native',
                         '--features full --example p', 'scripts/native_h264_cli.py',
                         'hostile_encoders_end_in_an_error_and_leave_nothing -- --exact',
                         '--test docs_site --test strict_safe_limit --test ffprobe_limits',
                         "for features in '' release-default release-full"):
            self.assertIn(required, job)
        base_step = job.split('- name: Preserve base-build byte references', 1)[1].split('- name:', 1)[0]
        self.assertIn('cd ../baseline', base_step)
        self.assertIn('cargo build --locked --release -p coreav-cli --features native-h264-encoder,native-h264\n'
                      '          cp target/release/coreav "$RUNNER_TEMP/native-base"', base_step)
        self.assertIn('--baseline-native "$RUNNER_TEMP/native-base"', job)
        for event in ('push:', 'pull_request:'):
            selected = workflow.split('  ' + event, 1)[1].split('  workflow_dispatch:', 1)[0]
            for path in ('crates/coreav-cli/**', 'crates/coreav-transcode/**', 'scripts/native_h264_cli.py',
                         'scripts/track_boxes.py'):
                self.assertIn(path, selected)

    def test_cli_oracles_install_nasm_before_x264(self):
        workflow = (ROOT / '.github/workflows/h264-encoder.yml').read_text()
        job = workflow.split('  native-h264-cli:', 1)[1].split('  h264-encoder-conformance:', 1)[0]
        self.assertLess(job.index('apt-get install -y --no-install-recommends nasm'),
                        job.index('scripts/install-x264.sh'))
        for stage in ('Installing pinned FFmpeg', 'Installing pinned x264',
                      'reason=pinned_ffmpeg_install_failed', 'reason=pinned_x264_install_failed'):
            self.assertIn(stage, job)


if __name__ == '__main__':
    unittest.main()
