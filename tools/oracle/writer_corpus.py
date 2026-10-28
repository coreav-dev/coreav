#!/usr/bin/env python3
"""The writer oracles on the real corpus, locally (#324).

For each real ISOBMFF file of corpus v1, one worker downloads it from R2
(SHA-256 checked), runs every writer through the `coreav` command and
judges each output with that writer's oracle:

- clip: four clips (head, middle, tail, whole,
  with D the video's duration in whole milliseconds), the default audio, or
  none when the default audio is refused (amendment point 7);
- split: every 10 s (`docs/design/split.md`, "Corpus bars"), decoded only
  for files of at most 500 MB, as the split corpus run did;
- remux, sanitize (the default keep) and extract-audio (the default
  stream);
- HLS: `scripts/hls_check.py`'s `check_one` with 6 s segments, as the HLS
  corpus run did.

Each oracle runs in its own process, from a scripts directory: this
checkout's, and with `--control DIR` (the `scripts/` of a checkout of
`main`), `main`'s too on every output this checkout's oracle fails, on the
same output. `--control-share` also runs it on that share of the passing
outputs, chosen by SHA-256, to show they pass there too. `--boxes-only`
runs only `scripts/writer_boxes.py`, the box checks, which need no
ffprobe or ffmpeg.

A writer that exits with none of the command's codes (a panic, a signal)
is counted as crashed, apart from its refusals. One record per file in
OUT/files/ (local, never committed); a stopped run resumes. The download and the outputs are deleted after each file. The
summary prints and writes counts only.

  writer_corpus.py run --out DIR --work DIR [--workers N] [--disk-budget GB] [--writers clip,split,...]
                       [--control DIR] [--control-share F] [--boxes-only] [--only SHA,...] [--limit N]
                       [--coreav PATH] [--bin DIR]
  writer_corpus.py summarize --out DIR
  writer_corpus.py audit-inputs --out RUN_DIR --work DIR
  writer_corpus.py materialize --out DIR --only SHA,... [--writers W,...] [--coreav PATH]
"""
import argparse
import concurrent.futures
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import time
import traceback
from fractions import Fraction

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(ROOT / "tools" / "stage3-report"))
sys.path.insert(0, str(ROOT / "scripts"))

import corpus  # noqa: E402
import track_boxes  # noqa: E402

WRITERS = ("clip", "split", "remux", "sanitize", "extract", "hls")
WRITER_TIMEOUT = 3600
ORACLE_TIMEOUT = 7200
SPLIT_DECODE_LIMIT = 500_000_000


def run(command, timeout=WRITER_TIMEOUT):
    return subprocess.run(command, capture_output=True, timeout=timeout, env=corpus.child_env(), check=False)


def ratio(value):
    """{'num', 'den'} of a summary time: its ticks over its timescale."""
    exact = Fraction(value["ticks"]["num"], value["ticks"]["den"] * value["timescale"])
    return {"num": exact.numerator, "den": exact.denominator}


def crashed(proc):
    """A writer that did not exit with one of the command's codes (README,
    "Exit codes": 1 to 3): a panic (101) or a signal."""
    return proc.returncode < 0 or proc.returncode > 3


def refused(proc):
    if crashed(proc):
        return {"status": "crashed", "exit": proc.returncode,
                "reason": proc.stderr.decode(errors="replace")[-300:]}
    try:
        error = json.loads(proc.stderr or b"{}")
    except ValueError:
        error = {}
    return {"status": "refused", "exit": proc.returncode,
            "reason": (error.get("message") or proc.stderr.decode(errors="replace")[-300:])[:300]}


def kept_track(src_movie, stream):
    """A manifest's kept track from a summary stream: its first sample's
    byte position and decode time read from the source's own boxes."""
    track = src_movie["tracks"][stream["stream_index"]]
    samples, first = track["samples"], stream["first_sample"]
    return {"stream_index": stream["stream_index"], "first_sample": first, "samples": stream["samples"],
            "timescale": track["timescale"], "first_pos": samples["offsets"][first] if first < len(
                samples["offsets"]) else None, "first_dts": sum(samples["durations"][:first])}


