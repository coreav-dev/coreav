#!/usr/bin/env python3
"""Mutation audit of the writers' known divergences: can an entry of
docs/known-divergences-{clip,split,remux,extract}.md cover a real CoreAV
bug? As tools/oracle/kd_audit.py does for the probe entries, each mutation
injects one bug-shaped change, here into a writer's output file, and the
writer's own oracle (scripts/<writer>_check.py) judges the mutated output
again. Every such change must stay visible: as a new problem, never as a
difference an entry covers.

The mutations are in place, so no box moves, on the first video or audio
track of the output (ISO/IEC 14496-12):
- sample timing: the first `stts` entry's delta +1 or -1, the first `ctts`
  offset +512;
- sample tables: the first sample size +1 (`stsz`), the first chunk offset
  +1 (`stco`/`co64`), a sync sample moved one sample later (`stss`);
- the edit list: the first media edit's media time +1, the first entry's
  duration +1 or -1;
- durations and timescales: `mdhd` duration +1, `mdhd` timescale +1,
  `tkhd` duration +1, `mvhd` duration +7;
- the sample entry: video width +16, audio channel count +1, audio sample
  rate halved, the AAC object type changed (2 and 5 swapped);
- the samples: one byte of the first sample flipped.

Verdicts, from each check's problems and covered differences before and
after the mutation:
- caught: a new problem;
- COVERED: no new problem, yet a covered difference the unmutated output
  did not have (an entry absorbed the bug): a loophole;
- ABSORBED: no new problem and no new covered difference, but with the
  known divergences switched off the mutation adds a problem: an entry that
  already covered the output hid it (an oracle that names only the entries
  it used, not their differences, shows a loophole only this way);
- agrees: only problems or covered differences removed;
- no effect: the same result, with or without the known divergences: the
  oracle does not see the change (reported, not an entry's fault);
- n/a: the mutation does not apply to the output.

usage: writer_kd_audit.py --writer clip|split|remux|extract --manifest M
                          [--mediainfo PATH] [--report OUT] [--workers N]
                          [--limit N] [--mutation NAME ...]
"""
import argparse
import json
import shutil
import struct
import sys
import tempfile
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))


def boxes(data, start, end):
    """(type, header start, payload start, end) of each box in [start, end)."""
    at = start
    while at + 8 <= end:
        size, kind = struct.unpack(">I4s", data[at:at + 8])
        header = 8
        if size == 1:
            size = struct.unpack(">Q", data[at + 8:at + 16])[0]
            header = 16
        elif size == 0:
            size = end - at
        if size < header or at + size > end:
            return
        yield kind, at, at + header, at + size
        at += size


def child(data, parent, kind):
    if parent is None:
        return None
    for box in boxes(data, parent[2], parent[3]):
        if box[0] == kind:
            return box
    return None


def tracks(data):
    """({'video': trak, 'audio': trak} of the first track of each handler,
    the movie box)."""
    found = {}
    moov = next((b for b in boxes(data, 0, len(data)) if b[0] == b"moov"), None)
    if moov is None:
        return found, None
    for trak in boxes(data, moov[2], moov[3]):
        if trak[0] != b"trak":
            continue
        hdlr = child(data, child(data, trak, b"mdia"), b"hdlr")
        if not hdlr:
            continue
        kind = {b"vide": "video", b"soun": "audio"}.get(bytes(data[hdlr[2] + 8:hdlr[2] + 12]))
        if kind and kind not in found:
            found[kind] = trak
    return found, moov


def stbl(data, trak):
    return child(data, child(data, child(data, trak, b"mdia"), b"minf"), b"stbl")


def u32(data, at):
    return struct.unpack(">I", data[at:at + 4])[0]


def put32(data, at, value):
    data[at:at + 4] = struct.pack(">I", value & 0xFFFFFFFF)


def bump(data, at, width, delta):
    fmt = ">Q" if width == 8 else ">I"
    value = struct.unpack(fmt, data[at:at + width])[0]
    data[at:at + width] = struct.pack(fmt, (value + delta) & ((1 << (8 * width)) - 1))


def full_field(data, box, offset_v0, offset_v1):
    """Position and width of a time field of a versioned full box."""
    version = data[box[2]]
    return box[2] + (offset_v1 if version else offset_v0), 8 if version else 4


# Each mutation: function(data, trak, moov) -> whether it applied.
def stts_delta(delta):
    def mutate(data, trak, _moov):
        box = child(data, stbl(data, trak), b"stts")
        if not box or u32(data, box[2] + 4) == 0 or u32(data, box[2] + 12) + delta <= 0:
            return False
        bump(data, box[2] + 12, 4, delta)
        return True
    return mutate


