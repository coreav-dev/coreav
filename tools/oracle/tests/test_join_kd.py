"""Keeps docs/known-divergences-join.md, its JSON twin and the predicates of
scripts/join_check.py in sync, and checks that the predicates reject near
misses (docs/design/join.md)."""
import copy
import importlib.util
import json
import pathlib
import re
import sys
import unittest
from unittest import mock
from fractions import Fraction

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts'))
SPEC = importlib.util.spec_from_file_location('join_check', ROOT / 'scripts' / 'join_check.py')
jc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(jc)


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-join.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-join.md').read_text()
        documented = set(re.findall(r'^## (KD-J-[A-Z-]+)$', page, re.M))
        self.assertEqual(documented, {entry['id'] for entry in twin['entries']})
        self.assertEqual({entry['predicate'] for entry in twin['entries']}, set(jc.PREDICATES))
        for entry in twin['entries']:
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')


def frames(times):
    return [(Fraction(t), 100, f'h{n}') for n, t in enumerate(times)]


class Listing(unittest.TestCase):
    def test_copies_at_one_position_are_dropped(self):
        packets = [
            {'pos': '10', 'flags': 'K__', 'pts': 0},
            {'pos': '20', 'flags': 'KD_', 'pts': 9},
            {'pos': '20', 'flags': 'K__', 'pts': 5},
            {'pos': '30', 'flags': '_D_', 'pts': 6},
        ]
        listed = jc.listed(packets)
        self.assertEqual([p['pos'] for p in listed], ['10', '20', '30'])
        self.assertEqual(listed[1]['pts'], 5)


class LaterEdit(unittest.TestCase):
    def evidence(self, expected, actual, ends, delays):
        kept = [{'start': Fraction(0), 'media': Fraction(0), 'gap': None},
                {'start': Fraction(10), 'media': Fraction(10), 'gap': None}]
        return {'video_frames': (expected, actual), 'video_input_ends': ends, 'video_delays': delays,
                'video_edits': {'output': [(e['start'], e['media']) for e in kept], 'inputs': kept}}

    def test_a_constant_shift_within_the_delay(self):
        expected = frames([0, 1, 2, 3, 4])
        later = [(t + 1 if i >= 2 else t, s, h) for i, (t, s, h) in enumerate(expected)]
        c = self.evidence(expected, later, [2, 5], [0, Fraction(2)])
        self.assertTrue(jc.video_later_edit('video:framemd5', {'video:framemd5'}, c))
        # More than the delay.
        too_late = [(t + 3 if i >= 2 else t, s, h) for i, (t, s, h) in enumerate(expected)]
        self.assertFalse(jc.video_later_edit('video:framemd5', set(), self.evidence(expected, too_late, [2, 5],
                                                                                         [0, Fraction(2)])))
        # The output's later edit other than the input's own.
        c = self.evidence(expected, later, [2, 5], [0, Fraction(2)])
        c['video_edits']['output'][1] = (Fraction(10), Fraction(21, 2))
        self.assertFalse(jc.video_later_edit('video:framemd5', set(), c))
        # The first input shifted.
        first = [(t + 1, s, h) for t, s, h in expected]
        self.assertFalse(jc.video_later_edit('video:framemd5', set(), self.evidence(expected, first, [2, 5],
                                                                                      [0, Fraction(2)])))
        # Frames out of order.
        swapped = list(later)
        swapped[2], swapped[3] = swapped[3], swapped[2]
        self.assertFalse(jc.video_later_edit('video:framemd5', set(), self.evidence(expected, swapped, [2, 5],
                                                                                      [0, Fraction(2)])))

    def test_an_early_shift_within_the_first_delay_clamped_to_the_previous_frame(self):
        # The second input shown 1.5 early (the first input's delay is 2):
        # its first frame would come before the previous frame, and is shown
        # at that frame's time.
        expected = frames([0, 1, 2, 3, 4, 5])
        early = [(t if i < 3 else max(t - Fraction(3, 2), Fraction(2)), s, h)
                 for i, (t, s, h) in enumerate(expected)]
        self.assertEqual([t for t, _, _ in early], [0, 1, 2, 2, Fraction(5, 2), Fraction(7, 2)])
        self.assertTrue(jc.video_later_edit('video:framemd5', set(),
                                            self.evidence(expected, early, [3, 6], [Fraction(2), Fraction(1)])))
        # More than the first input's delay early.
        self.assertFalse(jc.video_later_edit('video:framemd5', set(),
                                             self.evidence(expected, early, [3, 6], [Fraction(1), Fraction(1)])))
        # A frame held at the previous one's time that the shift does not
        # put before it.
        held = list(early)
        held[5] = (held[4][0], held[5][1], held[5][2])
        self.assertFalse(jc.video_later_edit('video:framemd5', set(),
                                             self.evidence(expected, held, [3, 6], [Fraction(2), Fraction(1)])))


