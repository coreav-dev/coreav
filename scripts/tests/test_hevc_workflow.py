import json
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest


class HevcWorkflow(unittest.TestCase):
    def test_public_profile_projection_omits_per_file_records(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/hevc-conformance.yml').read_text()
        step = workflow.split('- name: Retain only aggregate profile counts publicly\n', 1)[1]
        script = textwrap.dedent(step.split("<<'PY'\n", 1)[1].split('\n          PY', 1)[0])
        with tempfile.TemporaryDirectory() as directory:
            temp = pathlib.Path(directory)
            (temp / 'private').mkdir()
            record = dict(roster_sha256='a' * 64, complete=True,
                          counts=[dict(partition='development', profile='Main', bit_depth=8,
                                       status='inspected', files=1)],
                          files=[dict(source='private-canary', url='private-canary')])
            source = temp / 'private/corpus-census.json'
            source.write_text(json.dumps(record))
            subprocess.run([sys.executable, '-I', '-', str(temp)], input=script, text=True,
                           check=True, capture_output=True, timeout=10)
            text = (temp / 'profile-summary.json').read_text()
            summary = json.loads(text)
            self.assertNotIn('private-canary', text)
            self.assertEqual(summary['files'], 1)
            self.assertEqual(set(summary), {'roster_sha256', 'complete', 'counts', 'files'})
            self.assertEqual(json.loads(source.read_text()), record)


if __name__ == '__main__':
    unittest.main()
