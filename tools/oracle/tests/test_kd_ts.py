"""Keeps docs/known-divergences-ts.md, its JSON twin and the predicates of
scripts/oracle_kd_ts.py in sync, and checks the evidence reader and each
predicate on small transport streams built here."""
import importlib.util
import json
import pathlib
import re
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('oracle_kd_ts', ROOT / 'scripts' / 'oracle_kd_ts.py')
kd = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(kd)


def crc32(data):
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte << 24
        for _ in range(8):
            crc = ((crc << 1) ^ 0x04C11DB7 if crc & 0x80000000 else crc << 1) & 0xFFFFFFFF
    return crc


class Mux:
    """188-byte packets with continuity counters."""

    def __init__(self):
        self.out, self.counters = bytearray(), {}

    def packet(self, pid, start, payload, counter=None):
        if counter is None:
            counter = self.counters.get(pid, -1) + 1 & 15
        self.counters[pid] = counter
        header = bytes([0x47, (0x40 if start else 0) | pid >> 8, pid & 0xFF, 0x10 | counter])
        if len(payload) < 184:
            pad = 184 - len(payload)
            header = header[:3] + bytes([header[3] | 0x20, pad - 1]) + (b'\x00' + b'\xff' * (pad - 2) if pad > 1 else b'')
        self.out += header + payload
        assert len(self.out) % 188 == 0

    def unit(self, pid, data):
        for i in range(0, len(data), 184):
            self.packet(pid, i == 0, data[i:i + 184])

    def section(self, pid, table, ident, body):
        length = 5 + len(body) + 4
        s = bytes([table, 0xB0 | length >> 8, length & 0xFF]) + ident.to_bytes(2, 'big') + b'\xc1\x00\x00' + body
        self.unit(pid, b'\x00' + s + crc32(s).to_bytes(4, 'big'))

    def tables(self, streams):
        self.section(0, 0, 1, bytes([0, 1, 0xF0, 0x00]))
        body = bytes([0xE1, 0x00, 0xF0, 0x00])
        for kind, pid, descriptors in streams:
            body += bytes([kind, 0xE0 | pid >> 8, pid & 0xFF, 0xF0, len(descriptors)]) + descriptors
        self.section(0x1000, 2, 1, body)

    def programs(self, programs):
        """A PAT of several programs and one Program Map Table each;
        `programs` is {number: (pmt pid, [(stream type, pid)])}."""
        pat = b''
        for number, (pmt, _) in sorted(programs.items()):
            pat += bytes([number >> 8, number & 0xFF, 0xE0 | pmt >> 8, pmt & 0xFF])
        self.section(0, 0, 1, pat)
        for number, (pmt, streams) in sorted(programs.items()):
            body = bytes([0xE1, 0x00, 0xF0, 0x00])
            for kind, pid in streams:
                body += bytes([kind, 0xE0 | pid >> 8, pid & 0xFF, 0xF0, 0])
            self.section(pmt, 2, number, body)

    def pes(self, pid, pts, payload):
        t = bytes([0x21 | ((pts >> 30) & 7) << 1, (pts >> 22) & 0xFF, ((pts >> 15) & 0x7F) << 1 | 1,
                   (pts >> 7) & 0xFF, (pts & 0x7F) << 1 | 1])
        length = len(payload) + 8
        self.unit(pid, b'\x00\x00\x01\xc0' + length.to_bytes(2, 'big') + b'\x80\x80\x05' + t + payload)


def adts(length, rate=3):
    """An ADTS frame of AAC LC, `length` bytes; `rate` indexes the sampling
    frequency table (3 is 48 kHz, 7 is 22.05 kHz)."""
    return bytes([0xFF, 0xF1, 0x40 | rate << 2, 0x80 | (length >> 11) & 3, (length >> 3) & 0xFF,
                  (length & 7) << 5 | 0x1F, 0xFC]) + b'\x21' * (length - 7)


def eac3():
    """An E-AC-3 frame: 48 kHz, 6 blocks (1536 samples, 2880 ticks)."""
    return b'\x0b\x77\x01\x7f\x3f\x80' + b'\x00' * 762


def written(data):
    path = pathlib.Path(tempfile.mkdtemp()) / 'x.ts'
    path.write_bytes(bytes(data))
    return path


