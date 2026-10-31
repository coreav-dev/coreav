"""decode_breadth.py on the real binaries and streams the test encodes with the
pinned ffmpeg: V1's fixture check in CI (docs/measurements/compat-0.1.0/
codec-coverage-plan.md, "The tools"). Set COREAV_DECODE_ORACLE, COREAV_FFMPEG and
COREAV_FFPROBE to the built decode-oracle (features native-h264,dav1d,libvpx
on Linux) and the pinned ffmpeg and ffprobe; without them every test skips.
Streams CoreAV's build does not decode here (no sandbox off Linux) and
codecs the ffmpeg has no encoder for are left out. Synthetic media only."""
import argparse
import contextlib
import hashlib
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import decode_breadth  # noqa: E402

PROGRAMS = {k: os.environ.get(v) for k, v in (('decode_oracle', 'COREAV_DECODE_ORACLE'), ('ffmpeg', 'COREAV_FFMPEG'),
                                              ('ffprobe', 'COREAV_FFPROBE'))}
SANDBOX = sys.platform.startswith('linux')
FRAMES = '40'


@unittest.skipUnless(all(PROGRAMS.values()), 'COREAV_DECODE_ORACLE, COREAV_FFMPEG and COREAV_FFPROBE are not set')
class Fixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = pathlib.Path(tempfile.mkdtemp(prefix='decode-breadth-fixtures-'))
        cls.entries = []
        cls.made = {}
        cases = [
            ('h264.mp4', 'mp4', ['-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-bf', '2'], True),
            ('h264.mkv', 'mkv', ['-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-bf', '0'], True),
            ('h264_422.mp4', 'mp4', ['-c:v', 'libx264', '-pix_fmt', 'yuv422p'], True),
            ('h264_10bit.mp4', 'mp4', ['-c:v', 'libx264', '-pix_fmt', 'yuv420p10le'], True),
            ('h264_interlaced.mp4', 'mp4', ['-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-flags', '+ilme+ildct',
                                             '-x264opts', 'tff=1'], True),
            ('h264.avi', 'avi', ['-c:v', 'libx264', '-pix_fmt', 'yuv420p'], True),
            ('vp9.webm', 'webm', ['-c:v', 'libvpx-vp9'], SANDBOX),
            ('vp8.webm', 'webm', ['-c:v', 'libvpx'], SANDBOX),
            ('av1.mkv', 'mkv', ['-c:v', 'libsvtav1'], SANDBOX),
            ('hevc.mp4', 'mp4', ['-c:v', 'libx265', '-pix_fmt', 'yuv420p'], True),
        ]
        for name, ext, encoder, enabled in cases:
            path = cls.dir / name
            done = subprocess.run([PROGRAMS['ffmpeg'], '-nostdin', '-v', 'error', '-y', '-f', 'lavfi', '-i',
                                   'testsrc=size=160x96:rate=25:duration=4', '-frames:v', FRAMES, *encoder, str(path)],
                                  capture_output=True)
            if done.returncode == 0 and path.exists() and enabled:
                sha = hashlib.sha256(name.encode()).hexdigest()
                cls.made[name] = sha
                cls.entries.append({'sha256': sha, 'key': 'fixtures/' + name, 'ext': ext, 'path': str(path)})
        (cls.dir / 'files.json').write_text(json.dumps(cls.entries))
        opts = argparse.Namespace(files=str(cls.dir / 'files.json'), records=str(cls.dir / 'records.jsonl'), shard=None,
                                  fetch=None, deadline=None, limit_minutes=10.0, rerun_host=False, jobs=2,
                                  **PROGRAMS)
        with contextlib.redirect_stdout(io.StringIO()):
            decode_breadth.run(opts)
        cls.records = {r['sha256']: r for r in decode_breadth.load([cls.dir / 'records.jsonl'])}

    @classmethod
    def tearDownClass(cls):
        __import__('shutil').rmtree(cls.dir, ignore_errors=True)

    def record(self, name):
        if name not in self.made:
            self.skipTest(f'{name} could not be made here')
        return self.records[self.made[name]]

    def test_h264_in_an_mp4_and_a_matroska_agrees_with_the_reference(self):
        for name in ('h264.mp4', 'h264.mkv'):
            r = self.record(name)
            self.assertEqual((r['counted'], r['code'], r['host_limit']), (True, None, None), r)
            self.assertEqual((r['coreav']['backend'], r['reference']['frames'], r['coreav']['frames']), ('native', 40, 40))

    def test_h264_outside_the_native_decoder_is_out_of_scope(self):
        r = self.record('h264_422.mp4')
        self.assertEqual((r['code'], r['sub']), ('out_of_scope', 'chroma'))
        ten = self.record('h264_10bit.mp4')
        self.assertEqual((ten['code'], ten['sub']), ('out_of_scope', '10bit'))
        interlaced = self.record('h264_interlaced.mp4')
        if interlaced['properties']['field_order'] in (None, 'progressive', 'unknown'):
            self.skipTest('this encoder wrote a progressive stream')
        self.assertEqual((interlaced['code'], interlaced['sub']), ('out_of_scope', 'interlaced'))

    def test_a_container_coreav_does_not_read_is_not_read(self):
        r = self.record('h264.avi')
        self.assertEqual((r['code'], r['sub']), ('not_read', 'container'))

    def test_the_sandboxed_backends_agree_with_the_reference(self):
        for name, backend in (('vp9.webm', 'libvpx'), ('vp8.webm', 'libvpx'), ('av1.mkv', 'dav1d')):
            r = self.record(name)
            self.assertTrue(r['counted'], r)
            self.assertEqual(r['coreav']['backend'], backend)

    def test_a_codec_outside_the_build_is_listed_by_name(self):
        r = self.record('hevc.mp4')
        self.assertEqual((r['group'], r['code'], r['sub']), ('other', 'codec_not_in_build', 'hevc'))

    def test_the_summary_of_the_run_is_complete(self):
        summary = pathlib.Path(self.dir / 'summary.json')
        opts = argparse.Namespace(records=[str(self.dir / 'records.jsonl')], rerun=None,
                                  files=str(self.dir / 'files.json'), summary=str(summary))
        with contextlib.redirect_stdout(io.StringIO()):
            code = decode_breadth.summarize(opts)
        s = json.loads(summary.read_text())
        self.assertEqual((code, s['void'], s['files']), (0, False, len(self.entries)))
        self.assertEqual(s['denominator'], len(self.entries))


