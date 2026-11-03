"""Test pinned decoder acquisition with synthetic archives and command outcomes."""

import importlib.util
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('build_dav1d_oracle', ROOT / 'scripts/build_dav1d_oracle.py')
BUILD = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILD
SPEC.loader.exec_module(BUILD)


def archive_bytes(entries=None):
    output = io.BytesIO()
    if entries is None:
        entries = [('dav1d-1.2.3/' + name, tarfile.REGTYPE, b'synthetic legal notice\n')
                   for name in ('COPYING', 'doc/PATENTS', 'src/ext/x86/x86inc.asm')]
    with tarfile.open(fileobj=output, mode='w:xz') as archive:
        for name, kind, content in entries:
            member = tarfile.TarInfo(name)
            member.type = kind
            member.size = len(content) if kind == tarfile.REGTYPE else 0
            member.linkname = 'outside'
            archive.addfile(member, io.BytesIO(content))
    return output.getvalue()


class DecoderBuild(unittest.TestCase):
    publication_failures = ('post-publication-bytes', 'post-publication-error',
                            'post-probe-wrong-version', 'post-probe-error', 'post-probe-interrupt',
                            'post-probe-bytes', 'destination-collision')

    def test_single_existing_distribution_pin(self):
        expected = BUILD.pin()
        self.assertEqual(set(expected), {'DAV1D_VERSION', 'DAV1D_SHA256'})
        with tempfile.TemporaryDirectory() as temporary:
            pin = Path(temporary) / 'pins.sh'
            for content in ('DAV1D_VERSION=1.2.3\n',
                            'DAV1D_VERSION=1.2.3\nDAV1D_VERSION=1.2.3\nDAV1D_SHA256=' + 'a' * 64,
                            'DAV1D_VERSION=unregistered\nDAV1D_SHA256=' + 'a' * 64):
                pin.write_text(content)
                with self.subTest(content=content), patch.object(BUILD, 'PIN', pin):
                    with self.assertRaisesRegex(ValueError, 'distribution pin'):
                        BUILD.pin()

    def test_hash_and_archive_boundary(self):
        for name, kind in (('../escape', tarfile.REGTYPE),
                           ('dav1d-1.2.3/../escape', tarfile.REGTYPE),
                           ('dav1d-1.2.3/link', tarfile.SYMTYPE),
                           ('dav1d-1.2.3/link', tarfile.LNKTYPE),
                           ('other/file', tarfile.REGTYPE)):
            with self.subTest(name=name, kind=kind), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive = root / 'source.tar.xz'
                archive.write_bytes(archive_bytes([(name, kind, b'untrusted')]))
                expected = {'DAV1D_VERSION': '1.2.3', 'DAV1D_SHA256': BUILD.digest(archive)}
                with self.assertRaisesRegex(ValueError, 'unsafe'):
                    BUILD.extract(archive, root / 'unpacked', expected)
                expected['DAV1D_SHA256'] = '0' * 64
                with self.assertRaisesRegex(ValueError, 'pin'):
                    BUILD.extract(archive, root / 'unpacked', expected)
                self.assertFalse((root / 'unpacked').exists())

    def run_build(self, mutation=None, temporary_parent=None):
        with tempfile.TemporaryDirectory(dir=temporary_parent) as temporary:
            root = Path(temporary).resolve()
            archive = root / 'source.tar.xz'
            archive.write_bytes(archive_bytes())
            expected = {'DAV1D_VERSION': '1.2.3', 'DAV1D_SHA256': BUILD.digest(archive)}
            calls = []
            prefix = root / 'published'
            original_publish = BUILD.PROCESS.publish

            def publish(stage, destination):
                if mutation == 'destination-collision':
                    destination.mkdir()
                    (destination / 'pre-existing').write_bytes(b'preserve this destination')
                original_publish(stage, destination)
                if mutation == 'post-publication-bytes':
                    (destination / 'bin/dav1d').write_bytes(b'changed after publication')
                elif mutation == 'post-publication-error':
                    raise OSError('synthetic failure after rename')

            def runner(command, environment, timeout, *, record):
                calls.append((command, environment, timeout))
                self.assertNotIn('UNRELATED_BUILD_SECRET', environment)
                record.update(pid=987654321, exit_code=0, timed_out=False,
                              stdout='synthetic compiler diagnostic\n', stderr='')
                if command[0] == 'ninja' and command[1:] != ['--version']:
                    binary = Path(command[-1]) / 'tools/dav1d'
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(b'synthetic executable; never invoked')
                    if mutation == 'command-failure':
                        record['exit_code'] = 1
                    if mutation == 'timeout':
                        record['timed_out'] = True
                    if mutation == 'pin-change':
                        expected['DAV1D_VERSION'] = '1.2.4'
                elif command[1:] == ['--version']:
                    record['stdout'] = 'synthetic toolchain 1.0'
                    if command[0].endswith('/dav1d'):
                        record['stdout'] = 'wrong' if mutation == 'wrong-version' else '1.2.3'
                        if mutation == 'executable-change':
                            Path(command[0]).write_bytes(b'changed during version probe')
                        if Path(command[0]).parent.parent == prefix:
                            if mutation == 'post-probe-wrong-version':
                                record['stdout'] = 'wrong final version'
                            elif mutation == 'post-probe-error':
                                raise BUILD.PROCESS.BuildError('synthetic final probe failure')
                            elif mutation == 'post-probe-interrupt':
                                raise KeyboardInterrupt('synthetic final probe interruption')
                            elif mutation == 'post-probe-bytes':
                                Path(command[0]).write_bytes(b'changed during final probe')
                elif command[0] == 'awk':
                    record['stdout'] = '' if mutation == 'missing-notice' else 'synthetic assembly notice\n'

            with patch.object(BUILD, 'pin', side_effect=lambda: dict(expected)), \
                    patch.object(BUILD.platform, 'system', return_value='Linux'), \
                    patch.object(BUILD.platform, 'machine', return_value='x86_64'), \
                    patch.object(BUILD.PROCESS, 'publish', side_effect=publish), \
                    patch.dict('os.environ', {'UNRELATED_BUILD_SECRET': 'synthetic-test-value'}):
                if mutation:
                    error = {'post-publication-error': OSError,
                             'post-probe-error': BUILD.PROCESS.BuildError,
                             'post-probe-interrupt': KeyboardInterrupt,
                             'destination-collision': BUILD.PROCESS.BuildError}.get(mutation, ValueError)
                    with self.assertRaises(error):
                        BUILD.build(prefix, archive, runner=runner)
                    if mutation == 'destination-collision':
                        self.assertEqual((prefix / 'pre-existing').read_bytes(), b'preserve this destination')
                        self.assertEqual(len(list(prefix.iterdir())), 1)
                    else:
                        self.assertFalse(prefix.exists())
                    self.assertEqual(list(root.glob('.dav1d-oracle-*')), [])
                    return
                result = BUILD.build(prefix, archive, runner=runner)
                manifest = json.loads((prefix / 'manifest.json').read_text())
                self.assertEqual(result['sha256'], BUILD.digest(prefix / 'bin/dav1d'))
                self.assertEqual(result['version'], expected['DAV1D_VERSION'])
                self.assertEqual(manifest['inputs']['DAV1D_SHA256'], BUILD.digest(archive))
                self.assertEqual(len(manifest['notices']), 3)
                self.assertNotIn('synthetic compiler diagnostic', json.dumps(manifest))
                self.assertTrue(all(timeout <= 180 for _, _, timeout in calls))
                with self.assertRaisesRegex(ValueError, 'newly absent'):
                    BUILD.build(prefix, archive, runner=runner)
                self.assertEqual(result['sha256'], BUILD.digest(prefix / 'bin/dav1d'))
                self.assertEqual(manifest, json.loads((prefix / 'manifest.json').read_text()))

    def test_build_is_owned_pinned_bounded_and_published_once(self):
        self.run_build()

    def test_failed_or_changed_builds_never_publish(self):
        for mutation in ('command-failure', 'timeout', 'pin-change', 'wrong-version',
                         'executable-change', 'missing-notice'):
            with self.subTest(mutation=mutation):
                self.run_build(mutation)

    def test_post_publication_failures_remove_only_this_builds_output(self):
        for mutation in self.publication_failures:
            with self.subTest(mutation=mutation):
                self.run_build(mutation)

    def test_post_publication_failures_through_a_symlinked_temporary_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            physical = root / 'physical'
            physical.mkdir()
            alias = root / 'alias'
            alias.symlink_to(physical, target_is_directory=True)
            self.assertNotEqual(alias, alias.resolve())
            self.run_build(temporary_parent=alias)
            for mutation in self.publication_failures:
                with self.subTest(mutation=mutation):
                    self.run_build(mutation, temporary_parent=alias)

    def test_notice_extractor_copies_only_the_existing_legal_header(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'synthetic.asm'
            source.write_text(';* legal notice\n;* second line\n; implementation detail\nnot a notice\n')
            record = {}
            BUILD.PROCESS.run(['awk', '!/^;\\*/ { exit } { print }', str(source)],
                              BUILD.command_environment(), timeout=30, record=record)
            self.assertEqual(record['stdout'], ';* legal notice\n;* second line\n')


if __name__ == '__main__':
    unittest.main()
