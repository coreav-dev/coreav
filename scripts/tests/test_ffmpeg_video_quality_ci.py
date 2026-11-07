"""Keep the native video-quality media tests in required macOS verification."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
NAMES = ('h264_video_quality_is_ignored', 'video_quality_refusals_preserve_destinations')


class VideoQualityCI(unittest.TestCase):
    def test_required_job_runs_the_native_target_with_qualified_tools(self):
        workflow = json.loads(subprocess.check_output([
            'ruby', '-rjson', '-rpsych', '-e', 'puts JSON.generate(Psych.load_file(ARGV[0]))',
            str(ROOT / '.github/workflows/ci.yml'),
        ], text=True))
        steps = workflow['jobs']['check-macos']['steps']
        step, = [s for s in steps if s.get('name') == 'Ignored H264 video quality on macOS']
        self.assertEqual(step.get('if'), "env.COREAV_FFMPEG != '' && env.COREAV_FFPROBE != ''")
        self.assertEqual(step['env'], {
            'COREAV_REQUIRE_FFMPEG': '1',
            'COREAV_X264': '${{ runner.temp }}/coreav-test-oracle/x264',
        })
        provision, = [s for s in steps if s.get('name') == 'Provision the registered macOS oracle or select oracle-free tests']
        install, = [s for s in steps if s.get('name') == 'Declared sample aspect ratios on macOS']
        self.assertIn('scripts/install-oracle-ffmpeg-macos.sh', provision['run'])
        self.assertIn('scripts/install-x264.sh', install['run'])
        self.assertLess(steps.index(provision), steps.index(install))
        self.assertLess(steps.index(install), steps.index(step))
        lines = step['run'].splitlines()
        self.assertEqual(lines[0],
                         'cargo test --locked -p coreav-cli --features native-h264 '
                         '--test ffmpeg_video_quality -- --nocapture --color never '
                         '--test-threads=1 2>&1 | tee "$RUNNER_TEMP/video-quality-macos.log"')
        self.assertEqual(lines[1:], [
            f"grep -F 'test {name} ... ok' \"$RUNNER_TEMP/video-quality-macos.log\""
            for name in NAMES
        ])
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / 'video-quality-macos.log'
            for output, succeeds in [
                ('running 0 tests\ntest result: ok. 0 passed; 0 failed\n', False),
                (f'test {NAMES[0]} ... ok\n', False),
                (f'test {NAMES[0]} ... ok\ntest {NAMES[1]} ... FAILED\n', False),
                (''.join(f'test {name} ... ok\n' for name in NAMES), True),
            ]:
                log.write_text(output)
                result = subprocess.run(['bash', '-e', '-c', '\n'.join(lines[1:])],
                                        capture_output=True, text=True,
                                        env={'RUNNER_TEMP': directory, 'PATH': '/usr/bin:/bin'})
                self.assertEqual(result.returncode == 0, succeeds)
        roster = json.loads((ROOT / 'test-support/oracle-dependent.json').read_text())
        self.assertEqual(roster['crates/coreav-cli/tests/ffmpeg_video_quality.rs'], list(NAMES))


if __name__ == '__main__':
    unittest.main()
