"""Oracle-free Linux owner/cache contracts using banner-only stand-ins."""
import os
from pathlib import Path
import re
import shutil
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

    def test_rust_adapter_embeds_the_complete_owner_support_set(self):
        source = (ROOT / 'test-support/oracle.rs').read_text()
        embedded = set(re.findall(r'"\.\./scripts/([^\"]+)"', source))
        self.assertEqual(len(re.findall(r'\binclude_str!\(', source)), len(embedded))
        self.assertEqual(embedded, set(test_oracle.SUPPORT))
        root = self.root / 'embedded'
        test_oracle.bundle(root, [])
        self.assertEqual({path.name for path in (root / 'scripts').iterdir()}, embedded)

    @unittest.skipUnless(shutil.which('rustc'), 'Rust adapter compiler is not installed')
    def test_rust_adapter_qualifies_without_build_sources_and_retains_both_results(self):
        build = self.root / 'build'
        manifest = build / 'crates/probe'
        manifest.mkdir(parents=True)
        test_oracle.bundle(build, [])
        (build / 'test-support').mkdir()
        shutil.copy(ROOT / 'test-support/oracle.rs', build / 'test-support/oracle.rs')
        source = manifest / 'main.rs'
        source.write_text('#[path = "../../test-support/oracle.rs"] mod oracle;\nfn main() {\n'
                          'let mode = std::env::args().nth(1).unwrap();\n'
                          'if mode == "native" { return; }\n'
                          'for _ in 0..2 {\n'
                          'let result = oracle::try_require();\n'
                          'assert_eq!(result.is_ok(), mode == "valid");\n'
                          'if let Err(error) = result { assert!(error.contains("stage=oracle_qualification")); }\n'
                          '}\n}\n')
        binary = self.root / 'relocated'
        made = subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(binary)],
                              capture_output=True, text=True, timeout=60)
        self.assertEqual(made.returncode, 0, made.stderr)
        shutil.rmtree(build)
        temporary = self.root / 'support'
        temporary.mkdir()
        for mode in ('valid', 'invalid', 'native'):
            with self.subTest(mode=mode):
                self.pair(self.root, version=self.build if mode == 'valid' else '0.0.0')
                self.log.unlink(missing_ok=True)
                env = dict(os.environ, COREAV_FFMPEG=str(self.root / 'ffmpeg'),
                           COREAV_FFPROBE=str(self.root / 'ffprobe'), TMPDIR=str(temporary))
                if mode == 'native':
                    env['PATH'] = '/nonexistent'
                done = subprocess.run([str(binary), mode], env=env, capture_output=True,
                                      text=True, timeout=30)
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertEqual(list(temporary.iterdir()), [])
                if mode == 'native':
                    self.assertFalse(self.log.exists())
                else:
                    self.assertCountEqual(self.log.read_text().splitlines(), ['ffmpeg', 'ffprobe'])

    def test_avi_drivers_reject_each_different_selected_file_before_work(self):
        marker = self.root / 'unqualified.called'
        selected = self.root / 'other'
        selected.write_text(f'#!/bin/sh\ntouch "{marker}"\necho wrong\n')
        selected.chmod(0o700)
        for driver in ('avi_fixture_check.py', 'avi_v2_check.py'):
            for role in ('ffmpeg', 'ffprobe'):
                with self.subTest(driver=driver, role=role):
                    command = [sys.executable, '-I', str(ROOT / 'scripts' / driver), '--coreav', 'missing',
                               '--ffmpeg', str(selected if role == 'ffmpeg' else self.root / 'ffmpeg'),
                               '--ffprobe', str(selected if role == 'ffprobe' else self.root / 'ffprobe'),
                               '--work', str(self.root / 'uncreated')]
                    if driver == 'avi_v2_check.py':
                        command += ['--x264', 'missing', '--build-log', 'missing', '--fixtures', 'missing']
                    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('stage=oracle_qualification', result.stderr)
                    self.assertFalse(marker.exists())
                    self.assertFalse((self.root / 'uncreated').exists())

    def test_probe_only_entry_points_derive_the_sibling_after_path_resolution(self):
        shadow = self.root / 'shadow'
        shadow.mkdir()
        (shadow / 'ffmpeg').write_text('#!/bin/sh\necho "ffmpeg version 0.0.0"\n')
        (shadow / 'ffmpeg').chmod(0o700)
        self.child('import importlib.util, os\n'
                   'from pathlib import Path\nfrom unittest.mock import patch\n'
                   'root = Path(sys.argv[2])\nrepo = Path(sys.argv[1]).parent\n'
                   'require = owner.require\n'
                   'os.environ["PATH"] = str(root / "shadow") + os.pathsep + str(root) + os.pathsep + os.environ["PATH"]\n'
                   'cases = [("scripts/policy_check.py", ["--coreav", "missing", "--policy", "missing"]),\n'
                   '         ("tools/compat-corpus/frames.py", ["--coreav", "missing", "--media", "missing"]),\n'
                   '         ("scripts/ts_speed.py", ["--coreav", "missing", "--null", "missing", "--lav", "missing", "--report", "missing", "missing"])]\n'
                   'for index, (path, args) in enumerate(cases):\n'
                   '    spec = importlib.util.spec_from_file_location("driver" + str(index), repo / path)\n'
                   '    driver = importlib.util.module_from_spec(spec)\n    spec.loader.exec_module(driver)\n'
                   '    def inspect_selected(ffmpeg, ffprobe):\n'
                   '        pair = require(ffmpeg, ffprobe)\n'
                   '        assert pair.ffmpeg == root / "ffmpeg"\n'
                   '        assert pair.ffprobe == root / "ffprobe"\n'
                   '        raise StopIteration\n'
                   '    sys.argv = [path, *args, "--ffprobe", "ffprobe"]\n'
                   '    with patch.object(driver.test_oracle, "require", side_effect=inspect_selected):\n'
                   '        try: driver.main()\n'
                   '        except StopIteration: pass\n'
                   '        else: raise AssertionError("selection boundary was not reached")\n')

    def test_remux_checker_runs_only_the_retained_pair(self):
        (self.root / 'ffprobe').write_text('#!/bin/sh\n' +
            f'echo ffprobe >> "{self.log}"\n' +
            f'if [ "$1" = "-version" ]; then echo "ffprobe version {self.build}"; exit; fi\n' +
            'echo \'{"format":{"format_name":"mp4"},"streams":[]}\'\n')
        shadow = self.root / 'shadow'
        shadow.mkdir()
        (shadow / 'ffprobe').write_text('#!/bin/sh\nexit 99\n')
        (shadow / 'ffprobe').chmod(0o700)
        self.child('from pathlib import Path\nimport os\n'
                   'root = Path(sys.argv[2])\n'
                   'sys.path.insert(0, str(Path(sys.argv[1]).parent / "tools/mkv-adapter"))\n'
                   'import remux_corpus as driver\n'
                   'os.environ["PATH"] = str(root / "shadow") + os.pathsep + os.environ["PATH"]\n'
                   'pair = owner.require()\n'
                   'output = root / "output.mp4"\n'
                   'output.write_bytes(b"".join((8).to_bytes(4, "big") + kind for kind in (b"ftyp", b"moov", b"mdat")))\n'
                   'result = driver.check_with_oracle(output, output, {"streams": []}, None, pair)\n'
                   'assert result["ok"], result\n')
        self.assertCountEqual(self.log.read_text().splitlines(), ['ffmpeg', 'ffprobe', 'ffprobe', 'ffprobe'])

    def test_omitted_cli_selectors_honor_the_configured_pair_over_path(self):
        shadow = self.root / 'shadow'
        self.pair(shadow, version='0.0.0')
        self.child('import importlib.util, os\nfrom pathlib import Path\nfrom unittest.mock import patch\n'
                   'root = Path(sys.argv[2])\nrepo = Path(sys.argv[1]).parent\nrequire = owner.require\n'
                   'os.environ["PATH"] = str(root / "shadow") + os.pathsep + os.environ["PATH"]\n'
                   'cases = [("text_check", ["--coreav", "missing", "--media", "missing"]),\n'
                   '         ("audio_copy_check", ["--coreav", "missing", "--work", "missing"]),\n'
                   '         ("oracle", ["capture", "missing", "missing"]),\n'
                   '         ("thumbnail_oracle", ["--coreav", "missing", "--decode-oracle", "missing"]),\n'
                   '         ("exact_skip_check", ["--decode-oracle", "missing"]),\n'
                   '         ("seek_check", ["--decode-oracle", "missing"])]\n'
                   'for name, args in cases:\n'
                   '    spec = importlib.util.spec_from_file_location("driver_" + name, repo / "scripts" / (name + ".py"))\n'
                   '    driver = importlib.util.module_from_spec(spec)\n    spec.loader.exec_module(driver)\n'
                   '    def inspect_selected(ffmpeg=None, ffprobe=None):\n'
                   '        pair = require(ffmpeg, ffprobe)\n'
                   '        assert pair.ffmpeg == root / "ffmpeg"\n'
                   '        assert pair.ffprobe == root / "ffprobe"\n'
                   '        raise StopIteration\n'
                   '    sys.argv = [name, *args]\n'
                   '    with patch.object(driver.test_oracle, "require", side_effect=inspect_selected):\n'
                   '        try: driver.main()\n'
                   '        except StopIteration: pass\n'
                   '        else: raise AssertionError("selection boundary was not reached")\n')

    def test_transcode_select_qualifies_and_retains_the_pair_before_dispatch(self):
        self.child('from pathlib import Path\n'
                   'from unittest.mock import patch\n'
                   'import transcode_speed\n'
                   'root = Path(sys.argv[2])\n'
                   'sys.argv = ["transcode_speed.py", "select", "--work", str(root / "work"),\n'
                   '            "--selection", str(root / "selection"), "--ffmpeg", str(root / "ffmpeg"),\n'
                   '            "--ffprobe", str(root / "ffprobe")]\n'
                   'def inspect_selected(args):\n'
                   '    assert args.ffmpeg == str(root / "ffmpeg")\n'
                   '    assert args.ffprobe == str(root / "ffprobe")\n'
                   '    assert owner.require().ffmpeg == root / "ffmpeg"\n'
                   '    return 0\n'
                   'with patch.object(transcode_speed, "select", side_effect=inspect_selected) as run:\n'
                   '    assert transcode_speed.main() == 0\n'
                   '    assert run.call_count == 1\n')

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
            ('scripts/h264_parity_gcp.py', '--out', output, '--es-dir', 'missing', '--decoder', 'base=missing',
             '--ffmpeg', ffmpeg, '--gate-stream', 'missing', '--null-arm', 'base'),
            ('scripts/transcode_speed.py', 'select', '--ffmpeg', ffmpeg, '--ffprobe', ffprobe,
             '--work', output, '--selection', output + '.json'),
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
