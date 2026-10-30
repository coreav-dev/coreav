#!/usr/bin/env python3
"""An independent reader of an MP4 or MOV file's tracks, for the writer
oracles' box checks (`scripts/writer_boxes.py`).

Written from ISO/IEC 14496-12 (and the QuickTime File Format for sound
sample entry versions 1 and 2), apart from CoreAV's parser: it reads the
movie header, and for each track its header, edit list, media header,
handler, first sample entry and every sample, from the sample tables and
from the movie fragments of the same file (or of other files, for HLS
segments). A sample is its file offset, size, decode duration, composition
offset and sync flag.

Everything is read from the file's own bytes; nothing here runs ffprobe,
ffmpeg or CoreAV. A box this reader cannot read gives None, never a guess.
"""

import struct
import zlib
from array import array

MAX_MOOV = 256 << 20
MAX_INFLATED = 64 << 20
MAX_MOOF = 64 << 20
MAX_SAMPLES = 20_000_000
# ISO/IEC 14496-12 8.8.3.1: `sample_is_non_sync_sample`.
NON_SYNC = 0x00010000


class Unreadable(Exception):
    """A box is missing, short or past a cap."""


def boxes(data, start=0, end=None):
    """(type, payload start, end) of each box of `data[start:end]`."""
    end = len(data) if end is None else end
    at = start
    while at + 8 <= end:
        size, kind = struct.unpack_from(">I4s", data, at)
        header = 8
        if size == 1:
            if at + 16 > end:
                raise Unreadable("short box header")
            size, header = struct.unpack_from(">Q", data, at + 8)[0], 16
        elif size == 0:
            size = end - at
        if size < header or at + size > end:
            raise Unreadable(f"box {kind!r} runs past its parent")
        yield kind, at + header, at + size
        at += size


def child(data, parent, kind):
    """The first child box of a (start, end) range, as (start, end), or
    None."""
    if parent is None:
        return None
    for k, start, end in boxes(data, parent[0], parent[1]):
        if k == kind:
            return start, end
    return None


def children(data, parent, kind):
    return [(s, e) for k, s, e in boxes(data, parent[0], parent[1]) if k == kind]


def top_level(path):
    """(type, offset, header size, size) of each top-level box."""
    found = []
    with open(path, "rb") as f:
        total, at = f.seek(0, 2), 0
        while at + 8 <= total:
            f.seek(at)
            head = f.read(16)
            size, kind = struct.unpack(">I4s", head[:8])
            header = 8
            if size == 1:
                size, header = struct.unpack(">Q", head[8:16])[0], 16
            elif size == 0:
                size = total - at
            if size < header:
                raise Unreadable(f"top-level box {kind!r} of size {size}")
            found.append((kind, at, header, min(size, total - at)))
            at += size
    return found


def read_box(path, offset, header, size, cap):
    if size > cap:
        raise Unreadable(f"box of {size} bytes")
    with open(path, "rb") as f:
        f.seek(offset + header)
        data = f.read(size - header)
    if len(data) != size - header:
        raise Unreadable("file ends inside a box")
    return data


def u16(data, at):
    return struct.unpack_from(">H", data, at)[0]


def u32(data, at):
    return struct.unpack_from(">I", data, at)[0]


def be_array(code, data, start, count):
    """`count` big-endian values of an array type code from `data`."""
    width = array(code).itemsize
    if start + count * width > len(data):
        raise Unreadable("table runs past its box")
    values = array(code)
    values.frombytes(bytes(data[start:start + count * width]))
    if values.itemsize > 1:
        values.byteswap()
    return values


def full_header(data, box):
    """(version, flags) of a full box."""
    return data[box[0]], int.from_bytes(data[box[0] + 1:box[0] + 4], "big")


