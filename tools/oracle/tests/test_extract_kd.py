"""Keeps docs/known-divergences-extract.md, its JSON twin and the predicates of
scripts/extract_check.py in sync, and checks that each predicate rejects near
misses."""
import copy
import importlib.util
import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('extract_check', ROOT / 'scripts' / 'extract_check.py')
ec = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ec)


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-extract.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-extract.md').read_text()
        documented = set(re.findall(r'^## (KD-X-[A-Z-]+)$', page, re.M))
        self.assertEqual(documented, {entry['id'] for entry in twin['entries']})
        self.assertEqual({entry['predicate'] for entry in twin['entries']}, set(ec.PREDICATES))
        for entry in twin['entries']:
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')


def trak(tkhd, elst=None, mdhd=(48000, 480000)):
    return {'tkhd': tkhd, 'elst': elst, 'mdhd': mdhd, 'handler': 'soun'}


def evidence(**changes):
    base = {
        'fields': set(),
        'source_stream': {'codec_name': 'aac', 'profile': 'LC', 'nb_frames': '470', 'sample_rate': '48000',
                          'time_base': '1/48000', 'start_pts': 0, 'start_time': '0.000000',
                          'duration_ts': 480000, 'duration': '10.000000'},
        'output_stream': {'codec_name': 'aac', 'profile': 'LC', 'nb_frames': '470', 'sample_rate': '48000',
                          'time_base': '1/48000', 'start_pts': 0, 'start_time': '0.000000',
                          'duration_ts': 480000, 'duration': '10.000000'},
        'source_packets': [{'data_hash': 'h0', 'size': 9, 'pts': 0, 'dts': 0}],
        'output_packets': [{'data_hash': 'h0', 'size': 9, 'pts': 0, 'dts': 0}],
        'source_frames': [], 'output_frames': [],
        'mi_source': {'Duration': '10.022'}, 'mi_output': {'Duration': '10.000'},
        'source_headers': {'mvhd': (1000, 10022), 'traks': [trak(10022, [[10000, 1024]])]},
        'output_headers': {'mvhd': (1000, 10000), 'traks': [trak(10000, [[10000, 1024]])]},
        'source_trak': trak(10022, [[10000, 1024]]),
        'smpb': None,
        'several': False,
        'source_elst': (1000, 48000, [(10000, 1024, 0x10000)]),
        'output_elst': (1000, 48000, [(10000, 1024, 0x10000)]),
        'first_dts': None,
    }
    for key, value in changes.items():
        base[key] = value
    return base


def frames(start, n, delay=0):
    return [f'0, {start + i * 1024 - delay}, {start + i * 1024 - delay}, 1024, 4096, hash{i}' for i in range(n)]