class AudioLastCut(unittest.TestCase):
    """fragmented.mp4's segments joined: ffmpeg 9 cuts the last AAC frame of
    the joined file (136 of 1,024 samples) and not of the source."""

    FRAMES = [(Fraction(n * 1024, 44100), 2048, f'h{n}') for n in range(3)]

    def evidence(self, actual_last=(Fraction(2048, 44100), 272, 'cut'), short=136, sides=(None, (0, 888)), source=1024):
        actual = self.FRAMES[:-1] + [actual_last]
        c = {'ffprobe': 'ffprobe',
             'source_audio_last': (self.FRAMES, actual, [1024, 1024, source], [1024, 1024, short],
                                   ('source.mp4', 1), ('joined.mp4', 1))}
        files = dict(zip(('source.mp4', 'joined.mp4'), sides))
        with mock.patch.object(jc.remux_check, 'last_skip_samples', side_effect=lambda _, path, __: files[path]):
            return jc.audio_last_cut('audio:framemd5', {'audio:framemd5'}, c)

    def test_the_last_frame_cut_in_one_file(self):
        self.assertTrue(self.evidence())
        self.assertTrue(self.evidence(short=1024, source=136, sides=((0, 888), None),
                                      actual_last=(Fraction(2048, 44100), 2048, 'whole')))

    def test_rejects_near_misses(self):
        self.assertFalse(self.evidence(sides=(None, None)))
        self.assertFalse(self.evidence(sides=((0, 888), (0, 888))))
        self.assertFalse(self.evidence(sides=(None, (0, 887))))
        self.assertFalse(self.evidence(sides=(None, (1, 888))))
        self.assertFalse(self.evidence(actual_last=(Fraction(2049, 44100), 272, 'cut')))
        c = {'ffprobe': 'ffprobe', 'source_audio_last': (self.FRAMES, self.FRAMES[:1] + [(0, 0, 'x')] + self.FRAMES[2:],
                                                         [1024] * 3, [1024] * 3, ('s', 1), ('j', 1))}
        self.assertFalse(jc.audio_last_cut('audio:framemd5', {'audio:framemd5'}, c))
        self.assertFalse(jc.audio_last_cut('audio:framemd5', {'audio:framemd5', 'audio:packets'}, c))


class EditEnd(unittest.TestCase):
    def test_the_next_first_frame_after_the_cut_frame(self):
        expected = [(Fraction(0), 1, 'a'), (Fraction(2), 1, 'b'), (Fraction(3), 1, 'c'), (Fraction(5), 1, 'd')]
        # The first input's frames last 2; its edit ends at 3, inside 'b'.
        moved = [(Fraction(0), 1, 'a'), (Fraction(2), 1, 'b'), (Fraction(4), 1, 'c'), (Fraction(5), 1, 'd')]
        c = {'video_frames': (expected, moved), 'video_input_ends': [2, 4]}
        self.assertTrue(jc.video_edit_end('video:framemd5', set(), c))
        twice = moved[:2] + [moved[2]] + moved[2:]
        c['video_frames'] = (expected, twice)
        self.assertTrue(jc.video_edit_end('video:framemd5', set(), c))
        other = moved[:2] + [(Fraction(4), 1, 'x')] + moved[3:]
        c['video_frames'] = (expected, other)
        self.assertFalse(jc.video_edit_end('video:framemd5', set(), c))
        self.assertFalse(jc.video_edit_end('video:framemd5', {'video:packets'}, c))