def clip_positions(coreav, src):
    """The four clips, (label, start, end or None)
    in seconds."""
    proc = run([coreav, "probe", str(src)])
    if proc.returncode != 0:
        return None
    probe = json.loads(proc.stdout)
    video = next((s for s in probe["streams"] if s.get("kind") == "video" and s.get("duration")), None)
    if video is None:
        return None
    d = int(Fraction(video["duration"]["ticks"]["num"] * 1000,
                     video["duration"]["ticks"]["den"] * video["duration"]["timescale"]))
    clamp = lambda v: min(max(v, 0), max(d, 0))  # noqa: E731
    seen, clips = [], []
    for label, start, end in (("head", 0, 10_000), ("middle", d // 2 - 5_000, d // 2 + 5_000),
                              ("tail", d - 10_000, None), ("whole", 0, None)):
        key = (clamp(start), clamp(end if end is not None else d))
        if key in seen:
            continue
        seen.append(key)
        # Ends are requested as stated (clip-bench's positions clamp them
        # only to tell clips apart).
        clips.append((label, Fraction(clamp(start), 1000), None if end is None else Fraction(end, 1000)))
    return clips


def make_clips(coreav, src, out_dir, src_movie):
    outputs = []
    positions = clip_positions(coreav, src)
    if positions is None:
        return [{"label": "clip", "status": "refused", "reason": "no video duration"}]
    for label, start, end in positions:
        output = out_dir / f"clip-{label}.mp4"
        command = [coreav, "clip", "--start", str(float(start)), str(src), str(output)]
        if end is not None:
            command[4:4] = ["--end", str(float(end))]
        proc = run(command)
        no_audio = False
        if proc.returncode != 0 and b"AAC" in proc.stderr:
            no_audio = True
            proc = run(command[:2] + ["--audio", "none"] + command[2:])
        if proc.returncode != 0:
            outputs.append(dict(refused(proc), label=label))
            continue
        summary = json.loads(proc.stdout)
        streams = {s["kind"]: s for s in summary["streams"]}
        requested = summary["end"].get("requested")
        entry = {"source": str(src), "output": str(output), "label": label,
                 "end": {"num": requested["num"], "den": requested["den"]} if requested else None,
                 "snapped_start": ratio(summary["start"]["snapped"]), "written_end": ratio(summary["end"]["written"]),
                 "video": kept_track(src_movie, streams["video"]),
                 "audio": kept_track(src_movie, streams["audio"]) if "audio" in streams else None}
        outputs.append({"label": label, "status": "written", "no_audio": no_audio, "entry": entry})
    return outputs


def make_split(coreav, src, out_dir, size):
    command = [coreav, "split", "--every", "10", str(src), str(out_dir / "seg-%04d.mp4")]
    proc = run(command)
    no_audio = False
    if proc.returncode != 0 and b"AAC" in proc.stderr:
        no_audio = True
        proc = run(command[:2] + ["--audio", "none"] + command[2:])
    if proc.returncode != 0:
        return [dict(refused(proc), label="every10")]
    summary = json.loads(proc.stdout)
    first = summary["segments"][0]["streams"]
    video = next(s["stream_index"] for s in first if s["kind"] == "video")
    audio = next((s["stream_index"] for seg in summary["segments"] for s in seg["streams"] if s["kind"] == "audio"),
                 None)
    segments = []
    for seg in summary["segments"]:
        streams = {s["kind"]: s for s in seg["streams"]}
        segments.append({"output": seg["output"]["path"], "start": ratio(seg["start"]),
                         "video_start": ratio(seg["video_start"]), "end": ratio(seg["end"]),
                         "video_first": streams["video"]["first_sample"], "video_samples": streams["video"]["samples"],
                         "audio_first": streams["audio"]["first_sample"] if "audio" in streams else 0,
                         "audio_samples": streams["audio"]["samples"] if "audio" in streams else 0})
    entry = {"source": str(src), "label": "every10", "video_stream": video, "audio_stream": audio,
             "no_audio": no_audio, "segments": segments}
    return [{"label": "every10", "status": "written", "no_audio": no_audio, "entry": entry,
             "decode": size <= SPLIT_DECODE_LIMIT}]


def make_copy(coreav, src, out_dir, writer):
    output = out_dir / f"{writer}.{'m4a' if writer == 'extract' else 'mp4'}"
    command = {"remux": [coreav, "remux"], "sanitize": [coreav, "sanitize"],
               "extract": [coreav, "extract-audio"]}[writer] + [str(src), str(output)]
    proc = run(command)
    if proc.returncode != 0:
        return [dict(refused(proc), label=writer)]
    summary = json.loads(proc.stdout)
    if writer == "extract":
        entry = {"source": str(src), "stream_index": summary["stream"]["stream_index"], "output": str(output)}
    else:
        dropped = [d["stream_index"] if isinstance(d, dict) else d for d in summary.get("dropped_streams") or []]
        entry = {"source": str(src), "output": str(output), "dropped_streams": dropped,
                 "streams": [{"stream_index": s["stream_index"], "track_id": s["track_id"],
                              "source_track_id": s.get("source_track_id")} for s in summary["streams"]],
                 "keep": None if writer == "remux" else ["rotation", "language", "gapless", "kinds"]}
    return [{"label": writer, "status": "written", "entry": entry}]


# The oracle, in a child process, from a given scripts directory.

def oracle(scripts, writer, item, tools, boxes_only):
    """Runs `writer`'s oracle on one output in a child process: (problems,
    covered count, packets, seconds) or raises on a harness failure."""
    request = {"scripts": str(scripts), "writer": writer, "item": item, "tools": tools, "boxes_only": boxes_only}
    started = time.perf_counter()
    # The oracles keep decoded audio in temporary files that outlive their
    # process; each child gets its own directory, removed when it ends.
    with tempfile.TemporaryDirectory(prefix="writer-oracle-") as scratch:
        env = dict(corpus.child_env(), TMPDIR=scratch)
        proc = subprocess.run([sys.executable, "-I", str(pathlib.Path(__file__).resolve()), "oracle"],
                              input=json.dumps(request).encode(), capture_output=True, timeout=ORACLE_TIMEOUT,
                              env=env, check=False)
    if proc.returncode != 0:
        raise RuntimeError(f"oracle exit {proc.returncode}: {proc.stderr.decode(errors='replace')[-600:]}")
    result = json.loads(proc.stdout.decode().strip().splitlines()[-1])
    result["seconds"] = time.perf_counter() - started
    return result


def cmd_oracle():
    """The child: reads a request on standard input, prints the result."""
    request = json.loads(sys.stdin.read())
    sys.path.insert(0, request["scripts"])
    writer, item, tools = request["writer"], request["item"], request["tools"]
    entry = item.get("entry")
    ffprobe, ffmpeg, mediainfo = tools["ffprobe"], tools["ffmpeg"], tools["mediainfo"]
    if request["boxes_only"]:
        import clip_check as cc
        import writer_boxes as wb

        def several(src, out, si, oi):
            a, b = cc.full_edit_list(src, si), cc.full_edit_list(out, oi)
            return any(e is not None and cc.several_edits(e[2]) for e in (a, b))
        if writer == "clip":
            sev = several(entry["source"], entry["output"], entry["video"]["stream_index"], 0) or bool(
                entry.get("audio") and several(entry["source"], entry["output"], entry["audio"]["stream_index"], 1))
            found = wb.check_clip(entry, sev)
        elif writer == "split":
            found = []
            for n, seg in enumerate(entry["segments"]):
                sev = several(entry["source"], seg["output"], entry["video_stream"], 0) or bool(
                    entry.get("audio_stream") is not None and seg["audio_samples"]
                    and several(entry["source"], seg["output"], entry["audio_stream"], 1))
                found += [(f, f"segment {n}: {m}") for f, m in wb.check_segment(entry, seg, sev)]
        elif writer in ("remux", "sanitize"):
            found = wb.check_copy(entry["source"], entry["output"], [s["stream_index"] for s in entry["streams"]],
                                  writer)
        elif writer == "extract":
            found = wb.check_copy(entry["source"], entry["output"], [entry["stream_index"]], "extract")
        else:
            found = hls_boxes(item, tools)
            if found is None:
                print(json.dumps({"status": "refused", "problems": [], "covered": 0, "packets": None}))
                return 0
        print(json.dumps({"problems": [m for _, m in found], "fields": sorted({f for f, _ in found}),
                          "covered": 0, "packets": None}))
        return 0
    if writer == "clip":
        import clip_check
        problems, _info, covered, counts = clip_check.check(entry, ffprobe, ffmpeg, mediainfo)
    elif writer == "split":
        import split_check
        problems, _info, covered, counts = split_check.check(entry, ffprobe, ffmpeg, mediainfo,
                                                             decode=item.get("decode", True))
    elif writer in ("remux", "sanitize"):
        import remux_check
        found, covered, counts = remux_check.check(entry, ffprobe, ffmpeg, mediainfo)
        problems = [p["message"] for p in found]
    elif writer == "extract":
        import extract_check
        problems, _info, covered = extract_check.check(entry, ffprobe, ffmpeg, mediainfo)
        counts = None
    else:
        import types
        import hls_check

        def run_hls(cmd, check=True):
            proc = subprocess.run(cmd, capture_output=True, timeout=ORACLE_TIMEOUT)
            if check and proc.returncode != 0:
                raise RuntimeError(f"{cmd[0]} failed ({proc.returncode}): "
                                   f"{proc.stderr.decode(errors='replace')[-800:]}")
            return proc
        hls_check.run = run_hls
        args = types.SimpleNamespace(coreav=tools["coreav"], segment_duration="6", segments="sample",
                                     work=item["work"], keep=False, mediainfo=mediainfo, mediainfo_source={},
                                     master_decode=False, retry_without_audio=True)
        result = hls_check.check_one(args, pathlib.Path(item["source"]))
        print(json.dumps({"status": result.get("status"), "problems": result.get("failures") or [],
                          "covered": len(result.get("divergences") or []),
                          "packets": {"agree": result.get("packets_agree", 0),
                                      "disagree": result.get("packets_disagree", 0)},
                          "reason": result.get("message")}))
        return 0
    print(json.dumps({"problems": problems, "covered": len(covered), "packets": counts}))
    return 0


def hls_boxes(item, tools):
    """The HLS box checks alone: package as check_one does, then check each
    rendition's boxes."""
    import hls_check
    import writer_boxes as wb
    work = pathlib.Path(item["work"]) / "hls-boxes"
    shutil.rmtree(work, ignore_errors=True)
    command = [tools["coreav"], "hls", "package", "--segment-duration", "6", item["source"], str(work)]
    proc = subprocess.run(command, capture_output=True, check=False, timeout=WRITER_TIMEOUT)
    if proc.returncode != 0 and b"only AAC audio" in proc.stderr:
        shutil.rmtree(work, ignore_errors=True)
        proc = subprocess.run(command[:3] + ["--audio", "none"] + command[3:], capture_output=True, check=False,
                              timeout=WRITER_TIMEOUT)
    if proc.returncode != 0:
        return [("box:crash", f"hls package exited {proc.returncode}")] if crashed(proc) else None
    found = []
    renditions = json.loads(proc.stdout)["renditions"]
    for r in renditions:
        init, segments = hls_check.media_playlist(work / f"{r['rendition']}.m3u8")
        found += [(f, f"{r['rendition']}: {m}") for f, m in wb.check_rendition(
            item["source"], r["stream_index"], str(work / init), [str(work / n) for _, n in segments])]
    shutil.rmtree(work, ignore_errors=True)
    return found


# The run.

def chosen_for_control(sha, share):
    return share > 0 and int(hashlib.sha256(f"control:{sha}".encode()).hexdigest()[:8], 16) < share * (1 << 32)


def check_file(entry, args):
    sha = entry["sha256"]
    work = pathlib.Path(args.work) / sha
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    src = work / f"source.{entry['ext']}"
    record = {k: entry.get(k) for k in ("sha256", "size", "ext", "holdout", "tags")}
    record["outputs"] = []
    started = time.perf_counter()
    tools = {"ffprobe": f"{args.bin}/ffprobe", "ffmpeg": f"{args.bin}/ffmpeg", "mediainfo": f"{args.bin}/mediainfo",
             "coreav": args.coreav}
    try:
        if not corpus.download(entry, src):
            record["error"] = "download or SHA-256 check failed"
            return record
        try:
            src_movie = track_boxes.read_movie(str(src))
        except (track_boxes.Unreadable, OSError, IndexError, ValueError) as error:
            src_movie = None
            record["source_unreadable"] = str(error)[:200]
        for writer in args.writers:
            out_dir = work / writer
            out_dir.mkdir()
            try:
                if writer == "clip":
                    items = make_clips(args.coreav, src, out_dir, src_movie) if src_movie else \
                        [{"label": "clip", "status": "skipped", "reason": "source boxes unreadable"}]
                elif writer == "split":
                    items = make_split(args.coreav, src, out_dir, entry["size"])
                elif writer == "hls":
                    items = [{"label": "hls", "status": "written", "source": str(src), "work": str(out_dir)}]
                else:
                    items = make_copy(args.coreav, src, out_dir, writer)
            except Exception as error:  # noqa: BLE001
                items = [{"label": writer, "status": "error", "error": f"writer: {type(error).__name__}: {error}"[:400]}]
            for item in items:
                result = {"writer": writer, "label": item["label"], "status": item["status"],
                          "no_audio": item.get("no_audio", False)}
                for key in ("reason", "exit", "error"):
                    if key in item:
                        result[key] = item[key]
                if item["status"] == "written":
                    try:
                        mine = oracle(ROOT / "scripts", writer, item, tools, args.boxes_only)
                        result["oracle"] = mine
                        if writer == "hls" and mine.get("status") == "refused":
                            result["status"] = "refused"
                            result["reason"] = (mine.get("reason") or "")[:300]
                    except Exception as error:  # noqa: BLE001
                        result["harness"] = f"{type(error).__name__}: {error}"[:800]
                    mine = result.get("oracle")
                    if args.control and mine and result["status"] == "written" and (
                            mine["problems"] or chosen_for_control(sha, args.control_share)):
                        try:
                            result["control"] = oracle(pathlib.Path(args.control), writer, item, tools, False)
                        except Exception as error:  # noqa: BLE001
                            result["control_harness"] = f"{type(error).__name__}: {error}"[:800]
                record["outputs"].append(result)
            shutil.rmtree(out_dir, ignore_errors=True)
    except Exception as error:  # noqa: BLE001
        record["error"] = f"{type(error).__name__}: {str(error)[:500]}"
        record["traceback"] = traceback.format_exc()[-2000:]
    finally:
        record["seconds"] = time.perf_counter() - started
        shutil.rmtree(work, ignore_errors=True)
    return record


def write_json(path, value):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(value, indent=1) + "\n")
    tmp.replace(path)


