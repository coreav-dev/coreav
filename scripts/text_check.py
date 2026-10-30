#!/usr/bin/env python3
"""Black-box oracles for subtitle and chapter extraction (docs/design/text.md).

For every MP4, MOV, M4A, M4B, M4V and 3GP file of --media (the `generated`
lines of its MANIFEST when there is one), runs the coreav binary and checks
what it writes against the pinned ffprobe and ffmpeg, and MediaInfo when
--mediainfo is given:

Subtitles, for every stream ffprobe reads as `mov_text`:
- `coreav extract-subtitles --stream N` succeeds and its WebVTT passes a
  strict check of the W3C WebVTT syntax CoreAV promises (header, timestamps,
  end after start, starts in order, escaped text, no empty line in a cue);
- ffprobe reads the output as WebVTT with one subtitle stream whose packets
  are the cues: start, duration and payload hash;
- MediaInfo reads it as WebVTT with as many events as cues, starting at the
  first cue and ending at the last;
- the cues equal those of `ffmpeg -c:s webvtt` on the source stream: the same
  start and end, to the millisecond, and the same text.

Chapters, for every file where ffprobe or CoreAV finds chapters:
- `coreav chapters` succeeds, and its chapters equal ffprobe's
  `-show_chapters`: the same count, start and end times (exactly, as
  fractions of a second) and titles.

A difference counts as agreement only when entries of
docs/known-divergences-text.md cover it. Each cue entry is a
narrow rewrite of one side's cue list (for example "drop ffmpeg's cues
without text"), applied only when its conditions hold, rechecked here from
the file's own box headers and the oracles' output; the lists must then be
equal. Chapter entries are predicates on the same kind of evidence. The
entries used are reported apart from direct agreement.

Files that `coreav probe` rejects are listed apart and are not in the
denominator (probe has its own oracle); with --kd-overlay (from
scripts/oracle_kd.py) a rejected file fails unless KD-STRICT covers it.

The binaries are black boxes; no implementation source
is read. Exits 1 if any check fails or nothing was compared.
"""

import argparse
import fractions
import hashlib
import html
import json
import pathlib
import re
import struct
import subprocess
import sys
import tempfile
import zlib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import clip_check as cc  # noqa: E402
import track_boxes  # noqa: E402

IN_SCOPE = (".mp4", ".mov", ".m4a", ".m4b", ".m4v", ".3gp", ".3g2")
TIMEOUT = 600
KD_FILE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "known-divergences-text.json"

TIMING = re.compile(r"^(\d{2,}):([0-5]\d):([0-5]\d)\.(\d{3}) --> (\d{2,}):([0-5]\d):([0-5]\d)\.(\d{3})$")
LOOSE_TIMING = re.compile(
    r"^(?:(\d+):)?(\d{2}):(\d{2})\.(\d{3})[ \t]+-->[ \t]+(?:(\d+):)?(\d{2}):(\d{2})\.(\d{3})(?:[ \t].*)?$")
ENTITY = re.compile(r"&(amp|lt|gt);")
# The tags ffmpeg's WebVTT output uses for styling (black-box observation):
# bold, italic, underline and font colour.
STYLE_TAG = re.compile(r"</?(?:b|i|u)>|<font\b[^>]*>|</font>")
STYLE_BOXES = ("styl", "hlit", "hclr", "krok", "blnk")


def run(args, check=True):
    result = subprocess.run(args, capture_output=True, check=False, timeout=TIMEOUT)
    if check and result.returncode != 0:
        raise RuntimeError(f"{args[0]} failed ({result.returncode}): {result.stderr.decode(errors='replace')[:400]}")
    return result


def ffprobe_json(ffprobe, *args):
    return json.loads(run([ffprobe, "-v", "error", "-of", "json", *args]).stdout)


# ------------------------------------------------------------------ WebVTT

def ms(h, m, s, f):
    return ((int(h or 0) * 60 + int(m)) * 60 + int(s)) * 1000 + int(f)


def strict_webvtt(data):
    """Parses CoreAV's WebVTT, returning (cues, problems). Each cue is
    (start ms, end ms, payload text as written)."""
    problems = []
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return [], ["not UTF-8"]
    if not text.startswith("WEBVTT\n\n"):
        return [], ["no WEBVTT header followed by a blank line"]
    body = text[len("WEBVTT\n\n"):]
    if "\r" in body:
        problems.append("CR in the body")
    if body and not body.endswith("\n\n"):
        problems.append("the last cue is not followed by a blank line")
    cues, previous = [], -1
    for block in [b for b in body.split("\n\n") if b]:
        lines = block.split("\n")
        timing = TIMING.match(lines[0])
        if not timing:
            problems.append(f"bad timing line {lines[0]!r}")
            continue
        start, end = ms(*timing.groups()[:4]), ms(*timing.groups()[4:])
        if end <= start:
            problems.append(f"cue at {start} ms does not end after it starts")
        if start < previous:
            problems.append(f"cue at {start} ms starts before the previous one")
        previous = start
        payload = lines[1:]
        if not payload or any(not line.strip() for line in payload):
            problems.append(f"cue at {start} ms has an empty line or no text")
        joined = "\n".join(payload)
        if "-->" in joined or "<" in joined or ENTITY.sub("", joined).count("&"):
            problems.append(f"cue at {start} ms has unescaped text")
        cues.append((start, end, joined))
    return cues, problems


def loose_webvtt(text):
    """Parses ffmpeg's WebVTT output: (start ms, end ms, text) per cue."""
    cues = []
    for block in re.split(r"\n{2,}", text.replace("\r\n", "\n").replace("\r", "\n")):
        lines = block.split("\n")
        for i, line in enumerate(lines):
            timing = LOOSE_TIMING.match(line)
            if timing:
                g = timing.groups()
                cues.append((ms(*g[:4]), ms(*g[4:]), "\n".join(lines[i + 1:]).rstrip("\n")))
                break
    return cues


# ------------------------------------------------------------- box headers
# Written from ISO/IEC 14496-12 (boxes, tkhd, tref, elst, mdhd, hdlr, stsd,
# stts), independently of the code under test.

def children(data):
    at = 0
    while at + 8 <= len(data):
        size, kind = struct.unpack(">I4s", data[at:at + 8])
        header = 8
        if size == 1 and at + 16 <= len(data):
            size, header = struct.unpack(">Q", data[at + 8:at + 16])[0], 16
        elif size == 0:
            size = len(data) - at
        if size < header or at + size > len(data):
            return
        yield kind.decode("latin1"), data[at + header:at + size]
        at += size


def top_level(path, wanted, cap=64 << 20):
    with open(path, "rb") as f:
        total = f.seek(0, 2)
        at = 0
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
                return None
            if kind == wanted:
                if size > cap:
                    return None
                f.seek(at + header)
                return f.read(size - header)
            at += size
    return None