def packet(pos, pts, dts, duration, key=True):
    return {'pos': str(pos), 'pts': pts, 'dts': dts, 'duration': duration, 'size': 100 + pos % 7,
            'data_hash': f'h{pos}', 'flags': 'K__' if key else '___'}


def stream(first_pos, count, frame, delay=0):
    """`count` packets of `frame` ticks, decode times from 0, presentation
    times `delay` later; a keyframe every fourth."""
    return [packet(first_pos + i, i * frame + delay, i * frame, frame, i % 4 == 0) for i in range(count)]


def moved(packets, ticks, pts=0, dts=0):
    """The packets as `ffprobe` lists them in the output: times plus the
    input's offset `ticks`, minus `pts` and `dts` more (a positive offset in
    the evidence means the output is early)."""
    return [dict(p, pos=str(int(p['pos']) + 1000), pts=p['pts'] + ticks - pts, dts=p['dts'] + ticks - dts)
            for p in packets]


class Scenario:
    """A join of two inputs as the packet comparison sees it: each input's
    listing, its offset, the output's listing, and the headers the packet
    predicates read."""

    def __init__(self, kind, base, inputs, ticks, output, ctts=None, delays=None, edits=2, gap=None):
        self.kind, self.base, self.inputs, self.ticks, self.output = kind, base, inputs, ticks, output
        self.ctts, self.delays, self.edits = ctts, delays, edits
        # Each input's own edit as the output should hold it (from its media
        # time 1,024 ticks in), and, for audio, the distance from that media
        # time to the next packet start; the output's edits, as written.
        self.kept = [{'start': t * base, 'media': (t + 1024) * base, 'gap': gap} for t in ticks]
        self.written = [(e['start'], e['media']) for e in self.kept]

    def judge(self, output=None, ctts=None, written=None):
        """The uncovered and the covered differences, as `check` sorts
        them: [(field)], [(field, kd)]."""
        found, counts, evidence = [], {'agree': 0, 'disagree': 0}, {}
        jc.compare_packets(self.kind, self.inputs, [t * self.base for t in self.ticks],
                           self.output if output is None else output, self.base, counts, found, evidence)
        elst = [[1, 0]] + [[1, 1024]] * (self.edits - 1)
        traks = [{'elst': elst}, {'elst': elst}]
        evidence['output_headers'] = {'traks': traks}
        evidence[f'{self.kind}_edits'] = {'output': self.written if written is None else written,
                                          'inputs': self.kept}
        if self.kind == 'video':
            evidence['video_ctts_min'] = ctts or self.ctts
            evidence['video_delays'] = self.delays
        fields = {field for field, _ in found}
        uncovered, covered = [], []
        for field in sorted(fields):
            kd = jc.covering(field, fields, evidence)
            if kd:
                covered.append((field, kd))
            else:
                uncovered.append(field)
        return uncovered, covered


def video_later(delay_cancels=False):
    """Two clips of 12,800-tick video, 512 ticks a frame, with B-frame delays
    of 1,024 ticks (0.08 s) and 2,048; `ffmpeg` shows the second's frames
    1,024 ticks late. With `delay_cancels`, the first input brings a smallest
    composition offset of -1,024 ticks: `ffprobe` moves the output's decode
    times back by 1,024 and the second input's not at all, so its decode
    times agree (negative_cts.mp4 followed by timecode.mov)."""
    base = Fraction(1, 12800)
    first, second = stream(0, 8, 512, 1024), stream(100, 8, 512, 2048)
    ticks = [0, 8 * 512]
    out = moved(first, 0) + moved(second, ticks[1], pts=-1024, dts=0 if delay_cancels else -1024)
    ctts = ((-1024, 12800), [(-1024, 12800), (0, 12800)]) if delay_cancels else \
        ((0, 12800), [(0, 12800), (0, 12800)])
    return Scenario('video', base, [first, second], ticks, out, ctts, [Fraction(2, 25), Fraction(4, 25)])