class Predicates(unittest.TestCase):
    def test_tkhd_needs_the_sum_of_the_edits_and_each_files_own_value(self):
        c = evidence()
        self.assertTrue(ec.tkhd_follows_edits('mediainfo:Duration', c))
        self.assertTrue(ec.tkhd_follows_edits('mediainfo:FrameCount', c))
        self.assertFalse(ec.tkhd_follows_edits('mediainfo:Channels', c))
        wrong = copy.deepcopy(c)
        wrong['output_headers']['traks'][0]['tkhd'] = 10001  # not the edit sum
        self.assertFalse(ec.tkhd_follows_edits('mediainfo:Duration', wrong))
        same = evidence(source_trak=trak(10000, [[10000, 1024]]))
        self.assertFalse(ec.tkhd_follows_edits('mediainfo:Duration', same))
        self.assertFalse(ec.tkhd_follows_edits('mediainfo:Duration', evidence(mi_output={'Duration': '10.010'})))
        # Without an edit list: the media duration rounded to movie ticks.
        plain = evidence(output_headers={'mvhd': (600, 0), 'traks': [trak(438042, None, (24000, 17521664))]},
                         source_trak=trak(438041), source_headers={'mvhd': (600, 0), 'traks': [trak(438041)]},
                         mi_source={'Duration': '730.068'}, mi_output={'Duration': '730.070'})
        self.assertTrue(ec.tkhd_follows_edits('mediainfo:Duration', plain))
        # A fragmented source declares no track duration: MediaInfo gives
        # its media duration, which the output's header rounds.
        fragmented = evidence(output_headers={'mvhd': (600, 0), 'traks': [trak(35994, None, (24000, 1439744))]},
                              source_trak=trak(0), source_headers={'mvhd': (600, 0), 'traks': [trak(0)]},
                              mi_source={'Duration': '59.989'}, mi_output={'Duration': '59.990'})
        self.assertTrue(ec.tkhd_follows_edits('mediainfo:Duration', fragmented))
        self.assertFalse(ec.tkhd_follows_edits('mediainfo:Duration', dict(fragmented, mi_source={'Duration': '59.980'})))
        # A fragmented source whose header holds an unrelated duration.
        stale = dict(fragmented, source_trak=trak(90000))
        self.assertTrue(ec.tkhd_follows_edits('mediainfo:Duration', stale))
        self.assertFalse(ec.tkhd_follows_edits('mediainfo:Duration', dict(stale, mi_source={'Duration': '149.000'})))

    def test_edit_discard_needs_equal_packet_lists(self):
        c = evidence(source_stream={'nb_frames': '2'}, output_stream={'nb_frames': '2'})
        self.assertTrue(ec.edit_discard('nb_frames', c))
        self.assertFalse(ec.edit_discard('nb_frames', dict(c, fields={'packets'})))
        self.assertFalse(ec.edit_discard('nb_frames', dict(c, output_stream={'nb_frames': '3'})))
        self.assertFalse(ec.edit_discard('nb_frames', dict(c, output_packets=[])))
        self.assertFalse(ec.edit_discard('nb_frames', evidence(source_stream={'nb_frames': '1'},
                                                               output_stream={'nb_frames': '1'})))

    def gapless(self, delay=2112, padding=288, samples=477600, out_delay=None):
        out_delay = delay if out_delay is None else out_delay
        total = delay + padding + samples
        src = {'sample_rate': '48000', 'time_base': '1/48000', 'start_pts': delay,
               'start_time': f'{delay / 48000:.6f}', 'duration_ts': total, 'duration': f'{total / 48000:.6f}'}
        out = {'sample_rate': '48000', 'time_base': '1/48000', 'start_pts': 0, 'start_time': '0.000000',
               'duration_ts': samples, 'duration': f'{samples / 48000:.6f}'}
        return evidence(
            smpb=(delay, padding, samples), source_stream=src, output_stream=out,
            source_trak=trak(total, None, (48000, total)),
            output_headers={'mvhd': (48000, samples), 'traks': [trak(samples, [[samples, out_delay]])]},
            source_packets=[{'data_hash': 'a', 'size': 5, 'pts': 0, 'dts': 0},
                            {'data_hash': 'b', 'size': 6, 'pts': 1024, 'dts': 1024}],
            output_packets=[{'data_hash': 'a', 'size': 5, 'pts': -delay, 'dts': -delay},
                            {'data_hash': 'b', 'size': 6, 'pts': 1024 - delay, 'dts': 1024 - delay}],
            source_frames=frames(delay, 3), output_frames=frames(delay, 3, delay))

    def test_gapless_edit_needs_the_exact_delay_everywhere(self):
        c = self.gapless()
        for field in ('stream:start_pts', 'stream:start_time', 'stream:duration_ts', 'stream:duration',
                      'packets', 'framemd5'):
            self.assertTrue(ec.gapless_edit(field, c), field)
        self.assertFalse(ec.gapless_edit('stream:profile', c))
        # The output edit starts elsewhere, or the tag does not add up.
        self.assertFalse(ec.gapless_edit('stream:start_pts', self.gapless(out_delay=1024)))
        bad = self.gapless()
        bad['source_trak']['mdhd'] = (48000, 1)
        self.assertFalse(ec.gapless_edit('stream:start_pts', bad))
        edited = self.gapless()
        edited['source_trak']['elst'] = [[1, 0]]
        self.assertFalse(ec.gapless_edit('stream:start_pts', edited))
        # One payload differs, or one time is off by one tick.
        off = self.gapless()
        off['output_packets'][1]['data_hash'] = 'c'
        self.assertFalse(ec.gapless_edit('packets', off))
        late = self.gapless()
        late['output_frames'] = frames(2112, 3, 2111)
        self.assertFalse(ec.gapless_edit('framemd5', late))

    def test_gapless_conflict_needs_the_source_frames_inside_the_output(self):
        c = evidence(smpb=(2112, 988, 5082084), source_trak=trak(30024, [[30001, 1012]], (44100, 1324032)),
                     output_headers={'mvhd': (1000, 30001), 'traks': [trak(30001, [[30001, 1012]])]},
                     source_frames=frames(0, 6)[1:5], output_frames=frames(0, 6))
        self.assertTrue(ec.gapless_conflict('framemd5', c))
        self.assertFalse(ec.gapless_conflict('packets', c))
        changed = dict(c, output_frames=frames(0, 3) + ['0, 3072, 3072, 1024, 4096, other'] + frames(0, 6)[4:])
        self.assertFalse(ec.gapless_conflict('framemd5', changed))
        adds_up = copy.deepcopy(c)
        adds_up['smpb'] = (2112, 0, 1324032 - 2112)
        self.assertFalse(ec.gapless_conflict('framemd5', adds_up))
        self.assertFalse(ec.gapless_conflict('framemd5', dict(c, source_trak=trak(30024, None, (44100, 1324032)))))

    def test_profile_unnamed_needs_lc_on_the_output(self):
        c = evidence(source_stream={'codec_name': 'aac', 'profile': None},
                     output_stream={'codec_name': 'aac', 'profile': 'LC'})
        self.assertTrue(ec.profile_unnamed('stream:profile', c))
        self.assertFalse(ec.profile_unnamed('stream:profile', dict(c, output_stream={'codec_name': 'aac',
                                                                                     'profile': 'HE-AAC'})))
        self.assertFalse(ec.profile_unnamed('stream:profile', dict(c, source_stream={'codec_name': 'aac',
                                                                                     'profile': 'Main'})))


