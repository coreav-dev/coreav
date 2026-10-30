#!/usr/bin/env python3
"""Box checks shared by the writer oracles.

The black-box oracles (`ffprobe`, `ffmpeg`, MediaInfo) do not see some of
what a writer writes: they take an AAC track's channels and rate from its
decoder configuration, ignore the header durations, list no sample that an
edit leaves out, and for some codecs take keyframes from the bitstream
rather than from the sync sample table. These checks read those values from
both files' own boxes (`scripts/track_boxes.py`, a reader apart from
CoreAV's) and compare the output with its source, by the rules each
writer's design states (ISO/IEC 14496-12 where it defines them):

- **samples**: the output's samples are the source's kept run, one for
  one: the same bytes, size, decode duration and composition offset;
- **sync**: each output sample's sync flag is the source's, except where
  the container marks every sample of the copied run as sync, which clip
  and split rewrite (`docs/design/cut-points.md`): then the output
  marks exactly the samples the coded data starts decoding from;
- **entry**: the sample entry's type, a visual entry's width and height,
  and a sound entry's channel count, sample rate and AAC configuration
  bytes are the source's;
- **timescale**: the media timescale is the source's;
- **headers**: the media, track and movie header durations and the movie
  timescale follow from the samples and edits by the writer's rule;
- **audio edit** (clip and split): the audio edit's empty part, media time
  and duration are where the source presents the kept audio.

Each check returns (field, message) pairs; the oracles report them as
problems. Nothing here runs ffprobe, ffmpeg or CoreAV.
"""

import functools
import math
from fractions import Fraction

import track_boxes as tb

_MOVIES = {}

# H.264 NAL unit types (ITU-T H.264 table 7-1), HEVC IRAP types (ITU-T H.265
# table 7-1) and AV1 OBU types (AV1 5.3.1).
H264_IDR, H264_SEI, H264_SLICE = 5, 6, 1
AV1_SEQUENCE_HEADER, AV1_FRAME_HEADER, AV1_FRAME = 1, 3, 6


def movie(path):
    """The movie of a file, read once per path; None when unreadable."""
    if path not in _MOVIES:
        try:
            _MOVIES[path] = tb.read_movie(path)
        except Exception as error:  # noqa: BLE001 - reported as unreadable
            _MOVIES[path] = f"{type(error).__name__}: {error}"
    found = _MOVIES[path]
    return found if isinstance(found, dict) else None


def forget(path=None):
    """Drops cached movies (of one path, or all)."""
    if path is None:
        _MOVIES.clear()
    else:
        _MOVIES.pop(path, None)


def unreadable(path):
    found = _MOVIES.get(path)
    return found if isinstance(found, str) else None


def kind_of(track):
    return {"vide": "video", "soun": "audio"}.get(track["handler"], track["handler"])


# The coded data: where a decoder can start (docs/design/cut-points.md).

def _config_box(entry):
    """(type, payload) of the codec configuration box among a visual sample
    entry's children (`avcC`, `hvcC`, `av1C`, `vpcC`), or None."""
    payload = entry["payload"]
    for kind, start, end in tb.boxes(payload, 78, len(payload)):
        if kind in (b"avcC", b"hvcC", b"av1C", b"vpcC"):
            return kind, bytes(payload[start:end])
    return None


def _length_size(entry):
    """The NAL unit length size of an `avcC` or `hvcC` record (ISO/IEC
    14496-15 5.3.3.1, 8.3.3.1)."""
    found = _config_box(entry)
    size = None
    if found and found[0] == b"avcC" and len(found[1]) >= 5:
        size = (found[1][4] & 3) + 1
    if found and found[0] == b"hvcC" and len(found[1]) >= 22:
        size = (found[1][21] & 3) + 1
    # 3 is not a length the bindings allow: the record cannot be read.
    return size if size in (1, 2, 4) else None


def _nal_units(sample, size):
    """The NAL units of a sample, or None when a length runs past its end
    (a zero length ends the walk, as zero-filled padding would)."""
    units, at = [], 0
    while at + size <= len(sample):
        length = int.from_bytes(sample[at:at + size], "big")
        at += size
        if length == 0:
            break
        if at + length > len(sample):
            return None
        units.append(sample[at:at + length])
        at += length
    return units


def _rbsp(nal, limit=64):
    """The first bytes of a NAL unit's payload with emulation prevention
    bytes removed (ITU-T H.264 7.4.1)."""
    out, zeros = bytearray(), 0
    for byte in nal[:limit]:
        if zeros >= 2 and byte == 3:
            zeros = 0
            continue
        out.append(byte)
        zeros = zeros + 1 if byte == 0 else 0
    return bytes(out)


