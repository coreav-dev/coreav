"""Exercise the native SVT licence gate without compiling or executing native code."""

import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('svt_av1_licenses', ROOT / 'scripts/svt_av1_licenses.py')
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
RECIPE = runpy.run_path(str(gate.RECIPE))
FIXTURES = ROOT / 'scripts/tests/fixtures/svt-av1-notices'


class NativeLicenses(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.prefix = self.root / 'native installation'
        self.prefix.mkdir()
        self.target = next(iter(RECIPE['TARGETS']))
        self.manifest = {
            'target': self.target, 'version': RECIPE['SVT_VERSION'],
            'archive_sha256': RECIPE['ARCHIVE_SHA256'], 'sha256': {},
        }
        for name in ('lib/libSvtAv1Enc.a', 'bin/svt-av1-version',
                     'include/svt-av1/EbSvtAv1Enc.h'):
            path = self.prefix / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'nonexecutable fixture\n')
            self.record(path)
        for name in RECIPE['NOTICES'].values():
            path = self.prefix / 'share/licenses/svt-av1' / name
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(FIXTURES / name, path)
            self.record(path)
        self.write_manifest()

    def record(self, path):
        self.manifest['sha256'][path.relative_to(self.prefix).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()

    def write_manifest(self):
        (self.prefix / 'manifest.json').write_text(json.dumps(self.manifest), encoding='utf-8')

    def test_exact_legal_fixtures_pass_for_every_recipe_target(self):
        for target in RECIPE['TARGETS']:
            with self.subTest(target=target):
                self.manifest['target'] = target
                self.write_manifest()
                result = gate.check(self.prefix, target)
                self.assertEqual(result['component'], 'SVT-AV1')
                self.assertEqual(result['license'], 'BSD-3-Clause-Clear')
                self.assertEqual(result['version'], RECIPE['SVT_VERSION'])
                self.assertEqual(result['target'], target)
                self.assertEqual(len(result['notices']), len(RECIPE['NOTICES']))

    def test_wrong_identity_or_unsupported_target_fails(self):
        for field in ('target', 'version', 'archive_sha256'):
            with self.subTest(field=field):
                original = self.manifest[field]
                self.manifest[field] = 'wrong'
                self.write_manifest()
                with self.assertRaisesRegex(gate.LicenseError, 'identity'):
                    gate.check(self.prefix, self.target)
                self.manifest[field] = original
        self.write_manifest()
        with self.assertRaisesRegex(gate.LicenseError, 'unsupported'):
            gate.check(self.prefix, 'unsupported-target')
        with self.assertRaisesRegex(gate.LicenseError, 'absolute'):
            gate.check(Path('relative'), self.target)

    def test_each_replaced_notice_fails_even_with_a_rewritten_manifest(self):
        for name in RECIPE['NOTICES'].values():
            with self.subTest(name=name):
                path = self.prefix / 'share/licenses/svt-av1' / name
                original = path.read_bytes()
                for replacement in (b'', b'BSD-3-Clause-Clear\n', original.replace(b'\n', b'\r\n')):
                    path.write_bytes(replacement)
                    self.record(path)
                    self.write_manifest()
                    with self.assertRaisesRegex(gate.LicenseError, 'approved upstream text'):
                        gate.check(self.prefix, self.target)
                path.write_bytes(original)
                self.record(path)
                self.write_manifest()

    def test_each_missing_notice_and_extra_text_fail(self):
        for name in RECIPE['NOTICES'].values():
            with self.subTest(name=name):
                path = self.prefix / 'share/licenses/svt-av1' / name
                original = path.read_bytes()
                path.unlink()
                with self.assertRaises(gate.LicenseError):
                    gate.check(self.prefix, self.target)
                path.write_bytes(original)
        (self.prefix / 'share/licenses/svt-av1/unreviewed.txt').write_text('extra')
        with self.assertRaisesRegex(gate.LicenseError, 'approved roster'):
            gate.check(self.prefix, self.target)

    def test_retained_file_mutation_and_omitted_manifest_entries_fail(self):
        for name in ('lib/libSvtAv1Enc.a', 'bin/svt-av1-version',
                     'include/svt-av1/EbSvtAv1Enc.h'):
            with self.subTest(name=name):
                path = self.prefix / name
                original = path.read_bytes()
                path.write_bytes(b'changed')
                with self.assertRaisesRegex(gate.LicenseError, 'manifest hash'):
                    gate.check(self.prefix, self.target)
                path.write_bytes(original)
                value = self.manifest['sha256'].pop(name)
                self.write_manifest()
                with self.assertRaisesRegex(gate.LicenseError, 'roster'):
                    gate.check(self.prefix, self.target)
                self.manifest['sha256'][name] = value
                self.write_manifest()

    def test_symlinked_notice_directory_and_prefix_fail(self):
        notices = self.prefix / 'share/licenses/svt-av1'
        outside = self.root / 'outside'
        notices.rename(outside)
        notices.symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(gate.LicenseError, 'regular'):
            gate.check(self.prefix, self.target)
        alias = self.root / 'alias'
        alias.symlink_to(self.prefix, target_is_directory=True)
        with self.assertRaisesRegex(gate.LicenseError, 'nonsymlink'):
            gate.check(alias, self.target)

    def test_symlinked_notice_file_and_manifest_fail(self):
        notice = self.prefix / 'share/licenses/svt-av1/LICENSE.md'
        outside = self.root / 'outside'
        notice.rename(outside)
        notice.symlink_to(outside)
        with self.assertRaisesRegex(gate.LicenseError, 'regular'):
            gate.check(self.prefix, self.target)
        notice.unlink()
        outside.rename(notice)
        manifest = self.prefix / 'manifest.json'
        manifest.rename(outside)
        manifest.symlink_to(outside)
        with self.assertRaisesRegex(gate.LicenseError, 'regular'):
            gate.check(self.prefix, self.target)

    @unittest.skipUnless(hasattr(os, 'mkfifo'), 'requires Unix FIFOs')
    def test_fifo_manifest_or_notice_is_refused_without_reading(self):
        args = [sys.executable, '-I', str(ROOT / 'scripts/svt_av1_licenses.py'),
                '--prefix', str(self.prefix), '--target', self.target]
        for relative in ('manifest.json', 'share/licenses/svt-av1/PATENTS.md'):
            with self.subTest(relative=relative):
                path = self.prefix / relative
                original = path.read_bytes()
                path.unlink()
                os.mkfifo(path)
                result = subprocess.run(args, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn('regular', result.stderr)
                path.unlink()
                path.write_bytes(original)

    def test_malformed_duplicate_and_oversized_json_fail(self):
        valid = json.dumps(self.manifest)
        duplicates = valid.replace('"target":', '"target": "wrong", "target":', 1)
        nested = valid.replace('"sha256": {', '"sha256": {"lib/libSvtAv1Enc.a": "wrong",', 1)
        for text in ('{', 'null', '[]', '{}', duplicates, nested,
                     '[' * 2000 + '0' + ']' * 2000, ' ' * (256 * 1024 + 1)):
            with self.subTest(text=text[:32]):
                (self.prefix / 'manifest.json').write_text(text)
                with self.assertRaises(gate.LicenseError):
                    gate.check(self.prefix, self.target)

    def test_invalid_hash_rosters_never_supply_file_paths(self):
        original = copy.deepcopy(self.manifest['sha256'])
        for hashes in ([], None, {'../../outside': '0' * 64},
                       {**original, 'lib/libSvtAv1Enc.a': None},
                       {**original, 'lib/libSvtAv1Enc.a': 'invalid'}):
            with self.subTest(hashes=hashes):
                self.manifest['sha256'] = hashes
                self.write_manifest()
                with self.assertRaisesRegex(gate.LicenseError, 'roster'):
                    gate.check(self.prefix, self.target)

    def test_stale_or_incomplete_trusted_fingerprints_fail(self):
        original = json.loads(gate.POLICY.read_text())
        stale = {**original, 'archive_sha256': '0' * 64}
        incomplete = {**original, 'notices': {}}
        invalid = {**original, 'notices': {key: 'invalid' for key in original['notices']}}
        policy = self.root / 'policy.json'
        for data in (stale, incomplete, invalid, {**original, 'component': 'other'}):
            with self.subTest(data=data):
                policy.write_text(json.dumps(data))
                with patch.object(gate, 'POLICY', policy), self.assertRaises(gate.LicenseError):
                    gate.check(self.prefix, self.target)

    def test_command_records_only_native_identity_and_fails_cleanly(self):
        args = [sys.executable, '-I', str(ROOT / 'scripts/svt_av1_licenses.py'),
                '--prefix', str(self.prefix), '--target', self.target]
        result = subprocess.run(args, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), gate.check(self.prefix, self.target))
        self.assertNotIn(str(self.prefix), result.stdout)
        (self.prefix / 'manifest.json').write_text('not-json')
        result = subprocess.run(args, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertIn('invalid native licence JSON', result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        self.assertEqual(result.stdout, '')


if __name__ == '__main__':
    unittest.main()
