"""Keeps docs/known-divergences-split.md, its JSON twin and the predicates of
scripts/split_check.py in sync, checks that each predicate rejects near
misses, and that the comparisons reject a bug-shaped change
(#162): a packet held twice, missing, out of order or at the wrong time
(docs/design/split.md), and, for a source with several edits, a frame or a
window moved, dropped or shown twice (docs/design/edit-list-writers.md,
split section)."""
import copy
import importlib.util
import json
import pathlib
import re
import sys
import unittest
from fractions import Fraction
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts'))
SPEC = importlib.util.spec_from_file_location('split_check', ROOT / 'scripts' / 'split_check.py')
sc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sc)


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-split.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-split.md').read_text()
        documented = set(re.findall(r'^## (KD-S-[A-Z-]+)$', page, re.M))
        self.assertEqual(documented, {entry['id'] for entry in twin['entries']})
        self.assertEqual({entry['predicate'] for entry in twin['entries']}, set(sc.PREDICATES))
        for entry in twin['entries']:
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')


def packet(n, pts=None, flags='K__'):
    t = n * 1024 if pts is None else pts
    return {'data_hash': f'SHA256:{n:064x}', 'size': 100 + n, 'flags': flags, 'pts': t, 'dts': t}


class Coverage(unittest.TestCase):
    def setUp(self):
        self.src = [packet(n) for n in range(6)]
        # Two segments, the second starting at packet 3 (3072 ticks).
        self.outs = [
            ([packet(n) for n in range(3)], Fraction(0)),
            ([packet(n, pts=(n - 3) * 1024) for n in range(3, 6)], Fraction(3072)),
        ]

    def run_compare(self, outs, kind='video'):
        found = []
        agree, disagree = sc.compare_coverage(kind, self.src, outs, found)
        return agree, disagree, found

    def test_every_packet_once(self):
        self.assertEqual(self.run_compare(self.outs), (6, 0, []))

    def test_a_packet_in_two_segments(self):
        outs = copy.deepcopy(self.outs)
        outs[1][0].insert(0, packet(2, pts=-1024))
        agree, disagree, found = self.run_compare(outs)
        self.assertGreater(disagree, 0)
        self.assertTrue(found)

    def test_a_missing_packet(self):
        outs = copy.deepcopy(self.outs)
        del outs[1][0][1]
        agree, disagree, found = self.run_compare(outs)
        self.assertGreater(disagree, 0)
        self.assertTrue(any(field == 'video:missing' for field, _ in found))

    def test_packets_out_of_order(self):
        outs = copy.deepcopy(self.outs)
        outs[0][0][1], outs[0][0][2] = outs[0][0][2], outs[0][0][1]
        self.assertEqual(self.run_compare(outs)[1], 2)

    def test_a_wrong_time(self):
        outs = copy.deepcopy(self.outs)
        outs[1] = (outs[1][0], Fraction(3071))
        self.assertEqual(self.run_compare(outs)[1], 3)
        # Audio times are within one tick.
        self.assertEqual(self.run_compare(outs, kind='audio')[1], 0)

    def test_a_keyframe_flag(self):
        outs = copy.deepcopy(self.outs)
        outs[1][0][0]['flags'] = '___'
        self.assertEqual(self.run_compare(outs)[1], 1)


class SeveralDetection(unittest.TestCase):
    """Which splits take the several-edit comparisons: a segment that holds
    no audio (an ordinary case) must not turn them off."""

    def setUp(self):
        self.simple = (1000, 44100, [(1000, 0, 65536)])
        self.many = (1000, 44100, [(500, 0, 65536), (500, 2000, 65536)])

    def several(self, src, out, kept):
        """`sc.several_kind`, with `kept` saying which segments hold it."""
        holders = [n for n, k in enumerate(kept) if k]
        out = [e if k else None for e, k in zip(out, kept)]
        return sc.several_kind(src, out, holders)

    def test_a_segment_without_audio_keeps_the_comparison_on(self):
        self.assertTrue(self.several(self.many, [self.simple, None], [True, False]))
        self.assertTrue(self.several(self.simple, [self.many, None], [True, False]))

    def test_unreadable_boxes_turn_it_off(self):
        self.assertFalse(self.several(self.many, [None, self.simple], [True, True]))

    def test_one_edit_everywhere_stays_off(self):
        self.assertFalse(self.several(self.simple, [self.simple, None], [True, False]))