class _Golomb:
    """Bits of an RBSP: u(n), ue(v) and se(v) (ITU-T H.264 7.2, 9.1)."""

    def __init__(self, data, bit=0):
        self.data, self.bit = data, bit

    def u(self, n):
        value = 0
        for _ in range(n):
            if self.bit >= 8 * len(self.data):
                raise IndexError("short")
            value = (value << 1) | ((self.data[self.bit // 8] >> (7 - self.bit % 8)) & 1)
            self.bit += 1
        return value

    def ue(self):
        zeros = 0
        while not self.u(1):
            zeros += 1
            if zeros > 31:
                raise IndexError("long code")
        return (1 << zeros) - 1 + self.u(zeros)

    def se(self):
        value = self.ue()
        return (value + 1) // 2 if value % 2 else -(value // 2)


def h264_max_ref_frames(entry):
    """`max_num_ref_frames` of the first sequence parameter set of an
    `avcC` record (ITU-T H.264 7.3.2.1.1), or None."""
    found = _config_box(entry)
    if not found or found[0] != b"avcC" or len(found[1]) < 8 or not found[1][5] & 0x1F:
        return None
    record = found[1]
    length = int.from_bytes(record[6:8], "big")
    sps = _rbsp(record[8:8 + length], 1024)
    try:
        bits = _Golomb(sps, 8)
        profile = bits.u(8)
        bits.u(16)
        bits.ue()
        if profile in (100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135):
            chroma = bits.ue()
            if chroma == 3:
                bits.u(1)
            bits.ue()
            bits.ue()
            bits.u(1)
            if bits.u(1):
                for i in range(8 if chroma != 3 else 12):
                    if bits.u(1):
                        last = scale = 8
                        for _ in range(16 if i < 6 else 64):
                            if scale:
                                scale = (last + bits.se() + 256) % 256
                            last = scale or last
        bits.ue()
        order = bits.ue()
        if order == 0:
            bits.ue()
        elif order == 1:
            bits.u(1)
            bits.se()
            bits.se()
            for _ in range(bits.ue()):
                bits.se()
        return bits.ue()
    except IndexError:
        return None


def _h264_recovery_point(nal):
    """The (recovery_frame_cnt, exact_match_flag) of an SEI NAL unit's
    recovery point message (payload type 6, ITU-T H.264 7.3.2.3.1, D.1.8),
    or None."""
    data, at = _rbsp(nal, 4096), 1
    while at < len(data) and data[at] != 0x80:
        kind = 0
        while at < len(data) and data[at] == 0xFF:
            kind, at = kind + 255, at + 1
        if at >= len(data):
            return None
        kind, at = kind + data[at], at + 1
        size = 0
        while at < len(data) and data[at] == 0xFF:
            size, at = size + 255, at + 1
        if at >= len(data):
            return None
        size, at = size + data[at], at + 1
        if kind == 6:
            try:
                bits = _Golomb(data[at:at + size])
                return bits.ue(), bits.u(1)
            except IndexError:
                return None
        at += size
    return None


def h264_access(sample, size, max_ref_frames):
    """Whether decoding starts at an H.264 frame, by `cut-points.md`'s rule
    from ITU-T H.264: 'must' for an IDR picture, a frame with a recovery
    point SEI message of `recovery_frame_cnt` 0 and `exact_match_flag` 1,
    or an intra reference picture (every coded slice I or SI, `nal_ref_idc`
    not 0) of a stream whose sequence parameter set keeps at most one
    reference frame; 'no' for another readable frame (also when the
    reference frame count is unknown and would decide); None when its NAL
    units or a slice header cannot be read."""
    units = _nal_units(sample, size)
    if not units:
        return None
    slices, intra, reference, starts = 0, True, True, False
    for nal in units:
        kind = nal[0] & 0x1F
        if kind == H264_IDR:
            starts = True
        if kind == H264_SEI and _h264_recovery_point(nal) == (0, 1):
            starts = True
        if kind in (H264_SLICE, 2, H264_IDR):
            slices += 1
            try:
                bits = _Golomb(_rbsp(nal), 8)
                bits.ue()
                slice_type = bits.ue()
            except IndexError:
                return None
            intra = intra and slice_type in (2, 4, 7, 9)
            reference = reference and bool(nal[0] & 0x60)
    if not slices:
        return None
    if starts:
        return "must"
    # An unknown reference frame count allows no cut (`cut-points.md`).
    return "must" if intra and reference and max_ref_frames is not None and max_ref_frames <= 1 else "no"


def hevc_access(sample, size):
    """'must' when the sample starts with an IRAP picture (its first coded
    slice's `nal_unit_type` 16 to 23, read up to that NAL unit), else 'no':
    `cut-points.md` keeps no container flag for HEVC, so a sample that
    cannot be read is no cut point."""
    at = 0
    while at + size < len(sample):
        length = int.from_bytes(sample[at:at + size], "big")
        at += size
        if length == 0:
            continue
        # The first coded slice's header decides, whatever follows it.
        kind = (sample[at] >> 1) & 0x3F
        if kind < 32:
            return "must" if 16 <= kind <= 23 else "no"
        at += length
    return "no"


def _leb128(data, at):
    value = 0
    for i in range(8):
        if at >= len(data):
            return None, at
        byte = data[at]
        value |= (byte & 0x7F) << (7 * i)
        at += 1
        if not byte & 0x80:
            return value, at
    return None, at


def av1_access(sample):
    """'must' for an AV1 sync sample (AV1 ISOBMFF binding 2.4): a sequence
    header OBU, then a frame header of a key frame shown at once (AV1
    5.9.2), or any frame under a reduced still picture header; 'no' for
    another readable sample, one without a frame header included; None
    when its OBUs cannot be walked."""
    at, header, reduced = 0, False, False
    while at < len(sample):
        first = sample[at]
        kind, extension, sized = (first >> 3) & 0xF, first & 4, first & 2
        at += 2 if extension else 1
        if not sized:
            size = len(sample) - at
        else:
            size, at = _leb128(sample, at)
            if size is None:
                return None
        if at + size > len(sample):
            return None
        payload = sample[at:at + size]
        if kind == AV1_SEQUENCE_HEADER and payload:
            header, reduced = True, bool(payload[0] & 0x08)
        elif kind in (AV1_FRAME_HEADER, AV1_FRAME):
            if not header:
                return "no"
            if reduced:
                return "must"
            if not payload:
                return None
            show_existing, frame_type, shown = payload[0] >> 7, (payload[0] >> 5) & 3, (payload[0] >> 4) & 1
            return "must" if not show_existing and frame_type == 0 and shown else "no"
        at += size
    # No frame header: nothing in the sample starts decoding.
    return "no"


def access(codec, sample, size, max_ref_frames=None):
    if codec == "h264":
        return h264_access(sample, size, max_ref_frames)
    if codec == "hevc":
        return hevc_access(sample, size)
    if codec == "av1":
        return av1_access(sample)
    return None


def codec_of(entry):
    kind = (entry or {}).get("type")
    return {"avc1": "h264", "avc3": "h264", "hvc1": "hevc", "hev1": "hevc", "av01": "av1"}.get(kind)


# The checks.

def compare_samples(kind, source, src_track, parts, first, count, rewritten_sync=None):
    """The output's samples, `parts` = [(path, samples)] in order (one file,
    or an HLS rendition's segments), against the source's samples first to
    first + count - 1: bytes, size, decode duration, composition offset and
    sync flag. `rewritten_sync` is for the writers that rewrite a claim
    that every copied sample is sync (`cut-points.md`): 'run' (clip) or
    'track' (split), what that claim is about. Where the source marks
    every sample of it as sync, for H.264, HEVC and AV1, the output must
    mark the samples the coded data starts decoding from and no other.
    (field, message) pairs."""
    found = []
    src = src_track["samples"]
    total = sum(len(s["sizes"]) for _, s in parts)
    if count is None:
        count = len(src["sizes"]) - first
    if first < 0 or first + count > len(src["sizes"]) or total != count:
        return [(f"box:{kind}:samples", f"{kind} samples: {total} in the output, {count} kept of the "
                 f"source's {len(src['sizes'])} from sample {first}")]
    claimed = src["sync"][first:first + count] if rewritten_sync == "run" else src["sync"]
    codec = codec_of(src_track["entry"])
    size = _length_size(src_track["entry"]) if codec in ("h264", "hevc") else None
    # The rule rewrites the flags of H.264, HEVC and AV1 only; other codecs,
    # and tracks whose configuration record cannot be read, keep the
    # container's flags (`cut-points.md`), as every other writer does.
    claim_all = (rewritten_sync is not None and all(claimed) and codec is not None
                 and (codec == "av1" or size is not None))
    max_ref = h264_max_ref_frames(src_track["entry"]) if codec == "h264" else None
    differ = {"bytes": [], "size": [], "duration": [], "composition offset": [], "sync": []}
    i = 0
    for path, out in parts:
        n = len(out["sizes"])
        src_bytes = tb.iter_sample_bytes(source, src["offsets"], src["sizes"], first + i, n)
        out_bytes = tb.iter_sample_bytes(path, out["offsets"], out["sizes"], 0, n)
        for j, (a, b) in enumerate(zip(src_bytes, out_bytes)):
            k = first + i + j
            if out["sizes"][j] != src["sizes"][k]:
                differ["size"].append(i + j)
            elif a != b:
                differ["bytes"].append(i + j)
            if out["durations"][j] != src["durations"][k]:
                differ["duration"].append(i + j)
            # As stored: the same 32 bits, whatever the table's version
            # (a QuickTime source's version 0 table may hold signed values,
            # which a version 1 table states as such).
            if (out["cto"][j] - src["cto"][k]) % (1 << 32):
                differ["composition offset"].append(i + j)
            if claim_all:
                verdict = access(codec, a, size, max_ref)
                if verdict is None:
                    # Coded data that cannot be read keeps the flag a table
                    # or a fragment's flags state; an implied one is no cut
                    # point.
                    expected = bool(src["stated"][k])
                else:
                    expected = verdict == "must"
                ok = bool(out["sync"][j]) == expected
            else:
                ok = bool(out["sync"][j]) == bool(src["sync"][k])
            if not ok:
                differ["sync"].append(i + j)
        i += n
    for what, where in differ.items():
        if where:
            field = f"box:{kind}:sync" if what == "sync" else f"box:{kind}:samples"
            rule = (" (the samples the coded data starts from)" if what == "sync" and claim_all else "")
            found.append((field, f"{kind} sample {what}{rule} differs from the source's at {len(where)} of "
                          f"{count} samples, first at output sample {where[0]}"))
    return found


def cut_point(source, track, index):
    """Whether a source sample is a cut point (`cut-points.md`): the
    container marks it as a sync sample and the coded data agrees (H.264,
    HEVC, AV1 with a readable configuration; an H.264 or AV1 frame that
    cannot be read keeps a stated flag); other codecs by the flag alone."""
    samples = track["samples"]
    if not samples["sync"][index]:
        return False
    codec = codec_of(track["entry"])
    size = _length_size(track["entry"]) if codec in ("h264", "hevc") else None
    if codec is None or (codec != "av1" and size is None):
        return True
    sample = next(tb.iter_sample_bytes(source, samples["offsets"], samples["sizes"], index, 1))
    verdict = access(codec, sample, size, h264_max_ref_frames(track["entry"]) if codec == "h264" else None)
    return bool(samples["stated"][index]) if verdict is None else verdict == "must"


def compare_cut(source, track, first):
    if not cut_point(source, track, first):
        return [("box:video:cut", f"video: the first kept sample, source sample {first}, is no cut point")]
    return []


def _sound_channels(entry):
    """The channel counts a copy of a sound entry may state: its own, and
    for AAC the one its AudioSpecificConfig states (ISO/IEC 14496-3
    1.6.2.1, table 1.19, or its program config element, 4.4.1.1), which
    decoders follow; a mono configuration that signals parametric stereo
    (object type 29) also as 2 (8.6.4)."""
    counts = {entry.get("channels")}
    config = aac_config(entry)
    if config is not None:
        stated = config_channels(config)
        if stated:
            counts.add(stated)
        if stated == 1 and config and config[0] >> 3 == 29:
            counts.add(2)
    return counts


def _descriptor(body, at):
    """(tag, payload start, payload end) of an MPEG-4 descriptor (ISO/IEC
    14496-1 8.3.3)."""
    if at >= len(body):
        return None
    tag, at, size = body[at], at + 1, 0
    for _ in range(4):
        if at >= len(body):
            return None
        byte = body[at]
        size, at = (size << 7) | (byte & 0x7F), at + 1
        if not byte & 0x80:
            break
    return tag, at, min(at + size, len(body))


def aac_config(entry):
    """The DecoderSpecificInfo bytes of an `mp4a` entry's `esds` (ISO/IEC
    14496-1 7.2.6.5 to 7.2.6.7), empty when its decoder configuration has
    none, or None when the `esds` cannot be read."""
    if (entry or {}).get("type") != "mp4a":
        return None
    payload = entry["payload"]
    # ISO/IEC 14496-12's AudioSampleEntryV1 (12.2.3, only in a version 1
    # `stsd`) keeps its boxes where version 0 does; QuickTime's version 1
    # and 2 layouts add fields before them.
    iso_v1 = entry.get("stsd_version") == 1 and entry.get("sound_version") == 1
    start = 28 + (0 if iso_v1 else {1: 16, 2: 36}.get(entry.get("sound_version"), 0))
    try:
        esds = None
        for kind, s, e in tb.boxes(payload, start, len(payload)):
            if kind == b"esds":
                esds = (s, e)
            elif kind == b"wave":
                # QuickTime's wrapper of the decoder configuration.
                esds = esds or next(((s2, e2) for k2, s2, e2 in tb.boxes(payload, s, e) if k2 == b"esds"), None)
        if esds is None:
            return None
        body = payload[esds[0] + 4:esds[1]]
        es = _descriptor(body, 0)
        if not es or es[0] != 3:
            return None
        flags = body[es[1] + 2]
        at = es[1] + 3
        at += 2 if flags & 0x80 else 0
        if flags & 0x40:
            at += 1 + body[at]
        at += 2 if flags & 0x20 else 0
        dc = _descriptor(body, at)
        if not dc or dc[0] != 4:
            return None
        info = _descriptor(body, dc[1] + 13)
        # A stream without one (MPEG-1 audio, object type 0x6B) has none:
        # empty, not unread.
        return bytes(body[info[1]:info[2]]) if info and info[0] == 5 else b""
    except (tb.Unreadable, IndexError):
        return None


class _Bits:
    def __init__(self, data):
        self.data, self.at = data, 0

    def read(self, n):
        value = 0
        for _ in range(n):
            if self.at >= 8 * len(self.data):
                raise IndexError("short")
            value = (value << 1) | ((self.data[self.at // 8] >> (7 - self.at % 8)) & 1)
            self.at += 1
        return value


def config_channels(config):
    """The output channels an AudioSpecificConfig states, or None."""
    bits = _Bits(config)
    try:
        def object_type():
            value = bits.read(5)
            return 32 + bits.read(6) if value == 31 else value

        def frequency():
            if bits.read(4) == 15:
                bits.read(24)

        kind = object_type()
        frequency()
        configuration = bits.read(4)
        table = {1: 1, 2: 2, 3: 3, 4: 4, 5: 5, 6: 6, 7: 8, 11: 7, 12: 8, 13: 24, 14: 8}
        if configuration:
            return table.get(configuration)
        if kind in (5, 29):
            frequency()
            kind = object_type()
        if kind not in (1, 2, 3, 4, 6, 7, 17, 19, 20, 21, 22, 23):
            return None
        bits.read(1)
        if bits.read(1):
            bits.read(14)
        bits.read(1)
        # program_config_element (4.4.1.1, table 4.2).
        bits.read(4 + 2 + 4)
        front, side, back, lfe = bits.read(4), bits.read(4), bits.read(4), bits.read(2)
        bits.read(3 + 4)
        for width in (4, 4, 3):
            if bits.read(1):
                bits.read(width)
        channels = lfe
        for _ in range(front + side + back):
            channels += 2 if bits.read(1) else 1
            bits.read(4)
        return channels or None
    except IndexError:
        return None


def compare_entry(kind, src_track, out_track, video_config=True):
    """The sample entry's type and the fields the black boxes do not read
    from it, against the source's; with `video_config`, also a visual
    entry's codec configuration box, copied as stored by every writer but
    sanitization (`remux.md` "Sample entries")."""
    src, out = src_track.get("entry"), out_track.get("entry")
    if not src or not out:
        return [(f"box:{kind}:entry", f"{kind} sample entry: none in the {'source' if not src else 'output'}")]
    found = []
    if src["type"] != out["type"]:
        found.append((f"box:{kind}:entry", f"{kind} sample entry {out['type']!r}, source {src['type']!r}"))
    if kind == "video" and (src.get("width"), src.get("height")) != (out.get("width"), out.get("height")):
        found.append((f"box:{kind}:entry", f"video sample entry {out.get('width')}x{out.get('height')}, "
                      f"source {src.get('width')}x{src.get('height')}"))
    if kind == "video" and video_config and _config_box(src) != _config_box(out):
        found.append((f"box:{kind}:entry", "video sample entry: the codec configuration box differs from the "
                      "source's"))
    if kind == "audio":
        if out.get("channels") not in _sound_channels(src):
            found.append((f"box:{kind}:entry", f"audio sample entry: {out.get('channels')} channels, source "
                          f"{src.get('channels')} (configuration {sorted(_sound_channels(src) - {None})})"))
        if out.get("rate") != src.get("rate"):
            found.append((f"box:{kind}:entry", f"audio sample entry: rate {out.get('rate')}, source "
                          f"{src.get('rate')}"))
        # The decoder configuration is copied as stored (`remux.md` "Sample entries").
        if aac_config(src) != aac_config(out):
            found.append((f"box:{kind}:entry", "audio sample entry: the AudioSpecificConfig differs from the "
                          "source's"))
        elif out.get("type") == "mp4a" and aac_config(out) is None:
            # Two configurations neither of which can be read do not match.
            found.append((f"box:{kind}:entry", "audio sample entry: no AudioSpecificConfig read"))
    return found


def compare_timescale(kind, src_track, out_track):
    if src_track["timescale"] != out_track["timescale"]:
        return [(f"box:{kind}:timescale", f"{kind} media timescale {out_track['timescale']}, source "
                 f"{src_track['timescale']}")]
    return []


def declared_or_sum(src_movie, src_track, out_track):
    """The media duration a stream copy of the whole track writes
    (`docs/design/remux.md`): the source's declared
    one when it has no fragments and declares one, else the sum of the
    decode durations."""
    if not src_movie["fragmented"] and src_track["mdhd_duration"]:
        return src_track["mdhd_duration"]
    return sum(out_track["samples"]["durations"])


def kept_duration(src_movie, src_track, out_track, first):
    """The media duration the clip writer gives a kept run: the sum of
    its decode durations, plus, when it keeps the last sample of a source
    without fragments that declares a media duration, the source's
    difference between that duration and its own sum (unless that leaves
    none)."""
    total = sum(out_track["samples"]["durations"])
    src = src_track["samples"]
    if first + len(out_track["samples"]["sizes"]) == len(src["sizes"]) and not src_movie["fragmented"] \
            and src_track["mdhd_duration"]:
        carried = total + src_track["mdhd_duration"] - sum(src["durations"])
        if carried > 0:
            return carried
    return total


def edits_duration(track):
    return sum(d for d, _, _ in track["edits"] or [])


def compare_media_duration(kind, out_track, expected):
    if out_track["mdhd_duration"] != expected:
        return [(f"box:{kind}:mdhd", f"{kind} media header duration {out_track['mdhd_duration']}, "
                 f"expected {expected}")]
    return []


def compare_track_duration(kind, out_track, expected):
    if out_track["tkhd_duration"] != expected:
        return [(f"box:{kind}:tkhd", f"{kind} track header duration {out_track['tkhd_duration']}, "
                 f"expected {expected}")]
    return []


def compare_movie_duration(out_movie):
    """ISO/IEC 14496-12 8.2.2: the movie's duration is its longest track's."""
    longest = max((t["tkhd_duration"] for t in out_movie["tracks"]), default=0)
    if out_movie["movie_duration"] != longest:
        return [("box:mvhd", f"movie header duration {out_movie['movie_duration']}, the longest track "
                 f"{longest}")]
    return []


def clip_headers(kind, src_movie, src_track, out_movie, out_track, first):
    """The headers the clip writer writes for a clip or a
    split segment: the source's media timescale; a media duration of the kept
    run's (for the video, never shorter than its edits); a track duration
    of the sum of its edits (8.3.2)."""
    found = compare_timescale(kind, src_track, out_track)
    # Every media edit plays at rate 1: the writers refuse other rates.
    rates = sorted({r for _, t, r in out_track["edits"] or [] if t != -1 and r != 1})
    if rates:
        found.append((f"box:{kind}:edit", f"{kind} edit: media rate {rates}"))
    expected = kept_duration(src_movie, src_track, out_track, first)
    if kind == "video" and out_track["edits"]:
        # Its edits from the first media edit on, whose span the media covers.
        entries = list(out_track["edits"])
        while entries and entries[0][1] == -1:
            entries.pop(0)
        edits = Fraction(sum(d for d, _, _ in entries) * out_track["timescale"], out_movie["movie_timescale"])
        expected = max(expected, math.ceil(edits))
    found += compare_media_duration(kind, out_track, expected)
    if out_track["edits"] is not None:
        found += compare_track_duration(kind, out_track, edits_duration(out_track))
    else:
        # Without an edit list the track lasts its media (8.3.2).
        found += compare_track_duration(kind, out_track, _nearest(
            Fraction(out_track["mdhd_duration"] * out_movie["movie_timescale"], out_track["timescale"])))
    return found


def clip_movie(out_movie, several):
    """The movie timescale (for several
    edits, `edit-list-writers.md` clip decision 5) and duration of a clip
    or a split segment."""
    found = compare_movie_duration(out_movie)
    scales = [t["timescale"] for t in out_movie["tracks"]]
    if not scales:
        return found
    common = math.lcm(*scales)
    expected = common if common < 1 << 32 else scales[0]
    movie_ts = out_movie["movie_timescale"]
    if several:
        if movie_ts % scales[0]:
            found.append(("box:movie timescale", f"movie timescale {movie_ts}, not a multiple of the video's "
                          f"{scales[0]}"))
    elif movie_ts != expected:
        found.append(("box:movie timescale", f"movie timescale {movie_ts}, expected {expected} (media "
                      f"timescales {scales})"))
    return found


def source_window(src_movie, src_track):
    """The source track's one media window: (media start, media end,
    presented at in seconds), the end capped by the media's; None for
    several media edits. Without an edit list, the whole media from 0.
    Media times are the source's decode times: a fragmented source's start
    at its first `tfdt`."""
    ts = src_track["timescale"]
    _contiguous(src_track)
    media_end = src_track["samples"]["base_dts"] + sum(src_track["samples"]["durations"])
    if not src_track["edits"]:
        return Fraction(0), Fraction(media_end), Fraction(0)
    windows, at = [], Fraction(0)
    for duration, media_time, _ in src_track["edits"]:
        if media_time != -1:
            length = Fraction(duration * ts, src_movie["movie_timescale"])
            windows.append((Fraction(media_time), Fraction(media_time) + length, at))
        at += Fraction(duration, src_movie["movie_timescale"])
    if len(windows) != 1:
        return None
    m0, m1, start = windows[0]
    # An edit of duration 0 lasts to the end of the media (8.6.6).
    if m1 == m0 or m1 > media_end:
        m1 = Fraction(media_end)
    return m0, m1, start


def _contiguous(track):
    """Refuses a track whose fragments leave decode time gaps: the times
    derived from its durations would be wrong."""
    if track["samples"].get("gap"):
        raise tb.Unreadable("the source's fragments leave a decode time gap")


def _nearest(x):
    return math.floor(x + Fraction(1, 2))


def audio_edit(src_movie, src_track, out_movie, out_track, first, zero, end=None):
    """The audio edit of a clip or split segment, against where the source
    presents the kept samples (`docs/design/split.md`
    "Timing"), for a source with one media window: the audio is presented
    from the latest of the output's start, the window's start and its first
    kept sample's start, to the earliest of `end` (a clip's requested end),
    the window's end and its last kept sample's end. The empty edit and the
    media time are those times in the output, rounded to the nearest tick,
    and the media edit ends at its end so rounded."""
    window = source_window(src_movie, src_track)
    if window is None:
        return [("box:audio:edit", "audio edit: the source has several media edits")]
    m0, m1, start = window
    ts, movie_ts = src_track["timescale"], out_movie["movie_timescale"]
    durations = src_track["samples"]["durations"]
    count = len(out_track["samples"]["sizes"])
    first_dts = src_track["samples"]["base_dts"] + sum(durations[:first])
    last_end = first_dts + sum(durations[first:first + count])
    begin = max(zero, start, start + (first_dts - m0) / ts)
    media_start = m0 + (begin - start) * ts
    stop = start + (min(m1, last_end) - m0) / ts
    if end is not None:
        stop = min(stop, end)
    edits = out_track["edits"] if out_track["edits"] is not None else _identity(out_movie, out_track)
    empty = [d for d, t, _ in edits if t == -1]
    media = [(d, t) for d, t, _ in edits if t != -1]
    # Each edit ends at its exact end rounded, so that rounding does not add
    # up (`edit-list-writers.md`, clip decision 5).
    empty_ticks = _nearest((begin - zero) * movie_ts)
    expected = (empty_ticks, _nearest(media_start - first_dts), _nearest((stop - zero) * movie_ts) - empty_ticks)
    got = (sum(empty), media[0][1] if media else None, media[0][0] if media else None)
    shape = len(media) == 1 and len(edits) == len(media) + (1 if empty else 0) and \
        (not empty or edits[0][1] == -1)
    if not shape or got != expected:
        return [("box:audio:edit", f"audio edit: empty {got[0]}, media time {got[1]}, duration {got[2]}; "
                 f"from the source {expected[0]}, {expected[1]}, {expected[2]}"
                 + ("" if shape else f" ({len(edits)} entries)"))]
    return []


def split_audio_range(src_movie, src_track, first, count):
    """Where a split segment's audio is presented when the source's has
    several edits (`edit-list-writers.md`, split decision 5): from the
    first showing of the first of its samples that a window shows to the
    end of the first showing of the last of them, in seconds of the
    source's presentation; None when no window shows any. A sample's
    showing is its media's part inside a window, so one that starts in a
    gap is shown from the window's start."""
    ts, movie_ts = src_track["timescale"], src_movie["movie_timescale"]
    _contiguous(src_track)
    durations = src_track["samples"]["durations"]
    media_end = src_track["samples"]["base_dts"] + sum(durations)
    windows, at = [], Fraction(0)
    for duration, media_time, _ in src_track["edits"] or []:
        length = Fraction(duration * ts, movie_ts)
        if media_time != -1:
            m1 = media_time + length
            if duration == 0 or m1 > media_end:
                m1 = Fraction(max(media_end, media_time))  # capped by the media
            windows.append((Fraction(media_time), m1, at))
        at += Fraction(duration, movie_ts)

    def showing(dts, duration):
        for m0, m1, start in windows:
            lo, hi = max(dts, m0), min(dts + duration, m1)
            if hi > lo:
                return start + (lo - m0) / ts, start + (hi - m0) / ts
        return None

    dts = src_track["samples"]["base_dts"] + sum(durations[:first])
    shown = []
    for duration in durations[first:first + count]:
        span = showing(dts, duration)
        if span is not None:
            shown.append(span)
        dts += duration
    if not shown:
        return None
    return shown[0][0], shown[-1][1]


def _identity(out_movie, out_track):
    """The edit a track without an edit list has: its media from 0 for its
    media duration (ISO/IEC 14496-12 8.3.2, 8.6.6)."""
    duration = Fraction(out_track["mdhd_duration"] * out_movie["movie_timescale"], out_track["timescale"])
    return [(_nearest(duration), 0, 1)]


def audio_edits_several(src_movie, src_track, out_movie, out_track, first, zero, end, begin=None):
    """The audio edits of a clip of a source with several edits
    (`edit-list-writers.md`, clip decisions 4 to 6): the source's entries,
    empty ones included, cut to the clip's presentation from its start to
    its requested end, in order; a media entry's media time moved by the
    first kept sample's decode time; each entry ending at its exact end in
    the output, rounded to the nearest movie tick, and left out when that
    leaves it no ticks; no empty entry last. `begin`, where the presented
    range starts when later than `zero` (a split segment's audio), is
    preceded by one empty entry from `zero`."""
    begin = zero if begin is None else max(begin, zero)
    ts, src_movie_ts, movie_ts = src_track["timescale"], src_movie["movie_timescale"], out_movie["movie_timescale"]
    _contiguous(src_track)
    durations = src_track["samples"]["durations"]
    media_end = src_track["samples"]["base_dts"] + sum(durations)
    first_dts = src_track["samples"]["base_dts"] + sum(durations[:first])
    parts, at = [], Fraction(0)
    for duration, media_time, _ in src_track["edits"] or []:
        length = Fraction(duration, src_movie_ts)
        if media_time != -1 and (duration == 0 or media_time + length * ts > media_end):
            length = max(Fraction(media_end - media_time, ts), Fraction(0))  # capped by the media
        lo, hi = max(at, begin), min(at + length, end)
        if hi > lo:
            parts.append((lo, hi, None if media_time == -1 else media_time + (lo - at) * ts - first_dts))
        at += Fraction(duration, src_movie_ts)
    while parts and parts[-1][2] is None:
        parts.pop()
    expected, done = [], 0
    if parts and parts[0][0] > zero:
        parts.insert(0, (zero, parts[0][0], None))
    for lo, hi, media in parts:
        stop = _nearest((hi - zero) * movie_ts)
        if stop - done > 0:
            expected.append((stop - done, -1 if media is None else _nearest(media)))
        done = max(done, stop)
    edits = out_track["edits"] if out_track["edits"] is not None else _identity(out_movie, out_track)
    got = [(d, t) for d, t, _ in edits]
    if got != expected:
        return [("box:audio:edit", f"audio edits {got[:4]}{' ...' if len(got) > 4 else ''}; from the source "
                 f"{expected[:4]}{' ...' if len(expected) > 4 else ''} ({len(got)} and {len(expected)} entries)")]
    return []


# The writers. Each returns (field, message) pairs; a file this reader
# cannot read is a problem for an output and an unchecked source otherwise.

def _guarded(check):
    """A box the reader cannot read while a check runs (a track's samples
    are read when first used) is a problem, never a pass."""
    @functools.wraps(check)
    def guarded(*args, **kwargs):
        try:
            return check(*args, **kwargs)
        except Exception as error:  # noqa: BLE001 - any failure is a problem, never a pass
            return [("box:unreadable", f"boxes: {type(error).__name__}: {error}")]
    return guarded


def _load(source, output):
    src, out = movie(source), movie(output)
    if out is None:
        return None, None, [("box:unreadable", f"the output's boxes: {unreadable(output)}")]
    if src is None:
        return None, None, [("box:unreadable", f"the source's boxes: {unreadable(source)}")]
    return src, out, []


def _fraction(value):
    if isinstance(value, dict):
        return Fraction(value["num"], value["den"])
    return Fraction(value)


@_guarded
def check_clip(entry, several=False):
    """A clip: every kept track's samples, sample entry,
    timescale and headers, the movie's, and the audio edit (for a source
    with one window: `several` says whether either file of a track has
    several edits, judged by the oracle's own reader)."""
    src_movie, out_movie, found = _load(entry["source"], entry["output"])
    if found:
        return found
    kept = [("video", entry["video"])] + ([("audio", entry["audio"])] if entry.get("audio") else [])
    if len(out_movie["tracks"]) != len(kept):
        return [("box:tracks", f"{len(out_movie['tracks'])} tracks, {len(kept)} kept")]
    for out_index, (kind, track) in enumerate(kept):
        src_track = tb.track_by_index(src_movie, track["stream_index"])
        if src_track is None:
            found.append((f"box:{kind}:samples", f"{kind}: no source track {track['stream_index']}"))
            continue
        out_track = out_movie["tracks"][out_index]
        first = track["first_sample"]
        found += compare_samples(kind, entry["source"], src_track, [(entry["output"], out_track["samples"])],
                                 first, track["samples"], "run" if kind == "video" else None)
        if kind == "video":
            found += compare_cut(entry["source"], src_track, first)
        found += compare_entry(kind, src_track, out_track)
        found += clip_headers(kind, src_movie, src_track, out_movie, out_track, first)
        if kind == "audio":
            # The audio is cut at the clip's end: the requested one, or the
            # video's end where that is earlier. Only the video's edit is rounded up to a
            # whole video tick (point 5), which never moves it past the
            # video's end, a whole tick.
            written = _fraction(entry["written_end"])
            end = min(_fraction(entry["end"]), written) if entry.get("end") else written
            zero = _fraction(entry["snapped_start"])
            if source_window(src_movie, src_track) is not None:
                found += audio_edit(src_movie, src_track, out_movie, out_track, first, zero, end)
            else:
                found += audio_edits_several(src_movie, src_track, out_movie, out_track, first, zero, end)
    found += clip_movie(out_movie, several)
    return found


@_guarded
def check_segment(entry, segment, several=False):
    """One split segment (`docs/design/split.md`): as a clip, the sync
    claim taken over the whole track, the audio edit without the
    segment's end (its last frame may run past it)."""
    src_movie, out_movie, found = _load(entry["source"], segment["output"])
    if found:
        return found
    kept = [("video", entry["video_stream"], segment["video_first"], segment["video_samples"])]
    if entry.get("audio_stream") is not None and segment.get("audio_samples"):
        kept.append(("audio", entry["audio_stream"], segment["audio_first"], segment["audio_samples"]))
    if len(out_movie["tracks"]) != len(kept):
        return [("box:tracks", f"{len(out_movie['tracks'])} tracks, {len(kept)} kept")]
    for out_index, (kind, index, first, count) in enumerate(kept):
        src_track = tb.track_by_index(src_movie, index)
        if src_track is None:
            found.append((f"box:{kind}:samples", f"{kind}: no source track {index}"))
            continue
        out_track = out_movie["tracks"][out_index]
        found += compare_samples(kind, entry["source"], src_track, [(segment["output"], out_track["samples"])],
                                 first, count, "track" if kind == "video" else None)
        # Every segment but one starting with the source's first sample
        # starts at a cut point (`split.md`, "Where it cuts").
        if kind == "video" and first:
            found += compare_cut(entry["source"], src_track, first)
        found += compare_entry(kind, src_track, out_track)
        found += clip_headers(kind, src_movie, src_track, out_movie, out_track, first)
        if kind == "audio":
            zero = _fraction(segment["start"])
            if source_window(src_movie, src_track) is not None:
                found += audio_edit(src_movie, src_track, out_movie, out_track, first, zero)
            else:
                shown = split_audio_range(src_movie, src_track, first, count)
                if shown is None:
                    found.append(("box:audio:edit", "audio edit: no source edit shows the segment's audio"))
                else:
                    found += audio_edits_several(src_movie, src_track, out_movie, out_track, first, zero,
                                                 shown[1], begin=shown[0])
    found += clip_movie(out_movie, several)
    return found


@_guarded
def check_copy(source, output, kept, writer):
    """A stream copy of whole tracks: remux and sanitize
    (`docs/design/remux.md`) or extraction, `kept` the
    source track index of each output track in order. Every sample, the
    sample entry, the media timescale and the media duration (the source's
    declared one when it has no fragments and declares one, else the sum
    of the decode durations); for extraction also the movie timescale (the
    source's), the track duration (the sum of the edits, or without an edit
    list the media duration in movie ticks, rounded to the nearest) and the
    movie duration. Remux's track and movie durations are its oracle's own
    checks."""
    src_movie, out_movie, found = _load(source, output)
    if found:
        return found
    if len(out_movie["tracks"]) != len(kept):
        return [("box:tracks", f"{len(out_movie['tracks'])} tracks, {len(kept)} kept")]
    for out_track, index in zip(out_movie["tracks"], kept):
        src_track = tb.track_by_index(src_movie, index)
        kind = kind_of(out_track)
        if src_track is None:
            found.append((f"box:{kind}:samples", f"{kind}: no source track {index}"))
            continue
        found += compare_samples(kind, source, src_track, [(output, out_track["samples"])], 0, None)
        if kind in ("video", "audio"):
            # Sanitization rewrites H.264 and HEVC configurations to their
            # parameter sets; its oracle compares them (`sanitize.md`).
            found += compare_entry(kind, src_track, out_track, video_config=writer != "sanitize")
        found += compare_timescale(kind, src_track, out_track)
        found += compare_media_duration(kind, out_track, declared_or_sum(src_movie, src_track, out_track))
        if writer == "extract":
            if out_movie["movie_timescale"] != src_movie["movie_timescale"]:
                found.append(("box:movie timescale", f"movie timescale {out_movie['movie_timescale']}, source "
                              f"{src_movie['movie_timescale']}"))
            expected = edits_duration(out_track) if out_track["edits"] is not None else _nearest(
                Fraction(out_track["mdhd_duration"] * out_movie["movie_timescale"], out_track["timescale"]))
            found += compare_track_duration(kind, out_track, expected)
    if writer == "extract":
        found += compare_movie_duration(out_movie)
    return found


@_guarded
def check_rendition(source, stream_index, init, segments):
    """An HLS rendition (`docs/design/hls.md`, "Boxes" and "Timing"): the
    init segment's sample entry and timescale are the source track's, and
    the media segments' samples, taken in order, are its samples from where
    `hls.md` starts the rendition to where the edit stops presenting."""
    src_movie, init_movie, found = _load(source, init)
    if found:
        return found
    src_track = tb.track_by_index(src_movie, stream_index)
    if src_track is None or len(init_movie["tracks"]) != 1:
        return [("box:tracks", f"init segment: {len(init_movie['tracks'])} tracks, source track "
                 f"{stream_index}")]
    out_track = init_movie["tracks"][0]
    kind = kind_of(out_track)
    parts = tb.segment_samples(segments, out_track["id"], init_movie["trex"])
    src = src_track["samples"]
    count = sum(len(p["sizes"]) for _, p in parts)
    if kind == "audio" and count == len(src["sizes"]):
        # Every sample kept: nothing to place on the timeline (a source
        # whose fragments leave decode time gaps included).
        window = None
    else:
        window = source_window(src_movie, src_track)
        if window is None:
            return found + [(f"box:{kind}:samples", f"{kind}: the source has several media edits")]
    first = hls_start(source, src_track, window, kind)
    end = first + count
    if end > len(src["sizes"]):
        return found + [(f"box:{kind}:samples", f"{kind} samples: {count} in the media segments from source "
                         f"sample {first}, the source has {len(src['sizes'])}")]
    starts = decode_times(src)
    if end < len(src["sizes"]) and any(starts[i] + signed(src["cto"][i]) < window[1]
                                       for i in range(end, len(src["sizes"]))):
        return found + [(f"box:{kind}:samples", f"{kind} samples: {count} from sample {first}; a trailing "
                         f"sample left out is presented")]
    # Sync flags as stored: the packager rewrites no claim (hls.md, "Boxes").
    found += compare_samples(kind, source, src_track, parts, first, count)
    found += compare_entry(kind, src_track, out_track)
    found += compare_timescale(kind, src_track, out_track)
    return found


def signed(offset):
    """A composition offset as players read it: 32 bits, signed (`hls.md`
    plans with offsets of 2^31 and more of a version 0 table so read)."""
    offset %= 1 << 32
    return offset - (1 << 32) if offset >= 1 << 31 else offset


def decode_times(samples):
    times, at = [], samples["base_dts"]
    for duration in samples["durations"]:
        times.append(at)
        at += duration
    return times


def _starts_coded_sequence(codec, sample, size):
    """An IDR picture (H.264) or a first coded slice that is an IRAP
    picture (HEVC), read from NAL unit headers (`hls.md`, "Segments")."""
    if codec == "h264":
        return any(nal[0] & 0x1F == H264_IDR for nal in _nal_units(sample, size) or [])
    return hevc_access(sample, size) == "must"


def hls_start(source, track, window, kind):
    """The source sample a rendition starts with (`hls.md`, "Timing"):
    audio keeps every sample, however early the edit starts; video
    starts as `hls_video_start` says."""
    return hls_video_start(source, track, window) if kind == "video" else 0


def hls_video_start(source, track, window):
    """The sample a video rendition starts with (`hls.md`, "Timing"): the
    latest sync sample presented at or before the edit's media start after
    every earlier sample in decode order, while every flag so far is sync
    an IDR or IRAP picture; the first sample when none is."""
    samples = track["samples"]
    m0 = window[0]
    starts = decode_times(samples)
    codec = codec_of(track["entry"])
    size = _length_size(track["entry"]) if codec in ("h264", "hevc") else None
    found, latest, all_sync = 0, None, True
    for k, start in enumerate(starts):
        pts = start + signed(samples["cto"][k])
        all_sync = all_sync and bool(samples["sync"][k])
        if samples["sync"][k] and pts <= m0 and (latest is None or pts > latest):
            if all_sync and size is not None:
                sample = next(tb.iter_sample_bytes(source, samples["offsets"], samples["sizes"], k, 1))
                if not _starts_coded_sequence(codec, sample, size):
                    latest = pts if latest is None else max(latest, pts)
                    continue
            found = k
        latest = pts if latest is None else max(latest, pts)
    return found