def cmd_run(args):
    files = corpus.entries("isobmff")
    if args.only:
        wanted = tuple(args.only.split(","))
        files = [f for f in files if f["sha256"].startswith(wanted)]
    out = pathlib.Path(args.out)
    (out / "files").mkdir(parents=True, exist_ok=True)
    todo = [f for f in files if not (out / "files" / f"{f['sha256']}.json").exists()]
    todo.sort(key=lambda f: (-f["size"], f["sha256"]))
    if args.limit:
        todo = todo[-args.limit:]
    write_json(out / "run.json", {"writers": args.writers, "boxes_only": args.boxes_only,
                                  "control": bool(args.control), "control_share": args.control_share,
                                  "files": len(files)})
    # A file needs about three times its size on disk at once (the
    # download, an output, and a split's segments): files start largest
    # first while their total stays within the budget.
    budget = args.disk_budget * 1e9
    done, running, pending = 0, {}, list(todo)
    with concurrent.futures.ProcessPoolExecutor(args.workers) as pool:
        while pending or running:
            used = sum(3 * f["size"] for f in running.values())
            while pending and len(running) < args.workers:
                fits = next((f for f in pending if used + 3 * f["size"] <= budget), None)
                if fits is None and not running:
                    fits = pending[0]
                if fits is None:
                    break
                pending.remove(fits)
                running[pool.submit(check_file, fits, args)] = fits
                used += 3 * fits["size"]
            finished, _ = concurrent.futures.wait(running, return_when=concurrent.futures.FIRST_COMPLETED)
            for future in finished:
                f = running.pop(future)
                try:
                    record = future.result()
                except Exception as error:  # noqa: BLE001
                    record = {"sha256": f["sha256"], "error": f"worker: {type(error).__name__}: {error}"}
                write_json(out / "files" / f"{f['sha256']}.json", record)
                done += 1
                if done % 25 == 0:
                    print(f"writer_corpus: {done} of {len(todo)} files", flush=True)
    return summarize(out)