def audio_later():
    """Two clips of 44.1 kHz AAC; the second's edit starts 68 samples into a
    frame, and `ffmpeg` lists its packets 956 ticks early."""
    base = Fraction(1, 44100)
    first, second = stream(0, 6, 1024), stream(100, 6, 1024)
    ticks = [0, 6 * 1024]
    return Scenario('audio', base, [first, second], ticks, moved(first, 0) + moved(second, ticks[1], 956, 956),
                    gap=956 * base)


def dts_shifted():
    """chapters.mov (offsets of 0 or more) then negative_cts.mp4 (down to
    -1,024 ticks): every decode time of the first input 1,024 ticks lower."""
    base = Fraction(1, 12800)
    first, second = stream(0, 8, 512, 1024), stream(100, 8, 512, -1024)
    ticks = [0, 8 * 512]
    out = moved(first, 0, dts=1024) + moved(second, ticks[1])
    return Scenario('video', base, [first, second], ticks, out, ((-1024, 12800), [(0, 12800), (-1024, 12800)]),
                    [Fraction(2, 25), Fraction(0)])


def exact():
    base = Fraction(1, 12800)
    first, second = stream(0, 8, 512, 1024), stream(100, 8, 512, 1024)
    ticks = [0, 8 * 512]
    return Scenario('video', base, [first, second], ticks, moved(first, 0) + moved(second, ticks[1]),
                    ((0, 12800), [(0, 12800), (0, 12800)]), [Fraction(2, 25), Fraction(2, 25)])


SCENARIOS = {
    'video later edit': (video_later, 'KD-J-VIDEO-LATER-EDIT'),
    'video later edit, decode delay cancelled': (lambda: video_later(True), 'KD-J-VIDEO-LATER-EDIT'),
    'audio edit start': (audio_later, 'KD-J-AUDIO-EDIT-START'),
    'decode times shifted': (dts_shifted, 'KD-J-DTS-SHIFT'),
    'exact': (exact, None),
}


def with_packet(packets, index, **change):
    out = copy.deepcopy(packets)
    out[index].update(change)
    return out


def output_mutations(packets, half):
    """Bug-shaped changes to the output's listing: (label, packets). `half`
    is the index of the second input's first packet."""
    last = packets[-1]
    yield 'final packet omitted', packets[:-1]
    yield 'a packet omitted', packets[:3] + packets[4:]
    yield 'a packet of no input', packets + [dict(last, pos='9999', pts=last['pts'] + 512, dts=last['dts'] + 512)]
    yield 'a packet listed twice', packets[:half] + [packets[half]] + packets[half:]
    for at in (1, half + 1, len(packets) - 1):
        p = packets[at]
        yield f'packet {at}: dts far off', with_packet(packets, at, dts=p['dts'] + 1_000_000)
        yield f'packet {at}: dts a tick off', with_packet(packets, at, dts=p['dts'] + 1)
        yield f'packet {at}: pts far off', with_packet(packets, at, pts=p['pts'] - 1_000_000)
        yield f'packet {at}: pts a tick off', with_packet(packets, at, pts=p['pts'] + 1)
        yield f'packet {at}: pts and dts a tick off', with_packet(packets, at, pts=p['pts'] + 1, dts=p['dts'] + 1)
        yield f'packet {at}: other data', with_packet(packets, at, data_hash='other')
        yield f'packet {at}: other size', with_packet(packets, at, size=p['size'] + 1)
        yield f'packet {at}: keyframe flag flipped', with_packet(packets, at, flags='___' if p['flags'][0] == 'K'
                                                                 else 'K__')
    yield 'first input shifted', moved(packets[:half], -1000, 1, 1) + packets[half:]
    yield 'second input shifted a frame more', packets[:half] + [
        dict(p, pts=p['pts'] + 5000, dts=p['dts'] + 5000) for p in packets[half:]]
    yield 'second input half shifted', packets[:half] + [
        dict(p, pts=p['pts'] + 3, dts=p['dts'] + 3) if i % 2 else p for i, p in enumerate(packets[half:])]
    yield 'second input decode times only shifted', packets[:half] + [
        dict(p, dts=p['dts'] + 300) for p in packets[half:]]


