"""Supplemental V2 fixture recipes remain finite, explicit and reproducible."""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / 'gen-v2-fixtures.py'
SPEC = importlib.util.spec_from_file_location('gen_v2_fixtures', SCRIPT)
generator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = generator
SPEC.loader.exec_module(generator)

EXPECTED_FILES = (
    'v2_still_rgba.png', 'v2_still_rgba%03d.png', 'v2_still_rgba000.png',
    'v2_still_rgb.jpg', 'v2_audio_mp3_long.mp3',
    'v2_multi_audio_eng.mp4', 'v2_video_only.mp4', 'v2_audio_opus_long.ogg',
    'v2_audio_aac_long.m4a', 'v2_video_long.mp4',
    'v2_raw_yuv420p_1280x720.yuv', 'v2_raw_yuv420p16le_1280x720.yuv',
    'v2_raw_yuv420p_1920x1080.yuv', 'v2_raw_bgr24_640x480.raw',
    'v2_raw_rgb32_800x550.raw', 'v2_raw_yuv420p_400x300_long.yuv',
    'v2_subtitles.srt', 'v2_subtitles_movtext.mp4', 'v2_mpeg_ps_video.mpg',
    'v2_video_hevc.mp4', 'v2_hls_local.m3u8', 'v2_hls_segment.ts',
)
RAW_LENGTHS = {
    'v2_raw_yuv420p_1280x720.yuv': 1382400,
    'v2_raw_yuv420p16le_1280x720.yuv': 2764800,
    'v2_raw_yuv420p_1920x1080.yuv': 3110400,
    'v2_raw_bgr24_640x480.raw': 921600,
    'v2_raw_rgb32_800x550.raw': 1760000,
    'v2_raw_yuv420p_400x300_long.yuv': 19584000000,
}