def summarize(out):
    """Counts per writer: outputs written, refused, crashed, passing and failing
    with this checkout's oracle, and of the failing ones how many fail with
    `main`'s too (the control), by problem kind."""
    records = [json.loads(p.read_text()) for p in sorted((pathlib.Path(out) / "files").glob("*.json"))]
    summary = {"files": len(records), "errors": sum(1 for r in records if r.get("error")), "writers": {}}
    for r in records:
        for o in r.get("outputs", []):
            w = summary["writers"].setdefault(o["writer"], {
                "outputs": 0, "refused": 0, "crashed": 0, "written": 0, "pass": 0, "fail": 0, "harness": 0, "covered": 0,
                "control_run": 0, "control_fail": 0, "fail_both": 0, "fail_new": 0, "control_pass_checked": 0,
                "control_pass_fail": 0, "new_fields": {}, "packets_agree": 0, "packets_disagree": 0})
            w["outputs"] += 1
            if o["status"] == "refused":
                w["refused"] += 1
                continue
            if o["status"] == "crashed":
                w["crashed"] += 1
                continue
            if o["status"] != "written":
                w["harness"] += 1
                continue
            w["written"] += 1
            if "harness" in o or "oracle" not in o:
                w["harness"] += 1
                continue
            mine = o["oracle"]
            w["covered"] += 1 if mine.get("covered") else 0
            packets = mine.get("packets") or {}
            w["packets_agree"] += packets.get("agree", 0)
            w["packets_disagree"] += packets.get("disagree", 0)
            control = o.get("control")
            if mine["problems"]:
                w["fail"] += 1
                if control is not None:
                    w["control_run"] += 1
                    if control["problems"]:
                        w["control_fail"] += 1
                        w["fail_both"] += 1
                    new = [p for p in mine["problems"] if p not in control["problems"]]
                    if new:
                        w["fail_new"] += 1
                        for p in new:
                            # The check's words, without numbers or a split's segment.
                            kind = re.sub(r"^segment \d+: ", "", p)
                            kind = re.sub(r"\d+", "N", kind.split(":")[0] if ":" in kind else kind)[:60]
                            w["new_fields"][kind] = w["new_fields"].get(kind, 0) + 1
            else:
                w["pass"] += 1
                if control is not None:
                    w["control_pass_checked"] += 1
                    w["control_pass_fail"] += 1 if control["problems"] else 0
    write_json(pathlib.Path(out) / "summary.json", summary)
    print(json.dumps(summary, indent=1))
    return 0