class CoveringPredicates(unittest.TestCase):
    """A known divergence that cannot read the evidence a comparison
    collects does not cover its difference, and does not raise out of the
    check (scripts/split_check.py, `_covers`)."""

    def test_a_predicate_that_cannot_read_the_evidence_does_not_cover(self):
        def raises(field, fields, c):
            return c['missing']['key']
        self.assertFalse(sc._covers(raises, 'audio:pcm', set(), {}))
        self.assertTrue(sc._covers(lambda *a: True, 'audio:pcm', set(), {}))
        self.assertFalse(sc._covers(lambda *a: False, 'audio:pcm', set(), {}))


class Predicates(unittest.TestCase):
    def evidence(self, elst):
        audio = {'elst': elst, 'mdhd': (48000, 48128), 'stts_sum': 48128, 'tkhd': 0}
        video = {'elst': [[20480, 1024]], 'mdhd': (12800, 20480), 'stts_sum': 20480, 'tkhd': 0}
        return {
            'output_headers': {'brand': 'isom', 'mvhd': (192000, 0), 'traks': [video, audio]},
            'output_streams': [{'duration_ts': 20480}, {'duration_ts': 48128, 'sample_rate': '48000'}],
            'entry': {'source': '/nonexistent', 'audio': {'stream_index': 1, 'first_sample': 47,
                                                          'samples': 47}},
            'ffmpeg': 'ffmpeg',
            'audio_output_pcm': ([(Fraction(0), 1024)], 1, '/nonexistent.f32'),
        }

    def test_audio_restart_needs_audio_and_at_most_one_media_edit(self):
        c = self.evidence(None)
        self.assertFalse(sc.audio_restart('video:framemd5', set(), c))
        no_audio = copy.deepcopy(c)
        no_audio['entry']['audio'] = None
        self.assertFalse(sc.audio_restart('audio:pcm', set(), no_audio))
        no_output = copy.deepcopy(c)
        no_output['audio_output_pcm'] = None
        self.assertFalse(sc.audio_restart('audio:pcm', set(), no_output))
        two = self.evidence([[100, 0], [100, 2048]])
        self.assertFalse(sc.audio_restart('audio:pcm', set(), two))
        # The source cannot be read: no decoded packets to compare with.
        self.assertFalse(sc.audio_restart('audio:pcm', set(), c))

    def test_dts_shift_is_the_offsets_difference(self):
        c = {'video_timing': {0: {'pts': {0}, 'dts': {Fraction(-200)}, 'count': 3},
                              1: {'pts': {0}, 'dts': {Fraction(-100)}, 'count': 3}},
             'video_ctts_negative': {0: True, 1: True}, 'video_ctts_min': {0: -200, 1: -300},
             'source_headers': {'traks': [{'ctts_min': -400}]}, 'video_stream': 0}
        self.assertTrue(sc.dts_shift('video:packets', set(), c))
        self.assertFalse(sc.dts_shift('video:framemd5', set(), c))
        # A constant other than the source's offset less the segment's (#225).
        other = copy.deepcopy(c)
        other['video_ctts_min'][1] = -250
        self.assertFalse(sc.dts_shift('video:packets', set(), other))
        unread = copy.deepcopy(c)
        unread['source_headers']['traks'][0]['ctts_min'] = None
        self.assertFalse(sc.dts_shift('video:packets', set(), unread))
        presented = copy.deepcopy(c)
        presented['video_timing'][0]['pts'] = {Fraction(1)}
        self.assertFalse(sc.dts_shift('video:packets', set(), presented))

    def test_dts_shift_with_a_midframe_edit(self):
        # #576: offsets down to -335 in the source, -310 in the second
        # segment, and an edit at media time 169 with frames of 10 ticks:
        # the second segment's packets are 1 tick off in presentation
        # (KD-S-EDIT-MIDFRAME) and 1 + 25 in decode.
        c = {'video_timing': {1: {'pts': {Fraction(-1)}, 'dts': {Fraction(-26)}, 'count': 113}},
             'video_ctts_negative': {0: True, 1: True}, 'video_ctts_min': {0: -335, 1: -310},
             'source_headers': {'traks': [{'ctts_min': -335, 'elst': [[7830, 169]]}]}, 'video_stream': 0,
             'video_source_packets': [{'duration': 10}] * 5}
        self.assertTrue(sc.dts_shift('video:packets', set(), c))
        # Neither entry covers it alone.
        self.assertFalse(sc.edit_midframe('video:packets', set(), c))
        # The decode times must move by the presentation constant plus the
        # offsets' difference, not by either alone.
        for dts in ({Fraction(-25)}, {Fraction(-1)}, {Fraction(-27)}):
            other = copy.deepcopy(c)
            other['video_timing'][1]['dts'] = dts
            self.assertFalse(sc.dts_shift('video:packets', set(), other))
        # A presentation constant needs an edit inside a frame, smaller
        # than a frame, and the first segment's times equal.
        on_frame = copy.deepcopy(c)
        on_frame['source_headers']['traks'][0]['elst'] = [[7830, 170]]
        self.assertFalse(sc.dts_shift('video:packets', set(), on_frame))
        large = copy.deepcopy(c)
        large['video_timing'][1] = {'pts': {Fraction(-10)}, 'dts': {Fraction(-35)}, 'count': 113}
        self.assertFalse(sc.dts_shift('video:packets', set(), large))
        first = copy.deepcopy(c)
        first['video_timing'][0] = {'pts': {Fraction(-1)}, 'dts': {Fraction(-1)}, 'count': 335}
        self.assertFalse(sc.dts_shift('video:packets', set(), first))
        # Two presentation constants are not one.
        two = copy.deepcopy(c)
        two['video_ctts_min'][2] = -310
        two['video_timing'][2] = {'pts': {Fraction(-2)}, 'dts': {Fraction(-27)}, 'count': 3}
        self.assertFalse(sc.dts_shift('video:packets', set(), two))
        # A segment that only moves with the edit is fine beside one that
        # also shifts its decode times; one that moves only with the edit
        # is KD-S-EDIT-MIDFRAME's alone.
        both = copy.deepcopy(c)
        both['video_ctts_min'][2] = -335
        both['video_timing'][2] = {'pts': {Fraction(-1)}, 'dts': {Fraction(-1)}, 'count': 3}
        self.assertTrue(sc.dts_shift('video:packets', set(), both))
        # Every segment after the first moves with the edit.
        missing = copy.deepcopy(c)
        missing['video_ctts_min'][2] = -310
        self.assertFalse(sc.dts_shift('video:packets', set(), missing))
        alone = copy.deepcopy(c)
        alone['video_ctts_min'][1] = -335
        alone['video_timing'][1]['dts'] = {Fraction(-1)}
        self.assertFalse(sc.dts_shift('video:packets', set(), alone))

    def test_audio_restart_needs_the_sources_stored_durations(self):
        c = self.evidence([[100, 0]])
        c['output_headers']['traks'][1]['stts'] = [(46, 1024), (1, 1024)]
        c['source_headers'] = {'traks': [{}, {'stts': [(200, 1024)]}]}
        with mock.patch.object(sc.cc, 'kept_audio_decoded', return_value='/nonexistent.f32'), \
                mock.patch.object(sc.cc, '_restart_matches', return_value=True):
            self.assertTrue(sc.audio_restart('audio:pcm', set(), c))
            # Decode durations one tick short: the decode would still agree
            # with itself (#225).
            short = copy.deepcopy(c)
            short['output_headers']['traks'][1]['stts'] = [(47, 1023)]
            self.assertFalse(sc.audio_restart('audio:pcm', set(), short))
            fewer = copy.deepcopy(c)
            fewer['output_headers']['traks'][1]['stts'] = [(46, 1024)]
            self.assertFalse(sc.audio_restart('audio:pcm', set(), fewer))
            uneven = copy.deepcopy(c)
            uneven['source_headers']['traks'][1]['stts'] = [(50, 1024), (150, 960)]
            self.assertFalse(sc.audio_restart('audio:pcm', set(), uneven))
            unread = copy.deepcopy(c)
            del unread['source_headers']
            self.assertFalse(sc.audio_restart('audio:pcm', set(), unread))

    def test_sync_verified_needs_fewer_flags_in_every_segment_and_ffmpegs_key_frames(self):
        # Six source packets, all flagged; two segments of three that flag
        # their first packet only, as ffmpeg decodes them.
        src = [packet(n) for n in range(6)]

        def segment(first):
            return [dict(packet(n, pts=(n - first) * 1024), pos=str(n), flags='K__' if n == first else '___')
                    for n in range(first, first + 3)]
        c = {'ffprobe': 'ffprobe', 'video_source_packets': src,
             'video_segments': [(('a.mp4', 0), (segment(0), Fraction(0))),
                                (('b.mp4', 0), (segment(3), Fraction(3072)))]}
        keys = {'0', '3'}
        original = sc.cc.ffprobe_json
        sc.cc.ffprobe_json = lambda *args: {'frames': [{'key_frame': int(str(n) in keys), 'pkt_pos': str(n)}
                                                       for n in range(6)]}
        try:
            self.assertTrue(sc.sync_verified('video:packets', {'video:packets'}, c))
            self.assertFalse(sc.sync_verified('video:framemd5', {'video:packets'}, c))
            # A segment that does not start with a keyframe.
            late = copy.deepcopy(c)
            late['video_segments'][1][1][0][0]['flags'] = '___'
            self.assertFalse(sc.sync_verified('video:packets', {'video:packets'}, late))
            # A packet at the wrong time, or a source packet in no segment.
            moved = copy.deepcopy(c)
            moved['video_segments'][1][1][0][1]['pts'] += 1
            self.assertFalse(sc.sync_verified('video:packets', {'video:packets'}, moved))
            missing = copy.deepcopy(c)
            missing['video_segments'][1][1][0].pop()
            self.assertFalse(sc.sync_verified('video:packets', {'video:packets'}, missing))
            # No flag left out.
            same = copy.deepcopy(c)
            for _, (packets, _) in same['video_segments']:
                for p in packets:
                    p['flags'] = 'K__'
            keys = {str(n) for n in range(6)}
            self.assertFalse(sc.sync_verified('video:packets', {'video:packets'}, same))
            # ffmpeg decodes another frame as a key frame.
            keys = {'0', '3', '4'}
            self.assertFalse(sc.sync_verified('video:packets', {'video:packets'}, c))
        finally:
            sc.cc.ffprobe_json = original


