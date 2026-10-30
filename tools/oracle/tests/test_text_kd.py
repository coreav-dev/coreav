"""Keeps docs/known-divergences-text.md, its JSON twin and the predicates of
scripts/text_check.py in sync, and checks that each rewrite and predicate
rejects near misses (docs/design/text.md)."""
import importlib.util
import json
import pathlib
import re
import struct
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('text_check', ROOT / 'scripts' / 'text_check.py')
tc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(tc)


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-text.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-text.md').read_text()
        documented = re.findall(r'^## (KD-T-[A-Z-]+)$', page, re.M)
        self.assertEqual(documented, [entry['id'] for entry in twin['entries']], 'same entries, same order')
        self.assertEqual({entry['predicate'] for entry in twin['entries']},
                         set(tc.CUE_REWRITES) | set(tc.PREDICATES))
        for entry in twin['entries']:
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')


def evidence(**changes):
    base = {'styled': 0, 'invalid_text': 0, 'entry': 'tx3g', 'source_cr': False, 'timescale': 1000,
            'packets_without_duration': 0, 'format_ms': None, 'edit_shift_ms': None}
    base.update(changes)
    return base


class Rewrites(unittest.TestCase):
    def reconcile(self, e, ours, theirs):
        used, a, b = tc.reconcile(tc.known_divergences(), e, ours, theirs)
        return used if a == b else None

    def test_escape_and_style(self):
        ours = [(0, 10, 'a &amp; b')]
        self.assertEqual(self.reconcile(evidence(), ours, [(0, 10, 'a & b')]), ['KD-T-ESCAPE'])
        self.assertIsNone(self.reconcile(evidence(), ours, [(0, 10, 'a &amp; c')]))
        tagged = [(0, 10, '<b>a</b> & b')]
        self.assertIsNone(self.reconcile(evidence(), ours, tagged), 'tags need styling boxes')
        self.assertEqual(self.reconcile(evidence(styled=1), ours, tagged), ['KD-T-ESCAPE', 'KD-T-STYLE'])

    def test_empty_and_invalid(self):
        ours = [(0, 10, 'a'), (20, 30, 'b�')]
        theirs = [(0, 10, 'a'), (10, 20, '')]
        self.assertIsNone(self.reconcile(evidence(), ours, theirs), 'replaced text needs the count')
        raw = evidence(invalid_text=1, exact_ms=[(0, 10), (20, 30)], invalid_samples=[('b\ufffd', (20, 30))])
        self.assertIsNone(self.reconcile(evidence(invalid_text=1), ours, theirs), 'needs the file\'s samples')
        self.assertEqual(self.reconcile(raw, ours, theirs), ['KD-T-EMPTY', 'KD-T-INVALID'])
        # A dropped cue that is not the file's invalid sample: other text,
        # other times, or one cue fewer than such samples.
        self.assertIsNone(self.reconcile(raw, [(0, 10, 'a'), (20, 30, 'c\ufffd')], theirs))
        self.assertIsNone(self.reconcile(raw, [(0, 10, 'a'), (21, 30, 'b\ufffd')], theirs))
        two = dict(raw, invalid_samples=[('b\ufffd', (20, 30)), ('d\ufffd', (40, 50))])
        self.assertIsNone(self.reconcile(two, ours, theirs))
        self.assertIsNone(self.reconcile(evidence(), [(0, 10, 'a')], [(0, 10, 'a'), (10, 20, 'x')]))

    def test_quicktime_only_rewrites(self):
        self.assertIsNone(self.reconcile(evidence(source_cr=True), [(0, 10, 'a\nb')], [(0, 10, 'ab')]))
        self.assertIsNone(self.reconcile(evidence(entry='text'), [(0, 10, 'a\nb')], [(0, 10, 'ab')]), 'no CR')
        cr = evidence(entry='text', source_cr=True)
        self.assertEqual(self.reconcile(cr, [(0, 10, 'a\nb')], [(0, 10, 'ab')]), ['KD-T-CR'])
        self.assertIsNone(self.reconcile(cr, [(0, 10, 'a\nb')], [(0, 10, 'ac')]))

    def test_leading_spaces(self):
        # ffmpeg drops a cue's leading spaces in any mov_text: the
        # cue must be, spaces and times included, one of the source's own
        # samples.
        lead = ([(0, 10, ' a')], [(0, 10, 'a')])
        self.assertIsNone(self.reconcile(evidence(), *lead), 'needs the source samples')
        self.assertIsNone(self.reconcile(evidence(sample_cues={(0, 10, 'a')}), *lead), 'a sample without the space')
        self.assertIsNone(self.reconcile(evidence(sample_cues={(10, 20, ' a')}), *lead), 'another sample\'s text')
        self.assertEqual(self.reconcile(evidence(sample_cues={(0, 10, ' a')}), *lead), ['KD-T-LEAD-SPACE'])
        self.assertEqual(self.reconcile(evidence(entry='text', sample_cues={(0, 10, ' a')}), *lead),
                         ['KD-T-LEAD-SPACE'])
        self.assertIsNone(self.reconcile(evidence(sample_cues={(0, 10, ' a')}), [(0, 10, ' b')], [(0, 10, 'b')]),
                          'another text than the sample')
        self.assertIsNone(self.reconcile(evidence(sample_cues={(0, 10, ' a')}), [(0, 10, ' a'), (10, 20, 'c')],
                                         [(0, 10, 'a')]), 'a cue fewer')
        escaped = ([(0, 10, ' a &amp; b')], [(0, 10, 'a & b')])
        self.assertEqual(self.reconcile(evidence(sample_cues={(0, 10, ' a &amp; b')}), *escaped),
                         ['KD-T-ESCAPE', 'KD-T-LEAD-SPACE'])

    def test_zero_end(self):
        ours, theirs = [(5, 10, 'a'), (10, 20, 'b')], [(5, 5, 'a'), (10, 10, 'b')]
        exact = [(5, 10), (10, 20)]
        self.assertIsNone(self.reconcile(evidence(packets_without_duration=1, exact_ms=exact), ours, theirs))
        self.assertEqual(self.reconcile(evidence(packets_without_duration=2, exact_ms=exact), ours, theirs),
                         ['KD-T-ZERO-END'])
        self.assertIsNone(self.reconcile(evidence(packets_without_duration=2, exact_ms=exact), ours,
                                         [(5, 5, 'a'), (11, 11, 'b')]))
        # CoreAV's end is the source's exact end: ffmpeg gives none.
        self.assertIsNone(self.reconcile(evidence(packets_without_duration=2, exact_ms=exact),
                                         [(5, 10, 'a'), (10, 19, 'b')], theirs))
        self.assertIsNone(self.reconcile(evidence(packets_without_duration=2), ours, theirs), 'needs exact times')

    def test_edit_align_and_last_end(self):
        ours = [(1500, 2000, 'a'), (2500, 3000, 'b')]
        theirs = [(0, 500, 'a'), (1000, 9000, 'b')]
        self.assertIsNone(self.reconcile(evidence(edit_shift_ms=1500.4), ours, theirs), 'last end alone')
        exact = [(1500, 2000), (2500, 3000)]
        self.assertIsNone(self.reconcile(evidence(edit_shift_ms=1500.4, format_ms=9000), ours, theirs),
                          'needs the last sample\'s end')
        self.assertEqual(self.reconcile(evidence(edit_shift_ms=1500.4, format_ms=9000, exact_ms=exact), ours, theirs),
                         ['KD-T-EDIT-ALIGN', 'KD-T-LAST-END'])
        # CoreAV's last end other than the last sample's end.
        self.assertIsNone(self.reconcile(evidence(edit_shift_ms=1500.4, format_ms=9000, exact_ms=exact),
                                         [(1500, 2000, 'a'), (2500, 2999, 'b')], theirs))
        self.assertIsNone(self.reconcile(evidence(edit_shift_ms=1400, format_ms=9000), ours, theirs))
        self.assertIsNone(self.reconcile(evidence(format_ms=9000), [(0, 10, 'a')], [(0, 9000, 'b')]))

    def test_rounding(self):
        ours, theirs = [(0, 1067, 'a')], [(0, 1066, 'a')]
        self.assertIsNone(self.reconcile(evidence(timescale=1000), ours, theirs))
        self.assertIsNone(self.reconcile(evidence(timescale=30), ours, theirs), 'needs the exact times')
        self.assertEqual(self.reconcile(evidence(timescale=30, exact_ms=[(0, 1067)]), ours, theirs), ['KD-T-ROUND'])
        self.assertIsNone(self.reconcile(evidence(timescale=30, exact_ms=[(0, 1067)]), ours, [(0, 1065, 'a')]))
        # CoreAV's time 1 ms off the sample's exact time.
        self.assertIsNone(self.reconcile(evidence(timescale=30, exact_ms=[(0, 1066)]), ours, theirs))

    def test_fragment_samples(self):
        def box(kind, body):
            return struct.pack('>I4s', 8 + len(body), kind) + body

        def fragment(track, tfdt, durations, default=None):
            tfhd = box(b'tfhd', struct.pack('>I', 0x08 if default else 0) + struct.pack('>I', track)
                       + (struct.pack('>I', default) if default else b''))
            base = box(b'tfdt', struct.pack('>II', 0, tfdt))
            if durations is None:
                trun = box(b'trun', struct.pack('>II', 0, 2))
            else:
                trun = box(b'trun', struct.pack('>II', 0x100, len(durations))
                           + b''.join(struct.pack('>I', d) for d in durations))
            return box(b'moof', box(b'traf', tfhd + base + trun))

        data = (box(b'ftyp', b'isom') + fragment(3, 0, [24, 500]) + fragment(1, 0, None)
                + fragment(3, 524, None, default=250) + box(b'mdat', b'\0' * 4))
        with tempfile.NamedTemporaryFile(suffix='.mp4') as f:
            f.write(data)
            f.flush()
            self.assertEqual(tc.cc.fragment_samples(f.name, 3, 100), [(0, 24), (24, 500), (524, 250), (774, 250)])
            # The `trex` default where neither the run nor `tfhd` has one.
            self.assertEqual(tc.cc.fragment_samples(f.name, 1, 100), [(0, 100), (100, 100)])
            self.assertEqual(tc.cc.fragment_samples(f.name, 2, 100), [])
            self.assertIsNone(tc.cc.fragment_samples(f.name, 3, 100, cap=3))
            # Only as many samples as asked for: a long source stays readable.
            self.assertEqual(tc.cc.fragment_samples(f.name, 3, 100, cap=3, limit=3), [(0, 24), (24, 500), (524, 250)])
        trak = {'timescale': 1000, 'elst': None, 'stts': []}
        self.assertEqual(tc.exact_cue_times(trak, 1000, fragments=[(0, 24), (24, 500), (524, 0), (524, 500)]),
                         [(0, 24), (24, 524), (524, 1024)])

    def test_exact_cue_times(self):
        # 30 Hz, an empty edit of 0.5 s, then the media from 1 tick on.
        trak = {'elst': [(500, -1, 0x10000), (0, 1, 0x10000)], 'timescale': 30, 'stts': [(3, 1), (1, 32)]}
        self.assertEqual(tc.exact_cue_times(trak, 1000), [(500, 533), (533, 567), (567, 1633)])
        trak['elst'] = [(1000, 1, 0x20000)]
        self.assertIsNone(tc.exact_cue_times(trak, 1000), 'a rate other than 1')

    def test_sample_times_follow_the_edit(self):
        trak = {'elst': None, 'timescale': 10, 'stts': [(2, 10), (1, 0)]}
        self.assertEqual(tc.sample_times(trak, 1000), [(0, 1), (1, 2), (2, 2)])
        trak['elst'] = [(1500, 0, 0x10000)]
        self.assertEqual(tc.sample_times(trak, 1000), [(0, 1), (1, 1.5)])
        trak['elst'] = [(1500, 5, 0x10000)]
        self.assertIsNone(tc.sample_times(trak, 1000))

    def test_edit_align_shift_from_the_sample_table(self):
        trak = {'elst': [(1000, 1500, 0x10000)], 'timescale': 1000, 'stts': [(3, 1000)]}
        self.assertEqual(tc.edit_shift_ms(trak), 500)
        trak['elst'] = [(1000, 2000, 0x10000)]
        self.assertEqual(tc.edit_shift_ms(trak), 0)
        trak['elst'] = [(500, -1, 0x10000), (1000, 1500, 0x10000), (1000, 0, 0x10000)]
        self.assertIsNone(tc.edit_shift_ms(trak))


