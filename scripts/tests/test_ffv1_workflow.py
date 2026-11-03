"""The existing required CI gate runs the complete FFV1 oracle comparison."""

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import workflow_check


class Ffv1Workflow(unittest.TestCase):
    def test_required_check_runs_fixtures_mutations_and_native_cli_oracle_gate(self):
        root = Path(__file__).resolve().parents[2]
        lines = (root / '.github/workflows/ci.yml').read_text().splitlines()
        start = lines.index('  check:')
        stop = next(index for index in range(start + 1, len(lines))
                    if lines[index].startswith('  ') and not lines[index].startswith('   ') and lines[index].strip())
        check = '\n'.join(lines[start:stop])
        gate = check.index('- name: Require exact native FFV1 pixels and probe fields')
        self.assertLess(check.index('scripts/oracle_pin.py --verify-bin'), gate)
        step = check[gate:].split('\n      - ', 1)[0]
        self.assertNotIn('\n        if:', step)
        self.assertNotIn('continue-on-error', step)
        self.assertIn('timeout-minutes:', step)
        for command in ("-p 'test_ffv1*.py'", 'cargo run --locked -p coreav-ffv1 --example fixtures --',
                        'cargo build --locked -p coreav-cli --features native-h264', 'scripts/ffv1_check.py',
                        '--ffmpeg', '--ffprobe', '--coreav', '--fixtures', '--work'):
            self.assertIn(command, step)
        jobs = workflow_check.jobs_of(lines)
        self.assertIn('check', jobs['ci-report']['needs'])
        self.assertTrue(jobs['ci-report']['always'])


if __name__ == '__main__':
    unittest.main()
