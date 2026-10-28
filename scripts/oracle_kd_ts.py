#!/usr/bin/env python3
"""Apply the documented transport stream known divergences to coreav-oracle
reports.

Decision 3 (oracle and correctness policy) and proposal 6.2: a mismatch counts
as agreement only when a documented entry's predicate covers it; covered items
stay in the denominator and are reported apart from direct agreement. The
entries live in docs/known-divergences-ts.md with the twin
docs/known-divergences-ts.json; the captures come from tools/ts-adapter and
scripts/oracle-ffprobe.sh. Each predicate checks CoreAV's values against
the file itself, read by `Facts`, a reader written apart from CoreAV's
(with --media; without it no entry covers anything), as the ISO base
media overlay does (#162). Captures are read, never rewritten.

Usage: oracle_kd_ts.py --report R [--report R2 ...] --reference DIR
                       --candidate DIR --out OUT --media DIR
Exit status: 0 when every transport stream item agrees directly or via a
documented divergence, 1 otherwise, 2 for usage or input errors.
"""
import argparse
import collections
import json
import pathlib
import re
import sys
from fractions import Fraction

ROOT = pathlib.Path(__file__).resolve().parent.parent
KD_FILE = ROOT / 'docs' / 'known-divergences-ts.json'
IN_SCOPE = ('.ts', '.m2ts', '.mts', '.m2t')
# The Event Information Table's PID (ETSI EN 300 468 5.2.4): no Program
# Map Table declares it, and both readers list it as a stream.
EIT_PID = 0x12

# Codecs whose end `ffprobe` estimates without the last access unit's
# duration (one unit), or with half of it (MPEG-1 video).
END_FRACTION = {'eac3': 1, 'opus': 1, 'mpeg1video': 0.5}


def load(directory, name, level):
    path = pathlib.Path(directory) / f'{name}.{level}.json'
    return json.loads(path.read_text()) if path.exists() else {}


def props(item):
    return (item or {}).get('properties') or {}


def stream_at(document, index):
    streams = document.get('streams', [])
    return streams[index] if 0 <= index < len(streams) else None



# ---------------------------------------------------------------- evidence

WRAP, WINDOW = 1 << 33, 60 * 90_000
# Bytes at the end of the file CoreAV's probe reads for the last
# timestamps (`Limits::max_tail_bytes`), and most timestamps kept per PID.
TAIL_BYTES = 4 << 20
MAX_TIMES = 1 << 21
AAC_RATES = (96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350)
MPEG_FRAME_RATES = {1: (24000, 1001), 2: (24, 1), 3: (25, 1), 4: (30000, 1001), 5: (30, 1), 6: (50, 1),
                    7: (60000, 1001), 8: (60, 1)}


def unwrap(raw, reference):
    """The 33-bit timestamp on the file's timeline, the first timestamp of
    the file being the reference (docs/design/mpegts.md, "Timestamps")."""
    floor = reference - WINDOW
    if WRAP - reference <= WINDOW:
        return raw - WRAP if raw >= floor else raw
    if floor >= 0:
        return raw + WRAP if raw < floor else raw
    return raw - WRAP if raw >= floor + WRAP else raw


# Stream ids whose PES packets have no optional header, so no timestamp
# and no header length (ISO/IEC 13818-1 table 2-18).
BARE_STREAM_IDS = frozenset({0xBC, 0xBE, 0xBF, 0xF0, 0xF1, 0xF2, 0xF8, 0xFF})


def pes_timestamp(b):
    return ((b[0] >> 1) & 7) << 30 | b[1] << 22 | (b[2] >> 1) << 15 | b[3] << 7 | b[4] >> 1


def adts_frame(data, at):
    """(length, samples, rate) of an ADTS frame at `at`, or None."""
    h = data[at:at + 7]
    if len(h) < 7 or h[0] != 0xFF or h[1] & 0xF6 != 0xF0:
        return None
    index = (h[2] >> 2) & 0x0F
    length = (h[3] & 3) << 11 | h[4] << 3 | h[5] >> 5
    if index >= len(AAC_RATES) or length < 7:
        return None
    return length, 1024 * ((h[6] & 3) + 1), AAC_RATES[index]


def eac3_ticks(data):
    """The 90 kHz duration of the E-AC-3 frame at the start of `data`."""
    if len(data) < 6 or data[0:2] != b'\x0b\x77' or data[5] >> 3 <= 10:
        return None
    fscod = data[4] >> 6
    if fscod == 3:
        rate, blocks = (48000, 44100, 32000)[(data[4] >> 4) & 3] // 2, 6
    else:
        rate, blocks = (48000, 44100, 32000)[fscod], (1, 2, 3, 6)[(data[4] >> 4) & 3]
    return 256 * blocks * 90000 // rate


def ac3_ticks(data):
    """The 90 kHz duration of the AC-3 frame at the start of `data`
    (ATSC A/52 5.3.2: 1,536 samples, `fscod` giving the rate); `None` for
    E-AC-3 (`bsid` above 10) and for bytes that are not a frame."""
    if len(data) < 6 or data[0:2] != b'\x0b\x77' or data[5] >> 3 > 10:
        return None
    fscod = data[4] >> 6
    if fscod == 3:
        return None
    return 1536 * 90000 // (48000, 44100, 32000)[fscod]


def mpeg_audio_ticks(data):
    """The 90 kHz duration of the MPEG audio frame at the start of `data`
    (ISO/IEC 11172-3 and 13818-3): 384 samples for layer I, 1,152 for
    layer II, and for layer III 1,152 at MPEG-1 rates, 576 below them."""
    if len(data) < 4 or data[0] != 0xFF or data[1] & 0xE0 != 0xE0:
        return None
    version, layer = (data[1] >> 3) & 3, (data[1] >> 1) & 3
    index = (data[2] >> 2) & 3
    if version == 1 or layer == 0 or index == 3:
        return None
    rate = (44100, 48000, 32000)[index]
    if version == 2:        # MPEG-2
        rate //= 2
    elif version == 0:      # MPEG-2.5
        rate //= 4
    samples = {3: 384, 2: 1152, 1: 1152 if version == 3 else 576}[layer]
    return samples * 90000 // rate


def opus_ticks(data):
    """The 90 kHz duration of the Opus access unit at the start of `data`,
    after its control header (Opus TS draft 6.2; RFC 6716 3.1)."""
    at = 0
    if len(data) >= 2 and data[0] == 0x7F and data[1] & 0xE0 == 0xE0:
        flags, at = data[1], 2
        while at < len(data) and data[at] == 0xFF:
            at += 1
        at += 1 + (2 if flags & 0x10 else 0) + (2 if flags & 0x08 else 0)
        if flags & 0x04 and at < len(data):
            at += 1 + data[at]
    if at >= len(data):
        return None
    toc = data[at]
    config, code = toc >> 3, toc & 3
    if config < 12:
        frame = (480, 960, 1920, 2880)[config % 4]
    elif config < 16:
        frame = (480, 960)[config % 2]
    else:
        frame = (120, 240, 480, 960)[config % 4]
    if code == 3:
        if at + 1 >= len(data):
            return None
        frames = data[at + 1] & 0x3F
    else:
        frames = 1 if code == 0 else 2
    return frame * frames * 90000 // 48000


