#!/usr/bin/env python3
"""Black-box oracles for full-file remuxes (docs/design/remux.md).

Reads a manifest of remuxed files (from the `fixtures_round_trip` test of
coreav-isobmff-write, COREAV_REMUX_OUT, or from the corpus harness): each
entry names the source, the output, the kept streams (`stream_index` in the
source, in output order) and the dropped stream indexes. Checks every
output against its source with the pinned ffprobe and ffmpeg binaries, and
with MediaInfo when --mediainfo is given:

- layout: the output's top-level boxes are exactly `ftyp`, `moov`, `mdat`,
  in that order (the movie box first), and ffprobe reads it as an MP4 file
  (major brand `isom`);
- streams: every source stream ffprobe lists is kept or reported as
  dropped; each kept stream has the source stream's codec, profile,
  dimensions, pixel format, colour description, field order, sample rate,
  channel layout, time base, start, duration, frame count, rotation,
  disposition, language and tags (`handler_name` and `vendor_id` aside:
  the remux writes its own handler and no QuickTime vendor);
- packet checksums: `ffmpeg -copyts -i FILE -map 0:N -c copy -f framemd5`
  of each kept stream equals the source stream's: the stream header
  (codec, extradata checksum, time base, sample rate, channel layout) and
  every packet's times, duration, size, MD5 and side data. Every packet is
  counted towards the packet bar;
- metadata: ffprobe's format tags equal the source's (brands aside), so the
  movie's `udta` and `meta` boxes carry the same values;
- MediaInfo, when given: an MPEG-4 file whose tracks have the source
  tracks' format, profile, dimensions, rotation, sample rate, channels,
  channel layout, duration and frame count.

An entry with a `keep` list was sanitized (docs/design/sanitize.md), keeping
what it names (`rotation`, `language`, `dates`, `gapless`, `kinds`). The
media checks above are the same (they also show that the kept gapless
information and track kinds play as the source's); metadata differences
become requirements:

- only allowlisted boxes in the movie box tree, generic handlers; no
  `udta` or `meta` box in the movie or a track but, as kept, a track `udta`
  holding `kind` boxes from the DASH role and HTML kind vocabularies and a
  `udta` with exactly the canonical `iTunSMPB` `meta` box (ffprobe's
  `iTunSMPB` format tag); H.264 and HEVC configurations equal to the
  source's parameter sets, `colr`, `sv3d` and fixed-layout boxes in their
  written forms, sample entry children
  only from the allowlist, an empty compressor name, and creation times of
  0 unless `dates` is kept (read from the box headers);
- ffprobe: no format tags but the brands (and `creation_time`, equal to
  the source's, with `dates`), no stream tags but `language` (the source's
  with `language`, else `und`), `handler_name`, `vendor_id` and
  `creation_time` (with `dates`), no attached picture, the rotation the
  source's with `rotation`, else 0;
- MediaInfo: only structural keys in the general track, and no title,
  date, location, device, application or language (unless kept) keys in a
  stream's track. Keys MediaInfo reads from the coded video itself (the
  encoder's settings, `Encoded_Library*`) are not metadata boxes and are
  allowed.

A difference counts as agreement only when an entry of
docs/known-divergences-remux.md covers it: each entry's
predicate below rechecks its conditions from the files' box headers and the
oracles' own output. Covered differences are reported apart from direct
agreement and never hide another difference of the same output.

The binaries are black boxes; no implementation source
is read. Exits 1 if any check fails or the manifest is empty.
"""

import argparse
import fnmatch
import json
import os
import pathlib
import re
import struct
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import writer_boxes  # noqa: E402

STREAM_KEYS = (
    "codec_type", "codec_name", "profile", "codec_tag_string", "width", "height", "coded_width",
    "coded_height", "pix_fmt", "level", "color_range", "color_space", "color_transfer",
    "color_primaries", "chroma_location", "field_order", "sample_aspect_ratio", "sample_fmt",
    "sample_rate", "channels", "channel_layout", "bits_per_sample", "time_base", "start_pts",
    "duration_ts", "nb_frames", "r_frame_rate", "avg_frame_rate", "extradata_hash",
)
IGNORED_STREAM_TAGS = ("handler_name", "vendor_id")
IGNORED_FORMAT_TAGS = ("major_brand", "minor_version", "compatible_brands")
# Sanitization (docs/design/sanitize.md).
SANITIZED_FORMAT_TAGS = ("major_brand", "minor_version", "compatible_brands")
SANITIZED_STREAM_TAGS = ("language", "handler_name", "vendor_id")
CANONICAL_GAPLESS = re.compile(r" 00000000 [0-9A-F]{8} [0-9A-F]{8} [0-9A-F]{16}( 00000000){8}")
# The boxes a sanitized movie box may hold, by parent (ISO/IEC 14496-12).
SANITIZED_TREE = {
    "moov": {"mvhd", "trak", "udta"},
    "trak": {"tkhd", "edts", "mdia", "udta"},
    "edts": {"elst"},
    "mdia": {"mdhd", "hdlr", "minf"},
    "minf": {"vmhd", "smhd", "dinf", "stbl"},
    "dinf": {"dref"},
    "stbl": {"stsd", "stts", "ctts", "stsc", "stsz", "stco", "co64", "stss", "sgpd", "sbgp", "sdtp", "subs",
             "cslg", "stps", "padb", "stdp", "stsh"},
}
# The written handlers: type and name (`hdlr` after version and flags).
SANITIZED_HANDLERS = {bytes(8) + b"vide" + bytes(12) + b"VideoHandler\0",
                      bytes(8) + b"soun" + bytes(12) + b"SoundHandler\0"}
# Track kinds kept (ISO/IEC 23009-1 roles, HTML track kinds).
KINDS = {
    "urn:mpeg:dash:role:2011": {"caption", "subtitle", "main", "alternate", "supplementary", "commentary", "dub",
                                "description", "sign", "metadata", "enhanced-audio-intelligibility",
                                "emergency", "forced-subtitle", "easyreader", "karaoke"},
    "about:html-kind": {"alternative", "captions", "descriptions", "main", "main-desc", "sign", "subtitles",
                        "translation", "commentary"},
}
# The `meta` box sanitization writes for gapless information, but the value.
GAPLESS_HDLR = bytes(8) + b"mdirappl" + bytes(9)
VISUAL_KEPT = ("avcC", "hvcC", "av1C", "vpcC", "pasp", "clap", "colr", "btrt", "mdcv", "clli", "SmDm",
               "CoLL", "dvcC", "dvvC", "dvwC", "fiel", "st3d", "sv3d")
AUDIO_CONFIGS = ("esds", "dOps", "dfLa", "alac", "dac3", "dec3")
AUDIO_KEPT = (*AUDIO_CONFIGS, "btrt", "chnl")
MEDIAINFO_GENERAL_KEYS = (
    "@type", "Count", "StreamCount", "StreamKind", "VideoCount", "AudioCount", "TextCount", "FileExtension",
    "Format", "Format_Profile", "Format_Version", "CodecID", "CodecID_Compatible", "CodecID_Version", "FileSize",
    "Duration", "OverallBitRate", "OverallBitRate_Mode", "FrameRate", "FrameCount", "StreamSize", "HeaderSize",
    "DataSize", "FooterSize", "IsStreamable", "File_Modified_Date", "File_Modified_Date_Local", "extra",
)
MEDIAINFO_TRACK_DENIED = ("Title", "Date", "Location", "xyz", "Encoded_Application", "Encoded_Hardware",
                          "Make", "Model", "Comment", "Copyright", "Description", "Performer", "Album")
MEDIAINFO_KEYS = (
    "Format", "Format_Profile", "Format_Level", "Format_Tier", "Format_AdditionalFeatures",
    "CodecID", "Width", "Height", "Rotation", "PixelAspectRatio", "DisplayAspectRatio",
    "SamplingRate", "Channels", "ChannelLayout", "Duration", "FrameCount", "FrameRate",
    "BitDepth", "ColorSpace", "ChromaSubsampling", "colour_primaries",
    "transfer_characteristics", "matrix_coefficients", "colour_range", "HDR_Format", "Language",
)
KD_FILE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "known-divergences-remux.json"
THREADS = ["-threads", os.environ["REMUX_CHECK_THREADS"]] if os.environ.get("REMUX_CHECK_THREADS") else []