def ctts_offset(data, trak, _moov):
    box = child(data, stbl(data, trak), b"ctts")
    if not box or u32(data, box[2] + 4) == 0:
        return False
    bump(data, box[2] + 12, 4, 512)
    return True


def stsz_size(data, trak, _moov):
    box = child(data, stbl(data, trak), b"stsz")
    if not box or u32(data, box[2] + 4) != 0 or u32(data, box[2] + 8) == 0:
        return False
    bump(data, box[2] + 12, 4, 1)
    return True


def first_chunk(data, trak):
    """(position, width) of the first chunk offset."""
    table = stbl(data, trak)
    for kind, width in ((b"stco", 4), (b"co64", 8)):
        box = child(data, table, kind)
        if box and u32(data, box[2] + 4):
            return box[2] + 8, width
    return None


def chunk_offset(data, trak, _moov):
    found = first_chunk(data, trak)
    if not found:
        return False
    bump(data, found[0], found[1], 1)
    return True


def stss_move(data, trak, _moov):
    table = stbl(data, trak)
    box, sizes = child(data, table, b"stss"), child(data, table, b"stsz")
    if not box or not sizes:
        return False
    count, samples = u32(data, box[2] + 4), u32(data, sizes[2] + 8)
    entries = [u32(data, box[2] + 8 + 4 * i) for i in range(count)]
    for i, number in enumerate(entries):
        if number + 1 <= samples and number + 1 not in entries:
            put32(data, box[2] + 8 + 4 * i, number + 1)
            return True
    return False


def elst_entries(data, trak):
    box = child(data, child(data, trak, b"edts"), b"elst")
    if not box:
        return []
    version, count = data[box[2]], u32(data, box[2] + 4)
    width = 20 if version else 12
    return [(box[2] + 8 + i * width, version) for i in range(count)]


def elst_media_time(data, trak, _moov):
    for at, version in elst_entries(data, trak):
        field, fmt = (at + 8, ">q") if version else (at + 4, ">i")
        size = struct.calcsize(fmt)
        value = struct.unpack(fmt, data[field:field + size])[0]
        if value != -1:
            data[field:field + size] = struct.pack(fmt, value + 1)
            return True
    return False


def elst_duration(delta):
    def mutate(data, trak, _moov):
        entries = elst_entries(data, trak)
        if not entries:
            return False
        at, version = entries[0]
        width = 8 if version else 4
        value = struct.unpack(">Q" if version else ">I", data[at:at + width])[0]
        if value + delta <= 0:
            return False
        bump(data, at, width, delta)
        return True
    return mutate


def mdhd_duration(data, trak, _moov):
    mdhd = child(data, child(data, trak, b"mdia"), b"mdhd")
    bump(data, *full_field(data, mdhd, 16, 24), 1)
    return True


def mdhd_timescale(data, trak, _moov):
    mdhd = child(data, child(data, trak, b"mdia"), b"mdhd")
    at, _ = full_field(data, mdhd, 12, 20)
    bump(data, at, 4, 1)
    return True


def tkhd_duration(data, trak, _moov):
    bump(data, *full_field(data, child(data, trak, b"tkhd"), 20, 28), 1)
    return True


def mvhd_duration(data, _trak, moov):
    bump(data, *full_field(data, child(data, moov, b"mvhd"), 16, 24), 7)
    return True


def sample_entry(data, trak):
    stsd = child(data, stbl(data, trak), b"stsd")
    if not stsd or u32(data, stsd[2] + 4) == 0:
        return None
    return next(boxes(data, stsd[2] + 8, stsd[3]), None)


def u16_add(data, at, delta):
    data[at:at + 2] = struct.pack(">H", (struct.unpack(">H", data[at:at + 2])[0] + delta) & 0xFFFF)


def video_width(data, trak, _moov):
    entry = sample_entry(data, trak)
    if not entry:
        return False
    u16_add(data, entry[2] + 24, 16)
    return True


def audio_channels(data, trak, _moov):
    entry = sample_entry(data, trak)
    if not entry:
        return False
    u16_add(data, entry[2] + 16, 1)
    return True


