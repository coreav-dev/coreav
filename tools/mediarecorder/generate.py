#!/usr/bin/env python3
"""Generate real Chrome MediaRecorder WebM/Matroska files for parser tests.

Serves record.html on 127.0.0.1, opens it in headless Chrome once per
configuration in MATRIX, receives the recorded bytes by POST, writes them to
--out, probes each file with ffprobe and writes --out/manifest.json.

Standard library only. Run with: python3 -I tools/mediarecorder/generate.py --out DIR
"""

import argparse
import hashlib
import http.server
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent

VP8 = "video/webm;codecs=vp8,opus"
VP9 = "video/webm;codecs=vp9,opus"
AV1 = "video/webm;codecs=av1,opus"
H264 = "video/webm;codecs=h264,opus"
MKV_AVC = "video/x-matroska;codecs=avc1,opus"
WEBM = "video/webm"
OPUS = "audio/webm;codecs=opus"
AWEBM = "audio/webm"


def cfg(name, mime, w=320, h=240, fps=30, dur=3000, slice="none", audio=1, video=1, **extra):
    """One recording. dur is wall-clock ms between start() and stop()."""
    return dict(name=name, mime=mime, w=w, h=h, fps=fps, dur=dur, slice=slice,
                audio=audio, video=video, **extra)


# Fixed matrix. Optional keys: vbps, abps, pause ("AT:LEN,..." ms), resize ("WxH@AT").
MATRIX = [
    # VP8 + Opus
    cfg("vp8_opus_320x240_30_3s", VP8),
    cfg("vp8_opus_160x120_15_2s", VP8, 160, 120, 15, 2000),
    cfg("vp8_opus_640x480_30_5s_s1000", VP8, 640, 480, dur=5000, slice=1000),
    cfg("vp8_opus_1280x720_30_5s_s100", VP8, 1280, 720, dur=5000, slice=100),
    cfg("vp8_opus_360x640_30_4s_s1000", VP8, 360, 640, dur=4000, slice=1000),
    cfg("vp8_opus_320x240_30_6s_pause", VP8, dur=6000, pause="2000:1000"),
    cfg("vp8_opus_resize_320x240_to_640x360", VP8, dur=5000, slice=1000, resize="640x360@2000"),
    cfg("vp8_opus_640x480_30_5s_lowbps", VP8, 640, 480, dur=5000, vbps=100_000),
    cfg("vp8_opus_320x240_15_20s_s1000", VP8, fps=15, dur=20000, slice=1000),
    # VP9 + Opus
    cfg("vp9_opus_320x240_30_3s", VP9),
    cfg("vp9_opus_640x360_60_5s_s100", VP9, 640, 360, 60, 5000, slice=100),
    cfg("vp9_opus_1280x720_30_10s_s1000", VP9, 1280, 720, dur=10000, slice=1000),
    cfg("vp9_opus_360x640_30_4s", VP9, 360, 640, dur=4000),
    cfg("vp9_opus_320x240_30_8s_pause2_s1000", VP9, dur=8000, slice=1000, pause="2000:1000,5000:1000"),
    cfg("vp9_opus_resize_640x360_to_320x240", VP9, 640, 360, dur=5000, resize="320x240@2000"),
    cfg("vp9_opus_1280x720_30_5s_highbps", VP9, 1280, 720, dur=5000, vbps=8_000_000),
    cfg("vp9_videoonly_640x480_30_4s", "video/webm;codecs=vp9", 640, 480, dur=4000, audio=0),
    # AV1 + Opus
    cfg("av1_opus_320x240_30_3s", AV1),
    cfg("av1_opus_640x360_30_5s_s1000", AV1, 640, 360, dur=5000, slice=1000),
    cfg("av1_opus_1280x720_30_4s_s100", AV1, 1280, 720, dur=4000, slice=100),
    cfg("av1_opus_320x240_30_6s_pause", AV1, dur=6000, pause="2000:1000"),
    # H.264 + Opus in WebM
    cfg("h264_opus_webm_320x240_30_3s", H264),
    cfg("h264_opus_webm_1280x720_60_5s_s1000", H264, 1280, 720, 60, 5000, slice=1000),
    cfg("h264_opus_webm_360x640_30_4s", H264, 360, 640, dur=4000),
    cfg("h264_opus_webm_resize_320x240_to_640x480", H264, dur=5000, slice=1000, resize="640x480@2000"),
    # H.264 + Opus in Matroska
    cfg("avc1_opus_mkv_320x240_30_3s", MKV_AVC),
    cfg("avc1_opus_mkv_640x480_30_5s_s1000", MKV_AVC, 640, 480, dur=5000, slice=1000),
    cfg("avc1_opus_mkv_1280x720_60_5s_s100", MKV_AVC, 1280, 720, 60, 5000, slice=100),
    cfg("avc1_opus_mkv_320x240_30_6s_pause", MKV_AVC, dur=6000, pause="2000:1000"),
    cfg("avc1_videoonly_mkv_640x360_30_4s", "video/x-matroska;codecs=avc1", 640, 360, dur=4000, audio=0),
    # Browser default codecs
    cfg("default_webm_320x240_30_3s", WEBM),
    cfg("default_webm_640x480_30_5s_s1000", WEBM, 640, 480, dur=5000, slice=1000),
    cfg("default_webm_videoonly_320x240_15_3s", WEBM, fps=15, audio=0),
    # Audio only
    cfg("opus_audio_5s", OPUS, dur=5000, video=0),
    cfg("opus_audio_10s_s100", OPUS, dur=10000, slice=100, video=0),
    cfg("opus_audio_6s_pause", OPUS, dur=6000, pause="2000:1000", video=0),
    cfg("default_audio_webm_3s", AWEBM, video=0),
    cfg("default_audio_webm_20s_s1000_32k", AWEBM, dur=20000, slice=1000, video=0, abps=32_000),
    # Extra codec mappings, recorded only if supported
    cfg("pcm_audio_3s", "audio/webm;codecs=pcm", video=0),
    cfg("vp8_pcm_320x240_30_3s", "video/webm;codecs=vp8,pcm"),
    cfg("vp9_opus_mkv_320x240_30_3s", "video/x-matroska;codecs=vp9,opus"),
]


