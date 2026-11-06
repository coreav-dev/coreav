"""Shell consumers qualify and retain the oracle pair before creating outputs."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
CONSUMERS = (
    'scripts/gen-fixtures.sh',
    'scripts/gen-fixtures-mkv.sh',
    'scripts/gen-fixtures-ts.sh',
    'scripts/oracle-ffprobe.sh',
    'packages/cli/test/release-clips.sh',
)

sys.path.insert(0, str(ROOT / 'scripts'))
import test_oracle


class ShellOracleQualification(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for relative in CONSUMERS:
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, target)
        shutil.copyfile(ROOT / 'scripts/oracle-audio-args.txt',
                        self.root / 'scripts/oracle-audio-args.txt')
        self.log = self.root / 'calls.jsonl'
        self.scratch = self.root / 'scratch'
        self.scratch.mkdir()
        self.pair = self.root / 'retained pair'
        self.pair.mkdir()
        decoy = self.root / 'decoy'
        decoy.mkdir()
        for name in ('ffmpeg', 'ffprobe'):
            self.executable(decoy / name, '#!/bin/sh\necho decoy >> "$DECOY_LOG"\nexit 99\n')
            self.executable(self.pair / name, """#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
with open(os.environ['CALL_LOG'], 'a') as log:
    log.write(json.dumps([Path(sys.argv[0]).name, *sys.argv[1:]]) + '\\n')
if Path(sys.argv[0]).name == 'ffprobe':
    print('0,video' if 'csv=p=0' in sys.argv else '{}')
elif '-encoders' in sys.argv:
    print('libx264 libaom-av1')
elif os.environ.get('STOP_GENERATION') == '1':
    sys.exit(73)
elif sys.argv[-1] == '-':
    print('frame')
else:
    Path(sys.argv[-1]).write_bytes(b'clip')
""")
        self.executable(self.root / 'normalize', '#!/bin/sh\necho normalized\n')
        (self.root / 'scripts/test_oracle.py').write_text("""import json
import os
from pathlib import Path
import sys
with open(os.environ['CALL_LOG'], 'a') as log:
    log.write(json.dumps(['owner', *sys.argv[1:]]) + '\\n')
mode = os.environ.get('OWNER_MODE', 'success')
pair = Path(os.environ['RETAINED_PAIR'])
if mode == 'fail':
    print(pair / 'ffmpeg')
    print(pair / 'ffprobe')
    print('stage=oracle_qualification: rejected', file=sys.stderr)
    sys.exit(1)
if mode == 'empty':
    sys.exit(0)
print('' if mode == 'blank-first' else pair / 'ffmpeg')
if mode != 'one':
    print(pair / 'ffprobe')
if mode == 'three':
    print(pair / 'unexpected')
