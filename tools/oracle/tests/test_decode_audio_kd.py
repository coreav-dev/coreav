"""Keeps docs/known-divergences-decode-audio.md, its JSON twin and the
predicates of scripts/decode_audio_check.py in sync, and checks the
presentation model, that each entry covers only what it says, and that the
mutation audit finds an entry that would cover a bug ("Edit list bars" of
docs/design/audio-decode.md)."""
import importlib.util
import io
import json
import pathlib
import re
import struct
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('decode_audio_check', ROOT / 'scripts' / 'decode_audio_check.py')
dc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(dc)
ENTRIES = dc.known_divergences()
UNIT = dc.UNIT_RATE
PACKET = 100
TOTAL = 1000


def samples(values):
    return b''.join(struct.pack('<i', v) for v in values)


def ramp(a, b):
    """The ramp's samples a..b: sample n has the value n + 1, so that no
    media sample is silence."""
    return samples(range(a + 1, b + 1))


def reference(edits, ffmpeg, movie_timescale=1000, rate=1000):
    """A lossless mono track of TOTAL samples at `rate` in packets of PACKET
    samples, media timescale `rate`, with these raw edits."""
    plan = dc.pieces(edits, 0, rate, rate, movie_timescale)
    return {'plan': plan, 'full': ramp(0, TOTAL), 'frame': 4, 'lossy': False, 'ffmpeg': ffmpeg,
            'edges': list(range(0, TOTAL + 1, PACKET)), 'edits': edits, 'first_dts': 0, 'rate': rate,
            'timescale': rate, 'movie_timescale': movie_timescale, 'fragmented': False, 'total': TOTAL,
            'ffmpeg_failed': False}


def coreav(ref):
    """CoreAV's output and summary timeline for the reference: the model's."""
    return dc.model(ref['full'], ref['frame'], ref['plan']), dc.timeline(ref['plan'], ref['total'])


def evaluate(out, timeline, ref):
    return dc.verdicts(dc.in_memory(out, ref['full'], ref['ffmpeg'], ref), ref, ENTRIES, timeline)


def audit(out, timeline, ref):
    return dc.audit(out, timeline, ref['full'], ref['ffmpeg'], ref, ENTRIES)


