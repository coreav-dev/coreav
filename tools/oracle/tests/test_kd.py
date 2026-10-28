"""Keeps docs/known-divergences.md, its JSON twin and scripts/oracle_kd.py in sync,
and checks that each predicate rejects near misses."""
import hashlib
import functools
import importlib.util
import inspect
import json
import pathlib
import re
import shutil
import struct
from fractions import Fraction
import tempfile
import unittest
import zlib
from fractions import Fraction

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('oracle_kd', ROOT / 'scripts' / 'oracle_kd.py')
kd = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(kd)


class Reader:
    """Stands in for the overlay's file reader (`kd.Facts`) in tests without
    a file: `agrees` says whether CoreAV's raw values are the file's."""

    def __init__(self, agrees=True):
        self.agrees = agrees

    def stream_ok(self, *_):
        return self.agrees

    samples_ok = outside_model = aac_plain_lc = stream_ok


TRUSTED, DISTRUSTED = (lambda: Reader(True)), (lambda: Reader(False))


def stream(**fields):
    base = {'timescale': 100, 'duration_ts': 100, 'properties': {}}
    base.update(fields)
    return base


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences.md').read_text()
        documented = set(re.findall(r'^## (KD-[A-Z-]+)$', page, re.M))
        ids = {entry['id'] for entry in twin['entries']}
        self.assertEqual(documented, ids)
        for entry in twin['entries']:
            self.assertIn(entry['predicate'], kd.PREDICATES, entry['id'])
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')



REPORT = importlib.util.spec_from_file_location('report', ROOT / 'tools' / 'stage3-report' / 'report.py')
report = importlib.util.module_from_spec(REPORT)
REPORT.loader.exec_module(report)
MKV_SPEC = importlib.util.spec_from_file_location('oracle_kd_mkv', ROOT / 'scripts' / 'oracle_kd_mkv.py')
kd_mkv = importlib.util.module_from_spec(MKV_SPEC)
MKV_SPEC.loader.exec_module(kd_mkv)
TS_SPEC = importlib.util.spec_from_file_location('oracle_kd_ts', ROOT / 'scripts' / 'oracle_kd_ts.py')
kd_ts = importlib.util.module_from_spec(TS_SPEC)
TS_SPEC.loader.exec_module(kd_ts)


class DatedAgainstTheBars(unittest.TestCase):
    """Every known divergence of the probe overlays is dated against the
    stage 3 bars (docs/design/stage3-report.md, "Known divergences and the
    bars"): in PRE_BAR or in ADDED_AFTER_THE_BARS, never in neither, and
    with the fingerprint of its JSON record and predicate source, so an
    entry added or changed later is dated in the pull request that makes
    it."""

    def entries(self):
        """{id: fingerprint} of both overlays' entries."""
        found = {}
        # The evidence checks gate the entries that name them, so their code,
        # and the dispatch that applies them, are part of those entries'
        # fingerprints.
        text = (ROOT / 'scripts' / 'oracle_kd.py').read_text()
        evidence = text[text.index('# ' + '-' * 64 + ' evidence'):text.index('# ' + '-' * 64 + ' overlay')]
        evidence += ''.join(inspect.getsource(f) for f in (kd.stream_item, kd.packet_units, kd.reference_packet_units))
        # The transport stream entries' evidence: the file reader, and the
        # dispatch and helpers that apply the predicates.
        ts_text = (ROOT / 'scripts' / 'oracle_kd_ts.py').read_text()
        ts_evidence = ts_text[ts_text.index('# ' + '-' * 64 + ' evidence'):
                              ts_text.index('# ' + '-' * 64 + ' predicates')]
        # Every helper a transport stream predicate leans on, so that
        # widening one of them is dated like widening the predicate.
        ts_evidence += ''.join(inspect.getsource(f) for f in (
            kd_ts.format_from_streams, kd_ts.file_span, kd_ts.span_us, kd_ts.stream_item,
            kd_ts.packet_units, kd_ts.stream_duration, kd_ts.end_estimate,
            kd_ts.end_at_another_unit, kd_ts.timed_from_the_file, kd_ts.aac_profile,
            kd_ts.teletext_size, kd_ts.read_in_pieces, kd_ts.pes_places,
            kd_ts.coreav_duration_is_the_file_s, kd_ts.reference_ends,
            kd_ts.packets_of_their_pid, kd_ts.fingerprint, kd_ts.stream_permutation,
            kd_ts.listed_in_table_order, kd_ts.ordered_differently,
            kd_ts.order_covers_streams, kd_ts.packets_differing_under, kd_ts.ordered_paths,
            kd_ts.aac_sample_rate))
        for twin, overlay, shared in (('known-divergences.json', kd, evidence),
                                      ('known-divergences-mkv.json', kd_mkv, evidence),
                                      ('known-divergences-ts.json', kd_ts, ts_evidence)):
            for entry in json.loads((ROOT / 'docs' / twin).read_text())['entries']:
                source = inspect.getsource(overlay.PREDICATES[entry['predicate']])
                if 'evidence' in entry['conditions']:
                    source += shared
                text = json.dumps(entry, sort_keys=True) + source
                found[entry['id']] = hashlib.sha256(text.encode()).hexdigest()[:12]
        return found

    def known_ids(self):
        ids = set(self.entries())
        for overlay in ('oracle_kd.py', 'oracle_kd_mkv.py', 'oracle_kd_ts.py'):
            ids |= set(re.findall(r"'(KD-[A-Z0-9-]+)'", (ROOT / 'scripts' / overlay).read_text()))
        return ids

    def test_every_entry_is_dated(self):
        dated = set(report.PRE_BAR) | set(report.ADDED_AFTER_THE_BARS)
        self.assertEqual(self.known_ids() - dated, set(), 'date each new entry in tools/stage3-report/report.py')
        self.assertEqual(set(report.PRE_BAR) & set(report.ADDED_AFTER_THE_BARS), set())

    def test_a_changed_entry_is_dated_again(self):
        for kd_id, fingerprint in self.entries().items():
            if kd_id in report.ADDED_AFTER_THE_BARS:
                self.assertEqual(report.ADDED_AFTER_THE_BARS[kd_id][2], fingerprint,
                                 f'{kd_id} changed: add the pull request to its changed records in '
                                 'tools/stage3-report/report.py and update its fingerprint')

    def test_each_date_names_a_pull_request_and_commit(self):
        for kd_id, (added, changed, _) in report.ADDED_AFTER_THE_BARS.items():
            for record in (added, *changed):
                self.assertRegex(record, r'^#\d+ [0-9a-f]{7}$', kd_id)
        for kd_id, records in report.RETIRED.items():
            for record in records:
                self.assertRegex(record, r'^#\d+ [0-9a-f]{7}$', kd_id)

    def test_no_stale_names(self):
        listed = set(report.PRE_BAR) | set(report.ADDED_AFTER_THE_BARS)
        self.assertEqual(listed - set(self.entries()), set(), 'retired entries go to RETIRED')
        self.assertEqual(set(report.RETIRED) & set(self.entries()), set())


