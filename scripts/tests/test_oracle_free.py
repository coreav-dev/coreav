import json
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_free


class ExplicitSelection(unittest.TestCase):
    def test_roster_names_only_real_tests_not_helpers(self):
        roster = json.loads((ROOT / 'test-support/oracle-dependent.json').read_text())
        for file, names in roster.items():
            source = (ROOT / file).read_text()
            tests = set(re.findall(r'#\[test\](?:\s*#\[[^\]]+\])*\s*fn\s+(\w+)', source))
            if file.endswith('/sandboxed.rs'):
                tests = {'real encoders through the launcher'}
            for name in names:
                self.assertIn(name.rsplit('::', 1)[-1], tests, (file, name))

    def test_filter_is_explicit_and_exact_without_excluding_mock_helpers(self):
        args = oracle_free.command(['cargo', 'test', '--workspace'])
        self.assertIn('--exact', args)
        self.assertNotIn('ffmpeg', args)
        self.assertIn('convert::oracle::sample_conversion_is_exact_against_the_pinned_oracle', args)
        self.assertIn('--skip', args)
        with self.assertRaises(ValueError):
            oracle_free.command(['cargo', 'check'])


if __name__ == '__main__':
    unittest.main()