class Results:
    """Results posted back by record.html, keyed by recording id."""

    def __init__(self):
        self.cond = threading.Condition()
        self.items = {}

    def put(self, key, value):
        with self.cond:
            self.items[key] = value
            self.cond.notify_all()

    def wait(self, key, timeout):
        with self.cond:
            self.cond.wait_for(lambda: key in self.items, timeout)
            return self.items.pop(key, None)


def make_handler(results):
    page = (HERE / "record.html").read_bytes()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            if urllib.parse.urlsplit(self.path).path != "/record.html":
                return self.send_error(404)
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(page)))
            self.end_headers()
            self.wfile.write(page)

        def do_POST(self):
            url = urllib.parse.urlsplit(self.path)
            if url.path != "/result":
                return self.send_error(404)
            params = dict(urllib.parse.parse_qsl(url.query))
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            self.send_response(204)
            self.end_headers()
            results.put(params.get("id", ""), (params, body))

    return Handler


def chrome_version(chrome):
    return subprocess.run([chrome, "--version"], capture_output=True, text=True).stdout.strip()


def run_chrome(chrome, url, profile, log):
    args = [
        chrome, "--headless=new", f"--user-data-dir={profile}",
        "--no-first-run", "--no-default-browser-check", "--disable-gpu",
        "--autoplay-policy=no-user-gesture-required", "--use-fake-ui-for-media-stream",
        "--disable-background-timer-throttling", "--disable-renderer-backgrounding",
        "--disable-backgrounding-occluded-windows", "--window-size=1400,900", url,
    ]
    return subprocess.Popen(args, stdout=log, stderr=log, start_new_session=True)