def movie_headers(path):
    """{'udta': [box types], 'traks': [{id, enabled, chap, elst, timescale,
    duration, handler, entry, stts}]} in file order, or None. A compressed
    movie box (QuickTime `cmov`) is read inflated."""
    moov = top_level(path, b"moov")
    if moov is None:
        return None
    if b"cmov" in moov[:64]:
        try:
            moov = bytes(track_boxes.movie_payload(moov))
        except (track_boxes.Unreadable, zlib.error):
            return None
    out = {"udta": [], "traks": [], "timescale": None}
    try:
        for kind, body in children(moov):
            if kind == "mvhd":
                out["timescale"] = struct.unpack(">I", body[20:24] if body[0] else body[12:16])[0]
            if kind == "udta":
                out["udta"] = [k for k, _ in children(body)]
            if kind == "mvex":
                for k2, b2 in children(body):
                    if k2 == "trex" and len(b2) >= 16:
                        track, _, duration = struct.unpack(">III", b2[4:16])
                        out.setdefault("trex", {})[track] = duration
            if kind != "trak":
                continue
            trak = {"chap": [], "elst": None, "stts": [], "entry": None, "stsc": [], "stsz": (0, []), "chunks": []}
            for k2, b2 in children(body):
                if k2 == "tkhd":
                    trak["enabled"] = bool(b2[3] & 1)
                    trak["id"] = struct.unpack(">I", b2[20:24] if b2[0] else b2[12:16])[0]
                if k2 == "tref":
                    for k3, b3 in children(b2):
                        if k3 == "chap":
                            trak["chap"] += [i for (i,) in struct.iter_unpack(">I", b3[:len(b3) // 4 * 4])]
                if k2 == "edts":
                    for k3, b3 in children(b2):
                        if k3 == "elst":
                            version, count = b3[0], struct.unpack(">I", b3[4:8])[0]
                            width, layout = (20, ">Qqi") if version else (12, ">Iii")
                            # Every entry: the several-edit checks apply the
                            # whole list, and a truncated one would give a
                            # wrong expectation rather than none.
                            if 8 + count * width > len(b3):
                                return None
                            trak["elst"] = [struct.unpack(layout, b3[8 + i * width:8 + (i + 1) * width])
                                            for i in range(count)]
                if k2 == "mdia":
                    for k3, b3 in children(b2):
                        if k3 == "mdhd":
                            trak["timescale"], trak["duration"] = (struct.unpack(">IQ", b3[20:32]) if b3[0]
                                                                   else struct.unpack(">II", b3[12:20]))
                        if k3 == "hdlr":
                            trak["handler"] = b3[8:12].decode("latin1")
                        if k3 == "minf":
                            for k4, b4 in children(b3):
                                if k4 != "stbl":
                                    continue
                                for k5, b5 in children(b4):
                                    if k5 == "stsd" and len(b5) >= 16:
                                        trak["entry"] = b5[12:16].decode("latin1")
                                    if k5 == "stts":
                                        count = min(struct.unpack(">I", b5[4:8])[0], (len(b5) - 8) // 8)
                                        trak["stts"] = list(struct.iter_unpack(">II", b5[8:8 + count * 8]))
                                    if k5 == "stsc":
                                        count = min(struct.unpack(">I", b5[4:8])[0], (len(b5) - 8) // 12)
                                        trak["stsc"] = list(struct.iter_unpack(">III", b5[8:8 + count * 12]))
                                    if k5 == "stsz":
                                        size, count = struct.unpack(">II", b5[4:12])
                                        sizes = [] if size else [x for (x,) in struct.iter_unpack(
                                            ">I", b5[12:12 + min(count, (len(b5) - 12) // 4) * 4])]
                                        trak["stsz"] = (size, sizes if not size else [size] * count)
                                    if k5 in ("stco", "co64"):
                                        width = 4 if k5 == "stco" else 8
                                        count = min(struct.unpack(">I", b5[4:8])[0], (len(b5) - 8) // width)
                                        trak["chunks"] = [x for (x,) in struct.iter_unpack(
                                            ">I" if width == 4 else ">Q", b5[8:8 + count * width])]
            out["traks"].append(trak)
    except (struct.error, IndexError):
        return None
    return out


# --------------------------------------------------- known divergences
# Each cue rewrite takes the evidence dict `e` and the two cue lists, and
# returns the rewritten (ours, theirs), or None when its conditions do not
# hold.

def plain(text):
    return html.unescape(text)


def kd_escape(e, ours, theirs):
    """KD-T-ESCAPE: undo CoreAV's escapes of &, < and >."""
    if not any(ENTITY.search(t) for _, _, t in ours):
        return None
    return [(s, x, plain(t)) for s, x, t in ours], theirs


def kd_style(e, ours, theirs):
    """KD-T-STYLE: remove the tags ffmpeg writes for styling."""
    if not e["styled"] or not any(STYLE_TAG.search(t) for _, _, t in theirs):
        return None
    return ours, [(s, x, STYLE_TAG.sub("", t)) for s, x, t in theirs]


def kd_leading_spaces(e, ours, theirs):
    """KD-T-LEAD-SPACE: ffmpeg drops a cue's leading spaces. Each such cue
    must be one of the source's own samples (`e['sample_cues']`: its exact
    times and its text, read from the sample tables and written as CoreAV
    writes cue text), spaces included."""
    cues = e.get("sample_cues") or ()
    if len(ours) != len(theirs) or not cues:
        return None
    plain_cues = {(s, x, plain(t)) for s, x, t in cues}
    fixed, changed = [], False
    for a, b in zip(ours, theirs):
        if (a[2] != b[2] and a[2].startswith(" ") and a[2].lstrip(" ") == b[2]
                and (tuple(a) in cues or tuple(a) in plain_cues)):
            fixed.append((a[0], a[1], b[2]))
            changed = True
        else:
            fixed.append(a)
    return (fixed, theirs) if changed else None


def kd_empty(e, ours, theirs):
    """KD-T-EMPTY: drop ffmpeg's cues without text."""
    kept = [c for c in theirs if c[2].strip()]
    return (ours, kept) if len(kept) < len(theirs) else None


def kd_invalid(e, ours, theirs):
    """KD-T-INVALID: drop CoreAV's cues with replaced text; ffmpeg drops
    samples that are not valid UTF-8. Each cue dropped must be one of the
    file's own invalid samples (`invalid_samples`, read from its sample
    tables): its text, decoded with replacement, and its exact times where
    the edit model gives them (`exact_ms`); and as many cues as samples."""
    if not e["invalid_text"] or not e.get("invalid_samples"):
        return None
    dropped = [c for c in ours if "\ufffd" in c[2]]
    kept = [c for c in ours if "\ufffd" not in c[2]]
    if not dropped:
        return None
    samples = list(e["invalid_samples"])
    if e.get("exact_ms") is None or len(dropped) != len(samples):
        return None
    exact = set(e["exact_ms"])
    for cue in dropped:
        # As CoreAV writes it, or with its escapes undone where KD-T-ESCAPE
        # undid them already.
        match = next((s for s in samples if cue[2] in (s[0], plain(s[0])) and (cue[0], cue[1]) == s[1]), None)
        if match is None or (cue[0], cue[1]) not in exact:
            return None
        samples.remove(match)
    return kept, theirs


def kd_cr(e, ours, theirs):
    """KD-T-CR: ffmpeg drops the CR line breaks of QuickTime text."""
    if e["entry"] != "text" or not e["source_cr"] or len(ours) != len(theirs):
        return None
    joined, changed = [], False
    for a, b in zip(ours, theirs):
        if a[2] != b[2] and "\n" in a[2] and a[2].replace("\n", "") == b[2].replace("\n", ""):
            joined.append((a[0], a[1], b[2]))
            changed = True
        else:
            joined.append(a)
    return (joined, theirs) if changed else None


def kd_zero_end(e, ours, theirs):
    """KD-T-ZERO-END: ffmpeg ends a cue where it starts when it has no
    duration for the packet. CoreAV's end must then be the sample's exact
    end rounded as CoreAV rounds (`exact_ms`): ffmpeg gives none to compare
    it with."""
    if len(ours) != len(theirs) or not e["packets_without_duration"] or not e.get("exact_ms"):
        return None
    exact = set(e["exact_ms"])
    fixed, count = [], 0
    for a, b in zip(ours, theirs):
        if b[0] == b[1] and a[0] == b[0] and a[1] > a[0] and (a[0], a[1]) in exact:
            fixed.append((b[0], a[1], b[2]))
            count += 1
        else:
            fixed.append(b)
    return (ours, fixed) if 0 < count <= e["packets_without_duration"] else None


def kd_edit_align(e, ours, theirs):
    """KD-T-EDIT-ALIGN: ffmpeg starts the track's timeline at the first
    sample at or after the edit's media time, CoreAV at the media time."""
    shift = e["edit_shift_ms"]
    if not shift or len(ours) != len(theirs):
        return None
    # Each side rounds its own times to the millisecond.
    moved = []
    for i, (a, b) in enumerate(zip(ours, theirs)):
        if abs(a[0] - (b[0] + shift)) > 1.5:
            return None
        if abs(a[1] - (b[1] + shift)) <= 1.5:
            moved.append((a[0], a[1], b[2]))
        elif i == len(ours) - 1:
            moved.append((a[0], b[1], b[2]))  # left to KD-T-LAST-END
        else:
            return None
    return ours, moved


def kd_last_end(e, ours, theirs):
    """KD-T-LAST-END: ffmpeg ends the last cue at the end of the file (the
    format duration); CoreAV at the end of the last sample, which the file's
    own sample tables give (`exact_ms`)."""
    if not ours or len(ours) != len(theirs) or e["format_ms"] is None or not e.get("exact_ms"):
        return None
    a, b = ours[-1], theirs[-1]
    if a[0] != b[0] or a[2] != b[2] or a[1] == b[1] or abs(b[1] - e["format_ms"]) > 1:
        return None
    if a[1] != e["exact_ms"][-1][1]:
        return None
    return ours, theirs[:-1] + [(b[0], a[1], b[2])]


def kd_rounding(e, ours, theirs):
    """KD-T-ROUND: times 1 ms apart where milliseconds are not exact in the
    track's timescale; each side rounds differently. CoreAV's times must be
    a sample's exact times rounded as CoreAV rounds (`exact_ms`)."""
    timescale = e["timescale"]
    if not timescale or 1000 % timescale == 0 or len(ours) != len(theirs) or not e.get("exact_ms"):
        return None
    exact = set(e["exact_ms"])
    fixed, changed = [], False
    for a, b in zip(ours, theirs):
        if (a != b and a[2] == b[2] and abs(a[0] - b[0]) <= 1 and abs(a[1] - b[1]) <= 1
                and (a[0], a[1]) in exact):
            fixed.append(a)
            changed = True
        else:
            fixed.append(b)
    return (ours, fixed) if changed else None


CUE_REWRITES = {
    "escapes": kd_escape, "style_tags": kd_style, "leading_spaces": kd_leading_spaces, "empty_cues": kd_empty,
    "invalid_text": kd_invalid, "cr_line_breaks": kd_cr, "zero_end": kd_zero_end, "edit_align": kd_edit_align,
    "last_end": kd_last_end, "rounding": kd_rounding,
}


def mediainfo_hours(e):
    """KD-T-MI-HOURS: MediaInfo skips cues with timestamps of 100 hours or
    more (three or more hour digits, which WebVTT allows)."""
    late = [c for c in e["cues"] if c[1] >= 100 * 3_600_000]
    kept = [c for c in e["cues"] if c[1] < 100 * 3_600_000]
    if not late or not kept:
        return False
    return e["got"] == {"Format": "WebVTT", "Events_Total": str(len(kept)),
                        "Duration_Start": f"{kept[0][0] / 1000:.3f}", "Duration_End": f"{kept[-1][1] / 1000:.3f}"}


def edit_rate(e):
    """KD-T-EDIT-RATE: CoreAV refuses an edit at a rate other than 1."""
    return e["refused_edit"] and any(rate != 0x10000 for _, media_time, rate in e["elst"] or [] if media_time != -1)


def nero_chapters(e):
    """KD-T-CHPL: ffprobe reads Nero `chpl` chapters."""
    return not e["ours"] and bool(e["theirs"]) and "chpl" in e["udta"] and not e["chapter_text_tracks"]


def gopro_hilights(e):
    """KD-T-HMMT: ffprobe reads GoPro HiLight tags as chapters."""
    return (not e["ours"] and bool(e["theirs"]) and "HMMT" in e["udta"] and not e["chapter_text_tracks"]
            and all(not title for _, _, title in e["theirs"]))


def other_chapter_track(e):
    """KD-T-CHAP-ORDER: several text tracks are named as chapters; CoreAV
    reads the first one an enabled track names, ffprobe others. CoreAV's
    chapters must be that track's samples, read independently: their texts,
    and their times from `stts` when the track has no edit that moves them."""
    first = e["chapter_text_tracks"][:1]
    if len(e["all_chapter_text_tracks"]) < 2 or [e["ours_track"]] != first or not e["theirs"]:
        return False
    if [title for _, _, title in e["ours"]] != e["ours_samples"]:
        return False
    if e["ours_times"] is not None and [(a, b) for a, b, _ in e["ours"]] != e["ours_times"]:
        return False
    texts = {t.strip() for t in e["chapter_texts"]}
    return all(title.strip() in texts for _, _, title in e["theirs"])


PREDICATES = {"mediainfo_hours": mediainfo_hours, "edit_rate": edit_rate, "nero_chapters": nero_chapters, "gopro_hilights": gopro_hilights,
              "other_chapter_track": other_chapter_track}


def known_divergences():
    entries = json.loads(KD_FILE.read_text(encoding="utf-8"))["entries"]
    for e in entries:
        if e["predicate"] not in CUE_REWRITES and e["predicate"] not in PREDICATES:
            raise SystemExit(f"harness error: unknown predicate {e['predicate']}")
    return entries


def reconcile(entries, evidence, ours, theirs):
    """Applies the cue rewrites, each at most once, in the order of the
    entries and again while one applies, until the lists agree; returns the
    ids used and the rewritten lists."""
    used = []
    progress = True
    while ours != theirs and progress:
        progress = False
        for entry in entries:
            rewrite = CUE_REWRITES.get(entry["predicate"])
            if ours == theirs or entry["id"] in used or rewrite is None:
                continue
            result = rewrite(evidence, ours, theirs)
            if result is not None:
                ours, theirs = result
                used.append(entry["id"])
                progress = True
    return used, ours, theirs


# --------------------------------------------------------------- checks

def edit_shift_ms(trak):
    """With one media edit starting at media time M > 0: how far after M the
    first sample at or after M starts, in milliseconds."""
    edits = [x for x in trak.get("elst") or [] if x[1] != -1]
    if len(edits) != 1 or edits[0][1] <= 0 or not trak.get("timescale"):
        return None
    media_time, t = edits[0][1], 0
    for count, delta in trak.get("stts", []):
        if delta and t + count * delta >= media_time:
            t += -(-(media_time - t) // delta) * delta
            return (t - media_time) * 1000 / trak["timescale"]
        t += count * delta
    return None


def stream_evidence(path, index, headers, summary, args):
    traks = (headers or {}).get("traks", [])
    trak = traks[index] if index < len(traks) else {}
    trex = (headers or {}).get("trex") or {}
    fragments = cc.fragment_samples(path, trak["id"], trex.get(trak["id"])) if trak.get("id") in trex else []
    packets = ffprobe_json(args.ffprobe, "-select_streams", str(index), "-show_packets",
                           "-show_entries", "packet=pts,duration", str(path)).get("packets", [])
    fmt = ffprobe_json(args.ffprobe, "-show_format", str(path)).get("format", {})
    styling = summary["dropped"]["styling"]
    return {
        "styled": sum(styling.get(k, 0) for k in STYLE_BOXES),
        "invalid_text": summary["dropped"]["invalid_text"],
        "entry": trak.get("entry"),
        "source_cr": trak.get("entry") == "text" and any("\r" in t for t in sample_texts(path, trak)),
        "timescale": trak.get("timescale"),
        "packets_without_duration": sum(1 for p in packets if p.get("duration") is None),
        "format_ms": round(float(fmt["duration"]) * 1000) if fmt.get("duration") else None,
        "edit_shift_ms": edit_shift_ms(trak),
        "exact_ms": (None if fragments is None
                     else exact_cue_times(trak, (headers or {}).get("timescale"), fragments=fragments)),
        "invalid_samples": invalid_samples(path, trak, (headers or {}).get("timescale")),
        "sample_cues": source_cues(path, trak, (headers or {}).get("timescale")),
    }


def cue_lines(text):
    """A sample's text as WebVTT holds it, less the escapes CoreAV adds
    (`KD-T-ESCAPE` undoes the same ones): the lines that are not blank,
    joined by one newline (docs/design/text.md, decision 7)."""
    for separator in ("\r\n", "\r", "\u2028", "\u2029", "\u0085"):
        text = text.replace(separator, "\n")
    return "\n".join(line for line in text.split("\n") if line.strip())


def expected_cue_times(path, trak, headers, summary, args):
    """(start, end) in whole milliseconds of the cues the extractor should
    write: the times each presented sample gets through the edit list
    (`exact_cue_times`), less the samples whose text is empty, which
    `docs/design/text.md` (decision 7) drops. `None` when the track holds a
    sample the harness cannot account for, so that the comparison is never
    weakened silently."""
    dropped = summary["dropped"]
    if dropped["malformed"] or dropped["invalid_text"] or dropped["trailing_bytes"]:
        return None
    # `sample_texts` ends a sample's text at its first NUL, as CoreAV does
    # (docs/design/text.md, point 8), but keeps a byte order mark, where
    # CoreAV strips a UTF-8 one and decodes UTF-16 after a UTF-16 one.
    # Such a sample would make the text comparison below wrong rather than
    # absent.
    raws = sample_bytes(path, trak)
    if raws is None or any(raw.startswith((b"\xfe\xff", b"\xff\xfe", b"\xef\xbb\xbf"))
                           for raw in raws):
        return None
    trex = (headers or {}).get("trex") or {}
    fragments = cc.fragment_samples(path, trak["id"], trex.get(trak["id"])) if trak.get("id") in trex else []
    if fragments is None:
        return None
    times = exact_cue_times(trak, (headers or {}).get("timescale"), keep_empty=True, fragments=fragments)
    texts = sample_texts(path, trak)
    if times is None or texts is None or len(times) != len(texts):
        return None
    # A sample the edits show in several places gives a cue each, so the
    # counts are checked sample by sample: every sample is presented with
    # text, presented without it, or not presented.
    # Every sample is presented with text, presented without it, or not
    # presented; an empty list of intervals is still presented, with every
    # one of them rounding to no duration (`too_short`).
    shown = [(spans, text) for spans, text in zip(times, texts)
             if spans is not None and text.strip()]
    if len(shown) + dropped["empty"] + dropped["not_presented"] != summary["samples"]:
        return None
    return [(a, b, cue_lines(text)) for spans, text in shown for a, b in spans]


def check_subtitles(path, index, headers, args, work, entries):
    """Returns (problems, known divergences used, informational)."""
    out = work / f"{path.name}.{index}.vtt"
    result = run([args.coreav, "extract-subtitles", "--force", "--stream", str(index), str(path), str(out)],
                 check=False)
    if result.returncode != 0:
        traks = (headers or {}).get("traks", [])
        error = result.stderr.decode(errors="replace")
        evidence = {"refused_edit": result.returncode == 1 and "edit list" in error,
                    "elst": traks[index].get("elst") if index < len(traks) else None}
        if edit_rate(evidence):
            return [], ["KD-T-EDIT-RATE"], []
        return [f"coreav exit {result.returncode}: {error[:300]}"], [], []
    summary = json.loads(result.stdout)
    data = out.read_bytes()
    problems, info, covered = [], [], []
    if summary["output"]["bytes"] != len(data):
        problems.append(f"summary bytes {summary['output']['bytes']}, file {len(data)}")
    ours, syntax = strict_webvtt(data)
    problems += [f"WebVTT: {p}" for p in syntax]
    if summary["cues"] != len(ours):
        problems.append(f"summary cues {summary['cues']}, file {len(ours)}")

    # The output re-probed.
    if ours:
        probed = ffprobe_json(args.ffprobe, "-show_format", "-show_streams", "-show_packets",
                              "-show_data_hash", "SHA256", str(out))
        streams = probed.get("streams", [])
        if probed["format"].get("format_name") != "webvtt" or [s.get("codec_name") for s in streams] != ["webvtt"]:
            problems.append(f"ffprobe: format {probed['format'].get('format_name')!r}, "
                            f"streams {[s.get('codec_name') for s in streams]}")
        packets = [(int(p["pts"]), int(p["duration"]), p.get("data_hash")) for p in probed.get("packets", [])]
        expected = [(s, e - s, "SHA256:" + hashlib.sha256(t.encode()).hexdigest()) for s, e, t in ours]
        if packets != expected:
            differing = [i for i, (a, b) in enumerate(zip(packets, expected)) if a != b][:5]
            problems.append(f"ffprobe packets differ from the cues at {differing} "
                            f"({len(packets)} packets, {len(expected)} cues)")
        if args.mediainfo:
            tracks = json.loads(run([args.mediainfo, "--Output=JSON", str(out)]).stdout)["media"]["track"]
            text = [t for t in tracks if t.get("@type") == "Text"]
            want = {"Format": "WebVTT", "Events_Total": str(len(ours)),
                    "Duration_Start": f"{ours[0][0] / 1000:.3f}", "Duration_End": f"{ours[-1][1] / 1000:.3f}"}
            got = {k: text[0].get(k) for k in want} if len(text) == 1 else None
            if got != want and mediainfo_hours({"cues": ours, "got": got}):
                covered.append("KD-T-MI-HOURS")
            elif got != want:
                problems.append(f"MediaInfo: {got}, expected {want}")

    # A track with several media edits is judged on its own boxes, not
    # through ffmpeg's model of such a list (docs/design/edit-list-writers.md,
    # common decision 3, and its "`coreav-text` timing" section): ffmpeg
    # anchors later edits on decode times, rounds durations to whole ticks
    # and repeats samples at the boundaries (docs/design/edit-lists.md,
    # "What `ffprobe` lists", rules 1 to 9). Its cues are then reported, not
    # gated.
    traks = (headers or {}).get("traks") or []
    trak = next((x for x in traks if x.get("id") == (summary.get("stream") or {}).get("track_id")), None)
    trak = trak if trak is not None else (traks[index] if index < len(traks) else {})
    elst = trak.get("elst") or []
    simple = (len(elst) == 1 and elst[0][1] != -1) or (
        len(elst) == 2 and elst[0][1] == -1 and elst[1][1] != -1)
    several = bool(elst) and not simple
    if several:
        exact = expected_cue_times(path, trak, headers, summary, args)
        mine = [(s, e, plain(text)) for s, e, text in ours]
        if exact is None:
            problems.append("the cues could not be computed from the file's own boxes")
        elif mine != exact:
            differing = [i for i, (a, b) in enumerate(zip(mine, exact)) if a != b][:5]
            problems.append(f"cues differ from the edit list applied by 14496-12 8.6.6 at "
                            f"{differing} ({len(ours)} cues, {len(exact)} expected); "
                            f"first {mine[differing[0]] if differing else None} against "
                            f"{exact[differing[0]] if differing else None}")

    # Against ffmpeg's WebVTT of the source stream.
    reference = run([args.ffmpeg, "-v", "error", "-i", str(path), "-map", f"0:{index}", "-c:s", "webvtt",
                     "-f", "webvtt", "-"], check=False)
    if reference.returncode != 0:
        info.append(f"ffmpeg cannot convert the stream: {reference.stderr.decode(errors='replace')[:200]}")
        return problems, covered, info
    theirs = loose_webvtt(reference.stdout.decode("utf-8", errors="replace"))
    if several:
        # ffmpeg's cues are the second view and are reported, not gated: it
        # anchors a later edit on decode times, rounds durations to whole
        # ticks and repeats samples at the boundaries, so it writes cues
        # CoreAV does not (docs/design/edit-lists.md, "What `ffprobe`
        # lists", rules 1 to 9). The gate above compares every cue, text
        # included, with the file's own boxes.
        if ours != theirs:
            info.append(f"second view: ffmpeg's cues differ ({len(ours)} ours, {len(theirs)} ffmpeg); "
                        f"ffmpeg applies its own model of a list of several media edits "
                        f"(docs/design/edit-lists.md, rules 1 to 9)")
        return problems, covered, info
    if ours == theirs:
        return problems, covered, info
    used, ours, theirs = reconcile(entries, stream_evidence(path, index, headers, summary, args), ours, theirs)
    if ours != theirs:
        differing = [i for i, (a, b) in enumerate(zip(ours, theirs)) if a != b]
        detail = [(ours[i], theirs[i]) for i in differing[:3]]
        # Every difference, digested, so that one more shows in the message.
        digest = hashlib.sha256(repr((ours, theirs)).encode()).hexdigest()[:12]
        problems.append(f"cues differ from ffmpeg's ({len(ours)} ours, {len(theirs)} ffmpeg, "
                        f"after {used or 'no known divergence'}): {detail}, "
                        f"{len(differing)} differing, digest {digest}")
        return problems, covered, info
    return problems, covered + used, info


def sample_texts(path, trak, cap=10_000):
    """The text of each sample of a track (16-bit length, then the text, up
    to a NUL), read through its sample tables (ISO/IEC 14496-12 8.7)."""
    sizes, chunks, runs = trak["stsz"][1], trak["chunks"], trak["stsc"]
    texts, sample = [], 0
    with open(path, "rb") as f:
        for i, (first, per_chunk, _) in enumerate(runs):
            last = runs[i + 1][0] - 1 if i + 1 < len(runs) else len(chunks)
            for chunk in range(first, last + 1):
                if chunk - 1 >= len(chunks):
                    return texts
                at = chunks[chunk - 1]
                for _ in range(per_chunk):
                    if sample >= len(sizes) or len(texts) >= cap:
                        return texts
                    f.seek(at)
                    raw = f.read(min(sizes[sample], 1 << 16))
                    if len(raw) >= 2:
                        n = struct.unpack(">H", raw[:2])[0]
                        texts.append(raw[2:2 + n].split(b"\0")[0].decode("utf-8", errors="replace"))
                    at += sizes[sample]
                    sample += 1
    return texts


def invalid_samples(path, trak, movie_timescale):
    """(text decoded with replacement, (start, end) in milliseconds) of each
    presented sample whose text is not valid UTF-8 and has no UTF-16 byte
    order mark, from the file's own sample tables and edit list; None when
    the times are not known (`exact_cue_times`)."""
    times = exact_cue_times(trak, movie_timescale, keep_empty=True)
    if times is None or not trak.get("stsz"):
        return None
    found = []
    for raw, spans in zip(sample_bytes(path, trak), times):
        if not spans or raw.startswith((b"\xfe\xff", b"\xff\xfe")):
            continue
        try:
            raw.decode("utf-8")
        except UnicodeDecodeError:
            # A sample the edits show at several places is one cue each.
            found.extend((written_text(raw.decode("utf-8", errors="replace")), at) for at in spans)
    return found


def written_text(text):
    """A sample's text as CoreAV writes cue text (docs/design/text.md, point
    8): the TS 26.245 line breaks become LF, empty or white-space lines go,
    and `&`, `<`, `>` are escaped."""
    text = re.sub("\r\n|[\r\u2028\u2029\u0085]", "\n", text)
    text = "\n".join(line for line in text.split("\n") if line.strip())
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def source_cues(path, trak, movie_timescale):
    """(start, end, text) of every cue the track's samples give, from the
    file's own sample tables and edit list (`exact_cue_times`), the text as
    CoreAV writes cue text (UTF-16 with its byte order mark, else UTF-8);
    empty when the times are not known."""
    times = exact_cue_times(trak, movie_timescale, keep_empty=True)
    if times is None or not trak.get("stsz"):
        return set()
    cues = set()
    for raw, spans in zip(sample_bytes(path, trak), times):
        if raw.startswith((b"\xfe\xff", b"\xff\xfe")):
            text = raw.decode("utf-16", errors="replace")
        else:
            text = raw.decode("utf-8", errors="replace")
        cues.update((start, end, written_text(text)) for start, end in spans or [])
    return cues


def sample_bytes(path, trak, cap=100_000):
    """The text bytes of each sample (after the 16-bit length, up to a NUL),
    read through its sample tables (ISO/IEC 14496-12 8.7)."""
    sizes, chunks, runs = trak["stsz"][1], trak["chunks"], trak["stsc"]
    out, sample = [], 0
    with open(path, "rb") as f:
        for i, (first, per_chunk, _) in enumerate(runs):
            last = runs[i + 1][0] - 1 if i + 1 < len(runs) else len(chunks)
            for chunk in range(first, last + 1):
                if chunk - 1 >= len(chunks):
                    return out
                at = chunks[chunk - 1]
                for _ in range(per_chunk):
                    if sample >= len(sizes) or len(out) >= cap:
                        return out
                    f.seek(at)
                    raw = f.read(min(sizes[sample], 1 << 16))
                    n = struct.unpack(">H", raw[:2])[0] if len(raw) >= 2 else 0
                    out.append(raw[2:2 + n].split(b"\0")[0])
                    at += sizes[sample]
                    sample += 1
    return out


def edit_windows(edits, timescale, movie_timescale):
    """Each media edit of an edit list as `(media start, media end,
    presented at)` in media ticks, the end `None` for the simple model's
    open-ended edit (ISO/IEC 14496-12 8.6.6: the entries play one after
    another, an empty edit presenting nothing for its duration). `None` for
    a list outside the model CoreAV applies (docs/design/edit-lists.md,
    decision 1): a rate other than 1, a media time below -1, windows that do
    not increase without overlap, or a media edit of duration 0 beside
    others."""
    windows, at = [], fractions.Fraction(0)
    for duration, media_time, rate in edits:
        length = fractions.Fraction(duration * timescale, movie_timescale)
        if media_time < -1:
            return None
        if media_time >= 0:
            if rate != 0x10000:
                return None
            start = fractions.Fraction(media_time)
            end = start + length if duration else None
            if windows:
                previous = windows[-1][1]
                if previous is None or start < previous:
                    return None
            windows.append((start, end, at))
        at += length
    if len(windows) > 1 and any(end is None for _, end, _ in windows):
        return None
    return windows


def presented_intervals(start, delta, windows):
    """The parts of the sample whose composition interval is
    `[start, start + delta)` that the media edits present, on the
    presentation timeline, in order, runs with nothing between them merged
    (ISO/IEC 14496-12 8.6.6: the entries play one after another, each
    presenting its own window of the media). Empty when no edit presents
    it. Written from the specification, not from the code under test."""
    if not windows:
        return [(start, start + delta)]
    end = start + delta
    shown = []
    for m0, m1, at in windows:
        if delta:
            if not (start < m1 if m1 is not None else True) or not (end > m0):
                continue
        elif not (m0 <= start and (m1 is None or start < m1)):
            continue
        a = max(start, m0) - m0 + at
        b = (min(end, m1) if m1 is not None else end) - m0 + at
        a = max(a, fractions.Fraction(0))
        if b < a:
            continue
        if shown and shown[-1][1] == a:
            shown[-1] = (shown[-1][0], b)
        else:
            shown.append((a, b))
    return shown


def exact_cue_times(trak, movie_timescale, keep_empty=False, fragments=None):
    """(start, end) in whole milliseconds of each sample the track presents,
    from its own `stts`, then its movie fragments' samples (`fragments`, from
    fragment_samples), and its edit list (ISO/IEC 14496-12 8.6.1.2, 8.8,
    8.6.6),
    rounded as CoreAV says it rounds (docs/design/text.md: to the nearest,
    halves away from zero), cues that round to no duration left out. Each
    sample is cut to the media edit that maps it: the one whose media
    interval holds its start, else the first its interval reaches, else the
    next (docs/design/edit-lists.md, decision 2, as
    docs/design/edit-list-writers.md settles for `coreav-text`). `None` for
    a list outside the model. Independent of the code under test."""
    timescale = trak.get("timescale")
    edits = trak.get("elst") or []
    if not timescale or (edits and not movie_timescale):
        return None
    windows = edit_windows(edits, timescale, movie_timescale) if edits else []
    if windows is None or (edits and not windows):
        return None

    def ms(t):
        value = t * 1000
        return int(value + fractions.Fraction(1, 2)) if value >= 0 else -int(-value + fractions.Fraction(1, 2))
    def samples():
        t = 0
        for count, delta in trak.get("stts", []):
            for _ in range(count):
                yield t, delta
                t += delta
        yield from fragments or []

    times = []
    for n, (t, delta) in enumerate(samples()):
        if n >= 100_000:
            return None
        shown = presented_intervals(fractions.Fraction(t), delta, windows)
        if not shown:
            # No media edit shows the sample. With `keep_empty`, `None`
            # marks that, where an empty list marks a sample the edits show
            # but whose every interval rounds to no duration.
            if keep_empty:
                times.append(None)
            continue
        kept = [(ms(start / timescale), ms(end / timescale)) for start, end in shown]
        kept = [(a, b) for a, b in kept if b > a]
        if kept or keep_empty:
            times.append(kept)
    return times if keep_empty else [span for sample in times for span in sample]


def sample_times(trak, movie_timescale):
    """(start, end) in seconds of each presented sample from `stts`, when the
    track has no edit list or one media edit from media time 0 (which cuts
    the samples at its end); None otherwise."""
    edits = trak.get("elst")
    if edits and (len(edits) != 1 or edits[0][1] != 0 or not movie_timescale) or not trak.get("timescale"):
        return None
    limit = fractions.Fraction(edits[0][0], movie_timescale) if edits else None
    times, t = [], 0
    for count, delta in trak["stts"]:
        for _ in range(min(count, 10_000)):
            start = fractions.Fraction(t, trak["timescale"])
            end = fractions.Fraction(t + delta, trak["timescale"])
            t += delta
            if limit is not None:
                if start >= limit and delta or start > limit:
                    continue
                end = min(end, limit)
            times.append((start, end))
    return times


def exact_chapter_times(trak, movie_timescale):
    """(start, end) in seconds of each sample of the track, `None` where no
    media edit presents it, from its own `stts` and its edit list, each
    sample cut to the media edit that maps it (ISO/IEC 14496-12 8.6.6 and
    docs/design/edit-lists.md, decision 2). One entry per sample, so that
    the titles line up. Exact, as chapters keep exact ticks
    (docs/design/text.md, decision 7). `None` for a list outside the model.
    Independent of the code under test."""
    timescale = trak.get("timescale")
    edits = trak.get("elst") or []
    if not timescale or (edits and not movie_timescale):
        return None
    windows = edit_windows(edits, timescale, movie_timescale) if edits else []
    if windows is None or (edits and not windows):
        return None

    times, t = [], 0
    for count, delta in trak.get("stts", []):
        for _ in range(min(count, 10_000)):
            shown = presented_intervals(fractions.Fraction(t), delta, windows)
            t += delta
            times.append([(a / timescale, b / timescale) for a, b in shown] or None)
    return times


def check_chapters(path, probe, headers, args, entries):
    result = run([args.coreav, "chapters", str(path)], check=False)
    if result.returncode != 0:
        return [f"coreav exit {result.returncode}: {result.stderr.decode(errors='replace')[:300]}"], [], True
    document = json.loads(result.stdout)

    def seconds(t):
        return fractions.Fraction(t["ticks"]["num"], t["ticks"]["den"] * t["timescale"])

    ours = [(seconds(c["start"]), seconds(c["end"]), c["title"]) for c in document["chapters"]]
    theirs = []
    for c in probe.get("chapters", []):
        base = fractions.Fraction(c["time_base"])
        theirs.append((c["start"] * base, c["end"] * base, c.get("tags", {}).get("title", "")))
    if not ours and not theirs:
        return [], [], False
    traks = (headers or {}).get("traks", [])
    # A chapter track with several media edits is judged on its own boxes,
    # not through ffmpeg's model of such a list
    # (docs/design/edit-list-writers.md, common decision 3).
    own_trak = next((x for x in traks if x.get("id") == (document["stream"] or {}).get("track_id")), None)
    elst = (own_trak or {}).get("elst") or []
    simple = (len(elst) == 1 and elst[0][1] != -1) or (
        len(elst) == 2 and elst[0][1] == -1 and elst[1][1] != -1)
    if elst and not simple:
        exact = exact_chapter_times(own_trak, (headers or {}).get("timescale"))
        titles = sample_texts(path, own_trak)
        if exact is None or titles is None or len(exact) != len(titles):
            return ["the chapter times could not be computed from the file's own boxes"], [], True
        want = [(a, b, title) for spans, title in zip(exact, titles) if spans
                for a, b in spans]
        if ours != want:
            differing = [i for i, (a, b) in enumerate(zip(ours, want)) if a != b][:3]
            return [f"chapters differ from the edit list applied by 14496-12 8.6.6 at {differing} "
                    f"({len(ours)} coreav, {len(want)} expected)"], [], True
        return [], [], True
    if ours == theirs:
        return [], [], True
    text = {t.get("id"): t for t in traks if t.get("entry") in ("text", "tx3g")}

    def named(enabled_only):
        ids = [i for t in traks if t.get("enabled") or not enabled_only for i in t["chap"]]
        return [i for i in dict.fromkeys(ids) if i in text]

    others = named(False)
    ours_track = (document["stream"] or {}).get("track_id")
    own = text.get(ours_track, {})
    evidence = {"ours": ours, "theirs": theirs, "udta": (headers or {}).get("udta", []),
                "chapter_text_tracks": named(True), "all_chapter_text_tracks": others,
                "ours_track": ours_track,
                "ours_samples": sample_texts(path, own) if own else [],
                "ours_times": sample_times(own, (headers or {}).get("timescale")) if own else None,
                "chapter_texts": [x for i in others for x in sample_texts(path, text[i])] if len(others) > 1 else []}
    for entry in entries:
        predicate = PREDICATES.get(entry["predicate"])
        if "chapters" in entry["level"] and predicate and predicate(evidence):
            return [], [entry["id"]], True
    differing = [i for i, (a, b) in enumerate(zip(ours, theirs)) if a != b]
    detail = [(str(ours[i][0]), str(ours[i][1]), ours[i][2], str(theirs[i][0]), str(theirs[i][1]), theirs[i][2])
              for i in differing[:3]]
    digest = hashlib.sha256(repr((ours, theirs)).encode()).hexdigest()[:12]
    return [f"chapters differ ({len(ours)} coreav, {len(theirs)} ffprobe): {detail}, "
            f"{len(differing)} differing, digest {digest}"], [], True


def strict_files(overlay_path):
    if overlay_path is None:
        return set()
    overlay = json.loads(pathlib.Path(overlay_path).read_text(encoding="utf-8"))
    items = overlay["levels"]["streams"]["all"].get("covered_items")
    if items is None:
        raise SystemExit("harness error: overlay has no covered_items (old oracle_kd.py?)")
    return {item["file"] for item in items if "KD-STRICT" in item["kd"]}


# `text_sized.mp4` and `chapters_sized.mp4` carry a timed text track of
# 1,000,000 Hz in a 1,000 Hz movie, with cues at 0.25-0.9, 1.0-1.6 and
# 1.6-2.0 s. (source fixture, track position, name, the edit list to put on
# it, what it is for).
SEVERAL_EDITS = (
    ("text_sized.mp4", 1, "splice", ["700:0", "700:1300000"],
     "two media edits with media between them that neither shows: the first "
     "cue is cut at the first edit's end, the second is shown from the "
     "middle of the second edit, and the sample between them is not shown"),
    ("chapters_sized.mp4", 1, "splice", ["700:0", "700:1300000"],
     "the same on a chapter track"),
    ("text_sized.mp4", 1, "contiguous",
     ["500:0", "500:500000", "500:1000000", "500:1500000"],
     "four media edits that play the media back to back, which present "
     "exactly what no edit list does: every cue must come out whole"),
    ("chapters_sized.mp4", 1, "contiguous",
     ["500:0", "500:500000", "500:1000000", "500:1500000"],
     "the same on a chapter track"),
    ("text_sized.mp4", 1, "boundary", ["1000:0", "1000:1000000"],
     "two contiguous media edits whose boundary is a sample boundary: the "
     "cue at 1.0 s starts exactly where the second edit does"),
    ("text_sized.mp4", 1, "split-cue", ["400:0", "200:-1", "1600:500000"],
     "a splice inside the first cue with an empty edit over it, so that "
     "the two media edits show the one sample at two places: two cues"),
)


def several_edit_variants(media, work):
    """The fixtures above with their text track's edit list replaced by one
    with several media edits, written into `work`
    (`docs/design/edit-list-writers.md`, "`coreav-text` timing").

    They are built here and not in `scripts/gen-fixtures.sh` because the
    probe oracle has no model of what `ffprobe` lists for a timed text
    track with such a list (`docs/design/edit-lists.md`, "What `ffprobe`
    lists", measured for video and audio only), so a fixture carrying one
    would leave packets uncovered there."""
    craft = pathlib.Path(__file__).resolve().parent / "elst_craft.py"
    sources = {name for name, _track, _label, _edits, _why in SEVERAL_EDITS}
    present = {name for name in sources if (pathlib.Path(media) / name).exists()}
    if not present:
        # Not the fixture directory: a corpus run gets its several-edit
        # tracks from the corpus itself, if it has any.
        return [], []
    out, missing = [], []
    for name, track, label, edits, _why in SEVERAL_EDITS:
        source = pathlib.Path(media) / name
        if not source.exists():
            # Some of the sources are there, so this one was expected.
            missing.append(f"{name}: the source fixture is not in --media")
            continue
        target = work / f"several_edits_{label}_{name}"
        result = subprocess.run([sys.executable, "-I", str(craft), str(source), str(target),
                                 str(track), *edits], capture_output=True)
        if result.returncode == 0:
            out.append(target)
        else:
            missing.append(f"{name} ({label}): {result.stderr.decode(errors='replace')[:200]}")
    return out, missing


def media_files(media):
    media = pathlib.Path(media)
    manifest = media / "MANIFEST"
    if manifest.exists():
        names = [line.split(" ", 1)[1] for line in manifest.read_text().splitlines() if line.startswith("generated ")]
        paths = [media / n for n in names]
    else:
        paths = sorted(p for p in media.iterdir() if p.is_file())
    return [p for p in paths if p.suffix.lower() in IN_SCOPE]


def report_line(name, problems, used, info):
    print(f"{'FAIL' if problems else 'kd  ' if used else 'ok  '} {name}" + (f" ({', '.join(used)})" if used else ""))
    for problem in problems:
        print(f"     {problem}")
    for difference in info:
        print(f"     (informational) {difference}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--coreav", required=True)
    parser.add_argument("--media", required=True)
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mediainfo")
    parser.add_argument("--kd-overlay", help="known-divergences.json from scripts/oracle_kd.py")
    parser.add_argument("--require-several-edits", action="store_true",
                        help="fail when the several-edit inputs cannot be built from --media "
                             "(the fixture run; a corpus directory holds no source for them)")
    parser.add_argument("--report")
    args = parser.parse_args()

    entries = known_divergences()
    strict = strict_files(args.kd_overlay)
    results = {"subtitles": [], "chapters": [], "rejected": []}
    with tempfile.TemporaryDirectory() as tmp:
        work = pathlib.Path(tmp)
        variants, missing = several_edit_variants(args.media, work)
        if args.require_several_edits and not variants:
            missing.append("none was built, and --require-several-edits was given")
        for problem in missing:
            # A several-edit input that cannot be built would make its pass
            # criterion vacuous, so it is a failure of the run.
            results["subtitles"].append({"file": problem, "stream_index": None,
                                         "problems": [f"several-edit input not built: {problem}"],
                                         "covered": [], "informational": []})
            print(f"FAIL several-edit input not built: {problem}")
        for path in media_files(args.media) + variants:
            try:
                probe = ffprobe_json(args.ffprobe, "-show_streams", "-show_chapters", str(path))
            except (RuntimeError, ValueError, subprocess.TimeoutExpired):
                continue
            subtitle_streams = [s["index"] for s in probe.get("streams", []) if s.get("codec_name") == "mov_text"]
            ours = run([args.coreav, "probe", str(path)], check=False)
            if ours.returncode != 0:
                if subtitle_streams or probe.get("chapters"):
                    error = json.loads(ours.stderr or b"{}")
                    ok = path.name in strict
                    results["rejected"].append({"file": path.name, "kind": error.get("kind"),
                                                "covered": "KD-STRICT" if ok else None})
                    print(f"{'kd  ' if ok else 'FAIL'} {path.name}: coreav probe rejects it ({error.get('kind')})")
                continue
            headers = movie_headers(path)
            for index in subtitle_streams:
                try:
                    problems, used, info = check_subtitles(path, index, headers, args, work, entries)
                except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
                    problems, used, info = [f"harness: {error!r}"], [], []
                results["subtitles"].append({"file": path.name, "stream_index": index, "problems": problems,
                                             "covered": used, "informational": info})
                report_line(f"{path.name} stream {index}", problems, used, info)
            try:
                problems, used, compared = check_chapters(path, probe, headers, args, entries)
            except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
                problems, used, compared = [f"harness: {error!r}"], [], True
            if compared:
                results["chapters"].append({"file": path.name, "problems": problems, "covered": used})
                report_line(f"{path.name} chapters", problems, used, [])
    totals = {}
    for kind in ("subtitles", "chapters"):
        items = results[kind]
        passed = sum(1 for r in items if not r["problems"])
        direct = sum(1 for r in items if not r["problems"] and not r["covered"])
        by_kd = {}
        for r in items:
            if not r["problems"]:
                for kd in r["covered"]:
                    by_kd[kd] = by_kd.get(kd, 0) + 1
        totals[kind] = {"total": len(items), "passed": passed, "direct": direct, "by_kd": by_kd}
        print(f"{kind}: {passed}/{len(items)} pass every oracle ({direct} directly, "
              f"{passed - direct} through known divergences {by_kd})")
    rejected = [r for r in results["rejected"] if not r["covered"]]
    print(f"rejected by coreav probe: {len(results['rejected'])} ({len(rejected)} not covered by KD-STRICT)")
    if args.report:
        pathlib.Path(args.report).write_text(json.dumps({"totals": totals, **results}, indent=2) + "\n")
    compared = sum(t["total"] for t in totals.values())
    ok = compared > 0 and all(t["passed"] == t["total"] for t in totals.values())
    if args.kd_overlay:
        ok = ok and not rejected
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
