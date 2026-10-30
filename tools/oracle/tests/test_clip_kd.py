"""Keeps docs/known-divergences-clip.md, its JSON twin and the predicates of
scripts/clip_check.py in sync, and checks that each predicate rejects near
misses."""
import copy
import importlib.util
import json
import pathlib
import re
import shutil
import struct
import tempfile
import unittest
import zlib
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('clip_check', ROOT / 'scripts' / 'clip_check.py')
cc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cc)


class StoredDurations(unittest.TestCase):
    def test_stts_window_sum(self):
        entries = [(3, 1000), (2, 500), (1, 7)]
        self.assertEqual(cc.stts_window_sum(entries, 0, 6), 4007)
        self.assertEqual(cc.stts_window_sum(entries, 2, 2), 1500)
        self.assertEqual(cc.stts_window_sum(entries, 5, 1), 7)
        self.assertIsNone(cc.stts_window_sum(entries, 5, 2))
        self.assertIsNone(cc.stts_window_sum(None, 0, 1))


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-clip.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-clip.md').read_text()
        documented = set(re.findall(r'^## (KD-C-[A-Z-]+)$', page, re.M))
        self.assertEqual(documented, {entry['id'] for entry in twin['entries']})
        self.assertEqual({entry['predicate'] for entry in twin['entries']}, set(cc.PREDICATES))
        for entry in twin['entries']:
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')


def trak(elst, mdhd, stts_sum):
    return {'elst': elst, 'mdhd': mdhd, 'stts_sum': stts_sum, 'tkhd': 0}


def evidence(video=None, audio=None, streams=None, mvhd=(12800, 0)):
    traks = [video or trak([[20480, 1024]], (12800, 20480), 19456)]
    if audio is not None:
        traks.append(audio)
    return {
        'output_headers': {'brand': 'isom', 'mvhd': mvhd, 'traks': traks},
        'output_streams': streams or [{'duration_ts': 19456}, {'duration_ts': 34816, 'sample_rate': '48000'}],
        'entry': {'source': '/nonexistent', 'audio': None},
        'ffmpeg': 'ffmpeg',
    }


class Decode(unittest.TestCase):
    def test_a_decode_killed_by_a_signal_is_retried_with_one_thread(self):
        calls = []

        def fake(args):
            calls.append(args)
            if len(calls) == 1:
                raise RuntimeError("ffmpeg failed (-6): Assertion pkt failed")
            return b"ok"
        original = cc.run
        cc.run = fake
        try:
            self.assertEqual(cc.decode(["ffmpeg", "-threads", "2", "-i", "x"]), b"ok")
            self.assertEqual(calls[1], ["ffmpeg", "-threads", "1", "-i", "x"])
            # Without a thread option (ffmpeg's frame-threaded default, as in
            # the corpus runs) one thread is set before the input.
            calls.clear()
            self.assertEqual(cc.decode(["ffmpeg", "-v", "error", "-i", "x"]), b"ok")
            self.assertEqual(calls[1], ["ffmpeg", "-threads", "1", "-v", "error", "-i", "x"])
            # An ordinary failure is not retried.
            calls.clear()
            cc.run = lambda args: (_ for _ in ()).throw(RuntimeError("ffmpeg failed (1): bad"))
            with self.assertRaises(RuntimeError):
                cc.decode(["ffmpeg", "-threads", "2", "-i", "x"])
            with self.assertRaises(RuntimeError):
                cc.decode(["ffmpeg", "-i", "x"])
        finally:
            cc.run = original


def box(kind, payload):
    return struct.pack('>I4s', 8 + len(payload), kind) + payload


