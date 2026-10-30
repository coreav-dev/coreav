"""KD-HLS-3's predicate in scripts/hls_check.py takes the edit and the
composition times from the source file, not from the rendition alone: a
rendition whose times or edit are wrong must not be covered
(docs/measurements/kd-audit-writers.md)."""
import importlib.util
import pathlib
import shutil
import tempfile
import unittest
from fractions import Fraction
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('hls_check', ROOT / 'scripts' / 'hls_check.py')
hc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(hc)

PACKAGE = pathlib.Path('package')
# Media time 1000 lies between the samples at 900 and 1100 (timescale 1000).
TIMES = [0, 300, 600, 900, 1100, 1400]


def midframe(rendition_times=TIMES, source_times=TIMES, init_media_time=1000, source_edits=((5000, 1000, 1 << 16),),
             delta=Fraction(100, 1000)):
    with mock.patch.object(hc, 'init_edit_window', return_value=(init_media_time, 5000)), \
            mock.patch.object(hc, 'composition_times', return_value=list(rendition_times)), \
            mock.patch.object(hc, 'source_composition_times', return_value=None if source_times is None else list(source_times)), \
            mock.patch.object(hc.cc, 'full_edit_list', return_value=(1000, 1000, list(source_edits))):
        return hc.kd_midframe_edit(PACKAGE, 'init.mp4', 'video', 1000, delta, 'source.mp4', 0)


class MidframeEdit(unittest.TestCase):
    def test_holds_for_the_sources_edit_and_times(self):
        self.assertTrue(midframe())
        self.assertTrue(midframe(source_edits=((40, -1, 1 << 16), (5000, 1000, 1 << 16))))

    def test_rejects_a_rendition_timeline_moved_from_the_sources(self):
        # A tfdt one tick late moves every composition time with it.
        self.assertFalse(midframe(rendition_times=[t + 1 for t in TIMES], init_media_time=1001,
                                  source_edits=((5000, 1001, 1 << 16),)))
        self.assertFalse(midframe(rendition_times=[t + 1 for t in TIMES]))

    def test_rejects_an_init_edit_other_than_the_sources(self):
        self.assertFalse(midframe(init_media_time=1001, delta=Fraction(99, 1000)))
        self.assertFalse(midframe(source_edits=()))
        self.assertFalse(midframe(source_edits=((5000, 1000, 1 << 16), (5000, 2000, 1 << 16))))

    def test_rejects_without_the_source(self):
        with mock.patch.object(hc, 'init_edit_window', return_value=(1000, 5000)), \
                mock.patch.object(hc, 'composition_times', return_value=TIMES):
            self.assertFalse(hc.kd_midframe_edit(PACKAGE, 'init.mp4', 'video', 1000, Fraction(100, 1000)))

    def test_rejects_an_unreadable_source(self):
        self.assertFalse(midframe(source_times=None))
        with mock.patch.object(hc, 'init_edit_window', return_value=(1000, 5000)), \
                mock.patch.object(hc, 'composition_times', return_value=TIMES), \
                mock.patch.object(hc, 'source_composition_times', return_value=TIMES), \
                mock.patch.object(hc.cc, 'full_edit_list', return_value=None):
            self.assertFalse(hc.kd_midframe_edit(PACKAGE, 'init.mp4', 'video', 1000, Fraction(100, 1000),
                                                 'source.mp4', 0))



def sidx(reference_id, scale, durations, version=0, reference_type=0):
    """A top-level `sidx` box (ISO/IEC 14496-12 8.16.3)."""
    body = bytes([version, 0, 0, 0]) + reference_id.to_bytes(4, 'big') + scale.to_bytes(4, 'big')
    body += bytes(8 if version == 0 else 16) + bytes(2) + len(durations).to_bytes(2, 'big')
    for d in durations:
        body += ((reference_type << 31) | 1000).to_bytes(4, 'big') + d.to_bytes(4, 'big') + (0x90000000).to_bytes(4, 'big')
    return (8 + len(body)).to_bytes(4, 'big') + b'sidx' + body


