#!/usr/bin/env python3
"""Rewrites an MPEG transport stream into structures `ffmpeg` never writes,
for the transport stream fixtures (docs/design/mpegts.md). Written from
ISO/IEC 13818-1; reads and writes 188-byte packets only.

    ts_craft.py OP [ARGS] IN OUT

Operations:
  prefix N          N bytes of junk before the first packet
  lose PID EVERY    drop every EVERY-th packet of PID (continuity breaks)
  junk AFTER N      N junk bytes after packet AFTER (a lost sync)
  pad204            16 parity bytes after every packet (204-byte packets)
  duplicate PID     repeat every 10th packet of PID (duplicates, same counter)
  error PID         set transport_error_indicator on every 7th packet of PID
  cut FRACTION      keep the first FRACTION of the bytes (mid-packet)
  nulls EVERY       a null packet after every EVERY packets
  pmtchange         a PMT of version 0 that lists only the first stream
                    until halfway, then version 1 that adds the rest;
                    their packets before the change are dropped
  dtspts PID        every PES header of PID that states both times states
                    the decoding time its presentation time has
  ptsonly PID       every PES header of PID states its presentation time
                    alone; the decoding time becomes header stuffing
  delay PID FRAC    drop the packets of PID in the first FRAC of the file
  inject PID EVERY  a short PES packet of private_stream_2 on PID, which
                    no table declares, after every EVERY packets
  onepes PID        keep the bytes up to the second PES packet of PID, so
                    that the end of the file is what completes its first
  freeze PID        every PES header of PID states the time the first one
                    states, so the stream's times never step forward
  scramble PID      the PES payloads of PID become bytes that are no
                    frame header of any syntax
  audiodts PID N    every PES header of PID that states a presentation
                    time alone states a decoding time N ticks before it,
                    and the PID is packetized again to make room
"""
import sys


def packets(data):
    return [data[i:i + 188] for i in range(0, len(data) - len(data) % 188, 188)]


def pid_of(p):
    return ((p[1] & 0x1F) << 8) | p[2]


def crc32(data):
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte << 24
        for _ in range(8):
            crc = ((crc << 1) ^ 0x04C11DB7 if crc & 0x80000000 else crc << 1) & 0xFFFFFFFF
    return crc


def pmt_pid(ps):
    for p in ps:
        if pid_of(p) == 0 and p[1] & 0x40:
            sec = p[5 + p[4]:]
            return (sec[10] & 0x1F) << 8 | sec[11]
    raise SystemExit('no PAT')


def rewrite_pmt(p, keep_first_only, version):
    """The PMT section in one packet, its stream loop cut to the first
    entry or kept, under `version`."""
    start = 4 + (1 + p[4] if p[3] & 0x20 else 0)
    pointer = p[start]
    at = start + 1 + pointer
    length = (p[at + 1] & 0x0F) << 8 | p[at + 2]
    section = bytearray(p[at:at + 3 + length])
    body = section[8:-4]
    info = (body[2] & 0x0F) << 8 | body[3]
    loop = body[4 + info:]
    if keep_first_only:
        first = 5 + ((loop[3] & 0x0F) << 8 | loop[4])
        loop = loop[:first]
    body = body[:4 + info] + loop
    new = bytearray(section[:8]) + body
    new[5] = (new[5] & 0xC1) | (version << 1)
    total = len(new) + 4 - 3
    new[1] = (new[1] & 0xF0) | (total >> 8)
    new[2] = total & 0xFF
    new += crc32(new).to_bytes(4, 'big')
    out = bytearray(p[:at]) + new
    return bytes(out + b'\xff' * (188 - len(out))), [ (loop[i + 1] & 0x1F) << 8 | loop[i + 2] for i in [0] ]


# 13818-1 2.4.3.7 table 2-18: these stream ids carry no optional PES
# header, so they state no time and there is none to rewrite.
BARE_STREAM_IDS = (0xBC, 0xBE, 0xBF, 0xF0, 0xF1, 0xF2, 0xF8, 0xFF)


