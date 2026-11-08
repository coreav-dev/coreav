import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import native_h264_cli as cli


class NativeCli(unittest.TestCase):
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
        for event in ('push:', 'pull_request:'):
            selected = workflow.split('  ' + event, 1)[1].split('  workflow_dispatch:', 1)[0]
            for path in ('crates/coreav-cli/**', 'crates/coreav-transcode/**', 'scripts/native_h264_cli.py'):
                self.assertIn(path, selected)


if __name__ == '__main__':
    unittest.main()