class Generator(unittest.TestCase):
    def test_cli_names_and_exact_zero_filled_raw_lengths(self):
        self.assertEqual(generator.FILES, EXPECTED_FILES)
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'fixtures'
            stdout = io.StringIO()
            with patch.object(generator.subprocess, 'run') as child:
                with contextlib.redirect_stdout(stdout):
                    status = generator.main([
                        '--ffmpeg', '/unused/ffmpeg', '--ffprobe', '/unused/ffprobe',
                        '--out', str(out), '--only', *RAW_LENGTHS,
                    ])
            self.assertEqual(status, 0)
            self.assertEqual(stdout.getvalue().splitlines(),
                             [f'generated {name}' for name in RAW_LENGTHS])
            child.assert_not_called()
            self.assertEqual(set(path.name for path in out.iterdir()), set(RAW_LENGTHS))
            for name, length in RAW_LENGTHS.items():
                with self.subTest(name=name):
                    self.assertEqual((out / name).stat().st_size, length)
                    if length < 4000000:
                        self.assertEqual((out / name).read_bytes(), bytes(length))

    def test_only_expands_declared_dependencies_once(self):
        self.assertEqual(generator.selection([
            'v2_hls_local.m3u8', 'v2_hls_segment.ts', 'v2_hls_local.m3u8',
        ]), ('v2_hls_segment.ts', 'v2_hls_local.m3u8'))
        self.assertEqual(generator.selection(['v2_subtitles_movtext.mp4']),
                         ('v2_subtitles.srt', 'v2_subtitles_movtext.mp4'))
        self.assertEqual(generator.selection(['v2_still_rgba%03d.png']),
                         ('v2_still_rgba000.png', 'v2_still_rgba%03d.png'))
        for name in ('../bad', 'still_rgba.png', '/tmp/unsafe', 'v2_unknown.mp4'):
            with self.subTest(name=name), self.assertRaises(SystemExit) as error:
                with contextlib.redirect_stderr(io.StringIO()):
                    generator.main(['--ffmpeg', '/unused/ffmpeg', '--ffprobe',
                                    '/unused/ffprobe', '--out', '/unused', '--only', name])
            self.assertEqual(error.exception.code, 2)

    def test_generation_recipe_capture_is_deterministic_and_bounded(self):
        captures = []
        for _ in range(2):
            calls = []

            def child(command, **settings):
                calls.append((command, {key: value for key, value in settings.items()
                                        if key != 'cwd'}))
                self.assertGreater(settings['timeout'], 0)
                self.assertLessEqual(settings['timeout'], 180)
                self.assertEqual(settings['stdin'], subprocess.DEVNULL)
                if command[0].endswith('ffmpeg'):
                    (Path(settings['cwd']) / command[-1]).write_bytes(b'synthetic capture')
                    return subprocess.CompletedProcess(command, 0, '', '')
                return subprocess.CompletedProcess(command, 0, '{"streams":[{}]}', '')

            with tempfile.TemporaryDirectory() as temporary:
                with patch.object(generator.subprocess, 'run', side_effect=child):
                    names = generator.generate('/oracle/ffmpeg', '/oracle/ffprobe', temporary)
                self.assertEqual(set(names), set(EXPECTED_FILES))
                self.assertEqual(set(path.name for path in Path(temporary).iterdir()),
                                 set(EXPECTED_FILES))
                playlist = (Path(temporary) / 'v2_hls_local.m3u8').read_text()
                self.assertIn('#EXT-X-ENDLIST\n', playlist)
                self.assertEqual([line for line in playlist.splitlines()
                                  if not line.startswith('#')], ['v2_hls_segment.ts'])
                subtitle = (Path(temporary) / 'v2_subtitles.srt').read_text()
                self.assertIn('00:00:00,000 --> 00:00:01,000', subtitle)
            captures.append(calls)
        self.assertEqual(captures[0], captures[1])
        encodes = {command[-1]: command for command, _ in captures[0]
                   if command[0].endswith('ffmpeg')}
        self.assertIn('testsrc2=size=64x48:rate=1:duration=5027',
                      encodes['v2_video_long.mp4'])
        self.assertNotIn('-loop', encodes['v2_video_long.mp4'])
        for name, duration in (('v2_audio_mp3_long.mp3', 480),
                               ('v2_audio_opus_long.ogg', 130),
                               ('v2_audio_aac_long.m4a', 40),
                               ('v2_multi_audio_eng.mp4', 2),
                               ('v2_hls_segment.ts', 2)):
            self.assertIn(f'sine=frequency=440:sample_rate=48000:duration={duration}',
                          encodes[name])
        for command in encodes.values():
            for option in ('-threads', '-filter_threads', '-filter_complex_threads'):
                self.assertEqual(command[command.index(option) + 1], '1')
            self.assertIn('+bitexact', command)
        self.assertEqual(encodes['v2_multi_audio_eng.mp4'].count('1:a:0'), 4)
        self.assertIn('pools=none:frame-threads=1:wpp=0:log-level=error',
                      encodes['v2_video_hevc.mp4'])

    def test_real_child_failure_and_timeout_retain_diagnostics(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(generator.GenerationError,
                                        'stage=encode:test reason=exit-7') as error:
                generator.run('encode:test', [sys.executable, '-c',
                              'import sys; print("encoder failed", file=sys.stderr); sys.exit(7)'],
                              temporary, 10)
            self.assertIn('encoder failed', str(error.exception))
            with self.assertRaisesRegex(generator.GenerationError,
                                        'stage=probe:test reason=timeout'):
                generator.run('probe:test', [sys.executable, '-c',
                              'import time; time.sleep(60)'], temporary, 1)
            with self.assertRaisesRegex(generator.GenerationError, 'reason=launch'):
                generator.run('encode:test', ['/not/a/real/program'], temporary, 1)

    def test_failure_keeps_a_bounded_stderr_tail(self):
        result = subprocess.CompletedProcess(['ffmpeg'], 2, '', 'x' * 20000 + 'last diagnostic')
        with patch.object(generator.subprocess, 'run', return_value=result):
            with self.assertRaises(generator.GenerationError) as error:
                generator.run('encode:test', ['ffmpeg'], '.', 10)
        message = str(error.exception)
        self.assertIn('stage=encode:test reason=exit-2', message)
        self.assertTrue(message.endswith('last diagnostic'))
        self.assertEqual(len(message.split('stderr=', 1)[1]), generator.STDERR_LIMIT)

    def test_failed_generation_preserves_existing_outputs_and_cleans_staging(self):
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary)
            target = out / 'v2_still_rgba.png'
            target.write_bytes(b'previous complete fixture')
            stdout, stderr = io.StringIO(), io.StringIO()
            with patch.object(generator.subprocess, 'run', side_effect=subprocess.TimeoutExpired(
                    'ffmpeg', 180, stderr=b'partial encoder diagnostic')):
                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    status = generator.main([
                        '--ffmpeg', '/oracle/ffmpeg', '--ffprobe', '/oracle/ffprobe',
                        '--out', temporary, '--only', target.name,
                    ])
            self.assertEqual(status, 1)
            self.assertIn('stage=encode:v2_still_rgba.png reason=timeout', stderr.getvalue())
            self.assertIn('partial encoder diagnostic', stderr.getvalue())
            self.assertEqual(stdout.getvalue(), '')
            self.assertEqual(target.read_bytes(), b'previous complete fixture')
            self.assertEqual(list(out.iterdir()), [target])

    def test_probe_failure_does_not_publish_media(self):
        for probe_text in ('not JSON', '{}', '[]', '{"streams":[]}'):
            with self.subTest(probe=probe_text), tempfile.TemporaryDirectory() as temporary:
                def child(command, **settings):
                    if command[0].endswith('ffmpeg'):
                        (Path(settings['cwd']) / command[-1]).write_bytes(b'media')
                        return subprocess.CompletedProcess(command, 0, '', '')
                    return subprocess.CompletedProcess(command, 0, probe_text, '')

                with patch.object(generator.subprocess, 'run', side_effect=child):
                    with self.assertRaisesRegex(generator.GenerationError, 'stage=probe:'):
                        generator.generate('/oracle/ffmpeg', '/oracle/ffprobe', temporary,
                                           ['v2_still_rgba.png'])
                self.assertEqual(list(Path(temporary).iterdir()), [])


