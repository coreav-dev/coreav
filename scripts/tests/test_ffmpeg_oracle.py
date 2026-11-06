"""Exercise the development observer's actual oracle helper without media."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / 'crates/coreav-cli/tests/ffmpeg.rs'


class DevelopmentOracle(unittest.TestCase):
    def test_ci_rejects_fake_version_and_missing_oracle(self):
        helpers = re.findall(r'^fn oracle\(\) -> Option<\(PathBuf, PathBuf\)> \{\n.*?^\}',
                             DRIVER.read_text(), re.MULTILINE | re.DOTALL)
        self.assertEqual(len(helpers), 1)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'helper.rs'
            source.write_text('use std::path::PathBuf;\n'
                              f'#[path = {json.dumps(str(ROOT / "test-support/oracle.rs"))}] mod test_oracle;\n'
                              + helpers[0] + '\nfn main() {\n'
                              'let _ = oracle().expect("oracle helper skipped");\n'
                              'println!("oracle accepted");\n}\n')
            binary = root / 'helper'
            compiled = subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(binary)],
                                      capture_output=True, text=True, timeout=60)
            self.assertEqual(compiled.returncode, 0, compiled.stderr)
            pair = root / 'fake'
            pair.mkdir()
            for name in ('ffmpeg', 'ffprobe'):
                path = pair / name
                path.write_text(f'#!/bin/sh\nprintf "%s\\n" "{name} version 0.0.0"\n')
                path.chmod(0o700)
            for directory in (pair, root / 'missing'):
                with self.subTest(selection=directory.name):
                    env = dict(os.environ, CI='1', COREAV_FFMPEG=str(directory / 'ffmpeg'),
                               COREAV_FFPROBE=str(directory / 'ffprobe'), RUST_BACKTRACE='0')
                    result = subprocess.run([str(binary)], env=env, capture_output=True,
                                            text=True, timeout=30)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn('stage=oracle_qualification', result.stderr)
                    self.assertNotIn('oracle accepted', result.stdout)

    def test_development_helper_import_binds_the_shared_owner(self):
        modules = dict((name, path) for path, name in re.findall(
            r'#\[path\s*=\s*"([^"]+)"\]\s*mod\s+(\w+)\s*;', DRIVER.read_text()))
        self.assertIn('test_oracle', modules)
        self.assertEqual((DRIVER.parent / modules['test_oracle']).resolve(),
                         ROOT / 'test-support/oracle.rs')