class SeveralEdits(unittest.TestCase):
    """The several-edit comparisons of the split section
    (docs/design/edit-list-writers.md). Three windows of 1,000 media ticks,
    the second after a gap of 500; frames of 100 ticks; the split cuts at
    each window's start, so the three segments are the three windows and
    the second opens with the gap."""

    def setUp(self):
        self.src_windows = [(0, 1000, 0), (2000, 3000, 1500), (4000, 5000, 2500)]
        # (media time, duration, content) of the source's stored frames.
        self.src = (Fraction(1), [(n * 100, 100, f'{n:02x}', 100) for n in range(50)])
        # Each segment's stored frames start at 0 and its window maps them
        # from there; the starts are its place on the source's timeline.
        self.outs = []
        for (m0, m1, _at), start in zip(self.src_windows, [0, 1000, 2500]):
            frames = [(t - m0, 100, f'{t // 100:02x}', 100) for t in range(m0, m1, 100)]
            self.outs.append(((Fraction(1), frames), [(0, m1 - m0, 0)], Fraction(start)))
        # The second segment opens with the gap the first window left.
        self.outs[1] = (self.outs[1][0], [(0, 1000, 500)], Fraction(1000))

    def frames(self, outs=None):
        found = []
        sc.check_frames_several(self.src, self.src_windows, outs or self.outs, Fraction(3500), found)
        return found

    def test_every_presented_frame_once(self):
        self.assertEqual(self.frames(), [])

    def test_a_frame_in_two_segments(self):
        outs = copy.deepcopy(self.outs)
        # The second segment holds and presents the third's first frame as
        # well, at the same place on the source's timeline.
        base, frames = outs[1][0]
        frames.append((1000, 100, '28', 100))
        outs[1] = ((base, frames), [(0, 1100, 0)], outs[1][2])
        self.assertTrue(self.frames(outs))

    def test_a_window_dropped(self):
        outs = copy.deepcopy(self.outs)
        del outs[1]
        self.assertTrue(self.frames(outs))

    def test_a_segment_at_the_wrong_time(self):
        outs = copy.deepcopy(self.outs)
        outs[2] = (outs[2][0], outs[2][1], Fraction(2400))
        self.assertTrue(self.frames(outs))

    def test_a_gap_presented_as_media(self):
        # The second segment presents its window from its own start, so the
        # gap before it shows media the source does not.
        outs = copy.deepcopy(self.outs)
        outs[1] = (outs[1][0], [(0, 1000, 0)], Fraction(1000))
        self.assertTrue(self.frames(outs))

    def test_the_last_segment_ending_early_is_reported(self):
        # The segments stop at 2500 ticks, dropping the third window; the
        # end comes from CoreAV's own summary, so the comparison must not
        # take it for the source's end.
        found = []
        sc.check_frames_several(self.src, self.src_windows, self.outs[:2], Fraction(2500), found)
        self.assertTrue(any('after the last' in m for _, m in found))

    def test_a_frame_two_windows_show_may_be_shown_once_or_twice(self):
        # A source window that starts inside a frame an earlier window
        # already showed: only the segment that holds the frame can show
        # it, so the segments may show the repeat or not, and nothing else
        # (docs/design/edit-list-writers.md, split decision 5).
        windows = [(0, 150, 0), (100, 300, 500)]
        frames = [(Fraction(n * 100), Fraction(100), f'{n:02x}') for n in range(3)]
        full = sc.cc.iso_presented(frames, windows)
        once = sc.iso_presented_once(frames, windows)
        self.assertEqual([c for _, c in full].count('01'), 2)
        self.assertEqual([c for _, c in once].count('01'), 1)
        # The first showing is the earlier one.
        self.assertEqual(min(t for t, c in full if c == '01'), min(t for t, c in once if c == '01'))

    def test_a_frame_with_other_content(self):
        outs = copy.deepcopy(self.outs)
        base, frames = outs[2][0]
        frames[3] = (frames[3][0], frames[3][1], 'ff', frames[3][3])
        self.assertTrue(self.frames(outs))

    def test_the_second_view_samples_the_first_last_and_several_edit_segments(self):
        simple = (1000, 600, [(600, 0, 65536)])
        many = (1000, 600, [(300, 0, 65536), (300, 1200, 65536)])
        edits = {'video': (None, [simple, many, simple, simple, many, simple])}
        self.assertEqual(sc.second_view_of(6, edits), {0, 1, 4, 5})
        # Capped, the first in segment order kept.
        edits = {'video': (None, [many] * 20)}
        self.assertEqual(sc.second_view_of(20, edits), set(range(8)))
        self.assertEqual(sc.second_view_of(1, {'video': (None, [simple])}), {0})

    def test_a_segment_longer_than_its_video_by_more_than_a_frame(self):
        # A segment of 1 s of video whose movie lasts 2 s: a gap counted
        # both as audio here and as the next segment's empty edit.
        oh = {'mvhd': (1000, 2000), 'traks': [{'mdhd': (600, 600)}, {'mdhd': (48000, 48000)}]}
        kept = [('video', 0, 0), ('audio', 1, 1)]
        segment = {'audio_samples': 47}
        over, frame = sc.segment_overhang(oh, kept, segment, 1000, Fraction(0), Fraction(1))
        self.assertEqual(over, Fraction(1))
        self.assertGreater(over, frame)
        # One audio frame of overhang is allowed.
        oh = {'mvhd': (1000, 1021), 'traks': [{'mdhd': (600, 600)}, {'mdhd': (48000, 48128)}]}
        over, frame = sc.segment_overhang(oh, kept, segment, 1000, Fraction(0), Fraction(1))
        self.assertLessEqual(over, frame)
        # Without audio nothing is allowed past the video.
        oh = {'mvhd': (1000, 1001), 'traks': [{'mdhd': (600, 600)}]}
        over, frame = sc.segment_overhang(oh, [('video', 0, 0)], {}, 1000, Fraction(0), Fraction(1))
        self.assertGreater(over, frame)

    def test_edit_total_separates_the_leading_empty_from_the_span(self):
        entries = [(500, -1, 65536), (1000, 0, 65536), (500, -1, 65536), (1000, 2000, 65536)]
        self.assertEqual(sc.edit_total(entries, 1000), (Fraction(1, 2), Fraction(5, 2)))
        self.assertEqual(sc.edit_total([], 1000), (Fraction(0), Fraction(0)))
        # Every entry counts: cc.headers keeps only the first eight.
        many = [(100, 100 * n, 65536) for n in range(12)]
        self.assertEqual(sc.edit_total(many, 1000), (Fraction(0), Fraction(12, 10)))