# The mutation audit's corpus inputs (docs/design/writer-oracle-boxes.md,
# pass criterion 2) and their outputs.

def edit_after_first(src_movie, kind):
    """Whether the first track of `kind` has one media edit that leaves its
    first sample out: for video, an edit starting after the first sample's
    composition time; for audio, after its end."""
    handler = {"video": "vide", "audio": "soun"}[kind]
    track = next((t for t in src_movie["tracks"] if t["handler"] == handler), None)
    if track is None or not track["edits"]:
        return False
    media = [m for _, m, _ in track["edits"] if m != -1]
    samples = track["samples"]
    if len(media) != 1 or not len(samples["sizes"]):
        return False
    first = samples["cto"][0] if kind == "video" else samples["durations"][0]
    return media[0] > first if kind == "video" else media[0] >= first


def cmd_audit_inputs(args):
    """The inputs of pass criterion 2, from a corpus run's records: per
    writer, the 6 smallest files it wrote with audio and the 4 smallest
    without (clip, split, HLS), or the 10 smallest it wrote; for remux the 2
    smallest whose video edit leaves its first sample out, for extraction
    the 2 smallest whose audio edit does."""
    records = [json.loads(p.read_text()) for p in sorted((pathlib.Path(args.out) / "files").glob("*.json"))]
    sizes = {r["sha256"]: r["size"] for r in records if r.get("size")}
    written = {w: sorted({r["sha256"] for r in records for o in r.get("outputs", [])
                          if o["writer"] == w and o["status"] == "written" and o.get("oracle")
                          and not o.get("harness")}, key=lambda s: (sizes[s], s)) for w in WRITERS}
    by_sha = {f["sha256"]: f for f in corpus.entries("isobmff")}
    chosen = {}
    work = pathlib.Path(args.work)
    work.mkdir(parents=True, exist_ok=True)

    def source_movie(sha):
        src = work / f"{sha}.{by_sha[sha]['ext']}"
        if not src.exists() and not corpus.download(by_sha[sha], src):
            return None, None
        try:
            return src, track_boxes.read_movie(str(src))
        except (track_boxes.Unreadable, OSError, IndexError, ValueError):
            return src, None

    for writer in WRITERS:
        if writer in ("remux", "sanitize", "extract"):
            chosen[writer] = written[writer][:10]
        else:
            with_audio, without = [], []
            for sha in written[writer]:
                if len(with_audio) >= 6 and len(without) >= 4:
                    break
                src, movie = source_movie(sha)
                if movie is None:
                    continue
                audio = any(t["handler"] == "soun" and (t.get("entry") or {}).get("type") == "mp4a"
                            for t in movie["tracks"])
                (with_audio if audio else without).append(sha)
                if src is not None and sha not in sum(chosen.values(), []):
                    src.unlink(missing_ok=True)
            chosen[writer] = with_audio[:6] + without[:4]
    for writer, kind in (("remux", "video"), ("extract", "audio")):
        found = []
        for sha in written[writer]:
            if len(found) >= 2:
                break
            if "elst" not in by_sha[sha].get("tags", ()):
                continue
            src, movie = source_movie(sha)
            if movie is not None and edit_after_first(movie, kind):
                found.append(sha)
            if src is not None:
                src.unlink(missing_ok=True)
        chosen[f"{writer}-edit"] = found
    write_json(pathlib.Path(args.out) / "audit-inputs.json", chosen)
    print(json.dumps({k: len(v) for k, v in chosen.items()}))
    return 0


