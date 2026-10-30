#!/usr/bin/env python3
"""Mutation audit of the known divergences of the oracles that run `coreav`
themselves: scripts/text_check.py (docs/known-divergences-text.md),
scripts/hls_check.py (docs/known-divergences-hls.md) and
scripts/policy_check.py (docs/known-divergences-policy.md). The method is
tools/oracle/writer_kd_audit.py's: one bug-shaped change at a time, judged
by the writer's own oracle, must stay visible.

The oracle runs unchanged against a wrapper that runs the real `coreav` and
then changes its output as the environment's COREAV_MUTATION names, keeping
the command's own summary consistent with the changed output (a bug in
what CoreAV writes, not a mismatch any summary check would see):
- `extract-subtitles` (WebVTT): the first cue's start or end 1 ms later, the
  last cue's end 1 ms earlier, a letter of the first cue's text changed, the
  first cue left out or written twice, an escape undone (`&amp;` to `&`), a
  style tag dropped, a line break inside a cue made a space;
- `chapters` (JSON): the first chapter's start or end one tick later, its
  title changed, the last chapter left out, the first two titles swapped;
- `hls package` (each rendition): the first media segment's `tfdt` +1, its
  first sample's duration or size +1, its first composition offset +512,
  its first sync flag cleared, a byte of its first sample flipped, the
  first EXTINF +0.1 s, the init segment's `mdhd` timescale +1 and its width
  +16 (video) or channel count +1 (audio);
- `probe` and `check` (policy): the document's duration +25%, the video
  width +16, the bit rate doubled, the video codec renamed, the audio
  channels +1 or rate doubled, the last stream left out; the verdict that
  `check` prints is derived again from the changed document by
  tools/oracle/policy_ref.py, so it stays consistent with it.

Each mutation runs the oracle over the whole media directory with its known
divergences, and again without them where nothing new shows, as
writer_kd_audit.py does. Verdicts per item (a file's subtitle stream or
chapters, a package, a file under one policy): caught, COVERED (a known divergence the unmutated run did not
use), ABSORBED (seen only without the known divergences), agrees, no
effect, n/a (the item's output did not change).

usage: writer_kd_audit_cli.py --oracle text|hls|policy --coreav PATH --media DIR
                              [--mediainfo PATH] [--policy FILE ...]
                              [--report OUT]
"""
import argparse
import atexit
import contextlib
import io
import json
import os
import re
import shutil
import stat
import sys
import tempfile
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