def states_a_time(p):
    """The packet starts a PES packet whose header would state a time:
    its stream id is not a bare one. Says nothing about whether this
    script can reach that time."""
    if not p[1] & 0x40 or not p[3] & 0x10:
        return False
    at = 4 + (1 + p[4] if p[3] & 0x20 else 0)
    return (at + 4 <= 188 and bytes(p[at:at + 3]) == b'\x00\x00\x01'
            and p[at + 3] not in BARE_STREAM_IDS)


def pes_header(p):
    """(the offset of the PES header in packet `p`, its flags byte, its
    header_data_length); None when the packet starts no PES packet with
    an optional header, or when the packet does not hold both
    timestamps, which is as far as the callers read and write (19 bytes
    from the start code)."""
    if not p[1] & 0x40 or not p[3] & 0x10:
        return None
    at = 4 + (1 + p[4] if p[3] & 0x20 else 0)
    if at + 19 > 188 or bytes(p[at:at + 3]) != b'\x00\x00\x01':
        return None
    if p[at + 3] in BARE_STREAM_IDS:
        return None
    return at, p[at + 7], p[at + 8]


def timestamp(field, marker):
    """The five bytes of a timestamp (13818-1 table 2-21) under `marker`,
    the four bits before it."""
    value = (((field[0] >> 1) & 7) << 30 | field[1] << 22 | (field[2] >> 1) << 15
             | field[3] << 7 | field[4] >> 1)
    return bytes([(marker << 4) | ((value >> 29) & 0x0E) | 1,
                  (value >> 22) & 0xFF, ((value >> 14) & 0xFE) | 1,
                  (value >> 7) & 0xFF, ((value << 1) & 0xFE) | 1])