@unittest.skipUnless(os.environ.get('COREAV_V2_FFMPEG') and os.environ.get('COREAV_V2_FFPROBE'),
                     'optional short smoke requires caller-verified oracle paths')
class OracleSmoke(unittest.TestCase):
    def test_stills_and_multitrack_match_the_recipe_and_repeat_bytes(self):
        ffmpeg, ffprobe = os.environ['COREAV_V2_FFMPEG'], os.environ['COREAV_V2_FFPROBE']
        names = ('v2_still_rgba.png', 'v2_still_rgb.jpg', 'v2_multi_audio_eng.mp4',
                 'v2_still_rgba%03d.png')
        hashes = []
        for _ in range(2):
            with tempfile.TemporaryDirectory() as temporary:
                generator.generate(ffmpeg, ffprobe, temporary, names)
                hashes.append({name: hashlib.sha256((Path(temporary) / name).read_bytes()).hexdigest()
                               for name in names})
                percent = Path(temporary) / 'v2_still_rgba%03d.png'
                self.assertEqual(percent.read_bytes(),
                                 (Path(temporary) / 'v2_still_rgba000.png').read_bytes())
                self.assertEqual(percent.read_bytes(),
                                 (Path(temporary) / 'v2_still_rgba.png').read_bytes())
                for pattern in ('glob', 'sequence'):
                    generator.run(f'smoke-pattern:{pattern}', [
                        str(Path(ffmpeg).resolve()), '-v', 'error', '-nostdin',
                        '-filter_threads', '1', '-filter_complex_threads', '1',
                        '-threads', '1', '-pattern_type', pattern, '-framerate', '25',
                        '-i', percent.name, '-frames:v', '1', '-threads', '1', '-f', 'null', '-',
                    ], temporary, 30)
                streams = {}
                for name in names:
                    streams[name] = json.loads(generator.run('smoke-probe', [
                        str(Path(ffprobe).resolve()), '-v', 'error', '-threads', '1',
                        '-show_streams', '-of', 'json', name,
                    ], temporary, 30))['streams']
                self.assertEqual(streams[names[0]][0]['pix_fmt'], 'rgba')
                self.assertEqual(streams[names[1]][0]['codec_name'], 'mjpeg')
                multi = streams[names[2]]
                self.assertEqual([stream['codec_name'] for stream in multi],
                                 ['h264', 'aac', 'aac', 'aac', 'aac'])
                self.assertEqual(multi[1]['tags']['language'], 'eng')
                for stream in multi[1:]:
                    self.assertEqual(stream['sample_rate'], '48000')
                    self.assertEqual(stream['channels'], 1)
                for stream in (streams[names[0]][0], streams[names[1]][0], multi[0]):
                    self.assertEqual((stream['width'], stream['height']), (64, 48))
        self.assertEqual(hashes[0], hashes[1])


if __name__ == '__main__':
    unittest.main()