class SeveralEditsAudio(unittest.TestCase):
    """The audio comparison of the split section: the windows the segments
    present, against the source's. The decoded comparison is left out here
    (it reads files); a near miss in the windows must be reported."""

    def setUp(self):
        self.src_windows = [(0, 1000, 0), (2000, 3000, 1000)]
        # Two segments, the second holding the source's second window.
        self.outs = [(0, [(0, 1000, 0)], Fraction(0), 0),
                     (1, [(0, 1000, 0)], Fraction(1000), 2000)]

    def run_check(self, outs=None):
        found = []
        sc.check_audio_several(self.src_windows, outs or self.outs, found)
        return [field for field, _ in found]

    def test_the_windows_agree(self):
        self.assertEqual(self.run_check(), [])

    def test_an_edit_on_other_media(self):
        outs = copy.deepcopy(self.outs)
        outs[1] = (1, [(0, 1000, 0)], Fraction(1000), 2500)
        self.assertIn('audio:edits', self.run_check(outs))

    def test_an_edit_at_the_wrong_time(self):
        outs = copy.deepcopy(self.outs)
        outs[1] = (1, [(0, 1000, 0)], Fraction(1100), 2000)
        self.assertIn('audio:edits', self.run_check(outs))

    def test_a_segment_whose_audio_did_not_decode_is_reported(self):
        # The decoded comparison is per segment now, in check_segment.
        self.assertEqual(sc.decoded_run(None, None, 0, 1000), "audio did not decode")
        one = ([(Fraction(0), 1024)], 1, '/nonexistent')
        self.assertEqual(sc.decoded_run(one, ([(Fraction(0), 1024)], 2, '/nonexistent'), 0, 1000),
                         "1 source and 2 output channels")
        # A single decoded frame has no second frame to compare.
        self.assertIsNone(sc.decoded_run(one, one, 0, 1000))
        # The source has no frame at the segment's second frame's time.
        two = ([(Fraction(0), 1024), (Fraction(1, 2), 1024)], 1, '/nonexistent')
        self.assertEqual(sc.decoded_run(one, two, 0, 1000),
                         "the decoded audio differs from the source's run")
    def test_audio_presented_by_two_segments(self):
        outs = copy.deepcopy(self.outs)
        # The first segment reaches into the window the second presents.
        outs[0] = (0, [(0, 1500, 0)], Fraction(0), 0)
        self.assertIn('audio:edits', self.run_check(outs))

    def test_what_a_split_may_miss_is_bounded_per_span(self):
        # A window that starts inside a sample the window before showed,
        # across a break in the presentation: only the segment that holds
        # that sample can show that part, so the split may miss it and no
        # more (split decision 5). The sample at [1024, 2048) reaches 548
        # ticks into the second window.
        samples = [(n * 1024, 1024) for n in range(4)]
        self.assertEqual(sc.audio_coverage([(0, 1500, 0), (1500, 3000, 1800)], samples, None),
                         [(0, 1500, 0), (1800, 3300, 548)])
        # A boundary that runs on in both media and presentation shows
        # the sample once, without a break: nothing may be missed, and
        # the two windows are one span.
        self.assertEqual(sc.audio_coverage([(0, 1500, 0), (1500, 3000, 1500)], samples, None),
                         [(0, 3000, 0)])
        # Nor may anything where no sample straddles the boundary.
        self.assertEqual(sc.audio_coverage([(0, 1500, 0), (1500, 3000, 1800)],
                                           [(n * 500, 500) for n in range(8)], None),
                         [(0, 1500, 0), (1800, 3300, 0)])
        # Nor where the media is not contiguous past the sample.
        self.assertEqual(sc.audio_coverage([(0, 1500, 0), (4000, 5500, 1800)], samples, None),
                         [(0, 1500, 0), (1800, 3300, 0)])
        # A window wholly inside the straddling sample: all of it.
        self.assertEqual(sc.audio_coverage([(0, 1500, 0), (1500, 1600, 4000)], samples, None),
                         [(0, 1500, 0), (4000, 4100, 100)])
        # Two spans that meet become one, and their allowances add.
        self.assertEqual(sc.audio_coverage([(0, 1500, 0), (1500, 3200, 1800), (3300, 4096, 3500)],
                                           samples, None),
                         [(0, 1500, 0), (1800, 4296, 548 + 796)])
        # The first window's start is not a break, and media the samples
        # do not reach is not expected.
        self.assertEqual(sc.audio_coverage([(500, 1500, 0)], samples, None), [(0, 1000, 0)])
        self.assertEqual(sc.audio_coverage([(0, 4096, 0)], samples, 3000), [(0, 3000, 0)])
        # A window that starts exactly on a sample boundary straddles no
        # sample, so a break there allows nothing either.
        self.assertEqual(sc.audio_coverage([(0, 1024, 0), (1024, 3000, 1800)], samples, None),
                         [(0, 1024, 0), (1800, 3776, 0)])

    def test_the_coverage_bound_catches_what_is_missed_beyond_it(self):
        samples = [(n * 1024, 1024) for n in range(4)]

        def check(windows, shown):
            found = []
            sc.check_audio_several(windows, [(0, shown, Fraction(0), 0)], found, samples=samples)
            return [field for field, _ in found]
        windows = [(0, 1500, 0), (1500, 3000, 1800)]
        # Whole, and the straddling sample's part missed wherever it
        # falls: at the window's start, or as a hole inside it.
        self.assertNotIn('audio:edits', check(windows, [(0, 1500, 0), (1500, 3000, 1800)]))
        self.assertNotIn('audio:edits', check(windows, [(0, 1500, 0), (1548, 3000, 1848)]))
        self.assertNotIn('audio:edits', check(windows, [(0, 1500, 0), (1500, 1548, 1800),
                                                        (2048, 3000, 2348)]))
        # More than that missed (600 ticks against the 548 allowed), and
        # a window missed whole.
        self.assertIn('audio:edits', check(windows, [(0, 1500, 0), (2100, 3000, 2400)]))
        self.assertIn('audio:edits', check(windows, [(0, 1500, 0)]))
        # Audio where the source's edits show none: the segments present
        # a span outside every one of the source's.
        def outside(windows, shown):
            found = []
            sc.check_audio_several(windows, [(0, shown, Fraction(0), 0)], found, samples=samples)
            return [m for _, m in found if 'which the source' in m]
        self.assertTrue(outside([(0, 1500, 0)], [(0, 1500, 0), (1500, 3000, 4000)]))
        # Past the end of a span, and before its start.
        self.assertTrue(outside([(0, 1500, 0)], [(0, 2000, 0)]))
        self.assertTrue(outside([(1500, 3000, 1800)], [(1500, 3000, 1700)]))
        self.assertFalse(outside([(0, 1500, 0)], [(0, 1500, 0)]))
        # One span's allowance does not pay for a loss in another: the
        # second span allows 548 ticks and loses none, the first allows
        # none and loses 300, which is reported although the two
        # together are under 548.
        self.assertIn('audio:edits', check(windows, [(0, 1200, 0), (1500, 3000, 1800)]))
        # The whole of the first span lost, which no allowance covers.
        self.assertIn('audio:edits', check(windows, [(1800, 3300, 1800)]))