class Predicates(unittest.TestCase):
    def test_strict_needs_independent_evidence_of_the_claimed_defect(self):
        ok = {'probe_ok': True}

        def rejected(kind):
            return {'probe_ok': False, 'properties': {'rejection': kind}}
        self.assertTrue(kd.strict_rejection(ok, rejected('truncated_structure'), {'truncated'}))
        self.assertTrue(kd.strict_rejection(ok, rejected('invalid_structure'), {'misplaced'}))
        self.assertTrue(kd.strict_rejection(ok, rejected('invalid_structure'), {'no-mvhd'}))
        self.assertTrue(kd.strict_rejection(ok, rejected('invalid_structure'), {'second-moov'}))
        self.assertFalse(kd.strict_rejection(ok, rejected('truncated_structure'), {'no-mvhd'}))
        self.assertTrue(kd.strict_rejection(ok, rejected('limit'), {'deep'}))
        # A regression that rejects a valid file: no defect in its bytes.
        for kind in ('truncated_structure', 'invalid_structure', 'limit'):
            self.assertFalse(kd.strict_rejection(ok, rejected(kind), set()))
        self.assertFalse(kd.strict_rejection(ok, rejected('invalid_structure'), {'truncated'}))
        for kind in ('unsupported_container', 'codec_not_allowed', 'io'):
            self.assertFalse(kd.strict_rejection(ok, rejected(kind), {'truncated', 'misplaced', 'deep'}))
        self.assertFalse(kd.strict_rejection({'probe_ok': False}, rejected('limit'), {'deep'}))

    def test_structure_defects_reads_only_the_bytes(self):
        def box(kind, payload=b''):
            return (8 + len(payload)).to_bytes(4, 'big') + kind + payload
        mvhd = box(b'mvhd', bytes(100))
        valid = box(b'ftyp', b'isom') + box(b'moov', mvhd + box(b'trak', box(b'mdia'))) + box(b'mdat', b'x' * 10)
        self.assertEqual(kd.structure_defects(valid), set())
        self.assertEqual(kd.structure_defects(valid[:-1]), {'truncated'})
        self.assertEqual(kd.structure_defects(box(b'moov', mvhd + box(b'trak', box(b'trak')))), {'misplaced'})
        nested = box(b'mdia')
        for _ in range(40):
            nested = box(b'moov', mvhd + nested)
        self.assertEqual(kd.structure_defects(box(b'moov', mvhd + box(b'trak', nested))), {'deep'})
        # A child declaring 64 bytes inside an 8-byte payload.
        self.assertEqual(kd.structure_defects(box(b'moov', mvhd + (64).to_bytes(4, 'big') + b'trak')),
                         {'overrun'})
        # ISO/IEC 14496-12 8.2.1: one movie box, holding one movie header.
        self.assertEqual(kd.structure_defects(box(b'moov', box(b'trak'))), {'no-mvhd'})
        self.assertEqual(kd.structure_defects(box(b'moov', mvhd) + box(b'moov', mvhd)), {'second-moov'})
        # A movie header nested deeper does not count as the movie's, nor a
        # header in another movie box.
        self.assertEqual(kd.structure_defects(box(b'moov', box(b'trak', mvhd))), {'no-mvhd'})
        self.assertEqual(kd.structure_defects(box(b'moov', box(b'moov', mvhd))), {'no-mvhd'})
        self.assertEqual(kd.structure_defects(box(b'moov', mvhd + mvhd) + box(b'moov', box(b'trak'))),
                         {'second-moov', 'no-mvhd'})
        # QuickTime compressed and reference movies have no movie header.
        cmov = box(b'cmov', box(b'dcom', b'zlib') + box(b'cmvd', bytes(8)))
        self.assertEqual(kd.structure_defects(box(b'ftyp', b'qt  ') + box(b'moov', cmov)), set())
        self.assertEqual(kd.structure_defects(box(b'moov', box(b'rmra', box(b'rmda')))), set())
        # Valid layouts: the header after the tracks, a fragmented file, a
        # movie box inside another box.
        self.assertEqual(kd.structure_defects(box(b'moov', box(b'trak', box(b'mdia')) + mvhd)), set())
        fragmented = box(b'moov', mvhd + box(b'mvex')) + box(b'moof', box(b'traf')) + box(b'mdat') + box(b'mfra')
        self.assertEqual(kd.structure_defects(fragmented), set())
        self.assertEqual(kd.structure_defects(box(b'moov', mvhd) + box(b'free', box(b'moov'))), set())

    def test_edit_discard_needs_a_sample_outside_the_window(self):
        s = stream(properties={'edits': {'empty': 0, 'media_time': 0, 'segment': 2000},
                               'media_edit_duration_ticks': [200, 1], 'presentation_offset_ticks': [0, 1]})
        outside = {'discard': True, 'media_cts': 200, 'media_dts': 200, 'pts': 200, 'dts': 200, 'duration': 0}
        self.assertTrue(kd.edit_discard(s, outside))
        self.assertFalse(kd.edit_discard(s, dict(outside, media_cts=199, pts=199)))
        # The reference lists no such packet: its timestamps must be the
        # edit's mapping of its media times.
        self.assertFalse(kd.edit_discard(s, dict(outside, pts=201)))
        self.assertFalse(kd.edit_discard(s, dict(outside, dts=199)))
        self.assertFalse(kd.edit_discard(s, dict(outside, discard=False)))
        self.assertFalse(kd.edit_discard(stream(properties={'edits': 'none'}), outside))

    def test_ctts_shift_needs_the_exact_negative_minimum(self):
        packets = [{'media_dts': 0, 'media_cts': -10}, {'media_dts': 10, 'media_cts': 30}]
        s = stream(packets=packets, properties={'ctts_version': 1})
        cand = {'pos': 1, 'pts': 0, 'size': 5, 'keyframe': True, 'dts': 0}
        self.assertTrue(kd.ctts_shift(s, dict(cand, dts=-10), cand))
        self.assertFalse(kd.ctts_shift(s, dict(cand, dts=-9), cand))
        self.assertFalse(kd.ctts_shift(s, dict(cand, dts=-10, size=6), cand))
        self.assertFalse(kd.ctts_shift(stream(packets=packets, properties={'ctts_version': 0}),
                                       dict(cand, dts=-10), cand))
        # A QuickTime file's version 0 table, read signed, is covered; an ISO
        # file's version 0 table, read unsigned, is not.
        qt = stream(packets=packets, properties={'ctts_version': 0, 'ctts_signed': True})
        self.assertTrue(kd.ctts_shift(qt, dict(cand, dts=-10), cand))
        iso = stream(packets=packets, properties={'ctts_version': 0, 'ctts_signed': False})
        self.assertFalse(kd.ctts_shift(iso, dict(cand, dts=-10), cand))

    def test_edit_snap_with_a_signed_composition_shift(self):
        # Composition times 0, 512, -256 (signed, QuickTime): the edit at 100
        # starts at 512; ffprobe's decode times are also earlier by 256.
        packets = [{'media_dts': 0, 'media_cts': 0}, {'media_dts': 512, 'media_cts': 512},
                   {'media_dts': 1024, 'media_cts': 768}]
        s = stream(kind='video', packets=packets,
                   properties={'edits': {'empty': 0, 'media_time': 100, 'segment': 1000},
                               'ctts_version': 0, 'ctts_signed': True})
        self.assertEqual(kd.ctts_dts_shift(s), -256)
        cand = {'pos': 1, 'pts': 412, 'dts': 412, 'size': 5, 'keyframe': True}
        self.assertTrue(kd.edit_snap(s, dict(cand, pts=0, dts=-256), cand))
        self.assertFalse(kd.edit_snap(s, dict(cand, pts=0, dts=0), cand))
        unsigned = dict(s, properties=dict(s['properties'], ctts_signed=False))
        self.assertEqual(kd.ctts_dts_shift(unsigned), 0)
        self.assertFalse(kd.edit_snap(unsigned, dict(cand, pts=0, dts=-256), cand))

    def test_edit_snap_needs_the_first_composition_time_after_the_edit(self):
        # Composition times 0, 512, 1024, ...; the edit starts at 100, so
        # ffprobe starts it at 512.
        packets = [{'media_dts': 512 * i, 'media_cts': 512 * i} for i in range(4)]
        edits = {'empty': 0, 'media_time': 100, 'segment': 1000}
        s = stream(kind='video', packets=packets, properties={'edits': edits})
        self.assertEqual(kd.edit_snap_shift(s), 412)
        cand = {'pos': 1, 'pts': 412, 'dts': 412, 'size': 5, 'keyframe': True}
        self.assertTrue(kd.edit_snap(s, dict(cand, pts=0, dts=0), cand))
        self.assertFalse(kd.edit_snap(s, dict(cand, pts=0, dts=1), cand))
        self.assertFalse(kd.edit_snap(s, dict(cand, pts=0, dts=0, size=6), cand))
        self.assertFalse(kd.edit_snap(dict(s, kind='audio'), dict(cand, pts=0, dts=0), cand))
        on_a_frame = stream(kind='video', packets=packets, properties={'edits': dict(edits, media_time=512)})
        self.assertEqual(kd.edit_snap_shift(on_a_frame), 0)
        self.assertFalse(kd.edit_snap(on_a_frame, cand, cand))
        self.assertEqual(kd.edit_snap_shift(stream(kind='video', packets=packets, properties={'edits': 'none'})), 0)

    def test_pcm_grouping_needs_an_exact_tiling(self):
        cand = stream(codec_name='pcm_s16le', channels=1,
                      properties={'fixed_sample_size': 2, 'bits_per_sample': 16},
                      packets=[{'pos': 10 + 2 * i, 'size': 2, 'duration': 1, 'keyframe': True,
                                'pts': i, 'dts': i} for i in range(4)])
        ref = {'packets': [{'pos': 10, 'size': 4, 'duration': 2, 'keyframe': True, 'pts': 0, 'dts': 0},
                           {'pos': 14, 'size': 4, 'duration': 2, 'keyframe': True, 'pts': 2, 'dts': 2}]}
        self.assertTrue(kd.pcm_grouping(ref, cand))
        gap = json.loads(json.dumps(ref))
        gap['packets'][1]['pos'] = 16
        self.assertFalse(kd.pcm_grouping(gap, cand))
        timing = json.loads(json.dumps(ref))
        timing['packets'][1]['pts'] = 3
        self.assertFalse(kd.pcm_grouping(timing, cand))
        self.assertFalse(kd.pcm_grouping(ref, dict(cand, codec_name='aac')))

    def test_stts_short_needs_the_warning_and_the_exact_values(self):
        cand = {'streams': [stream(duration_ts=196, properties={
            'mdhd_duration_ticks': 196, 'stts_duration_ticks': 180,
            'media_edit_duration_ticks': None, 'warnings': ['duration conflict: mdhd 196 ticks']})]}
        ref = {'streams': [stream(duration_ts=180)]}
        self.assertTrue(kd.stts_short_stream(ref, cand, 0))
        self.assertFalse(kd.stts_short_stream({'streams': [stream(duration_ts=181)]}, cand, 0))
        no_warning = json.loads(json.dumps(cand))
        no_warning['streams'][0]['properties']['warnings'] = []
        self.assertFalse(kd.stts_short_stream(ref, no_warning, 0))

    def test_edit_duration_needs_both_rules_exactly(self):
        # AAC at 48 kHz in a 48 kHz movie: an empty edit of 11,228 ticks,
        # then a media edit of 836,160 ticks from media time 2,112; the
        # media header and the stts sum are 838,656.
        props = {'mdhd_duration_ticks': 838_656, 'stts_duration_ticks': 838_656,
                 'media_edit_duration_ticks': [836_160, 1],
                 'edits': {'empty': 11_228, 'media_time': 2_112, 'segment': 836_160}}
        cand = {'properties': {'movie_timescale': 48_000},
                'streams': [stream(timescale=48_000, duration_ts=836_160, properties=props)]}
        ref = {'streams': [stream(timescale=48_000, duration_ts=838_656)]}
        self.assertTrue(kd.edit_duration_stream(ref, cand, 0))
        # Any other reference value, or a candidate off its own rule, is not
        # covered.
        self.assertFalse(kd.edit_duration_stream({'streams': [stream(timescale=48_000, duration_ts=838_000)]},
                                                 cand, 0))
        off = json.loads(json.dumps(cand))
        off['streams'][0]['duration_ts'] = 836_000
        self.assertFalse(kd.edit_duration_stream(ref, off, 0))
        # The empty edit counts in the reference's rule.
        props_short = dict(props, mdhd_duration_ticks=900_000, stts_duration_ticks=900_000)
        cand_short = {'properties': {'movie_timescale': 48_000},
                      'streams': [stream(timescale=48_000, duration_ts=836_160, properties=props_short)]}
        self.assertTrue(kd.edit_duration_stream({'streams': [stream(timescale=48_000, duration_ts=847_388)]},
                                                cand_short, 0))
        # A short stts sum alone, under an edit that does not cut the media,
        # is KD-STTS-SHORT's case: not covered here.
        stts_only = {'mdhd_duration_ticks': 1000, 'stts_duration_ticks': 900,
                     'media_edit_duration_ticks': [2000, 1],
                     'edits': {'empty': 0, 'media_time': 0, 'segment': 2000}}
        cand_stts = {'properties': {'movie_timescale': 1000},
                     'streams': [stream(timescale=1000, duration_ts=1000, properties=stts_only)]}
        self.assertFalse(kd.edit_duration_stream({'streams': [stream(timescale=1000, duration_ts=900)]},
                                                 cand_stts, 0))
        cand_stts['streams'][0]['properties'] = dict(stts_only, media_edit_duration_ticks=[950, 1],
                                                     edits=dict(stts_only['edits'], segment=950))
        cand_stts['streams'][0]['duration_ts'] = 950
        self.assertFalse(kd.edit_duration_stream({'streams': [stream(timescale=1000, duration_ts=900)]},
                                                 cand_stts, 0))

    def test_frag_start_needs_the_first_tfdt_exactly(self):
        cand = {'streams': [stream(index=0, timescale=48000, duration_ts=95232,
                                   properties={'fragment_start_ticks': 866304})]}
        ref = lambda d: {'streams': [stream(index=0, timescale=48000, duration_ts=d)]}
        self.assertTrue(kd.frag_start_stream(ref(961536), cand, 0))
        self.assertFalse(kd.frag_start_stream(ref(961535), cand, 0))
        at_zero = {'streams': [stream(index=0, timescale=48000, duration_ts=95232,
                                      properties={'fragment_start_ticks': 0})]}
        self.assertFalse(kd.frag_start_stream(ref(95232), at_zero, 0))

    def test_frag_start_with_a_segment_index_ends_at_the_index(self):
        # First tfdt 1,000; the samples span 3,900 ticks; the sidx's earliest
        # presentation time plus its subsegments ends at 5,000.
        props = {'fragment_start_ticks': 1000, 'sidx_end_ticks': 5000, 'stts_duration_ticks': 3900}
        cand = {'streams': [stream(index=0, timescale=1000, duration_ts=3900, properties=props)]}
        ref = lambda d: {'streams': [stream(index=0, timescale=1000, duration_ts=d)]}
        self.assertTrue(kd.frag_start_stream(ref(5000), cand, 0))
        self.assertFalse(kd.frag_start_stream(ref(4900), cand, 0))
        # CoreAV's value must be its samples' sum, and end at or before the
        # index's end from the first tfdt: a short or long value is not
        # covered.
        for wrong in (1, 3899, 4001):
            bad = {'streams': [stream(index=0, timescale=1000, duration_ts=wrong, properties=props)]}
            self.assertFalse(kd.frag_start_stream(ref(5000), bad, 0), wrong)
        past = dict(props, stts_duration_ticks=4100)
        self.assertFalse(kd.frag_start_stream(
            ref(5000), {'streams': [stream(index=0, timescale=1000, duration_ts=4100, properties=past)]}, 0))

    def test_trun_shift_takes_the_smallest_offset_of_the_whole_track(self):
        # Offsets 0, 100 and -100: every reference pts is 100 later.
        packets = [{'media_dts': 0, 'media_cts': 0}, {'media_dts': 100, 'media_cts': 200},
                   {'media_dts': 200, 'media_cts': 100}]
        s = stream(packets=packets, properties={'fragment_start_ticks': 0, 'edits': 'none'})
        self.assertEqual(kd.trun_pts_shift(s), 100)
        cand = {'pos': 1, 'pts': 0, 'dts': 0, 'size': 5, 'keyframe': True}
        self.assertTrue(kd.trun_shift(s, dict(cand, pts=100), cand))
        self.assertFalse(kd.trun_shift(s, dict(cand, pts=99), cand))
        self.assertFalse(kd.trun_shift(s, dict(cand, pts=100, dts=100), cand))
        # Only fragment-only tracks without an edit list, with a negative
        # smallest offset.
        edited = stream(packets=packets, properties={'fragment_start_ticks': 0,
                                                     'edits': {'empty': 0, 'media_time': 0, 'segment': 300}})
        self.assertFalse(kd.trun_shift(edited, dict(cand, pts=100), cand))
        in_moov = stream(packets=packets, properties={'fragment_start_ticks': None, 'edits': 'none'})
        self.assertFalse(kd.trun_shift(in_moov, dict(cand, pts=100), cand))
        positive = stream(packets=packets[:2], properties={'fragment_start_ticks': 0, 'edits': 'none'})
        self.assertFalse(kd.trun_shift(positive, dict(cand, pts=100), cand))

    def test_ctts_v0_reads_wrapped_offsets_signed(self):
        # An ISO version 0 table storing -3,000 and 100, read unsigned.
        packets = [{'media_dts': 0, 'media_cts': (1 << 32) - 3000}, {'media_dts': 100, 'media_cts': 200}]
        s = stream(packets=packets, properties={'ctts_version': 0, 'ctts_signed': False})
        iso = {'major_brand': 'isom'}
        self.assertEqual(kd.ctts_v0_shift(s, iso), -3000)
        wrapped = dict(packets[0], pos=1, size=5, keyframe=True, pts=(1 << 32) - 3000, dts=0)
        self.assertTrue(kd.ctts_v0(s, dict(wrapped, pts=-3000, dts=-3000), wrapped, movie=iso))
        self.assertFalse(kd.ctts_v0(s, dict(wrapped, pts=-3000, dts=0), wrapped, movie=iso))
        plain = dict(packets[1], pos=2, size=5, keyframe=False, pts=200, dts=100)
        self.assertTrue(kd.ctts_v0(s, dict(plain, dts=-2900), plain, movie=iso))
        self.assertFalse(kd.ctts_v0(s, dict(plain, pts=200 - (1 << 32), dts=-2900), plain, movie=iso))
        signed = stream(packets=packets, properties={'ctts_version': 0, 'ctts_signed': True})
        self.assertIsNone(kd.ctts_v0_shift(signed, iso))
        self.assertFalse(kd.ctts_v0(signed, dict(wrapped, pts=-3000, dts=-3000), wrapped, movie=iso))
        # A QuickTime file, or one without a brand, read unsigned would be a
        # CoreAV bug: not covered.
        for movie in ({'major_brand': 'qt  '}, {'major_brand': None}, None):
            self.assertIsNone(kd.ctts_v0_shift(s, movie))
            self.assertFalse(kd.ctts_v0(s, dict(wrapped, pts=-3000, dts=-3000), wrapped, movie=movie))

    def test_pcm_end_ends_at_the_movie_duration_without_an_edit_list(self):
        # 24-bit stereo at 48 kHz; mvhd 601 ticks at 600 Hz is 48,080 ticks.
        pcm = stream(index=0, codec_name='pcm_s24le', channels=2, timescale=48000, time_base=[1, 48000],
                     duration_ts=48000, properties={'fixed_sample_size': 6, 'bits_per_sample': 24,
                                                    'edits': 'none', 'mdhd_duration_ticks': 48000})
        cand = {'properties': {'movie_timescale': 600, 'raw_movie_duration_ticks': 601}, 'streams': [pcm]}
        ref = lambda d: {'streams': [stream(index=0, timescale=48000, duration_ts=d)]}
        self.assertTrue(kd.pcm_end_stream(ref(48080), cand, 0))
        self.assertFalse(kd.pcm_end_stream(ref(48000), cand, 0))
        self.assertFalse(kd.pcm_end_stream(ref(48079), cand, 0))
        not_pcm = {**cand, 'streams': [dict(pcm, codec_name='aac')]}
        self.assertFalse(kd.pcm_end_stream(ref(48080), not_pcm, 0))
        # CoreAV's value must be the duration its duration rule gives
        # (here the media header's): a wrong or missing value
        # is not covered.
        for wrong in (47999, 48001, 12345, None):
            bad = {**cand, 'streams': [dict(pcm, duration_ts=wrong)]}
            self.assertFalse(kd.pcm_end_stream(ref(48080), bad, 0), wrong)
        # An unknown media header duration (0) gives no value under the
        # duration rule.
        unknown = dict(pcm, duration_ts=0, properties=dict(pcm['properties'], mdhd_duration_ticks=0))
        self.assertFalse(kd.pcm_end_stream(ref(48080), {**cand, 'streams': [unknown]}, 0))

    def test_format_span_counts_other_streams_ending_within_a_second(self):
        # Video to 10 s; a subtitle to 10.5 s counts, one to 11 s does not.
        starts = [Fraction(0)] * 3
        self.assertEqual(kd._span_us(starts, [10, Fraction(21, 2), 11], ['video', 'subtitle', 'data']), 10_500_000)
        self.assertEqual(kd._span_us(starts[:2], [10, 11], ['video', 'subtitle']), 10_000_000)
        # Without audio or video, every stream counts.
        self.assertEqual(kd._span_us(starts[:2], [10, 11], ['subtitle', 'data']), 11_000_000)

    def test_unplaced_streams_count_only_on_the_reference_side(self):
        # A 1 s video, and a 2 s one whose edit list CoreAV does not model.
        reference = {'format_duration_us': 2_000_000,
                     'streams': [stream(index=0, kind='video', timescale=1000, duration_ts=1000),
                                 stream(index=1, kind='video', timescale=1000, duration_ts=2000)]}
        candidate = {'format_duration_us': 1_000_000, 'properties': {'movie_timescale': 1000},
                     'streams': [stream(index=0, kind='video', timescale=1000, duration_ts=1000,
                                        properties={'edits': 'none'}),
                                 stream(index=1, kind='video', timescale=1000, duration_ts=1990,
                                        properties={'edits': 'unsupported'})]}
        self.assertEqual(kd.unplaced_format(reference, candidate), {'KD-EDIT-UNPLACED'})
        self.assertIsNone(kd.unplaced_format(dict(reference, format_duration_us=2_000_001), candidate))
        self.assertIsNone(kd.unplaced_format(reference, dict(candidate, format_duration_us=2_000_000)))
        modelled = dict(candidate, streams=[candidate['streams'][0],
                                            dict(candidate['streams'][1], properties={'edits': 'none'})])
        self.assertIsNone(kd.unplaced_format(reference, modelled))

    def test_edit_duration_with_a_short_stts_after_an_empty_edit(self):
        # 90 kHz video in a 1 kHz movie: empty 37, media edit 120,136 ms
        # (1,081,224 ticks); mdhd 1,081,558, stts 1,081,226. CoreAV gives
        # the edit; ffprobe the stts sum, below its edit term (which counts
        # the empty edit): the edit list makes the difference.
        props = {'mdhd_duration_ticks': 1_081_558, 'stts_duration_ticks': 1_081_226,
                 'media_edit_duration_ticks': [1_081_224, 1],
                 'edits': {'empty': 37, 'media_time': 0, 'segment': 120_136}}
        cand = {'properties': {'movie_timescale': 1000},
                'streams': [stream(index=0, timescale=90_000, duration_ts=1_081_224, properties=props)]}
        self.assertTrue(kd.edit_duration_stream(
            {'streams': [stream(index=0, timescale=90_000, duration_ts=1_081_226)]}, cand, 0))

    def test_aac_profile_entries_need_a_plain_lc_configuration(self):
        lc = {'index': 1, 'codec_name': 'aac', 'profile': 'LC', 'sample_rate': 24000,
              'properties': {'profile_code_points': kd.AAC_LC_CODE_POINTS}}
        cand = {'streams': [lc]}

        def ref(profile):
            return {'streams': [{'index': 1, 'codec_name': 'aac', 'profile': profile}]}
        self.assertTrue(kd.aac_implicit_sbr(ref('HE-AAC'), cand, 1))
        self.assertFalse(kd.aac_implicit_sbr(ref('HE-AACv2'), cand, 1))
        self.assertFalse(kd.aac_implicit_sbr(ref('HE-AAC'), {'streams': [dict(lc, sample_rate=48000)]}, 1))
        self.assertFalse(kd.aac_implicit_sbr(ref('HE-AAC'), cand, 0))
        explicit = dict(lc, properties={'profile_code_points': 'Aac { object_type: 2, sbr: true, ps: false }'})
        self.assertFalse(kd.aac_implicit_sbr(ref('HE-AAC'), {'streams': [explicit]}, 1))
        self.assertTrue(kd.aac_lc_unnamed(ref(None), cand, 1))
        self.assertFalse(kd.aac_lc_unnamed(ref('Main'), cand, 1))
        self.assertFalse(kd.aac_lc_unnamed(ref(None), {'streams': [dict(lc, profile='Main')]}, 1))
        self.assertFalse(kd.aac_lc_unnamed(ref(None), {'streams': [explicit]}, 1))
        detail = {'gating_differences': [{'path': 'streams/1/profile'}]}
        self.assertEqual(kd.stream_item(detail, ref('HE-AAC'), cand, set, facts=TRUSTED)[:2],
                         ('kd', {'KD-AAC-IMPLICIT-SBR'}))
        self.assertEqual(kd.stream_item(detail, ref('Main'), cand, set, facts=TRUSTED)[0], 'uncovered')
        # Without the file's AudioSpecificConfig showing plain LC: uncovered.
        self.assertEqual(kd.stream_item(detail, ref('HE-AAC'), cand, set, facts=DISTRUSTED)[0], 'uncovered')
        level = {'gating_differences': [{'path': 'streams/1/level'}]}
        self.assertEqual(kd.stream_item(level, ref(None), cand, set)[0], 'uncovered')