class PacketMutations(unittest.TestCase):
    """Each packet known divergence covers its pattern, and a bug-shaped
    change to the output's packets on top of it stays visible: an
    uncovered difference (the known-divergence audit's method, on the packet evidence)."""

    def test_each_pattern_is_covered(self):
        for name, (make, kd) in SCENARIOS.items():
            uncovered, covered = make().judge()
            self.assertEqual(uncovered, [], name)
            self.assertEqual({k for _, k in covered}, {kd} if kd else set(), name)

    def test_bug_shaped_changes_stay_visible(self):
        for name, (make, _) in SCENARIOS.items():
            scenario = make()
            half = len(scenario.inputs[0])
            baseline = scenario.judge()
            for label, packets in output_mutations(scenario.output, half):
                uncovered, covered = scenario.judge(packets)
                if scenario.kind == 'audio' and label.startswith('packet 1:') and 'tick' in label:
                    # Within the one tick audio times are compared to:
                    # nothing the comparison gates on changes.
                    self.assertEqual((uncovered, covered), baseline, label)
                    continue
                self.assertTrue(uncovered, f'{name}: {label} covered by {covered}')

    def test_the_output_smallest_offset_alone(self):
        # A smallest offset in the output's header other than the inputs'
        # (the value the decode-time terms come from, moved alone).
        for name in ('video later edit, decode delay cancelled', 'decode times shifted'):
            scenario = SCENARIOS[name][0]()
            (out_min, scale), inputs = scenario.ctts
            for wrong in (out_min - 512, 0):
                uncovered, _ = scenario.judge(ctts=((wrong, scale), inputs))
                self.assertTrue(uncovered, f'{name}: output offset {wrong}')
                # Moved together with what it makes `ffprobe` do: every
                # output decode time moved back by the new offset instead.
                further = out_min - wrong
                packets = [dict(p, dts=p['dts'] - further) for p in scenario.output]
                uncovered, covered = scenario.judge(packets, ctts=((wrong, scale), inputs))
                self.assertTrue(uncovered, f'{name}: output offset {wrong} with its decode times, {covered}')

    def test_one_edit(self):
        # Later edits are what the edit known divergences need.
        for name in ('video later edit', 'audio edit start'):
            scenario = SCENARIOS[name][0]()
            scenario.edits = 1
            uncovered, _ = scenario.judge()
            self.assertTrue(uncovered, name)

    def test_a_shift_past_the_bounds(self):
        # Video: later than the second input's delay (2,048 ticks), or
        # earlier than the first's (1,024). Audio: a frame or more, or none.
        scenario = video_later()
        half = len(scenario.inputs[0])
        for extra in (2048 - 1024 + 1, -(1024 + 1024 + 1)):
            packets = scenario.output[:half] + [dict(p, pts=p['pts'] + extra, dts=p['dts'] + extra)
                                                for p in scenario.output[half:]]
            self.assertTrue(scenario.judge(packets)[0], extra)
        scenario = audio_later()
        for extra in (956, -68 - 1):
            packets = scenario.output[:half] + [dict(p, pts=p['pts'] - extra, dts=p['dts'] - extra)
                                                for p in scenario.output[half:]]
            self.assertTrue(scenario.judge(packets)[0], extra)

    def test_frames_of_2048_ticks(self):
        # SBR: frames of 2,048 ticks, an offset up to one frame.
        base = Fraction(1, 44100)
        first, second = stream(0, 6, 2048), stream(100, 6, 2048)
        ticks = [0, 6 * 2048]
        scenario = Scenario('audio', base, [first, second], ticks, moved(first, 0) + moved(second, ticks[1], 1412, 1412),
                            gap=1412 * base)
        self.assertEqual(scenario.judge()[0], [])
        scenario.output = moved(first, 0) + moved(second, ticks[1], 2048, 2048)
        self.assertTrue(scenario.judge()[0])

    def test_the_edits_themselves(self):
        # The output's later edit moved alone (a media time rounded to a
        # frame boundary, a video edit a fraction of a frame off, an edit
        # started elsewhere), or an audio edit that starts on a packet
        # boundary, where nothing is dropped: never covered.
        for name in ('video later edit', 'video later edit, decode delay cancelled', 'audio edit start'):
            scenario = SCENARIOS[name][0]()
            start, media = scenario.written[1]
            for moved_edit in ((start, media + 68 * scenario.base), (start, media - 256 * scenario.base),
                               (start + scenario.base, media)):
                uncovered, covered = scenario.judge(written=[scenario.written[0], moved_edit])
                self.assertTrue(uncovered, f'{name}: {moved_edit} covered by {covered}')
        scenario = audio_later()
        scenario.kept[1]['gap'] = None
        self.assertTrue(scenario.judge()[0])
        scenario.kept[1]['gap'] = 955 * scenario.base
        self.assertTrue(scenario.judge()[0])

    def test_an_offset_between_ticks(self):
        # The second input's offset falls 0.2 tick past a tick of 48 kHz: its
        # times are off by 1,017.2 ticks for a gap of 1,017 (corpus
        # 061589fae6b7), within less than a tick.
        base = Fraction(1, 48000)
        first, second = stream(0, 6, 1024), stream(100, 6, 1024)
        offset = 6 * 1024 + Fraction(1, 5)
        out = moved(first, 0) + [dict(p, pos=str(int(p['pos']) + 1000), pts=p['pts'] + 6 * 1024 - 1017,
                                      dts=p['dts'] + 6 * 1024 - 1017) for p in second]
        scenario = Scenario('audio', base, [first, second], [0, offset], out, gap=1017 * base)
        self.assertEqual(scenario.judge(), ([], [('audio:packets', 'KD-J-AUDIO-EDIT-START')]))
        scenario.kept[1]['gap'] = 1016 * base
        self.assertTrue(scenario.judge()[0])