FFMPEG = PROGRAMS['ffmpeg']
T709 = 'colorprim=bt709:transfer=bt709:colormatrix=bt709'
T601 = 'colorprim=smpte170m:transfer=smpte170m:colormatrix=smpte170m'
V709 = ['-colorspace', 'bt709', '-color_primaries', 'bt709', '-color_trc', 'bt709']
V601 = ['-colorspace', 'smpte170m', '-color_primaries', 'smpte170m', '-color_trc', 'smpte170m']
VP9 = ['-c:v', 'libvpx-vp9', '-b:v', '500k', '-g', '25', '-auto-alt-ref', '0']


@unittest.skipUnless(FFMPEG, 'COREAV_FFMPEG is not set')
class UnconvertedReference(unittest.TestCase):
    """The reference's no-conversion pin (codec-coverage-plan.md, "The reference"): a stream whose colour
    description, range, pixel format or size changes mid-stream gives the MD5s of its segments decoded one at a time,
    where the registered command without the pin converts them, and a stream with no change gives the MD5s the
    command gives without the pin."""

    @classmethod
    def setUpClass(cls):
        cls.dir = pathlib.Path(tempfile.mkdtemp(prefix='decode-breadth-unconverted-'))
        (cls.dir / 'gconv').mkdir()
        cls.env = {'PATH': os.environ.get('PATH', ''), 'HOME': os.environ.get('HOME', ''),
                   'GCONV_PATH': str(cls.dir / 'gconv')}

    @classmethod
    def tearDownClass(cls):
        __import__('shutil').rmtree(cls.dir, ignore_errors=True)

    def ff(self, *args):
        done = subprocess.run([FFMPEG, '-nostdin', '-v', 'error', '-y', *args], capture_output=True, text=True,
                              env=self.env)
        self.assertEqual(done.returncode, 0, done.stderr)

    def source(self, w, h, hue):
        return ['-f', 'lavfi', '-i', f'testsrc2=size={w}x{h}:rate=25:duration=1,hue=h={hue}']

    def h264(self, name, ext, segments):
        """Segments (width, height, pix_fmt, x264 parameters) encoded one by one, joined as Annex B, each segment
        and the whole muxed into EXT."""
        whole, parts = b'', []
        for i, (w, h, pix_fmt, params) in enumerate(segments):
            raw = self.dir / f'{name}.{i}.h264'
            self.ff(*self.source(w, h, 60 * i), '-pix_fmt', pix_fmt, '-c:v', 'libx264', '-preset', 'veryfast',
                    '-g', '25', '-bf', '0', '-x264-params', ':'.join(['scenecut=0', *params]), '-f', 'h264', str(raw))
            whole += raw.read_bytes()
            parts.append(raw)
        (self.dir / f'{name}.h264').write_bytes(whole)
        for raw in [*parts, self.dir / f'{name}.h264']:
            self.ff('-f', 'h264', '-framerate', '25', '-i', str(raw), '-c', 'copy', str(raw.with_suffix('.' + ext)))
        return self.dir / f'{name}.{ext}', [p.with_suffix('.' + ext) for p in parts]

    def vp9(self, name, segments):
        parts = []
        for i, (w, h, pix_fmt, tags) in enumerate(segments):
            part = self.dir / f'{name}.{i}.webm'
            self.ff(*self.source(w, h, 60 * i), '-pix_fmt', pix_fmt, *VP9, *tags, str(part))
            parts.append(part)
        listing = self.dir / f'{name}.txt'
        listing.write_text(''.join(f"file '{p}'\n" for p in parts))
        self.ff('-f', 'concat', '-safe', '0', '-i', str(listing), '-c', 'copy', str(self.dir / f'{name}.webm'))
        return self.dir / f'{name}.webm', parts

    def md5s(self, path, unconverted, codec='h264', pix_fmt='yuv420p', decoder=None):
        backend = 'libvpx' if codec == 'vp9' and not decoder else 'ffmpeg'
        cmd = decode_breadth.thumbnail_oracle.framemd5_command(FFMPEG, path, path.suffix[1:], 0, None, backend, codec,
                                                               pix_fmt, unconverted=unconverted)
        if decoder:
            cmd[cmd.index('-i'):cmd.index('-i')] = ['-c:v', decoder]
        done = subprocess.run(cmd, capture_output=True, text=True, env=self.env)
        self.assertEqual((done.returncode, done.stderr), (0, ''), cmd)
        return [line.rsplit(',', 1)[-1].strip() for line in done.stdout.splitlines() if line and line[0] != '#']

    def assert_unconverted(self, whole, parts, **kw):
        truth = [md5 for part in parts for md5 in self.md5s(part, False, **kw)]
        self.assertEqual(len(truth), 25 * len(parts))
        self.assertEqual(self.md5s(whole, True, **kw), truth, whole.name)
        self.assertNotEqual(self.md5s(whole, False, **kw), truth, f'{whole.name} does not make FFmpeg convert')

    def test_a_change_of_colour_description_range_or_pixel_format_is_not_converted(self):
        cases = {
            'colour': ('mp4', [(320, 240, 'yuv420p', [T709]), (320, 240, 'yuv420p', [T601]),
                               (320, 240, 'yuv420p', [T709])]),
            'range': ('mkv', [(320, 240, 'yuv420p', [T709, 'fullrange=off']),
                              (320, 240, 'yuv420p', [T709, 'fullrange=on']),
                              (320, 240, 'yuv420p', [T709, 'fullrange=off'])]),
            'pixel_format': ('mp4', [(320, 240, 'yuv420p', [T709]), (320, 240, 'yuv422p', [T709]),
                                     (320, 240, 'yuv420p', [T709])]),
            'colour_and_size': ('mkv', [(320, 240, 'yuv420p', [T709]), (160, 120, 'yuv420p', [T601]),
                                        (320, 240, 'yuv420p', [T709])]),
        }
        for name, (ext, segments) in cases.items():
            with self.subTest(name):
                self.assert_unconverted(*self.h264(name, ext, segments))

    def test_vp9_and_a_stream_with_alpha_are_not_converted(self):
        segments = [(320, 240, 'yuv420p', V709), (160, 120, 'yuv420p', V601), (320, 240, 'yuv420p', V709)]
        self.assert_unconverted(*self.vp9('vp9_colour_and_size', segments), codec='vp9')
        alpha = [(w, h, 'yuva420p', tags) for w, h, _, tags in segments]
        self.assert_unconverted(*self.vp9('alpha_colour_and_size', alpha), codec='vp9', pix_fmt='yuva420p',
                                decoder='libvpx-vp9')

    def test_a_stream_without_a_change_keeps_its_md5s(self):
        plain, _ = self.h264('plain', 'mp4', [(320, 240, 'yuv420p', [])])
        tagged, _ = self.h264('tagged', 'mkv', [(320, 240, 'yuv420p', [T709])])
        ten, _ = self.h264('ten_bit', 'mp4', [(320, 240, 'yuv420p10le', [])])
        for path in (plain, tagged, ten):
            self.assertEqual(self.md5s(path, True), self.md5s(path, False), path.name)
        vp9, _ = self.vp9('vp9', [(320, 240, 'yuv420p', V709)])
        self.assertEqual(self.md5s(vp9, True, codec='vp9'), self.md5s(vp9, False, codec='vp9'))
        alpha, _ = self.vp9('alpha', [(320, 240, 'yuva420p', [])])
        kw = {'codec': 'vp9', 'pix_fmt': 'yuva420p', 'decoder': 'libvpx-vp9'}
        self.assertEqual(self.md5s(alpha, True, **kw), self.md5s(alpha, False, **kw))


    def pix_fmt(self, path):
        done = subprocess.run([PROGRAMS['ffprobe'], '-v', 'error', '-select_streams', 'v:0', '-show_entries',
                               'stream=pix_fmt', '-of', 'csv=p=0', str(path)], capture_output=True, text=True,
                              env=self.env)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.strip()

    def test_the_alpha_path_never_reaches_a_codec_v1_counts(self):
        """With -pix_fmt (an alpha format) the pin keeps colour and range, but a mid-stream change of pixel format
        is still converted to the first frame's format without alpha. That path is taken only where ffprobe lists
        an alpha format: VP9 and VP8 with alpha are listed without it, while an alpha codec where the change is
        converted, ProRes, is in group other, which misses as codec_not_in_build before any MD5 is compared."""
        if not PROGRAMS['ffprobe']:
            self.skipTest('COREAV_FFPROBE is not set')
        for name, encoder in (('vp9', ['-c:v', 'libvpx-vp9']), ('vp8', ['-c:v', 'libvpx'])):
            path = self.dir / f'{name}_alpha.webm'
            self.ff(*self.source(64, 48, 0), '-pix_fmt', 'yuva420p', *encoder, '-auto-alt-ref', '0', str(path))
            self.assertEqual(self.pix_fmt(path), 'yuv420p', name)
            self.assertNotIn('-pix_fmt', decode_breadth.thumbnail_oracle.framemd5_command(
                FFMPEG, path, 'webm', 0, None, 'libvpx', name, self.pix_fmt(path), unconverted=True))
        parts = []
        for i, (profile, pix_fmt) in enumerate((('4444', 'yuva444p10le'), ('standard', 'yuv422p10le'))):
            part = self.dir / f'prores.{i}.mov'
            self.ff(*self.source(32, 32, 60 * i), '-pix_fmt', pix_fmt, '-c:v', 'prores_ks', '-profile:v', profile,
                    str(part))
            parts.append(part)
        listing = self.dir / 'prores.txt'
        listing.write_text(''.join(f"file '{p}'\n" for p in parts))
        whole = self.dir / 'prores.mov'
        self.ff('-f', 'concat', '-safe', '0', '-i', str(listing), '-c', 'copy', str(whole))
        alpha = self.pix_fmt(whole)
        self.assertTrue(alpha.startswith('yuva'), alpha)
        self.assertNotIn('prores', decode_breadth.GROUPS)
        truth = [m for p in parts for m in self.md5s(p, False, codec='prores', pix_fmt=self.pix_fmt(p))]
        self.assertNotEqual(self.md5s(whole, True, codec='prores', pix_fmt=alpha), truth)


if __name__ == '__main__':
    unittest.main()