WRAPPER = r'''#!/usr/bin/env python3
import json, os, re, subprocess, sys
real, mutation = os.environ["COREAV_REAL"], os.environ.get("COREAV_MUTATION", "")
argv = sys.argv[1:]
done = subprocess.run([real] + argv, capture_output=True)
out = done.stdout
TIME = re.compile(r"(\d+):(\d\d):(\d\d)\.(\d\d\d)")

def ms(m):
    return ((int(m.group(1)) * 60 + int(m.group(2))) * 60 + int(m.group(3))) * 1000 + int(m.group(4))

def fmt(t):
    return f"{t // 3600000:02d}:{t // 60000 % 60:02d}:{t // 1000 % 60:02d}.{t % 1000:03d}"

def shift(line, which, delta):
    times = list(TIME.finditer(line))
    if len(times) < 2:
        return line
    m = times[which]
    return line[:m.start()] + fmt(max(ms(m) + delta, 0)) + line[m.end():]

def vtt(text, name):
    head, *blocks = text.split("\n\n")
    cues = [i for i, b in enumerate(blocks) if "-->" in b]
    if not cues:
        return None
    first, last = cues[0], cues[-1]
    def edit(i, f):
        lines = blocks[i].split("\n")
        k = next(j for j, l in enumerate(lines) if "-->" in l)
        out = f(lines, k)
        if out is None:
            return False
        blocks[i] = "\n".join(out)
        return True
    if name == "cue start +1 ms":
        ok = edit(first, lambda l, k: l[:k] + [shift(l[k], 0, 1)] + l[k + 1:])
    elif name == "cue end +1 ms":
        ok = edit(first, lambda l, k: l[:k] + [shift(l[k], 1, 1)] + l[k + 1:])
    elif name == "last cue end -1 ms":
        ok = edit(last, lambda l, k: l[:k] + [shift(l[k], 1, -1)] + l[k + 1:])
    elif name == "cue letter changed":
        def letter(l, k):
            for j in range(k + 1, len(l)):
                m = re.search(r"[A-Za-z]", l[j])
                if m:
                    c = m.group(0)
                    l[j] = l[j][:m.start()] + ("b" if c != "b" else "c") + l[j][m.end():]
                    return l
            return None
        ok = edit(first, letter)
    elif name == "first cue left out":
        del blocks[first]
        ok = True
    elif name == "first cue twice":
        blocks.insert(first, blocks[first])
        ok = True
    elif name == "escape undone":
        ok = False
        for i in cues:
            if "&amp;" in blocks[i] or "&lt;" in blocks[i]:
                blocks[i] = blocks[i].replace("&amp;", "&", 1) if "&amp;" in blocks[i] else blocks[i].replace("&lt;", "<", 1)
                ok = True
                break
    elif name == "style tag dropped":
        ok = False
        for i in cues:
            m = re.search(r"</?[biu]>", blocks[i])
            if m:
                blocks[i] = blocks[i][:m.start()] + blocks[i][m.end():]
                ok = True
                break
    elif name == "line break joined":
        ok = False
        for i in cues:
            lines = blocks[i].split("\n")
            k = next(j for j, l in enumerate(lines) if "-->" in l)
            if len(lines) > k + 2:
                blocks[i] = "\n".join(lines[:k + 1] + [lines[k + 1] + " " + lines[k + 2]] + lines[k + 3:])
                ok = True
                break
    else:
        return None
    return "\n\n".join([head] + blocks) if ok else None

def chapters(doc, name):
    ch = doc.get("chapters") or []
    if not ch:
        return False
    def tick(t):
        t["ticks"]["num"] += t["ticks"]["den"]
        if "seconds" in t:
            t["seconds"] = t["ticks"]["num"] / t["ticks"]["den"] / t["timescale"]
    if name == "chapter start +1 tick":
        tick(ch[0]["start"])
    elif name == "chapter end +1 tick":
        tick(ch[0]["end"])
    elif name == "chapter title changed":
        ch[0]["title"] = ch[0]["title"] + "x"
    elif name == "last chapter left out":
        ch.pop()
    elif name == "titles swapped":
        if len(ch) < 2 or ch[0]["title"] == ch[1]["title"]:
            return False
        ch[0]["title"], ch[1]["title"] = ch[1]["title"], ch[0]["title"]
    else:
        return False
    return True

import struct

def boxes(data, start, end):
    at = start
    while at + 8 <= end:
        size, kind = struct.unpack(">I4s", data[at:at + 8])
        head = 8
        if size == 1:
            size, head = struct.unpack(">Q", data[at + 8:at + 16])[0], 16
        if size < head or at + size > end:
            return
        yield kind, at + head, at + size
        at += size

def find(data, path, start=0, end=None):
    end = len(data) if end is None else end
    for kind, s, e in boxes(data, start, end):
        if kind == path[0]:
            return (s, e) if len(path) == 1 else find(data, path[1:], s, e)
    return None

def bump(data, at, width, delta):
    fmt = ">Q" if width == 8 else ">I"
    v = struct.unpack(fmt, data[at:at + width])[0]
    data[at:at + width] = struct.pack(fmt, (v + delta) & ((1 << (8 * width)) - 1))

def trun_fields(data):
    """Positions in the first trun: (count, first sample's fields by name)."""
    t = find(data, [b"moof", b"traf", b"trun"])
    if not t:
        return None
    flags = int.from_bytes(data[t[0] + 1:t[0] + 4], "big")
    at = t[0] + 8 + (4 if flags & 1 else 0)
    fields = {}
    if flags & 4:
        fields["first_flags"] = at
        at += 4
    for bit, name in ((0x100, "duration"), (0x200, "size"), (0x400, "flags"), (0x800, "offset")):
        if flags & bit:
            fields[name] = at
            at += 4
    return fields

def tfhd_fields(data):
    t = find(data, [b"moof", b"traf", b"tfhd"])
    if not t:
        return {}
    flags = int.from_bytes(data[t[0] + 1:t[0] + 4], "big")
    at = t[0] + 8 + (8 if flags & 1 else 0) + (4 if flags & 2 else 0)
    fields = {}
    for bit, name in ((8, "duration"), (0x10, "size"), (0x20, "flags")):
        if flags & bit:
            fields[name] = at
            at += 4
    return fields

def media_segment(data, name):
    if name == "tfdt +1":
        t = find(data, [b"moof", b"traf", b"tfdt"])
        if not t:
            return False
        bump(data, t[0] + 4, 8 if data[t[0]] else 4, 1)
        return True
    run, head = trun_fields(data) or {}, tfhd_fields(data)
    if name in ("sample duration +1", "sample size +1"):
        key = name.split()[1]
        at = run.get(key) or head.get(key)
        if at is None:
            return False
        bump(data, at, 4, 1)
        return True
    if name == "composition offset +512":
        if "offset" not in run:
            return False
        bump(data, run["offset"], 4, 512)
        return True
    if name == "sync flag cleared":
        at = run.get("first_flags") or run.get("flags") or head.get("flags")
        if at is None:
            return False
        value = struct.unpack(">I", data[at:at + 4])[0]
        if value & 0x10000:
            return False
        data[at:at + 4] = struct.pack(">I", value | 0x10000)
        return True
    if name == "sample byte flipped":
        m = find(data, [b"mdat"])
        if not m or m[1] - m[0] < 32:
            return False
        data[m[0] + 16] ^= 0x5A
        return True
    return False

def init_segment(data, name):
    if name == "init mdhd timescale +1":
        m = find(data, [b"moov", b"trak", b"mdia", b"mdhd"])
        if not m:
            return False
        bump(data, m[0] + (20 if data[m[0]] else 12), 4, 1)
        return True
    if name in ("init width +16", "init channels +1"):
        st = find(data, [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"])
        if not st:
            return False
        entry = next(boxes(data, st[0] + 8, st[1]), None)
        if not entry:
            return False
        at = entry[1] + (24 if name == "init width +16" else 16)
        data[at:at + 2] = struct.pack(">H", (struct.unpack(">H", data[at:at + 2])[0] + (16 if "width" in name else 1)) & 0xFFFF)
        return True
    return False

def hls(package, rendition, name):
    playlist = os.path.join(package, f"{rendition}.m3u8")
    if not os.path.exists(playlist):
        return False
    lines = open(playlist).read().split("\n")
    media = [l for l in lines if l and not l.startswith("#")]
    init = next((re.search(r'URI="([^"]+)"', l).group(1) for l in lines if l.startswith("#EXT-X-MAP")), None)
    if name == "extinf +0.1 s":
        for i, l in enumerate(lines):
            if l.startswith("#EXTINF:"):
                value, rest = l[8:].split(",", 1)
                digits = len(value.split(".")[1]) if "." in value else 0
                lines[i] = f"#EXTINF:{float(value) + 0.1:.{digits or 3}f},{rest}"
                open(playlist, "w").write("\n".join(lines))
                return True
        return False
    target = os.path.join(package, init) if name.startswith("init") and init else (
        os.path.join(package, media[0]) if media else None)
    if not target or not os.path.exists(target):
        return False
    data = bytearray(open(target, "rb").read())
    ok = init_segment(data, name) if name.startswith("init") else media_segment(data, name)
    if ok:
        open(target, "wb").write(bytes(data))
    return ok

def scale_time(t, factor):
    if isinstance(t, dict) and "ticks" in t:
        t["ticks"]["num"] = t["ticks"]["num"] * factor[0] // factor[1]
        t["seconds"] = t["ticks"]["num"] / t["ticks"]["den"] / t["timescale"]

def probe_doc(doc, name):
    streams = [s for s in doc.get("streams") or [] if isinstance(s, dict)]
    video = next((s for s in streams if s.get("kind") == "video" and s.get("video")), None)
    audio = next((s for s in streams if s.get("kind") == "audio" and s.get("audio")), None)
    if name == "duration +25%":
        if not doc.get("duration"):
            return False
        for t in [doc.get("duration"), doc.get("presentation_duration")] + [s.get("duration") for s in streams]:
            scale_time(t, (5, 4))
        return True
    if name == "video width +16" and video:
        video["video"]["width"] += 16
        return True
    if name == "bit rate x2":
        hit = False
        for st in streams:
            b = st.get("bit_rate")
            if isinstance(b, dict) and b.get("bits_per_second"):
                b["bits_per_second"] *= 2
                b["exact"]["num"] *= 2
                hit = True
        return hit
    if name == "video codec renamed" and video:
        video["codec"] = "hevc" if video.get("codec") != "hevc" else "h264"
        return True
    if name == "audio channels +1" and audio and audio["audio"].get("channels"):
        audio["audio"]["channels"] += 1
        return True
    if name == "audio rate x2" and audio and audio["audio"].get("sample_rate"):
        audio["audio"]["sample_rate"] *= 2
        return True
    if name == "last stream left out" and streams:
        doc["streams"] = doc["streams"][:-1]
        return True
    return False

changed = False
if done.returncode == 0 and mutation and argv:
    kind, _, name = mutation.partition(":")
    if kind == "vtt" and argv[0] == "extract-subtitles":
        path = argv[-1]
        text = open(path, encoding="utf-8").read()
        new = vtt(text, name)
        if new is not None:
            data = new.encode()
            open(path, "wb").write(data)
            summary = json.loads(out)
            summary["output"]["bytes"] = len(data)
            summary["cues"] = new.count("-->")
            out = (json.dumps(summary, indent=2) + "\n").encode()
            changed = True
    elif kind == "policy" and argv[0] == "probe" and "--scan-frame-sizes" in argv:
        doc = json.loads(out)
        if probe_doc(doc, name):
            out = (json.dumps(doc, indent=2) + "\n").encode()
            changed = True
    elif kind in ("hls-video", "hls-audio") and argv[:2] == ["hls", "package"]:
        changed = hls(argv[-1], kind.split("-")[1], name)
    elif kind == "chapters" and argv[0] == "chapters":
        doc = json.loads(out)
        if chapters(doc, name):
            out = (json.dumps(doc, indent=2) + "\n").encode()
            changed = True
if mutation.startswith("policy:") and argv and argv[0] == "check" and done.returncode in (0, 1):
    # The verdict a CoreAV that measured the mutated facts would give: the
    # policy evaluated on the mutated probe document.
    sys.path.insert(0, os.environ["COREAV_SCRIPTS"])
    import policy_ref
    path, policy_file = argv[-1], argv[argv.index("--policy") + 1]
    probed = subprocess.run([real, "probe", "--scan-fragments", "--scan-frame-sizes", path], capture_output=True)
    if probed.returncode == 0:
        doc = json.loads(probed.stdout)
        if probe_doc(doc, mutation.partition(":")[2]):
            reasons = policy_ref.evaluate(policy_ref.load_policy(open(policy_file).read()), doc)
            verdict = {"schema": "coreav.verdict/1", "verdict": "reject" if reasons else "accept",
                       "reasons": reasons}
            out = (json.dumps(verdict) + "\n").encode()
            done = subprocess.CompletedProcess(done.args, 1 if reasons else 0, out, done.stderr)
            changed = True
if changed and os.environ.get("COREAV_CHANGED"):
    with open(os.environ["COREAV_CHANGED"], "a") as log:
        log.write(" ".join(argv) + "\n")
sys.stdout.buffer.write(out)
sys.stderr.buffer.write(done.stderr)
sys.exit(done.returncode)
'''