class QuickTimeText(unittest.TestCase):
    def documents(self, dropped_fourcc='text', reference_fourcc='text', extra=0):
        reference = {'streams': [{'index': 0, 'codec_fourcc': 'avc1'}, {'index': 1, 'codec_fourcc': reference_fourcc}]
                     + [{'index': 2 + i, 'codec_fourcc': 'mp4a'} for i in range(extra)]}
        candidate = {'streams': [{'index': 0, 'codec_fourcc': 'avc1'}],
                     'properties': {'dropped': [{'stream_index': 1, 'codec_fourcc': dropped_fourcc,
                                                 'reason': 'CodecNotAllowed', 'track_id': 2}]}}
        return reference, candidate

    def test_only_dropped_quicktime_text_streams_are_covered(self):
        self.assertTrue(kd.quicktime_text_dropped(*self.documents()))
        self.assertFalse(kd.quicktime_text_dropped(*self.documents(dropped_fourcc='ac-3')))
        self.assertFalse(kd.quicktime_text_dropped(*self.documents(reference_fourcc='tx3g')))
        # A stream missing without being reported as dropped.
        self.assertFalse(kd.quicktime_text_dropped(*self.documents(extra=1)))
        reference, candidate = self.documents()
        candidate['properties']['dropped'] = []
        self.assertFalse(kd.quicktime_text_dropped(reference, candidate))

    def test_stream_item_and_packets(self):
        reference, candidate = self.documents()
        detail = {'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set)[:2], ('kd', {'KD-QT-TEXT'}))
        # The field each entry covered, for the compat check (#219).
        self.assertEqual(kd.stream_item(detail, reference, candidate, set)[3],
                         {'streams/count': {'kd': ['KD-QT-TEXT'], 'candidate': None, 'reference': None}})
        # The count is no packet unit; other units are judged on their own.
        detail = {'packets_disagree': 0, 'gating_differences': [{'path': 'streams/count'}]}
        packets = {'streams': [{'index': 0}, {'index': 1}]}
        self.assertEqual(kd.packet_units(detail, packets, candidate, set, lambda: reference), ({}, 0, []))
        self.assertEqual(kd.packet_units(detail, packets, candidate, set)[2], ['streams/count'])
        # ffprobe lists the dropped stream's packets (a text track it reads as
        # mov_text): they count as disagreeing without being listed.
        listed = {'streams': [{'index': 0}, {'index': 1, 'packets': [{}, {}, {}]}]}
        detail = {'packets_disagree': 3, 'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.packet_units(detail, listed, candidate, set, lambda: reference),
                         ({'KD-QT-TEXT': 3}, 0, []))
        reference['streams'][1]['codec_fourcc'] = 'tx3g'
        self.assertEqual(kd.packet_units(detail, listed, candidate, set, lambda: reference)[2], ['streams/count'])


class NotAllowed(unittest.TestCase):
    def documents(self, dropped_fourcc='rtmd', reference_fourcc='rtmd', reason='CodecNotAllowed', extra=0):
        reference = {'streams': [{'index': 0, 'codec_fourcc': 'avc1'}, {'index': 1, 'codec_fourcc': reference_fourcc}]
                     + [{'index': 2 + i, 'codec_fourcc': 'mp4a'} for i in range(extra)]}
        candidate = {'streams': [{'index': 0, 'codec_fourcc': 'avc1'}],
                     'properties': {'dropped': [{'stream_index': 1, 'codec_fourcc': dropped_fourcc,
                                                 'reason': reason, 'track_id': 2}]}}
        return reference, candidate

    def test_only_streams_dropped_by_the_allowlist_are_covered(self):
        self.assertTrue(kd.allowlist_dropped(*self.documents()))
        self.assertTrue(kd.allowlist_dropped(*self.documents('gpmd', 'gpmd')))
        # A codec on the default allowlist is never covered: CoreAV failing
        # to recognise it is a bug.
        reference, candidate = self.documents('mp4a', 'mp4a')
        reference['streams'][1]['codec_name'] = 'aac'
        self.assertFalse(kd.allowlist_dropped(reference, candidate))
        reference['streams'][1]['codec_name'] = 'pcm_s16le'
        self.assertFalse(kd.allowlist_dropped(reference, candidate))
        reference, candidate = self.documents('tmcd', 'tmcd')
        self.assertFalse(kd.allowlist_dropped(reference, candidate))
        reference, candidate = self.documents('mp4v', 'mp4v')
        reference['streams'][1]['codec_name'] = 'mpeg4'
        self.assertTrue(kd.allowlist_dropped(reference, candidate))
        # The dropped fourcc must be the reference's at that index.
        self.assertFalse(kd.allowlist_dropped(*self.documents(reference_fourcc='mp4a')))
        self.assertFalse(kd.allowlist_dropped(*self.documents(reason='Unsupported')))
        # A stream missing without being reported as dropped.
        self.assertFalse(kd.allowlist_dropped(*self.documents(extra=1)))
        reference, candidate = self.documents()
        candidate['properties']['dropped'] = []
        self.assertFalse(kd.allowlist_dropped(reference, candidate))

    def test_stream_item_and_packets(self):
        reference, candidate = self.documents()
        detail = {'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set)[:2], ('kd', {'KD-NOT-ALLOWED'}))
        # Any other field stays uncovered: streams pair by index, so a
        # dropped stream's fields are never compared.
        detail = {'gating_differences': [{'path': 'streams/count'}, {'path': 'streams/1/kind'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set)[0], 'uncovered')
        # Another field of the file stays uncovered.
        detail = {'gating_differences': [{'path': 'streams/count'}, {'path': 'format_duration_us'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set)[0], 'uncovered')
        listed = {'streams': [{'index': 0}, {'index': 1, 'packets': [{}, {}]}]}
        detail = {'packets_disagree': 2, 'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.packet_units(detail, listed, candidate, set, lambda: reference),
                         ({'KD-NOT-ALLOWED': 2}, 0, []))
        # A protected entry is matched through the file's frma box.
        reference, candidate = self.documents('enca', 'mp4a')
        self.assertFalse(kd.allowlist_dropped(reference, candidate))
        self.assertTrue(kd.allowlist_dropped(reference, candidate, lambda: {1: 'mp4a'}))
        self.assertFalse(kd.allowlist_dropped(reference, candidate, lambda: {1: 'ac-3'}))
        # A QuickTime text track stays KD-QT-TEXT.
        text = self.documents('text', 'text')
        self.assertEqual(kd.packet_units(detail, listed, text[1], set, lambda: text[0]),
                         ({'KD-QT-TEXT': 2}, 0, []))

    def mixed(self, text_fourcc='text', other_name=None):
        """A camera file: video, a `priv` data track and a QuickTime text
        track that `ffprobe` reads as `mov_text`, both dropped."""
        reference = {'streams': [{'index': 0, 'codec_fourcc': 'avc1'},
                                 {'index': 1, 'codec_fourcc': 'priv', 'codec_name': other_name},
                                 {'index': 2, 'codec_fourcc': text_fourcc, 'codec_name': 'mov_text'}]}
        candidate = {'streams': [{'index': 0, 'codec_fourcc': 'avc1'}],
                     'properties': {'dropped': [
                         {'stream_index': 1, 'codec_fourcc': 'priv', 'reason': 'CodecNotAllowed', 'track_id': 2},
                         {'stream_index': 2, 'codec_fourcc': 'text', 'reason': 'CodecNotAllowed', 'track_id': 3}]}}
        return reference, candidate

    def test_text_and_other_dropped_streams_together(self):
        reference, candidate = self.mixed()
        # Neither entry explains both drops alone.
        self.assertFalse(kd.quicktime_text_dropped(reference, candidate))
        self.assertFalse(kd.allowlist_dropped(reference, candidate))
        # Only KD-NOT-ALLOWED makes the mix coverable, so it counts all of it.
        self.assertEqual(kd.dropped_entries(reference, candidate), {1: 'KD-NOT-ALLOWED', 2: 'KD-NOT-ALLOWED'})
        detail = {'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set)[:2], ('kd', {'KD-NOT-ALLOWED'}))
        listed = {'streams': [{'index': 0}, {'index': 1, 'packets': [{}] * 4}, {'index': 2, 'packets': [{}] * 3}]}
        detail = {'packets_disagree': 7, 'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.packet_units(detail, listed, candidate, set, lambda: reference),
                         ({'KD-NOT-ALLOWED': 7}, 0, []))
        # Each part must meet its own entry: a text track the reference does
        # not hold as `text`, or another track whose codec is allowlisted, is
        # not covered.
        self.assertIsNone(kd.dropped_entries(*self.mixed(text_fourcc='tx3g')))
        self.assertIsNone(kd.dropped_entries(*self.mixed(other_name='aac')))
        # A stream missing without being reported as dropped.
        reference, candidate = self.mixed()
        reference['streams'].append({'index': 3, 'codec_fourcc': 'mp4a'})
        self.assertIsNone(kd.dropped_entries(reference, candidate))
        # Two drops reported at one index.
        reference, candidate = self.mixed()
        candidate['properties']['dropped'][1]['stream_index'] = 1
        self.assertIsNone(kd.dropped_entries(reference, candidate))


def box(kind, payload=b''):
    return struct.pack('>I', 8 + len(payload)) + kind + payload


class CoverArt(unittest.TestCase):
    """KD-COVER-ART: the cover pictures of the item list, which ffprobe
    lists as streams."""

    def directory(self):
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        return directory

    def file(self, types=(13,), full_meta=True, under_udta=True, hdlr=True):
        covr = box(b'covr', b''.join(box(b'data', struct.pack('>II', t, 0) + b'image') for t in types))
        handler = box(b'hdlr', bytes(4) + bytes(4) + b'mdir' + bytes(12) + b'\0') if hdlr else b''
        meta = box(b'meta', (bytes(4) if full_meta else b'') + handler + box(b'ilst', covr))
        moov = box(b'moov', box(b'mvhd', bytes(100)) + (box(b'udta', meta) if under_udta else meta))
        directory = self.directory()
        (pathlib.Path(directory) / 'f.m4a').write_bytes(box(b'ftyp', b'M4A ' + bytes(4)) + moov)
        return directory

    def documents(self, codecs=('mjpeg',), dropped=False):
        reference = {'streams': [{'index': 0, 'kind': 'audio', 'codec_name': 'aac', 'codec_fourcc': 'mp4a'}]
                     + ([{'index': 1, 'kind': 'data', 'codec_name': None, 'codec_fourcc': 'rtmd'}] if dropped else [])
                     + [{'index': 1 + dropped + i, 'kind': 'video', 'codec_name': c, 'codec_fourcc': None}
                        for i, c in enumerate(codecs)]}
        candidate = {'streams': [{'index': 0, 'kind': 'audio', 'codec_name': 'aac', 'codec_fourcc': 'mp4a'}],
                     'properties': {'dropped': [{'stream_index': 1, 'codec_fourcc': 'rtmd',
                                                 'reason': 'CodecNotAllowed', 'track_id': 2}] if dropped else []}}
        return reference, candidate

    def test_pictures_are_read_from_the_item_list(self):
        self.assertEqual(kd.cover_art(self.file((13, 14)), 'f.m4a'), ['mjpeg', 'png'])
        # QuickTime's meta box, which is not a full box; moov/meta.
        self.assertEqual(kd.cover_art(self.file((14,), full_meta=False), 'f.m4a'), ['png'])
        self.assertEqual(kd.cover_art(self.file((27,), under_udta=False), 'f.m4a'), ['bmp'])
        # A picture of another type, or with a type-set byte, is no codec;
        # a `meta` without its `hdlr` holds no pictures.
        self.assertEqual(kd.cover_art(self.file((0,)), 'f.m4a'), [None])
        self.assertEqual(kd.cover_art(self.file((0x01000000 | 13,)), 'f.m4a'), [None])
        self.assertEqual(kd.cover_art(self.file(hdlr=False), 'f.m4a'), [])
        self.assertEqual(kd.cover_art(self.file(full_meta=False, hdlr=False), 'f.m4a'), [])
        # QuickTime metadata: items numbered by their key; only the artwork
        # key's item holds pictures.
        def key(name):
            return struct.pack('>I', 8 + len(name)) + b'mdta' + name
        keys = box(b'keys', bytes(4) + struct.pack('>I', 2) + key(b'com.apple.quicktime.title')
                   + key(b'com.apple.quicktime.artwork'))
        picture = box(b'data', struct.pack('>II', 14, 0) + b'image')
        ilst = box(b'ilst', box(struct.pack('>I', 1), box(b'data', struct.pack('>II', 1, 0) + b'title'))
                   + box(struct.pack('>I', 2), picture))
        meta = box(b'meta', box(b'hdlr', bytes(24) + b'\0') + keys + ilst)
        directory = self.directory()
        (pathlib.Path(directory) / 'f.mov').write_bytes(box(b'ftyp', b'qt  ' + bytes(4)) + box(b'moov', meta))
        self.assertEqual(kd.cover_art(directory, 'f.mov'), ['png'])
        self.assertEqual(kd.cover_art(self.file(()), 'f.m4a'), [])

    def test_only_the_trailing_pictures_are_covered(self):
        reference, candidate = self.documents()
        self.assertEqual(kd.cover_art_streams(reference, candidate, ['mjpeg']), {1})
        self.assertEqual(kd.stream_count_entries(reference, candidate, None, lambda: ['mjpeg']), {'KD-COVER-ART'})
        # Another codec, one picture fewer or more, a picture of another
        # type, or no cover at all.
        for covers in (['png'], [], ['mjpeg', 'mjpeg'], [None]):
            self.assertIsNone(kd.cover_art_streams(reference, candidate, covers))
        self.assertIsNone(kd.stream_count_entries(reference, candidate, None, None))
        # A missing stream that is not last, or not video.
        reference, candidate = self.documents(('mjpeg', 'mjpeg'))
        candidate['streams'].append(dict(reference['streams'][2]))
        self.assertIsNone(kd.cover_art_streams(reference, candidate, ['mjpeg']))
        reference, candidate = self.documents()
        reference['streams'][1]['kind'] = 'data'
        self.assertIsNone(kd.cover_art_streams(reference, candidate, ['mjpeg']))
        # A track (it has a sample-entry fourcc) CoreAV fails to list.
        reference, candidate = self.documents()
        reference['streams'][1]['codec_fourcc'] = 'jpeg'
        self.assertIsNone(kd.cover_art_streams(reference, candidate, ['mjpeg']))

    def test_with_streams_the_allowlist_drops(self):
        reference, candidate = self.documents(('png',), dropped=True)
        self.assertEqual(kd.stream_count_entries(reference, candidate, None, lambda: ['png']),
                         {'KD-COVER-ART', 'KD-NOT-ALLOWED'})
        # The dropped stream must still meet KD-NOT-ALLOWED.
        candidate['properties']['dropped'][0]['codec_fourcc'] = 'gpmd'
        self.assertIsNone(kd.stream_count_entries(reference, candidate, None, lambda: ['png']))

    def test_stream_item_and_packets(self):
        reference, candidate = self.documents(('mjpeg',), dropped=True)
        media = self.file((13,))
        detail = {'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set, media, 'f.m4a')[:2],
                         ('kd', {'KD-COVER-ART', 'KD-NOT-ALLOWED'}))
        self.assertEqual(kd.stream_item(detail, reference, candidate, set, self.file((14,)), 'f.m4a')[0],
                         'uncovered')
        listed = {'streams': [dict(s, packets=[{'pos': s['index'], 'size': 1}] * (3 if s['index'] == 1 else 1))
                              for s in reference['streams']]}
        detail = {'packets_disagree': 4, 'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.packet_units(detail, listed, candidate, set, lambda: reference,
                                         covers=lambda: ['mjpeg']),
                         ({'KD-COVER-ART': 1, 'KD-NOT-ALLOWED': 3}, 0, []))
        # A picture stream with more than one packet: its packets stay
        # uncovered.
        listed['streams'][2]['packets'] = [{'pos': 2, 'size': 1}] * 2
        detail['packets_disagree'] = 5
        self.assertEqual(kd.packet_units(detail, listed, candidate, set, lambda: reference,
                                         covers=lambda: ['mjpeg'])[1], 2)

    def test_with_a_quicktime_text_track(self):
        reference, candidate = self.documents(('png',), dropped=True)
        reference['streams'][1].update(codec_fourcc='text', codec_name='bin_data')
        candidate['properties']['dropped'][0]['codec_fourcc'] = 'text'
        self.assertEqual(kd.stream_count_entries(reference, candidate, None, lambda: ['png']),
                         {'KD-COVER-ART', 'KD-QT-TEXT'})
        listed = {'streams': [dict(s, packets=[{'pos': s['index'], 'size': 1}] * (2 if s['index'] == 1 else 1))
                              for s in reference['streams']]}
        detail = {'packets_disagree': 3, 'gating_differences': [{'path': 'streams/count'}]}
        self.assertEqual(kd.packet_units(detail, listed, candidate, set, lambda: reference,
                                         covers=lambda: ['png']),
                         ({'KD-COVER-ART': 1, 'KD-QT-TEXT': 2}, 0, []))


class ChapterText(unittest.TestCase):
    """KD-CHAPTER-TEXT: a `tx3g` track another track names in `tref/chap`,
    which ffprobe lists as a `bin_data` data stream without packets."""

    def file(self, chap_ids=(2,), tref_kind=b'chap', moov=None):
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        if moov is None:
            tref = box(b'tref', box(tref_kind, b''.join(struct.pack('>I', i) for i in chap_ids)))
            moov = box(b'moov', box(b'mvhd', bytes(100)) + box(b'trak', self.tkhd(1) + tref)
                       + box(b'trak', self.tkhd(2, version=1)))
        (pathlib.Path(directory) / 'f.m4a').write_bytes(box(b'ftyp', b'M4A ' + bytes(4)) + moov)
        return directory

    @staticmethod
    def tkhd(track_id, version=0):
        # Version and flags, the creation and modification times, the ID.
        times = bytes(16 if version == 1 else 8)
        return box(b'tkhd', bytes([version, 0, 0, 1]) + times + struct.pack('>I', track_id) + bytes(60))

    def documents(self, reference_name='bin_data', track_id=2):
        reference = {'streams': [{'index': 0, 'kind': 'audio', 'codec_name': 'aac', 'codec_fourcc': 'mp4a'},
                                 {'index': 1, 'kind': 'data' if reference_name == 'bin_data' else 'subtitle',
                                  'codec_name': reference_name, 'codec_fourcc': 'tx3g'}]}
        candidate = {'streams': [{'index': 0, 'kind': 'audio', 'codec_name': 'aac', 'codec_fourcc': 'mp4a',
                                  'properties': {'track_id': 1}},
                                 {'index': 1, 'kind': 'subtitle', 'codec_name': 'mov_text', 'codec_fourcc': 'tx3g',
                                  'properties': {'track_id': track_id}}]}
        return reference, candidate

    def test_chapter_references_are_read_from_the_file(self):
        # Positions among the tracks, matched by their tkhd track IDs.
        self.assertEqual(kd.chapter_tracks(self.file(), 'f.m4a'), {1})
        self.assertEqual(kd.chapter_tracks(self.file((2, 0, 1)), 'f.m4a'), {0, 1})
        # An ID no track has, or another reference type, names nothing.
        self.assertEqual(kd.chapter_tracks(self.file((3,)), 'f.m4a'), set())
        self.assertEqual(kd.chapter_tracks(self.file(tref_kind=b'hint'), 'f.m4a'), set())

    def test_malformed_boxes_name_no_chapter_track(self):
        trak = box(b'trak', self.tkhd(2))
        chap = box(b'chap', struct.pack('>I', 2))
        for moov in (b'', box(b'moov')[:6], struct.pack('>I', 0xFFFFFFFF) + b'moov' + trak,
                     box(b'moov', box(b'trak', self.tkhd(1) + box(b'tref', struct.pack('>I', 99) + b'chap')) + trak),
                     box(b'moov', box(b'trak', self.tkhd(1) + box(b'tref', box(b'chap', bytes(4100)))) + trak),
                     box(b'moov', box(b'trak', box(b'tkhd', bytes(10)) + box(b'tref', chap)))):
            self.assertEqual(kd.chapter_tracks(self.file(moov=moov), 'f.m4a'), set(), moov)

    def test_only_a_named_tx3g_track_is_covered(self):
        reference, candidate = self.documents()
        self.assertTrue(kd.chapter_text(reference, candidate, 1, lambda: {1}))
        self.assertFalse(kd.chapter_text(reference, candidate, 1, lambda: {0}))
        self.assertFalse(kd.chapter_text(reference, candidate, 1, None))
        self.assertFalse(kd.chapter_text(*self.documents('mov_text'), 1, lambda: {1}))
        reference, candidate = self.documents()
        candidate['streams'][1]['codec_fourcc'] = 'text'
        self.assertFalse(kd.chapter_text(reference, candidate, 1, lambda: {1}))

    def test_stream_item_and_packets(self):
        directory = self.file()
        reference, candidate = self.documents()
        detail = {'gating_differences': [{'path': 'streams/1/kind'}, {'path': 'streams/1/codec_name'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set, directory, 'f.m4a')[:2],
                         ('kd', {'KD-CHAPTER-TEXT'}))
        # Without the reference in the file, the fields stay uncovered.
        self.assertEqual(kd.stream_item(detail, reference, candidate, set, self.file(()), 'f.m4a')[0], 'uncovered')
        # Another field of the stream stays uncovered.
        other = {'gating_differences': [{'path': 'streams/1/kind'}, {'path': 'streams/1/duration_ts'}]}
        self.assertEqual(kd.stream_item(other, reference, candidate, set, directory, 'f.m4a')[0], 'uncovered')
        # CoreAV's packets of the stream, of which ffprobe lists none.
        packets_reference = {'streams': [dict(reference['streams'][0], packets=[]),
                                         dict(reference['streams'][1], packets=[])]}
        # Chapter samples at media times 0 and 1000, without an edit list.
        sample = lambda pos, t: {'pos': pos, 'pts': t, 'dts': t, 'media_cts': t, 'media_dts': t}  # noqa: E731
        text = dict(candidate['streams'][1], properties=dict(candidate['streams'][1].get('properties') or {},
                                                             edits='none', presentation_offset_ticks=[0, 1]))
        packets_candidate = {'streams': [dict(candidate['streams'][0], packets=[]),
                                         dict(text, packets=[sample(10, 0), sample(20, 1000)])]}
        units = {'packets_disagree': 2, 'gating_differences': [
            {'path': 'streams/1/packets/extra', 'candidate': sample(10, 0)},
            {'path': 'streams/1/packets/extra', 'candidate': sample(20, 1000)}]}
        self.assertEqual(kd.packet_units(units, packets_reference, packets_candidate, set, lambda: reference,
                                         chapters=lambda: {1}, facts=TRUSTED), ({'KD-CHAPTER-TEXT': 2}, 0, []))
        # CoreAV's chapter samples must be the file's.
        self.assertEqual(kd.packet_units(units, packets_reference, packets_candidate, set, lambda: reference,
                                         chapters=lambda: {1}, facts=DISTRUSTED)[1], 2)
        # The reference lists no chapter packet: CoreAV's timestamps must be
        # its media times as the edit list maps them.
        shifted = dict(units, gating_differences=[dict(units['gating_differences'][0],
                                                       candidate=dict(sample(10, 0), pts=1))]
                       + units['gating_differences'][1:])
        self.assertEqual(kd.packet_units(shifted, packets_reference, packets_candidate, set, lambda: reference,
                                         chapters=lambda: {1}, facts=TRUSTED)[1], 1)
        self.assertEqual(kd.packet_units(units, packets_reference, packets_candidate, set, lambda: reference,
                                         chapters=lambda: set())[1], 2)
        # A stream of which ffprobe lists packets is not a chapter track.
        packets_reference['streams'][1]['packets'] = [{'pos': 10}]
        self.assertEqual(kd.packet_units(units, packets_reference, packets_candidate, set, lambda: reference,
                                         chapters=lambda: {1}, facts=TRUSTED)[0], {})



class EditMulti(unittest.TestCase):
    """KD-EDIT-MULTI: edit lists of several entries (docs/design/edit-lists.md)."""
    UNIT = 0x10000

    @staticmethod
    def video(count=300, gop=30, offsets=None):
        """30 fps at 600 Hz: (cts, dts, duration, sync) per sample."""
        offsets = offsets or [0] * count
        return [(20 * i + offsets[i], 20 * i, 20, i % gop == 0) for i in range(count)]

    def track(self, edits, audio=False, has_ctts=False, timescale=600):
        return kd.EditTrack(1000, timescale, audio, has_ctts, [(d, t, self.UNIT) for d, t in edits])

    def test_ffprobe_listing_of_a_splice(self):
        # Measured: 0..89 shown, 90 listed after it, then 120..209 and 210.
        listing = kd.multi_edit_listing(self.video(), self.track([(3000, 0), (3000, 2400)]))
        self.assertEqual(len(listing), 182)
        self.assertEqual(listing[89], (89, 1780, 1780, False))
        self.assertEqual(listing[90], (90, 1800, 1800, True))
        # The second edit's first sample's decode time lands at its start.
        self.assertEqual(listing[91], (120, 1800, 1800, False))
        self.assertEqual(listing[-1], (210, 3600, 3600, True))
        # With a ctts box, to the second sync sample after the last shown.
        listing = kd.multi_edit_listing(self.video(), self.track([(3000, 0), (3000, 2400)], has_ctts=True))
        self.assertEqual([i for i, *_ in listing[90:122]], list(range(90, 121)) + [120])

    def test_ffprobe_listing_inside_a_group_of_pictures(self):
        # The second edit starts at 2500: preroll from the sync sample at 2400.
        listing = kd.multi_edit_listing(self.video(), self.track([(3000, 0), (3000, 2500)]))
        second = listing[91:]
        self.assertEqual(second[0], (120, 1700, 1700, True))
        self.assertEqual(second[5], (125, 1800, 1800, False))

    def test_ffprobe_listing_of_audio(self):
        # AAC at 44.1 kHz: 1024-sample frames, all sync.
        samples = [(1024 * i, 1024 * i, 1024, True) for i in range(431)]
        listing = kd.multi_edit_listing(samples, self.track([(2000, 0), (3000, 50000)], audio=True, timescale=44100))
        # The second edit's preroll starts one second before its media time.
        self.assertEqual(listing[87][0], 5)
        self.assertEqual(listing[87][3], True)
        self.assertEqual(listing[131], (49, 50176 + 38024, 50176 + 38024, False))

    def test_coreav_mapping(self):
        mapping = kd.multi_edit_mapping(self.video(), self.track([(3000, 0), (3000, 2500)]))
        # Exact: media 2500 is presented at 1800.
        self.assertEqual(mapping[125], (1800, 1800, True))
        self.assertEqual(mapping[90], (1100, 1100, False))
        # Overlapping windows are no list of several edits.
        self.assertIsNone(kd.multi_edit_mapping(self.video(), self.track([(3000, 0), (3000, 600)])))

    def documents(self, edits, listing_track=None):
        samples = self.video()
        track = self.track(edits)
        mapping = kd.multi_edit_mapping(samples, track)
        candidate = {'timescale': 600, 'properties': {'edits': 'multiple'}, 'packets': [
            {'pos': 1000 + i, 'size': 10, 'keyframe': s[3], 'media_cts': s[0], 'media_dts': s[1],
             'duration': s[2], 'pts': m[0], 'dts': m[1], 'discard': not m[2]}
            for i, (s, m) in enumerate(zip(samples, mapping))]}
        reference = {'packets': [
            {'pos': 1000 + i, 'size': 10, 'keyframe': samples[i][3], 'pts': pts, 'dts': dts, 'discard': discard}
            for i, pts, dts, discard in kd.multi_edit_listing(samples, listing_track or track)]}
        return reference, candidate, track

    def test_the_predicate_rechecks_both_sides(self):
        reference, candidate, track = self.documents([(3000, 0), (3000, 2500)])
        self.assertTrue(kd.edit_multi(reference, candidate, track))
        # One reference packet off the model.
        reference['packets'][100]['pts'] += 1
        self.assertFalse(kd.edit_multi(reference, candidate, track))
        reference, candidate, track = self.documents([(3000, 0), (3000, 2500)])
        candidate['packets'][100]['pts'] += 1
        self.assertFalse(kd.edit_multi(reference, candidate, track))
        # A missing packet, or a list CoreAV does not report as several.
        reference, candidate, track = self.documents([(3000, 0), (3000, 2500)])
        del reference['packets'][-1]
        self.assertFalse(kd.edit_multi(reference, candidate, track))
        reference, candidate, track = self.documents([(3000, 0), (3000, 2500)])
        candidate['properties']['edits'] = 'unsupported'
        self.assertFalse(kd.edit_multi(reference, candidate, track))
        self.assertFalse(kd.edit_multi(reference, candidate, None))

    def test_the_edit_list_is_read_from_the_file(self):
        def full(kind, version, body):
            return box(kind, bytes([version, 0, 0, 0]) + body)
        elst = full(b'elst', 1, struct.pack('>I', 2) + struct.pack('>Qqi', 3000, 0, self.UNIT)
                    + struct.pack('>Qqi', 3000, -1, self.UNIT))
        mdhd = full(b'mdhd', 0, struct.pack('>IIII', 0, 0, 44100, 0) + bytes(4))
        hdlr = full(b'hdlr', 0, bytes(4) + b'soun' + bytes(13))
        stbl = box(b'stbl', full(b'ctts', 0, struct.pack('>I', 0)))
        trak = box(b'trak', box(b'edts', elst) + box(b'mdia', mdhd + hdlr + box(b'minf', stbl)))
        mvhd = full(b'mvhd', 1, struct.pack('>QQIQ', 0, 0, 1000, 0) + bytes(80))
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        (pathlib.Path(directory) / 'f.mov').write_bytes(box(b'ftyp', b'qt  ' + bytes(4)) + box(b'moov', mvhd + trak))
        self.assertEqual(kd.edit_track(directory, 'f.mov', 0),
                         kd.EditTrack(1000, 44100, True, True, [(3000, 0, self.UNIT), (3000, -1, self.UNIT)]))
        self.assertIsNone(kd.edit_track(directory, 'f.mov', 1))

    def test_duration_of_several_edits(self):
        # 2000 + 1000 (empty) + 3000 ms at 600 Hz: ffprobe 3600, CoreAV the
        # media edits, 3000.
        entries = [[2000, 0, self.UNIT], [1000, -1, self.UNIT], [3000, 1200, self.UNIT]]
        candidate = {'properties': {'movie_timescale': 1000}, 'streams': [{
            'index': 0, 'timescale': 600, 'duration_ts': 3000, 'properties': {
                'edits': 'multiple', 'edit_entries': entries, 'mdhd_duration_ticks': 6000,
                'stts_duration_ticks': 6000, 'media_edit_duration_ticks': [3000, 1]}}]}
        reference = {'streams': [{'index': 0, 'duration_ts': 3600}]}
        self.assertTrue(kd.edit_duration_stream(reference, candidate, 0))
        reference['streams'][0]['duration_ts'] = 3000
        self.assertFalse(kd.edit_duration_stream(reference, candidate, 0))



class TextEnd(unittest.TestCase):
    """KD-TEXT-END: ffprobe extends a timed text track without an edit list
    to the latest end of any track of the file; every value the entry uses
    is read from the file (`kd.Facts`)."""

    @staticmethod
    def file(video_edit=9000, text_mdhd=3000, text_handler=b'sbtl', text_edit=None):
        """A 1000 Hz movie: video (track 1, 30000 Hz, 10 s, an edit of
        `video_edit` movie ticks) and timed text (track 2, 1000 Hz, three
        1000-tick samples, media header `text_mdhd`)."""
        mvhd = full_box(b'mvhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 10000) + b'\0' * 80)

        def trak(track_id, timescale, duration, handler, count, delta, edit):
            tkhd = full_box(b'tkhd', 0, 0, b'\0' * 8 + struct.pack('>I', track_id) + b'\0' * 68)
            mdhd = full_box(b'mdhd', 0, 0, b'\0' * 8 + struct.pack('>II', timescale, duration) + b'\0' * 4)
            hdlr = full_box(b'hdlr', 0, 0, b'\0' * 4 + handler + b'\0' * 13)
            stbl = box(b'stbl', full_box(b'stts', 0, 0, struct.pack('>III', 1, count, delta))
                       + full_box(b'stsz', 0, 0, struct.pack('>II', 4, count))
                       + full_box(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, count, 1))
                       + full_box(b'stco', 0, 0, struct.pack('>II', 1, 0)))
            edts = box(b'edts', full_box(b'elst', 0, 0, struct.pack('>I', len(edit))
                                         + b''.join(struct.pack('>IiI', *e) for e in edit))) if edit else b''
            return box(b'trak', tkhd + edts + box(b'mdia', mdhd + hdlr + box(b'minf', stbl)))
        moov = box(b'moov', mvhd + trak(1, 30000, 300000, b'vide', 10, 30000, [(video_edit, 0, 1 << 16)])
                   + trak(2, 1000, text_mdhd, text_handler, 3, 1000, text_edit))
        return box(b'ftyp', b'mp42\0\0\0\0') + moov + box(b'mdat', bytes(64))

    def write(self, data):
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        (pathlib.Path(directory) / 'f.mp4').write_bytes(data)
        return directory

    @staticmethod
    def documents(reference_duration=9000, coreav_duration=3000, mdhd=3000, codec='mov_text'):
        candidate = {'properties': {'fragmented': False, 'movie_timescale': 1000, 'raw_movie_duration_ticks': 10000,
                                    'major_brand': 'mp42'},
                     'streams': [{'index': 0, 'codec_name': 'h264', 'timescale': 30000, 'duration_ts': 270000,
                                  'properties': {'track_id': 1}},
                                 {'index': 1, 'codec_name': codec, 'timescale': 1000, 'duration_ts': coreav_duration,
                                  'properties': {'track_id': 2, 'edits': 'none', 'mdhd_duration_ticks': mdhd,
                                                 'stts_duration_ticks': 3000, 'duration_source': 'MediaHeader',
                                                 'edit_entries': [], 'ctts_version': None,
                                                 'media_edit_duration_ticks': None,
                                                 'presentation_offset_ticks': [0, 1]}}]}
        reference = {'streams': [{'index': 0, 'duration_ts': 270000}, {'index': 1, 'duration_ts': reference_duration}]}
        return reference, candidate

    def facts(self, **file):
        directory = self.write(self.file(**file))
        return directory, functools.cache(lambda: kd.read_facts(directory, 'f.mp4'))

    def test_the_latest_end_from_the_file(self):
        directory, facts = self.facts()
        reference, candidate = self.documents()
        self.assertTrue(kd.text_end(reference, candidate, 1, facts))
        detail = {'gating_differences': [{'path': 'streams/1/duration_ts'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set, directory, 'f.mp4')[:2],
                         ('kd', {'KD-TEXT-END'}))
        # Another value, or a reference shorter than CoreAV's: not covered.
        self.assertFalse(kd.text_end(*self.documents(9001), 1, facts))
        self.assertFalse(kd.text_end(*self.documents(2500), 1, facts))

    def test_coreav_must_report_the_files_media_header(self):
        # CoreAV misreads the text track's media header, its reported
        # evidence moving with it: the file shows 3000.
        directory, facts = self.facts()
        reference, candidate = self.documents(coreav_duration=3500, mdhd=3500)
        self.assertFalse(kd.text_end(reference, candidate, 1, facts))
        detail = {'gating_differences': [{'path': 'streams/1/duration_ts'}]}
        self.assertEqual(kd.stream_item(detail, reference, candidate, set, directory, 'f.mp4')[0], 'uncovered')
        # The text track's samples run past its media header.
        _, facts = self.facts(text_mdhd=2500)
        self.assertFalse(kd.text_end(*self.documents(coreav_duration=2500, mdhd=2500), 1, facts))

    def test_only_timed_text_without_an_edit_list(self):
        _, facts = self.facts()
        self.assertFalse(kd.text_end(*self.documents(codec='aac'), 1, facts))
        _, facts = self.facts(text_handler=b'soun')
        self.assertFalse(kd.text_end(*self.documents(), 1, facts))
        _, facts = self.facts(text_edit=[(3000, 0, 1 << 16)])
        self.assertFalse(kd.text_end(*self.documents(), 1, facts))
        reference, candidate = self.documents()
        candidate['properties']['fragmented'] = True
        self.assertFalse(kd.text_end(reference, candidate, 1, self.facts()[1]))
        reference, candidate = self.documents()
        candidate['streams'][1]['properties']['duration_source'] = 'Observed'
        self.assertFalse(kd.text_end(reference, candidate, 1, self.facts()[1]))

    def test_the_video_edit_gives_the_end(self):
        _, facts = self.facts(video_edit=4000)
        self.assertTrue(kd.text_end(*self.documents(4000), 1, facts))
        self.assertFalse(kd.text_end(*self.documents(9000), 1, facts))

class Runs(unittest.TestCase):
    def streams(self, run=8):
        reference = {'packets': [{'pos': 100 + 8 * i, 'size': 8, 'pts': 4 * i, 'dts': 4 * i, 'duration': 4,
                                  'keyframe': True} for i in range(2)]}
        candidate = {'codec_name': 'pcm_s16le', 'channels': 1,
                     'properties': {'fixed_sample_size': 2, 'bits_per_sample': 16},
                     'packets': [{'pos': 100, 'size': 2, 'pts': 0, 'dts': 0, 'duration': 1, 'keyframe': True,
                                  'run': run}]}
        return reference, candidate

    def test_pcm_tiling_walks_runs(self):
        self.assertTrue(kd.pcm_grouping(*self.streams()))
        # A run that stops short, or runs past the reference packets.
        self.assertFalse(kd.pcm_grouping(*self.streams(run=7)))
        self.assertFalse(kd.pcm_grouping(*self.streams(run=9)))
        # The same samples split into two runs at a packet boundary and
        # inside a packet.
        reference, candidate = self.streams()
        first = dict(candidate['packets'][0], run=4)
        candidate['packets'] = [first, dict(first, pos=108, pts=4, dts=4)]
        self.assertTrue(kd.pcm_grouping(reference, candidate))
        candidate['packets'] = [dict(first, run=3), dict(first, pos=106, pts=3, dts=3, run=5)]
        self.assertTrue(kd.pcm_grouping(reference, candidate))
        # A timestamp that does not continue.
        candidate['packets'] = [dict(first, run=3), dict(first, pos=106, pts=4, dts=4, run=5)]
        self.assertFalse(kd.pcm_grouping(reference, candidate))

    def test_runs_outside_the_edit_are_discarded(self):
        stream = {'properties': {'edits': {'media_time': 100, 'empty': 0}, 'media_edit_duration_ticks': [50, 1],
                                 'presentation_offset_ticks': [0, 1]}}
        run = {'pos': 0, 'size': 2, 'duration': 1, 'discard': True, 'media_cts': 10, 'media_dts': 10, 'pts': -90,
               'dts': -90, 'run': 20, 'unpaired': 20}
        self.assertTrue(kd.run_discarded(stream, run))
        # A run that reaches into the edit, or straddles it.
        self.assertFalse(kd.run_discarded(stream, dict(run, run=100)))
        self.assertFalse(kd.run_discarded(stream, dict(run, media_cts=90, media_dts=90, pts=-10, dts=-10, run=200)))
        self.assertTrue(kd.run_discarded(stream, dict(run, media_cts=150, media_dts=150, pts=50, dts=50)))
        # Timestamps off the edit's mapping.
        self.assertFalse(kd.run_discarded(stream, dict(run, pts=-89)))
        self.assertFalse(kd.run_discarded(stream, dict(run, discard=False)))

    def test_unpaired_run_samples_count_as_units(self):
        reference, candidate = self.streams()
        detail = {'packets_disagree': 8, 'gating_differences': [
            {'path': 'streams/0/packets/0/size'}, {'path': 'streams/0/packets/1/size'},
            {'path': 'streams/0/packets/extra_run', 'candidate': {'unpaired': 6}}]}
        documents = ({'streams': [dict(reference, index=0)]}, {'streams': [dict(candidate, index=0)]})
        self.assertEqual(kd.packet_units(detail, *documents, set, facts=TRUSTED), ({'KD-PCM': 8}, 0, []))
        self.assertEqual(kd.packet_units(detail, *documents, set, facts=DISTRUSTED)[:2], ({}, 8))
        detail['packets_disagree'] = 9
        self.assertEqual(kd.packet_units(detail, *documents, set)[2], ['unit count 8 differs from 9'])


class ReferenceUnit(unittest.TestCase):
    """Packets in the reference's unit (docs/design/stage3-report.md,
    "Packet unit"): one unit per ffprobe packet of a constant-size PCM
    stream."""
    streams = Runs.streams

    def test_each_tiled_reference_packet_is_one_unit(self):
        self.assertEqual(kd.pcm_reference_units(*self.streams()), (2, 0, {'KD-PCM': 2}, 0, []))

    def test_a_mistiled_packet_still_counts_as_a_mismatch(self):
        reference, candidate = self.streams()
        reference['packets'][1]['duration'] = 5
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        reference, candidate = self.streams()
        reference['packets'][1]['pts'] = 5
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        # Samples of another size, or a codec that is not PCM.
        reference, candidate = self.streams()
        self.assertEqual(kd.pcm_reference_units(reference, dict(candidate, codec_name='aac'))[:4], (2, 0, {}, 2))
        # A packet no candidate sample starts.
        reference['packets'][1]['pos'] = 109
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (3, 0, {'KD-PCM': 1}, 2))

    def test_the_last_packet_may_end_where_the_edit_ends(self):
        reference, candidate = self.streams()
        candidate.update(timescale=1, time_base=[1, 1])
        # The edit ends at media tick 6.5, rounded to 7 (half ticks round
        # up, unconfirmed): ffprobe's last packet lasts 3 ticks, not the 4
        # its samples last.
        candidate['properties'].update(edits={'media_time': 0, 'empty': 0, 'segment': 13},
                                       media_edit_duration_ticks=[13, 2])
        reference['packets'][1]['duration'] = 3
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 2}, 0))
        self.assertTrue(kd.pcm_grouping(reference, candidate))
        # Any other duration, a packet that is not the last, or no edit.
        for duration in (2, 5):
            reference['packets'][1]['duration'] = duration
            self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
            self.assertFalse(kd.pcm_grouping(reference, candidate))
        reference['packets'][1]['duration'] = 3
        candidate['properties']['media_edit_duration_ticks'] = [11, 2]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        candidate['properties']['media_edit_duration_ticks'] = [13, 2]
        reference['packets'][1]['duration'] = 4
        reference['packets'][0]['duration'] = 3
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        self.assertFalse(kd.pcm_grouping(reference, candidate))
        reference['packets'][0]['duration'] = 4
        reference['packets'][1]['duration'] = 3
        del candidate['properties']['edits']
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        self.assertFalse(kd.pcm_grouping(reference, candidate))
        # Malformed edit data is no evidence, and no error.
        for edits, ticks in (({'empty': 0}, [13, 2]), ({'media_time': None}, [13, 2]),
                             ({'media_time': 0}, [13]), ({'media_time': 0}, [13, 0]), ('none', [13, 2])):
            candidate['properties'].update(edits=edits, media_edit_duration_ticks=ticks)
            self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
            self.assertFalse(kd.pcm_grouping(reference, candidate))

    def test_without_an_edit_list_the_last_packet_may_end_with_the_movie(self):
        reference, candidate = self.streams()
        candidate.update(timescale=1, time_base=[1, 1])
        candidate['properties']['edits'] = 'none'
        # The movie lasts 35 ticks of a quarter second: 8.75 media ticks,
        # rounded to 9, so ffprobe's last packet runs one tick past its
        # samples.
        movie = {'movie_timescale': 4, 'raw_movie_duration_ticks': 35}
        reference['packets'][1]['duration'] = 5
        self.assertEqual(kd.pcm_reference_units(reference, candidate, movie)[:4], (2, 0, {'KD-PCM': 2}, 0))
        self.assertTrue(kd.pcm_grouping(reference, candidate, movie))
        # Another length, no movie duration, or a time base that is not
        # the media timescale.
        for other, base in ((dict(movie, raw_movie_duration_ticks=41), [1, 1]), ({}, [1, 1]), (movie, [1, 2])):
            candidate['time_base'] = base
            self.assertEqual(kd.pcm_reference_units(reference, candidate, other)[:4], (2, 0, {'KD-PCM': 1}, 1))
            self.assertFalse(kd.pcm_grouping(reference, candidate, other))

    def test_an_empty_edit_does_not_move_the_end(self):
        reference, candidate = self.streams()
        candidate.update(timescale=1, time_base=[1, 1])
        candidate['properties'].update(edits={'media_time': 0, 'empty': 2, 'segment': 7},
                                       media_edit_duration_ticks=[7, 1])
        # The empty edit shifts the timestamps by 2; ffprobe still ends the
        # stream at the edit's duration, 7.
        for packet in reference['packets'] + candidate['packets']:
            packet.update(pts=packet['pts'] + 2, dts=packet['dts'] + 2)
        reference['packets'][1]['duration'] = 1
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 2}, 0))
        self.assertTrue(kd.pcm_grouping(reference, candidate))
        # An edit that does not start at media time 0 is no evidence.
        candidate['properties']['edits']['media_time'] = 1
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        self.assertFalse(kd.pcm_grouping(reference, candidate))

    def test_the_last_packet_samples_keep_the_stream_step(self):
        reference, candidate = self.streams()
        candidate.update(timescale=1, time_base=[1, 1])
        candidate['properties'].update(edits={'media_time': 0, 'empty': 0, 'segment': 7},
                                       media_edit_duration_ticks=[7, 1])
        reference['packets'][1]['duration'] = 3
        first = dict(candidate['packets'][0], media_cts=0, run=4)
        # The last packet's samples all last 2, not 1: no cover, though the
        # packet starts a new run and its samples share one length.
        candidate['packets'] = [first, dict(first, pos=108, pts=4, dts=4, media_cts=4, duration=2)]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        self.assertFalse(kd.pcm_grouping(reference, candidate))
        # Every sample lasts 2: the first packet's own duration rejects it.
        candidate['packets'] = [dict(p, duration=2) for p in candidate['packets']]
        self.assertFalse(kd.pcm_grouping(reference, candidate))
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[2].get('KD-PCM', 0), 0)
        # A stream of one packet has no earlier packet to check the step.
        reference, candidate = self.streams(run=4)
        candidate.update(timescale=1, time_base=[1, 1])
        candidate['properties'].update(edits={'media_time': 0, 'empty': 0, 'segment': 3},
                                       media_edit_duration_ticks=[3, 1])
        reference['packets'] = [dict(reference['packets'][0], duration=3)]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (1, 0, {}, 1))
        self.assertFalse(kd.pcm_grouping(reference, candidate))

    def test_the_movie_reaches_the_rule_from_a_file(self):
        reference, candidate = self.streams()
        candidate.update(timescale=1, time_base=[1, 1])
        candidate['properties']['edits'] = 'none'
        reference['packets'][1]['duration'] = 5
        detail = {'file': 'f.mov', 'packets_agree': 0, 'packets_disagree': 8, 'gating_differences': [
            {'path': 'streams/0/packets/0/size'}, {'path': 'streams/0/packets/1/size'},
            {'path': 'streams/0/packets/extra_run', 'candidate': {'unpaired': 6}}]}
        documents = ({'streams': [dict(reference, index=0)]},
                     {'properties': {'movie_timescale': 4, 'raw_movie_duration_ticks': 36},
                      'streams': [dict(candidate, index=0)]})
        self.assertEqual(self.units(detail, *documents), (({'KD-PCM': 8}, 0, []), (2, 0, {'KD-PCM': 2}, 0, [])))
        documents[1]['properties']['raw_movie_duration_ticks'] = 40
        self.assertEqual(self.units(detail, *documents)[1][:4], (2, 0, {'KD-PCM': 1}, 1))

    def test_the_cut_last_packet_needs_samples_of_one_length(self):
        reference, candidate = self.streams()
        candidate.update(timescale=1, time_base=[1, 1])
        candidate['properties'].update(edits={'media_time': 0, 'empty': 0, 'segment': 13},
                                       media_edit_duration_ticks=[13, 2])
        reference['packets'][1]['duration'] = 3
        first = dict(candidate['packets'][0], media_cts=0)
        # The last packet starts inside a second run: still cut at the edit.
        candidate['packets'] = [dict(first, run=2), dict(first, pos=104, pts=2, dts=2, media_cts=2, run=6)]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 2}, 0))
        self.assertTrue(kd.pcm_grouping(reference, candidate))
        # Its last sample lasts longer than the others.
        candidate['packets'] = [dict(first, run=7), dict(first, pos=114, pts=7, dts=7, media_cts=7, duration=100, run=1)]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (2, 0, {'KD-PCM': 1}, 1))
        self.assertFalse(kd.pcm_grouping(reference, candidate))

    def test_samples_outside_every_reference_packet_are_one_unit_per_stretch(self):
        reference, candidate = self.streams(run=12)
        # Four samples past the reference packets: one stretch.
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (3, 0, {'KD-PCM': 2}, 1))
        # Split into two runs, still one stretch.
        first = dict(candidate['packets'][0], run=10)
        candidate['packets'] = [first, dict(first, pos=120, pts=10, dts=10, run=2)]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (3, 0, {'KD-PCM': 2}, 1))
        # Two stretches: before and after the reference packets.
        reference['packets'] = [dict(p, pos=p['pos'] + 2, pts=p['pts'] + 1, dts=p['dts'] + 1)
                                for p in reference['packets']]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (4, 0, {'KD-PCM': 2}, 2))

    def test_stretches_outside_the_edit_are_discarded(self):
        reference, candidate = self.streams(run=12)
        candidate['properties'].update(edits={'media_time': 0, 'empty': 0}, media_edit_duration_ticks=[8, 1],
                                       presentation_offset_ticks=[0, 1])
        entry = dict(candidate['packets'][0], run=8, discard=False, media_cts=0, media_dts=0)
        candidate['packets'] = [entry, dict(entry, pos=116, pts=8, dts=8, media_cts=8, media_dts=8,
                                            discard=True, run=4)]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (3, 0, {'KD-EDIT-DISCARD': 1,
                                                                                  'KD-PCM': 2}, 0))
        candidate['properties']['media_edit_duration_ticks'] = [10, 1]
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (3, 0, {'KD-PCM': 2}, 1))

    def test_packets_of_one_sample_agree_directly(self):
        _, candidate = self.streams()
        reference = {'packets': [{'pos': 100 + 2 * i, 'size': 2, 'pts': i, 'dts': i, 'duration': 1,
                                  'keyframe': True} for i in range(8)]}
        self.assertEqual(kd.pcm_reference_units(reference, candidate), (8, 8, {}, 0, []))

    def test_overlapping_candidate_packets_are_not_laid_out(self):
        reference, candidate = self.streams()
        candidate['packets'].append(dict(candidate['packets'][0], pos=104, run=2))
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (3, 0, {}, 3))
        reference, candidate = self.streams()
        candidate['packets'][0]['pos'] = None
        self.assertEqual(kd.pcm_reference_units(reference, candidate)[:4], (3, 0, {}, 3))

    def file(self, disagree=8, duration=4):
        reference, candidate = self.streams()
        reference['packets'][1]['duration'] = duration
        detail = {'file': 'f.mov', 'packets_agree': 0, 'packets_disagree': disagree, 'gating_differences': [
            {'path': 'streams/0/packets/0/size'}, {'path': 'streams/0/packets/1/size'},
            {'path': 'streams/0/packets/extra_run', 'candidate': {'unpaired': 6}}]}
        return detail, {'streams': [dict(reference, index=0)]}, {'streams': [dict(candidate, index=0)]}

    def units(self, detail, reference, candidate):
        breakdown = {}
        per_sample = kd.packet_units(detail, reference, candidate, set, breakdown=breakdown, facts=TRUSTED)
        return per_sample, kd.reference_packet_units(detail, reference, candidate, per_sample, breakdown, TRUSTED)

    def test_a_file_replaces_its_pcm_streams_per_sample_counts(self):
        per_sample, units = self.units(*self.file())
        self.assertEqual(per_sample, ({'KD-PCM': 8}, 0, []))
        self.assertEqual(units, (2, 0, {'KD-PCM': 2}, 0, []))
        # The per-sample count leaves the whole stream uncovered; the
        # reference unit only the mistiled packet.
        per_sample, units = self.units(*self.file(duration=5))
        self.assertEqual(per_sample[:2], ({}, 8))
        self.assertEqual(units, (2, 0, {'KD-PCM': 1}, 1, ['streams/0/packets/1/pcm_tiling']))

    def test_a_packet_differing_in_several_fields_counts_once(self):
        reference, candidate = self.streams()
        reference['packets'][1].update(pts=5, dts=5)
        video = [{'pos': 1 + i, 'size': 1, 'pts': i, 'dts': i, 'keyframe': True} for i in range(4)]
        detail = {'file': 'f.mov', 'packets_agree': 4, 'packets_disagree': 8, 'gating_differences': [
            {'path': 'streams/0/packets/0/size'}, {'path': 'streams/0/packets/1/size'},
            {'path': 'streams/0/packets/1/pts'}, {'path': 'streams/0/packets/1/dts'},
            {'path': 'streams/0/packets/extra_run', 'candidate': {'unpaired': 6}}]}
        documents = ({'streams': [dict(reference, index=0), {'index': 1, 'packets': video}]},
                     {'streams': [dict(candidate, index=0), {'index': 1, 'packets': video}]})
        self.assertEqual(self.units(detail, *documents)[1][:4], (6, 4, {'KD-PCM': 1}, 1))
        # Not attributed to streams: the mistiled packet stays uncovered.
        detail['gating_differences'].append({'path': 'streams/indexes'})
        self.assertEqual(self.units(detail, *documents)[1][:4], (6, 4, {}, 2))

    def test_unattributed_differences_stay_uncovered(self):
        per_sample, units = self.units(*self.file(disagree=9))
        self.assertEqual(per_sample[1:], (9, ['unit count 8 differs from 9']))
        # One unit the overlay cannot place, and the stream's two packets.
        self.assertEqual(units[:4], (3, 0, {}, 3))

    def test_files_without_runs_keep_the_comparison_unit(self):
        detail = {'file': 'f.mp4', 'packets_agree': 3, 'packets_disagree': 1,
                  'gating_differences': [{'path': 'streams/0/packets/1/pts'}]}
        packets = [{'pos': i, 'size': 1, 'pts': i, 'dts': i, 'keyframe': True} for i in range(4)]
        documents = {'streams': [{'index': 0, 'packets': packets}]}, {'streams': [{'index': 0, 'packets': packets}]}
        per_sample, units = self.units(detail, *documents)
        self.assertEqual(units, (4, 3) + per_sample)

