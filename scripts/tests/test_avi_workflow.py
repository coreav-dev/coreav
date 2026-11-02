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


if __name__ == '__main__':
    unittest.main()
