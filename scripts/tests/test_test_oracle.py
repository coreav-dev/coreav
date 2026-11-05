"""Oracle-free Linux owner/cache contracts using banner-only stand-ins."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin
import test_oracle


@unittest.skipUnless(sys.platform == 'linux', 'Linux banner-only stand-in contracts')
class Qualification(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.log = self.root / 'calls'
        self.build = oracle_pin.load()['BUILD']
        self.pair(self.root)

    def pair(self, directory, *, version=None):
        directory.mkdir(exist_ok=True)
        for name in ('ffmpeg', 'ffprobe'):
            path = directory / name
            path.write_text('#!/bin/sh\n' +
                            f'printf "%s\\n" {name} >> "{self.log}"\n' +
                            f'printf "%s\\n" "{name} version {version or self.build}"\n')
            path.chmod(0o700)

    def child(self, code, *, directory=None):
        directory = directory or self.root
        env = dict(os.environ, COREAV_FFMPEG=str(directory / 'ffmpeg'),
                   COREAV_FFPROBE=str(directory / 'ffprobe'))
        prelude = 'import sys; sys.path.insert(0, sys.argv[1]); import test_oracle as owner\n'
        result = subprocess.run([sys.executable, '-I', '-c', prelude + code,
                                 str(ROOT / 'scripts'), str(self.root)],
                                env=env, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def test_concurrent_and_repeated_success_verifies_each_program_once(self):
        self.child('from concurrent.futures import ThreadPoolExecutor\n'
                   'with ThreadPoolExecutor(8) as pool:\n'
                   '    found = list(pool.map(lambda _: owner.require(), range(20)))\n'
                   'assert all(item is found[0] for item in found)\n'
                   'assert owner.require() is found[0]\n')
        self.assertCountEqual(self.log.read_text().splitlines(), ['ffmpeg', 'ffprobe'])

    def test_failed_selection_stays_failed_after_repair_until_a_fresh_process(self):
        self.pair(self.root, version='0.0.0')
        self.child('from pathlib import Path\n'
                   'for attempt in range(2):\n'
                   '    try:\n'
                   '        owner.require()\n'
                   '    except owner.QualificationError:\n'
                   '        pass\n'
                   '    else:\n'
                   '        raise AssertionError("unverified oracle accepted")\n'
                   '    if attempt == 0:\n'
                   f'        build = {self.build!r}\n'
                   '        for name in ("ffmpeg", "ffprobe"):\n'
                   '            (Path(sys.argv[2]) / name).write_text("#!/bin/sh\\necho \'" + name + " version " + build + "\'\\n")\n')
        self.assertEqual(len(self.log.read_text().splitlines()), 2)
        self.child('owner.require()\n')

    def test_second_selection_is_rejected_without_running_it(self):
        other = self.root / 'other'
        self.pair(other)
        self.child('owner.require()\n'
                   'from pathlib import Path\n'
                   'try:\n'
                   '    owner.require(Path(sys.argv[2]) / "other/ffmpeg")\n'
                   'except owner.QualificationError as error:\n'
                   '    assert "selection changed" in str(error)\n'
                   'else:\n'
                   '    raise AssertionError("second selection accepted")\n')
        self.assertEqual(len(self.log.read_text().splitlines()), 2)

    def test_missing_siblings_and_malformed_or_failed_versions_fail(self):
        for name in ('ffmpeg', 'ffprobe'):
            with self.subTest(missing=name):
                self.pair(self.root)
                (self.root / name).unlink()
                self.child('try:\n    owner.require()\n'
                           'except owner.QualificationError:\n    pass\n'
                           'else:\n    raise AssertionError("missing oracle accepted")\n')
        for body in ('echo malformed', 'exit 1'):
            with self.subTest(body=body):
                self.pair(self.root)
                (self.root / 'ffmpeg').write_text('#!/bin/sh\n' + body + '\n')
                self.child('try:\n    owner.require()\n'
                           'except owner.QualificationError:\n    pass\n'
                           'else:\n    raise AssertionError("invalid oracle accepted")\n')

    def test_verifier_failure_is_cached(self):
        self.child('from unittest.mock import patch\n'
                   'import subprocess\n'
                   'with patch.object(owner.oracle_pin, "verify_bin", side_effect=subprocess.TimeoutExpired("oracle", 10)) as verify:\n'
                   '    for _ in range(2):\n'
                   '        try:\n            owner.require()\n'
                   '        except owner.QualificationError:\n            pass\n'
                   '        else:\n            raise AssertionError("timeout accepted")\n'
                   '    assert verify.call_count == 1\n')
        self.assertFalse(self.log.exists())

    def test_symlink_only_selection_does_not_borrow_its_targets_sibling(self):
        selected = self.root / 'selected'
        selected.mkdir()
        (selected / 'ffmpeg').symlink_to(self.root / 'ffmpeg')
        self.child('import os\n'
                   'os.environ.pop("COREAV_FFPROBE", None)\n'
                   'try:\n    owner.require()\n'
                   'except owner.QualificationError:\n    pass\n'
                   'else:\n    raise AssertionError("incomplete selected installation accepted")\n',
                   directory=selected)

    def test_macos_delegates_once_to_the_existing_platform_verifier(self):
        self.child('from unittest.mock import patch\n'
                   'from pathlib import Path\n'
                   'with patch.object(owner.sys, "platform", "darwin"), \\\n'
                   '     patch.object(owner.oracle_pin, "verify_bin", side_effect=AssertionError("Linux verifier used")), \\\n'
                   '     patch.object(owner.oracle_pin_macos, "verify_bin", return_value=("bad pin",)) as verify:\n'
                   '    for _ in range(2):\n'
                   '        try:\n            owner.require()\n'
                   '        except owner.QualificationError:\n            pass\n'
                   '        else:\n            raise AssertionError("bad macOS pin accepted")\n'
                   '    verify.assert_called_once_with(Path(sys.argv[2]))\n')

    def test_relocated_bundle_keeps_the_same_pin_and_complete_import_closure(self):
        root = self.root / 'bundle'
        test_oracle.bundle(root, ['transcode_speed.py', 'vp9_speed.py', 'h264_parity.py', 'ts_speed.py'])
        copied = root / 'scripts'
        for name in ('oracle_pin.py', 'oracle_pin_macos.py', 'test_oracle.py', 'oracle_audio.py',
                     'oracle-audio-args.txt', 'composed_audio.py', 'composed_audio_source.py'):
            self.assertEqual((copied / name).read_bytes(), (ROOT / 'scripts' / name).read_bytes())
        self.assertFalse((copied / 'hevc_intra.py').exists())
        for name in ('transcode_speed.py', 'vp9_speed.py', 'h264_parity.py', 'ts_speed.py'):
            help_result = subprocess.run([sys.executable, '-I', str(copied / name), '--help'],
                                         capture_output=True, text=True, timeout=30)
            self.assertEqual(help_result.returncode, 0, help_result.stderr)
        result = subprocess.run([sys.executable, '-I', str(copied / 'test_oracle.py'), '--resolve',
                                 '--ffmpeg', str(self.root / 'ffmpeg'),
                                 '--ffprobe', str(self.root / 'ffprobe')],
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(),
                         [str(self.root / 'ffmpeg'), str(self.root / 'ffprobe')])

    def test_vp9_main_retains_the_qualified_path_pair_despite_cwd_shadowing(self):
        shadow = self.root / 'shadow'
        self.pair(shadow, version='0.0.0')
        self.child('import os\n'
                   'from pathlib import Path\n'
                   'from unittest.mock import patch\n'
                   'import vp9_speed\n'
                   'root = Path(sys.argv[2])\n'
                   'os.chdir(root / "shadow")\n'
                   'os.environ["PATH"] = str(root) + os.pathsep + os.environ.get("PATH", "")\n'
                   'sys.argv = ["vp9_speed.py", "oracle", "--ffmpeg", "ffmpeg", "--ffprobe", "ffprobe",\n'
                   '            "--selection", "missing", "--files", "missing", "--out", "uncreated",\n'
                   '            "--decode-oracle", "missing"]\n'
                   'def inspect_selected(args):\n'
                   '    assert args.ffmpeg == str(root / "ffmpeg"), args.ffmpeg\n'
                   '    assert args.ffprobe == str(root / "ffprobe"), args.ffprobe\n'
                   '    return 0\n'
                   'with patch.object(vp9_speed, "oracle", side_effect=inspect_selected) as run:\n'
                   '    assert vp9_speed.main() == 0\n'
                   '    assert run.call_count == 1\n')

    def test_active_entry_points_reject_before_creating_fixture_or_report_output(self):
        self.pair(self.root, version='0.0.0')
        ffmpeg, ffprobe = str(self.root / 'ffmpeg'), str(self.root / 'ffprobe')
        output = str(self.root / 'uncreated')
        commands = (
            ('scripts/h264_agreement.py', '--ffmpeg', ffmpeg, 'missing.h264'),
            ('scripts/h264_modes_identity.py', '--streams', 'missing', '--generate', ffmpeg),
            ('scripts/h264_speed.py', '--ffmpeg', ffmpeg, 'missing.h264'),
            ('scripts/h264_synth.py', '--source', 'missing', '--out', output),
            ('scripts/seek_check.py', '--decode-oracle', 'missing', '--ffprobe', ffprobe,
             '--report', output, 'missing.mp4'),
            ('scripts/ts_speed.py', '--coreav', 'missing', '--null', 'missing', '--lav', 'missing',
             '--coreav-cli', 'missing', '--ffprobe', ffprobe, '--report', output, 'missing.ts'),
            ('scripts/composed_audio_selftest.py', '--coreav', 'missing', '--ffmpeg', ffmpeg,
             '--ffprobe', ffprobe, '--opus-compare', 'missing', '--fixtures', 'missing'),
            ('scripts/standalone_audio_probe_check.py', '--coreav', 'missing', '--oracle-dir',
             str(self.root), '--directory', output),
            ('scripts/stsd_craft.py', '--fixture', ffmpeg, output),
            ('scripts/policy_check.py', '--coreav', 'missing', '--policy', 'missing',
             '--media', 'missing', '--ffprobe', ffprobe),
            ('scripts/text_check.py', '--coreav', 'missing', '--media', 'missing',
             '--ffmpeg', ffmpeg, '--ffprobe', ffprobe),
            ('scripts/vp9_corpus.py', 'run', '--files', 'missing', '--native', 'missing',
             '--ffmpeg', ffmpeg, '--ffprobe', ffprobe, '--private', output, '--summary', output),
            ('tools/hostile/generate.py', output, 'missing', ffmpeg, '--parts', 'pixelsmash'),
            ('tools/stage3-report/run.py', '--work', output, '--steps', 'fixtures'),
            ('tools/mediarecorder/generate.py', '--out', output),
            ('tools/compat-corpus/frames.py', '--coreav', 'missing', '--ffprobe', ffprobe,
             '--media', 'missing'),
            ('tools/mkv-adapter/bench.py', '--media', 'missing', '--out', output),
            ('tools/mkv-adapter/corpus.py', 'run', '--work', 'missing', '--out', output),
            ('tools/mkv-adapter/remux_corpus.py', 'run', '--work', 'missing', '--out', output),
            ('bench/range-audio/gaps.py', 'missing', output),
            ('bench/range-audio/run.py', 'missing', output),
            ('bench/npx/run.py', 'missing', output),
        )
        env = dict(os.environ, COREAV_FFMPEG=ffmpeg, COREAV_FFPROBE=ffprobe,
                   FFMPEG_BIN=ffmpeg, PATH=str(self.root) + os.pathsep + os.environ.get('PATH', ''))
        for script, *arguments in commands:
            with self.subTest(script=script):
                result = subprocess.run([sys.executable, '-I', str(ROOT / script), *arguments],
                                         env=env, capture_output=True, text=True, timeout=30)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('stage=oracle_qualification', result.stderr)
                self.assertFalse(Path(output).exists())


if __name__ == '__main__':
    unittest.main()