class Sync(unittest.TestCase):
    def test_page_twin_and_predicates_agree(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-ts.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-ts.md').read_text()
        self.assertEqual([e['id'] for e in twin['entries']], re.findall(r'^## (KD-TS-[A-Z-]+)$', page, re.M))
        for entry in twin['entries']:
            self.assertIn(entry['predicate'], kd.PREDICATES)
            self.assertIn('evidence', entry['conditions'])


class Evidence(unittest.TestCase):
    def eac3_file(self):
        mux = Mux()
        mux.tables([(0x06, 0x101, b'\x7a\x01\x00')])
        for k in range(3):
            mux.pes(0x101, 90000 + 2880 * k, eac3())
        return written(mux.out)

    def stream(self, duration, start=90000, codec='eac3'):
        return {'codec_name': codec, 'duration_ts': duration,
                'properties': {'pid': 0x101, 'start_pts': start, 'end_unit_ticks': 2880}}

    def test_end_unit_from_the_file(self):
        facts = kd.Facts(self.eac3_file())
        self.assertEqual(facts.end_unit(0x101), ('eac3', 90000, 90000 + 2 * 2880, 2880))

    def test_end_estimate_pins_coreav_to_the_file(self):
        facts = kd.Facts(self.eac3_file())
        ours = 3 * 2880
        self.assertTrue(kd.end_estimate(self.stream(ours - 2880), self.stream(ours), facts))
        # CoreAV one unit long, the reference following it: not the file's.
        self.assertFalse(kd.end_estimate(self.stream(ours), self.stream(ours + 2880), facts))
        # Another codec named, or no file.
        self.assertFalse(kd.end_estimate(self.stream(ours - 2880), self.stream(ours, codec='aac'), facts))
        self.assertFalse(kd.end_estimate(self.stream(ours - 2880), self.stream(ours), None))

    def test_format_needs_coreav_span_and_every_stream_covered(self):
        facts = kd.Facts(self.eac3_file())
        ours, theirs = 3 * 2880, 2 * 2880
        span = lambda ticks: (ticks * 1_000_000 + 45_000) // 90_000
        cand = {'format_duration_us': span(ours), 'streams': [self.stream(ours)]}
        ref = {'format_duration_us': span(theirs), 'streams': [self.stream(theirs)]}
        self.assertTrue(kd.format_from_streams(ref, cand, facts, {}))
        self.assertFalse(
            kd.format_from_streams(ref, dict(cand, format_duration_us=span(ours) + 1), facts, {}))
        # Every stream agreeing and the file's duration a microsecond
        # apart is the rounding clause; further apart is not covered.
        same = {'format_duration_us': span(ours) + 1, 'streams': [self.stream(ours)]}
        self.assertTrue(kd.format_from_streams(same, cand, facts, {}))
        far = {'format_duration_us': span(ours) + 100, 'streams': [self.stream(ours)]}
        self.assertFalse(kd.format_from_streams(far, cand, facts, {}))

    def test_end_at_another_unit_needs_the_reference_own_unit_end(self):
        facts = kd.Facts(self.eac3_file())
        ours = 3 * 2880
        # The file's three E-AC-3 frames: the reference ends at the second.
        packets = {'streams': [{'index': 0, 'packets': [
            {'pts': 90000 + 2880 * k, 'dts': 90000 + 2880 * k, 'duration': 2880} for k in range(3)]}]}
        self.assertTrue(
            kd.end_at_another_unit(self.stream(ours - 2880), self.stream(ours), facts, packets, 0))
        # An end no packet of the reference's own list gives.
        self.assertFalse(
            kd.end_at_another_unit(self.stream(ours - 1), self.stream(ours), facts, packets, 0))
        # An end later than CoreAV's, at a unit of the reference's own
        # list, is this entry too: the reference takes the unit from the
        # frame rate it reports, which rounding can make longer.
        longer = {'streams': [dict(packets['streams'][0], packets=[
            {'pts': 90000 + 2880 * k, 'dts': 90000 + 2880 * k, 'duration': 2883} for k in range(3)])]}
        self.assertTrue(
            kd.end_at_another_unit(self.stream(ours + 3), self.stream(ours), facts, longer, 0))
        # A candidate duration that is not the file's.
        self.assertFalse(
            kd.end_at_another_unit(self.stream(ours - 2880), self.stream(ours + 1), facts, packets, 0))

    def test_timed_from_the_file_needs_the_file_duration_or_none(self):
        facts = kd.Facts(self.eac3_file())
        ours = 3 * 2880
        whole = (ours * 1_000_000 + 45_000) // 90_000
        reference = {'format_duration_us': whole}
        self.assertTrue(
            kd.timed_from_the_file(self.stream(None), self.stream(ours), facts, reference))
        self.assertTrue(
            kd.timed_from_the_file(self.stream(ours), self.stream(ours), facts, reference))
        # A reference duration that is neither none nor the file's.
        self.assertFalse(
            kd.timed_from_the_file(self.stream(ours // 2), self.stream(ours), facts, reference))
        # A candidate duration that is not the file's.
        self.assertFalse(
            kd.timed_from_the_file(self.stream(None), self.stream(ours + 1), facts, reference))

    def test_aac_profile_needs_the_configuration_the_file_states(self):
        mux = Mux()
        mux.tables([(0x0F, 0x101, b'')])
        mux.pes(0x101, 90000, adts(100))
        facts = kd.Facts(written(mux.out))
        # The ADTS header of `adts()` states AAC LC (profile 1).
        self.assertEqual(facts.aac_object[0x101], 2)
        ours = {'codec_name': 'aac', 'profile': 'LC', 'properties': {'pid': 0x101}}
        self.assertTrue(kd.aac_profile({'profile': 'HE-AAC'}, ours, facts))
        self.assertTrue(kd.aac_profile({'profile': 'HE-AACv2'}, ours, facts))
        # Another profile on either side is not this entry.
        self.assertFalse(kd.aac_profile({'profile': 'Main'}, ours, facts))
        self.assertFalse(kd.aac_profile({'profile': 'HE-AAC'}, dict(ours, profile='Main'), facts))
        self.assertFalse(kd.aac_profile({'profile': 'HE-AAC'}, ours, None))

    def two_programs(self):
        mux = Mux()
        mux.programs({1: (0x1000, [(0x0F, 0x101)]), 2: (0x1001, [(0x0F, 0x201)])})
        mux.pes(0x101, 90000, adts(100))
        mux.pes(0x201, 90000, adts(200))
        return kd.Facts(written(mux.out))

    def order_captures(self, facts, swap, cross=False):
        """A two-stream document on each side, the reference's streams in
        the other order when `swap`; with `cross` the candidate's packets
        are swapped between its streams, which is a bug, not an order."""
        places = [min(facts.pes_offsets[pid]) for pid in (0x101, 0x201)]
        packets = [[{'pos': places[0], 'size': 100, 'pts': 1, 'dts': 1, 'keyframe': True}],
                   [{'pos': places[1], 'size': 200, 'pts': 2, 'dts': 2, 'keyframe': True}]]
        streams = [{'codec_name': 'aac', 'kind': 'audio', 'duration_ts': 10,
                    'properties': {'pid': 0x101, 'start_pts': 0}},
                   {'codec_name': 'aac', 'kind': 'audio', 'duration_ts': 20,
                    'properties': {'pid': 0x201, 'start_pts': 0}}]
        at = [1, 0] if swap else [0, 1]
        mine = [1, 0] if cross else [0, 1]
        return (
            {'format_duration_us': 1,
             'streams': [dict(streams[i], index=k) for k, i in enumerate(at)]},
            {'format_duration_us': 1, 'streams': streams},
            {'streams': [dict(packets=packets[i], index=k) for k, i in enumerate(at)]},
            {'streams': [{'index': k, 'packets': packets[i]} for k, i in enumerate(mine)]},
        )

    def test_stream_order_needs_the_tables_and_a_permutation(self):
        facts = self.two_programs()
        self.assertEqual(sorted(facts.programs), [1, 2])
        rstreams, cstreams, rpackets, cpackets = self.order_captures(facts, swap=True)
        order = kd.ordered_differently(rstreams, cstreams, facts, rpackets, cpackets)
        self.assertEqual(order, {0: 1, 1: 0})
        self.assertIsNotNone(
            kd.order_covers_streams(rstreams, cstreams, facts, rpackets, order))
        self.assertEqual(kd.packets_differing_under(rpackets, cpackets, order), 0)
        # The same order is not this entry, nor is a file of one program.
        same = self.order_captures(facts, swap=False)
        self.assertIsNone(kd.ordered_differently(same[0], same[1], facts, same[2], same[3]))
        mux = Mux()
        mux.tables([(0x0F, 0x101, b'')])
        mux.pes(0x101, 90000, adts(100))
        one = kd.Facts(written(mux.out))
        self.assertIsNone(kd.ordered_differently(rstreams, cstreams, one, rpackets, cpackets))
        # A candidate whose streams are not the tables' is never covered.
        other = {'format_duration_us': 1,
                 'streams': [dict(cstreams['streams'][1]), dict(cstreams['streams'][0])]}
        self.assertIsNone(kd.ordered_differently(rstreams, other, facts, rpackets, cpackets))

    def test_stream_order_rejects_packets_of_another_stream(self):
        # The bug this entry must never cover: CoreAV lists the packets of
        # one stream under another, which on two streams the oracle gates
        # alike would look exactly like a reordering.
        facts = self.two_programs()
        rstreams, cstreams, rpackets, cpackets = self.order_captures(
            facts, swap=False, cross=True)
        self.assertFalse(kd.packets_of_their_pid(cstreams, cpackets, facts))
        self.assertIsNone(kd.ordered_differently(rstreams, cstreams, facts, rpackets, cpackets))
        # A stream whose packets no stream of the reference lists is not a
        # reordering either.
        lonely = {'streams': [{'index': 0, 'packets': [
            {'pos': min(facts.pes_offsets[0x101]), 'size': 7, 'pts': 1, 'dts': 1,
             'keyframe': True}]}, {'index': 1, 'packets': []}]}
        self.assertIsNone(kd.stream_permutation(rpackets, lonely))

    def test_teletext_size_is_the_decoder_page(self):
        mux = Mux()
        # A private stream with a DVB teletext descriptor (tag 0x56).
        mux.tables([(0x06, 0x101, bytes([0x56, 5, 0x67, 0x65, 0x72, 0x10, 0x00]))])
        mux.pes(0x101, 90000, b'\x02' * 40)
        facts = kd.Facts(written(mux.out))
        ours = {'width': None, 'height': None, 'properties': {'pid': 0x101}}
        self.assertTrue(kd.teletext_size({'width': 492, 'height': 250}, ours, facts))
        self.assertFalse(kd.teletext_size({'width': 720, 'height': 576}, ours, facts))
        self.assertFalse(kd.teletext_size({'width': 492, 'height': 250},
                                          dict(ours, width=492), facts))

    def test_duplicates_and_units_from_the_file(self):
        mux = Mux()
        mux.tables([(0x0F, 0x101, b'')])
        mux.pes(0x101, 90000, adts(100) + adts(150))
        # The PES packet's first transport packet, repeated.
        first = len(mux.out)
        mux.pes(0x101, 94000, adts(400))
        repeat = bytes(mux.out[first:first + 188])
        mux.out[first + 188:first + 188] = repeat
        facts = kd.Facts(written(mux.out), keep=[0x101])
        self.assertEqual(facts.duplicates[0x101], [first + 188])
        units = facts.units(0x101)
        self.assertEqual([u[1:3] for u in units], [(100, 90000), (150, 91920), (400, 94000)])
        packets = [{'pos': pos, 'size': size, 'pts': pts, 'dts': dts, 'keyframe': key}
                   for pos, size, pts, dts, key, _ in units]
        stream = {'properties': {'pid': 0x101}, 'packets': packets}
        diff = [{'path': 'streams/0/packets/2/size', 'reference': {'pos': first}, 'candidate': None}]
        self.assertTrue(kd.after_duplicate(stream, diff, facts))
        wrong = dict(stream, packets=[dict(p, size=p['size'] + (i == 1)) for i, p in enumerate(packets)])
        self.assertFalse(kd.after_duplicate(wrong, diff, facts))
        early = [{'path': 'streams/0/packets/0/size', 'reference': {'pos': 0}, 'candidate': None}]
        self.assertFalse(kd.after_duplicate(stream, early, facts))

    def test_frame_time_covers_a_tick_of_interpolation(self):
        # 22.05 kHz: a frame of 1,024 samples lasts 4,179.59 ticks, so a
        # run interpolated by truncation drifts and one rounded once does
        # not. One PES packet, three frames, one stated time.
        mux = Mux()
        mux.tables([(0x0F, 0x101, b'')])
        mux.pes(0x101, 90000, adts(100, 7) + adts(150, 7) + adts(120, 7))
        facts = kd.Facts(written(mux.out), keep=[0x101])
        units = facts.units(0x101)
        self.assertEqual([u[2] for u in units], [90000, 94180, 98359])
        self.assertEqual([u[5] for u in units], [True, False, False])
        packets = [{'pos': pos, 'size': size, 'pts': pts, 'dts': dts, 'keyframe': key}
                   for pos, size, pts, dts, key, _ in units]
        stream = {'properties': {'pid': 0x101}, 'packets': packets}
        one_tick = [{'path': 'streams/0/packets/1/pts', 'candidate': 94180, 'reference': 94179},
                    {'path': 'streams/0/packets/1/dts', 'candidate': 94180, 'reference': 94179}]
        self.assertTrue(kd.frame_time(stream, one_tick, facts))
        # Two ticks apart is not covered, nor is another field, nor the
        # first frame of a PES packet, which the file does time.
        two = [dict(one_tick[0], reference=94178)]
        self.assertFalse(kd.frame_time(stream, two, facts))
        other = [{'path': 'streams/0/packets/1/size', 'candidate': 150, 'reference': 151}]
        self.assertFalse(kd.frame_time(stream, other, facts))
        stated = [{'path': 'streams/0/packets/0/pts', 'candidate': 90000, 'reference': 90001}]
        self.assertFalse(kd.frame_time(stream, stated, facts))
        # A candidate list that is not the file's is never covered.
        wrong = dict(stream, packets=[dict(p, pts=p['pts'] + 1) for p in packets])
        self.assertFalse(kd.frame_time(wrong, one_tick, facts))
        self.assertFalse(kd.frame_time(stream, one_tick, None))

    def test_wrap(self):
        near = (1 << 33) - 9000
        self.assertEqual(kd.unwrap(near, near), -9000)
        self.assertEqual(kd.unwrap(500, near), 500)
        far = (1 << 33) - 70 * 90000
        self.assertEqual(kd.unwrap(500, far), 500 + (1 << 33))


def bits(fields):
    """`fields`, (value, width) pairs, packed most significant first and
    padded to whole bytes."""
    value, width = 0, 0
    for v, n in fields:
        value, width = value << n | v, width + n
    pad = -width % 8
    return (value << pad).to_bytes((width + pad) // 8, 'big')


def with_crc1(frame, words=None):
    """`frame` with the `crc1` that makes its first 5/8 check (ATSC A/52
    7.10.1), of `words` words or as many as its header states. The
    register is linear in `crc1`, so the bits that cancel the rest of the
    5/8 are solved for."""
    words = words or kd.ac3_frame_words(frame[4] >> 6, frame[4] & 0x3F)
    covered = 2 * ((words >> 1) + (words >> 3))

    def remainder(crc1):
        return kd.ac3_crc(crc1.to_bytes(2, 'big') + bytes(frame[4:covered]))

    target, basis = remainder(0), {}
    for bit in range(16):
        v, c = remainder(1 << bit) ^ remainder(0), 1 << bit
        for top in reversed(range(16)):
            if not v >> top & 1:
                continue
            if top not in basis:
                basis[top] = (v, c)
                break
            v, c = v ^ basis[top][0], c ^ basis[top][1]
    crc1 = 0
    for top in reversed(range(16)):
        if target >> top & 1:
            target, crc1 = target ^ basis[top][0], crc1 ^ basis[top][1]
    return frame[:2] + crc1.to_bytes(2, 'big') + frame[4:]


def ac3_annex_d(acmod=7, xbsi1e=1, levels=(1, 4, 5, 4, 5)):
    """An AC-3 syncframe of 64 words (48 kHz, 32 kbit/s) in the alternate
    syntax (`bsid` 6) with no optional words before its first extra
    information, and a `crc1` that checks."""
    fields = [(0x0B77, 16), (0, 16), (0, 2), (0, 6), (6, 5), (0, 3), (acmod, 3)]
    fields += [(0, 2)] * ((acmod & 1 and acmod != 1) + bool(acmod & 4) + (acmod == 2))
    fields += [(0, 1), (31, 5), (0, 1), (0, 1), (0, 1), (0, 1), (1, 1), (xbsi1e, 1)]
    if xbsi1e:
        fields += list(zip(levels, (2, 3, 3, 3, 3)))
    frame = bits(fields + [(0, 1), (0, 1)])
    return with_crc1(frame + bytes(128 - len(frame)))


class SpecificationDecides(unittest.TestCase):
    def vbi_file(self, data_identifier=0x99, descriptor=True, keep=(0x101,), later=None):
        mux = Mux()
        tags = bytes([0x45, 6, 0x04, 1, 0xF0, 0x05, 1, 0xF7]) if descriptor else b''
        mux.tables([(0x06, 0x101, tags)])
        for k in range(3):
            unit = bytes([0xC3, 0x2C]) + bytes(44)
            data = bytes([data_identifier]) + unit * 3
            if k and later is not None:
                data = later
            mux.pes(0x101, 90000 + 3600 * k, data)
        return kd.Facts(written(mux.out), keep=keep)

    def test_vbi_payloads_are_every_pes_packet_s_whatever_is_kept(self):
        mpeg_audio = bytes([0xFF, 0xFD, 0xE0, 0xC4]) + bytes(140)
        for keep in ((), (0x101,)):
            self.assertTrue(kd.vbi_payloads(self.vbi_file(keep=keep), 0x101))
            self.assertFalse(kd.vbi_payloads(self.vbi_file(keep=keep, later=mpeg_audio), 0x101))
        self.assertFalse(kd.vbi_payloads(self.vbi_file(keep=()), 0x102))

    def vbi_documents(self, facts):
        """Both sides' streams and packets of the VBI stream of `vbi_file`,
        as the oracle adapters give them: the reference names no PID."""
        common = {'codec_fourcc': '[6][0][0][0]', 'timescale': 90000, 'duration_ts': 7200}
        theirs = dict(common, kind='audio', codec_name='mp3', sample_rate=0, channels=0)
        ours = dict(common, kind='data', codec_name='bin_data', properties={'pid': 0x101})
        listed = [{'pos': offset, 'size': len(payload), 'pts': pts, 'dts': pts, 'keyframe': True}
                  for offset, pts, payload in facts.pes[0x101]]
        theirs_listed = [{'pos': listed[0]['pos'], 'size': 429, 'pts': listed[0]['pts'],
                          'dts': listed[0]['pts'], 'keyframe': True}]
        return theirs, ours, listed, theirs_listed

    def test_vbi_as_audio_needs_the_descriptor_and_data_identifier(self):
        facts = self.vbi_file()
        theirs, ours, listed, theirs_listed = self.vbi_documents(facts)
        rlisted = {'packets': theirs_listed}
        self.assertTrue(kd.vbi_as_audio(theirs, ours, facts, rlisted))
        self.assertTrue(kd.vbi_as_audio(theirs, ours, self.vbi_file(0x10), rlisted))
        self.assertTrue(kd.vbi_as_audio({'codec_type': 'audio', 'codec_name': 'mp3', 'id': '0x101'},
                                        {'codec_type': 'data', 'codec_name': 'bin_data', 'id': '0x101'},
                                        facts))
        self.assertFalse(kd.vbi_as_audio(theirs, ours, self.vbi_file(0x20), rlisted))
        self.assertFalse(kd.vbi_as_audio(theirs, ours, self.vbi_file(descriptor=False), rlisted))
        self.assertFalse(kd.vbi_as_audio(dict(theirs, kind='data'), ours, facts, rlisted))
        self.assertFalse(kd.vbi_as_audio(theirs, dict(ours, kind='audio'), facts, rlisted))
        # Another PID's stream on the reference's side: named, or found by
        # where its first packet starts.
        self.assertFalse(kd.vbi_as_audio(dict(theirs, id='0x102'), ours, facts))
        self.assertFalse(kd.vbi_as_audio(theirs, ours, facts, {'packets': [dict(theirs_listed[0], pos=0)]}))
        self.assertFalse(kd.vbi_as_audio(theirs, ours, facts))
        # A candidate field the bytes do not justify.
        for name, value in (('timescale', 1), ('duration_ts', 999999999), ('codec_name', 'h264'),
                            ('width', 1920), ('sample_rate', 48000)):
            self.assertFalse(kd.vbi_as_audio(theirs, dict(ours, **{name: value}), facts, rlisted), name)

    def test_vbi_overlay_covers_only_what_the_bytes_justify(self):
        media = written(b'')
        facts = self.vbi_file()
        media.write_bytes(facts.path.read_bytes())
        theirs, ours, listed, theirs_listed = self.vbi_documents(facts)
        reference, packets = {'streams': [theirs]}, {'streams': [{'packets': theirs_listed}]}

        def streams(candidate_stream, paths):
            kd._FACTS.clear()
            detail = {'file': media.name, 'gating_differences': [{'path': p} for p in paths]}
            return kd.stream_item(detail, reference, {'streams': [candidate_stream]}, media.parent,
                                  packets, {'streams': [dict(candidate_stream, packets=listed)]})[:3]

        self.assertEqual(streams(ours, ['streams/0/kind', 'streams/0/codec_name']),
                         ('kd', {'KD-TS-VBI-AS-AUDIO'}, []))
        for name, value in (('timescale', 1), ('duration_ts', 999999999), ('width', 1920)):
            path = f'streams/0/{name}'
            status, _, uncovered = streams(dict(ours, **{name: value}), ['streams/0/kind', path])
            self.assertEqual((status, uncovered), ('uncovered', ['streams/0/kind', path]), name)
        self.assertEqual(streams(dict(ours, codec_name='h264'), ['streams/0/codec_name'])[0], 'uncovered')

        def units(candidate_listed):
            kd._FACTS.clear()
            paths = [f'streams/0/packets/{i}/size' for i in range(len(listed))]
            detail = {'file': media.name, 'packets_disagree': len(paths),
                      'gating_differences': [{'path': p} for p in paths]}
            candidate = {'streams': [dict(ours, packets=candidate_listed)]}
            return kd.packet_units(detail, candidate, media.parent, reference, {'streams': [ours]},
                                   packets)[:2]

        self.assertEqual(units(listed), ({'KD-TS-VBI-AS-AUDIO': 3}, 0))
        first = listed[0]
        for change in ({'pts': first['pts'] + 1}, {'size': first['size'] + 1}, {'pos': 0},
                       {'keyframe': False}):
            self.assertEqual(units([dict(first, **change)] + listed[1:]), ({}, 3), change)
        self.assertEqual(units(listed + [listed[-1]]), ({}, 3))

    def test_vbi_listing_counts_the_pes_packets_before_the_tables(self):
        mux = Mux()
        unit = bytes([0x99]) + (bytes([0xC3, 0x2C]) + bytes(44)) * 3
        mux.pes(0x101, 86400, unit)
        mux.tables([(0x06, 0x101, bytes([0x45, 6, 0x04, 1, 0xF0, 0x05, 1, 0xF7]))])
        for k in range(2):
            mux.pes(0x101, 90000 + 3600 * k, unit)
        facts = kd.Facts(written(mux.out), keep=[0x101])
        self.assertEqual(len(facts.pes_before_tables[0x101]), 1)
        listed = [{'pos': offset, 'size': len(payload), 'pts': pts, 'dts': pts, 'keyframe': True}
                  for offset, pts, payload in facts.pes_before_tables[0x101] + facts.pes[0x101]]
        ours = {'properties': {'pid': 0x101}}
        self.assertTrue(kd.vbi_listing(dict(ours, packets=listed), facts))
        self.assertFalse(kd.vbi_listing(dict(ours, packets=listed[1:]), facts))
        # What is kept for one PID changes nothing else the reader finds.
        plain = kd.Facts(facts.path)
        self.assertEqual((plain.first_pts, plain.times, plain.pes_leads),
                         (facts.first_pts, facts.times, facts.pes_leads))

    def wrap_file(self, first=5_479_446_912, other=6_353_039_068):
        mux = Mux()
        mux.programs({1: (0x1000, [(0x02, 0x101)]), 2: (0x1001, [(0x02, 0x102)])})
        for k in range(3):
            mux.pes(0x101, first + 3600 * k, b'\x00' * 100)
        mux.pes(0x102, other, b'\x00' * 100)
        return kd.Facts(written(mux.out))

    def test_program_wrap_needs_a_program_that_does_not_wrap(self):
        start, base = 5_479_446_912, {'id': '0x101', 'time_base': '1/90000'}
        theirs = dict(base, start_pts=start + kd.WRAP, start_time=kd.seconds(start + kd.WRAP))
        ours = dict(base, start_pts=start, start_time=kd.seconds(start))
        facts = self.wrap_file()
        self.assertTrue(kd.program_wrap(theirs, ours, facts))
        self.assertTrue(kd.program_wrap({'start_pts': start + kd.WRAP},
                                        {'properties': {'pid': 0x101, 'start_pts': start}}, facts))
        self.assertFalse(kd.program_wrap(dict(theirs, start_pts=start), ours, facts))
        self.assertFalse(kd.program_wrap(theirs, dict(ours, start_pts=start + 1), facts))
        # Each timing field the record names, on either side.
        for side, name, value in (('ours', 'start_time', '-123.456789'), ('ours', 'time_base', '1/1000'),
                                  ('theirs', 'start_time', kd.seconds(start)),
                                  ('theirs', 'time_base', '1/1000')):
            changed = {name: value}
            if side == 'theirs':
                self.assertFalse(kd.program_wrap(dict(theirs, **changed), ours, facts), changed)
            else:
                self.assertFalse(kd.program_wrap(theirs, dict(ours, **changed), facts), changed)
        near = kd.WRAP - 1000
        self.assertFalse(kd.program_wrap(dict(theirs, start_pts=near + kd.WRAP, start_time=None),
                                         dict(ours, start_pts=near, start_time=None),
                                         self.wrap_file(first=near)))

    def ac3_file(self, frame):
        mux = Mux()
        mux.tables([(0x06, 0x101, b'\x6a\x01\x00')])
        mux.pes(0x101, 90000, frame)
        return kd.Facts(written(mux.out))

    def test_annex_d_downmix_is_what_the_frame_states(self):
        coded = {'dmix_mode': '1', 'ltrt_cmixlev': '0.707107', 'ltrt_surmixlev': '0.594604',
                 'loro_cmixlev': '0.707107', 'loro_surmixlev': '0.594604'}
        absent = {'dmix_mode': '0', 'ltrt_cmixlev': '0.000000', 'ltrt_surmixlev': '0.000000',
                  'loro_cmixlev': '0.000000', 'loro_surmixlev': '0.000000'}
        ours = dict(coded, id='0x101')
        self.assertEqual(kd.annex_d_downmix(ac3_annex_d()), coded)
        self.assertTrue(kd.annex_d_ignored(absent, ours, self.ac3_file(ac3_annex_d())))
        self.assertFalse(kd.annex_d_ignored(coded, ours, self.ac3_file(ac3_annex_d())))
        self.assertFalse(kd.annex_d_ignored(absent, dict(ours, ltrt_cmixlev='1.000000'),
                                            self.ac3_file(ac3_annex_d())))
        self.assertIsNone(kd.annex_d_downmix(ac3_annex_d(acmod=2)))
        self.assertIsNone(kd.annex_d_downmix(ac3_annex_d(xbsi1e=0)))
        self.assertIsNone(kd.annex_d_downmix(ac3_annex_d(levels=(1, 4, 1, 4, 5))))
        self.assertEqual(kd.annex_d_downmix(ac3_annex_d(acmod=3)),
                         dict(coded, ltrt_surmixlev='0.000000', loro_surmixlev='0.000000'))
        self.assertEqual(kd.annex_d_downmix(ac3_annex_d(acmod=6)),
                         dict(coded, ltrt_cmixlev='0.000000', loro_cmixlev='0.000000'))

    def test_annex_d_needs_an_ac3_syncframe(self):
        absent = {'dmix_mode': '0', 'ltrt_cmixlev': '0.000000', 'ltrt_surmixlev': '0.000000',
                  'loro_cmixlev': '0.000000', 'loro_surmixlev': '0.000000'}
        frame = ac3_annex_d()
        ours = dict(kd.annex_d_downmix(frame), id='0x101')
        # The reserved sample rate code, and a frame size code table 5.18
        # does not list, each with a crc1 that checks.
        reserved = with_crc1(frame[:4] + bytes([frame[4] | 0xC0]) + frame[5:], 64)
        unlisted = with_crc1(frame[:4] + bytes([38]) + frame[5:], 64)
        corrupt = frame[:20] + bytes([frame[20] ^ 1]) + frame[21:]
        for bad in (reserved, unlisted, corrupt, frame[:60]):
            self.assertIsNone(kd.annex_d_downmix(bad))
            self.assertFalse(kd.annex_d_ignored(absent, ours, self.ac3_file(bad)))
        mux = Mux()
        mux.tables([(0x06, 0x101, b'')])
        mux.pes(0x101, 90000, frame)
        self.assertFalse(kd.annex_d_ignored(absent, ours, kd.Facts(written(mux.out))))


if __name__ == '__main__':
    unittest.main()