def cmd_materialize(args):
    """Writes the outputs of `--only` files for the mutation audit: per
    writer a directory with the outputs and a manifest in the form the
    fixture tests write (tools/oracle/writer_kd_audit.py reads it), and for
    HLS a directory of the sources (writer_kd_audit_cli.py packages them)."""
    out = pathlib.Path(args.out)
    by_sha = {f["sha256"]: f for f in corpus.entries("isobmff")}
    manifests = {w: [] for w in WRITERS if w != "hls"}
    (out / "media").mkdir(parents=True, exist_ok=True)
    for sha in args.only.split(","):
        entry = by_sha[sha]
        src = out / "media" / f"{sha[:12]}.{entry['ext']}"
        if not src.exists() and not corpus.download(entry, src):
            raise SystemExit(f"download failed: {sha[:12]}")
        movie = track_boxes.read_movie(str(src))
        for writer in args.writers:
            if writer == "hls":
                continue
            out_dir = out / writer / sha[:12]
            shutil.rmtree(out_dir, ignore_errors=True)
            out_dir.mkdir(parents=True)
            if writer == "clip":
                items = make_clips(args.coreav, src, out_dir, movie)
            elif writer == "split":
                items = make_split(args.coreav, src, out_dir, entry["size"])
            else:
                items = make_copy(args.coreav, src, out_dir, writer)
            manifests[writer] += [i["entry"] for i in items if i["status"] == "written"]
    for writer, entries in manifests.items():
        if writer in args.writers:
            write_json(out / writer / "manifest.json", entries)
            print(f"{writer}: {len(entries)} outputs")
    return 0


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "oracle":
        return cmd_oracle()
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("run")
    p.add_argument("--out", required=True)
    p.add_argument("--work", required=True)
    p.add_argument("--workers", type=int, default=4)
    p.add_argument("--disk-budget", type=float, default=24, help="GB of disk the running files may use")
    p.add_argument("--writers", default=",".join(WRITERS))
    p.add_argument("--control")
    p.add_argument("--control-share", type=float, default=0.0)
    p.add_argument("--boxes-only", action="store_true")
    p.add_argument("--only")
    p.add_argument("--limit", type=int, default=0)
    p.add_argument("--coreav", default=str(ROOT / "target" / "release" / "coreav"))
    p.add_argument("--bin", default="/opt/coreav-oracle-bin")
    s = sub.add_parser("summarize")
    s.add_argument("--out", required=True)
    a = sub.add_parser("audit-inputs")
    a.add_argument("--out", required=True, help="a corpus run's directory")
    a.add_argument("--work", required=True)
    m = sub.add_parser("materialize")
    m.add_argument("--out", required=True)
    m.add_argument("--only", required=True, help="full SHA-256s, comma separated")
    m.add_argument("--writers", default=",".join(WRITERS))
    m.add_argument("--coreav", default=str(ROOT / "target" / "release" / "coreav"))
    args = parser.parse_args()
    if args.command == "summarize":
        return summarize(args.out)
    if args.command == "audit-inputs":
        return cmd_audit_inputs(args)
    if args.command == "materialize":
        args.writers = [w for w in args.writers.split(",") if w]
        return cmd_materialize(args)
    args.writers = [w for w in args.writers.split(",") if w]
    unknown = set(args.writers) - set(WRITERS)
    if unknown:
        parser.error(f"unknown writers {sorted(unknown)}")
    return cmd_run(args)


if __name__ == "__main__":
    sys.exit(main())
