"""Exercise actual FFmpeg test helpers without media."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / 'crates/coreav-cli/tests/ffmpeg.rs'
HELPERS = ((DRIVER, 'oracle'),
           (ROOT / 'crates/coreav-cli/tests/ffmpeg_concat.rs', 'oracle'),
           (ROOT / 'crates/coreav-cli/tests/ffmpeg_mpegts.rs', 'tools'))
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin


class DevelopmentOracle(unittest.TestCase):
    def test_ci_rejects_fake_version_and_missing_oracle(self):
        for driver, function in HELPERS:
            with self.subTest(helper=driver.name), tempfile.TemporaryDirectory() as temporary:
                helpers = re.findall(r'^fn ' + function + r'\(\) -> Option<\(PathBuf, PathBuf\)> \{\n.*?^\}',
                                     driver.read_text(), re.MULTILINE | re.DOTALL)
                self.assertEqual(len(helpers), 1)
                root = Path(temporary)
                source = root / 'helper.rs'
                source.write_text('use std::path::PathBuf;\n'
                                  f'#[path = {json.dumps(str(ROOT / "test-support/oracle.rs"))}] mod test_oracle;\n'
                                  + helpers[0] + '\nfn main() {\n'
                                  f'let _ = {function}().expect("oracle helper skipped");\n'
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
        for driver, _ in HELPERS:
            with self.subTest(helper=driver.name):
                modules = dict((name, path) for path, name in re.findall(
                    r'#\[path\s*=\s*"([^"]+)"\]\s*mod\s+(\w+)\s*;', driver.read_text()))
                self.assertIn('test_oracle', modules)
                self.assertEqual((driver.parent / modules['test_oracle']).resolve(),
                                 ROOT / 'test-support/oracle.rs')

    def test_wav_qualification_rejects_untrusted_pair_before_fixture_work(self):
        driver = ROOT / 'crates/coreav-cli/tests/ffmpeg_wav.rs'
        split = driver.read_text().split('fn aac_s16_wav_forms_match_complete_pinned_outputs_and_retain_native_metadata_cases() {', 1)
        self.assertEqual(len(split), 2)
        split = split[1].split('    let dir = scratch(', 1)
        self.assertEqual(len(split), 2)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'wav.rs'
            source.write_text('use std::path::{Path, PathBuf};\nuse std::process::Command;\n'
                              f'#[path = {json.dumps(str(ROOT / "test-support/oracle.rs"))}] mod test_oracle;\n'
                              'fn main() {\n' + split[0] + 'println!("oracle accepted");\n}\n')
            binary = root / 'wav'
            env = dict(os.environ, CARGO_MANIFEST_DIR=str(ROOT / 'crates/coreav-cli'))
            compiled = subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(binary)],
                                      env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(compiled.returncode, 0, compiled.stderr)
            banner = oracle_pin.load()['BUILD'] if sys.platform == 'darwin' else '0.0.0'
            for name in ('ffmpeg', 'ffprobe'):
                executable = root / name
                executable.write_text(f'#!/bin/sh\nprintf "%s\\n" "{name} version {banner}"\n')
                executable.chmod(0o700)
            env.update(CI='1', COREAV_FFMPEG=str(root / 'ffmpeg'),
                       COREAV_FFPROBE=str(root / 'ffprobe'), RUST_BACKTRACE='0')
            result = subprocess.run([str(binary)], env=env, capture_output=True, text=True, timeout=30)
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn('stage=oracle_qualification', result.stderr)
            self.assertNotIn('oracle accepted', result.stdout)
