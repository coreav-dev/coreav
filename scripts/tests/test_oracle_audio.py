"""Oracle audio inputs use the shared options, after any video thread setting."""
import ast
from fractions import Fraction
import os
import pathlib
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import clip_check
import decode_audio_check
import extract_check
import hls_check
import join_check
import mkv_remux_check
import oracle_audio
import track_boxes


class AudioOracle(unittest.TestCase):
    def assert_input(self, command):
        at = command.index('-i')
        self.assertEqual(command[at - 2:at], ['-threads:a', '1'])
        self.assertNotIn('-threads:a', command[at + 2:])

    def test_shared_options_only_set_audio_input_threads(self):
        self.assertEqual(oracle_audio.INPUT_ARGS, ('-threads:a', '1'))

    def test_repository_audio_commands_have_one_owner(self):
        self.assertEqual(list(oracle_audio.problems(ROOT)), [])

    def test_lint_rejects_the_old_clip_hls_and_join_commands(self):
        for rel in ('scripts/clip_check.py', 'scripts/extract_check.py', 'scripts/mkv_remux_check.py', 'scripts/hls_check.py', 'scripts/join_check.py',
                    'scripts/decode_audio_check.py', 'scripts/aac_agreement.py', 'scripts/audio_copy_check.py'):
            old = (ROOT / rel).read_text().replace('*AUDIO_DECODE_INPUT, ', '')
            found = list(oracle_audio.python_problems(old, rel))
            self.assertTrue(found, rel)
            self.assertTrue(all('oracle_audio.INPUT_ARGS' in p for p in found))

    def test_lint_rejects_output_options_and_each_unowned_input(self):
        for command in (
                "[ffmpeg, '-i', source, *AUDIO_DECODE_INPUT, '-f', 'f32le', '-']",
                "[ffmpeg, *AUDIO_DECODE_INPUT, '-i', source, '-i', other, '-f', 'f32le', '-']",
                "[ffmpeg, '-i', source, '-c:v', 'copy', '-f', 'f32le', '-']",
                "[ffmpeg, '-i', source, '-c', 'copy', '-c:a', 'pcm_f32le', '-f', 'f32le', '-']",
                "[ffmpeg, '-i', source, '-f', 'framemd5', '-']",
                "[ffmpeg, '-i', source, '-map', '0:a:0', '-f', 'md5', '-']"):
            text = 'from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT\ndef compare():\n    run(' + command + ')'
            self.assertTrue(list(oracle_audio.python_problems(text, 'tools/new.py')))

    def test_lint_requires_the_owner_import_and_refuses_reassignment(self):
        command = "def compare():\n    run([ffmpeg, *AUDIO_DECODE_INPUT, '-i', source, '-f', 'f32le', '-'])"
        owner = 'from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT\n'
        self.assertEqual(list(oracle_audio.python_problems(owner + command, 'tools/new.py')), [])
        for prefix in ('', owner + 'AUDIO_DECODE_INPUT = []\n'):
            self.assertTrue(list(oracle_audio.python_problems(prefix + command, 'tools/new.py')))

    def test_lint_rejects_new_shell_and_rust_audio_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / 'tests').mkdir()
            (root / 'check.sh').write_text('ffmpeg -i input -f framemd5 -\n')
            (root / 'tests/new.rs').write_text('Command::new(&ffmpeg).args(["-i", "in", "-f", "f32le", "-"]).output();')
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            found = list(oracle_audio.problems(root))
            self.assertEqual(len(found), 2)
            self.assertTrue(all(oracle_audio.ARG_FILE in p for p in found))

    def test_clip_frames_keep_video_threads_but_override_audio(self):
        for video in (False, True):
            with patch.object(clip_check, 'THREADS', ['-threads', '8']), patch.object(clip_check, 'decode', return_value=b'') as run:
                clip_check.framemd5('ffmpeg', 'source.mp4', 2, video=video)
                command = run.call_args.args[0]
                self.assert_input(command)
                self.assertEqual(command[command.index('-threads') + 1], '8')

    def test_clip_pcm_input_with_and_without_edits(self):
        self.addCleanup(clip_check.clear_cache)
        for raw in (False, True):
            with patch.object(clip_check, 'framemd5', return_value=(Fraction(1, 48000), [(0, 8, 'hash', 2)])), \
                    patch.object(clip_check, 'decode') as run:
                clip_check.pcm('ffmpeg', 'source.mp4', 2, 2, raw=raw)
                self.assert_input(run.call_args.args[0])

    def test_hls_frames_preserve_general_thread_override(self):
        for index in (None, 1):
            with patch.object(hls_check, 'THREADS', ['-threads', '8']), \
                    patch.object(hls_check, 'run', return_value=SimpleNamespace(stdout=b'')) as run:
                hls_check.frames('master.m3u8', index)
                command = run.call_args.args[0]
                self.assert_input(command)
                self.assertEqual(command[command.index('-threads') + 1], '8')

    def test_join_adts_and_frame_positions(self):
        self.addCleanup(clip_check.clear_cache)
        done = subprocess.CompletedProcess([], 0, b'', b'')
        with patch.object(join_check.subprocess, 'run', return_value=done) as run:
            self.assertIsNotNone(join_check._decode_adts('ffmpeg', [b'adts']))
            self.assertEqual(len(run.call_args_list), 2)
            for call in run.call_args_list:
                self.assert_input(call.args[0])

    def test_decode_audio_input_with_and_without_edits(self):
        for lossy in (False, True):
            for ignore in (False, True):
                self.assert_input(decode_audio_check.decode_args(SimpleNamespace(ffmpeg='ffmpeg'), 'source.mp4',
                                                                {'index': 1, 'lossy': lossy}, ignore))

    def test_extraction_frames_use_owned_audio_threads_with_and_without_edits(self):
        for raw in (False, True):
            with patch.object(extract_check, 'run', return_value=b'') as run:
                extract_check.framemd5('ffmpeg', 'source.mp4', 0, raw=raw)
                self.assert_input(run.call_args.args[0])

    def test_matroska_hash_helper_owns_audio_even_when_copy_is_selected_later(self):
        for copy in (False, True):
            with patch.object(mkv_remux_check, 'run', return_value=(0, b'')) as run:
                mkv_remux_check.framemd5('source.mkv', 0, copy=copy)
                self.assert_input(run.call_args.args[0])

    def test_every_python_audio_input_places_shared_options_last(self):
        for rel in ('scripts/clip_check.py', 'scripts/extract_check.py', 'scripts/mkv_remux_check.py', 'scripts/hls_check.py', 'scripts/join_check.py',
                    'scripts/decode_audio_check.py', 'scripts/aac_agreement.py', 'scripts/audio_copy_check.py'):
            for seq in ast.walk(ast.parse((ROOT / rel).read_text())):
                if not isinstance(seq, ast.List):
                    continue
                stars = [i for i, n in enumerate(seq.elts) if isinstance(n, ast.Starred) and
                         'AUDIO_DECODE_INPUT' in ast.unparse(n)]
                if not stars:
                    continue
                rest = seq.elts[stars[-1] + 1:]
                if rest:
                    self.assertEqual(ast.literal_eval(rest[0]), '-i', (rel, seq.lineno))
                else:
                    self.fail(f'{rel}:{seq.lineno}: put the shared options immediately before -i')

    def test_shell_and_rust_read_the_shared_file_before_input(self):
        shell = (ROOT / 'scripts/oracle-ffprobe.sh').read_text()
        self.assertIn('mapfile -t AUDIO_DECODE_INPUT < "$ROOT/' + oracle_audio.ARG_FILE + '"', shell)
        self.assertIn('"${AUDIO_DECODE_INPUT[@]}" -i "$MEDIA_FILE"', shell)
        rust = (ROOT / 'crates/coreav-cli/tests/decode_audio.rs').read_text()
        self.assertIn('.args(include_str!("../../../' + oracle_audio.ARG_FILE + '").split_whitespace())\n        .arg("-i")', rust)

    @unittest.skipUnless(os.environ.get('COREAV_FIXTURES') and os.environ.get('COREAV_FFMPEG'),
                         'needs existing fixtures and the pinned FFmpeg')
    def test_clipped_lossless_fixtures_decode_the_source_samples(self):
        ffmpeg = os.environ['COREAV_FFMPEG']
        self.addCleanup(clip_check.clear_cache)
        extract_run = extract_check.run
        with tempfile.TemporaryDirectory() as tmp, patch.object(clip_check, 'THREADS', ['-threads', '8']):
            for name in ('codec_flac16_44k.mp4', 'codec_alac16.m4a'):
                with self.subTest(name=name):
                    source = pathlib.Path(os.environ['COREAV_FIXTURES']) / name
                    movie = track_boxes.read_movie(source)
                    duration = movie['movie_timescale'] * 2 // 5
                    start = movie['tracks'][0]['timescale'] * 3 // 5
                    edited = pathlib.Path(tmp) / name
                    subprocess.run([sys.executable, '-I', str(ROOT / 'scripts/elst_craft.py'), str(source), str(edited),
                                    '0', f'{duration}:{start}'], check=True)
                    whole = clip_check.pcm(ffmpeg, str(source), 0, 1)
                    clipped = clip_check.pcm(ffmpeg, str(edited), 0, 1)
                    self.assertIsNotNone(whole)
                    self.assertIsNotNone(clipped)
                    self.assertEqual(pathlib.Path(clipped[2]).read_bytes(), pathlib.Path(whole[2]).read_bytes()[start * 4:])
                    for raw in (False, True):
                        _, expected = clip_check.framemd5(ffmpeg, str(edited), 0, raw=raw)
                        with patch.object(extract_check, 'run', side_effect=lambda cmd: extract_run([cmd[0], '-threads', '8', *cmd[1:]])):
                            actual = [line.split(',') for line in extract_check.framemd5(ffmpeg, str(edited), 0, raw=raw)]
                        self.assertEqual([(int(p[2]), int(p[4]), p[5].strip(), int(p[3])) for p in actual], expected)


if __name__ == '__main__':
    unittest.main()
