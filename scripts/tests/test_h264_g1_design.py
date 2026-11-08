import math
from pathlib import Path
import re
import subprocess
import sys
import unittest


ROOT = Path(__file__).resolve().parents[2]


class CoefficientDesignTests(unittest.TestCase):
    def assert_reference(self, text):
        section = text.split('### Stronger default coefficient budget\n', 1)[1].split('\n### ', 1)[0]
        source = (ROOT / 'crates/coreav-h264-encoder/src/levels.rs').read_text(encoding='utf-8')
        budget = re.search(r'const AC_COORDINATES: usize = (\d+);', source).group(1)
        self.assertIn(f'Measured ordinary-block budget: `{budget}`.', section, 'Record the measured tradeoff for the current coefficient budget')
        self.assertRegex(section, r'Measured candidate: `[0-9a-f]{40}`; direct baseline: `[0-9a-f]{40}`', 'Bind the documented tradeoff to both complete source identities')
        rows = [line.split('|')[1:-1] for line in section.splitlines() if re.match(r'^\| [A-E] \|', line)]
        self.assertEqual([row[0].strip() for row in rows], list('ABCDE'), 'Record every input, not just the throughput limit')
        for row in rows:
            self.assertEqual(len(row), 6, 'Record Y/U/V and both one/four-worker throughputs')
            for value, suffix in zip(row[1:], ('%', '%', '%', '×', '×')):
                value = value.strip()
                self.assertTrue(value.endswith(suffix))
                self.assertTrue(math.isfinite(float(value[:-1].replace('−', '-'))))
        self.assertIn('shared-host reference', section, 'Keep the shared-host qualification with the result')

    def test_current_budget_has_source_bound_measured_tradeoff(self):
        self.assert_reference((ROOT / 'docs/design/h264-encoder-pipeline.md').read_text(encoding='utf-8'))

    def test_missing_result_and_changed_budget_are_rejected(self):
        text = (ROOT / 'docs/design/h264-encoder-pipeline.md').read_text(encoding='utf-8')
        start = text.index('Measured ordinary-block budget:')
        end = text.index('Count the ceiling before implementing', start)
        missing = text[:start] + text[end:]
        changed = re.sub(r'Measured ordinary-block budget: `(\d+)`\.', lambda match: f'Measured ordinary-block budget: `{int(match.group(1)) + 1}`.', text, count=1)
        for invalid in (missing, changed):
            with self.subTest(missing=invalid == missing), self.assertRaises(AssertionError):
                self.assert_reference(invalid)

    def test_document_reads_do_not_use_locale_defaults(self):
        result = subprocess.run([
            sys.executable, '-X', 'warn_default_encoding', '-W', 'error::EncodingWarning',
            str(Path(__file__).resolve()),
            'CoefficientDesignTests.test_current_budget_has_source_bound_measured_tradeoff',
            'CoefficientDesignTests.test_missing_result_and_changed_budget_are_rejected',
        ], capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