def run(args):
    result = subprocess.run(args, capture_output=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(f"{args[0]} failed ({result.returncode}): {result.stderr.decode(errors='replace')[:400]}")
    return result.stdout


def ffprobe(binary, path):
    return json.loads(run([binary, "-v", "error", "-of", "json", "-show_streams", "-show_format",
                           "-show_data_hash", "SHA256", path]))


def framemd5(binary, path, index):
    """The framemd5 lines of one stream copied as is, without the line
    naming the muxer's version."""
    out = run([binary, "-v", "error", *THREADS, "-copyts", "-i", path, "-map", f"0:{index}", "-c", "copy",
               "-f", "framemd5", "-"])
    return [line for line in out.decode(errors="replace").splitlines() if not line.startswith("#software")]


def _boxes(f, at, end):
    """(type, payload offset, end) of every box in [at, end) of an open file
    (ISO/IEC 14496-12 4.2); stops at a box that does not fit."""
    while at + 8 <= end:
        f.seek(at)
        head = f.read(16)
        size, kind = struct.unpack(">I4s", head[:8])
        header = 8
        if size == 1:
            size, header = struct.unpack(">Q", head[8:16])[0], 16
        elif size == 0:
            size = end - at
        if size < header or at + size > end:
            return
        if kind == b"uuid":
            header += 16
        yield kind.decode("latin1"), at + header, at + size
        at += size


def major_brand(path):
    with open(path, "rb") as f:
        ftyp = next(((a, e) for k, a, e in _boxes(f, 0, f.seek(0, 2)) if k == "ftyp"), None)
        if ftyp is None:
            return None
        f.seek(ftyp[0])
        return f.read(4).decode("latin1")


def trak_count(path):
    with open(path, "rb") as f:
        moov = next(((a, e) for k, a, e in _boxes(f, 0, f.seek(0, 2)) if k == "moov"), None)
        return 0 if moov is None else sum(1 for k, _, _ in _boxes(f, *moov) if k == "trak")


def top_level(path):
    """Top-level box types of a file."""
    with open(path, "rb") as f:
        return [kind for kind, _, _ in _boxes(f, 0, f.seek(0, 2))]


def trak_box(path, stream_index, names):
    """Payload of the box at `names` (such as `mdia`, `minf`, `stbl`,
    `stts`) under the `trak` at `stream_index`, or None."""
    with open(path, "rb") as f:
        moov = next(((a, e) for k, a, e in _boxes(f, 0, f.seek(0, 2)) if k == "moov"), None)
        if moov is None:
            return None
        traks = [(a, e) for k, a, e in _boxes(f, *moov) if k == "trak"]
        if stream_index >= len(traks):
            return None
        at, end = traks[stream_index]
        for name in names:
            found = next(((a, e) for k, a, e in _boxes(f, at, end) if k == name), None)
            if found is None:
                return None
            at, end = found
        f.seek(at)
        return f.read(end - at)


def trak_fields(path, stream_index):
    """Track header fields (ISO/IEC 14496-12 8.3.2), the edit list entries
    (8.6.6) and the sample entry's child box types of a track, or None."""
    tkhd = trak_box(path, stream_index, ["tkhd"])
    if not tkhd or len(tkhd) < 84:
        return None
    version = tkhd[0]
    at = 4 + (32 if version == 1 else 20) + 8
    if len(tkhd) < at + 8 + 36 + 8:
        return None
    layer, group, volume = struct.unpack(">hhh", tkhd[at:at + 6])
    matrix = struct.unpack(">9i", tkhd[at + 8:at + 44])
    width, height = struct.unpack(">II", tkhd[at + 44:at + 52])
    duration = struct.unpack(">Q", tkhd[28:36])[0] if version == 1 else struct.unpack(">I", tkhd[20:24])[0]
    fields = {"flags": int.from_bytes(tkhd[1:4], "big"), "layer": layer, "alternate_group": group,
              "duration": duration,
              "matrix": matrix, "width": width, "height": height, "volume": volume, "edits": None}
    elst = trak_box(path, stream_index, ["edts", "elst"])
    if elst and len(elst) >= 8:
        count = struct.unpack(">I", elst[4:8])[0]
        width_, layout = (20, ">Qqi") if elst[0] == 1 else (12, ">Iii")
        fields["edits"] = [list(struct.unpack(layout, elst[8 + k * width_:8 + (k + 1) * width_]))
                           for k in range(min(count, (len(elst) - 8) // width_))]
    fields["entry"] = entry_children(path, stream_index)
    return fields


def media_header(path, stream_index):
    """(timescale, duration) of a track's media header (8.4.2), or None."""
    mdhd = trak_box(path, stream_index, ["mdia", "mdhd"])
    if not mdhd or len(mdhd) < 20:
        return None
    if mdhd[0] == 1:
        return struct.unpack(">I", mdhd[20:24])[0], struct.unpack(">Q", mdhd[24:32])[0]
    return struct.unpack(">II", mdhd[12:20])


def movie_header(path):
    """(timescale, duration) of the movie header (8.2.2), or None."""
    with open(path, "rb") as f:
        moov = next(((a, e) for k, a, e in _boxes(f, 0, f.seek(0, 2)) if k == "moov"), None)
        if moov is None:
            return None
        mvhd = next(((a, e) for k, a, e in _boxes(f, *moov) if k == "mvhd"), None)
        if mvhd is None:
            return None
        f.seek(mvhd[0])
        body = f.read(36)
    if body[:1] == b"\x01":
        return struct.unpack(">I", body[20:24])[0], struct.unpack(">Q", body[24:32])[0]
    return struct.unpack(">II", body[12:20])


def first_decode_time(path, stream_index):
    """The decode time of a track's first sample: 0 with samples in the
    movie box, else the first fragment's base decode time (8.8.12)."""
    stts = stts_entries(path, stream_index)
    if stts and sum(n for n, _ in stts) > 0:
        return 0
    tkhd = trak_box(path, stream_index, ["tkhd"])
    if not tkhd or len(tkhd) < 24:
        return 0
    at = 20 if tkhd[0] == 1 else 12
    track_id = struct.unpack(">I", tkhd[at:at + 4])[0]
    with open(path, "rb") as f:
        for kind, a, e in _boxes(f, 0, f.seek(0, 2)):
            if kind != "moof":
                continue
            for k2, a2, e2 in _boxes(f, a, e):
                if k2 != "traf":
                    continue
                mine, base = False, 0
                for k3, a3, e3 in _boxes(f, a2, e2):
                    f.seek(a3)
                    body = f.read(min(e3 - a3, 16))
                    if k3 == "tfhd" and len(body) >= 8:
                        mine = struct.unpack(">I", body[4:8])[0] == track_id
                    if k3 == "tfdt" and len(body) >= 8:
                        base = struct.unpack(">Q", body[4:12])[0] if body[0] == 1 else struct.unpack(">I", body[4:8])[0]
                if mine:
                    return base
    return 0


def stts_entries(path, stream_index):
    stts = trak_box(path, stream_index, ["mdia", "minf", "stbl", "stts"])
    if not stts or len(stts) < 8:
        return None
    count = struct.unpack(">I", stts[4:8])[0]
    return [struct.unpack(">II", stts[8 + 8 * k:16 + 8 * k]) for k in range(min(count, (len(stts) - 8) // 8))]


def mdhd_duration(path, stream_index):
    mdhd = trak_box(path, stream_index, ["mdia", "mdhd"])
    if not mdhd or len(mdhd) < 20:
        return None
    return struct.unpack(">Q", mdhd[24:32])[0] if mdhd[0] == 1 else struct.unpack(">I", mdhd[16:20])[0]


def last_fragment_duration(path, stream_index):
    """The stored decode duration of the last sample of a track in the movie
    fragments (ISO/IEC 14496-12 8.8.7, 8.8.8, 8.8.3), or None."""
    tkhd = trak_box(path, stream_index, ["tkhd"])
    if not tkhd or len(tkhd) < 24:
        return None
    # After version and flags: creation and modification times (4 or 8
    # bytes each), then track_ID.
    at = 20 if tkhd[0] == 1 else 12
    track_id = struct.unpack(">I", tkhd[at:at + 4])[0]
    last = None
    with open(path, "rb") as f:
        total = f.seek(0, 2)
        trex_default = None
        for kind, at, end in _boxes(f, 0, total):
            if kind == "moov":
                for k2, a2, e2 in _boxes(f, at, end):
                    if k2 == "mvex":
                        for k3, a3, e3 in _boxes(f, a2, e2):
                            if k3 == "trex":
                                f.seek(a3)
                                body = f.read(e3 - a3)
                                if len(body) >= 24 and struct.unpack(">I", body[4:8])[0] == track_id:
                                    trex_default = struct.unpack(">I", body[12:16])[0]
            if kind != "moof":
                continue
            for k2, a2, e2 in _boxes(f, at, end):
                if k2 != "traf":
                    continue
                default = trex_default
                mine = False
                for k3, a3, e3 in _boxes(f, a2, e2):
                    f.seek(a3)
                    body = f.read(min(e3 - a3, 1 << 24))
                    if k3 == "tfhd" and len(body) >= 8:
                        flags = int.from_bytes(body[1:4], "big")
                        mine = struct.unpack(">I", body[4:8])[0] == track_id
                        at_ = 8 + (8 if flags & 1 else 0) + (4 if flags & 2 else 0)
                        if flags & 8 and len(body) >= at_ + 4:
                            default = struct.unpack(">I", body[at_:at_ + 4])[0]
                    elif k3 == "trun" and mine and len(body) >= 8:
                        flags = int.from_bytes(body[1:4], "big")
                        count = struct.unpack(">I", body[4:8])[0]
                        at_ = 8 + (4 if flags & 1 else 0) + (4 if flags & 4 else 0)
                        width = sum(4 for bit in (0x100, 0x200, 0x400, 0x800) if flags & bit)
                        if count == 0:
                            continue
                        if flags & 0x100:
                            entry = at_ + (count - 1) * width
                            if len(body) >= entry + 4:
                                last = struct.unpack(">I", body[entry:entry + 4])[0]
                        else:
                            last = default
    return last


def last_sample_delta(path, stream_index):
    """The decode duration of a track's last sample, from its `stts`
    (ISO/IEC 14496-12 8.6.1.2)."""
    stts = trak_box(path, stream_index, ["mdia", "minf", "stbl", "stts"])
    if not stts or len(stts) < 8:
        return None
    count = struct.unpack(">I", stts[4:8])[0]
    if count == 0 or len(stts) < 8 + 8 * count:
        return None
    return struct.unpack(">I", stts[8 + 8 * (count - 1) + 4:8 + 8 * count])[0]


def entry_children(path, stream_index):
    """Child box types of the first sample entry of the `trak` at
    `stream_index`."""
    return [k for k, _ in entry_boxes(path, stream_index)]


def entry_boxes(path, stream_index):
    """(type, payload) of the children of the first sample entry of the
    `trak` at `stream_index` (ISO/IEC 14496-12 8.5.2; the fixed fields of a
    visual entry, or of a sound entry in its QuickTime version 0, 1 or 2
    layout, come first)."""
    with open(path, "rb") as f:
        moov = next(((a, e) for k, a, e in _boxes(f, 0, f.seek(0, 2)) if k == "moov"), None)
        if moov is None:
            return []
        traks = [(a, e) for k, a, e in _boxes(f, *moov) if k == "trak"]
        if stream_index >= len(traks):
            return []
        path_ = ["mdia", "minf", "stbl", "stsd"]
        at, end = traks[stream_index]
        handler = None
        for name in path_:
            found = None
            for k, a, e in _boxes(f, at, end):
                if k == "hdlr" and name == "minf":
                    f.seek(a + 8)
                    handler = f.read(4)
                if k == name:
                    found = (a, e)
            if found is None:
                return []
            at, end = found
        entry = next(_boxes(f, at + 8, end), None)
        if entry is None:
            return []
        _, a, e = entry
        if handler == b"vide":
            fixed = 78
        elif handler == b"soun":
            f.seek(a + 8)
            version = struct.unpack(">H", f.read(2))[0]
            fixed = {1: 44, 2: 64}.get(version, 28)
        else:
            return []
        found = []
        for k, x, y in _boxes(f, a + fixed, e):
            f.seek(x)
            found.append((k, f.read(y - x)))
        return found


def _nal_units(config, at, count):
    """`count` length-prefixed NAL units from `at`: (units, end), or None."""
    units = []
    for _ in range(count):
        if at + 2 > len(config):
            return None
        end = at + 2 + struct.unpack(">H", config[at:at + 2])[0]
        if end > len(config):
            return None
        units.append(config[at + 2:end])
        at = end
    return units, at


def _length_prefixed(units):
    return b"".join(struct.pack(">H", len(u)) + u for u in units)


def hevc_parameter_sets(config):
    """An HEVC configuration (ISO/IEC 14496-15 8.3.3.1) with only its VPS,
    SPS and PPS (NAL unit types 32 to 34, each unit's header matching its
    array), as sanitization writes it; None when it does not parse."""
    if len(config) < 23:
        return None
    at, arrays = 23, []
    for _ in range(config[22]):
        if at + 3 > len(config):
            return None
        head, count = config[at], struct.unpack(">H", config[at + 1:at + 3])[0]
        parsed = _nal_units(config, at + 3, count)
        if parsed is None:
            return None
        units, at = parsed
        nal_type = head & 0x3F
        kept = [u for u in units if 32 <= nal_type <= 34 and u[:1] and (u[0] >> 1) & 0x3F == nal_type]
        if kept:
            arrays.append(bytes([head]) + struct.pack(">H", len(kept)) + _length_prefixed(kept))
    return config[:22] + bytes([len(arrays)]) + b"".join(arrays)


def avc_parameter_sets(config):
    """An H.264 configuration (ISO/IEC 14496-15 5.3.3.1) with only its SPS
    and PPS (NAL unit types 7 and 8) and, for the profiles that have them,
    the extension fields with SPS extensions (type 13); None when it does
    not parse."""
    if len(config) < 6:
        return None
    out, at = bytearray(config[:5]), 5
    for mask, nal_type in ((0x1F, 7), (0xFF, 8)):
        if at >= len(config):
            return None
        head = config[at]
        parsed = _nal_units(config, at + 1, head & mask)
        if parsed is None:
            return None
        units, at = parsed
        kept = [u for u in units if u[:1] and u[0] & 0x1F == nal_type]
        out += bytes([(head & ~mask & 0xFF) | len(kept)]) + _length_prefixed(kept)
    if config[1] in (100, 110, 122, 144) and len(config) >= at + 4:
        fields = config[at:at + 4]
        parsed = _nal_units(config, at + 4, fields[3])
        if parsed is None:
            return None
        units, at = parsed
        kept = [u for u in units if u[:1] and u[0] & 0x1F == 13]
        out += fields[:3] + bytes([len(kept)]) + _length_prefixed(kept)
    return bytes(out)


# Payload sizes of the fixed-layout boxes sanitization keeps.
FIXED_SIZES = {"pasp": 8, "btrt": 12, "clap": 32, "clli": 4, "mdcv": 24, "SmDm": 28, "CoLL": 8, "dvcC": 24,
               "dvvC": 24, "dvwC": 24, "fiel": 2, "st3d": 5}


CONFIGURATIONS = {"hvcC": hevc_parameter_sets, "avcC": avc_parameter_sets}


def configuration_extras_left_out(src, index):
    """Whether sanitizing the stream leaves NAL units of its H.264 or HEVC
    configuration out, so its extradata differs by design."""
    for kind, payload in entry_boxes(src, index):
        if kind in CONFIGURATIONS and CONFIGURATIONS[kind](payload) != payload:
            return True
    return False


def rotation(stream):
    for side in stream.get("side_data_list") or []:
        if "rotation" in side:
            return side["rotation"]
    return 0


def tags(stream, ignored):
    out = {k: v for k, v in (stream.get("tags") or {}).items() if k not in ignored}
    if "language" in out or stream.get("codec_type") in ("video", "audio"):
        out["language"] = out.get("language", "und")
    return out


def mediainfo_tracks(binary, path):
    data = json.loads(run([binary, "--Output=JSON", path]))
    return data["media"]["track"]


def _packet(line):
    """(stream, dts, pts, duration, size, hash and side data) of a framemd5
    packet line."""
    parts = [part.strip() for part in line.split(",")]
    if len(parts) < 6:
        return None
    try:
        return int(parts[0]), int(parts[1]), int(parts[2]), int(parts[3]), int(parts[4]), tuple(parts[5:])
    except ValueError:
        return None


def problem(field, message, **data):
    return {"field": field, "message": message, "data": data}


def compare_framemd5(i, source, output, extradata=True):
    """(problems, packet lines of the output) of one stream's framemd5;
    without `extradata`, the extradata line of the header is not compared
    (the box-level checks compare it instead)."""
    found = []
    head = lambda lines: [x for x in lines if x.startswith("#") and (extradata or not x.startswith("#extradata"))]
    src_head, out_head = head(source), head(output)
    if src_head != out_head:
        found.append(problem(f"stream:{i}:framemd5:header",
                             f"stream {i} framemd5 header: {sorted(set(src_head) ^ set(out_head))[:4]}",
                             source=src_head, output=out_head))
    src_packets = [line for line in source if line and not line.startswith("#")]
    out_packets = [line for line in output if line and not line.startswith("#")]
    for n, (a, b) in enumerate(zip(src_packets, out_packets)):
        if a != b:
            following = out_packets[n + 1] if n + 1 < len(out_packets) else None
            found.append(problem(f"stream:{i}:packet:{n}", f"stream {i} packet {n}: {a!r} != {b!r}",
                                 source=a, output=b, next_output=following))
    if len(src_packets) != len(out_packets):
        found.append(problem(f"stream:{i}:packets:count",
                             f"stream {i}: {len(src_packets)} packets in the source, {len(out_packets)} in the output",
                             missing=abs(len(src_packets) - len(out_packets))))
    return found, len(out_packets)


def check(entry, ffprobe_bin, ffmpeg_bin, mediainfo_bin):
    """(problems, covered, packet counts) of one output."""
    src, out = entry["source"], entry["output"]
    keep = entry.get("keep")
    sanitized = keep is not None
    keep = set(keep or ())
    found = []
    kinds = top_level(out)
    if kinds != ["ftyp", "moov", "mdat"]:
        found.append(problem("layout", f"layout: top-level boxes {kinds}"))
    s, o = ffprobe(ffprobe_bin, src), ffprobe(ffprobe_bin, out)
    # From the box: ffprobe's `major_brand` tag can come from copied metadata.
    brand = major_brand(out)
    if brand != "isom":
        found.append(problem("format", f"format: major brand {brand!r}"))
    if "mp4" not in o["format"].get("format_name", ""):
        found.append(problem("format", f"format: {o['format'].get('format_name')!r}"))
    kept = [k["stream_index"] for k in entry["streams"]]
    # Cover art in the metadata is listed by ffprobe as an attached picture
    # stream after the tracks; it is metadata, compared by count. (A track,
    # such as an MJPEG chapter image track, can be flagged too.)
    def pictures(streams, path):
        traks = trak_count(path)
        return [st for st in streams if (st.get("disposition") or {}).get("attached_pic")
                and st["index"] >= traks]
    pics_s, pics_o = pictures(s["streams"], src), pictures(o["streams"], out)
    if sanitized and pics_o:
        found.append(problem("sanitize", f"sanitize: {len(pics_o)} attached pictures"))
    elif not sanitized and len(pics_s) != len(pics_o):
        found.append(problem("attached_pics", f"attached pictures: {len(pics_s)} != {len(pics_o)}"))
    tracks_s = [st for st in s["streams"] if st not in pics_s]
    tracks_o = [st for st in o["streams"] if st not in pics_o]
    listed = set(kept) | set(entry["dropped_streams"])
    unlisted = [st["index"] for st in tracks_s if st["index"] not in listed]
    if unlisted:
        found.append(problem("streams", f"streams: source streams {unlisted} neither kept nor reported as dropped"))
    if len(tracks_o) != len(kept):
        found.append(problem("streams", f"streams: {len(tracks_o)} in the output, {len(kept)} kept"))
    by_index = {st["index"]: st for st in tracks_s}
    packets = 0
    first = {}
    fragmented = "moof" in top_level(src)
    for i, index in enumerate(kept):
        if index not in by_index or i >= len(tracks_o):
            found.append(problem("streams", f"stream {i}: missing (source stream {index})"))
            continue
        a, b = by_index[index], tracks_o[i]
        src_lines, out_lines = framemd5(ffmpeg_bin, src, index), framemd5(ffmpeg_bin, out, i)
        first[i] = (next((x for x in src_lines if x and not x.startswith("#")), ""),
                    next((x for x in out_lines if x and not x.startswith("#")), ""))
        # Sanitization leaves NAL units other than parameter sets out of an
        # H.264 or HEVC configuration: the box-level checks compare it.
        extras = sanitized and configuration_extras_left_out(src, index)
        lines, count = compare_framemd5(i, src_lines, out_lines, extradata=not extras)
        found.extend(lines)
        packets += count
        found.extend(compare_boxes(i, src, index, out, fragmented, keep if sanitized else None))
        for key in STREAM_KEYS:
            if extras and key == "extradata_hash":
                continue
            if a.get(key) != b.get(key):
                found.append(problem(f"stream:{i}:{key}", f"stream {i} {key}: {a.get(key)!r} != {b.get(key)!r}",
                                     source=a.get(key), output=b.get(key), output_packets=count,
                                     output_avg=b.get("avg_frame_rate")))
        expected_rotation = rotation(a) if not sanitized or "rotation" in keep else 0
        if expected_rotation != rotation(b):
            found.append(problem(f"stream:{i}:rotation", f"stream {i} rotation: {expected_rotation} != {rotation(b)}"))
        if a.get("disposition") != b.get("disposition"):
            found.append(problem(f"stream:{i}:disposition",
                                 f"stream {i} disposition: {a.get('disposition')} != {b.get('disposition')}"))
        if sanitized:
            found.extend(sanitized_stream_tags(i, a, b, keep))
            continue
        ta, tb = tags(a, IGNORED_STREAM_TAGS), tags(b, IGNORED_STREAM_TAGS)
        for key in sorted(set(ta) | set(tb)):
            if ta.get(key) != tb.get(key):
                found.append(problem(f"stream:{i}:tag:{key}",
                                     f"stream {i} tag {key}: {ta.get(key)!r} != {tb.get(key)!r}",
                                     source=ta.get(key), output=tb.get(key)))
    src_tags = {k: v for k, v in s["format"].get("tags", {}).items() if k not in IGNORED_FORMAT_TAGS}
    out_tags = {k: v for k, v in o["format"].get("tags", {}).items() if k not in IGNORED_FORMAT_TAGS}
    if sanitized:
        allowed = {"creation_time": src_tags.get("creation_time")} if "dates" in keep else {}
        for key, value in out_tags.items():
            if key == "iTunSMPB" and "gapless" in keep and CANONICAL_GAPLESS.fullmatch(value):
                continue
            if key not in allowed or allowed[key] != value:
                found.append(problem("sanitize", f"sanitize: format tag {key}={value!r}"))
        src_tags = out_tags = {}
    for key in sorted(set(src_tags) | set(out_tags)):
        if src_tags.get(key) != out_tags.get(key):
            found.append(problem(f"format:tag:{key}",
                                 f"format tag {key}: {src_tags.get(key)!r} != {out_tags.get(key)!r}",
                                 source=src_tags.get(key), output=out_tags.get(key)))
    # What the black boxes do not read, from both files' own boxes
    # (scripts/writer_boxes.py): every sample with its sync flag, the sample
    # entries, the media timescales and durations.
    found.extend(problem(field, message) for field, message in writer_boxes.check_copy(src, out, kept,
                                                                                  "sanitize" if sanitized else "remux"))
    writer_boxes.forget()
    # The movie duration is the longest kept track (8.2.2).
    movie = movie_header(out)
    durations = [f["duration"] for f in (trak_fields(out, n) for n in range(len(kept))) if f]
    if movie and durations and movie[1] != max(durations):
        found.append(problem("box:mvhd", f"movie duration {movie[1]} != the longest track's {max(durations)}"))
    if mediainfo_bin:
        found.extend(check_mediainfo(mediainfo_bin, src, out, kept, keep if sanitized else None))
    if sanitized:
        found.extend(sanitized_boxes(out, keep))
    context = {"entry": entry, "source_top_level": top_level(src), "source_streams": s["streams"],
               "kept": kept, "first": first, "ffprobe": ffprobe_bin}
    problems, covered = apply_known_divergences(found, context)
    disagree = sum(_packets_of(p) for p in problems)
    agree = packets - sum(1 for p in problems if ":packet:" in p["field"])
    return problems, covered, {"agree": max(agree, 0), "disagree": disagree}


def compare_boxes(i, src, index, out, fragmented, keep=None):
    """The track header fields, edit list and sample entry children the
    remux copies, read from both files' box headers; for a sanitized output,
    the allowlisted children and the identity matrix unless the rotation
    is kept."""
    found = []
    a, b = trak_fields(src, index), trak_fields(out, i)
    if a is None or b is None:
        return [problem(f"box:{i}:tkhd", f"stream {i}: no readable track header")]
    if keep is not None and "rotation" not in keep:
        a = dict(a, matrix=(0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000))
    for key in ("flags", "layer", "alternate_group", "matrix", "width", "height"):
        if a[key] != b[key]:
            found.append(problem(f"box:{i}:{key}", f"stream {i} track header {key}: {a[key]} != {b[key]}"))
    if a["volume"] != b["volume"] and a["width"] == 0:
        found.append(problem(f"box:{i}:volume", f"stream {i} track header volume: {a['volume']} != {b['volume']}"))
    if not fragmented and a["edits"] != b["edits"] and (a["edits"] or b["edits"]):
        found.append(problem(f"box:{i}:edits", f"stream {i} edit list: {a['edits']} != {b['edits']}"))
    if fragmented:
        expected_edits = fragmented_edits(src, index, out, i)
        if expected_edits is not None and not edits_match(expected_edits, b["edits"] or []):
            found.append(problem(f"box:{i}:edits", f"stream {i} edit list: {expected_edits} != {b['edits']}"))
    # Track durations: the source's as stored when unfragmented (and not
    # 0), else the sum of the output's edits or its media duration.
    if not fragmented and a["duration"]:
        if b["duration"] != a["duration"]:
            found.append(problem(f"box:{i}:duration",
                                 f"stream {i} track duration: {a['duration']} != {b['duration']}"))
    else:
        movie, media = movie_header(out), media_header(out, i)
        if movie and media:
            expected = (sum(e[0] for e in b["edits"]) if b["edits"]
                        else round(media[1] * movie[0] / media[0]))
            if abs(b["duration"] - expected) > (0 if b["edits"] else 1):
                found.append(problem(f"box:{i}:duration",
                                     f"stream {i} track duration: {expected} != {b['duration']}"))
    # Audio: the configuration box first, QuickTime's wave and chan left out.
    expected = a["entry"]
    config = next((k for k in AUDIO_CONFIGS if k in expected or k in b["entry"]), None)
    if config:
        expected = [config] + [k for k in expected if k not in (config, "wave", "chan")]
    if keep is not None:
        allowed = AUDIO_KEPT if config else VISUAL_KEPT
        expected = [k for k in expected if k in allowed]
        source_boxes, output_boxes = entry_boxes(src, index), entry_boxes(out, i)
        # An ICC profile can name the device: only coded colour parameters stay.
        for kind, payload in source_boxes:
            if kind == "colr" and payload[:4] not in (b"nclx", b"nclc") and "colr" in expected:
                expected.remove("colr")
        for kind, rewrite in CONFIGURATIONS.items():
            config = next((p for k, p in source_boxes if k == kind), None)
            written = next((p for k, p in output_boxes if k == kind), None)
            if config is not None and written != rewrite(config):
                found.append(problem(f"box:{i}:{kind}", f"stream {i}: {kind} is not the source's parameter sets"))
        for kind, payload in output_boxes:
            size = {b"nclx": 11, b"nclc": 10}.get(payload[:4]) if kind == "colr" else FIXED_SIZES.get(kind)
            if (kind == "colr" or kind in FIXED_SIZES) and len(payload) != size:
                found.append(problem("sanitize", f"sanitize: stream {i} {kind} of {len(payload)} bytes"))
            if kind == "sv3d" and not spherical_written(payload):
                found.append(problem("sanitize", f"sanitize: stream {i} sv3d holds more than its projection"))
    if expected != b["entry"]:
        found.append(problem(f"box:{i}:entry", f"stream {i} sample entry boxes: {expected} != {b['entry']}"))
    return found


def spherical_written(payload):
    """Whether a `sv3d` payload is as sanitization writes it: an empty
    `svhd` and a `proj` of `prhd`, `equi`, `cbmp` or `mshp` only."""
    boxes = _payload_boxes(payload)
    if [k for k, _ in boxes] != ["svhd", "proj"] or boxes[0][1] != bytes(5):
        return False
    return all(k in ("prhd", "equi", "cbmp", "mshp") for k, _ in _payload_boxes(boxes[1][1]))


def _payload_boxes(payload):
    """(type, payload) of the boxes in a byte string."""
    found, at = [], 0
    while at + 8 <= len(payload):
        size = struct.unpack(">I", payload[at:at + 4])[0]
        if size < 8 or at + size > len(payload):
            break
        found.append((payload[at + 4:at + 8].decode("latin1"), payload[at + 8:at + size]))
        at += size
    return found


def fragmented_edits(src, index, out, i):
    """The edit list the remux must write for a fragmented source's track,
    in the output's movie ticks, as (duration, media time, rate, exact):
    the source's entries scaled exactly to the output's movie timescale with
    media times moved by the first decode time; a last entry of duration 0
    gets the media it presents; without an edit list, a first decode time
    above 0 becomes an empty edit then the media. Durations computed from
    media ticks may be rounded unless exact."""
    source = trak_fields(src, index)
    s_movie, o_movie, media = movie_header(src), movie_header(out), media_header(out, i)
    stts = stts_entries(out, i)
    if not source or not s_movie or not o_movie or not media or stts is None:
        return None
    m_src, m_out, ts = s_movie[0], o_movie[0], media[0]
    if m_out % m_src:
        return [("movie timescale", m_src, m_out)]
    total = sum(n * d for n, d in stts)
    d0 = first_decode_time(src, index)

    def ticks(media_ticks):
        exact = media_ticks * m_out % ts == 0
        return (media_ticks * m_out + ts // 2) // ts, exact
    entries = source["edits"] or []
    if not entries:
        if d0 == 0:
            return []
        (empty, e1), (span, e2) = ticks(d0), ticks(total)
        return [(empty, -1, 0x10000, e1), (span, 0, 0x10000, e2)]
    expected = []
    for n, (duration, media_time, rate) in enumerate(entries):
        moved = media_time if media_time == -1 else media_time - d0
        if n == len(entries) - 1 and duration == 0 and media_time >= 0:
            value, exact = ticks(total - moved)
            expected.append((value, moved, rate, exact))
        else:
            expected.append((duration * (m_out // m_src), moved, rate, True))
    return expected


def edits_match(expected, actual):
    if len(expected) != len(actual) or any(len(e) != 4 for e in expected):
        return False
    for (duration, media_time, rate, exact), (a_duration, a_time, a_rate) in zip(expected, actual):
        if a_time != media_time or (a_rate & 0xffffffff) != (rate & 0xffffffff):
            return False
        if a_duration != duration and (exact or abs(a_duration - duration) > 1):
            return False
    return True


def sanitized_stream_tags(i, a, b, keep):
    found = []
    allowed = set(SANITIZED_STREAM_TAGS) | ({"creation_time"} if "dates" in keep else set())
    generic = {"handler_name": ("VideoHandler", "SoundHandler"), "vendor_id": ("[0][0][0][0]",)}
    for key, value in (b.get("tags") or {}).items():
        if key not in allowed or value not in generic.get(key, (value,)):
            found.append(problem("sanitize", f"sanitize: stream {i} tag {key}={value!r}"))
    src_language = (a.get("tags") or {}).get("language", "und")
    language = (b.get("tags") or {}).get("language", "und")
    expected = src_language if "language" in keep else "und"
    if language != expected:
        found.append(problem(f"stream:{i}:tag:language", f"stream {i} language: {expected!r} != {language!r}"))
    if "dates" in keep and (a.get("tags") or {}).get("creation_time") != (b.get("tags") or {}).get("creation_time"):
        found.append(problem(f"stream:{i}:tag:creation_time", f"stream {i} creation_time differs"))
    return found


def sanitized_boxes(path, keep):
    """No metadata box in the movie box tree, an empty compressor name, and
    creation times of 0 unless the dates are kept (ISO/IEC 14496-12 8.2.2,
    8.3.2, 8.4.2, 12.1.3)."""
    found = []
    with open(path, "rb") as f:
        moov = next(((a, e) for k, a, e in _boxes(f, 0, f.seek(0, 2)) if k == "moov"), None)
        if moov is None:
            return [problem("sanitize", "sanitize: no movie box")]

        def walk(at, end, trail):
            for kind, a, e in _boxes(f, at, end):
                here = trail + [kind]
                if kind not in SANITIZED_TREE.get(trail[-1], set()):
                    found.append(problem("sanitize", f"sanitize: box {'/'.join(here)}"))
                if kind == "udta":
                    found.extend(problem("sanitize", f"sanitize: {'/'.join(here)} holds {what}")
                                 for what in kept_user_data(f, a, e, keep, in_track=trail[-1] == "trak"))
                if kind == "hdlr":
                    f.seek(a)
                    if f.read(e - a) not in SANITIZED_HANDLERS:
                        found.append(problem("sanitize", f"sanitize: {'/'.join(here)} not a generic handler"))
                if kind in ("mvhd", "tkhd", "mdhd") and "dates" not in keep:
                    f.seek(a)
                    head = f.read(20)
                    width = 8 if head[:1] == b"\x01" else 4
                    times = head[4:4 + 2 * width]
                    if any(times):
                        found.append(problem("sanitize", f"sanitize: {'/'.join(here)} times not 0"))
                if kind in ("trak", "mdia", "minf", "stbl", "edts", "dinf"):
                    walk(a, e, here)
                if kind == "stsd":
                    entry = next(_boxes(f, a + 8, e), None)
                    if entry is not None and entry[0] in ("avc1", "avc3", "hvc1", "hev1", "av01", "vp09"):
                        f.seek(entry[1] + 42)
                        if any(f.read(32)):
                            found.append(problem("sanitize", f"sanitize: compressor name in {'/'.join(here)}"))
        walk(moov[0], moov[1], ["moov"])
    return found


def kept_user_data(f, at, end, keep, in_track):
    """What a sanitized `udta` holds beyond what `keep` allows: in a track,
    `kind` boxes (version 0, a scheme and value of KINDS) with `kinds`, and
    with `gapless` one `meta` box: the `mdir` handler and an `ilst` with a
    single `----` item, `com.apple.iTunes` `iTunSMPB`, in canonical form."""
    def payload(a, e):
        f.seek(a)
        return f.read(e - a)

    extra = []
    for kind, a, e in _boxes(f, at, end):
        if kind == "kind" and "kinds" in keep and in_track:
            body = payload(a, e)
            parts = body[4:].split(b"\0")
            if (body[:4] == bytes(4) and len(parts) == 3 and parts[2] == b""
                    and parts[1].decode("latin1") in KINDS.get(parts[0].decode("latin1"), set())):
                continue
        if kind == "meta" and "gapless" in keep:
            children = list(_boxes(f, a + 4, e))
            generic = (payload(a, a + 4) == bytes(4) and [k for k, _, _ in children] == ["hdlr", "ilst"]
                       and payload(children[0][1], children[0][2]) == GAPLESS_HDLR)
            items = list(_boxes(f, children[1][1], children[1][2])) if generic else []
            if len(items) == 1 and items[0][0] == "----":
                parts = {k: payload(x, y) for k, x, y in _boxes(f, items[0][1], items[0][2])}
                value = parts.get("data", b"")[8:].decode("ascii", "replace")
                if (set(parts) == {"mean", "name", "data"}
                        and parts["mean"] == bytes(4) + b"com.apple.iTunes"
                        and parts["name"] == bytes(4) + b"iTunSMPB"
                        and CANONICAL_GAPLESS.fullmatch(value)):
                    continue
        extra.append(kind)
    return extra


def _packets_of(found):
    if ":packet:" in found["field"]:
        return 1
    if found["field"].endswith(":packets:count"):
        return found["data"]["missing"]
    return 0


def check_mediainfo(binary, src, out, kept, keep=None):
    found = []
    s, o = mediainfo_tracks(binary, src), mediainfo_tracks(binary, out)
    general = next((t for t in o if t.get("@type") == "General"), {})
    if general.get("Format") != "MPEG-4":
        found.append(problem("mediainfo:format", f"MediaInfo: format {general.get('Format')!r}"))
    if keep is not None:
        for key in general:
            if key not in MEDIAINFO_GENERAL_KEYS:
                found.append(problem("sanitize", f"sanitize: MediaInfo general {key}={general[key]!r}"))
        for track in o:
            if track.get("@type") == "General":
                continue
            for key in track:
                denied = any(word in key for word in MEDIAINFO_TRACK_DENIED)
                if denied or (key == "Language" and "language" not in keep):
                    found.append(problem("sanitize", f"sanitize: MediaInfo {track.get('@type')} {key}="
                                                     f"{track[key]!r}"))
    src_by_order = {int(t["StreamOrder"]): t for t in s if t.get("@type") != "General" and
                    str(t.get("StreamOrder", "")).isdigit()}
    out_by_order = {int(t["StreamOrder"]): t for t in o if t.get("@type") != "General" and
                    str(t.get("StreamOrder", "")).isdigit()}
    for i, index in enumerate(kept):
        a, b = src_by_order.get(index), out_by_order.get(i)
        if a is None:
            continue
        if b is None:
            found.append(problem(f"mediainfo:{i}:missing", f"MediaInfo: no track for stream {i}"))
            continue
        for key in MEDIAINFO_KEYS:
            if keep is not None and ((key == "Language" and "language" not in keep)
                                     or (key == "Rotation" and "rotation" not in keep)):
                continue
            if a.get(key) != b.get(key):
                found.append(problem(f"mediainfo:{i}:{key}",
                                     f"MediaInfo stream {i} {key}: {a.get(key)!r} != {b.get(key)!r}",
                                     source=a.get(key), output=b.get(key), source_track=a, output_track=b))
    return found


# Known divergences (docs/known-divergences-remux.md). Each predicate
# rechecks its entry's conditions from the files and the oracles' output.


# MediaInfo rounds each frame count on its own, so two counts can be a frame
# further apart than the durations; one more where the output's last sample
# is shorter than the one before it (a partial frame, counted as a sample in
# the source and cut by the output's duration).
FRAME_ROUNDING = 1.0


def frame_slack(path, index):
    try:
        stts = stts_entries(path, index) or []
    except OSError:
        stts = []
    partial = len(stts) >= 2 and stts[-1][0] == 1 and stts[-1][1] < stts[-2][1]
    return FRAME_ROUNDING + (1 if partial else 0)


def _stream(field):
    return int(field.split(":")[1])


def _fragmented(c):
    return "moof" in c["source_top_level"]


def fragment_frame_count(found, c):
    """KD-R-FRAG-FRAMES: ffprobe counts only the movie box samples of a
    fragmented source; the output's count is every copied packet."""
    d = found["data"]
    source = d.get("source")
    return (_fragmented(c) and d.get("output") == str(d.get("output_packets"))
            and (source is None or int(source) < d["output_packets"]))


def fragment_duration(found, c):
    """KD-R-FRAG-DURATION: in a fragmented source ffmpeg reports a packet
    duration other than the stored one; the output's reaches exactly to the
    next packet, whose times equal the source's, or for the last packet
    equals the duration the source's fragments store for it."""
    d = found["data"]
    a, b = _packet(d["source"]), _packet(d["output"])
    if not (_fragmented(c) and a and b):
        return False
    if not (a[:3] == b[:3] and a[4:] == b[4:] and a[3] != b[3]):
        return False
    following = _packet(d["next_output"] or "")
    if following:
        return b[3] == following[1] - b[1]
    # The last packet: the duration the source stores for its last sample.
    i = _stream(found["field"])
    return b[3] == last_fragment_duration(c["entry"]["source"], c["kept"][i])


def dropped_timecode(found, c):
    """KD-R-TIMECODE-TAG: ffprobe shows a timecode track's start as a tag of
    the video stream and of the file; the timecode track is dropped and
    reported."""
    d = found["data"]
    dropped = set(c["entry"]["dropped_streams"])
    return (d.get("output") is None and d.get("source") is not None
            and any(st.get("codec_tag_string") == "tmcd" and st["index"] in dropped
                    for st in c["source_streams"]))


def quicktime_channel_layout(found, c):
    """KD-R-CHAN: MediaInfo takes the channel layout of a QuickTime sound
    entry from its `chan` box, which the MP4 output leaves out (reported)."""
    i = _stream(found["field"])
    index = c["kept"][i]
    return ("chan" in entry_children(c["entry"]["source"], index)
            and "chan" not in entry_children(c["entry"]["output"], i))


def _edit_to_end(path, stream_index):
    """(media time, media timescale) of a track whose edit list ends with a
    media edit of duration 0, or None (ISO/IEC 14496-12 8.6.6, 8.4.2)."""
    elst = trak_box(path, stream_index, ["edts", "elst"])
    mdhd = trak_box(path, stream_index, ["mdia", "mdhd"])
    if not elst or not mdhd or len(elst) < 8:
        return None
    version, count = elst[0], struct.unpack(">I", elst[4:8])[0]
    width, layout = (20, ">Qq") if version == 1 else (12, ">Ii")
    if count == 0 or len(elst) < 8 + width * count:
        return None
    duration, media_time = struct.unpack(layout, elst[8 + width * (count - 1):8 + width * (count - 1) + width - 4])
    timescale_at = 20 if mdhd[0] == 1 else 12
    if duration != 0 or media_time <= 0 or len(mdhd) < timescale_at + 4:
        return None
    return media_time, struct.unpack(">I", mdhd[timescale_at:timescale_at + 4])[0]


def _movie_timescale(path):
    with open(path, "rb") as f:
        moov = next(((a, e) for k, a, e in _boxes(f, 0, f.seek(0, 2)) if k == "moov"), None)
        if moov is None:
            return None
        mvhd = next(((a, e) for k, a, e in _boxes(f, *moov) if k == "mvhd"), None)
        if mvhd is None:
            return None
        f.seek(mvhd[0])
        head = f.read(24)
        at = 20 if head[:1] == b"\x01" else 12
        return struct.unpack(">I", head[at:at + 4])[0] if len(head) >= at + 4 else None


def fragment_edit_to_end(found, c):
    """KD-R-FRAG-EDIT-END: ffprobe and MediaInfo read a fragmented source
    whose last edit has duration 0 (to the end of the media) as lasting the
    whole media; the output writes the edit's length, the media from the
    edit's media time on (rounded to the movie timescale), so its duration
    is shorter by that media time."""
    field = found["field"]
    if not _fragmented(c) or not field.endswith((":Duration", ":FrameCount", ":duration_ts")):
        return False
    i = _stream(field)
    edit = _edit_to_end(c["entry"]["source"], c["kept"][i])
    d = found["data"]
    if field.endswith(":duration_ts"):
        movie = _movie_timescale(c["entry"]["source"])
        if edit is None or not movie or d.get("source") is None or d.get("output") is None:
            return False
        tick = edit[1] / movie
        return abs(int(d["source"]) - int(d["output"]) - edit[0]) <= tick
    a, b = d.get("source_track") or {}, d.get("output_track") or {}
    try:
        gap = float(a["Duration"]) - float(b["Duration"])
    except (KeyError, TypeError, ValueError):
        return False
    if edit is None or edit[1] == 0 or abs(gap - edit[0] / edit[1]) > 0.0015:
        return False
    if found["field"].endswith(":Duration"):
        return True
    try:
        frames = int(a["FrameCount"]) - int(b["FrameCount"])
        return abs(frames - gap * float(b["FrameRate"])) <= frame_slack(c["entry"]["output"], i)
    except (KeyError, TypeError, ValueError):
        return False


def final_packet_capped(found, c):
    """KD-R-LAST-DURATION: the files store the same last sample duration and
    the same media header duration, shorter than the sample durations add
    up to; ffmpeg reports the stored duration in one file and the duration
    up to the media header's end in the other."""
    d = found["data"]
    if d.get("next_output") is not None or _fragmented(c):
        return False
    a, b = _packet(d["source"]), _packet(d["output"])
    if not (a and b and a[:3] == b[:3] and a[4:] == b[4:] and a[3] != b[3]):
        return False
    i = _stream(found["field"])
    src, out = c["entry"]["source"], c["entry"]["output"]
    s_stts, o_stts = stts_entries(src, c["kept"][i]), stts_entries(out, i)
    s_mdhd, o_mdhd = mdhd_duration(src, c["kept"][i]), mdhd_duration(out, i)
    if not s_stts or s_stts != o_stts or s_mdhd is None or s_mdhd != o_mdhd:
        return False
    total = sum(n * delta for n, delta in s_stts)
    stored = s_stts[-1][1]
    capped = s_mdhd - (total - stored)
    return s_mdhd < total and capped != stored and sorted((a[3], b[3])) == sorted((stored, capped))


def base_time(found, c):
    """KD-R-BASE-TIME: a fragmented source without an edit list whose first
    decode time d0 is above 0 gets an empty edit E followed by the media
    from time 0. ffmpeg presents the output's first packet at the empty
    edit's end, the source's at d0 plus its composition offset, so every
    time differs by E (in media ticks) - d0 - that offset; it counts the
    source's duration from 0 and the output's from the media; MediaInfo
    counts the output's empty edit."""
    if not _fragmented(c):
        return False
    field = found["field"]
    i = _stream(field)
    src, out = c["entry"]["source"], c["entry"]["output"]
    s_fields, o_fields = trak_fields(src, c["kept"][i]), trak_fields(out, i)
    if s_fields is None or o_fields is None:
        return False
    if not s_fields or not o_fields or s_fields["edits"] is not None:
        return False
    edits = o_fields["edits"] or []
    if len(edits) != 2 or edits[0][1] != -1 or edits[1][1] != 0:
        return False
    movie, mdhd = _movie_timescale(out), trak_box(out, i, ["mdia", "mdhd"])
    if not movie or not mdhd:
        return False
    timescale = struct.unpack(">I", mdhd[20:24] if mdhd[0] == 1 else mdhd[12:16])[0]
    first = c.get("first", {}).get(i)
    if not first:
        return False
    a0, b0 = _packet(first[0]), _packet(first[1])
    if not a0 or not b0:
        return False
    d0, offset = a0[1], a0[2] - a0[1]
    empty = edits[0][0] * timescale / movie
    shift = empty - d0 - offset
    # The source's first sample: its composition offset and duration.
    window = max(offset, 0) + a0[3]
    d = found["data"]
    if ":packet:" in field:
        a, b = _packet(d["source"]), _packet(d["output"])
        return bool(a and b and a[0] == b[0] and a[3:] == b[3:]
                    and b[1] - a[1] == b[2] - a[2] and abs(b[1] - a[1] - shift) < 1)
    if field.endswith(":start_pts"):
        # The output starts at the empty edit's end; the source within its
        # first sample.
        gap = int(d["source"]) - int(d["output"])
        return int(d["output"]) == empty and 0 <= gap <= window
    if field.endswith(":duration_ts"):
        # The output's is its media; the source's is counted from time 0,
        # to its first sample or past it.
        stts = stts_entries(out, i)
        if not stts or int(d["output"]) != sum(n * delta for n, delta in stts):
            return False
        gap = int(d["source"]) - int(d["output"]) - d0
        return 0 <= gap <= window
    if field.endswith(":Duration"):
        try:
            gap = float(d["output"]) - float(d["source"])
        except (TypeError, ValueError):
            return False
        return abs(gap - edits[0][0] / movie) <= 0.0015
    if field.endswith(":FrameCount"):
        # MediaInfo counts the empty edit's length in frames.
        try:
            frames = int(d["output"]) - int(d["source"])
            rate = float((d.get("output_track") or {})["FrameRate"])
        except (KeyError, TypeError, ValueError):
            return False
        return abs(frames - edits[0][0] / movie * rate) <= frame_slack(out, i)
    return False


PROBE_WINDOW_KEYS = ("profile", "codec_name", "sample_fmt", "channel_layout", "bits_per_sample")


def _probe_all(binary, path, index):
    """One stream as ffprobe reads it with a probe window over the whole
    file (2 GB and 2,000 s, the most it accepts)."""
    data = json.loads(run([binary, "-v", "error", "-of", "json", "-probesize", "2000M", "-analyzeduration",
                           "2000M", "-select_streams", str(index), "-show_streams", path]))
    streams = data.get("streams") or []
    return streams[0] if streams else {}


def probe_window(found, c):
    """KD-R-PROBE-WINDOW: ffprobe identifies some codec parameters from the
    packets it reads first (5 MB or 5 s by default); when the source stores
    a stream's first packets beyond that window it misses them there. Read
    with a window over the whole file, both files give the same value, and
    the output gives it by default."""
    field = found["field"]
    i = _stream(field)
    key = field.split(":")[2]
    if key == "framemd5":
        heads = (found["data"].get("source") or [], found["data"].get("output") or [])
        differing = set(heads[0]) ^ set(heads[1])
        if not differing or any(not line.startswith("#codec_id") for line in differing):
            return False
        key = "codec_name"
    elif key not in PROBE_WINDOW_KEYS:
        return False
    a = _probe_all(c["ffprobe"], c["entry"]["source"], c["kept"][i])
    b = _probe_all(c["ffprobe"], c["entry"]["output"], i)
    if a.get(key) is None or a.get(key) != b.get(key):
        return False
    # Only the source may be the file whose packets lie beyond the window:
    # the output must give the whole-file value by default.
    if field.endswith(":framemd5:header"):
        out_lines = found["data"].get("output") or []
        return any(line.startswith("#codec_id") and line.rstrip().endswith(" " + str(b.get(key)))
                   for line in out_lines)
    return found["data"].get("output") == b.get(key)


def vfr_rate(found, c):
    """KD-R-VFR-RATE: for a stream of more than one decode duration,
    ffprobe's `r_frame_rate` is a guess from the frames it analyzes before
    it stops probing, which depends on how the streams are interleaved. The
    files store the same decode durations (`stts`), and ffprobe's average
    frame rate is the same."""
    if not found["field"].endswith(":r_frame_rate"):
        return False
    i = _stream(found["field"])
    a = stts_entries(c["entry"]["source"], c["kept"][i])
    b = stts_entries(c["entry"]["output"], i)
    if not a or a != b or len({delta for _, delta in a}) < 2:
        return False
    s = next((st for st in c["source_streams"] if st["index"] == c["kept"][i]), {})
    return s.get("avg_frame_rate") == found["data"].get("output_avg")


def fragment_stated_edit(found, c):
    """KD-R-FRAG-STATED-EDIT: MediaInfo reports a fragmented source's
    duration from its samples, ignoring an edit list that states its
    length; the output, no longer fragmented, carries that edit list (and a
    track duration that sums it), which MediaInfo reports."""
    field = found["field"]
    if not _fragmented(c) or not field.endswith((":Duration", ":FrameCount")):
        return False
    i = _stream(field)
    src, out = c["entry"]["source"], c["entry"]["output"]
    s_fields, o_fields = trak_fields(src, c["kept"][i]), trak_fields(out, i)
    if not s_fields or not o_fields or not s_fields["edits"] or not o_fields["edits"]:
        return False
    if any(e[0] == 0 for e in s_fields["edits"]):
        return False
    stts = stts_entries(out, i)
    mdhd = trak_box(out, i, ["mdia", "mdhd"])
    movie = _movie_timescale(src)
    if not stts or not mdhd or not movie:
        return False
    timescale = struct.unpack(">I", mdhd[20:24] if mdhd[0] == 1 else mdhd[12:16])[0]
    samples = sum(n * delta for n, delta in stts) / timescale
    # What the source's own edits state, in its own movie timescale.
    edit = sum(e[0] for e in s_fields["edits"]) / movie
    d = found["data"]
    a, b = d.get("source_track") or {}, d.get("output_track") or {}
    try:
        source, output = float(a["Duration"]), float(b["Duration"])
    except (KeyError, TypeError, ValueError):
        return False
    if abs(source - samples) > 0.0005 or abs(output - edit) > 0.0005:
        return False
    if field.endswith(":Duration"):
        return True
    try:
        frames = int(b["FrameCount"]) - int(a["FrameCount"])
        return abs(frames - (output - source) * float(b["FrameRate"])) <= frame_slack(out, i)
    except (KeyError, TypeError, ValueError):
        return False


PREDICATES = {
    "fragment_stated_edit": fragment_stated_edit,
    "probe_window": probe_window,
    "vfr_rate": vfr_rate,
    "final_packet_capped": final_packet_capped,
    "base_time": base_time,
    "fragment_edit_to_end": fragment_edit_to_end,
    "fragment_frame_count": fragment_frame_count,
    "fragment_duration": fragment_duration,
    "dropped_timecode": dropped_timecode,
    "quicktime_channel_layout": quicktime_channel_layout,
}


def known_divergences():
    if not KD_FILE.exists():
        return []
    entries = json.loads(KD_FILE.read_text(encoding="utf-8"))["entries"]
    return [(e["id"], e["field_pattern"], PREDICATES[e["predicate"]]) for e in entries]


def apply_known_divergences(found, context):
    problems, covered = [], []
    entries = known_divergences()
    for item in found:
        # The box-level checks compare the files' own boxes; no known
        # divergence covers them.
        kd = None if item["field"].startswith("box:") else next(
            (kd for kd, pattern, predicate in entries
             if fnmatch.fnmatch(item["field"], pattern) and predicate(item, context)), None)
        if kd:
            covered.append({"kd": kd, "field": item["field"], "message": item["message"]})
        else:
            problems.append(item)
    return problems, covered


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mediainfo")
    parser.add_argument("--report")
    args = parser.parse_args()

    with open(args.manifest, encoding="utf-8") as f:
        manifest = json.load(f)
    results = []
    for entry in manifest:
        try:
            problems, covered, counts = check(entry, args.ffprobe, args.ffmpeg, args.mediainfo)
        except (RuntimeError, ValueError, KeyError, OSError) as error:
            problems = [problem("harness", f"harness: {error}")]
            covered, counts = [], {"agree": 0, "disagree": 0}
        results.append({**entry, "problems": [p["message"] for p in problems], "covered": covered,
                        "packets": counts})
        status = "FAIL" if problems else "kd  " if covered else "ok  "
        print(f"{status} {entry['output'].rsplit('/', 1)[-1]}")
        for item in problems[:20]:
            print(f"     {item['message']}")
        if len(problems) > 20:
            print(f"     ... {len(problems) - 20} more")
        kds = sorted({c["kd"] for c in covered})
        for kd in kds:
            print(f"     ({kd}) {sum(1 for c in covered if c['kd'] == kd)} differences")
    passed = sum(1 for r in results if not r["problems"])
    direct = sum(1 for r in results if not r["problems"] and not r["covered"])
    agree = sum(r["packets"]["agree"] for r in results)
    disagree = sum(r["packets"]["disagree"] for r in results)
    print(f"{passed}/{len(results)} outputs pass every oracle ({direct} directly, {passed - direct} through "
          f"known divergences); packets {agree}/{agree + disagree} agree")
    if args.report:
        with open(args.report, "w", encoding="utf-8") as f:
            json.dump({"passed": passed, "direct": direct, "total": len(results),
                       "packets": {"agree": agree, "disagree": disagree}, "results": results}, f, indent=2)
    return 0 if results and passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