MULTI = [(1000, 0, 0x10000), (500, -1, 0x10000), (1000, 96000, 0x10000)]


class EditMulti(unittest.TestCase):
    """KD-X-EDIT-MULTI: `ffprobe` lists more packets than the track has
    samples for a list of several media edits. The entry must hold only
    when the output carries the source's entries."""

    def several(self, **changes):
        base = {
            'fields': {'nb_frames'},
            'several': True,
            'first_dts': 0,
            'source_elst': (1000, 48000, MULTI),
            'output_elst': (1000, 48000, MULTI),
            'source_stream': {'nb_frames': '470'},
            'output_stream': {'nb_frames': '470'},
            'source_packets': [{}] * 499,
            'output_packets': [{}] * 499,
        }
        base.update(changes)
        return evidence(**base)

    def test_covers_the_listing_of_a_list_of_several_media_edits(self):
        self.assertTrue(ec.edit_multi('nb_frames', self.several()))

    def test_rejects_an_output_whose_entries_are_not_the_source_s(self):
        for entries in (
            [(1000, 0, 0x10000), (500, -1, 0x10000), (1000, 96001, 0x10000)],   # a media time moved
            [(1000, 0, 0x10000), (500, -1, 0x10000), (1001, 96000, 0x10000)],   # a duration changed
            [(1000, 0, 0x10000), (500, -1, 0x10000), (1000, 96000, 0x20000)],   # a rate changed
            [(1000, 0, 0x10000), (1000, 96000, 0x10000)],                       # an entry left out
            MULTI + [(10, 0, 0x10000)],                                         # an entry added
        ):
            self.assertFalse(ec.edit_multi('nb_frames', self.several(output_elst=(1000, 48000, entries))),
                             entries)

    def test_rejects_output_timescales_that_are_not_the_source_s(self):
        self.assertFalse(ec.edit_multi('nb_frames', self.several(output_elst=(600, 48000, MULTI))))
        self.assertFalse(ec.edit_multi('nb_frames', self.several(output_elst=(1000, 44100, MULTI))))

    def test_rejects_a_media_time_that_did_not_move_with_the_decode_times(self):
        # The source starts at decode time 1,024, so every media time must
        # be 1,024 lower in the output.
        moved = [(d, m if m == -1 else m - 1024, r) for d, m, r in MULTI]
        self.assertTrue(ec.edit_multi('nb_frames', self.several(first_dts=1024,
                                                                output_elst=(1000, 48000, moved))))
        self.assertFalse(ec.edit_multi('nb_frames', self.several(first_dts=1024)))

    def test_rejects_a_frame_count_that_differs(self):
        self.assertFalse(ec.edit_multi('nb_frames', self.several(output_stream={'nb_frames': '469'})))

    def test_rejects_a_listing_no_longer_than_the_frame_count(self):
        self.assertFalse(ec.edit_multi('nb_frames',
                                       self.several(source_packets=[{}] * 470, output_packets=[{}] * 470)))

    def test_rejects_a_simple_list_and_a_differing_packet_list(self):
        self.assertFalse(ec.edit_multi('nb_frames', self.several(several=False)))
        self.assertFalse(ec.edit_multi('nb_frames', self.several(fields={'nb_frames', 'packets'})))
        self.assertFalse(ec.edit_multi('nb_frames', self.several(output_packets=[{}] * 498)))

    def test_rejects_another_field(self):
        self.assertTrue(ec.edit_multi('nb_frames', self.several()))
        self.assertFalse(ec.edit_multi('stream:duration', self.several()))


