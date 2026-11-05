import importlib.util
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('summary_workflow_check', ROOT / 'scripts/workflow_check.py')
workflow_check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(workflow_check)


class InputSummaryWorkflow(unittest.TestCase):
    def test_default_cli_build_inputs_trigger_summary_checks(self):
        workflow = workflow_check.workflow_yaml((ROOT / '.github/workflows/input-summary.yml').read_text())
        crates = (
            'audio', 'avi', 'cli', 'decode', 'frame', 'isobmff', 'isobmff-write',
            'kernels-audio', 'mkv', 'mpegts', 'opus', 'policy', 'range', 'text',
        )
        required = {'Cargo.toml', 'Cargo.lock'} | {f'crates/coreav-{name}/**' for name in crates}
        paths = set(workflow['on']['pull_request']['paths'])
        self.assertEqual(sorted(required - paths), [],
                         'Add the missing default CLI build inputs to input-summary.yml pull_request.paths')
