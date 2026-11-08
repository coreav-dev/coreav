import pathlib
import os
import subprocess
import sys
import textwrap
import unittest
from unittest.mock import patch

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import hevc_inverse_reference as inverse


class InverseReference(unittest.TestCase):
    def setUp(self):
        self.oracle = inverse.reference.test_oracle.Oracle(
            pathlib.Path('/qualified/ffmpeg'), pathlib.Path('/qualified/ffprobe'))
        self.expected = dict(depth=8, frames=2, output_md5='same')

    def test_pixel_verification_precedes_every_call_and_rotates_all_three_arms(self):
        order = []

        def verify(binary, oracle, source):
            self.assertEqual(oracle, self.oracle)
            order.append(str(binary))
            return self.expected

        def command(args):
            self.assertEqual(order, ['/baseline', '/candidate'])
            stdout = b'frame=2\n' if args[0] == self.oracle.ffmpeg else \
                b'{"pictures":2,"decoded_pictures":2,"bins":42}'
            return stdout, dict(wall=1.0, user=0.5, system=0.1)

        with patch.object(inverse.reference, 'verify', side_effect=verify), \
                patch.object(inverse.reference, 'command', side_effect=command):
            row = inverse.compare('/baseline', '/candidate', self.oracle, '/input', 5)
        self.assertEqual(row['output_md5'], 'same')
        self.assertEqual(len(row['calls']), 15)
        self.assertEqual([c['arm'] for c in row['calls'][:9]], [
            'baseline', 'candidate', 'ffmpeg_one',
            'candidate', 'ffmpeg_one', 'baseline',
            'ffmpeg_one', 'baseline', 'candidate'])
        self.assertEqual({c['repeat'] for c in row['calls']}, set(range(5)))

    def test_output_mismatch_prevents_timing(self):
        with patch.object(inverse.reference, 'verify', side_effect=[self.expected,
                dict(self.expected, output_md5='changed')]), \
                patch.object(inverse.reference, 'command') as command:
            with self.assertRaisesRegex(ValueError, 'candidate_output_identity'):
                inverse.compare('/baseline', '/candidate', self.oracle, '/input', 5)
        command.assert_not_called()

    def test_missing_decode_work_is_fatal_even_when_process_succeeds(self):
        with patch.object(inverse.reference, 'verify', return_value=self.expected), \
                patch.object(inverse.reference, 'command', return_value=(
                    b'{"pictures":1,"decoded_pictures":1,"bins":42}', {})):
            with self.assertRaisesRegex(ValueError, 'native_work'):
                inverse.compare('/baseline', '/candidate', self.oracle, '/input', 5)

    def test_workflow_admits_both_exact_arms_before_builds_or_credentials(self):
        text = (SCRIPTS.parent / '.github/workflows/hevc-inverse-reference.yml').read_text()
        admission = text.index('Admit exact repository commits')
        self.assertLess(text.index('path: baseline'), admission)
        self.assertLess(text.index('path: candidate'), admission)
        self.assertLess(admission, text.index('cargo build'))
        self.assertLess(admission, text.index('secrets.'))
        self.assertIn('for arm in baseline candidate', text)
        self.assertIn('branch -r --contains "$sha"', text)
        self.assertIn('vars.COREAV_CORPUS_R2_BUCKET', text)
        self.assertNotIn('secrets.COREAV_CORPUS_R2_BUCKET', text)

    def test_existing_dispatch_preserves_reference_and_threading_unless_inverse_is_selected(self):
        text = (SCRIPTS.parent / '.github/workflows/hevc-reference.yml').read_text()
        self.assertIn('if: inputs.inverse', text)
        self.assertIn("if: ${{ !inputs.inverse && inputs.baseline == '' && !inputs.sanitizer_only }}", text)
        self.assertIn("if: ${{ !inputs.inverse && (inputs.baseline != '' || inputs.sanitizer_only) }}", text)
        self.assertIn('uses: ./.github/workflows/hevc-inverse-reference.yml', text)
        for name in ('baseline', 'candidate', 'only'):
            self.assertIn(name + ': ${{ inputs.' + name + ' }}', text)
        reusable = (SCRIPTS.parent / '.github/workflows/hevc-inverse-reference.yml').read_text()
        self.assertIn('  workflow_call:', reusable)

    def test_dispatch_rejects_invalid_inputs_before_checkout_or_build(self):
        text = (SCRIPTS.parent / '.github/workflows/hevc-inverse-reference.yml').read_text()
        validation = text.index('      - name: Validate dispatch inputs')
        self.assertLess(validation, text.index('actions/checkout@'))
        self.assertLess(validation, text.index('cargo build'))
        self.assertLess(validation, text.index('secrets.'))
        script = text[validation:].split('        run: |\n', 1)[1].split('      - uses:', 1)[0]
        script = textwrap.dedent(script)
        valid = dict(BASELINE='a' * 40, CANDIDATE='b' * 40, ONLY='c00,d25')
        for name, value in [('BASELINE', 'a' * 39), ('CANDIDATE', 'g' * 40),
                            ('ONLY', ''), ('ONLY', 'c04'), ('ONLY', 'd26'),
                            ('ONLY', 'c00,'), ('ONLY', 'c00, d01')]:
            with self.subTest(name=name, value=value):
                result = subprocess.run(['bash', '-c', script], capture_output=True,
                                        env={**os.environ, **valid, name: value}, timeout=5)
                self.assertNotEqual(result.returncode, 0)
        for only in [f'c{i:02}' for i in range(4)] + [f'd{i:02}' for i in range(26)] + ['c00,d25,c00']:
            result = subprocess.run(['bash', '-c', script], capture_output=True,
                                    env={**os.environ, **valid, 'ONLY': only}, timeout=5)
            self.assertEqual(result.returncode, 0, only)


if __name__ == '__main__':
    unittest.main()