class EditListModel(unittest.TestCase):
    """`several_apply` and `outside_supported_model` follow the model of
    docs/design/edit-lists.md decision 1, which skips an empty edit before
    it looks at the rate."""

    def test_several_media_edits_apply(self):
        self.assertTrue(ec.several_apply(1000, 48000, MULTI))
        # An empty edit at any rate presents nothing either way.
        self.assertTrue(ec.several_apply(1000, 48000, [(1000, 0, 0x10000), (10, -1, 0), (1000, 96000, 0x10000)]))

    def test_the_refusals_follow_coreav_s_classification(self):
        """`refuses_edit_list` over every shape of the simple model of edit
        lists and docs/design/edit-lists.md decision 1, in a
        movie with fragments and in one without."""
        unit = 0x10000
        cases = [
            # (entries, refused without fragments, refused with them)
            ([], False, False),
            ([(1000, 0, unit)], False, False),
            ([(0, 0, unit)], True, True),                        # to the end of the media
            ([(1000, -1, unit), (1000, 0, unit)], False, False),
            ([(1000, -1, unit), (0, 0, unit)], True, True),
            ([(1000, -5, unit)], True, True),                    # a media time below -1
            ([(1000, -1, unit), (1000, -5, unit)], True, True),
            ([(1000, 0, 0x20000)], True, True),                  # a rate other than 1
            ([(1000, -1, unit), (1000, 0, 0x20000)], True, True),
            ([(1000, -1, unit)], True, True),                    # an empty edit alone
            (MULTI, False, True),                                # several: without fragments only
            ([(1000, 0, unit), (1000, 96000, unit)], False, True),
            ([(1000, 0, unit), (10, -1, 0), (1000, 96000, unit)], False, True),
            ([(1000, 0, unit), (1000, 0, unit)], True, True),    # windows that do not increase
            ([(1000, 0, unit), (1000, 96000, 0x20000)], True, True),
            ([(1000, 0, unit), (0, 96000, unit)], True, True),
            ([(1000, 0, unit), (1000, -5, unit)], True, True),
            ([(1000, -1, unit), (1000, 0, unit), (1000, -1, unit)], False, True),
        ]
        for entries, plain, fragmented in cases:
            self.assertEqual(ec.refuses_edit_list(1000, 48000, entries, False), plain, entries)
            self.assertEqual(ec.refuses_edit_list(1000, 48000, entries, True), fragmented, entries)

    def test_a_media_edit_outside_the_model_does_not(self):
        for entries in (
            [(1000, 0, 0x10000), (1000, 0, 0x10000)],           # windows that do not increase
            [(1000, 0, 0x10000), (1000, 96000, 0x20000)],       # a rate other than 1
            [(1000, 0, 0x10000), (0, 96000, 0x10000)],          # a media edit of duration 0
            [(1000, 0, 0x10000), (1000, -5, 0x10000)],          # a media time below -1
            [(1000, -1, 0x10000), (500, -1, 0x10000)],          # no media edit
        ):
            self.assertFalse(ec.several_apply(1000, 48000, entries), entries)


if __name__ == '__main__':
    unittest.main()