def audio_rate(data, trak, _moov):
    entry = sample_entry(data, trak)
    if not entry:
        return False
    rate = u32(data, entry[2] + 24) >> 16
    if rate < 2:
        return False
    # Halved, not doubled: twice 44,100 or 48,000 does not fit the field's
    # 16-bit integer part.
    put32(data, entry[2] + 24, (rate // 2) << 16)
    return True


def aac_object_type(data, trak, _moov):
    entry = sample_entry(data, trak)
    if not entry or entry[0] != b"mp4a":
        return False
    esds = next((b for b in boxes(data, entry[2] + 28, entry[3]) if b[0] == b"esds"), None)
    if not esds:
        return False
    # The DecoderSpecificInfo descriptor (tag 5, ISO/IEC 14496-1 7.2.6.7):
    # its first byte holds the five bits of the audio object type
    # (ISO/IEC 14496-3 1.6.2.1).
    body = bytes(data[esds[2] + 4:esds[3]])
    at = body.find(b"\x05")
    while at != -1:
        pos, length = at + 1, 0
        for _ in range(4):
            if pos >= len(body):
                break
            byte = body[pos]
            length = (length << 7) | (byte & 0x7F)
            pos += 1
            if not byte & 0x80:
                break
        if 2 <= length <= 64 and pos < len(body) and body[pos] >> 3 in (2, 5):
            new = 5 if body[pos] >> 3 == 2 else 2
            data[esds[2] + 4 + pos] = (new << 3) | (data[esds[2] + 4 + pos] & 0x07)
            return True
        at = body.find(b"\x05", at + 1)
    return False


def sample_byte(data, trak, _moov):
    found = first_chunk(data, trak)
    sizes = child(data, stbl(data, trak), b"stsz")
    if not found or not sizes:
        return False
    offset = struct.unpack(">Q" if found[1] == 8 else ">I", data[found[0]:found[0] + found[1]])[0]
    size = u32(data, sizes[2] + 4) or (u32(data, sizes[2] + 12) if u32(data, sizes[2] + 8) else 0)
    at = offset + min(size - 1, 16)
    if size < 2 or at >= len(data):
        return False
    data[at] ^= 0x5A
    return True


MUTATIONS = [
    ("stts delta +1", stts_delta(1)),
    ("stts delta -1", stts_delta(-1)),
    ("ctts offset +512", ctts_offset),
    ("stsz first size +1", stsz_size),
    ("chunk offset +1", chunk_offset),
    ("stss sync moved", stss_move),
    ("elst media time +1", elst_media_time),
    ("elst duration +1", elst_duration(1)),
    ("elst duration -1", elst_duration(-1)),
    ("mdhd duration +1", mdhd_duration),
    ("mdhd timescale +1", mdhd_timescale),
    ("tkhd duration +1", tkhd_duration),
    ("mvhd duration +7", mvhd_duration),
    ("sample byte flipped", sample_byte),
]
BY_KIND = {
    "video": [("video width +16", video_width)],
    "audio": [("audio channels +1", audio_channels), ("audio rate halved", audio_rate),
              ("aac object type swapped", aac_object_type)],
}


def mutate_file(src, dst, kind, mutation):
    data = bytearray(Path(src).read_bytes())
    found, moov = tracks(data)
    try:
        if kind not in found or not mutation(data, found[kind], moov):
            return False
    except (TypeError, IndexError, struct.error):
        return False  # a box the mutation needs is missing or short
    Path(dst).write_bytes(bytes(data))
    return True


class NoKnownDivergences:
    """Switches off a writer oracle's known divergences."""

    def __init__(self, writer):
        names = {"clip": ["clip_check"], "split": ["split_check", "clip_check"], "remux": ["remux_check"],
                 "extract": ["extract_check"]}[writer]
        self.modules = [__import__(n) for n in names]

    def __enter__(self):
        self.saved = [m.known_divergences for m in self.modules]
        for m in self.modules:
            m.known_divergences = lambda: []

    def __exit__(self, *_):
        for m, saved in zip(self.modules, self.saved):
            m.known_divergences = saved


def run_check(writer, entry, tools, kds=True):
    """(problems, covered) of one entry, as lists of strings and (kd,
    message) pairs; without `kds`, with the known divergences off."""
    if not kds:
        with NoKnownDivergences(writer):
            return run_check(writer, entry, tools)
    if writer == "clip":
        import clip_check as m
        problems, _info, covered, _counts = m.check(entry, *tools)
        return list(problems), [(c["kd"], c["problem"]) for c in covered]
    if writer == "split":
        import split_check as m
        problems, _info, covered = m.check(entry, *tools)[:3]
        return list(problems), [(c["kd"], c["problem"]) for c in covered]
    if writer == "remux":
        import remux_check as m
        problems, covered, _counts = m.check(entry, *tools)
        return [p["message"] for p in problems], [(c["kd"], c["message"]) for c in covered]
    if writer == "extract":
        import extract_check as m
        problems, _info, covered = m.check(entry, *tools)
        return list(problems), [(c["kd"], c["problem"]) for c in covered]
    raise SystemExit(f"unknown writer {writer}")


def outputs(writer, entry):
    """(label, path, entry with that output replaced) of the outputs an
    entry's mutations go into: a split's first and last segments, any other
    writer's output."""
    if writer == "split":
        segments = entry["segments"]
        def put(n):
            def replace(e, path):
                e = json.loads(json.dumps(e))
                e["segments"][n]["output"] = path
                return e
            return replace
        return [(f"segment {n}", segments[n]["output"], put(n)) for n in sorted({0, len(segments) - 1})]
    return [("output", entry["output"], lambda e, path: dict(e, output=path))]


def verdict(base, after):
    (problems, covered), (problems2, covered2) = base, after
    if Counter(problems2) - Counter(problems):
        return "caught"
    if Counter(covered2) - Counter(covered):
        return "COVERED"
    if Counter(problems2) != Counter(problems) or Counter(covered2) != Counter(covered):
        return "agrees"
    return "no effect"


def audit_entry(args):
    writer, entry, tools, work, only = args
    name = entry.get("output") or entry["segments"][0]["output"]
    try:
        base = run_check(writer, entry, tools)
    except Exception as error:  # noqa: BLE001
        return [{"output": name, "verdict": "error", "error": f"baseline: {type(error).__name__}: {error}"[:300]}]
    base_off = None
    results = []
    tmp = Path(tempfile.mkdtemp(dir=work))
    try:
        for label, path, put in outputs(writer, entry):
            for kind in ("video", "audio"):
                for mutation_name, mutation in MUTATIONS + BY_KIND[kind]:
                    if only and mutation_name not in only:
                        continue
                    record = {"output": path, "where": label, "kind": kind, "mutation": mutation_name,
                              "base_kds": sorted({kd for kd, _ in base[1]})}
                    dst = tmp / f"m{len(results)}{Path(path).suffix}"
                    if not mutate_file(path, dst, kind, mutation):
                        results.append(dict(record, verdict="n/a"))
                        continue
                    try:
                        after = run_check(writer, put(entry, str(dst)), tools)
                    except Exception as error:  # noqa: BLE001
                        # The oracle did not accept the mutated output.
                        after = (base[0] + [f"harness: {type(error).__name__}"], base[1])
                    record["verdict"] = verdict(base, after)
                    if record["verdict"] == "COVERED":
                        record["new_covered"] = sorted((Counter(after[1]) - Counter(base[1])).keys())
                    elif record["verdict"] in ("no effect", "agrees"):
                        # Would the oracle have seen it without the entries?
                        try:
                            if base_off is None:
                                base_off = run_check(writer, entry, tools, kds=False)
                            off = run_check(writer, put(entry, str(dst)), tools, kds=False)
                            if Counter(off[0]) - Counter(base_off[0]):
                                record["verdict"] = "ABSORBED"
                                record["hidden"] = sorted((Counter(off[0]) - Counter(base_off[0])).keys())[:3]
                        except Exception:  # noqa: BLE001
                            pass
                    results.append(record)
                    dst.unlink(missing_ok=True)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--writer", required=True, choices=("clip", "split", "remux", "extract"))
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mediainfo")
    parser.add_argument("--report")
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--mutation", action="append", default=[], help="only these mutations, by name")
    args = parser.parse_args()
    entries = json.loads(Path(args.manifest).read_text())
    if args.limit:
        entries = entries[:args.limit]
    tools = (args.ffprobe, args.ffmpeg, args.mediainfo)
    work = tempfile.mkdtemp(prefix="writer-kd-audit-")
    results = []
    try:
        with ProcessPoolExecutor(args.workers) as pool:
            for batch in pool.map(audit_entry, [(args.writer, e, tools, work, set(args.mutation)) for e in entries]):
                results.extend(batch)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    counts = Counter(r["verdict"] for r in results)
    print(f"writer_kd_audit {args.writer}: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    for r in results:
        if r["verdict"] == "COVERED":
            print(f"COVERED {Path(r['output']).name} {r['where']} {r['kind']}: {r['mutation']}")
            for kd, message in r["new_covered"][:3]:
                print(f"    ({kd}) {message[:160]}")
        elif r["verdict"] == "ABSORBED":
            print(f"ABSORBED {Path(r['output']).name} {r['where']} {r['kind']}: {r['mutation']}")
            for message in r["hidden"]:
                print(f"    hidden: {message[:160]}")
        elif r["verdict"] == "error":
            print(f"ERROR {Path(r['output']).name}: {r['error']}")
    if args.report:
        Path(args.report).write_text(json.dumps({"writer": args.writer, "counts": counts, "results": results},
                                                indent=1) + "\n")
    if not counts.get("caught"):
        print(f"writer_kd_audit {args.writer}: no change was caught", file=sys.stderr)
        return 1
    return 1 if counts.get("COVERED") or counts.get("ABSORBED") or counts.get("error") else 0


if __name__ == "__main__":
    sys.exit(main())