class Ends(unittest.TestCase):
    """The output's presentation ends against the last input's files."""

    def judge(self, ends, tkhd=(1700, 1700), mvhd=1700):
        # Output tracks 0 (video) and 1 (audio); the last input presents
        # video up to 0.9 s and audio up to 0.92 s of its own, from 0.8 s.
        own = {('out.mp4', 0): ends[0], ('out.mp4', 1): ends[1], ('in1.mp4', 0): Fraction(9, 10),
               ('in1.mp4', 1): Fraction(92, 100)}
        inputs = [{'path': 'in0.mp4', 'offset': {'num': 0, 'den': 1}, 'video_stream': 0, 'audio_stream': 1},
                  {'path': 'in1.mp4', 'offset': {'num': 4, 'den': 5}, 'video_stream': 0, 'audio_stream': 1,
                   'video_end': {'num': 17, 'den': 10}}]
        oh = {'mvhd': (1000, mvhd), 'traks': [{'tkhd': tkhd[0], 'elst': [[1, 0]]}, {'tkhd': tkhd[1], 'elst': [[1, 0]]}]}
        found = []
        saved = jc.own_end
        jc.own_end = lambda ffprobe, path, index: own[(path, index)]
        try:
            jc.check_ends('ffprobe', inputs, [('video', 'video_stream'), ('audio', 'audio_stream')], 'out.mp4', oh,
                          found)
        finally:
            jc.own_end = saved
        return [field for field, _ in found]

    def test_ends(self):
        self.assertEqual(self.judge((Fraction(17, 10), Fraction(17, 10))), [])
        # Audio ended earlier, where gapless information trims padding.
        self.assertEqual(self.judge((Fraction(17, 10), Fraction(169, 100)), tkhd=(1700, 1690)), [])
        # The last video edit longer or shorter than the input's span, or
        # past what the input presents.
        self.assertIn('video:end', self.judge((Fraction(22, 10), Fraction(17, 10)), tkhd=(2200, 1700), mvhd=2200))
        self.assertIn('video:end', self.judge((Fraction(15, 10), Fraction(17, 10)), tkhd=(1500, 1700)))
        # The last audio edit past what the input presents.
        self.assertIn('audio:end', self.judge((Fraction(17, 10), Fraction(22, 10)), tkhd=(1700, 2200), mvhd=2200))
        # Headers that disagree with the edits.
        self.assertIn('video:end', self.judge((Fraction(17, 10), Fraction(17, 10)), tkhd=(1600, 1700)))
        self.assertIn('end', self.judge((Fraction(17, 10), Fraction(17, 10)), mvhd=1800))