class Bits:
    """Reads bits of a buffer, most significant first."""

    def __init__(self, data):
        self.data, self.at = data, 0

    def read(self, n):
        value = 0
        for _ in range(n):
            if self.at >= len(self.data) * 8:
                return None
            value = value << 1 | (self.data[self.at // 8] >> (7 - self.at % 8)) & 1
            self.at += 1
        return value


def latm_configuration(data):
    """The `audioObjectType` and sampling rate of the `StreamMuxConfig` of
    the first LOAS frame of `data` that carries one (ISO/IEC 14496-3 1.7.3
    and 1.6.2.1); `None` when no frame of it does."""
    at = 0
    while at + 3 <= len(data):
        if data[at] != 0x56 or data[at + 1] & 0xE0 != 0xE0:
            at += 1
            continue
        length = 3 + ((data[at + 1] & 0x1F) << 8 | data[at + 2])
        bits = Bits(data[at + 3:at + length])
        # useSameStreamMux, audioMuxVersion.
        if bits.read(1) == 0 and bits.read(1) == 0:
            bits.read(1 + 6 + 4 + 3)
            object_type = bits.read(5)
            if object_type == 31:
                more = bits.read(6)
                object_type = None if more is None else 32 + more
            index = bits.read(4)
            rate = bits.read(24) if index == 15 else (
                AAC_RATES[index] if index is not None and index < len(AAC_RATES) else None)
            if object_type is not None and rate is not None:
                return object_type, rate
        at += max(length, 1)
    return None


def adts_rate(data):
    """The sampling rate of the first ADTS frame of `data`."""
    for at in range(max(0, len(data) - 7)):
        frame = adts_frame(data, at)
        if frame is not None:
            return frame[2]
    return None


def adts_object_type(data):
    """The `profile` of the first ADTS frame of `data` as an audio object
    type (ISO/IEC 13818-7 table 32: `profile` + 1)."""
    for at in range(max(0, len(data) - 7)):
        if adts_frame(data, at) is not None:
            return (data[at + 2] >> 6) + 1
    return None


def mpeg_video(data):
    """('mpeg1video' or 'mpeg2video', frame period in 90 kHz ticks) from a
    sequence header and whether a sequence extension follows it."""
    at = data.find(b'\x00\x00\x01\xb3')
    if at < 0 or at + 8 > len(data):
        return None
    rate = MPEG_FRAME_RATES.get(data[at + 7] & 0x0F)
    extension = any(data[i + 4] >> 4 == 1 for i in range(at, len(data) - 4)
                    if data[i:i + 4] == b'\x00\x00\x01\xb5')
    if rate is None:
        return None
    return ('mpeg2video' if extension else 'mpeg1video'), Fraction(90000 * rate[1], rate[0])


def same_but_pcr(a, b):
    if a is None:
        return False
    def pcr(p):
        return p[3] & 0x20 and p[4] >= 7 and p[5] & 0x10
    if pcr(a) and pcr(b):
        return a[:6] == b[:6] and a[12:] == b[12:]
    return a == b


class Facts:
    """What a reader written apart from CoreAV's finds in one transport
    stream: the streams of its PMTs, each stream's first and last PES
    timestamps, the starts of its first and latest PES packets, the
    duplicate packets
    of each PID, and, for the PIDs asked for, every PES packet."""

    def __init__(self, path, keep=()):
        self.path = pathlib.Path(path)
        self.file_bytes = self.path.stat().st_size
        self.size, self.first = self._layout()
        self.streams = {}            # pid -> {'type', 'descriptors'}
        self.programs = set()        # the program numbers of the PAT
        self.program_pids = {}       # program number -> its elementary PIDs
        # Per PID, the first timestamp of its program, raw: a program
        # keeps a system time base of its own (13818-1 2.4.2.1).
        self.pid_reference = {}
        self.first_pts, self.last, self.head = {}, {}, {}
        # Every PES timestamp of each PID with the offset of the packet
        # that starts it, for the end CoreAV's rule gives from the tail.
        self.times = collections.defaultdict(list)
        # The first PES timestamp of each PID, recorded before any table
        # names it, as CoreAV records it.
        self.first_pes = {}      # pid -> (offset, raw timestamp)
        self.pids_seen = set()   # every PID with a payload
        self.pes_offsets = {}    # pid -> the offsets of its PES packets
        # pid -> {the offset of a PES packet: the (pts, dts) its header
        # states}, for the decoding time an entry checks against the file.
        self.stated = collections.defaultdict(dict)
        self.offsets_kept = 0
        self.aac_object = {}
        self.aac_rate = {}       # pid -> the sampling rate its configuration states
        self.duplicates = collections.defaultdict(list)
        self.pes = collections.defaultdict(list)
        self.reference = None
        self._scan(set(keep))

    def _layout(self):
        with open(self.path, 'rb') as f:
            head = f.read(1 << 20)
        for start in range(min(len(head), 1 << 20)):
            for size, prefix in ((188, 0), (192, 4), (204, 0)):
                positions = [start + prefix + k * size for k in range(3)]
                if all(p < len(head) and head[p] == 0x47 for p in positions):
                    return (size, prefix), start
        raise ValueError('not a transport stream')

    def _packets(self):
        (size, prefix), at = self.size, self.first
        with open(self.path, 'rb') as f:
            f.seek(at)
            while True:
                stored = f.read(size)
                if len(stored) < prefix + 188:
                    return
                packet = stored[prefix:prefix + 188]
                if packet[0] != 0x47:
                    # Lost sync: no fact is taken past it.
                    return
                yield at, packet
                at += size

    def _scan(self, keep):
        pmt_pids, sections, counters, building, previous = {0}, {}, {}, {}, {}
        for offset, p in self._packets():
            pid = (p[1] & 0x1F) << 8 | p[2]
            control = (p[3] >> 4) & 3
            start = 4
            discontinuity = False
            if control & 2:
                length = p[4]
                discontinuity = length > 0 and bool(p[5] & 0x80)
                start = 5 + length
            if not control & 1 or start >= 188 or pid == 0x1FFF:
                continue
            self.pids_seen.add(pid)
            counter = p[3] & 0x0F
            # A duplicate repeats the packet before it, its PCR aside
            # (13818-1 2.4.3.3); the same counter on other bytes is a loss.
            if counters.get(pid) == counter and not discontinuity and same_but_pcr(previous.get(pid), p):
                self.duplicates[pid].append(offset)
                continue
            counters[pid] = counter
            previous[pid] = p
            payload, unit_start = p[start:], bool(p[1] & 0x40)
            if pid in pmt_pids:
                if unit_start:
                    sections[pid] = payload[1 + payload[0]:]
                else:
                    sections[pid] = sections.get(pid, b'') + payload
                self._table(pid, sections[pid], pmt_pids)
                continue
            if unit_start and bytes(payload[:3]) == b'\x00\x00\x01' and self.offsets_kept < MAX_TIMES:
                # Where each PES packet of the PID starts, whether or not
                # a table has named it yet: the packets before its table
                # are the stream's too.
                self.pes_offsets.setdefault(pid, set()).add(offset)
                self.offsets_kept += 1
            if unit_start and pid not in self.first_pes:
                # The first timestamp of the PID, whether or not a table
                # names it yet: a stream starts at its first PES packet,
                # which commonly comes before the tables.
                head = bytes(payload)
                if (len(head) >= 14 and head[:3] == b'\x00\x00\x01'
                        and head[3] not in BARE_STREAM_IDS):
                    flags, at = head[7] >> 6, 9
                    if flags & 2:
                        self.first_pes[pid] = (offset, pes_timestamp(head[at:at + 5]))
                    elif flags == 1 and len(head) >= 19:
                        self.first_pes[pid] = (offset, pes_timestamp(head[at + 5:at + 10]))
            if pid not in self.streams:
                continue
            if unit_start:
                self._end(pid, building.pop(pid, None), keep)
                building[pid] = [offset, bytearray(payload)]
            elif pid in building:
                building[pid][1] += payload
        for pid, unit in building.items():
            self._end(pid, unit, keep)
        self._references()

    def _references(self):
        """The wrap reference of every PID of every program: the first
        timestamp of the earliest PES packet of any of the program's
        PIDs. A PID that two programs declare keeps the first one's."""
        for number in sorted(self.program_pids):
            pids = self.program_pids[number]
            found = [self.first_pes[pid] for pid in pids if pid in self.first_pes]
            if not found:
                continue
            _, time = min(found)
            for pid in pids:
                self.pid_reference.setdefault(pid, time)

    def _table(self, pid, data, pmt_pids):
        if len(data) < 3:
            return
        total = 3 + ((data[1] & 0x0F) << 8 | data[2])
        if len(data) < total:
            return
        body = data[8:total - 4]
        if data[0] == 0 and pid == 0:
            for i in range(0, len(body) - 3, 4):
                number = body[i] << 8 | body[i + 1]
                if number:
                    self.programs.add(number)
                    pmt_pids.add((body[i + 2] & 0x1F) << 8 | body[i + 3])
        elif data[0] == 2:
            number = data[3] << 8 | data[4]
            program = self.program_pids.setdefault(number, [])
            info = (body[2] & 0x0F) << 8 | body[3]
            at = 4 + info
            while at + 5 <= len(body):
                es = (body[at + 1] & 0x1F) << 8 | body[at + 2]
                length = (body[at + 3] & 0x0F) << 8 | body[at + 4]
                descriptors, d = [], body[at + 5:at + 5 + length]
                while len(d) >= 2:
                    descriptors.append((d[0], bytes(d[2:2 + d[1]])))
                    d = d[2 + d[1]:]
                self.streams.setdefault(es, {'type': body[at], 'descriptors': descriptors})
                if es not in program:
                    program.append(es)
                at += 5 + length

    def _end(self, pid, unit, keep):
        if unit is None:
            return
        offset, data = unit
        if len(data) < 9 or data[:3] != b'\x00\x00\x01':
            return
        if data[3] in BARE_STREAM_IDS:
            # No optional header: no timestamp, and the payload starts
            # after the six-byte header (13818-1 2.4.3.7).
            flags, header = 0, 6
        else:
            flags, header = data[7] >> 6, 9 + data[8]
        declared = data[4] << 8 | data[5]
        end = 6 + declared if declared else len(data)
        pts = pes_timestamp(data[9:14]) if flags & 2 and len(data) >= 14 else None
        dts = pes_timestamp(data[14:19]) if flags == 3 and len(data) >= 19 else None
        payload = bytes(data[header:end])
        if self.reference is None and (pts is not None or dts is not None):
            self.reference = dts if dts is not None else pts
        self.head.setdefault(pid, payload[:1 << 16])
        # The audio object type the stream's own configuration states,
        # from the first payload that carries one.
        if self.aac_object.get(pid) is None:
            kind = self.streams.get(pid, {}).get('type')
            if kind == 0x0F:
                self.aac_object[pid] = adts_object_type(payload)
                self.aac_rate[pid] = adts_rate(payload)
            elif kind == 0x11:
                found = latm_configuration(payload)
                self.aac_object[pid], self.aac_rate[pid] = found or (None, None)
        if pts is not None or dts is not None:
            self.first_pts.setdefault(pid, pts if pts is not None else dts)
        if pts is not None and len(self.times[pid]) < MAX_TIMES:
            self.times[pid].append((offset, pts))
        if pts is not None and len(self.stated[pid]) < MAX_TIMES:
            self.stated[pid][offset] = (pts, dts)
        if pts is not None:
            # The latest presentation time: with reordered frames, the last
            # PES packet in the file is not the last shown.
            previous = self.last.get(pid)
            if previous is None or self.timeline(pts) >= self.timeline(previous[1]):
                self.last[pid] = (offset, pts, payload[:1 << 16])
        if pid in keep:
            self.pes[pid].append((offset, pts, payload))

    def untimed(self, pid):
        """The file states no time for the stream on `pid`: a table
        declares it, or packets of it are there, and none of its PES
        packets states a presentation or a decoding time -- a stream the
        file declares and never sends among them. A PID neither declared
        nor seen is not untimed, it is unknown."""
        return ((pid in self.pids_seen or pid in self.streams)
                and pid not in self.first_pes)

    def absent_from_the_tail(self, pid):
        """The file times the stream, but nowhere in the last
        [`TAIL_BYTES`], which is all CoreAV reads for a stream's end: it
        has no end to report either."""
        if pid not in self.first_pes:
            return False
        floor = max(0, self.file_bytes - TAIL_BYTES)
        return not any(offset >= floor for offset, _ in self.times.get(pid, ()))

    def timeline(self, raw, pid=None):
        """`raw` on the timeline of the program of `pid`, the file's when
        no program declares it."""
        reference = self.pid_reference.get(pid)
        if reference is None:
            reference = self.reference if self.reference is not None else raw
        return unwrap(raw, reference)

    def codec(self, pid):
        """The codec name the file's tables and first bytes give."""
        stream = self.streams.get(pid)
        if stream is None:
            return None
        kind, tags = stream['type'], dict(stream['descriptors'])
        registration = tags.get(0x05, b'')[:4]
        if kind == 0x87 or (kind == 0x06 and (0x7A in tags or registration == b'EAC3')):
            return 'eac3'
        if kind == 0x06 and registration == b'Opus':
            return 'opus'
        if kind in (0x01, 0x02):
            found = mpeg_video(self.head.get(pid, b''))
            return found[0] if found else None
        if kind == 0x06 and (0x6A in tags or registration == b'AC-3'):
            return 'ac3'
        if kind == 0x06 and 0x56 in tags:
            return 'dvb_teletext'
        if kind == 0x06 and 0x59 in tags:
            return 'dvb_subtitle'
        return {0x03: 'mpeg_audio', 0x04: 'mpeg_audio', 0x0F: 'aac', 0x1B: 'h264', 0x24: 'hevc',
                0x81: 'ac3'}.get(kind)

    def frame_ticks(self, pid, payload):
        """The 90 kHz duration of the audio frame at the start of a PES
        payload of `pid`, for the codecs whose frames carry their length;
        `None` for the others and for a frame this reader cannot read."""
        codec = self.codec(pid)
        if codec == 'eac3':
            return eac3_ticks(payload)
        if codec == 'opus':
            return opus_ticks(payload)
        if codec == 'aac':
            frame = adts_frame(payload, 0)
            return None if frame is None else frame[1] * 90000 // frame[2]
        if codec == 'ac3':
            return ac3_ticks(payload)
        if codec == 'mpeg_audio':
            return mpeg_audio_ticks(payload)
        return None

    def end_kind(self, pid):
        """How the duration of the stream's last access unit is found
        (docs/design/mpegts.md, "Durations"): `frames` when its frames
        carry their length, `step` for a video or audio stream whose
        frames this reader cannot read, `none` for a subtitle, metadata or
        unnamed stream, whose units state what they last."""
        stream = self.streams.get(pid)
        if stream is None:
            return None
        kind, tags = stream['type'], dict(stream['descriptors'])
        registration = tags.get(0x05, b'')[:4]
        if kind in (0x01, 0x02, 0x1B, 0x20, 0x24, 0x33):
            return 'step'
        if kind in (0x03, 0x04, 0x0F, 0x81, 0x87):
            return 'frames'
        if kind == 0x11 or (kind == 0x80 and registration == b'HDMV'):
            # LATM, whose configuration may be out of band, and Blu-ray
            # LPCM: sound whose frames this reader does not read.
            return 'step'
        if kind == 0x06:
            if 0x7A in tags or registration == b'EAC3' or 0x6A in tags or registration == b'AC-3':
                return 'frames'
            if registration == b'Opus':
                return 'frames'
        return 'none'

    def coreav_end(self, pid):
        """(start, end) for the stream on `pid` by CoreAV's rule: from its
        first PES timestamp to the latest PES timestamp in the last
        [`TAIL_BYTES`] of the file plus the duration of the unit that
        timestamp times. `None` when this reader cannot work it out, and
        when the tail holds no timestamp of the stream."""
        kind = self.end_kind(pid)
        if kind is None or pid not in self.first_pes:
            return None
        floor = max(0, self.file_bytes - TAIL_BYTES)
        tail = [(offset, self.timeline(raw, pid)) for offset, raw in self.times.get(pid, [])
                if offset >= floor]
        if not tail:
            return None
        last = max(time for _, time in tail)
        if kind == 'none':
            unit = 0
        elif kind == 'step':
            ordered = sorted({time for _, time in tail})
            steps = [b - a for a, b in zip(ordered, ordered[1:]) if b > a]
            if not steps:
                return None
            unit = min(steps)
        else:
            # The frame the latest timestamp times: the first of the PES
            # packet that states it.
            latest = self.last.get(pid)
            if latest is None or latest[0] < floor:
                return None
            unit = self.frame_ticks(pid, latest[2])
            if unit is None:
                return None
        return self.timeline(self.first_pes[pid][1], pid), last + unit

    def end_unit(self, pid):
        """(codec, start, last PES PTS, the duration of the unit it times),
        on the file's timeline; None when the file does not give them."""
        codec, last = self.codec(pid), self.last.get(pid)
        if codec is None or last is None or pid not in self.first_pts:
            return None
        payload = last[2]
        if codec == 'eac3':
            unit = eac3_ticks(payload)
        elif codec == 'opus':
            unit = opus_ticks(payload)
        elif codec in ('mpeg1video', 'mpeg2video'):
            found = mpeg_video(self.head.get(pid, b''))
            unit = found[1] if found else None
        else:
            unit = None
        if unit is None:
            return None
        return codec, self.timeline(self.first_pts[pid]), self.timeline(last[1]), unit

    def units(self, pid):
        """Every packet CoreAV's rules make of the PID's PES packets, from
        the file: (pos, size, pts, dts, keyframe), for ADTS AAC, split into
        frames (a frame cut by the end of a PES packet is finished in the
        next one; the first frame that starts in a PES packet takes its PTS
        and position, the others the samples since the last stated time
        after it, rounded once; every frame is a keyframe, and its DTS is
        its PTS less the distance the PES packet that times it states
        between the two). None for other codecs, whose keyframes this
        reader does not read."""
        codec = self.codec(pid)
        out = []
        if codec == 'aac':
            carried, anchor, following, ahead = b'', None, None, 0
            for offset, pts, payload in self.pes.get(pid, []):
                data, at, first = carried + payload, 0, True
                while at < len(data):
                    frame = adts_frame(data, at)
                    if frame is None:
                        rest = data[at:]
                        # The start of a header the end of the PES packet
                        # cuts: it goes on in the next one.
                        if (len(rest) < 7 and rest[0] == 0xFF
                                and (len(rest) < 2 or rest[1] & 0xF6 == 0xF0)):
                            break
                        at += 1
                        continue
                    length, samples, rate = frame
                    if at + length > len(data):
                        break
                    stated = None
                    if at >= len(carried) and first:
                        first = False
                        stated = self.timeline(pts, pid) if pts is not None else None
                        pos = offset
                    else:
                        pos = None
                    # A stated time anchors the run; a frame without one
                    # starts the samples since the anchor after it, rounded
                    # once (docs/design/mpegts.md, "Packets").
                    if stated is not None:
                        anchor, time = [stated, 0, rate], stated
                        # How far ahead of its decoding time the PES
                        # packet that times this run states it is; the
                        # frames of the run keep it.
                        times = self.stated.get(pid, {}).get(offset)
                        ahead = (0 if times is None or times[1] is None
                                 else self.timeline(times[0], pid) - self.timeline(times[1], pid))
                    elif anchor is not None:
                        time = anchor[0] + (anchor[1] * 90000 + anchor[2] // 2) // anchor[2]
                    else:
                        time = following
                    if anchor is not None:
                        anchor[1] += samples
                    following = None if time is None else time + samples * 90000 // rate
                    out.append((pos, length, time, None if time is None else time - ahead,
                                True, stated is not None))
                    at += length
                carried = data[at:]
            if carried and adts_frame(carried, 0):
                out.append((None, len(carried), following,
                            None if following is None else following - ahead, True, False))
            return out
        return None


_FACTS = {}


def facts_of(media_dir, file, keep=()):
    """The file's facts, read once per file and PID set; None without a
    media directory or for a file the reader cannot read."""
    if not media_dir:
        return None
    key = (file, tuple(sorted(keep)))
    if key not in _FACTS:
        try:
            _FACTS[key] = Facts(pathlib.Path(media_dir) / file, keep)
        except (OSError, ValueError, IndexError):
            _FACTS[key] = None
    return _FACTS[key]


# ---------------------------------------------------------------- predicates

def end_estimate(rstream, cstream, facts):
    """The reference ends the stream that fraction of its last access unit
    earlier, and CoreAV's duration is the file's: from the stream's first
    timestamp to its latest PES timestamp plus the unit that timestamp
    times, each read from the file (`Facts.end_unit`)."""
    if not rstream or not cstream or facts is None:
        return False
    found = facts.end_unit(props(cstream).get('pid'))
    if found is None:
        return False
    codec, start, last, unit = found
    fraction = END_FRACTION.get(codec)
    r, c = rstream.get('duration_ts'), cstream.get('duration_ts')
    if fraction is None or codec != cstream.get('codec_name') or r is None or c is None or unit <= 0:
        return False
    return c == last + unit - start and Fraction(c - r) == unit * Fraction(fraction)


_ENDS = {}


def reference_ends(reference_packets, index):
    """Every time the reference's own packet list gives as the end of an
    access unit of stream `index`: a packet's presentation or decoding
    time plus its duration. Read once per stream, keyed by the file the
    document names."""
    file = reference_packets.get('file')
    key = (file, index)
    if file is not None and key in _ENDS:
        return _ENDS[key]
    stream = stream_at(reference_packets, index)
    ends = set()
    for packet in (stream or {}).get('packets') or ():
        # A packet the reference gives no duration is a unit that lasts
        # nothing of its own (a subtitle, an identifier): it ends where it
        # starts.
        duration = packet.get('duration') or 0
        for which in ('pts', 'dts'):
            time = packet.get(which)
            if time is not None:
                ends.add(time + duration)
    if file is None:
        # A document a caller built, not a capture of a file: it has no
        # name to key a cache on.
        return ends
    _ENDS[key] = ends
    return ends


def coreav_duration_is_the_file_s(cstream, facts):
    """CoreAV's duration for the stream runs from its first PES timestamp
    to the latest one in the file's tail plus the unit that timestamp
    times, each read from the file (`Facts.coreav_end`)."""
    if facts is None or not cstream:
        return None
    found = facts.coreav_end(props(cstream).get('pid'))
    start = props(cstream).get('start_pts')
    if found is None or start is None:
        return None
    file_start, file_end = found
    if start != file_start:
        return None
    return file_end - file_start


def end_at_another_unit(rstream, cstream, facts, packets, index):
    """`ffprobe` ends the stream at an access unit of its own list, not at
    the one CoreAV ends it at; and CoreAV's duration is the one the file
    gives. It reads less of the file's tail, so its unit is commonly an
    earlier one, and it takes the unit's duration from the frame rate it
    reports rather than from the step between the file's timestamps, so
    the two can differ by the rounding of that rate."""
    if not rstream or not cstream:
        return False
    ours = coreav_duration_is_the_file_s(cstream, facts)
    theirs = rstream.get('duration_ts')
    start = props(cstream).get('start_pts')
    if ours is None or theirs is None or cstream.get('duration_ts') != ours:
        return False
    return theirs >= 0 and start + theirs in reference_ends(packets, index)


def timed_from_the_file(rstream, cstream, facts, reference):
    """`ffprobe` does not time the stream from its own access units: it
    reports no duration at all, or the duration it reports for the file.
    CoreAV's is the one the file gives, or none where the file states no
    time for the stream at all -- which the reader must show, not merely
    fail to work out."""
    if facts is None or not rstream or not cstream:
        return False
    ours = coreav_duration_is_the_file_s(cstream, facts)
    if ours is None:
        pid = props(cstream).get('pid')
        # CoreAV gives no duration only where the file gives it nothing to
        # end on: no time at all for the stream, or none in the tail it
        # reads. The reader must show that, not merely fail to work it out.
        if cstream.get('duration_ts') is not None or not (
                facts.untimed(pid) or facts.absent_from_the_tail(pid)):
            return False
    elif cstream.get('duration_ts') != ours:
        return False
    theirs = rstream.get('duration_ts')
    if theirs is None:
        return True
    whole = reference.get('format_duration_us')
    if whole is None:
        return False
    return abs(theirs * 1_000_000 - whole * 90_000) <= 1_000_000


def stream_duration(rstream, cstream, facts, packets, index, reference):
    """The entries that cover a stream's `duration_ts`: the end estimate,
    whose fraction of the last unit `ffprobe` leaves out; its reading less
    of the file's tail; and its timing a stream from the file instead of
    from the stream."""
    if end_estimate(rstream, cstream, facts) or end_at_another_unit(
            rstream, cstream, facts, packets, index):
        return 'KD-TS-END-ESTIMATE'
    if timed_from_the_file(rstream, cstream, facts, reference):
        return 'KD-TS-STREAM-TIMING'
    return None


# The profile `ffprobe` names an AAC audio object type (ISO/IEC 14496-3
# 1.5.1.1), as CoreAV names it from the stream's configuration.
AAC_PROFILES = {1: 'Main', 2: 'LC', 3: 'SSR', 4: 'LTP', 5: 'HE-AAC', 29: 'HE-AACv2'}


def aac_profile(rstream, cstream, facts):
    """`ffprobe` names the stream HE-AAC or HE-AACv2 because its decoder
    finds spectral band replication in the payload; CoreAV names the
    profile the stream's own configuration states, which is what it reads
    without decoding."""
    if facts is None or not rstream or not cstream:
        return False
    stated = facts.aac_object.get(props(cstream).get('pid'))
    if stated is None or AAC_PROFILES.get(stated) != cstream.get('profile'):
        return False
    return rstream.get('profile') in ('HE-AAC', 'HE-AACv2')


# The fields the comparator gates a stream on (tools/oracle/src/compare.rs).
GATING = ('kind', 'codec_fourcc', 'codec_name', 'width', 'height', 'rotation', 'timescale',
          'duration_ts', 'color_primaries', 'color_transfer', 'color_matrix', 'color_range',
          'profile', 'level')
# The picture `ffprobe` gives a DVB teletext stream, which is the page its
# own decoder lays out, not anything the file states.
TELETEXT_PAGE = (492, 250)


def teletext_size(rstream, cstream, facts):
    """`ffprobe` gives a DVB teletext stream the size of the page its
    decoder lays out; the file states no picture for it, so CoreAV gives
    none."""
    if facts is None or not rstream or not cstream:
        return False
    if facts.codec(props(cstream).get('pid')) != 'dvb_teletext':
        return False
    if cstream.get('width') is not None or cstream.get('height') is not None:
        return False
    return (rstream.get('width'), rstream.get('height')) == TELETEXT_PAGE


def fingerprint(stream):
    """What tells one stream of a file from another: the position and size
    of each of its packets."""
    return tuple((p.get('pos'), p.get('size')) for p in stream.get('packets') or ())


def packets_of_their_pid(cstreams, cpackets, facts):
    """Every packet CoreAV lists for a stream starts in a PES packet of
    the PID it gives that stream, which the file shows
    (`Facts.pes_offsets`). Without it a permutation found from the packets
    themselves would pair two streams whose packets CoreAV had swapped."""
    if facts is None:
        return False
    shift = 16 if facts.size[0] == 204 else 0
    for index, cstream in enumerate(cstreams.get('streams') or ()):
        pid = props(cstream).get('pid')
        offsets = facts.pes_offsets.get(pid)
        listed = [p.get('pos') for p in (stream_at(cpackets, index) or {}).get('packets') or ()
                  if p.get('pos') is not None]
        if not listed:
            # A stream with no position of its own: the Event Information
            # Table, or one whose packets the file does not place.
            continue
        if not offsets or any(pos + shift not in offsets for pos in listed):
            return False
    return True


def stream_permutation(reference, candidate, rnames=(), cnames=(), cpids=()):
    """From each candidate stream to the reference stream that lists the
    same packets, found from the packets themselves: the positions and
    sizes of a stream's packets are a fingerprint no other stream of the
    file shares. `rnames` and `cnames` are the codec names the two sides
    give their streams and `cpids` the PIDs the candidate gives its own,
    which the packet captures do not carry. `None` when the two sides do
    not list the same streams."""
    rstreams = reference.get('streams') or []
    cstreams = candidate.get('streams') or []
    if not rstreams or len(rstreams) != len(cstreams):
        return None
    by = {}
    for index, stream in enumerate(rstreams):
        by.setdefault(fingerprint(stream), []).append(index)
    order, left = {}, set(range(len(rstreams)))
    for index, stream in enumerate(cstreams):
        found = by.get(fingerprint(stream))
        if found:
            order[index] = found.pop(0)
            left.discard(order[index])
    # The Event Information Table names itself: it is one table at one
    # PID the standard fixes (ETSI EN 300 468 5.2.4), listed by both
    # readers as the `epg` stream, so it needs no fingerprint to be told
    # from the elementary streams -- which it needs, since the two
    # readers list different numbers of its sections. The candidate's
    # must be that PID: a stream CoreAV named `epg` at any other is not
    # the table, and pairs by its packets like the rest.
    def epg(names, i):
        return i < len(names) and names[i] == 'epg'

    ours = [i for i in range(len(cstreams))
            if i not in order and epg(cnames, i)
            and i < len(cpids) and cpids[i] == EIT_PID]
    theirs = [i for i in sorted(left) if epg(rnames, i)]
    if len(ours) == 1 and len(theirs) == 1:
        order[ours[0]] = theirs[0]
        left.discard(theirs[0])
    # Streams neither side lists a packet for pair in order; a candidate
    # stream with packets that no reference stream lists is not a
    # reordering of the reference's streams.
    for index in range(len(cstreams)):
        if index in order:
            continue
        if fingerprint(cstreams[index]):
            return None
        at = next((k for k in sorted(left) if not fingerprint(rstreams[k])), None)
        if at is None:
            return None
        order[index] = at
        left.discard(at)
    return order


def listed_in_table_order(candidate, facts):
    """CoreAV lists the streams the Program Map Tables declare in the
    order they declare them, and nothing else but PIDs no table declares:
    the Event Information Table, and PIDs the file carries PES packets on
    (`Facts.pes_offsets`). The standard places neither, and the entry
    needs no more of the reference than the permutation it is checked
    against."""
    pids = [props(s).get('pid') for s in candidate.get('streams') or ()]
    if any(pid is None for pid in pids):
        return False
    listed = set(pids)
    if len(pids) != len(listed):
        return False
    declared = set(facts.streams)
    if [pid for pid in pids if pid in declared] != [pid for pid in facts.streams if pid in listed]:
        return False
    return all(pid == EIT_PID or pid in facts.pes_offsets
               for pid in pids if pid not in declared)


def ordered_differently(reference, candidate, facts, rpackets, cpackets):
    """The two sides list the same streams of a file of more than one
    program in a different order: the permutation, else `None`. CoreAV's
    order is the order the file's tables declare them in."""
    if facts is None or len(facts.programs) <= 1 or not listed_in_table_order(candidate, facts):
        return None
    if not packets_of_their_pid(candidate, cpackets, facts):
        return None
    order = stream_permutation(
        rpackets, cpackets,
        [s.get('codec_name') for s in reference.get('streams') or ()],
        [s.get('codec_name') for s in candidate.get('streams') or ()],
        [props(s).get('pid') for s in candidate.get('streams') or ()],
    )
    if order is None or all(index == at for index, at in order.items()):
        return None
    return order


def order_covers_streams(reference, candidate, facts, rpackets, order):
    """Every stream field agrees under the permutation, or is covered by
    another entry; the ids of the entries used, else `None`."""
    ids = {'KD-TS-STREAM-ORDER'}
    durations = False
    for index, cstream in enumerate(candidate.get('streams') or ()):
        rstream = stream_at(reference, order[index])
        if rstream is None:
            return None
        for field in GATING:
            if cstream.get(field) == rstream.get(field):
                continue
            if field == 'duration_ts':
                entry = stream_duration(rstream, cstream, facts, rpackets, order[index], reference)
            elif field == 'profile':
                entry = 'KD-TS-AAC-PROFILE' if aac_profile(rstream, cstream, facts) else None
            elif field in ('width', 'height'):
                entry = 'KD-TS-TELETEXT-SIZE' if teletext_size(rstream, cstream, facts) else None
            else:
                entry = None
            if not entry:
                return None
            if field == 'duration_ts':
                durations = True
            ids.add(entry)
    if candidate.get('format_duration_us') != reference.get('format_duration_us'):
        # The file's duration follows from its streams', so a stream a
        # documented divergence covers moves it too; it is covered by the
        # same rule as outside a permutation, with the streams paired as
        # the permutation pairs them.
        if not (durations and format_from_streams(reference, candidate, facts, rpackets, order)):
            return None
        ids.add('KD-TS-END-ESTIMATE')
    return ids


def packets_differing_under(reference, candidate, order):
    """How many packets still differ when each candidate stream is read
    against the reference stream the permutation gives it."""
    differing = 0
    for index, cstream in enumerate(candidate.get('streams') or ()):
        rstream = stream_at(reference, order[index]) or {}
        ours = [(p.get('pos'), p.get('size'), p.get('pts'), p.get('dts'), p.get('keyframe'))
                for p in cstream.get('packets') or ()]
        theirs = [(p.get('pos'), p.get('size'), p.get('pts'), p.get('dts'), p.get('keyframe'))
                  for p in rstream.get('packets') or ()]
        differing += sum(1 for a, b in zip(ours, theirs) if a != b) + abs(len(ours) - len(theirs))
    return differing


# The most a reference packet of a PES payload holds, measured on the
# real transport streams of `corpus/v1-ts-audio`: an access unit that
# starts past this many bytes of its PES payload carries a position on
# the reference's side and none on CoreAV's.
PIECE = 200 << 10


def pes_places(pid, facts, sizes):
    """Where each access unit of `sizes` starts: (the offset of the PES
    packet it starts in, how far into that packet's payload). The units
    of a stream tile the payloads of its PES packets one after another.
    `None` when the sizes do not tile them."""
    packets = facts.pes.get(pid)
    if not packets:
        return None
    places, at, index, base = [], 0, 0, 0
    for size in sizes:
        while index < len(packets) and at >= base + len(packets[index][2]):
            base += len(packets[index][2])
            index += 1
        if index >= len(packets):
            return None
        places.append((packets[index][0], at - base))
        at += size
    total = sum(len(payload) for _, _, payload in packets)
    return places if at == total else None


def read_in_pieces(cstream, rstream, facts):
    """`ffprobe` reads a PES packet in pieces of at most 200 KiB and gives
    the first access unit of each piece the position of the piece, so a
    unit that starts past that many bytes of its PES payload carries one
    where CoreAV, which gives a position to the first unit of a PES packet
    only, gives none. Nothing else of the two lists differs."""
    if facts is None or not cstream or not rstream:
        return False
    ours = [(p.get('pos'), p.get('size'), p.get('pts'), p.get('dts'), p.get('keyframe'))
            for p in cstream.get('packets') or ()]
    theirs = [(p.get('pos'), p.get('size'), p.get('pts'), p.get('dts'), p.get('keyframe'))
              for p in rstream.get('packets') or ()]
    if not ours or len(ours) != len(theirs):
        return False
    pid = props(cstream).get('pid')
    if facts.codec(pid) not in ('h264', 'hevc'):
        return False
    places = pes_places(pid, facts, [unit[1] for unit in ours])
    if places is None:
        return False
    shift = 16 if facts.size[0] == 204 else 0
    extra = False
    for unit, other, (offset, into) in zip(ours, theirs, places):
        if unit[1:] != other[1:]:
            return False
        start = None if into else offset - shift
        if unit[0] != start:
            return False
        if other[0] == unit[0]:
            continue
        # The reference gives a position CoreAV does not: the unit starts
        # in a piece of its own.
        if other[0] != offset - shift or into < PIECE:
            return False
        extra = True
    return extra


def file_span(candidate, starts, ends):
    """The file's duration by CoreAV's rule: the span of its streams, or,
    for a file of more than one program, the longest span within one
    program, since each program keeps a system time base of its own."""
    programs = {number for s in candidate.get('streams') or ()
                for number in props(s).get('programs') or ()}
    if len(programs) <= 1:
        return span_us(starts, ends)
    spans = []
    for number in programs:
        of = [(props(s).get('start_pts'), s.get('duration_ts'))
              for s in candidate.get('streams') or ()
              if number in (props(s).get('programs') or ())]
        timed = [(start, start + duration) for start, duration in of
                 if start is not None and duration is not None]
        if timed:
            spans.append(span_us([start for start, _ in timed], [end for _, end in timed]))
    return max(spans) if spans else None


def span_us(starts, ends):
    return ((max(ends) - min(starts)) * 1_000_000 + 45_000) // 90_000


def format_from_streams(reference, candidate, facts, packets, order=None):
    """Every stream's duration agrees or is covered by an entry, CoreAV's
    format duration is the span of its own streams, and the reference's is
    the span of the streams it times. The two can differ by a microsecond
    of rounding even where every stream agrees: CoreAV takes the span in
    90 kHz ticks and converts it once, the reference works in
    microseconds. `order` pairs the streams where the two sides list them
    in a different order (KD-TS-STREAM-ORDER)."""
    starts, ends, their_starts, their_ends = [], [], [], []
    for index, cstream in enumerate(candidate.get('streams', [])):
        at = order[index] if order is not None else index
        rstream = stream_at(reference, at)
        start = props(cstream).get('start_pts')
        ours, duration = cstream.get('duration_ts'), (rstream or {}).get('duration_ts')
        if ours != duration and not stream_duration(rstream, cstream, facts, packets, at, reference):
            return False
        if start is not None:
            their_starts.append(start)
            if ours is not None:
                starts.append(start)
                ends.append(start + ours)
            if duration is not None:
                their_ends.append(start + duration)
    if not starts or not their_ends:
        return False
    # A file of several programs lasts as long as its longest program, not
    # the span of every stream (docs/design/mpegts.md, "Durations").
    ours_span = file_span(candidate, starts, ends)
    if ours_span is None:
        return False
    # The reference reports the file's duration in microseconds and the
    # streams' in 90 kHz ticks, so its span can be a tick either way.
    their_span = reference.get('format_duration_us')
    return (candidate.get('format_duration_us') == ours_span
            and their_span is not None
            and abs(their_span - span_us(their_starts, their_ends)) <= 1_000_000 // 90_000 + 1)


def after_duplicate(cstream, diffs, facts):
    """The packet differs in or after the PES packet that holds a duplicate
    transport packet of its stream, which the file shows (`Facts`) and
    CoreAV discarded, and every packet CoreAV lists for the stream is the
    one the file gives without the duplicate (`Facts.units`)."""
    pid = props(cstream).get('pid')
    if facts is None or not facts.duplicates.get(pid):
        return False
    expected = listed_as_the_file_gives(cstream, facts)
    if expected is None:
        return False
    first = min(facts.duplicates[pid])
    starts = [unit[0] for unit in expected if unit[0] is not None and unit[0] <= first]
    begin = max(starts, default=0)
    positions = [p for d in diffs for side in ('reference', 'candidate')
                 for p in [(d.get(side) or {}).get('pos') if isinstance(d.get(side), dict) else None]
                 if p is not None]
    return not positions or begin <= max(positions)


_LISTED = {}


def file_units(cstream, facts):
    """The stream's packets as the file gives them (`Facts.units`), with
    the positions the oracle's vocabulary uses; `None` for a codec the
    reader does not read. Read once per stream: a file of a hundred
    thousand packets is judged unit by unit."""
    if facts is None:
        return None
    key = (facts.path, props(cstream).get('pid'))
    if key not in _LISTED:
        units = facts.units(props(cstream).get('pid'))
        shift = 16 if facts.size[0] == 204 else 0
        _LISTED[key] = (
            None
            if units is None
            else [(None if pos is None else pos - shift, *rest) for pos, *rest in units]  # noqa: E501
        )
    return _LISTED[key]


def listed_as_the_file_gives(cstream, facts):
    """Every packet CoreAV lists for the stream (position, size,
    timestamps, keyframe flag) is the one the file gives; the file's units
    when so, else None."""
    expected = file_units(cstream, facts)
    if expected is None:
        return None
    listed = [(p.get('pos'), p.get('size'), p.get('pts'), p.get('dts'), p.get('keyframe'))
              for p in cstream.get('packets', [])]
    return expected if listed == [unit[:5] for unit in expected] else None


def stated_decode_time(cstream, diffs, facts):
    """The packet is a coded picture whose PES packet states no decoding
    time of its own -- none at all, or one equal to the presentation time
    -- so that ISO/IEC 13818-1 2.7.5 makes the two the same; CoreAV
    reports the time the file states and the reference one of its own,
    reconstructed from the order the pictures are coded in. The two sides
    differ in the decoding time alone, and CoreAV's is the file's own:
    the packet starts at a PES packet of the stream's PID
    (`Facts.stated`) whose header states that presentation time."""
    pid = props(cstream).get('pid')
    if facts is None or facts.codec(pid) not in ('h264', 'hevc'):
        return False
    stated = facts.stated.get(pid) or {}
    listed = cstream.get('packets', [])
    for d in diffs:
        parts = d['path'].split('/')
        if len(parts) != 5 or parts[4] != 'dts':
            return False
        index = int(parts[3])
        packet = listed[index] if 0 <= index < len(listed) else None
        if packet is None:
            return False
        # The file states the time, for this unit: it is the first access
        # unit of the PES packet at that position (13818-1 2.4.3.7).
        times = stated.get(packet.get('pos'))
        if times is None:
            return False
        pts, dts = times
        if dts is not None and dts != pts:
            return False
        ours = d.get('candidate')
        if ours != packet.get('pts') or ours != packet.get('dts'):
            return False
        if facts.timeline(pts, pid) != ours:
            return False
        # The reference reports no decoding time, or an earlier one: a
        # later one is not a reordering of the pictures.
        theirs = d.get('reference')
        if theirs is not None and not (isinstance(theirs, int) and theirs < ours):
            return False
    return bool(diffs)


def frame_time(cstream, diffs, facts):
    """The packet is an audio frame the file states no time for -- not the
    first frame that starts in its PES packet -- the two sides differ
    only in its timestamps, and by at most one 90 kHz tick or, on the
    presentation time alone, by the distance the file states between the
    stream's two times, which the reference does not carry to a frame it
    times itself. The packet CoreAV lists at that place is the one the
    file gives (`Facts.units`): its position, size, timestamps and
    keyframe flag."""
    expected = file_units(cstream, facts)
    if expected is None:
        return False
    listed = cstream.get('packets', [])
    for d in diffs:
        parts = d['path'].split('/')
        if len(parts) != 5 or parts[4] not in ('pts', 'dts'):
            return False
        index = int(parts[3])
        unit = expected[index] if 0 <= index < len(expected) else None
        packet = listed[index] if 0 <= index < len(listed) else None
        if unit is None or packet is None:
            return False
        # The packet is the one the file gives at that place, and the file
        # states no time for it: it is not the first frame that starts in
        # a PES packet, or that packet's header carries no timestamp.
        ours = (packet.get('pos'), packet.get('size'), packet.get('pts'), packet.get('dts'),
                packet.get('keyframe'))
        if unit[5] or ours != unit[:5]:
            return False
        theirs = d.get('reference')
        mine = d.get('candidate')
        if not isinstance(mine, int) or not isinstance(theirs, int):
            return False
        if mine != unit[2 if parts[4] == 'pts' else 3]:
            return False
        if abs(mine - theirs) > 1:
            # The reference does not carry to a frame it times itself the
            # distance the file states between the stream's presentation
            # and decoding times: it reports the decoding time for both.
            if parts[4] != 'pts' or theirs != packet.get('dts') or theirs == mine:
                return False
    return bool(diffs)


def aac_sample_rate(media_dir, name, index, theirs, ours):
    """A compat key the stream oracle does not gate: `ffprobe` reports
    twice the configured sampling rate for an AAC stream whose payload
    carries spectral band replication, which its decoder finds and this
    reader does not (KD-TS-AAC-PROFILE)."""
    if not theirs or not ours:
        return False
    text = str(ours.get('id', ''))
    if not text.startswith('0x'):
        return False
    try:
        pid = int(text, 16)
        mine, other = int(ours.get('sample_rate')), int(theirs.get('sample_rate'))
    except (TypeError, ValueError):
        return False
    facts = facts_of(media_dir, name)
    if facts is None or facts.aac_object.get(pid) != 2:
        return False
    # CoreAV's rate is the one the file's own configuration states; the
    # reference's is twice it, which band replication doubles.
    if facts.aac_rate.get(pid) != mine:
        return False
    return theirs.get('profile') in ('HE-AAC', 'HE-AACv2') and other == 2 * mine


# Compat keys the stream oracle does not gate, with the entry that covers
# them and the predicate that reads the file (scripts/compat_check.py).
COMPAT_ENTRIES = {'streams.sample_rate': ('KD-TS-AAC-PROFILE', aac_sample_rate)}


PREDICATES = {
    'end_estimate': end_estimate,
    'end_at_another_unit': end_at_another_unit,
    'timed_from_the_file': timed_from_the_file,
    'stream_duration': stream_duration,
    'aac_profile': aac_profile,
    'teletext_size': teletext_size,
    'read_in_pieces': read_in_pieces,
    'aac_sample_rate': aac_sample_rate,
    'ordered_differently': ordered_differently,
    'packets_of_their_pid': packets_of_their_pid,
    'format_from_streams': format_from_streams,
    'after_duplicate': after_duplicate,
    'frame_time': frame_time,
    'stated_decode_time': stated_decode_time,
}


# ---------------------------------------------------------------- overlay

def ordered_paths(reference, candidate, order, ids):
    """The field paths an order-covered item carries for the compat check:
    every stream field, with CoreAV's value and the reference's under the
    permutation. The comparison's own paths are of the identity pairing,
    which for such a file names the wrong stream."""
    paths = {}
    for index, cstream in enumerate(candidate.get('streams') or ()):
        rstream = stream_at(reference, order[index]) or {}
        for field in GATING:
            if cstream.get(field) != rstream.get(field):
                paths[f'streams/{index}/{field}'] = {
                    'kd': sorted(ids), 'candidate': cstream.get(field),
                    'reference': rstream.get(field)}
    if candidate.get('format_duration_us') != reference.get('format_duration_us'):
        paths['format_duration_us'] = {
            'kd': sorted(ids), 'candidate': candidate.get('format_duration_us'),
            'reference': reference.get('format_duration_us')}
    return paths


def stream_item(detail, reference, candidate, media_dir, packets, candidate_packets):
    ids, uncovered = set(), []
    facts = facts_of(media_dir, detail['file'])
    # The two sides may list the same streams in a different order, which
    # makes every field of every stream after the first difference
    # disagree; then the whole item is judged under the permutation.
    order = ordered_differently(reference, candidate, facts, packets, candidate_packets)
    if order is not None:
        found = order_covers_streams(reference, candidate, facts, packets, order)
        if found is not None:
            return 'kd', found, [], ordered_paths(reference, candidate, order, found)
    for diff in detail['gating_differences']:
        path = diff['path']
        parts = path.split('/')
        if path == 'format_duration_us':
            if format_from_streams(reference, candidate, facts, packets):
                ids.add('KD-TS-END-ESTIMATE')
                continue
        elif len(parts) == 3 and parts[0] == 'streams' and parts[2] in ('width', 'height'):
            index = int(parts[1])
            if teletext_size(stream_at(reference, index), stream_at(candidate, index), facts):
                ids.add('KD-TS-TELETEXT-SIZE')
                continue
        elif len(parts) == 3 and parts[0] == 'streams' and parts[2] == 'profile':
            index = int(parts[1])
            if aac_profile(stream_at(reference, index), stream_at(candidate, index), facts):
                ids.add('KD-TS-AAC-PROFILE')
                continue
        elif len(parts) == 3 and parts[0] == 'streams' and parts[2] == 'duration_ts':
            index = int(parts[1])
            entry = stream_duration(stream_at(reference, index), stream_at(candidate, index),
                                    facts, packets, index, reference)
            if entry:
                ids.add(entry)
                continue
        uncovered.append(path)
    return ('kd' if not uncovered else 'uncovered'), ids, uncovered, None


def packet_units(detail, candidate, media_dir, reference_streams, candidate_streams,
                 reference_packets):
    disagree = detail['packets_disagree']
    diffs = detail['gating_differences']
    facts = facts_of(media_dir, detail['file'])
    # The same streams in a different order: every packet of a stream the
    # permutation pairs is the other side's, so the order explains every
    # disagreement but those that are left under it.
    order = ordered_differently(reference_streams, candidate_streams, facts, reference_packets,
                                candidate)
    if order is not None and order_covers_streams(
            reference_streams, candidate_streams, facts, reference_packets, order) is not None:
        left = packets_differing_under(reference_packets, candidate, order)
        if left <= disagree:
            return ({'KD-TS-STREAM-ORDER': disagree - left}, left,
                    [f'{left} packets differ under the stream order'] if left else [])
    units = collections.defaultdict(list)
    extra_index = collections.Counter()
    for d in diffs:
        parts = d['path'].split('/')
        if len(parts) >= 4 and parts[0] == 'streams' and parts[2] == 'packets':
            stream = int(parts[1])
            if parts[3] in ('extra', 'missing'):
                extra_index[(stream, parts[3])] += 1
                units[(stream, parts[3], extra_index[(stream, parts[3])])].append(d)
            else:
                units[(stream, 'paired', int(parts[3]))].append(d)
        else:
            return {}, disagree, [d['path']]
    cstreams = candidate.get('streams', [])
    pids = {props(s).get('pid') for s in cstreams}
    duplicated = facts_of(media_dir, detail['file'])
    # The reader keeps the PES packets of the PIDs an entry needs to list
    # the stream's units itself: those with a duplicate packet, and those
    # of a stream whose packets differ.
    differing = {props(cstreams[i]).get('pid') for (i, _, _) in units if i < len(cstreams)}
    keep = sorted(
        pid
        for pid in pids
        if pid is not None
        and (pid in differing or (duplicated is not None and duplicated.duplicates.get(pid)))
    )
    facts = facts_of(media_dir, detail['file'], keep) if keep else duplicated
    covered, uncovered, notes = collections.Counter(), 0, []
    # Streams whose lists differ only in the positions the reference gives
    # the units after a piece of 200 KiB: pairing by position then leaves
    # units of the stream unpaired, so the whole stream is judged at once.
    pieces = {
        stream
        for stream in {s for (s, _, _) in units}
        if stream < len(cstreams)
        and read_in_pieces(cstreams[stream], stream_at(reference_packets, stream), facts)
    }
    for (stream, _, _), unit_diffs in units.items():
        if stream in pieces:
            covered['KD-TS-POSITION'] += 1
            continue
        cstream = cstreams[stream] if stream < len(cstreams) else None
        if cstream is not None and after_duplicate(cstream, unit_diffs, facts):
            covered['KD-TS-DUPLICATE'] += 1
        elif cstream is not None and frame_time(cstream, unit_diffs, facts):
            covered['KD-TS-FRAME-TIME'] += 1
        elif cstream is not None and stated_decode_time(cstream, unit_diffs, facts):
            covered['KD-TS-DECODE-TIME'] += 1
        else:
            uncovered += 1
            if len(notes) < 5:
                notes.append(unit_diffs[0]['path'])
    total = sum(covered.values()) + uncovered
    if total != disagree:
        return {}, disagree, [f'unit count {total} differs from {disagree}']
    return dict(covered), uncovered, notes


def overlay(reports, reference_dir, candidate_dir, media_dir=None):
    entries = {e['id'] for e in json.loads(KD_FILE.read_text())['entries']}
    result = {'schema': 'coreav/kd-overlay/1', 'levels': {}}
    details = collections.defaultdict(list)
    for report in reports:
        for level in report['levels']:
            details[level['level']].extend(level['details'])
    for name, level_details in details.items():
        groups = {scope: {'items': 0, 'direct': 0, 'via_kd': 0, 'uncovered': 0, 'by_kd': collections.Counter(),
                          'reference_failures': 0, 'uncovered_items': [], 'covered_items': []}
                  for scope in ('in_scope', 'all')}
        for detail in level_details:
            file = detail['file']
            scopes = ['all'] + (['in_scope'] if file.endswith(IN_SCOPE) else [])
            if detail.get('reference_failure'):
                for scope in scopes:
                    groups[scope]['reference_failures'] += 1
                continue
            reference, candidate = load(reference_dir, file, name), load(candidate_dir, file, name)
            if name == 'streams':
                if detail['agrees']:
                    status, ids, missing, paths = 'direct', set(), [], None
                else:
                    status, ids, missing, paths = stream_item(
                        detail, reference, candidate, media_dir,
                        load(reference_dir, file, 'packets'),
                        load(candidate_dir, file, 'packets'))
                for scope in scopes:
                    g = groups[scope]
                    g['items'] += 1
                    g[{'direct': 'direct', 'kd': 'via_kd', 'uncovered': 'uncovered'}[status]] += 1
                    g['by_kd'].update(ids if status == 'kd' else ())
                    if status == 'kd':
                        # The field paths the entries covered, with the
                        # values they were checked on, so that
                        # scripts/compat_check.py can require the compat
                        # output to be those values (#219). A covered item
                        # has no uncovered path left, so every gating
                        # difference of it is one of them; the transport
                        # stream overlay has one stream-level entry, so each
                        # path carries the item's ids.
                        g['covered_items'].append({
                            'file': file, 'kd': sorted(ids),
                            'paths': paths if paths is not None else {
                                d['path']: {'kd': sorted(ids), 'candidate': d.get('candidate'),
                                            'reference': d.get('reference')}
                                for d in detail['gating_differences']}})
                    if status == 'uncovered':
                        g['uncovered_items'].append({'file': file, 'fields': missing})
            else:
                covered, uncovered, notes = ({}, 0, []) if detail['agrees'] else packet_units(
                    detail, candidate, media_dir, load(reference_dir, file, 'streams'),
                    load(candidate_dir, file, 'streams'), reference)
                direct = detail['packets_agree']
                for scope in scopes:
                    g = groups[scope]
                    g['items'] += direct + detail['packets_disagree']
                    g['direct'] += direct
                    g['via_kd'] += sum(covered.values())
                    g['uncovered'] += uncovered
                    g['by_kd'].update(covered)
                    if uncovered:
                        g['uncovered_items'].append({'file': file, 'units': uncovered, 'examples': notes})
        for g in groups.values():
            unknown = set(g['by_kd']) - entries
            if unknown:
                raise SystemExit(f'undocumented divergence ids: {sorted(unknown)}')
            g['by_kd'] = dict(sorted(g['by_kd'].items()))
            g['agreement'] = (g['direct'] + g['via_kd']) / g['items'] if g['items'] else None
            g['passed'] = g['items'] > 0 and g['uncovered'] == 0
        result['levels'][name] = groups
    result['passed'] = all(levels['in_scope']['passed'] for levels in result['levels'].values())
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', required=True, action='append')
    parser.add_argument('--reference', required=True)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--media', help='directory of the captured files: the evidence every entry checks')
    args = parser.parse_args()
    try:
        reports = [json.loads(pathlib.Path(r).read_text()) for r in args.report]
        result = overlay(reports, args.reference, args.candidate, args.media)
    except (OSError, ValueError, KeyError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 2
    pathlib.Path(args.out).write_text(json.dumps(result, indent=2) + '\n')
    for level, groups in result['levels'].items():
        for scope, g in groups.items():
            print(f"{level:8} {scope:9} items {g['items']:>8}  direct {g['direct']:>8}  via KD {g['via_kd']:>8}"
                  f"  uncovered {g['uncovered']:>6}  {g['by_kd']}")
    print('passed' if result['passed'] else 'FAILED')
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