POLICY_MUTATIONS = ["policy:" + n for n in (
    "duration +25%", "video width +16", "bit rate x2", "video codec renamed", "audio channels +1",
    "audio rate x2", "last stream left out")]

HLS_MUTATIONS = [f"hls-{r}:{n}" for r in ("video", "audio") for n in (
    "tfdt +1", "sample duration +1", "sample size +1", "composition offset +512", "sync flag cleared",
    "sample byte flipped", "extinf +0.1 s", "init mdhd timescale +1",
    "init width +16" if r == "video" else "init channels +1")]

TEXT_MUTATIONS = ["vtt:" + n for n in (
    "cue start +1 ms", "cue end +1 ms", "last cue end -1 ms", "cue letter changed", "first cue left out",
    "first cue twice", "escape undone", "style tag dropped", "line break joined")] + ["chapters:" + n for n in (
        "chapter start +1 tick", "chapter end +1 tick", "chapter title changed", "last chapter left out",
        "titles swapped")]


def run_text(argv, kds):
    """text_check's report for argv, with or without its known divergences."""
    import text_check
    saved = text_check.known_divergences
    if not kds:
        text_check.known_divergences = lambda: []
    try:
        with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
            report = f.name
        old = sys.argv
        sys.argv = ["text_check.py", *argv, "--report", report]
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                text_check.main()
            finally:
                sys.argv = old
        data = json.loads(Path(report).read_text())
        os.unlink(report)
    finally:
        text_check.known_divergences = saved
    items = {}
    for r in data["subtitles"]:
        items[(r["file"], f"stream {r['stream_index']}")] = (r["problems"], r["covered"])
    for r in data["chapters"]:
        items[(r["file"], "chapters")] = (r["problems"], r["covered"])
    return items


