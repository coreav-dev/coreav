"""AVI comparisons follow the shared MP3 module and encoder transport."""
import fnmatch
from pathlib import Path
import re
import unittest


class AviWorkflow(unittest.TestCase):
    def test_module_and_transport_changes_run_the_avi_gate(self):
        root = Path(__file__).resolve().parents[2]
        text = (root / '.github/workflows/avi-output.yml').read_text()
        patterns = re.findall(r'^      - (\S+)$', text, re.M)
        for path in ('crates/coreav-transcode/src/lib.rs', 'crates/coreav-transcode/src/mp3.rs',
                     'crates/coreav-transcode/src/encoder.rs', 'crates/coreav-cli/tests/avi.rs'):
            self.assertTrue(any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns), path)

    def test_supplemental_input_changes_generate_and_record_the_same_files(self):
        root = Path(__file__).resolve().parents[2]
        text = (root / '.github/workflows/avi-output.yml').read_text()
        patterns = re.findall(r'^      - (\S+)$', text, re.M)
        for path in ('scripts/gen-v2-fixtures.py', 'scripts/v2_fixture_inputs.py',
                     'tools/compat-corpus/v2-fixtures.json', 'tools/compat-corpus/encode_breadth.py'):
            self.assertTrue(any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns), path)
        self.assertIn('scripts/gen-v2-fixtures.py --ffmpeg', text)
        self.assertIn('--out "$RUNNER_TEMP/avi-fixtures"', text)
        runner = (root / 'scripts/avi_v2_check.py').read_text()
        self.assertIn('for name in v2.INPUT_FILES:', runner)
        self.assertIn('for k in v2.INPUT_FILES:', runner)
        self.assertIn("'fixture_sha256': fixture_hashes", runner)


if __name__ == '__main__':
    unittest.main()