class CompressedMovie(unittest.TestCase):
    """A QuickTime movie whose `moov` holds only a compressed movie (`cmov`)
    is read through it (the source then read as having no tracks)."""

    def write(self, moov_payload):
        tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        path = tmp / 'movie.mov'
        path.write_bytes(box(b'ftyp', b'qt  \0\0\0\0') + box(b'moov', moov_payload))
        return str(path)

    def test_headers_and_edits_read_the_inflated_movie(self):
        mvhd = box(b'mvhd', bytes(12) + struct.pack('>II', 600, 1200) + bytes(80))
        tkhd = box(b'tkhd', bytes(12) + struct.pack('>I', 1) + bytes(4) + struct.pack('>I', 1200) + bytes(60))
        elst = box(b'elst', bytes(4) + struct.pack('>IIiI', 1, 1200, 169, 1 << 16))
        mdhd = box(b'mdhd', bytes(12) + struct.pack('>II', 600, 1200) + bytes(4))
        hdlr = box(b'hdlr', bytes(8) + b'vide' + bytes(12))
        trak = box(b'trak', tkhd + box(b'edts', elst) + box(b'mdia', mdhd + hdlr))
        movie = box(b'moov', mvhd + trak)
        compressed = zlib.compress(movie)
        cmov = box(b'cmov', box(b'dcom', b'zlib') + box(b'cmvd', struct.pack('>I', len(movie)) + compressed))
        path = self.write(cmov)
        headers = cc.headers(path)
        self.assertEqual(headers['mvhd'], (600, 1200))
        self.assertEqual(len(headers['traks']), 1)
        self.assertEqual(headers['traks'][0]['elst'], [[1200, 169]])
        self.assertEqual(headers['traks'][0]['mdhd'], (600, 1200))
        self.assertEqual(cc.full_edit_list(path, 0), (600, 600, [(1200, 169, 1 << 16)]))
        # A compressed movie that does not inflate to its stated size is
        # unreadable, not a movie without tracks.
        bad = box(b'cmov', box(b'dcom', b'zlib') + box(b'cmvd', struct.pack('>I', len(movie) + 1) + compressed))
        self.assertIsNone(cc.headers(self.write(bad)))