class SourcePtsShift(unittest.TestCase):
    def evidence(self):
        # The joined video's smallest offset is -100 ticks at 6,000 a
        # second; the source has no edit list.
        base = Fraction(1, 6000)
        sf = [(t, 10, f'h{t}', 100) for t in (100, 200, 300)]
        of = [(t, 10, f'h{t + 100}', 100) for t in (0, 100, 200)]
        return {'source_headers': {'traks': [{}]}, 'source_video_index': 0,
                'output_headers': {'traks': [{'ctts_min': -100, 'mdhd': (6000, 0)}]},
                'source_video_start_gap': Fraction(1, 60),
                'source_video_packets': (True, {'pts': {Fraction(100)}, 'dts': {Fraction(100)}}, base, True),
                'source_video_frames': (base, sf, base, of, Fraction(0), Fraction(300, 6000))}

    def test_every_field_shifted_by_the_smallest_offset(self):
        for field in ('video:start_time', 'video:packets', 'video:framemd5'):
            self.assertTrue(jc.source_pts_shift(field, set(), self.evidence()), field)

    def test_near_misses(self):
        cases = [
            # The source has an edit list: every field.
            (lambda c: c['source_headers']['traks'][0].update(elst=[[1, 0]]), 'video:packets'),
            (lambda c: c['source_headers']['traks'][0].update(elst=[[1, 0]]), 'video:start_time'),
            # No negative offset in the joined video.
            (lambda c: c['output_headers']['traks'][0].update(ctts_min=0), 'video:packets'),
            # Start times apart by another amount.
            (lambda c: c.update(source_video_start_gap=Fraction(1, 30)), 'video:start_time'),
            # A packet that differs in data.
            (lambda c: c.update(source_video_packets=(False, c['source_video_packets'][1], Fraction(1, 6000),
                                                      True)), 'video:packets'),
            # Two differences.
            (lambda c: c['source_video_packets'][1]['pts'].add(Fraction(99)), 'video:packets'),
        ]
        for change, field in cases:
            c = self.evidence()
            change(c)
            self.assertFalse(jc.source_pts_shift(field, set(), c), field)
        c = self.evidence()
        sb, sf, ob, of, offset, end = c['source_video_frames']
        c['source_video_frames'] = (sb, sf, ob, of[:1] + of[2:], offset, end)
        self.assertFalse(jc.source_pts_shift('video:framemd5', set(), c))
        self.assertFalse(jc.source_pts_shift('audio:packets', set(), self.evidence()))