def run_hls(argv, kds):
    """hls_check's report for argv, with or without its known divergences
    (without them, every coverage counts as a failure)."""
    import hls_check
    saved = hls_check.Report.covered
    if not kds:
        hls_check.Report.covered = lambda self, kd, what: self.fail(f"(off) {kd}: {what}")
    try:
        with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
            report = f.name
        old = sys.argv
        sys.argv = ["hls_check.py", *argv, "--report", report, "--jobs", "4"]
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                hls_check.main()
            except SystemExit:
                pass
            finally:
                sys.argv = old
        data = json.loads(Path(report).read_text())
        os.unlink(report)
    finally:
        hls_check.Report.covered = saved
    return {(r["file"], "package"): (list(r.get("failures") or []),
                                     [(d["id"], d["detail"]) for d in r.get("divergences") or []])
            for r in data["results"] if r.get("status") != "refused"}


def run_policy(argv, kds):
    """policy_check's records for argv, keyed by (file, policy): its failures
    and an uncovered oracle difference as problems, the entries covering one
    as covered; with or without its known divergences."""
    import policy_check
    import policy_ref
    saved = policy_ref.covered
    if not kds:
        policy_ref.covered = lambda *a, **k: []
    try:
        with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
            report = f.name
        old = sys.argv
        sys.argv = ["policy_check.py", *argv, "--report", report]
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                policy_check.main()
            except SystemExit:
                pass
            finally:
                sys.argv = old
        data = json.loads(Path(report).read_text())
        os.unlink(report)
    finally:
        policy_ref.covered = saved
    items = {}
    for r in data["records"]:
        problems = [json.dumps(f, sort_keys=True) if not isinstance(f, str) else f for f in r.get("failures") or []]
        covered = [(kd, json.dumps(r.get("oracle_diff"), sort_keys=True)) for kd in r.get("oracle_covered_by") or []]
        if r.get("oracle") is False and not covered:
            problems.append("oracle: " + json.dumps(r.get("oracle_diff"), sort_keys=True))
        items[(r["file"], r["policy"])] = (problems, covered)
    return items