class Predicates(unittest.TestCase):
    def test_stts_short_needs_the_decode_durations_below_the_edit(self):
        c = evidence()
        self.assertTrue(cc.stts_short('video:duration', set(), c))
        self.assertFalse(cc.stts_short('audio:duration', set(), c))
        # ffprobe reports something else than the stts sum.
        other = copy.deepcopy(c)
        other['output_streams'][0]['duration_ts'] = 19457
        self.assertFalse(cc.stts_short('video:duration', set(), other))
        # The media header is shorter than the edit.
        short = evidence(video=trak([[20480, 1024]], (12800, 20000), 19456))
        self.assertFalse(cc.stts_short('video:duration', set(), short))
        # The stts sum is not shorter than the edit.
        long_stts = evidence(video=trak([[20480, 1024]], (12800, 20480), 20480),
                             streams=[{'duration_ts': 20480}])
        self.assertFalse(cc.stts_short('video:duration', set(), long_stts))
        # The edit compared in media ticks: a movie timescale of twice the
        # media's doubles the stored edit.
        doubled = evidence(video=trak([[40960, 1024]], (12800, 20480), 19456), mvhd=(25600, 0))
        self.assertTrue(cc.stts_short('video:duration', set(), doubled))

    def test_empty_edit_duration_follows_the_rule_with_the_empty_edit(self):
        # Audio at 48 kHz in a 12,800 Hz movie: 0.278 s empty, 0.722 s media,
        # 34 frames of 1024 kept.
        audio = trak([[3558, -1], [9241, 2]], (48000, 34816), 34816)
        c = evidence(audio=audio)
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), c), 'needs the source')
        c['entry']['audio'] = {'stream_index': 1, 'first_sample': 10, 'samples': 34}
        c['source_headers'] = copy.deepcopy(c['output_headers'])
        # The source stores 100 frames of 1024; samples 10 to 43 are kept.
        c['source_headers']['traks'][1]['stts'] = [(100, 1024)]
        self.assertTrue(cc.empty_edit_duration('audio:duration', set(), c))
        self.assertFalse(cc.empty_edit_duration('audio:pcm', set(), c))
        # The output's media timescale other than the source's.
        wrong = copy.deepcopy(c)
        wrong['source_headers']['traks'][1]['mdhd'] = (44100, 34816)
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), wrong))
        # Decode durations other than the kept samples' stored ones.
        short = copy.deepcopy(c)
        short['entry']['audio']['samples'] = 33
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), short))
        uneven = copy.deepcopy(c)
        uneven['source_headers']['traks'][1]['stts'] = [(40, 1024), (60, 960)]
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), uneven))
        beyond = copy.deepcopy(c)
        beyond['source_headers']['traks'][1]['stts'] = [(40, 1024)]
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), beyond))
        # Without an empty edit the rule gives the edit itself.
        plain = copy.deepcopy(c)
        plain['output_headers']['traks'][1]['elst'] = [[9241, 2]]
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), plain))
        # ffprobe's value is not the rule's.
        off = copy.deepcopy(c)
        off['output_streams'][1]['duration_ts'] = 34814
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), off))
        # The rule happens to give the media edit: nothing to cover.
        edit = (9241 * 48000 * 2 + 12800) // (2 * 12800)
        same = copy.deepcopy(c)
        same['output_headers']['traks'][1]['stts_sum'] = edit
        same['output_headers']['traks'][1]['mdhd'] = (48000, edit)
        same['output_streams'][1]['duration_ts'] = edit
        self.assertFalse(cc.empty_edit_duration('audio:duration', set(), same))

    def test_audio_restart_needs_one_media_edit_and_output_audio(self):
        audio = trak([[3558, -1], [9241, 2]], (48000, 34816), 34816)
        c = evidence(audio=audio)
        self.assertFalse(cc.audio_restart('audio:pcm', set(), c))  # no audio in the entry
        c['entry']['audio'] = {'stream_index': 1, 'first_sample': 0, 'samples': 3}
        self.assertFalse(cc.audio_restart('audio:pcm', set(), c))  # no decoded output
        c['audio_output_pcm'] = ([], 1, b'\0' * 8)
        self.assertFalse(cc.audio_restart('video:framemd5', set(), c))
        two = copy.deepcopy(c)
        two['output_headers']['traks'][1]['elst'] = [[100, 0], [100, 1024]]
        self.assertFalse(cc.audio_restart('audio:pcm', set(), two))

    def test_audio_edit_rate_rule_matches_the_measurements(self):
        # Media times and where ffmpeg started, as measured (frames of
        # 1,024 ticks decoded at twice the timescale).
        for media, start in [(0, 0), (500, 500), (1024, 2048), (1100, 2048), (1200, 2048),
                             (1376, 2048), (1500, 2048), (2048, 4096), (2500, 4096)]:
            self.assertEqual(cc.ffmpeg_audio_edit_start(media, 1024, 2), start)

    def test_audio_edit_rate_needs_a_scaled_rate_and_ffmpegs_start(self):
        import os
        import random
        import shutil
        import tempfile
        channels = 2
        decoded = random.Random(31).randbytes(40960)  # 5,120 stereo samples
        tmp = tempfile.mkdtemp()
        source_pcm = os.path.join(tmp, 'kept.f32')

        def kept(_c):
            with open(source_pcm, 'wb') as f:
                f.write(decoded)
            return source_pcm

        def output_from(sample):
            path = os.path.join(tmp, f'out{sample}.f32')
            with open(path, 'wb') as f:
                f.write(decoded[sample * 8:sample * 8 + 8000])
            return path

        audio = trak([[12000, 1200]], (24000, 24576), 24576)
        c = evidence(audio=audio, streams=[{}, {'sample_rate': '48000'}])
        c['entry']['audio'] = {'stream_index': 1, 'first_sample': 0, 'samples': 3}
        c['audio_run'] = ([{'duration': 1024}] * 3, [], 0)
        c['audio_output_pcm'] = ([], channels, output_from(2048))
        original, timing = cc.kept_audio_decoded, cc._audio_timing_is_the_sources
        cc.kept_audio_decoded = kept
        try:
            # Timing other than the source's (checked on its own below).
            cc._audio_timing_is_the_sources = lambda *_: False
            self.assertFalse(cc.audio_edit_rate('audio:pcm', set(), c))
            cc._audio_timing_is_the_sources = lambda *_: True
            self.assertTrue(cc.audio_edit_rate('audio:pcm', set(), c))
            self.assertFalse(cc.audio_edit_rate('video:framemd5', set(), c))
            # The edit's own position, or another one.
            for sample in (2400, 2047):
                other = copy.deepcopy(c)
                other['audio_output_pcm'] = ([], channels, output_from(sample))
                self.assertFalse(cc.audio_edit_rate('audio:pcm', set(), other))
            # A media time on a whole frame.
            whole = copy.deepcopy(c)
            whole['output_headers']['traks'][1]['elst'] = [[12000, 1024]]
            self.assertFalse(cc.audio_edit_rate('audio:pcm', set(), whole))
            # Decoded at the media timescale.
            same = copy.deepcopy(c)
            same['output_streams'][1]['sample_rate'] = '24000'
            self.assertFalse(cc.audio_edit_rate('audio:pcm', set(), same))
            # Two media edits, or packets of different durations.
            two = copy.deepcopy(c)
            two['output_headers']['traks'][1]['elst'] = [[100, 0], [100, 1200]]
            self.assertFalse(cc.audio_edit_rate('audio:pcm', set(), two))
            uneven = copy.deepcopy(c)
            uneven['audio_run'] = ([{'duration': 1024}, {'duration': 960}], [], 0)
            self.assertFalse(cc.audio_edit_rate('audio:pcm', set(), uneven))
        finally:
            cc.kept_audio_decoded, cc._audio_timing_is_the_sources = original, timing
            shutil.rmtree(tmp, ignore_errors=True)

    def test_audio_timing_is_the_sources_reads_both_files(self):
        audio = trak([[100, 0]], (48000, 34816), 34816)
        audio['stts'] = [(34, 1024)]
        c = evidence(audio=audio)
        c['entry']['audio'] = {'stream_index': 1, 'first_sample': 10, 'samples': 34}
        c['source_headers'] = copy.deepcopy(c['output_headers'])
        c['source_headers']['traks'][1]['stts'] = [(60, 1024)]
        with mock.patch.object(cc, '_audio_window_kept', return_value=True), \
                mock.patch.object(cc, '_audio_media_time_is_the_sources', return_value=True):
            self.assertTrue(cc._audio_timing_is_the_sources(c, audio, 0))
            # Decode durations one tick short, or one sample fewer.
            short = copy.deepcopy(audio)
            short['stts'] = [(34, 1023)]
            self.assertFalse(cc._audio_timing_is_the_sources(c, short, 0))
            fewer = copy.deepcopy(audio)
            fewer['stts'] = [(33, 1024)]
            self.assertFalse(cc._audio_timing_is_the_sources(c, fewer, 0))
            beyond = copy.deepcopy(c)
            beyond['entry']['audio']['first_sample'] = 30
            self.assertFalse(cc._audio_timing_is_the_sources(beyond, audio, 0))
            unread = copy.deepcopy(c)
            del unread['source_headers']
            self.assertFalse(cc._audio_timing_is_the_sources(unread, audio, 0))
        with mock.patch.object(cc, '_audio_window_kept', return_value=True), \
                mock.patch.object(cc, '_audio_media_time_is_the_sources', return_value=False):
            self.assertFalse(cc._audio_timing_is_the_sources(c, audio, 0))
        with mock.patch.object(cc, '_audio_window_kept', return_value=False), \
                mock.patch.object(cc, '_audio_media_time_is_the_sources', return_value=True):
            self.assertFalse(cc._audio_timing_is_the_sources(c, audio, 0))

    def test_aac_profile_needs_the_same_configuration(self):
        c = evidence()
        c['output_streams'] = [{}, {'codec_name': 'aac', 'profile': 'unknown'}]
        c['audio_source_stream'] = {'codec_name': 'aac', 'profile': 'LC'}
        c['audio_source_extradata'] = c['audio_output_extradata'] = '00000000: 1190'
        self.assertTrue(cc.aac_profile('audio:profile', set(), c))
        self.assertFalse(cc.aac_profile('audio:channels', set(), c))
        other = copy.deepcopy(c)
        other['audio_output_extradata'] = '00000000: 1210'
        self.assertFalse(cc.aac_profile('audio:profile', set(), other))
        renamed = copy.deepcopy(c)
        renamed['output_streams'][1]['profile'] = 'HE-AAC'
        self.assertTrue(cc.aac_profile('audio:profile', set(), renamed))
        same = copy.deepcopy(c)
        same['output_streams'][1]['profile'] = 'LC'
        self.assertFalse(cc.aac_profile('audio:profile', set(), same))
        mp3 = copy.deepcopy(c)
        mp3['audio_source_stream']['codec_name'] = 'mp3'
        self.assertFalse(cc.aac_profile('audio:profile', set(), mp3))

    def test_ctts_unsigned_needs_the_table_and_frames_in_order(self):
        c = evidence()
        c['output_headers']['traks'][0]['ctts_unsigned_large'] = True
        c['source_headers'] = {'brand': 'qt  ', 'mvhd': (600, 0), 'traks': [{'ctts_unsigned_large': True}]}
        c['entry'] = {'video': {'stream_index': 0}, 'audio': None}
        frames = [(i, 10, f'h{i}') for i in range(10)]
        c['video_frames'] = (frames, frames[:9])
        self.assertTrue(cc.ctts_unsigned('video:framemd5', set(), c))
        # A frame missing just before the last one.
        near_end = copy.deepcopy(c)
        near_end['video_frames'] = (frames, frames[:8] + frames[9:])
        self.assertTrue(cc.ctts_unsigned('video:framemd5', set(), near_end))
        # A frame missing in the middle of the clip.
        long = [(i, 10, f'h{i}') for i in range(30)]
        middle = copy.deepcopy(c)
        middle['video_frames'] = (long, long[:15] + long[16:])
        self.assertFalse(cc.ctts_unsigned('video:framemd5', set(), middle))
        self.assertFalse(cc.ctts_unsigned('video:duration', set(), c))
        changed = copy.deepcopy(c)
        changed['video_frames'] = (frames, frames[:8] + [(8, 10, 'other')])
        self.assertFalse(cc.ctts_unsigned('video:framemd5', set(), changed))
        many = copy.deepcopy(c)
        many['video_frames'] = (frames, frames[:5])
        self.assertFalse(cc.ctts_unsigned('video:framemd5', set(), many))
        signed = copy.deepcopy(c)
        signed['output_headers']['traks'][0]['ctts_unsigned_large'] = False
        self.assertFalse(cc.ctts_unsigned('video:framemd5', set(), signed))

    def test_edit_midframe_needs_a_constant_offset_below_a_frame(self):
        from fractions import Fraction
        c = evidence()
        c['source_headers'] = {'brand': 'mp42', 'mvhd': (30000, 0), 'traks': [{'elst': [[100, 64679]]}]}
        c['entry'] = {'video': {'stream_index': 0}, 'audio': None}
        run = [{'data_hash': f'h{i}', 'size': 9, 'flags': 'K_' if i == 0 else '__', 'pts': 55055 + 1001 * i,
                'dts': 55055 + 1001 * i, 'duration': 1001} for i in range(4)]
        out = [dict(p, pts=1001 * i, dts=1001 * i) for i, p in enumerate(run)]
        c['video_run'] = (run, out, Fraction(55441))
        self.assertTrue(cc.edit_midframe('video:packets', set(), c))
        self.assertFalse(cc.edit_midframe('audio:packets', set(), c))
        # The edit starts on a frame boundary.
        on_frame = copy.deepcopy(c)
        on_frame['source_headers']['traks'][0]['elst'] = [[100, 65065]]
        self.assertFalse(cc.edit_midframe('video:packets', set(), on_frame))
        # Offsets that are not constant, or a different payload.
        uneven = copy.deepcopy(c)
        uneven['video_run'][1][2]['pts'] += 1
        self.assertFalse(cc.edit_midframe('video:packets', set(), uneven))
        other = copy.deepcopy(c)
        other['video_run'][1][1]['data_hash'] = 'x'
        self.assertFalse(cc.edit_midframe('video:packets', set(), other))
        # An offset of a whole frame or more.
        far = copy.deepcopy(c)
        far['video_run'] = (run, out, Fraction(55055 - 1100))
        self.assertFalse(cc.edit_midframe('video:packets', set(), far))

    def test_edit_midframe_frames_follow_the_offset(self):
        from fractions import Fraction
        c = evidence()
        c['source_headers'] = {'brand': 'mp42', 'mvhd': (30000, 0), 'traks': [{'elst': [[100, 64679]]}]}
        c['entry'] = {'video': {'stream_index': 0}, 'audio': None}
        run = [{'data_hash': f'h{i}', 'size': 9, 'flags': 'K_' if i == 0 else '__', 'pts': 55055 + 1001 * i,
                'dts': 55055 + 1001 * i, 'duration': 1001} for i in range(32)]
        out = [dict(p, pts=1001 * i, dts=1001 * i) for i, p in enumerate(run)]
        c['video_run'] = (run, out, Fraction(55441))
        tb = Fraction(1, 30000)
        source = [(55055 + 1001 * i, 9, f'f{i}', None) for i in range(32)]
        clip = [(1001 * i, 9, f'f{i}', None) for i in range(30)]
        c['video_framemd5'] = ((tb, source), (tb, clip), Fraction(55441, 30000), Fraction(55441 + 30030, 30000))
        fields = {'video:packets', 'video:framemd5'}
        self.assertTrue(cc.edit_midframe('video:framemd5', fields, c))
        # The frame at the clip's end may be kept as well.
        at_end = copy.deepcopy(c)
        at_end['video_framemd5'] = ((tb, source), (tb, clip + [(30030, 9, 'f30', None)]),
                                    *c['video_framemd5'][2:])
        self.assertTrue(cc.edit_midframe('video:framemd5', fields, at_end))
        # A frame left out near the end needs unsigned offsets in the clip.
        short = copy.deepcopy(c)
        short['video_framemd5'] = ((tb, source), (tb, clip[:29]), *c['video_framemd5'][2:])
        self.assertFalse(cc.edit_midframe('video:framemd5', fields, short))
        short['output_headers']['traks'][0]['ctts_unsigned_large'] = True
        self.assertTrue(cc.edit_midframe('video:framemd5', fields, short))
        # Even then, not a frame in the middle.
        middle = copy.deepcopy(short)
        middle['video_framemd5'] = ((tb, source), (tb, clip[:15] + clip[16:]), *c['video_framemd5'][2:])
        self.assertFalse(cc.edit_midframe('video:framemd5', fields, middle))
        # A changed frame.
        changed = copy.deepcopy(c)
        changed['video_framemd5'] = ((tb, source), (tb, clip[:29] + [(29029, 9, 'x', None)]),
                                     *c['video_framemd5'][2:])
        self.assertFalse(cc.edit_midframe('video:framemd5', fields, changed))

    def test_edit_midframe_head_clip_spares_only_the_frame_past_the_edit(self):
        from fractions import Fraction
        # 30,000 Hz, frames of 1,001 ticks; both edits start at media time
        # 125,401, 276 ticks into a frame; the clip's edit lasts 0.3 s. On
        # the exact timeline, ffmpeg's last frame (0.3003 s) starts 725
        # ticks later, past the edit's end.
        def case(source_media=125401, clip_media=125401, durations=None, start=0, edit=9000):
            c = evidence(video=trak([[edit, clip_media]], (30000, 0), 0), mvhd=(30000, 0))
            c['source_headers'] = {'brand': 'isom', 'mvhd': (1000, 0),
                                   'traks': [{'elst': [[10034, source_media]]}]}
            c['entry'] = {'video': {'stream_index': 0}, 'audio': None,
                          'snapped_start': {'num': start, 'den': 1}}
            c['video_run'] = ([{'duration': d} for d in (durations or [1001] * 10)], [], 0)
            return c
        frames = [(Fraction(1001 * k, 30000), 10, f'h{k}') for k in range(10)]
        fields = {'video:framemd5'}
        good = case()
        good['video_frames'] = (frames, frames[:9])
        self.assertTrue(cc.edit_midframe('video:framemd5', fields, good))
        # Not the last frame: the first, two, or one in the middle.
        for actual in (frames[1:], frames[1:9], frames[:4] + frames[5:]):
            c = case()
            c['video_frames'] = (frames, actual)
            self.assertFalse(cc.edit_midframe('video:framemd5', fields, c))
        # The last frame, but well inside the clip's edit (an off-by-one).
        long = case(edit=30000)
        long['video_frames'] = (frames, frames[:9])
        self.assertFalse(cc.edit_midframe('video:framemd5', fields, long))
        # Either edit on a frame boundary, a variable frame rate, or a clip
        # that does not start where the source does.
        empty = case()
        empty['output_headers']['traks'][0]['elst'] = [[300, -1], [9000, 125401]]
        for c in (case(source_media=125125), case(clip_media=125125),
                  case(durations=[1001, 2002] * 5), case(start=3003), empty):
            c['video_frames'] = (frames, frames[:9])
            self.assertFalse(cc.edit_midframe('video:framemd5', fields, c))

    def test_audio_window_is_one_preroll_packet_then_the_clip(self):
        from fractions import Fraction
        # Frames of 1,024 ticks; a clip from 2,500 to 10,000 ticks: pre-roll
        # frame 1, then frames 2 (presented at 2,500) to 9 (presented at 10,000).
        src = [{'pts': 1024 * i, 'duration': 1024, 'flags': 'K__'} for i in range(20)]

        def case(start, count, zero=2500, end=10000):
            return {'audio_source_packets': src, 'audio_run': (src[start:start + count], [], 0),
                    'audio_run_start': start, 'audio_base': Fraction(1),
                    'entry': {'snapped_start': {'num': zero, 'den': 1}, 'written_end': {'num': end, 'den': 1}}}
        self.assertTrue(cc._audio_window_kept(case(1, 9)))
        for start, count in [(2, 8), (0, 10), (1, 8), (1, 10)]:
            self.assertFalse(cc._audio_window_kept(case(start, count)), (start, count))
        # From the source's start: no earlier packet, so no pre-roll.
        self.assertTrue(cc._audio_window_kept(case(0, 10, zero=0)))
        # Encoder priming (discarded packets) still gives the pre-roll: the
        # packet before the first presented one, discarded or not.
        primed_src = [dict(p, flags='K_D') if i < 2 else p for i, p in enumerate(src)]
        for start, count, kept in [(1, 9, True), (2, 8, False), (0, 10, False)]:
            primed = case(start, count)
            primed['audio_source_packets'] = primed_src
            self.assertEqual(cc._audio_window_kept(primed), kept, (start, count))
        # Audio that starts after the snapped start: no earlier packet.
        late = case(0, 8, zero=0)
        self.assertFalse(cc._audio_window_kept(late))  # stops before the end
        self.assertTrue(cc._audio_window_kept(case(0, 10, zero=0)))

    def test_pts_shift_needs_no_source_edit_and_the_clips_smallest_offset(self):
        from fractions import Fraction
        c = evidence()
        c['source_headers'] = {'brand': 'mp42', 'mvhd': (600, 0), 'traks': [{'elst': None}]}
        c['entry'] = {'video': {'stream_index': 0}, 'audio': None}
        c['output_headers']['traks'][0]['ctts_min'] = -100
        run = [{'data_hash': f'h{i}', 'size': 9, 'flags': 'K_' if i == 0 else '__',
                'pts': 1762400 + 100 * i, 'dts': 1762300 + 100 * i, 'duration': 100} for i in range(30)]
        out = [dict(p, pts=100 * i, dts=100 * i - 100) for i, p in enumerate(run)]
        c['video_run'] = (run, out, Fraction(1762300))
        tb = Fraction(1, 6000)
        source = [(1762400 + 100 * i, 9, f'f{i}', None) for i in range(32)]
        clip = [(100 * i, 9, f'f{i}', None) for i in range(30)]
        c['video_framemd5'] = ((tb, source), (tb, clip), Fraction(1762300, 6000), Fraction(1762300 + 3000, 6000))
        fields = {'video:packets', 'video:framemd5'}
        self.assertTrue(cc.pts_shift('video:packets', fields, c))
        self.assertTrue(cc.pts_shift('video:framemd5', fields, c))
        self.assertFalse(cc.pts_shift('video:duration', fields, c))
        # An edit list in the source.
        edited = copy.deepcopy(c)
        edited['source_headers']['traks'][0]['elst'] = [[1000, 0]]
        self.assertFalse(cc.pts_shift('video:packets', fields, edited))
        # No negative offset in the clip, or another size than the shift.
        for smallest in (None, 0, -200):
            other = copy.deepcopy(c)
            other['output_headers']['traks'][0]['ctts_min'] = smallest
            self.assertFalse(cc.pts_shift('video:packets', fields, other))
        # Only the presentation times shifted.
        pts_only = copy.deepcopy(c)
        pts_only['video_run'] = (run, [dict(p, dts=p['dts'] + 100) for p in out], Fraction(1762300))
        self.assertFalse(cc.pts_shift('video:packets', fields, pts_only))
        # A changed frame.
        changed = copy.deepcopy(c)
        changed['video_framemd5'] = ((tb, source), (tb, clip[:29] + [(2900, 9, 'x', None)]),
                                     *c['video_framemd5'][2:])
        self.assertFalse(cc.pts_shift('video:framemd5', fields, changed))

    def test_sync_verified_needs_fewer_flags_and_ffmpegs_key_frames(self):
        from fractions import Fraction
        c = evidence()
        c['ffprobe'] = 'ffprobe'
        c['entry']['output'] = '/nonexistent'
        run = [{'data_hash': f'h{i}', 'size': 9, 'flags': 'K__', 'pts': 1000 + 100 * i, 'dts': 1000 + 100 * i}
               for i in range(6)]
        out = [dict(p, pts=100 * i, dts=100 * i, pos=str(40 + 10 * i), flags='K__' if i in (0, 3) else '___')
               for i, p in enumerate(run)]
        c['video_run'] = (run, out, Fraction(1000))
        decoded = {'frames': [{'key_frame': int(i in (0, 3)), 'pkt_pos': str(40 + 10 * i)} for i in range(6)]}
        original = cc.ffprobe_json
        cc.ffprobe_json = lambda *args: decoded
        try:
            self.assertTrue(cc.sync_verified('video:packets', {'video:packets'}, c))
            self.assertFalse(cc.sync_verified('video:framemd5', {'video:packets'}, c))
            # No flag left out: nothing to cover.
            same = copy.deepcopy(c)
            same['video_run'] = (run, [dict(p, flags='K__') for p in out], Fraction(1000))
            self.assertFalse(cc.sync_verified('video:packets', {'video:packets'}, same))
            # A flag the source does not have.
            added = copy.deepcopy(c)
            added['video_run'][0][3]['flags'] = '___'
            self.assertFalse(cc.sync_verified('video:packets', {'video:packets'}, added))
            # The clip does not start with a keyframe.
            first = copy.deepcopy(c)
            first['video_run'][1][0]['flags'] = '___'
            self.assertFalse(cc.sync_verified('video:packets', {'video:packets'}, first))
            # A packet that differs in its data or its time, or one missing.
            for key, value in (('data_hash', 'x'), ('size', 8), ('pts', 201)):
                other = copy.deepcopy(c)
                other['video_run'][1][2][key] = value
                self.assertFalse(cc.sync_verified('video:packets', {'video:packets'}, other))
            short = copy.deepcopy(c)
            short['video_run'] = (run, out[:5], Fraction(1000))
            self.assertFalse(cc.sync_verified('video:packets', {'video:packets'}, short))
            # ffmpeg decodes another frame as a key frame, or does not
            # decode a flagged packet.
            for frames in ([{'key_frame': int(i in (0, 4)), 'pkt_pos': str(40 + 10 * i)} for i in range(6)],
                           [f for f in decoded['frames'] if f['pkt_pos'] != '70']):
                cc.ffprobe_json = lambda *args, frames=frames: {'frames': frames}
                self.assertFalse(cc.sync_verified('video:packets', {'video:packets'}, c))
        finally:
            cc.ffprobe_json = original

    def test_dts_shift_needs_equal_presentation_times(self):
        from fractions import Fraction
        c = evidence()
        c['output_headers']['traks'][0]['ctts_reads_negative'] = True
        run = [{'data_hash': f'h{i}', 'size': 9, 'flags': '__', 'pts': 12400 + 100 * i, 'dts': 12000 + 100 * i}
               for i in range(4)]
        out = [dict(p, pts=100 * i, dts=100 * i - 200) for i, p in enumerate(run)]
        c['video_run'] = (run, out, Fraction(12400))
        self.assertFalse(cc.dts_shift('video:packets', set(), c), 'needs both files\' offsets')
        # The source's smallest offset -400, the clip's -200: ffprobe moves
        # the source's decode times 200 further back.
        c['entry']['video'] = {'stream_index': 0}
        c['source_headers'] = copy.deepcopy(c['output_headers'])
        c['source_headers']['traks'][0]['ctts_min'] = -400
        c['output_headers']['traks'][0]['ctts_min'] = -200
        self.assertTrue(cc.dts_shift('video:packets', set(), c))
        # Another constant than the offsets give.
        other = copy.deepcopy(c)
        other['source_headers']['traks'][0]['ctts_min'] = -500
        self.assertFalse(cc.dts_shift('video:packets', set(), other))
        moved = copy.deepcopy(c)
        moved['video_run'][1][1]['pts'] += 1
        self.assertFalse(cc.dts_shift('video:packets', set(), moved))
        same = copy.deepcopy(c)
        same['video_run'] = (run, [dict(p, pts=100 * i, dts=100 * i - 400) for i, p in enumerate(run)],
                             Fraction(12400))
        self.assertFalse(cc.dts_shift('video:packets', set(), same))
        plain = copy.deepcopy(c)
        plain['output_headers']['traks'][0]['ctts_reads_negative'] = False
        self.assertFalse(cc.dts_shift('video:packets', set(), plain))

    def test_audio_jitter_needs_durations_unlike_the_frames(self):
        audio = trak([[100, 0]], (48000, 34816), 34816)
        c = evidence(audio=audio)
        c['entry']['audio'] = {'stream_index': 1, 'first_sample': 0, 'samples': 3}
        c['audio_output_pcm'] = ([(0, 1024), (1, 1024)], 1, '/nonexistent')
        c['audio_run'] = ([{'duration': 1024}, {'duration': 1024}], [], 0)
        # Stored durations equal to the frame length: nothing to cover.
        self.assertFalse(cc.audio_jitter('audio:pcm', set(), c))
        self.assertFalse(cc.audio_jitter('video:framemd5', set(), c))
        # Decoded frames of different lengths.
        mixed = copy.deepcopy(c)
        mixed['audio_run'] = ([{'duration': 25}, {'duration': 41}], [], 0)
        mixed['audio_output_pcm'] = ([(0, 1024), (1, 960)], 1, '/nonexistent')
        self.assertFalse(cc.audio_jitter('audio:pcm', set(), mixed))
        # A media timescale other than the sample rate.
        other = copy.deepcopy(mixed)
        other['audio_output_pcm'] = ([(0, 1024), (1, 1024)], 1, '/nonexistent')
        other['output_headers']['traks'][1]['mdhd'] = (44100, 34816)
        self.assertFalse(cc.audio_jitter('audio:pcm', set(), other))

    def test_adts_frames_are_split_by_their_lengths(self):
        frame = bytes([0xFF, 0xF1, 0x50, 0x80, 0x01, 0x3F, 0xFC]) + b'\x01\x02'
        # frame_length 9: ((0x80 & 3) << 11) | (0x01 << 3) | (0x3F >> 5) = 9.
        self.assertEqual(cc._adts_frames(frame * 3), [frame] * 3)
        self.assertIsNone(cc._adts_frames(frame + b'\x00'))
        self.assertIsNone(cc._adts_frames(b'\x00' * 9))


class Positions(unittest.TestCase):
    def test_times_map_through_gaps(self):
        from fractions import Fraction
        spans = [(Fraction(0), 1024), (Fraction(3528, 44100), 1024)]
        self.assertEqual(cc.position(spans, Fraction(3528, 44100), 44100), 1024)
        self.assertEqual(cc.position(spans, Fraction(3530, 44100), 44100), 1026)
        self.assertIsNone(cc.position(spans, Fraction(2000, 44100), 44100))


if __name__ == '__main__':
    unittest.main()