class SeveralEdits(unittest.TestCase):
    """The audio check of joins with several edits (common decision 3 of
    docs/design/edit-list-writers.md) on two inputs that each present one
    second of AAC after 2,112 samples of priming."""
    TS = 48_000
    PARTS = [('a.mp4', None, Fraction(0)), ('b.mp4', None, Fraction(1))]
    TRACKS = [(TS, [(Fraction(2112), Fraction(50_112), Fraction(0))])] * 2
    RUNS = [(0, 0, 50_176), (50_176, 0, 50_176)]

    def check(self, windows):
        found = []
        jc.several_audio(False, self.PARTS, self.TRACKS, (self.TS, windows), self.RUNS, found)
        return found

    def joined(self):
        return [(Fraction(2112), Fraction(50_112), Fraction(0)),
                (Fraction(52_288), Fraction(100_288), Fraction(48_000))]

    def test_each_input_at_its_offset_passes(self):
        self.assertEqual(self.check(self.joined()), [])

    def test_other_media_fails(self):
        windows = self.joined()
        windows[1] = (windows[1][0] + 100, windows[1][1] + 100, windows[1][2])
        self.assertEqual([f for f, _ in self.check(windows)], ['audio:edits'])

    def test_a_later_time_fails(self):
        windows = self.joined()
        windows[1] = (windows[1][0], windows[1][1], windows[1][2] + 10)
        self.assertEqual([f for f, _ in self.check(windows)], ['audio:edits'])

    def test_a_missing_input_fails(self):
        self.assertEqual([f for f, _ in self.check(self.joined()[:1])], ['audio:edits'])

    def test_a_gap_inside_an_input_is_expected(self):
        tracks = [(self.TS, [(Fraction(2112), Fraction(26_112), Fraction(0)),
                             (Fraction(26_112), Fraction(50_112), Fraction(30_000))])] + self.TRACKS[1:]
        windows = [(Fraction(2112), Fraction(26_112), Fraction(0)),
                   (Fraction(26_112), Fraction(50_112), Fraction(30_000)),
                   (Fraction(52_288), Fraction(100_288), Fraction(54_000))]
        parts = [self.PARTS[0], ('b.mp4', None, Fraction(54_000, self.TS))]
        found = []
        jc.several_audio(False, parts, tracks, (self.TS, windows), self.RUNS, found)
        self.assertEqual(found, [])
        found = []
        closed = [(Fraction(2112), Fraction(50_112), Fraction(0)), windows[2]]
        jc.several_audio(False, parts, tracks, (self.TS, closed), self.RUNS, found)
        self.assertEqual([f for f, _ in found], ['audio:edits'])


class ConcatAgree(unittest.TestCase):
    """The packet comparison with ffmpeg concat's output."""

    @staticmethod
    def packet(data, flags='K_', size=10):
        return {'data_hash': data, 'size': size, 'flags': flags}

    def test_equal_packets_agree(self):
        p = [self.packet('a'), self.packet('b', '__')]
        self.assertEqual(jc.concat_agree(p, copy.deepcopy(p)), [True, True])

    def test_keyframes_agree_without_parameter_sets(self):
        ours, theirs = [self.packet('a', size=10)], [self.packet('x', size=30)]
        stripped = ([self.packet('s')], [self.packet('s')])
        self.assertEqual(jc.concat_agree(ours, theirs, stripped), [True])
        self.assertEqual(jc.concat_agree(ours, theirs), [False])

    def test_a_keyframe_flag_ffmpeg_lacks_fails(self):
        stripped = ([self.packet('s')], [self.packet('s')])
        for ours, theirs in (('K_', '__'), ('__', 'K_')):
            with self.subTest(ours=ours, theirs=theirs):
                self.assertEqual(jc.concat_agree([self.packet('a', ours)], [self.packet('a', theirs)], stripped),
                                 [False])
                self.assertEqual(jc.concat_agree([self.packet('a', ours)], [self.packet('x', theirs)], stripped),
                                 [False])

    def test_other_data_without_parameter_sets_fails(self):
        stripped = ([self.packet('s')], [self.packet('t')])
        self.assertEqual(jc.concat_agree([self.packet('a')], [self.packet('x')], stripped), [False])

if __name__ == '__main__':
    unittest.main()