class SegmentIndex(unittest.TestCase):
    def write(self, data):
        tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        path = tmp / 'source.mp4'
        path.write_bytes(data)
        return path

    def test_reads_the_one_index(self):
        box = sidx(1, 90000, [359430, 359431])
        self.assertEqual(hc.source_index(self.write(box)), (1, 90000, [(1000, 359430), (1000, 359431)], len(box)))
        box = sidx(2, 1000, [5], version=1)
        self.assertEqual(hc.source_index(self.write(b'\0\0\0\x08free' + box)), (2, 1000, [(1000, 5)], 8 + len(box)))

    def test_refuses_two_indexes_or_a_reference_to_an_index(self):
        self.assertIsNone(hc.source_index(self.write(sidx(1, 1000, [5]) * 2)))
        self.assertIsNone(hc.source_index(self.write(sidx(1, 1000, [5], reference_type=1))))
        self.assertIsNone(hc.source_index(self.write(b'')))


# Three subsegments of 100 bytes from offset 1000, each one track fragment
# of two samples of 100 ticks (timescale 1000) whose decode time lies 150
# ticks past where the durations reach, while the index gives each the 200
# ticks its durations add up to.
INDEX = (1, 1000, [(100, 200), (100, 200), (100, 200)], 1000)
RUNS = [(1000, 0, [100, 100], [0, 0]), (1100, 350, [100, 100], [0, 0]), (1200, 700, [100, 100], [0, 0])]
CONTINUOUS = [(1000, 0, [100, 100], [0, 0]), (1100, 200, [100, 100], [0, 0]), (1200, 400, [100, 100], [0, 0])]
SEGMENTS = [(Fraction(2, 10), 'a0.m4s'), (Fraction(2, 10), 'a1.m4s'), (Fraction(2, 10), 'a2.m4s')]
STARTS = [Fraction(0), Fraction(2, 10), Fraction(4, 10)]


def drift(index=INDEX, runs=RUNS, in_table=0, segments=SEGMENTS, starts=STARTS, counts=(2, 2, 2)):
    sizes = iter(counts)
    with mock.patch.object(hc, 'source_index', return_value=index), \
            mock.patch.object(hc, 'source_fragments', return_value=(1000, in_table, list(runs))), \
            mock.patch.object(hc, 'trun_samples', side_effect=lambda _: [None] * next(sizes)):
        return hc.kd_index_drift('source.mp4', 1, PACKAGE, segments, starts)