def main():
    op, *args = sys.argv[1:-2]
    src, dst = sys.argv[-2], sys.argv[-1]
    data = open(src, 'rb').read()
    ps = packets(data)
    out = bytearray()
    if op == 'prefix':
        out += bytes((i * 37 + 11) % 251 for i in range(int(args[0]))) + data
    elif op == 'lose':
        pid, every, seen = int(args[0], 0), int(args[1]), 0
        for p in ps:
            if pid_of(p) == pid:
                seen += 1
                if seen % every == 0:
                    continue
            out += p
    elif op == 'junk':
        after, n = int(args[0]), int(args[1])
        for i, p in enumerate(ps):
            out += p
            if i == after:
                out += bytes((k * 13 + 5) % 251 for k in range(n)).replace(b'\x47', b'\x48')
    elif op == 'pad204':
        for p in ps:
            out += p + bytes(16)
    elif op == 'duplicate':
        pid, seen = int(args[0], 0), 0
        for p in ps:
            out += p
            if pid_of(p) == pid:
                seen += 1
                if seen % 10 == 0:
                    out += p
    elif op == 'error':
        pid, seen = int(args[0], 0), 0
        for p in ps:
            if pid_of(p) == pid:
                seen += 1
                if seen % 7 == 0:
                    p = bytes([p[0], p[1] | 0x80]) + p[2:]
            out += p
    elif op == 'cut':
        out += data[:int(len(data) * float(args[0]))]
    elif op == 'nulls':
        every = int(args[0])
        null = bytes([0x47, 0x1F, 0xFF, 0x10]) + bytes([0xFF] * 184)
        for i, p in enumerate(ps):
            out += p
            if i % every == every - 1:
                out += null
    elif op in ('dtspts', 'ptsonly'):
        pid = int(args[0], 0)
        for p in ps:
            found = pes_header(p) if pid_of(p) == pid else None
            if found is not None and found[1] >> 6 == 3:
                at, _, _ = found
                q = bytearray(p)
                if op == 'dtspts':
                    # The decoding time the presentation time has, under
                    # the marker '0001' a DTS carries beside a PTS.
                    q[at + 14:at + 19] = timestamp(p[at + 9:at + 14], 1)
                else:
                    # Only a PTS, under its own marker '0010'; the five
                    # bytes the decoding time filled become stuffing,
                    # which the header's length still counts.
                    q[at + 7] = (q[at + 7] & 0x3F) | 0x80
                    q[at + 9:at + 14] = timestamp(p[at + 9:at + 14], 2)
                    q[at + 14:at + 19] = b'\xff' * 5
                p = bytes(q)
            out += p
    elif op == 'delay':
        pid, fraction = int(args[0], 0), float(args[1])
        until = int(len(ps) * fraction)
        for i, p in enumerate(ps):
            if i < until and pid_of(p) == pid:
                continue
            out += p
    elif op == 'audiodts':
        pid, delta = int(args[0], 0), int(args[1])
        # This operation packetizes the PID again, which keeps no
        # adaptation field of the packets it replaces: not the clock
        # reference, and not the indicators beside it. A PID that
        # carries the program's clock is therefore not one it may
        # rewrite; on an audio PID the indicators are not read.
        if any(pid_of(p) == pid and p[3] & 0x20 and p[4] > 0 and p[5] & 0x10 for p in ps):
            sys.exit(f'audiodts: PID {pid:#x} carries a PCR')
        # The PES packets of the PID, each with the index of the transport
        # packet it starts at.
        groups, order = [], []
        for i, p in enumerate(ps):
            if pid_of(p) != pid or not p[3] & 0x10:
                continue  # a packet with no payload is left alone
            start = 4 + (1 + p[4] if p[3] & 0x20 else 0)
            if p[1] & 0x40 or not groups:
                groups.append(bytearray())
                order.append(i)
            groups[-1] += p[start:]
        rewritten = {}
        counter = 0
        for at, data in zip(order, groups):
            if len(data) >= 14 and bytes(data[:3]) == b'\x00\x00\x01' and data[7] >> 6 == 2:
                pts = ((data[9] >> 1 & 7) << 30 | data[10] << 22 | (data[11] >> 1) << 15
                       | data[12] << 7 | data[13] >> 1)
                # 0 is an unbounded PES packet (13818-1 2.4.3.7), which
                # stays unbounded; any other length grows by the five
                # bytes of the decoding time, as does the header's own
                # length, whose other bytes (stuffing, extensions) are
                # kept after the two timestamps.
                length = data[4] << 8 | data[5]
                hlen = data[8]
                head = bytearray(data[:9])
                head[4], head[5] = ((length + 5) >> 8) & 0xFF, (length + 5) & 0xFF
                if not length:
                    head[4] = head[5] = 0
                head[7], head[8] = 0xC0, hlen + 5
                later = (pts - delta) & ((1 << 33) - 1)
                body = head + timestamp(data[9:14], 3) + timestamp(
                    bytes([(later >> 29) & 0x0E | 1, (later >> 22) & 0xFF,
                           ((later >> 14) & 0xFE) | 1, (later >> 7) & 0xFF,
                           ((later << 1) & 0xFE) | 1]), 1) + data[14:]
            else:
                body = bytes(data)
            stored = []
            for k in range(0, len(body), 184):
                chunk = body[k:k + 184]
                header = bytes([0x47, (0x40 if k == 0 else 0) | (pid >> 8), pid & 0xFF])
                if len(chunk) < 184:
                    pad = 184 - len(chunk)
                    field = bytes([pad - 1]) + (bytes([0]) + b'\xff' * (pad - 2) if pad > 1 else b'')
                    stored.append(header + bytes([0x30 | counter]) + field + bytes(chunk))
                else:
                    stored.append(header + bytes([0x10 | counter]) + bytes(chunk))
                counter = (counter + 1) & 0x0F
            rewritten[at] = b''.join(stored)
        for i, p in enumerate(ps):
            if i in rewritten:
                out += rewritten[i]
            elif pid_of(p) != pid or not p[3] & 0x10:
                out += p
    elif op == 'freeze':
        # Every PES packet of the PID states the time the first states:
        # the stream shows no step, so no picture period (13818-1 2.7.5
        # states the times, not the steps between them).
        pid, first, missed = int(args[0], 0), None, 0
        for p in ps:
            starts = pid_of(p) == pid and p[1] & 0x40 and p[3] & 0x10
            found = pes_header(p) if starts else None
            if found is not None and found[1] >> 6 in (2, 3):
                at, flags, _ = found
                if first is None:
                    first = bytes(p[at + 9:at + 14])
                q = bytearray(p)
                q[at + 9:at + 14] = timestamp(first, 3 if flags >> 6 == 3 else 2)
                if flags >> 6 == 3:
                    q[at + 14:at + 19] = timestamp(first, 1)
                p = bytes(q)
            elif starts and found is None and states_a_time(p):
                # A PES packet whose header this cannot reach would keep
                # the time it states, which gives the stream the step
                # the whole operation exists to take away. One whose
                # header reads and states no time -- 13818-1 2.7.4 asks
                # for one only every 0.7 s -- gives it no step either
                # way, and is left alone.
                missed += 1
            out += p
        if missed:
            sys.exit(f'freeze: {missed} PES packets of PID {pid:#x} state a time this cannot read')
    elif op == 'scramble':
        # The payload bytes of the PID become bytes between 0x20 and
        # 0x4F, which hold no sync byte of any syntax this reader knows
        # (0xFF for ADTS and MPEG audio, 0x56 for LOAS, 0x0B for AC-3,
        # 0x7F for Opus), so a reader looking for a frame header looks
        # through the whole payload and finds none.
        pid, missed = int(args[0], 0), 0
        for p in ps:
            if pid_of(p) == pid and p[3] & 0x10:
                if p[1] & 0x40:
                    found = pes_header(p)
                    if found is None:
                        # Where the payload begins is not known, so the
                        # packet is left alone rather than written over.
                        missed += 1
                        out += p
                        continue
                    start = found[0] + 9 + found[2]
                else:
                    start = 4 + (1 + p[4] if p[3] & 0x20 else 0)
                q = bytearray(p)
                for i in range(start, 188):
                    q[i] = (i * 7 + 3) % 0x30 + 0x20
                p = bytes(q)
            out += p
        if missed:
            sys.exit(f'scramble: {missed} PES packets of PID {pid:#x} start where this cannot read')
    elif op == 'onepes':
        pid, starts = int(args[0], 0), 0
        for p in ps:
            if pid_of(p) == pid and p[1] & 0x40:
                starts += 1
                if starts == 2:
                    break
            out += p
    elif op == 'inject':
        # private_stream_2 (13818-1 2.4.3.7 table 2-18): a six-byte header
        # and opaque payload, no timestamps -- the shape a broadcast
        # capture's undeclared PIDs have.
        pid, every, counter = int(args[0], 0), int(args[1]), 0
        for i, p in enumerate(ps):
            out += p
            if i % every == every - 1:
                payload = bytes([0, 0, 1, 0xBF, 0, 14]) + bytes((k * 7 + 3) % 251 for k in range(14))
                head = bytes([0x47, 0x40 | (pid >> 8), pid & 0xFF, 0x30 | counter])
                pad = 184 - len(payload) - 2
                out += head + bytes([pad + 1, 0]) + b'\xff' * pad + payload
                counter = (counter + 1) & 0x0F
    elif op == 'pmtchange':
        pmt = pmt_pid(ps)
        half = len(ps) // 2
        listed = None
        for p in ps:
            if pid_of(p) == pmt:
                listed = rewrite_pmt(p, True, 0)[1]
                break
        for i, p in enumerate(ps):
            pid = pid_of(p)
            if pid == pmt:
                p = rewrite_pmt(p, i < half, 0 if i < half else 1)[0]
            elif i < half and pid not in (0, pmt, *listed) and pid != 0x1FFF:
                continue
            out += p
    else:
        sys.exit(f'unknown operation {op}')
    # An operation that names a PID and found none of it would copy the
    # file and report success, which an ffmpeg that renumbers its PIDs
    # would turn into a fixture that tests nothing.
    if op in ('lose', 'duplicate', 'error', 'dtspts', 'ptsonly', 'onepes', 'audiodts', 'delay',
              'freeze', 'scramble'):
        pid = int(args[0], 0)
        if not any(pid_of(p) == pid for p in ps):
            sys.exit(f'{op}: no packet of PID {pid:#x}')
    if op in ('dtspts', 'ptsonly') and out == data:
        sys.exit(f'{op}: no PES header of PID {int(args[0], 0):#x} states both times')
    open(dst, 'wb').write(out)


if __name__ == '__main__':
    main()