""")
        self.env = dict(os.environ, PATH=str(decoy) + os.pathsep + os.environ['PATH'],
                        CALL_LOG=str(self.log), DECOY_LOG=str(self.root / 'decoy.log'),
                        RETAINED_PAIR=str(self.pair), TMPDIR=str(self.scratch),
                        COREAV_ORACLE=str(self.root / 'normalize'))
        for name in ('FFMPEG', 'FFPROBE', 'COREAV_FFMPEG', 'COREAV_FFPROBE'):
            self.env.pop(name, None)

    def executable(self, path, source):
        path.write_text(source)
        path.chmod(0o755)

    def run_consumer(self, relative, **env):
        arguments = ['frames', str(self.root / 'input.mp4')] if 'oracle-ffprobe' in relative else [
            str(self.root / 'output')]
        return subprocess.run([os.environ.get('COREAV_TEST_BASH', '/bin/bash'),
                               str(self.root / relative), *arguments],
                              env=dict(self.env, **env), capture_output=True, text=True)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_rejected_and_malformed_pairs_fail_before_any_outputs(self):
        for relative in CONSUMERS:
            for mode in ('fail', 'one', 'three', 'empty', 'blank-first'):
                with self.subTest(consumer=relative, mode=mode):
                    result = self.run_consumer(relative, OWNER_MODE=mode)
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertIn('stage=oracle_qualification', result.stderr)
                    self.assertFalse((self.root / 'output').exists())
                    self.assertEqual(list(self.scratch.iterdir()), [])
                    self.assertFalse((self.root / 'decoy.log').exists())
        self.assertTrue(all(call[0] == 'owner' for call in self.calls()))

    def test_fixture_generators_dispatch_only_to_retained_ffmpeg(self):
        for relative in CONSUMERS[:3]:
            with self.subTest(consumer=relative):
                self.log.unlink(missing_ok=True)
                result = self.run_consumer(relative, STOP_GENERATION='1')
                self.assertNotEqual(result.returncode, 0)
                calls = self.calls()
                self.assertEqual(calls[0], ['owner', '--resolve'])
                self.assertTrue(any(call[0] == 'ffmpeg' and '-i' in call for call in calls))
                if relative != 'scripts/gen-fixtures-ts.sh':
                    self.assertTrue(any('-encoders' in call for call in calls))
                self.assertFalse((self.root / 'decoy.log').exists())
                shutil.rmtree(self.root / 'output')

    def test_release_clips_dispatches_all_encodes_to_retained_ffmpeg(self):
        result = self.run_consumer(CONSUMERS[-1])
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        self.assertEqual(calls[0], ['owner', '--resolve'])
        self.assertEqual(sum('-encoders' in call for call in calls), 1)
        self.assertEqual(sum('-i' in call for call in calls), 6)
        self.assertEqual(len(list((self.root / 'output').iterdir())), 6)
        self.assertFalse((self.root / 'decoy.log').exists())

    def test_pair_parsing_does_not_require_the_mapfile_builtin(self):
        startup = self.root / 'bash-startup'
        startup.write_text('if type mapfile >/dev/null 2>&1; then enable -n mapfile; fi\n')
        result = self.run_consumer(CONSUMERS[-1], BASH_ENV=str(startup))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / 'decoy.log').exists())

    def test_probe_retains_both_paths_and_only_passes_supplied_overrides(self):
        for overrides, expected in (
                ({}, []),
                ({'FFMPEG': 'chosen ffmpeg'}, ['--ffmpeg', 'chosen ffmpeg']),
                ({'FFPROBE': 'chosen ffprobe'}, ['--ffprobe', 'chosen ffprobe']),
                ({'FFMPEG': 'chosen ffmpeg', 'FFPROBE': 'chosen ffprobe'},
                 ['--ffmpeg', 'chosen ffmpeg', '--ffprobe', 'chosen ffprobe'])):
            with self.subTest(overrides=overrides):
                self.log.unlink(missing_ok=True)
                result = self.run_consumer('scripts/oracle-ffprobe.sh', **overrides)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, 'normalized\n')
                calls = self.calls()
                self.assertEqual(calls[0], ['owner', '--resolve', *expected])
                self.assertEqual([call[0] for call in calls], ['owner', 'ffprobe', 'ffmpeg'])
                self.assertFalse((self.root / 'decoy.log').exists())
                self.assertEqual(list(self.scratch.iterdir()), [])

    def test_probe_default_selection_preserves_audio_arguments_without_mapfile(self):
        startup = self.root / 'bash-startup'
        startup.write_text('if type mapfile >/dev/null 2>&1; then enable -n mapfile; fi\n')
        for text in ('-threads:a\n1\n', '-threads:a\n1'):
            with self.subTest(arguments=text):
                self.log.unlink(missing_ok=True)
                (self.root / 'scripts/oracle-audio-args.txt').write_text(text)
                result = self.run_consumer('scripts/oracle-ffprobe.sh', BASH_ENV=str(startup))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, 'normalized\n')
                calls = self.calls()
                self.assertEqual(calls[0], ['owner', '--resolve'])
                self.assertEqual(calls[-1], ['ffmpeg', '-nostdin', '-v', 'error', '-threads:a', '1',
                    '-i', str(self.root / 'input.mp4'), '-map', '0:0', '-fps_mode', 'passthrough',
                    '-f', 'framemd5', '-'])
                self.assertFalse((self.root / 'decoy.log').exists())
                self.assertEqual(list(self.scratch.iterdir()), [])


class FixtureCacheInputs(unittest.TestCase):
    def test_both_fixture_cache_keys_bind_every_owner_support_file(self):
        generator = (ROOT / 'scripts/gen-fixtures.sh').read_text()
        inputs = {'scripts/gen-fixtures.sh', 'scripts/install-oracle-ffmpeg.sh',
                  'scripts/oracle-ffmpeg.env', *('scripts/' + name for name in test_oracle.SUPPORT),
                  'docs/design/composed-opus-aac-v2.json', 'scripts/composed_audio.py', 'scripts/composed_audio_fixtures.py',
                  *('scripts/' + name for name in re.findall(r'\$SCRIPTS/([A-Za-z0-9_.-]+)', generator))}
        for workflow in ('.github/actions/ci-fixtures/action.yml', '.github/workflows/wasm.yml'):
            with self.subTest(workflow=workflow), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for name in inputs:
                    target = root / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes((ROOT / name).read_bytes())
                lines = (ROOT / workflow).read_text().splitlines()
                start = next(i for i, line in enumerate(lines) if 'helpers=$(grep' in line)
                stop = next(i for i in range(start, len(lines)) if 'GITHUB_OUTPUT' in lines[i])
                command = '\n'.join(line.lstrip() for line in lines[start:stop + 1])
                output = root / 'key'

                def key():
                    output.unlink(missing_ok=True)
                    run = subprocess.run(['bash', '-euo', 'pipefail', '-c', command], cwd=root,
                                         env=dict(os.environ, GITHUB_OUTPUT=str(output)),
                                         capture_output=True, text=True, timeout=30)
                    self.assertEqual(run.returncode, 0, run.stderr)
                    return output.read_text()

                baseline = key()
                for name in test_oracle.SUPPORT:
                    path = root / 'scripts' / name
                    original = path.read_bytes()
                    path.write_bytes(original + b'\n')
                    changed = key().splitlines()
                    self.assertEqual(len(changed), len(baseline.splitlines()), (workflow, name))
                    self.assertTrue(all(a != b for a, b in zip(changed, baseline.splitlines())), (workflow, name))
                    path.write_bytes(original)
                for name in test_oracle.SUPPORT:
                    path = root / 'scripts' / name
                    original = path.read_bytes()
                    path.unlink()
                    output.unlink(missing_ok=True)
                    run = subprocess.run(['bash', '-euo', 'pipefail', '-c', command], cwd=root,
                                         env=dict(os.environ, GITHUB_OUTPUT=str(output)),
                                         capture_output=True, text=True, timeout=30)
                    self.assertNotEqual(run.returncode, 0, (workflow, name))
                    self.assertFalse(output.exists(), (workflow, name))
                    path.write_bytes(original)


if __name__ == '__main__':
    unittest.main()
