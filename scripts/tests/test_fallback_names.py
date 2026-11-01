import contextlib
import io
import pathlib
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import fallback_names as names


class FallbackNames(unittest.TestCase):
    def setUp(self):
        temporary = self.enterContext(tempfile.TemporaryDirectory())
        self.root = pathlib.Path(temporary)
        self.out = self.root / 'names.rs'
        self.source = self.root / 'ffprobe'
        self.source.mkdir()
        (self.source / 'keys.rs').write_text('("public_key", value)')
        self.directory = self.root / 'oracle bin'
        for attribute, value in [('ROOT', self.root), ('OUT', self.out), ('SOURCE', self.source)]:
            self.enterContext(patch.object(names, attribute, value))
        self.verify = self.enterContext(patch.object(names.oracle_pin, 'verify_bin', return_value=[]))
        self.listing = self.enterContext(patch.object(names, 'run', side_effect=self.listings))
        vocabulary = SimpleNamespace(FFMPEG_VALUE=set(), FFMPEG_CODEC=set(), FFMPEG_FLAG=set(),
                                     FFPROBE_VALUE=set(), FFPROBE_FLAG=set())
        self.enterContext(patch.object(names, 'classify', return_value=vocabulary))

    def listings(self, program, *args):
        if args == ('-encoders',):
            return ' V..... public_encoder\n'
        if args == ('-formats',):
            return ' DE  public_format,alias\n'
        if args == ('-sections',):
            return '.... public_section/alias_section\n'
        self.assertEqual(args, ('-h', 'full'))
        return '-sample <value>  sample\n-flag  flag\n' + ''.join(
            f'  -{option} <flags>\n     public 1 ...........\n' for option in names.NAMED_VALUES)

    def invoke(self, check=False):
        stderr = io.StringIO()
        argv = ['fallback_names.py', *(['--check'] if check else []), str(self.directory)]
        with patch.object(sys, 'argv', argv), contextlib.redirect_stderr(stderr):
            code = names.main()
        return code, stderr.getvalue()

    def test_write_then_check_preserves_the_file(self):
        self.assertEqual(self.invoke(), (0, ''))
        before = self.out.read_bytes(), self.out.stat().st_mtime_ns
        self.assertEqual(self.invoke(check=True), (0, ''))
        self.assertEqual((self.out.read_bytes(), self.out.stat().st_mtime_ns), before)
        self.verify.assert_called_with(self.directory)
        for name in ['-sample', '-flag', 'public_encoder', 'public_format,alias',
                     'alias_section', 'public_key']:
            self.assertIn(name, self.out.read_text())

    def test_drift_fails_without_writing_and_names_the_fix(self):
        self.invoke()
        self.out.write_text(self.out.read_text().replace('public_encoder', 'stale_encoder'))
        before = self.out.read_bytes(), self.out.stat().st_mtime_ns
        code, error = self.invoke(check=True)
        self.assertEqual(code, 1)
        self.assertIn('-stale_encoder', error)
        self.assertIn('+public_encoder', error)
        self.assertIn(f"scripts/install-oracle-ffmpeg.sh '{self.directory}' && "
                      f"python3 -I scripts/fallback_names.py '{self.directory}'", error)
        self.assertEqual((self.out.read_bytes(), self.out.stat().st_mtime_ns), before)

    def test_missing_file_fails_without_creating_it(self):
        self.assertEqual(self.invoke(check=True)[0], 1)
        self.assertFalse(self.out.exists())

    def test_both_oracle_versions_are_verified_before_listing(self):
        for tool in ['ffmpeg', 'ffprobe']:
            with self.subTest(tool=tool):
                self.verify.return_value = [f'{tool}: expected the oracle from scripts/oracle-ffmpeg.env']
                for check in [False, True]:
                    with self.assertRaisesRegex(SystemExit, tool):
                        self.invoke(check=check)
                self.listing.assert_not_called()
                self.assertFalse(self.out.exists())

    def test_invalid_names_never_replace_the_file(self):
        self.invoke()
        before = self.out.read_bytes()
        self.listing.side_effect = lambda program, *args: (
            ' V..... bad"name\n' if args == ('-encoders',) else self.listings(program, *args))
        for check in [False, True]:
            with self.assertRaisesRegex(SystemExit, 'ENCODERS'):
                self.invoke(check=check)
            self.assertEqual(self.out.read_bytes(), before)

    def test_formatter_failure_never_replaces_the_file(self):
        self.invoke()
        before = self.out.read_bytes()
        with patch.object(names.subprocess, 'run', side_effect=subprocess.CalledProcessError(101, 'rustfmt')):
            for check in [False, True]:
                with self.assertRaises(subprocess.CalledProcessError):
                    self.invoke(check=check)
                self.assertEqual(self.out.read_bytes(), before)

    def test_unformatted_file_is_drift(self):
        self.invoke()
        self.out.write_text(self.out.read_text().replace('pub const', 'pub  const'))
        before = self.out.read_bytes()
        self.assertEqual(self.invoke(check=True)[0], 1)
        self.assertEqual(self.out.read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