class Predicates(unittest.TestCase):
    def test_edit_rate(self):
        self.assertTrue(tc.edit_rate({'refused_edit': True, 'elst': [(5, -1, 0x10000), (5, 0, 0x1ab5c)]}))
        self.assertFalse(tc.edit_rate({'refused_edit': True, 'elst': [(5, -1, 0), (5, 0, 0x10000)]}))
        self.assertFalse(tc.edit_rate({'refused_edit': False, 'elst': [(5, 0, 0x1ab5c)]}))

    def test_mediainfo_hours(self):
        cues = [(0, 1000, 'a'), (2000, 400 * 3_600_000, 'b')]
        got = {'Format': 'WebVTT', 'Events_Total': '1', 'Duration_Start': '0.000', 'Duration_End': '1.000'}
        self.assertTrue(tc.mediainfo_hours({'cues': cues, 'got': got}))
        self.assertFalse(tc.mediainfo_hours({'cues': cues[:1], 'got': got}))
        self.assertFalse(tc.mediainfo_hours({'cues': cues, 'got': dict(got, Events_Total='2')}))

    def chapters(self, **changes):
        base = {'ours': [], 'theirs': [(0, 1, 'x')], 'udta': ['chpl'], 'chapter_text_tracks': [],
                'all_chapter_text_tracks': [], 'ours_track': None, 'ours_samples': [], 'ours_times': None,
                'chapter_texts': []}
        base.update(changes)
        return base

    def test_chpl_and_hmmt(self):
        self.assertTrue(tc.nero_chapters(self.chapters()))
        self.assertFalse(tc.nero_chapters(self.chapters(udta=[])))
        self.assertFalse(tc.nero_chapters(self.chapters(chapter_text_tracks=[2])))
        self.assertFalse(tc.nero_chapters(self.chapters(ours=[(0, 1, 'x')])))
        self.assertFalse(tc.gopro_hilights(self.chapters(udta=['HMMT'])), 'titled')
        self.assertTrue(tc.gopro_hilights(self.chapters(udta=['HMMT'], theirs=[(0, 1, '')])))

    def test_chapter_order(self):
        e = self.chapters(chapter_text_tracks=[2], all_chapter_text_tracks=[2, 4], ours_track=2,
                          ours=[(0, 1, 'One')], ours_samples=['One'], ours_times=[(0, 1)],
                          chapter_texts=['One', 'Uno'], theirs=[(0, 1, 'Uno')])
        self.assertTrue(tc.other_chapter_track(e))
        # CoreAV's own chapters must be its track's samples.
        self.assertFalse(tc.other_chapter_track(dict(e, ours_samples=['Eins'])))
        self.assertFalse(tc.other_chapter_track(dict(e, ours_times=[(0, 2)])))
        self.assertTrue(tc.other_chapter_track(dict(e, ours_times=None)))
        self.assertFalse(tc.other_chapter_track(dict(e, ours_track=4)))
        self.assertFalse(tc.other_chapter_track(dict(e, all_chapter_text_tracks=[2])))
        self.assertFalse(tc.other_chapter_track(dict(e, theirs=[(0, 1, 'Eins')])))


if __name__ == '__main__':
    unittest.main()
