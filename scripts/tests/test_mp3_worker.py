"""Keep the MP3 protocol regression in both oracle-enabled source builds."""
from pathlib import Path
import shlex
import unittest


class WorkerWorkflow(unittest.TestCase):
    def test_both_transcode_fixture_builds_run_the_protocol_target(self):
        root = Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/ci.yml').read_text().replace('\\\n', ' ')
        commands = [shlex.split(line.strip()) for line in workflow.splitlines()
                    if line.strip().startswith('cargo test ')]
        for features in ('ffmpeg-fallback,dav1d,libvpx,libopus',
                         'ffmpeg-fallback,dav1d,libvpx,libopus,aac'):
            selected = [args for args in commands if '--no-run' not in args and '--features' in args and
                        args[args.index('--features') + 1] == features and
                        '--test' in args and 'transcode' in args]
            self.assertEqual(len(selected), 1, features)
            protocol = [args for args in commands if '--no-run' not in args and '--features' in args and
                        args[args.index('--features') + 1] == features and '--test' in args and
                        args[args.index('--test') + 1] == 'mp3_worker']
            self.assertEqual(len(protocol), 1,
                             'Run --test mp3_worker beside each complete transcode fixture target')


if __name__ == '__main__':
    unittest.main()