def changed_items(log):
    """The items whose output the wrapper changed, from its log of argv."""
    found = set()
    if not Path(log).exists():
        return found
    for line in Path(log).read_text().splitlines():
        parts = line.split()
        if parts[0] == "extract-subtitles":
            stream = parts[parts.index("--stream") + 1]
            found.add((Path(parts[-2]).name, f"stream {stream}"))
        elif parts[0] == "chapters":
            found.add((Path(parts[-1]).name, "chapters"))
        elif parts[:2] == ["hls", "package"]:
            found.add((Path(parts[-2]).name, "package"))
        elif parts[0] == "check":
            found.add((Path(parts[-1]).name, Path(parts[parts.index("--policy") + 1]).stem))
    return found


def verdict(base, after):
    (p, c), (p2, c2) = base, after
    if Counter(p2) - Counter(p):
        return "caught"
    if Counter(c2) - Counter(c):
        return "COVERED"
    if Counter(p2) != Counter(p) or Counter(c2) != Counter(c):
        return "agrees"
    return "no effect"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--oracle", required=True, choices=("text", "hls", "policy"))
    parser.add_argument("--policy", action="append", default=[], help="policy files (--oracle policy)")
    parser.add_argument("--coreav", required=True)
    parser.add_argument("--media", required=True)
    parser.add_argument("--mediainfo")
    parser.add_argument("--report")
    args = parser.parse_args()
    work = Path(tempfile.mkdtemp(prefix="writer-kd-audit-cli-"))
    atexit.register(shutil.rmtree, work, True)
    wrapper = work / "coreav"
    wrapper.write_text(WRAPPER)
    wrapper.chmod(wrapper.stat().st_mode | stat.S_IXUSR)
    os.environ["COREAV_REAL"] = str(Path(args.coreav).resolve())
    argv = ["--coreav", str(wrapper), "--media", args.media]
    if args.mediainfo:
        argv += ["--mediainfo", args.mediainfo]
    os.environ["COREAV_SCRIPTS"] = str(ROOT / "scripts")
    if args.oracle == "policy":
        argv = ["--coreav", str(wrapper), "--media", args.media, "--ffprobe", "ffprobe", "--oracle-frames"]
        for policy in args.policy:
            argv += ["--policy", policy]
    run_text_or_hls = {"text": run_text, "hls": run_hls, "policy": run_policy}[args.oracle]
    mutations = {"text": TEXT_MUTATIONS, "hls": HLS_MUTATIONS, "policy": POLICY_MUTATIONS}[args.oracle]
    os.environ.pop("COREAV_MUTATION", None)
    base, base_off = run_text_or_hls(argv, True), None
    results = []
    for mutation in mutations:
        log = work / "changed.log"
        log.unlink(missing_ok=True)
        os.environ["COREAV_MUTATION"], os.environ["COREAV_CHANGED"] = mutation, str(log)
        after = run_text_or_hls(argv, True)
        changed = changed_items(log)
        off = None
        for item in sorted(set(base) | set(after)):
            record = {"item": list(item), "mutation": mutation}
            if item not in changed:
                record["verdict"] = "n/a"
                results.append(record)
                continue
            v = verdict(base.get(item, ([], [])), after.get(item, ([], [])))
            if v in ("no effect", "agrees"):
                if base_off is None:
                    os.environ.pop("COREAV_MUTATION", None)
                    base_off = run_text_or_hls(argv, False)
                    os.environ["COREAV_MUTATION"] = mutation
                if off is None:
                    off = run_text_or_hls(argv, False)
                hidden = Counter(off.get(item, ([], []))[0]) - Counter(base_off.get(item, ([], []))[0])
                if hidden:
                    v = "ABSORBED"
                    record["hidden"] = sorted(hidden)[:3]
            elif v == "COVERED":
                record["new_covered"] = sorted(set(after[item][1]) - set(base.get(item, ([], []))[1]))
            record["verdict"] = v
            results.append(record)
    os.environ.pop("COREAV_MUTATION", None)
    counts = Counter(r["verdict"] for r in results)
    print(f"writer_kd_audit_cli {args.oracle}: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    for r in results:
        if r["verdict"] in ("COVERED", "ABSORBED"):
            print(f"{r['verdict']} {r['item'][0]} {r['item'][1]}: {r['mutation']} "
                  f"{r.get('new_covered') or r.get('hidden')}")
    if args.report:
        # The entries each item's unmutated output already uses: what the
        # audit inputs reach.
        baseline = {" | ".join(item): sorted(set(re.findall(r"KD-[A-Z0-9-]+[A-Z0-9]", json.dumps(covered))))
                    for item, (_, covered) in sorted(base.items())}
        Path(args.report).write_text(json.dumps({"oracle": args.oracle, "counts": counts, "baseline": baseline,
                                                 "results": results}, indent=1) + "\n")
    if not counts.get("caught"):
        # Nothing caught means nothing was tested (the wrapper or the oracle
        # did not run): fail rather than pass.
        print("writer_kd_audit_cli: no change was caught", file=sys.stderr)
        return 1
    return 1 if counts.get("COVERED") or counts.get("ABSORBED") else 0


if __name__ == "__main__":
    sys.exit(main())