def kill(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(5)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()
    except ProcessLookupError:
        pass


def ffprobe(path):
    p = subprocess.run(
        ["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", str(path)],
        capture_output=True, text=True)
    info = {"exit": p.returncode, "stderr": p.stderr.strip()[:2000], "codecs": []}
    try:
        data = json.loads(p.stdout or "{}")
    except json.JSONDecodeError:
        return info
    info["codecs"] = [s.get("codec_name") for s in data.get("streams", [])]
    info["streams"] = [
        {k: s[k] for k in ("codec_type", "codec_name", "width", "height", "sample_rate", "channels")
         if k in s} for s in data.get("streams", [])]
    fmt = data.get("format", {})
    info["format_name"] = fmt.get("format_name")
    info["duration"] = fmt.get("duration")
    return info


def ebml_summary(data):
    """DocType and whether the Segment size is the 8-byte unknown-size marker.

    A byte search, not a parser: enough to label what Chrome wrote.
    """
    head = data[:256]
    i = head.find(b"\x42\x82")  # DocType
    doctype = None
    if 0 <= i and i + 3 < len(head):
        n = head[i + 2] & 0x7F  # one-byte size, as Chrome writes it
        doctype = head[i + 3:i + 3 + n].decode("ascii", "replace")
    j = head.find(b"\x18\x53\x80\x67")  # Segment
    unknown = head[j + 4:j + 12] == b"\x01" + b"\xff" * 7 if j >= 0 else None
    return {"docType": doctype, "segmentSizeUnknown": unknown}


def record(c, port, chrome, results, tmp, out, version):
    params = {k: v for k, v in c.items() if k != "name"}
    query = urllib.parse.urlencode({"id": c["name"], **params})
    url = f"http://127.0.0.1:{port}/record.html?{query}"
    ext = ".mkv" if c["mime"].startswith("video/x-matroska") else ".webm"
    entry = {"name": c["name"], "file": None, "mimeType": c["mime"], "chrome": version,
             "params": params, "skipped": False, "reason": None}
    profile = Path(tempfile.mkdtemp(dir=tmp, prefix="profile-"))
    with open(tmp / f"{c['name']}.chrome.log", "wb") as log:
        proc = run_chrome(chrome, url, profile, log)
        try:
            got = results.wait(c["name"], c["dur"] / 1000 + 45)
        finally:
            kill(proc)
    shutil.rmtree(profile, ignore_errors=True)

    if got is None:
        entry.update(skipped=True, reason="timeout waiting for upload")
        return entry
    meta, body = got
    if meta.get("status") != "ok":
        entry.update(skipped=True, reason=f"{meta.get('status')}: {body.decode(errors='replace')}")
        return entry
    path = out / (c["name"] + ext)
    path.write_bytes(body)
    entry.update(file=path.name, size=len(body), sha256=hashlib.sha256(body).hexdigest(),
                 recorderMimeType=meta.get("actual"), chunks=int(meta.get("chunks", 0)),
                 **ebml_summary(body), ffprobe=ffprobe(path))
    return entry


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--chrome", default="google-chrome")
    ap.add_argument("--only", help="comma-separated config names to run")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    matrix = MATRIX
    if args.only:
        wanted = set(args.only.split(","))
        matrix = [c for c in MATRIX if c["name"] in wanted]

    version = chrome_version(args.chrome)
    results = Results()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), make_handler(results))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]

    entries = []
    with tempfile.TemporaryDirectory(prefix="mediarecorder-") as tmp:
        for i, c in enumerate(matrix, 1):
            e = record(c, port, args.chrome, results, Path(tmp), args.out, version)
            status = f"skipped ({e['reason']})" if e["skipped"] else \
                f"{e['size']} bytes, ffprobe exit {e['ffprobe']['exit']} {e['ffprobe']['codecs']}"
            print(f"[{i}/{len(matrix)}] {c['name']}: {status}", flush=True)
            entries.append(e)
    server.shutdown()

    manifest = {"chrome": version, "count": sum(not e["skipped"] for e in entries),
                "skipped": sum(e["skipped"] for e in entries), "entries": entries}
    (args.out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"{manifest['count']} recorded, {manifest['skipped']} skipped -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