class IndexDrift(unittest.TestCase):
    def test_the_drift_is_the_sources_decode_time_gaps(self):
        self.assertEqual(drift(), [0, Fraction(15, 100), Fraction(3, 10)])
        # The first segment may also span the earliest presentation time.
        self.assertEqual(drift(starts=[0, Fraction(7, 10), Fraction(9, 10)]), [0, Fraction(15, 100), Fraction(3, 10)])
        # A gap inside a subsegment moves the later ones only.
        split = [(1000, 0, [100], [0]), (1050, 100, [100], [0]), (1100, 350, [100, 100], [0, 0]),
                 (1200, 600, [100, 100], [0, 0])]
        self.assertEqual(drift(runs=split), [0, Fraction(15, 100), Fraction(2, 10)])

    def test_rejects_near_misses(self):
        # Fragments that continue their durations: no drift to cover.
        self.assertIsNone(drift(runs=CONTINUOUS))
        # A writer that cuts a consistent source at other samples, or the
        # inconsistent one: the counts are the index's byte ranges', not
        # the segments' own (a review finding).
        self.assertIsNone(drift(runs=CONTINUOUS, counts=(4, 1, 1)))
        self.assertIsNone(drift(counts=(4, 1, 1)))
        self.assertIsNone(drift(counts=(2, 2, 1)))
        # Starts that are not the index's running sums.
        self.assertIsNone(drift(starts=[0, Fraction(2, 10), Fraction(41, 100)]))
        # An index whose durations are not the stored durations' sums: the
        # drift is then more than the gaps.
        self.assertIsNone(drift(index=(1, 1000, [(100, 200), (100, 190), (100, 200)], 1000),
                                starts=[0, Fraction(2, 10), Fraction(39, 100)]))
        # An index of another track, with another subsegment count, or byte
        # ranges that leave a fragment out.
        self.assertIsNone(drift(index=(2,) + INDEX[1:]))
        self.assertIsNone(drift(index=(1, 1000, [(100, 200), (200, 400)], 1000)))
        self.assertIsNone(drift(index=(1, 1000, INDEX[2], 1050)))
        self.assertIsNone(drift(index=None))
        # Samples in the sample table.
        self.assertIsNone(drift(in_table=2))

    def test_check_starts_covers_only_with_the_drift(self):
        src = [{'pts': Fraction(t, 1000)} for t in (0, 100, 350, 450, 700, 800)]
        for found, covered in ((None, False), ([0, Fraction(15, 100), Fraction(3, 10)], True)):
            report = mock.Mock()
            sizes = iter([2, 2, 2] * 2)
            with mock.patch.object(hc, 'init_edit_window', return_value=None), \
                    mock.patch.object(hc, 'trun_samples', side_effect=lambda _: [None] * next(sizes)), \
                    mock.patch.object(hc, 'kd_index_drift', return_value=found):
                hc.check_starts(report, 'audio', SEGMENTS, STARTS, PACKAGE, src, Fraction(1, 10), 'init.mp4', 1000,
                                Fraction(0), 'source.mp4', 1)
            self.assertEqual(report.fail.called, not covered)
            self.assertEqual(report.covered.called, covered)
            if covered:
                self.assertEqual(report.covered.call_args[0][0], 'KD-HLS-10')


class DecodeGaps(unittest.TestCase):
    def test_the_last_sample_before_a_decode_time_gap(self):
        with mock.patch.object(hc, 'source_fragments', return_value=(1000, 0, RUNS)):
            self.assertEqual(hc.source_decode_gaps('source.mp4', 1), {1: 100, 3: 100})
        continuous = [(0, 0, [100, 100], [0, 0]), (1, 200, [100, 90], [0, 0]), (2, None, [100], [0])]
        with mock.patch.object(hc, 'source_fragments', return_value=(1000, 3, continuous)):
            self.assertEqual(hc.source_decode_gaps('source.mp4', 1), {})
        with mock.patch.object(hc, 'source_fragments', return_value=None):
            self.assertEqual(hc.source_decode_gaps('source.mp4', 1), {})


class LeftOutFrames(unittest.TestCase):
    """An AV1 source of the writer corpus whose last sample (decode duration 0) lies at the
    edit's end; ffmpeg decodes its frame from the source, the rendition
    leaves the sample out."""

    def test_frames_at_the_left_out_packets_times(self):
        end = Fraction(2417664, 12288)
        left_out = [{'pts': end, 'discard': True}]
        self.assertTrue(hc.kd_left_out_frames([(end, 'h')], left_out))
        # A frame elsewhere, a second frame at that time, or none left out.
        self.assertFalse(hc.kd_left_out_frames([(end - Fraction(512, 12288), 'h')], left_out))
        self.assertFalse(hc.kd_left_out_frames([(end, 'h'), (end, 'h')], left_out))
        self.assertFalse(hc.kd_left_out_frames([(end, 'h')], []))
        self.assertFalse(hc.kd_left_out_frames([], left_out))


class Twin(unittest.TestCase):
    def test_the_twin_lists_the_predicates(self):
        hc.check_twin()


if __name__ == '__main__':
    unittest.main()