class DocumentsInSync(unittest.TestCase):
    def test_ids_predicates_and_dates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-decode-audio.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-decode-audio.md').read_text()
        documented = set(re.findall(r'^## (KD-A-[A-Z-]+)$', page, re.M))
        ids = {entry['id'] for entry in twin['entries']}
        self.assertEqual(documented, ids)
        self.assertEqual(ids, set(dc.ADDED_AFTER_THE_BARS))
        self.assertEqual({e['predicate'] for e in twin['entries']}, set(dc.PREDICATES))
        self.assertEqual({k for kinds in dc.APPLY.values() for k in kinds}, ids)
        for entry in twin['entries']:
            self.assertTrue(entry['basis'])
            self.assertIn('raw edit list', entry['conditions'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')


class Model(unittest.TestCase):
    def test_boundaries_are_exact_sums_rounded_once(self):
        # 1/3 s three times at 1,000 samples a second: 333, 334, 333.
        edits = [(1, 0, UNIT), (1, 400, UNIT), (1, 800, UNIT)]
        plan = dc.pieces(edits, 0, 1000, 1000, 3)
        self.assertEqual(plan, [('media', 0, 333), ('media', 400, 734), ('media', 800, 1133)])
        # Halves round up.
        self.assertEqual(dc.to_samples(1, 1, 2), 1)
        self.assertEqual(dc.to_samples(3, 1, 2), 2)

    def test_silence_and_windows(self):
        ref = reference([(50, -1, UNIT), (200, 100, UNIT), (30, -1, UNIT), (100, 500, UNIT)], b'')
        out, timeline = coreav(ref)
        self.assertEqual(out, bytes(200) + ramp(100, 300) + bytes(120) + ramp(500, 600))
        self.assertEqual(timeline, {'start': 50, 'silence': [{'at': 0, 'samples': 50}, {'at': 250, 'samples': 30}]})
        # Windows past the media's end are cut there.
        self.assertEqual(dc.effective([('media', 900, 1200), ('media', 1300, 1400)], TOTAL),
                         [('media', 900, 1000), ('media', 1000, 1000)])

    def test_applied_shapes(self):
        applies = dc.applies
        self.assertTrue(applies([(10, 0, UNIT)], 1000, 1000, False))
        self.assertTrue(applies([(5, -1, UNIT), (10, 0, UNIT)], 1000, 1000, False))
        # A duration of 0 lasts to the end only with fragments.
        self.assertFalse(applies([(0, 0, UNIT)], 1000, 1000, False))
        self.assertTrue(applies([(0, 0, UNIT)], 1000, 1000, True))
        # Other rates, overlapping windows, dwells and negative times are not
        # applied; several edits not with fragments.
        self.assertFalse(applies([(10, 0, 2 * UNIT)], 1000, 1000, False))
        self.assertFalse(applies([(10, 0, UNIT), (10, 5, UNIT)], 1000, 1000, False))
        self.assertTrue(applies([(10, 0, UNIT), (10, 10, UNIT)], 1000, 1000, False))
        self.assertFalse(applies([(10, 0, UNIT), (10, 10, UNIT)], 1000, 1000, True))
        self.assertFalse(applies([(10, 0, UNIT), (0, 10, UNIT)], 1000, 1000, False))
        self.assertFalse(applies([(10, -2, UNIT)], 1000, 1000, False))
        self.assertFalse(applies([(5, -1, UNIT)], 1000, 1000, False))
        self.assertFalse(applies([], 1000, 1000, False))
        self.assertIsNone(dc.pieces([(10, 0, 2 * UNIT)], 0, 1000, 1000, 1000))


class Entries(unittest.TestCase):
    def verdict(self, edits, ffmpeg):
        ref = reference(edits, ffmpeg)
        out, timeline = coreav(ref)
        return evaluate(out, timeline, ref)

    def test_leading_empty(self):
        edits = [(50, -1, UNIT), (300, 0, UNIT)]
        r = self.verdict(edits, ramp(0, 300))
        self.assertEqual((r['model'], r['timeline'], r['ffmpeg'], r['entries']),
                         (True, True, 'covered', ['KD-A-LEADING-EMPTY']))
        # ffmpeg writing anything else is not covered.
        self.assertEqual(self.verdict(edits, ramp(0, 299))['ffmpeg'], 'uncovered')
        self.assertEqual(self.verdict(edits, bytes(40) + ramp(0, 300))['ffmpeg'], 'uncovered')

    def test_edit_end(self):
        edits = [(250, 0, UNIT)]
        r = self.verdict(edits, ramp(0, 300))
        self.assertEqual((r['ffmpeg'], r['entries']), ('covered', ['KD-A-EDIT-END']))
        # The rest of the packet, not more and not less; the end of the
        # media is not extended.
        self.assertEqual(self.verdict(edits, ramp(0, 301))['ffmpeg'], 'uncovered')
        self.assertEqual(self.verdict(edits, ramp(0, 299))['ffmpeg'], 'uncovered')
        self.assertEqual(self.verdict([(300, 0, UNIT)], ramp(0, 400))['ffmpeg'], 'uncovered')
        self.assertEqual(self.verdict([(300, 0, UNIT)], ramp(0, 300))['ffmpeg'], 'equal')

    def test_later_edit_start(self):
        edits = [(200, 0, UNIT), (200, 450, UNIT)]
        r = self.verdict(edits, ramp(0, 200) + ramp(500, 700))
        self.assertEqual((r['ffmpeg'], r['entries']), ('covered', ['KD-A-EDIT-END', 'KD-A-LATER-EDIT-START']))
        # The first edit keeps its exact start.
        self.assertEqual(self.verdict([(200, 50, UNIT)], ramp(100, 300))['ffmpeg'], 'uncovered')

    def test_inner_empty(self):
        edits = [(200, 0, UNIT), (150, -1, UNIT), (200, 500, UNIT)]
        r = self.verdict(edits, ramp(0, 200) + ramp(0, 200) + ramp(500, 700))
        self.assertEqual((r['ffmpeg'], r['entries']), ('covered', ['KD-A-INNER-EMPTY']))
        # Silence in its place, or a fill of another length, is not covered.
        self.assertEqual(self.verdict(edits, ramp(0, 200) + bytes(600) + ramp(500, 700))['ffmpeg'], 'equal')
        self.assertEqual(self.verdict(edits, ramp(0, 200) + ramp(0, 150) + ramp(500, 700))['ffmpeg'],
                         'uncovered')

    def test_entries_check_what_they_drop(self):
        ref = reference([(50, -1, UNIT), (200, 0, UNIT), (150, -1, UNIT), (200, 450, UNIT)],
                        ramp(0, 200) + ramp(0, 200) + ramp(500, 700))
        out, timeline = coreav(ref)
        self.assertEqual(evaluate(out, timeline, ref)['entries'],
                         ['KD-A-EDIT-END', 'KD-A-INNER-EMPTY', 'KD-A-LATER-EDIT-START', 'KD-A-LEADING-EMPTY'])
        # A sample changed in the leading silence, in the inner silence or in
        # the samples a later edit's start drops.
        for at in (10, 300, 50 + 200 + 150 + 10):
            bad = out[:4 * at] + samples([7]) + out[4 * at + 4:]
            self.assertEqual(evaluate(bad, timeline, ref)['ffmpeg'], 'uncovered', at)
        # A list CoreAV does not apply has no entries.
        ref = reference([(100, 0, 2 * UNIT)], ramp(0, 50))
        self.assertEqual(evaluate(ref['full'], {'start': 0, 'silence': []}, ref)['ffmpeg'], 'uncovered')

    def test_lossy_fill_first_packet_in_length_only(self):
        edits = [(200, 0, UNIT), (150, -1, UNIT), (200, 500, UNIT)]
        ref = reference(edits, None)
        floats = lambda a, b: b''.join(struct.pack('<f', (n + 1) / 4096) for n in range(a, b))
        ref['full'], ref['lossy'] = floats(0, TOTAL), True
        out, timeline = coreav(ref)
        first = b''.join(struct.pack('<f', 0.5) for _ in range(PACKET))
        ref['ffmpeg'] = floats(0, 200) + first + floats(PACKET, 200) + floats(500, 700)
        self.assertEqual(evaluate(out, timeline, ref)['ffmpeg'], 'covered')
        # The second packet of the fill is compared.
        ref['ffmpeg'] = floats(0, 200) + first + first + floats(500, 700)
        self.assertEqual(evaluate(out, timeline, ref)['ffmpeg'], 'uncovered')


class Audit(unittest.TestCase):
    EDITS = [(50, -1, UNIT), (200, 0, UNIT), (150, -1, UNIT), (200, 450, UNIT)]
    FFMPEG = ramp(0, 200) + ramp(0, 200) + ramp(500, 700)

    def test_every_mutation_caught_and_none_covered(self):
        ref = reference(self.EDITS, self.FFMPEG)
        out, timeline = coreav(ref)
        verdicts = audit(out, timeline, ref)
        self.assertGreater(len(verdicts), 20)
        self.assertEqual({v for v in verdicts.values()}, {'caught'})
        self.assertIn('the leading silence dropped, as ffmpeg drops it', verdicts)

    def test_a_loose_entry_is_found(self):
        # An entry that dropped samples without checking them would cover a
        # bug there: the audit must say so.
        def loose(piece, ops, ctx):
            moved = dc.later_edit_start(piece, ops, ctx)
            return (moved[0], []) if moved else None
        ref = reference(self.EDITS, self.FFMPEG)
        out, timeline = coreav(ref)
        with mock.patch.dict(dc.PREDICATES, {'later_edit_start': loose}):
            verdicts = audit(out, timeline, ref)
        self.assertIn('COVERED', verdicts.values())

    def test_lossy_mutations(self):
        floats = b''.join(struct.pack('<f', v) for v in (0.0, 0.5, -0.5, 1e-6))
        ref = {'frame': 4, 'lossy': True}
        for at in range(4):
            self.assertFalse(dc.same(dc.flipped(floats, at, ref), floats, True), at)
        # Within the tolerance: equal for the comparison.
        quiet = b''.join(struct.pack('<f', 1e-6 * k) for k in range(100))
        self.assertTrue(dc.same(quiet[4:] + quiet[-4:], quiet, True))
        self.assertFalse(dc.same(quiet[4:], quiet, True))

    def test_summary_mutations_are_caught_by_the_timeline(self):
        ref = reference(self.EDITS, self.FFMPEG)
        out, timeline = coreav(ref)
        r = evaluate(out, {**timeline, 'start': timeline['start'] + 1}, ref)
        self.assertEqual((r['model'], r['timeline'], r['ffmpeg']), (True, False, 'covered'))
        self.assertFalse(dc.passed(r))


class Streaming(unittest.TestCase):
    def test_small_chunks_give_the_same_verdicts(self):
        # Reads of a few samples at a time: the full decode is read behind
        # its furthest read (an edit end's packet, then the next window) and
        # from its start (an inner empty edit's fill).
        cases = [([(50, -1, UNIT), (200, 0, UNIT), (150, -1, UNIT), (200, 450, UNIT)],
                  ramp(0, 200) + ramp(0, 200) + ramp(500, 700)),
                 ([(150, 0, UNIT), (200, 150, UNIT)], ramp(0, 200) + ramp(200, 400)),
                 ([(250, 0, UNIT)], ramp(0, 300))]
        for edits, ffmpeg in cases:
            ref = reference(edits, ffmpeg)
            out, timeline = coreav(ref)
            whole = evaluate(out, timeline, ref)
            for chunk in (4, 12, 40, 400):
                with mock.patch.object(dc, 'CHUNK', chunk):
                    self.assertEqual(evaluate(out, timeline, ref), whole, (edits, chunk))
                    self.assertEqual(set(audit(out, timeline, ref).values()), {'caught'})
            self.assertTrue(dc.passed(whole), edits)

    def test_source_reads(self):
        with mock.patch.object(dc, 'CHUNK', 3):
            source = dc.Source(io.BytesIO(bytes(range(40))), keep=5, prefix=4)
            self.assertEqual(source.take(10, 4), bytes(range(10, 14)))
            self.assertEqual(source.take(9, 2), bytes([9, 10]))
            self.assertEqual(source.take(0, 4), bytes(range(4)))
            self.assertEqual(source.take(30, 20), bytes(range(30, 40)))
            with self.assertRaises(ValueError):
                source.take(10, 2)
            self.assertTrue(source.ends_at(40))
            self.assertFalse(dc.Source(io.BytesIO(bytes(4))).ends_at(3))


class Wav(unittest.TestCase):
    def test_data_chunk(self):
        fmt = b'fmt ' + struct.pack('<I', 16) + bytes(16)
        streamed = b'RIFF' + b'\xff' * 4 + b'WAVE' + fmt + b'data' + b'\xff' * 4 + b'abcd'
        self.assertEqual(dc.wav_data(streamed), b'abcd')
        sized = b'RIFF' + bytes(4) + b'WAVE' + fmt + b'data' + struct.pack('<I', 2) + b'abcd'
        self.assertEqual(dc.wav_data(sized), b'ab')
        self.assertIsNone(dc.wav_data(b'RIFF....AVI '))


if __name__ == '__main__':
    unittest.main()
