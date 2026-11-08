import pathlib
import re
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class HevcSimdWorkflow(unittest.TestCase):
    def test_both_full_comparisons_honor_forced_scalar_without_changing_membership(self):
        workflow = (ROOT / '.github/workflows/hevc-conformance.yml').read_text()
        self.assertIn('scalar:\n        description: Force scalar kernels', workflow)
        for name in ('hevc-vectors', 'profile-census'):
            job = re.split(r'\n  [a-z][\w-]*:\n', workflow.split('  ' + name + ':\n', 1)[1], maxsplit=1)[0]
            self.assertIn("--features native-hevc${{ inputs.scalar && ',scalar' || '' }}", job)
        self.assertIn('--features native-hevc,scalar --all-targets', workflow)

    def test_differential_campaign_is_bounded_and_covers_both_architectures(self):
        workflow = (ROOT / '.github/workflows/hevc-conformance.yml').read_text()
        job = workflow.split('  kernel-differential:\n', 1)[1]
        self.assertIn("if: github.event_name == 'workflow_dispatch' && !inputs.scalar", job)
        self.assertIn('os: [ubuntu-24.04, macos-15]', job)
        self.assertIn('grep -qw avx2 /proc/cpuinfo', job)
        for target in ('horizontal', 'vertical', 'sao'):
            self.assertIn('crates/coreav-kernels-hevc/fuzz ' + target, job)
        self.assertIn('600 "$RUNNER_TEMP/fuzz"', job)
        self.assertIn('scripts/fuzz_upload.py "$RUNNER_TEMP/fuzz/findings"', job)
        self.assertNotIn('upload-artifact', job)
        census = workflow.split('  profile-census:\n', 1)[1].split('\n  kernel-differential:', 1)[0]
        self.assertIn('name: hevc-profile-summary-${{ matrix.shard }}', census)