def sample_entry(data, stsd):
    """The first sample entry of `stsd` (8.5.2): its type, payload bytes and
    the fields the checks compare. Audio: the channel count and sample
    rate of a version 0 entry, or of QuickTime's version 1 (the same
    fields) and version 2 (the values in its extension) layouts. Video: the
    width and height."""
    if stsd is None or stsd[1] - stsd[0] < 8 or u32(data, stsd[0] + 4) == 0:
        return None
    first = next(boxes(data, stsd[0] + 8, stsd[1]), None)
    if first is None:
        return None
    kind, start, end = first
    payload = bytes(data[start:end])
    entry = {"type": kind.decode("latin1"), "payload": payload, "stsd_version": data[stsd[0]]}
    if len(payload) >= 28:
        entry["width"], entry["height"] = struct.unpack_from(">HH", payload, 24)
        version = u16(payload, 8)
        entry["sound_version"] = version
        if version == 2 and len(payload) >= 44:
            entry["channels"] = u32(payload, 40)
            entry["rate"] = struct.unpack_from(">d", payload, 32)[0]
        else:
            entry["channels"] = u16(payload, 16)
            entry["rate"] = u32(payload, 24) / 65536
    return entry


def stbl_samples(data, stbl):
    """The samples of a sample table (8.5 to 8.7), as columns."""
    def box(kind):
        return child(data, stbl, kind)

    stsz, stz2 = box(b"stsz"), box(b"stz2")
    if stsz is not None:
        size, count = u32(data, stsz[0] + 4), u32(data, stsz[0] + 8)
        if count > MAX_SAMPLES:
            raise Unreadable(f"{count} samples")
        sizes = array("I", [size]) * count if size else be_array("I", data, stsz[0] + 12, count)
    elif stz2 is not None:
        field, count = data[stz2[0] + 7], u32(data, stz2[0] + 8)
        if count > MAX_SAMPLES or field not in (4, 8, 16):
            raise Unreadable(f"stz2 of {count} {field}-bit sizes")
        if field == 16:
            sizes = array("I", be_array("H", data, stz2[0] + 12, count))
        elif field == 8:
            sizes = array("I", be_array("B", data, stz2[0] + 12, count))
        else:
            packed = be_array("B", data, stz2[0] + 12, (count + 1) // 2)
            sizes = array("I", ((packed[i // 2] >> (4 if i % 2 == 0 else 0)) & 15 for i in range(count)))
    else:
        if box(b"stts") is None:
            raise Unreadable("no sample size table")
        sizes = array("I")
    count = len(sizes)

    durations = array("I")
    stts = box(b"stts")
    if stts is not None:
        n = u32(data, stts[0] + 4)
        table = be_array("I", data, stts[0] + 8, 2 * n)
        for i in range(n):
            durations.extend(array("I", [table[2 * i + 1]]) * min(table[2 * i], count - len(durations)))
    if len(durations) != count:
        raise Unreadable(f"stts gives {len(durations)} of {count} samples")

    offsets_cto = array("q", [0]) * count
    ctts = box(b"ctts")
    if ctts is not None:
        version = data[ctts[0]]
        n = u32(data, ctts[0] + 4)
        table = be_array("I", data, ctts[0] + 8, 2 * n)
        at = 0
        for i in range(n):
            value = table[2 * i + 1]
            if version == 1 and value >= 1 << 31:
                value -= 1 << 32
            for _ in range(min(table[2 * i], count - at)):
                offsets_cto[at] = value
                at += 1
        if sum(table[0::2]) != count:
            raise Unreadable(f"ctts gives {sum(table[0::2])} of {count} samples")

    # Without a sync sample table every sample is a sync sample (8.6.2),
    # implied rather than stated.
    stss = box(b"stss")
    if stss is None:
        sync, stated = bytearray(b"\x01") * count, bytearray(count)
    else:
        sync, stated = bytearray(count), bytearray(b"\x01") * count
        n = u32(data, stss[0] + 4)
        for number in be_array("I", data, stss[0] + 8, n):
            if not 1 <= number <= count:
                raise Unreadable(f"stss names sample {number} of {count}")
            sync[number - 1] = 1

    chunks = None
    for kind, code in ((b"stco", "I"), (b"co64", "Q")):
        found = box(kind)
        if found is not None:
            chunks = be_array(code, data, found[0] + 8, u32(data, found[0] + 4))
    stsc = box(b"stsc")
    offsets = array("Q", [0]) * count
    if count:
        if chunks is None or stsc is None:
            raise Unreadable("no chunk offsets")
        n = u32(data, stsc[0] + 4)
        table = be_array("I", data, stsc[0] + 8, 3 * n)
        sample = 0
        for i in range(n):
            first, per_chunk = table[3 * i], table[3 * i + 1]
            last = table[3 * (i + 1)] if i + 1 < n else len(chunks) + 1
            for chunk in range(first, last):
                if chunk < 1 or chunk > len(chunks):
                    raise Unreadable("stsc names a chunk past stco")
                at = chunks[chunk - 1]
                for _ in range(per_chunk):
                    if sample >= count:
                        break
                    offsets[sample] = at
                    at += sizes[sample]
                    sample += 1
        if sample != count:
            raise Unreadable(f"chunks hold {sample} of {count} samples")
    return {"offsets": offsets, "sizes": sizes, "durations": durations, "cto": offsets_cto, "sync": sync,
            "stated": stated, "base_dts": 0, "gap": False}


COLUMNS = ("offsets", "sizes", "durations", "cto", "sync", "stated")


def empty_samples():
    return {"offsets": array("Q"), "sizes": array("I"), "durations": array("I"), "cto": array("q"),
            "sync": bytearray(), "stated": bytearray(), "base_dts": 0, "gap": False}


def extend(samples, more):
    """Appends the columns of `more` to `samples`. The first fragment's
    `tfdt` gives the decode times their start; a later one that does not
    continue them marks a gap (`gap`), which the checks that derive times
    from the durations refuse."""
    start = more.get("decode_time")
    if start is not None:
        if not samples["sizes"]:
            samples["base_dts"] = start
        elif start != samples["base_dts"] + sum(samples["durations"]):
            samples["gap"] = True
    for key in COLUMNS:
        samples[key].extend(more[key])


def traf_samples(data, traf, moof_offset, data_end, trex):
    """The samples of one track fragment (8.8.7 to 8.8.8), its track ID and
    its first decode time (`tfdt`, or None); `data_end` is where the data
    of the previous track fragment of the same `moof` ended (None for the
    first), the default base of a later one."""
    tfhd = child(data, traf, b"tfhd")
    if tfhd is None:
        raise Unreadable("traf without tfhd")
    _, flags = full_header(data, tfhd)
    track_id = u32(data, tfhd[0] + 4)
    defaults = dict(trex.get(track_id) or {"duration": 0, "size": 0, "flags": 0})
    at = tfhd[0] + 8
    base = None
    if flags & 0x01:
        base = struct.unpack_from(">Q", data, at)[0]
        at += 8
    if flags & 0x02:
        at += 4
    for bit, key in ((0x08, "duration"), (0x10, "size"), (0x20, "flags")):
        if flags & bit:
            defaults[key] = u32(data, at)
            at += 4
    if base is None:
        base = moof_offset if flags & 0x020000 or data_end is None else data_end
    tfdt = child(data, traf, b"tfdt")
    decode_time = None
    if tfdt is not None:
        decode_time = (struct.unpack_from(">Q", data, tfdt[0] + 4)[0] if data[tfdt[0]]
                       else u32(data, tfdt[0] + 4))
    samples = empty_samples()
    position = base
    for trun in children(data, traf, b"trun"):
        version, tflags = full_header(data, trun)
        count = u32(data, trun[0] + 4)
        if len(samples["sizes"]) + count > MAX_SAMPLES:
            raise Unreadable(f"{count} samples in a run")
        at = trun[0] + 8
        if tflags & 0x01:
            position = base + struct.unpack_from(">i", data, at)[0]
            at += 4
        first_flags = None
        if tflags & 0x04:
            first_flags = u32(data, at)
            at += 4
        fields = [bit for bit in (0x100, 0x200, 0x400, 0x800) if tflags & bit]
        width = 4 * len(fields)
        if at + count * width > trun[1]:
            raise Unreadable("trun runs past its box")
        for i in range(count):
            values = dict(zip(fields, struct.unpack_from(">" + "I" * len(fields), data, at + i * width)))
            duration = values.get(0x100, defaults["duration"])
            size = values.get(0x200, defaults["size"])
            sample_flags = values.get(0x400, first_flags if i == 0 and first_flags is not None
                                      else defaults["flags"])
            cto = values.get(0x800, 0)
            if version and cto >= 1 << 31:
                cto -= 1 << 32
            samples["offsets"].append(position)
            samples["sizes"].append(size)
            samples["durations"].append(duration)
            samples["cto"].append(cto)
            samples["sync"].append(0 if sample_flags & NON_SYNC else 1)
            samples["stated"].append(1)
            position += size
    return track_id, decode_time, samples, position


def movie_payload(payload):
    """The payload of a movie box, through a compressed movie box when it
    holds one and no movie header (QuickTime File Format, compressed movie
    resources: `cmov` holds `dcom`, the algorithm, and `cmvd`, the inflated
    size and the compressed data, which inflates to a whole `moov` box).
    Only `zlib` (RFC 1950)."""
    found = {k: (s, e) for k, s, e in boxes(payload)}
    if b"mvhd" in found or b"cmov" not in found:
        return payload
    cmov = found[b"cmov"]
    inner = {k: (s, e) for k, s, e in boxes(payload, *cmov)}
    dcom, cmvd = inner.get(b"dcom"), inner.get(b"cmvd")
    if dcom is None or cmvd is None or bytes(payload[dcom[0]:dcom[0] + 4]) != b"zlib":
        raise Unreadable("compressed movie box without zlib data")
    size = u32(payload, cmvd[0])
    if size > MAX_INFLATED:
        raise Unreadable(f"compressed movie of {size} bytes")
    inflater = zlib.decompressobj()
    data = inflater.decompress(bytes(payload[cmvd[0] + 4:cmvd[1]]), size + 1)
    if len(data) != size:
        raise Unreadable("compressed movie inflates to another size")
    movie = list(boxes(data))
    if len(movie) != 1 or movie[0][0] != b"moov":
        raise Unreadable("compressed movie is not one moov box")
    return memoryview(data)[movie[0][1]:movie[0][2]]


class Track(dict):
    """A track's fields; its 'samples' are read on first use, so a movie
    whose other tracks hold millions of samples (PCM) costs nothing for
    them."""

    def __missing__(self, key):
        if key != "samples" or "_load" not in self:
            raise KeyError(key)
        if "_error" in self:
            raise Unreadable(self["_error"])
        try:
            samples = self["_load"]()
        except (Unreadable, OSError, IndexError, ValueError, struct.error, zlib.error) as error:
            self["_error"] = f"{type(error).__name__}: {error}"
            raise Unreadable(self["_error"]) from error
        self["samples"] = samples
        return samples


def read_movie(path):
    """The movie of a file: {'movie_timescale', 'movie_duration', 'tracks':
    [...], 'fragmented', 'trex'}, tracks in `moov` order, each {'id',
    'handler', 'tkhd_duration', 'timescale', 'mdhd_duration', 'edits'
    (None without an edit list, else (duration, media time, rate) entries),
    'entry', 'samples' (the sample table's then the fragments', read when
    first used)}. Raises Unreadable, also when a track's samples are first
    used."""
    layout = top_level(path)
    moov = next((b for b in layout if b[0] == b"moov"), None)
    if moov is None:
        raise Unreadable("no moov")
    data = movie_payload(memoryview(read_box(path, *moov[1:], MAX_MOOV)))
    movie = {"tracks": [], "fragmented": any(b[0] == b"moof" for b in layout), "trex": {}}
    mvhd = child(data, (0, len(data)), b"mvhd")
    if mvhd is None:
        raise Unreadable("no mvhd")
    if data[mvhd[0]]:
        movie["movie_timescale"], movie["movie_duration"] = struct.unpack_from(">IQ", data, mvhd[0] + 20)
    else:
        movie["movie_timescale"], movie["movie_duration"] = struct.unpack_from(">II", data, mvhd[0] + 12)
    mvex = child(data, (0, len(data)), b"mvex")
    if mvex is not None:
        for trex in children(data, mvex, b"trex"):
            track_id, _, duration, size, flags = struct.unpack_from(">IIIII", data, trex[0] + 4)
            movie["trex"][track_id] = {"duration": duration, "size": size, "flags": flags}
    found = {}

    def loader(track, stbl):
        def load():
            samples = stbl_samples(data, stbl)
            if movie["fragmented"]:
                if "all" not in found:
                    found["all"] = fragments_of(path, layout, movie["trex"])
                for more in found["all"].get(track["id"], []):
                    extend(samples, more)
            return samples
        return load

    for trak in children(data, (0, len(data)), b"trak"):
        track, stbl = read_trak(data, trak)
        track["_load"] = loader(track, stbl)
        movie["tracks"].append(track)
    return movie


def read_trak(data, trak):
    tkhd = child(data, trak, b"tkhd")
    mdia = child(data, trak, b"mdia")
    mdhd = child(data, mdia, b"mdhd")
    hdlr = child(data, mdia, b"hdlr")
    if tkhd is None or mdhd is None or hdlr is None:
        raise Unreadable("trak without tkhd, mdhd or hdlr")
    track = Track()
    if data[tkhd[0]]:
        track["id"] = u32(data, tkhd[0] + 20)
        track["tkhd_duration"] = struct.unpack_from(">Q", data, tkhd[0] + 28)[0]
    else:
        track["id"] = u32(data, tkhd[0] + 12)
        track["tkhd_duration"] = u32(data, tkhd[0] + 20)
    if data[mdhd[0]]:
        track["timescale"], track["mdhd_duration"] = struct.unpack_from(">IQ", data, mdhd[0] + 20)
    else:
        track["timescale"], track["mdhd_duration"] = struct.unpack_from(">II", data, mdhd[0] + 12)
    track["handler"] = bytes(data[hdlr[0] + 8:hdlr[0] + 12]).decode("latin1")
    track["edits"] = None
    elst = child(data, child(data, trak, b"edts"), b"elst")
    if elst is not None:
        version, count = data[elst[0]], u32(data, elst[0] + 4)
        width, layout = (20, ">Qqhh") if version else (12, ">Iihh")
        if elst[0] + 8 + count * width > elst[1]:
            raise Unreadable("elst runs past its box")
        track["edits"] = [struct.unpack_from(layout, data, elst[0] + 8 + i * width)[:3] for i in range(count)]
    stbl = child(data, child(data, mdia, b"minf"), b"stbl")
    if stbl is None:
        raise Unreadable("trak without stbl")
    track["entry"] = sample_entry(data, child(data, stbl, b"stsd"))
    return track, stbl


def fragments_of(path, layout, trex):
    """{track ID: [samples of each track fragment, with its `tfdt` decode
    time or None]} of the movie fragments listed in `layout`."""
    found = {}
    for kind, offset, header, size in layout:
        if kind != b"moof":
            continue
        data = memoryview(read_box(path, offset, header, size, MAX_MOOF))
        end = None
        for k, start, stop in boxes(data):
            if k != b"traf":
                continue
            # Positions are file offsets: the moof starts at `offset`, its
            # payload buffer `header` bytes later.
            track_id, decode_time, samples, end = traf_samples(data, (start, stop), offset, end, trex)
            found.setdefault(track_id, []).append(dict(samples, decode_time=decode_time))
    return found


def segment_samples(paths, track_id, trex):
    """The samples of `track_id` in a list of media segment files (HLS), in
    order: each file's movie fragments, offsets within that file, as
    (path, samples) pairs."""
    out = []
    for path in paths:
        samples = empty_samples()
        for more in fragments_of(path, top_level(path), trex).get(track_id, []):
            extend(samples, more)
        out.append((path, samples))
    return out


def track_by_index(movie, index):
    """The track at position `index` of `moov`, as ffprobe numbers streams
    of an MP4 file; None past the end."""
    tracks = movie["tracks"]
    return tracks[index] if 0 <= index < len(tracks) else None


def iter_sample_bytes(path, offsets, sizes, first, count):
    """The bytes of samples first to first + count - 1, one at a time, read
    in blocks of adjacent samples (at most 8 MiB each)."""
    with open(path, "rb") as f:
        i = first
        while i < first + count:
            j, end = i, offsets[i] + sizes[i]
            while j + 1 < first + count and offsets[j + 1] == end and end - offsets[i] < 8 << 20:
                j += 1
                end += sizes[j]
            f.seek(offsets[i])
            block = f.read(end - offsets[i])
            at = 0
            for k in range(i, j + 1):
                yield block[at:at + sizes[k]]
                at += sizes[k]
            i = j + 1