if __name__ == '__main__':
    unittest.main()


def full_box(kind, version, flags, payload=b''):
    return box(kind, bytes([version]) + flags.to_bytes(3, 'big') + payload)


class Evidence(unittest.TestCase):
    """The overlay's own reader (`kd.Facts`) and the checks built on it: a
    CoreAV value whose evidence the file does not show is not covered."""

    def write(self, data, name='f.mp4'):
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        (pathlib.Path(directory) / name).write_bytes(data)
        return directory, name

    def movie(self, brand=b'isom', offsets=(0, 1024, -512, 512), ctts_version=0, handler_type=0,
              edits=((4096, 0, 1 << 16),), compressed=False, sizes=(10, 20, 30, 40), stss=None):
        """Four 512-tick samples of 10, 20, 30 and 40 bytes in one chunk, at
        timescale 1000 in a 1000 Hz movie; `brand` None for no `ftyp`; with
        `compressed`, the media data first and a zlib `cmov` movie box."""
        mvhd = full_box(b'mvhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 2048) + b'\0' * 80)
        tkhd = full_box(b'tkhd', 0, 0, b'\0' * 8 + struct.pack('>I', 1) + b'\0' * 68)
        mdhd = full_box(b'mdhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 2048) + b'\0' * 4)
        hdlr = full_box(b'hdlr', 0, 0, struct.pack('>I', handler_type) + b'vide' + b'\0' * 13)
        elst = full_box(b'elst', 0, 0, struct.pack('>I', len(edits))
                        + b''.join(struct.pack('>IiI', *e) for e in edits))
        stts = full_box(b'stts', 0, 0, struct.pack('>III', 1, 4, 512))
        ctts = full_box(b'ctts', ctts_version, 0, struct.pack('>I', len(offsets))
                        + b''.join(struct.pack('>Ii', 1, o) for o in offsets))
        stsz = full_box(b'stsz', 0, 0, struct.pack('>II', 0, 4) + b''.join(struct.pack('>I', s) for s in sizes))
        stsc = full_box(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, 4, 1))
        if stss is not None:
            stsc += full_box(b'stss', 0, 0, struct.pack('>I', len(stss)) + b''.join(struct.pack('>I', n) for n in stss))

        def build(chunk):
            stco = full_box(b'stco', 0, 0, struct.pack('>II', 1, chunk))
            stbl = box(b'stbl', stts + ctts + stsz + stsc + stco)
            trak = box(b'trak', tkhd + box(b'edts', elst) + box(b'mdia', mdhd + hdlr + box(b'minf', stbl)))
            ftyp = box(b'ftyp', brand + b'\0\0\0\0') if brand else b''
            return ftyp + box(b'moov', mvhd + trak)
        if compressed:
            ftyp = box(b'ftyp', brand + b'\0\0\0\0')
            moov = build(len(ftyp) + 8)[len(ftyp):]
            cmov = box(b'cmov', box(b'dcom', b'zlib') + box(b'cmvd', struct.pack('>I', len(moov)) + zlib.compress(moov)))
            return ftyp + box(b'mdat', bytes(sum(sizes))) + box(b'moov', cmov)
        head = build(0)
        return build(len(head) + 8) + box(b'mdat', bytes(sum(sizes)))

    def test_samples_from_the_tables(self):
        data = self.movie()
        facts = kd.read_facts(*self.write(data))
        samples = facts.samples(1)
        mdat = data.index(b'mdat') + 4
        self.assertEqual([(s.pos, s.size, s.dts, s.duration) for s in samples],
                         [(mdat, 10, 0, 512), (mdat + 10, 20, 512, 512), (mdat + 30, 30, 1024, 512),
                          (mdat + 60, 40, 1536, 512)])
        # ISO version 0 offsets are unsigned.
        self.assertEqual([s.cts - s.dts for s in samples], [0, 1024, (1 << 32) - 512, 512])
        self.assertEqual((facts.movie_timescale, facts.movie_duration, facts.brand), (1000, 2048, 'isom'))
        self.assertEqual(facts.tracks[1]['edits'], [(4096, 0, 1 << 16)])

    def test_a_compressed_movie_box(self):
        data = self.movie(brand=b'qt  ', compressed=True)
        facts = kd.read_facts(*self.write(data))
        mdat = data.index(b'mdat') + 4
        self.assertEqual([(s.pos, s.size, s.cts - s.dts) for s in facts.samples(1)],
                         [(mdat, 10, 0), (mdat + 10, 20, 1024), (mdat + 30, 30, -512), (mdat + 60, 40, 512)])
        self.assertEqual((facts.movie_timescale, facts.tracks[1]['mdhd_duration']), (1000, 2048))
        # A size that does not match the inflated data: no facts.
        at = data.index(b'cmvd') + 4
        bad = data[:at] + struct.pack('>I', struct.unpack('>I', data[at:at + 4])[0] + 1) + data[at + 4:]
        self.assertIsNone(kd.read_facts(*self.write(bad)))

    def test_sync_samples(self):
        facts = kd.read_facts(*self.write(self.movie(stss=(1, 3))))
        self.assertEqual(facts.sync(1), [True, False, True, False])
        self.assertEqual(kd.read_facts(*self.write(self.movie())).sync(1), [True] * 4)

    def test_quicktime_offsets_are_signed(self):
        for brand, handler_type in ((b'qt  ', 0), (None, int.from_bytes(b'mhlr', 'big'))):
            facts = kd.read_facts(*self.write(self.movie(brand=brand, handler_type=handler_type)))
            self.assertEqual([s.cts - s.dts for s in facts.samples(1)], [0, 1024, -512, 512], brand)
        # Without `ftyp` and without the QuickTime handler: the ISO reading.
        facts = kd.read_facts(*self.write(self.movie(brand=None)))
        self.assertEqual(facts.samples(1)[2].cts - facts.samples(1)[2].dts, (1 << 32) - 512)

    def candidate(self, data, **changes):
        """CoreAV's documents for the movie, as the adapter would write them."""
        facts = kd.read_facts(*self.write(data))
        packets = [{'pos': s.pos, 'size': s.size, 'media_dts': s.dts, 'media_cts': s.cts, 'duration': s.duration,
                    'keyframe': True} for s in facts.samples(1)]
        stream = {'index': 0, 'timescale': 1000, 'duration_ts': 2048, 'packets': packets,
                  'properties': {'track_id': 1, 'mdhd_duration_ticks': 2048, 'stts_duration_ticks': 2048,
                                 'edit_entries': [[4096, 0, 1 << 16]],
                                 'edits': {'empty': 0, 'media_time': 0, 'segment': 4096},
                                 'ctts_version': 0, 'ctts_signed': False, 'media_edit_duration_ticks': [4096, 1],
                                 'presentation_offset_ticks': [0, 1]}}
        stream['properties'].update(changes)
        movie = {'properties': {'movie_timescale': 1000, 'raw_movie_duration_ticks': 2048, 'major_brand': 'isom'},
                 'streams': [stream]}
        return facts, movie, stream

    def test_stream_values_must_be_the_files(self):
        data = self.movie()
        facts, movie, stream = self.candidate(data)
        self.assertTrue(facts.stream_ok(movie, stream))
        self.assertTrue(facts.samples_ok(movie, stream))
        # A parsing bug that moves a value and CoreAV's evidence together.
        for prop, value in (('mdhd_duration_ticks', 2049), ('stts_duration_ticks', 2560),
                            ('edit_entries', [[4196, 0, 1 << 16]]),
                            ('edits', {'empty': 0, 'media_time': 512, 'segment': 4096}),
                            ('ctts_version', 1), ('ctts_signed', True), ('media_edit_duration_ticks', [4097, 1])):
            facts, movie, stream = self.candidate(data, **{prop: value})
            self.assertFalse(facts.stream_ok(movie, stream), prop)
        facts, movie, stream = self.candidate(data)
        movie['properties']['raw_movie_duration_ticks'] = 2049
        self.assertFalse(facts.stream_ok(movie, stream))
        # A media edit CoreAV did not report (no duration cap), a
        # list it sorted otherwise, and a presentation offset off the empty
        # edits.
        for prop, value in (('media_edit_duration_ticks', None), ('edits', 'unsupported'), ('edits', 'none'),
                            ('presentation_offset_ticks', [1, 1])):
            facts, movie, stream = self.candidate(data, **{prop: value})
            self.assertFalse(facts.stream_ok(movie, stream), (prop, value))
        # A value the check needs that CoreAV left out is no evidence.
        for prop in ('mdhd_duration_ticks', 'edit_entries', 'media_edit_duration_ticks'):
            facts, movie, stream = self.candidate(data)
            del stream['properties'][prop]
            self.assertFalse(facts.stream_ok(movie, stream), prop)
        # Two streams claiming one track.
        facts, movie, stream = self.candidate(data)
        movie['streams'].append(dict(stream, index=1))
        self.assertFalse(facts.stream_ok(movie, stream))

    def test_no_cap_from_a_list_outside_the_model(self):
        # Rate 2: outside the model. CoreAV reports it unsupported, with no
        # media edit duration to cap the stream.
        data = self.movie(edits=((4096, 0, 2 << 16),))
        unsupported = {'edits': 'unsupported', 'edit_entries': [[4096, 0, 2 << 16]],
                       'media_edit_duration_ticks': None}
        facts, movie, stream = self.candidate(data, **unsupported)
        self.assertTrue(facts.stream_ok(movie, stream))
        facts, movie, stream = self.candidate(data, **dict(unsupported, media_edit_duration_ticks=[4096, 1]))
        self.assertFalse(facts.stream_ok(movie, stream))
        # Called simple, it is sorted otherwise than the file says.
        facts, movie, stream = self.candidate(data, edit_entries=[[4096, 0, 2 << 16]])
        self.assertFalse(facts.stream_ok(movie, stream))

    def test_every_packet_must_be_the_files_sample(self):
        data = self.movie()
        for field, change in (('media_dts', 1), ('media_cts', 1), ('pos', 1), ('size', 1), ('duration', 1),
                              ('keyframe', None)):
            for at in (0, 3):
                facts, movie, stream = self.candidate(data)
                if change is None:
                    # Without a sync sample table, every sample is a sync sample.
                    stream['packets'][at][field] = False
                else:
                    stream['packets'][at][field] += change
                self.assertFalse(facts.samples_ok(movie, stream), (field, at))
        # Every media time moved together (a decode time bug).
        facts, movie, stream = self.candidate(data)
        for p in stream['packets']:
            p['media_dts'] += 1
            p['media_cts'] += 1
        self.assertFalse(facts.samples_ok(movie, stream))
        # A packet left out, or listed twice.
        facts, movie, stream = self.candidate(data)
        del stream['packets'][2]
        self.assertFalse(facts.samples_ok(movie, stream))
        facts, movie, stream = self.candidate(data)
        stream['packets'].insert(1, dict(stream['packets'][1]))
        self.assertFalse(facts.samples_ok(movie, stream))

    def test_coded_data_keyframes_for_h264_and_av1(self):
        # The adapter reads H.264 and AV1 flags from the coded data
        # (tools/oracle/SPEC.md, "Keyframes"): a flag off the table is still
        # the file's sample for them, not for another codec.
        data = self.movie(stss=(1, 3))
        for codec, ok in (('h264', True), ('av1', True), ('hevc', False), (None, False)):
            facts, movie, stream = self.candidate(data)
            stream['codec_name'] = codec
            self.assertEqual(facts.samples_ok(movie, stream), ok, codec)

    def test_runs_of_equal_samples(self):
        # Four 10-byte samples with no offsets: one run in the file.
        data = self.movie(offsets=(0, 0, 0, 0), sizes=(10, 10, 10, 10))
        facts, movie, stream = self.candidate(data)
        first = dict(stream['packets'][0])
        for run, ok in ((4, True), (3, False), (5, False)):
            stream['packets'] = [dict(first, run=run)]
            self.assertEqual(facts.samples_ok(movie, stream), ok, run)
        # Split differently from the file's chunking, still the same samples.
        stream['packets'] = [dict(first, run=1), dict(first, pos=first['pos'] + 10, media_dts=512, media_cts=512,
                                                      run=3)]
        self.assertTrue(facts.samples_ok(movie, stream))
        stream['packets'] = [dict(first, run=4, duration=511)]
        self.assertFalse(facts.samples_ok(movie, stream))

    def test_a_track_fragment_after_another_tracks(self):
        # Track 8's fragment (base at the moof, its data at an offset), then
        # track 7's with no base and no data offset: it starts where track
        # 8's data ends (ISO/IEC 14496-12 8.8.7).
        mvhd = full_box(b'mvhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 0) + b'\0' * 80)

        def trak(track_id):
            tkhd = full_box(b'tkhd', 0, 0, b'\0' * 8 + struct.pack('>I', track_id) + b'\0' * 68)
            mdhd = full_box(b'mdhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 0) + b'\0' * 4)
            hdlr = full_box(b'hdlr', 0, 0, b'\0' * 4 + b'vide' + b'\0' * 13)
            stbl = box(b'stbl', full_box(b'stts', 0, 0, b'\0' * 4) + full_box(b'stsz', 0, 0, b'\0' * 8)
                       + full_box(b'stsc', 0, 0, b'\0' * 4) + full_box(b'stco', 0, 0, b'\0' * 4))
            return box(b'trak', tkhd + box(b'mdia', mdhd + hdlr + box(b'minf', stbl)))
        trex = b''.join(full_box(b'trex', 0, 0, struct.pack('>IIIII', t, 1, 100, size, 0)) for t, size in ((7, 5), (8, 30)))
        head = box(b'ftyp', b'iso6\0\0\0\0') + box(b'moov', mvhd + trak(7) + trak(8) + box(b'mvex', trex))

        def moof(offset):
            other = box(b'traf', full_box(b'tfhd', 0, 0x020000, struct.pack('>I', 8))
                        + full_box(b'trun', 0, 0x000001, struct.pack('>Ii', 1, offset)))
            ours = box(b'traf', full_box(b'tfhd', 0, 0, struct.pack('>I', 7))
                       + full_box(b'trun', 0, 0, struct.pack('>I', 2)))
            return box(b'moof', full_box(b'mfhd', 0, 0, struct.pack('>I', 1)) + other + ours)
        size = len(moof(0))
        data = head + moof(size + 8) + box(b'mdat', bytes(40))
        facts = kd.read_facts(*self.write(data))
        mdat = len(head) + size + 8
        self.assertEqual([(s.pos, s.size, s.dts) for s in facts.samples(7)], [(mdat + 30, 5, 0), (mdat + 35, 5, 100)])
        self.assertEqual([(s.pos, s.size) for s in facts.samples(8)], [(mdat, 30)])

    def fragmented(self, sidx=True, offsets=True):
        """An empty movie box, then one fragment of two 100-tick samples from
        decode time 1000, offsets 0 and -100 (`trun` version 1)."""
        mvhd = full_box(b'mvhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 0) + b'\0' * 80)
        tkhd = full_box(b'tkhd', 0, 0, b'\0' * 8 + struct.pack('>I', 7) + b'\0' * 68)
        mdhd = full_box(b'mdhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 0) + b'\0' * 4)
        hdlr = full_box(b'hdlr', 0, 0, b'\0' * 4 + b'vide' + b'\0' * 13)
        stbl = box(b'stbl', full_box(b'stts', 0, 0, b'\0' * 4) + full_box(b'stsz', 0, 0, b'\0' * 8)
                   + full_box(b'stsc', 0, 0, b'\0' * 4) + full_box(b'stco', 0, 0, b'\0' * 4))
        trak = box(b'trak', tkhd + box(b'mdia', mdhd + hdlr + box(b'minf', stbl)))
        trex = full_box(b'trex', 0, 0, struct.pack('>IIIII', 7, 1, 100, 50, 0))
        moov = box(b'ftyp', b'iso6\0\0\0\0') + box(b'moov', mvhd + trak + box(b'mvex', trex))
        index = full_box(b'sidx', 0, 0, struct.pack('>IIIIHH', 7, 1000, 900, 0, 0, 1)
                         + struct.pack('>III', 0, 400, 0)) if sidx else b''

        def moof(data_offset):
            tfhd = full_box(b'tfhd', 0, 0x020000, struct.pack('>I', 7))
            tfdt = full_box(b'tfdt', 1, 0, struct.pack('>Q', 1000))
            if offsets:
                trun = full_box(b'trun', 1, 0x000801, struct.pack('>Ii', 2, data_offset) + struct.pack('>ii', 0, -100))
            else:
                trun = full_box(b'trun', 0, 0x000001, struct.pack('>Ii', 2, data_offset))
            return box(b'moof', full_box(b'mfhd', 0, 0, struct.pack('>I', 1)) + box(b'traf', tfhd + tfdt + trun))
        first = moof(0)
        return moov + index + moof(len(first) + 8) + box(b'mdat', bytes(100))

    def test_fragment_samples_tfdt_and_sidx(self):
        data = self.fragmented()
        facts = kd.read_facts(*self.write(data))
        mdat = data.rindex(b'mdat') + 4
        self.assertEqual(facts.samples(7), [kd.Sample(mdat, 50, 1000, 1000, 100),
                                            kd.Sample(mdat + 50, 50, 1100, 1000, 100)])
        self.assertEqual(facts.first_tfdt(7), 1000)
        self.assertEqual(facts.sidx_end(7, 1000), 1300)
        self.assertIsNone(facts.sidx_end(7, 90000))
        self.assertIsNone(kd.read_facts(*self.write(self.fragmented(sidx=False))).sidx_end(7, 1000))
        # Samples that all take the defaults come as one run after the first.
        data = self.fragmented(offsets=False)
        facts = kd.read_facts(*self.write(data))
        mdat = data.rindex(b'mdat') + 4
        self.assertEqual([(r.pos, r.count) for r in facts.runs(7)], [(mdat, 1), (mdat + 50, 1)])
        self.assertEqual([(s.pos, s.dts, s.cts) for s in facts.samples(7)], [(mdat, 1000, 1000), (mdat + 50, 1100, 1100)])

    def test_edit_lists_outside_the_model(self):
        movie = {'properties': {'movie_timescale': 1000, 'raw_movie_duration_ticks': 2048, 'major_brand': 'isom'}}
        stream = {'properties': {'track_id': 1}}
        for edits, outside in ((((4096, 0, 1 << 16),), False),
                               (((1000, -1, 1 << 16), (1000, 0, 1 << 16), (1000, 1000, 1 << 16)), False),
                               (((1000, 0, 1 << 16), (1000, 500, 1 << 16)), True),
                               (((1000, 1000, 1 << 16), (1000, 0, 1 << 16)), True),
                               (((1000, 0, 2 << 16),), True), (((1000, 0, 0),), True),
                               (((1000, -2, 1 << 16),), True), (((0, 0, 1 << 16),), True)):
            facts = kd.read_facts(*self.write(self.movie(edits=edits)))
            self.assertEqual(facts.outside_model(movie, stream), outside, edits)

    def test_plain_lc_needs_the_files_configuration(self):
        def esds(asc):
            dsi = b'\x05' + bytes([len(asc)]) + asc
            dcd = b'\x04' + bytes([13 + len(dsi)]) + b'\x40\x15' + b'\0' * 11 + dsi
            es = b'\x03' + bytes([3 + len(dcd)]) + b'\0\x01\0' + dcd
            return full_box(b'esds', 0, 0, es)

        def file(asc):
            entry = box(b'mp4a', b'\0' * 6 + b'\0\x01' + b'\0' * 8 + struct.pack('>HHHHI', 2, 16, 0, 0, 24000 << 16)
                        + esds(asc))
            stsd = full_box(b'stsd', 0, 0, struct.pack('>I', 1) + entry)
            mvhd = full_box(b'mvhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 0) + b'\0' * 80)
            tkhd = full_box(b'tkhd', 0, 0, b'\0' * 8 + struct.pack('>I', 2) + b'\0' * 68)
            mdhd = full_box(b'mdhd', 0, 0, b'\0' * 8 + struct.pack('>II', 24000, 0) + b'\0' * 4)
            hdlr = full_box(b'hdlr', 0, 0, b'\0' * 4 + b'soun' + b'\0' * 13)
            trak = box(b'trak', tkhd + box(b'mdia', mdhd + hdlr + box(b'minf', box(b'stbl', stsd))))
            return box(b'ftyp', b'M4A \0\0\0\0') + box(b'moov', mvhd + trak)
        stream = {'sample_rate': 24000, 'properties': {'track_id': 2}}
        # Object type 2, 24 kHz (index 6), stereo.
        lc = bytes([2 << 3 | 6 >> 1, (6 & 1) << 7 | 2 << 3])
        self.assertTrue(kd.read_facts(*self.write(file(lc))).aac_plain_lc(stream))
        self.assertFalse(kd.read_facts(*self.write(file(lc))).aac_plain_lc(dict(stream, sample_rate=48000)))
        # An explicit SBR extension after the configuration, or object type 5.
        self.assertFalse(kd.read_facts(*self.write(file(lc + b'\x56\xe5\x00'))).aac_plain_lc(stream))
        he = bytes([5 << 3 | 6 >> 1, (6 & 1) << 7 | 2 << 3])
        self.assertFalse(kd.read_facts(*self.write(file(he))).aac_plain_lc(stream))
