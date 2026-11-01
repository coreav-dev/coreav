#!/usr/bin/env python3
"""The CI benchmark guard (docs/design/benchmarks.md): the number of
instructions the `coreav` command executes for each case beyond its
start-up (`coreav --version`, counted in the same run), counted by
Valgrind's cachegrind with the cache simulation off, against the stored
baseline.

usage: bench_guard.py --coreav PATH --fixtures DIR
                      [--baseline bench/guard/baseline.json] [--write COUNTS.json]

Each case runs one command on one fixture of scripts/gen-fixtures.sh, or on
a Matroska, WebM or transport stream copy of one that
`ffmpeg -fflags +bitexact` writes
(scripts/gen-fixtures-mkv.sh writes random segment IDs, so its files differ
from run to run). Outputs go to standard output, which goes to /dev/null;
`split` and `hls package`, which write files, write into a new directory
per run under the guard's work directory, removed at the end.
`join` joins inputs the guard writes first from the case's fixture with
the same binary (INPUTS, not counted), into such a directory too.
A case fails when its count exceeds the baseline's by more than the
baseline's tolerance, unless the work moved out of start-up (as when a
subcommand's options are built only when it runs): start-up fell in the
same run, and the case's total, the whole command with its start-up, is
no more than the baseline's start-up plus the case's count. The total is
reported beside the split for every case of the shipped build. A case whose fixture bytes differ from the
baseline's is still compared and the difference is reported: the pinned
FFmpeg writes some fixtures differently on different runner CPUs, which
moves the counts far less than the tolerance. A case whose fixture is
missing fails if the baseline has it. Start-up is guarded as a case too.
The rustc and Valgrind versions are recorded and a difference from the
baseline's is reported. Exits 1 when a case is slower beyond the
tolerance, or a case or command could not run; without a baseline file it
only reports. --write saves the counts in the baseline's format, for a
pull request that updates it.

URL cases run a command on a fixture served by a local HTTP server
that answers range requests, and count what the read plan
(crates/coreav-range) asks for: the requests and the bytes received, as
`COREAV_REMOTE_STATS` reports them, and what the server sees: the requests
it received, and the rounds they came in. Each response's head is held
back RESPONSE_DELAY; a round starts with a request that arrives while no
other head is pending, so requests the plan starts together form one
round and a request that waits for an earlier response starts another. A
change that adds requests, round trips or bytes shows in them. A URL case
fails with more requests or rounds than the baseline's, or more bytes
beyond a tolerance; when its fixture's bytes differ from the baseline's
(another CPU's FFmpeg), with URL_SLACK more requests and rounds and
URL_BYTES_TOLERANCE_OTHER more bytes. Fewer never fails. Two of its
fixtures, large enough to need the plan's pipelines and with audio chunks
megabytes apart, are encoded here with the pinned FFmpeg (GENERATED); one
that cannot be encoded fails its cases.
"""
import argparse
import hashlib
import http.server
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
POLICY = ROOT / "docs/schema/policy-examples/web-upload.json"

# name: (fixture, command arguments with {} for the fixture's path,
# {index} for the HLS index of it that the guard builds first, {inputs:SET}
# for the files of an INPUTS set the guard writes first from it, and {out}
# for a new empty directory, for the commands that write files rather than
# standard output)
CASES = {
    "probe-mp4": ("basic.mp4", ["probe", "{}"]),
    "probe-fragmented": ("fragmented_many.mp4", ["probe", "{}"]),
    "probe-mkv": ("basic.mkv", ["probe", "{}"]),
    "probe-webm": ("vp9.webm", ["probe", "{}"]),
    # Transport streams: the head and tail the probe reads, and the whole
    # file through the packet listing (docs/design/mpegts.md).
    "probe-ts": ("basic.ts", ["probe", "{}"]),
    "probe-packets-ts": ("basic.ts", ["probe", "--scan-frame-sizes", "{}"]),
    # `coreav ffprobe` (docs/design/compat-cli.md, B6): the stream and
    # format sections in JSON, and the packet listing in the default writer.
    "ffprobe-mp4": ("fragmented_many.mp4",
                    ["ffprobe", "-v", "error", "-of", "json", "-show_format", "-show_streams", "{}"]),
    "ffprobe-mkv": ("basic.mkv",
                    ["ffprobe", "-v", "error", "-of", "json", "-show_format", "-show_streams", "{}"]),
    "ffprobe-ts": ("basic.ts",
                   ["ffprobe", "-v", "error", "-of", "json", "-show_format", "-show_streams", "{}"]),
    "ffprobe-packets-mp4": ("basic.mp4", ["ffprobe", "-v", "error", "-show_packets", "{}"]),
    "ffprobe-packets-mkv": ("basic.mkv", ["ffprobe", "-v", "error", "-show_packets", "{}"]),
    "ffprobe-packets-ts": ("basic.ts", ["ffprobe", "-v", "error", "-show_packets", "{}"]),
    # A payload that is no frame of the syntax its table names: the
    # probe searches every one of them whole.
    "probe-ts-no-frames": ("ts_no_frames.ts", ["probe", "{}"]),
    # Access units that wait for a picture period the stream never
    # shows: what the listing holds, and what it costs to hold it.
    "probe-packets-ts-no-period": ("ts_no_period.ts", ["probe", "--scan-frame-sizes", "{}"]),
    # Every frame header (and, for Matroska, the block scan).
    "probe-frame-sizes-mp4": ("basic.mp4", ["probe", "--scan-frame-sizes", "{}"]),
    "probe-frame-sizes-mkv": ("basic.mkv", ["probe", "--scan-frame-sizes", "{}"]),
    "remux-mov": ("basic.mov", ["remux", "{}", "-"]),
    "remux-fragmented": ("fragmented_many.mp4", ["remux", "{}", "-"]),
    "remux-mkv": ("basic.mkv", ["remux", "{}", "-"]),
    "remux-webm": ("vp9.webm", ["remux", "{}", "-"]),
    "clip": ("basic.mp4", ["clip", "--start", "0.5", "--end", "1.5", "{}", "-"]),
    # Across the splice at 3 s: two output edits (docs/design/edit-list-writers.md).
    "clip-multi-edit": ("edit_multi_splice.mp4", ["clip", "--start", "0.5", "--end", "4", "{}", "-"]),
    "extract-audio": ("basic.mp4", ["extract-audio", "{}", "-"]),
    # docs/design/audio-decode.md: FLAC and MP3 in Matroska and MP4, and Opus
    # in WebM, to WAV.
    # One thread: counts that do not move with the runner's CPUs.
    "decode-audio-flac-mkv": ("flac.mka", ["decode-audio", "--threads", "1", "{}", "-"]),
    "decode-audio-flac-mp4": ("flac.m4a", ["decode-audio", "--threads", "1", "{}", "-"]),
    "decode-audio-mp3-mkv": ("mp3.mka", ["decode-audio", "--threads", "1", "{}", "-"]),
    "decode-audio-mp3-mp4": ("mp3.m4a", ["decode-audio", "--threads", "1", "{}", "-"]),
    "decode-audio-opus": ("opus.webm", ["decode-audio", "--threads", "1", "{}", "-"]),
    # AAC LC (docs/design/audio-decode.md, "AAC-LC", bar 6): opt-in, so run
    # with --decoder-coreav, whose build has the feature `aac`
    # (DECODER_CASES); basic.mp4's own AAC stream, copied.
    "decode-audio-aac-mkv": ("aac.mka", ["decode-audio", "--threads", "1", "{}", "-"]),
    "decode-audio-aac-mp4": ("aac.m4a", ["decode-audio", "--threads", "1", "{}", "-"]),
    # The fixtures' mono 44.1 kHz audio resampled to 16 kHz, as for
    # transcription, and to 48 kHz stereo ("Resampling and downmixing").
    "decode-audio-16k-mono": ("mp3.mka", ["decode-audio", "--threads", "1", "--rate", "16000", "--channels", "1", "{}", "-"]),
    "decode-audio-resample": ("flac.mka", ["decode-audio", "--threads", "1", "--rate", "48000", "--channels", "2", "{}", "-"]),
    "sanitize": ("basic.mp4", ["sanitize", "{}", "-"]),
    "check": ("basic.mp4", ["check", "--policy", str(POLICY), "{}"]),
    # The Matroska path: the structure audit and the block scan.
    "check-mkv": ("basic.mkv", ["check", "--policy", str(POLICY), "{}"]),
    "split": ("basic.mp4", ["split", "--every", "1", "{}", "{out}/seg-%d.mp4"]),
    # Several edits: the segments around the splice at 3 s carry the
    # windows in their range (docs/design/edit-list-writers.md).
    "split-multi-edit": ("edit_multi_splice.mp4", ["split", "--every", "1", "{}", "{out}/seg-%d.mp4"]),
    "extract-subtitles": ("text_styled.mp4", ["extract-subtitles", "{}", "-"]),
    "chapters": ("chapters.mp4", ["chapters", "{}"]),
    "hls-index": ("hls_h264.mp4", ["hls", "index", "{}", "-"]),
    "hls-segment": ("hls_h264.mp4", ["hls", "get", "{index}", "{}", "video-1.m4s", "-"]),
    "hls-package": ("hls_h264.mp4", ["hls", "package", "{}", "{out}/hls"]),
    # A split's segments joined back into one edit (docs/design/join.md).
    "join": ("basic.mp4", ["join", "-o", "-", "{inputs:segments}"]),
    "join-fragmented": ("fragmented_many.mp4", ["join", "-o", "-", "{inputs:segments}"]),
    # Two clips that keep their own edits: one output edit per input.
    "join-clips": ("basic.mp4", ["join", "-o", "-", "{inputs:clips}"]),
    # docs/design/thumbnails.md: the coreav process only (probe, frame
    # choice, the frame copied out of the worker, resampling, conversion and
    # encoding); the worker and FFmpeg are other processes, not counted.
    # Run with --decoder-coreav (DECODER_CASES).
    "thumbnail": ("basic.mp4", ["thumbnail", "{}", "--width", "160", "-o", "-"]),
    # docs/design/transcode.md, bar B8: the coreav process only (demux,
    # frames from the worker, writes to the encoder, the Annex B reader,
    # the MP4 writer); the worker, its FFmpeg and the encoder (the ffmpeg
    # on PATH, the pinned one) are other processes, not counted. One
    # encoder thread: an encoded stream, and so the reader's work, that
    # does not move with the runner's CPUs. `--scale` to half the size, so
    # that the resize (Picture::resize_with) is counted too. Run with
    # --decoder-coreav.
    "transcode": ("basic.mp4", ["transcode", "{}", "{out}/transcoded.mp4", "--encoder", "ffmpeg",
                                "--preset", "ultrafast", "--threads", "1", "--scale", "160:-2"]),
}

# Inputs the guard writes from a case's fixture with the same binary, not
# counted: set: [command arguments with {} for the fixture's path and {dir}
# for the set's directory]. The set's files are the directory's, in the
# order of the numbers in their names.
INPUTS = {
    "segments": [["split", "--every", "1", "{}", "{dir}/seg-%d.mp4"]],
    "clips": [["clip", "--start", "0.3", "--end", "1.1", "{}", "{dir}/clip-1.mp4"],
              ["clip", "--start", "1.1", "--end", "1.9", "{}", "{dir}/clip-2.mp4"]],
}

# Fixtures written here: name: (fixture it is written from, format, how):
# by stream copy, or with its audio encoded to FLAC, MP3 or Opus (the encoders
# are deterministic, and `+bitexact` keeps version strings out).
COPY = ("-map", "0", "-c", "copy")
FLAC = ("-map", "0:a", "-c:a", "flac")
MP3 = ("-map", "0:a", "-c:a", "libmp3lame", "-b:a", "128k")
OPUS = ("-map", "0:a", "-c:a", "libopus", "-b:a", "64k")
AUDIO_COPY = ("-map", "0:a", "-c", "copy")
DERIVED = {
    "basic.mkv": ("basic.mp4", "matroska", COPY),
    "vp9.webm": ("vp9.mp4", "webm", COPY),
    "basic.ts": ("basic.mp4", "mpegts", COPY),
    "flac.mka": ("basic.mp4", "matroska", FLAC),
    "flac.m4a": ("basic.mp4", "mp4", FLAC),
    "mp3.mka": ("basic.mp4", "matroska", MP3),
    "mp3.m4a": ("basic.mp4", "mp4", MP3),
    "opus.webm": ("basic.mp4", "webm", OPUS),
    "aac.mka": ("basic.mp4", "matroska", AUDIO_COPY),
    "aac.m4a": ("basic.mp4", "mp4", AUDIO_COPY),
}

# Cases that need a decoder the shipped (default) build does not have: run
# with --decoder-coreav, a build with the FFmpeg fallback and the opt-in AAC
# decoder, and counted beyond that build's own start-up.
DECODER_CASES = {"thumbnail", "transcode", "decode-audio-aac-mkv", "decode-audio-aac-mp4"}

# URL cases: name: (fixture, command arguments with {url} for the fixture's
# URL and {dir} for a fresh output directory).
URL_CASES = {
    # The movie box at the end of 80 MB: the start, the tail, the tables.
    "url-probe": ("large_av.mp4", ["probe", "{url}"]),
    "url-probe-front": ("large_av_faststart.mp4", ["probe", "{url}"]),
    "url-probe-mkv": ("basic.mkv", ["probe", "{url}"]),
    # The audio of an interleaved file: what the copy pass's advice asks for.
    "url-extract-audio": ("large_av.mp4", ["extract-audio", "{url}", "-"]),
    "url-clip": ("large_av.mp4", ["clip", "--start", "20", "--end", "26", "{url}", "-"]),
    "url-remux": ("large_av.mp4", ["remux", "{url}", "-"]),
    "url-sanitize": ("large_av.mp4", ["sanitize", "{url}", "-"]),
    "url-split": ("large_av.mp4", ["split", "--every", "10", "{url}", "{dir}/%03d.mp4"]),
    "url-check": ("large_av.mp4", ["check", "--policy", str(POLICY), "{url}"]),
    "url-extract-subtitles": ("text_styled.mp4", ["extract-subtitles", "{url}", "-"]),
    "url-chapters": ("chapters.mp4", ["chapters", "{url}"]),
}

# Fixtures encoded here for the URL cases: 48 s of 720p H.264 at about
# 13 Mbit/s with AAC audio in chunks of 2 s, so that the audio chunks lie
# about 3 MB apart; one with the movie box first. Single-threaded and
# bitexact, so that the pinned FFmpeg writes the same bytes on every run.
LARGE_ENCODE = ["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=25:duration=48",
                "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=48",
                "-c:v", "libx264", "-preset", "ultrafast", "-qp", "4", "-g", "50", "-threads", "1",
                "-c:a", "aac", "-b:a", "128k", "-chunk_duration", "2000000",
                "-fflags", "+bitexact", "-flags", "+bitexact", "-map_metadata", "-1"]
BITEXACT = ["-threads", "1", "-fflags", "+bitexact", "-flags", "+bitexact", "-f", "mpegts"]
GENERATED = {
    "large_av.mp4": LARGE_ENCODE,
    "large_av_faststart.mp4": LARGE_ENCODE + ["-movflags", "+faststart"],
    # The sources of the two crafted transport streams below: 40 s of
    # sound, and 4,000 pictures, each a stream of one PID so that the
    # case counts the work its own finding is about.
    "ts_sound.ts": ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=40",
                    "-c:a", "aac", "-b:a", "128k", *BITEXACT],
    "ts_pictures.ts": ["-f", "lavfi", "-i", "testsrc2=size=32x32:rate=100:duration=40",
                       "-c:v", "libx264", "-preset", "ultrafast", "-g", "100", *BITEXACT],
}

# Transport streams `scripts/ts_craft.py` writes from a GENERATED one,
# for the two costs a reader pays on a file that is not what it says it
# is: name: (source, operation and its arguments).
CRAFTED = {
    # Every PES payload of the sound is bytes that are no frame header,
    # so the probe searches each one whole and learns nothing from any.
    "ts_no_frames.ts": ("ts_sound.ts", ["scramble", "0x100"]),
    # Every PES packet of the pictures states one time, so the stream
    # shows no picture period and its access units wait for one.
    "ts_no_period.ts": ("ts_pictures.ts", ["freeze", "0x100"]),
}

# Every fixture this script writes rather than reads from --fixtures.
WRITTEN_HERE = frozenset(DERIVED) | frozenset(GENERATED) | frozenset(CRAFTED)

# A URL case fails with any request or round more than the baseline's, or
# more than URL_BYTES_TOLERANCE more bytes (runner runs agreed on requests
# and within 0.12% on bytes). When the fixture's bytes differ from the
# baseline's (the pinned FFmpeg writes some fixtures differently on another
# CPU), a boundary may move: then URL_SLACK more requests and rounds and
# URL_BYTES_TOLERANCE_OTHER more bytes.
URL_BYTES_TOLERANCE = 0.02
URL_SLACK = 1
URL_BYTES_TOLERANCE_OTHER = 0.10
# Cases whose requests are started one after another by a pipeline that
# follows the responses, so that scheduling splits or merges rounds from run
# to run while requests and bytes stay the same: `url-extract-audio` read 2
# to 5 rounds over 14 runs of the same build, 16 requests and 33,049,730
# bytes every time (idle and under load). They may take this many more rounds
# than the baseline's (the largest count seen plus one); requests and bytes keep their bounds.
URL_ROUNDS_SLACK = {"url-extract-audio": 4}
# Each response's head is held back this long, so that requests the plan
# starts together (the tail with the first request, a pipeline's segments)
# arrive while the first is pending, and count as one round; long enough
# that a runner's scheduling stall does not split one burst into two.
RESPONSE_DELAY = 0.1
URL_TIMEOUT = 120

STATS = re.compile(r'^\{"remote_stats":(\{.*\})\}$', re.MULTILINE)

REFS = re.compile(r"^==\d+== I\s+refs:\s+([\d,]+)\s*$", re.MULTILINE)


def instructions(coreav, args):
    """The instructions `coreav ARGS` executes, or the error. With a fallback
    that names no program, so that a command that stopped being native fails
    here instead of being handed to FFmpeg; not in strict-safe mode, which
    would also refuse the FFmpeg decoder and encoder of the thumbnail and
    transcode cases (docs/design/compat-cli.md, "Strict-safe mode")."""
    done = subprocess.run(
        ["valgrind", "--tool=cachegrind", "--cache-sim=no", "--cachegrind-out-file=/dev/null",
         coreav, *args],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, timeout=600,
        env={**{k: v for k, v in os.environ.items() if k != "COREAV_STRICT_SAFE"},
             "COREAV_FALLBACK": "/nonexistent/coreav-fallback"})
    if done.returncode != 0:
        return None, f"exit {done.returncode}: {done.stderr[-500:]}"
    match = REFS.search(done.stderr)
    if not match:
        return None, "no instruction count in Valgrind's output"
    return int(match.group(1).replace(",", "")), None


def argument(template, path, index, out):
    """One argument of a case: the fixture's path for `{}`, the HLS index
    for `{index}`, and the case's output directory in place of a leading
    `{out}`."""
    if template == "{}":
        return str(path)
    if template == "{index}":
        return str(index)
    if template.startswith("{out}"):
        return out + template[len("{out}"):]
    return template


def derive(fixtures, work):
    """Writes the DERIVED fixtures into `work`; a fixture
    whose source is missing or that ffmpeg cannot write is left out."""
    for name, (source, form, how) in DERIVED.items():
        if (fixtures / source).is_file():
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(fixtures / source), *how,
                            "-fflags", "+bitexact", "-f", form, str(work / name)],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120, check=False)


def craft(work):
    """Writes the CRAFTED fixtures into `work` from the GENERATED ones;
    returns why each one that could not be written failed."""
    errors = {}
    for name, (source, operation) in CRAFTED.items():
        if not (work / source).is_file():
            errors[name] = f"its source {source} was not written"
            continue
        done = subprocess.run([sys.executable, "-I", str(ROOT / "scripts" / "ts_craft.py"),
                               *operation, str(work / source), str(work / name)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
                              timeout=300, check=False)
        if done.returncode != 0 or not (work / name).is_file():
            errors[name] = f"ts_craft.py exit {done.returncode}: {done.stderr[-300:]}"
            (work / name).unlink(missing_ok=True)
    return errors


def generate(work):
    """Encodes the GENERATED fixtures into `work`; returns why each one
    ffmpeg could not write failed (its cases fail)."""
    errors = {}
    for name, encode in GENERATED.items():
        try:
            done = subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", *encode, str(work / name)],
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, timeout=600,
                                  check=False)
        except subprocess.TimeoutExpired:
            errors[name] = "ffmpeg timed out after 600 s"
            continue
        if done.returncode != 0 or not (work / name).is_file():
            errors[name] = f"ffmpeg exit {done.returncode}: {done.stderr[-300:]}"
    # A partial file of a failed encode is not served.
    for name in errors:
        (work / name).unlink(missing_ok=True)
    return errors


class RangeServer:
    """A local HTTP server for the URL cases: GET with `Range: bytes=A-B`
    or `bytes=-N` answers 206 with the bytes and `Content-Range`, and a
    strong ETag; without a range, 200 and the whole file. One thread per
    connection, as the read plan opens several at once."""

    def __init__(self, files):
        self.lock = threading.Lock()
        self.requests = 0
        # Requests whose response head is pending, and requests not yet
        # answered to their end.
        self.pending = 0
        self.inflight = 0
        self.rounds = 0
        handler = self._handler(files, self)

        class Server(http.server.ThreadingHTTPServer):
            daemon_threads = True
            request_queue_size = 64

            def handle_error(self, request, client_address):
                # A client that drops a connection it no longer needs.
                if not isinstance(sys.exc_info()[1], (BrokenPipeError, ConnectionResetError)):
                    super().handle_error(request, client_address)

        self.server = Server(("127.0.0.1", 0), handler)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def url(self, name):
        return f"http://127.0.0.1:{self.port}/{name}"

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def idle(self, wait=10.0):
        """Waits until every request has been answered or dropped; whether
        it came to that within `wait` seconds."""
        deadline = time.monotonic() + wait
        while time.monotonic() < deadline:
            with self.lock:
                if self.inflight == 0:
                    return True
            time.sleep(0.01)
        return False

    def reset(self):
        """Starts the counts of one case, once the last one's requests end."""
        self.idle()
        with self.lock:
            self.requests = self.rounds = 0

    def counts(self):
        """The counts of the case, or None if requests were still open."""
        if not self.idle():
            return None
        with self.lock:
            return {"requests": self.requests, "rounds": self.rounds}

    @staticmethod
    def _handler(files, owner):
        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def do_GET(self):
                with owner.lock:
                    owner.requests += 1
                    if owner.pending == 0:
                        owner.rounds += 1
                    owner.pending += 1
                    owner.inflight += 1
                try:
                    time.sleep(RESPONSE_DELAY)
                    with owner.lock:
                        owner.pending -= 1
                    self.answer()
                finally:
                    with owner.lock:
                        owner.inflight -= 1

            def answer(self):
                path = files.get(self.path.lstrip("/"))
                if path is None:
                    self.send_error(404)
                    return
                size = path.stat().st_size
                start, end, status = 0, size - 1, 200
                wanted = self.headers.get("Range")
                if wanted:
                    match = re.fullmatch(r"bytes=(\d*)-(\d*)", wanted.strip())
                    if not match or (not match.group(1) and not match.group(2)):
                        self.send_error(416)
                        return
                    if match.group(1):
                        start = int(match.group(1))
                        end = min(int(match.group(2)), size - 1) if match.group(2) else size - 1
                    else:
                        start = max(0, size - int(match.group(2)))
                    if start >= size or start > end:
                        self.send_response(416)
                        self.send_header("Content-Range", f"bytes */{size}")
                        self.send_header("Content-Length", "0")
                        self.end_headers()
                        return
                    status = 206
                try:
                    self.send_response(status)
                    self.send_header("Accept-Ranges", "bytes")
                    self.send_header("ETag", f'"{size}-{path.name}"')
                    self.send_header("Content-Length", str(end - start + 1))
                    if status == 206:
                        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
                    self.end_headers()
                    with open(path, "rb") as handle:
                        handle.seek(start)
                        left = end - start + 1
                        while left > 0:
                            chunk = handle.read(min(left, 1 << 16))
                            if not chunk:
                                break
                            self.wfile.write(chunk)
                            left -= len(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    # The plan cancels a response it no longer needs.
                    pass

        return Handler


def url_counts(coreav, args, server):
    """What `coreav ARGS` asked the server for: the requests it received,
    the most in flight at once, and the bytes the readers received
    (`COREAV_REMOTE_STATS`, all of them together); or the error."""
    env = {k: v for k, v in os.environ.items()
           if k.lower() not in ("http_proxy", "https_proxy", "all_proxy") and not k.startswith("COREAV_REMOTE")}
    env.update({"COREAV_REMOTE_STATS": "1", "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"})
    server.reset()
    try:
        done = subprocess.run([coreav, *args], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
                              env=env, timeout=URL_TIMEOUT)
    except subprocess.TimeoutExpired:
        return None, f"timed out after {URL_TIMEOUT} s"
    if done.returncode != 0:
        return None, f"exit {done.returncode}: {done.stderr[-500:]}"
    stats = [json.loads(m) for m in STATS.findall(done.stderr)]
    if not stats:
        return None, "no reader statistics on standard error"
    counts = server.counts()
    if counts is None:
        return None, "the server still had requests open 10 s after the command ended"
    return {**counts, "bytes": sum(s["bytes_received"] for s in stats)}, None


def compare_url(name, got, before, same_bytes, failed):
    """The URL case's change against the baseline's and its status."""
    slack = 0 if same_bytes else URL_SLACK
    tolerance = URL_BYTES_TOLERANCE if same_bytes else URL_BYTES_TOLERANCE_OTHER
    more_requests = got["requests"] - before["requests"]
    more_rounds = got["rounds"] - before.get("rounds", got["rounds"])
    more_bytes = got["bytes"] / max(1, before["bytes"]) - 1
    change = f"{more_requests:+d} requests, {more_rounds:+d} rounds, {more_bytes:+.1%} bytes"
    if more_requests > slack or more_rounds > max(slack, URL_ROUNDS_SLACK.get(name, 0)) or more_bytes > tolerance:
        failed.append(name)
        return change, "MORE requests, rounds or bytes"
    if more_requests < 0 or more_rounds < 0 or more_bytes < -tolerance:
        return change, "fewer: the baseline can be lowered"
    return change, "ok"


def index_of(coreav, fixture, work):
    """The HLS index of `fixture`, built into `work` by `coreav hls
    index` (not counted), or None if it cannot be built."""
    path = work / f"{fixture.name}.cavhls"
    if not path.is_file():
        done = subprocess.run([coreav, "hls", "index", str(fixture), str(path)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120, check=False)
        if done.returncode != 0:
            return None
    return path


def inputs_of(coreav, fixture, name, work):
    """The files of the INPUTS set `name` written from `fixture` into a new
    directory under `work` (not counted), or None if one cannot be
    written."""
    directory = tempfile.mkdtemp(prefix=f"{name}-", dir=work)
    for command in INPUTS[name]:
        args = [str(fixture) if a == "{}" else a.replace("{dir}", directory) for a in command]
        done = subprocess.run([coreav, *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120,
                              check=False)
        if done.returncode != 0:
            return None
    numbered = [(re.search(r"(\d+)\.mp4$", f.name), f) for f in pathlib.Path(directory).iterdir()]
    if not numbered or any(m is None for m, _ in numbered):
        return None
    files = [f for _, f in sorted(numbered, key=lambda x: int(x[0].group(1)))]
    return [str(f) for f in files] or None


def version(command):
    """The first line of `command --version`, or None."""
    try:
        done = subprocess.run([command, "--version"], capture_output=True, text=True, timeout=60)
    except OSError:
        return None
    lines = done.stdout.strip().splitlines()
    return lines[0] if lines else None


def compare(name, count, before, tolerance, failed):
    """The change against the baseline's count and the case's status."""
    change = count / before - 1
    if change > tolerance:
        failed.append(name)
        return change, "SLOWER"
    if change < -tolerance:
        return change, "faster: the baseline can be lowered"
    return change, "ok"


def compare_case(name, count, before, total, before_startup, tolerance, failed):
    """A case's change against the reference's count, the change of its
    total (the whole command, start-up included, against the reference's
    start-up plus the case's count), and its status. A case above its
    tolerance fails unless the work moved out of start-up: start-up fell in
    the same run and the whole command does no more work than the
    reference's. Without the reference's start-up (a decoder build's case),
    the case alone."""
    if before_startup is None:
        return (*compare(name, count, before, tolerance, failed), None)
    reference = before_startup + before
    total_change = total / reference - 1
    change = count / before - 1
    if change > tolerance:
        if total - count < before_startup and total <= reference:
            return change, "moved from start-up: the whole command does no more work", total_change
        failed.append(name)
        if total > reference:
            return change, "SLOWER, and so is the whole command", total_change
        return change, "SLOWER", total_change
    if change < -tolerance:
        return change, "faster: the baseline can be lowered", total_change
    return change, "ok", total_change


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--coreav", required=True)
    parser.add_argument("--fixtures", required=True, type=pathlib.Path)
    parser.add_argument("--decoder-coreav",
                        help="a build with a decoder backend, for the cases that need one (DECODER_CASES)")
    parser.add_argument("--baseline", default=ROOT / "bench/guard/baseline.json", type=pathlib.Path)
    parser.add_argument("--write", type=pathlib.Path)
    parser.add_argument("--report-only", action="store_true",
                        help="print and write the counts without comparing (the base side of a pull request)")
    args = parser.parse_args()
    baseline = json.loads(args.baseline.read_text()) if args.baseline.exists() else None
    tolerance = baseline["tolerance"] if baseline else None
    if args.report_only:
        # Compare nothing; the counts written keep the tolerance.
        baseline = None
    expected = baseline["cases"] if baseline else {}
    tools = {"rustc": version("rustc"), "valgrind": version("valgrind")}

    work = pathlib.Path(tempfile.mkdtemp(prefix="bench-guard-"))
    try:
        return run_cases(args, baseline, tolerance, expected, tools, work)
    finally:
        shutil.rmtree(work, ignore_errors=True)


def run_cases(args, baseline, tolerance, expected, tools, work):
    derive(args.fixtures, work)
    not_generated = generate(work)
    not_generated.update(craft(work))
    rows, failed, broken = [], [], []
    startup, error = instructions(args.coreav, ["--version"])
    if error:
        print(f"coreav --version failed: {error}", file=sys.stderr)
        return 1
    # Start-up is subtracted from every case, and guarded as a case itself.
    if baseline:
        change, status = compare("start-up", startup, baseline["startup"], tolerance, failed)
        rows.append(("start-up (`coreav --version`)", startup, change, status, None, None))
    else:
        rows.append(("start-up (`coreav --version`)", startup, None, "no baseline", None, None))
    counts = {}
    decoder_startup = None
    if args.decoder_coreav:
        decoder_startup, error = instructions(args.decoder_coreav, ["--version"])
        if error:
            print(f"the decoder build's --version failed: {error}", file=sys.stderr)
            return 1
    for name, (fixture, template) in CASES.items():
        coreav, own_startup = args.coreav, startup
        if name in DECODER_CASES:
            if decoder_startup is None:
                # Without a decoder build the case cannot run; one the
                # baseline has must.
                if name in expected and not args.report_only:
                    broken.append(name)
                rows.append((name, None, None, "no decoder build", None, None))
                continue
            coreav, own_startup = args.decoder_coreav, decoder_startup
        path = (work if fixture in WRITTEN_HERE else args.fixtures) / fixture
        if not path.is_file():
            # A case the baseline has must run: a fixture that went missing
            # would otherwise hide every regression of the case.
            if name in expected:
                broken.append(name)
            why = not_generated.get(fixture, "")
            rows.append((name, None, None, f"fixture missing {why}".strip(), None, None))
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        index = None
        if "{index}" in template:
            index = index_of(args.coreav, path, work)
            if index is None and args.report_only:
                rows.append((name, None, None, "not in this build", None, None))
                continue
            if index is None:
                broken.append(name)
                rows.append((name, None, None, "command failed: the HLS index could not be built", None, None))
                continue
        prepared = {}
        for a in template:
            if a.startswith("{inputs:"):
                prepared[a] = inputs_of(args.coreav, path, a[len("{inputs:"):-1], work)
        if None in prepared.values():
            if args.report_only:
                rows.append((name, None, None, "not in this build", None, None))
                continue
            broken.append(name)
            rows.append((name, None, None, "command failed: its inputs could not be written", None, None))
            continue
        # A new directory per run, so that a writing command never finds the
        # files of an earlier one.
        out = tempfile.mkdtemp(prefix=f"{name}-", dir=work) if any("{out}" in a for a in template) else None
        command = []
        for a in template:
            command.extend(prepared[a] if a in prepared else [argument(a, path, index, out)])
        total, error = instructions(coreav, command)
        if error and args.report_only:
            # The base commit of a pull request that adds the case (a new
            # command): nothing to compare it with.
            rows.append((name, None, None, "not in this build", None, None))
            continue
        if error:
            broken.append(name)
            rows.append((name, None, None, f"command failed: {error}", None, None))
            continue
        count = total - own_startup
        counts[name] = {"fixture": fixture, "sha256": digest, "instructions": count}
        before = expected.get(name)
        if before is None:
            rows.append((name, count, None, "no baseline", None if name in DECODER_CASES else total, None))
            continue
        change, status, total_change = compare_case(name, count, before["instructions"], total,
                                                    baseline["startup"] if name not in DECODER_CASES else None,
                                                    tolerance, failed)
        if before["sha256"] != digest:
            status += " (fixture bytes differ from the baseline's)"
        rows.append((name, count, change, status, None if name in DECODER_CASES else total, total_change))

    expected_urls = baseline.get("url_cases", {}) if baseline else {}
    url_rows, url_counts_ = [], {}
    files = {}
    for fixture, _ in URL_CASES.values():
        path = (work if fixture in WRITTEN_HERE else args.fixtures) / fixture
        if path.is_file():
            files[fixture] = path
    server = RangeServer(files)
    try:
        for name, (fixture, template) in URL_CASES.items():
            path = files.get(fixture)
            if path is None:
                # A generated fixture that could not be encoded is an error
                # on either side; a missing fixture of gen-fixtures.sh fails
                # if the baseline has the case.
                if fixture in not_generated or name in expected_urls:
                    broken.append(name)
                url_rows.append((name, None, None, f"fixture missing {not_generated.get(fixture, '')}".strip()))
                continue
            out_dir = pathlib.Path(tempfile.mkdtemp(prefix=f"{name}-", dir=work))
            got, error = url_counts(args.coreav, [a.replace("{url}", server.url(fixture)).replace("{dir}", str(out_dir))
                                                  for a in template], server)
            if error and args.report_only:
                url_rows.append((name, None, None, "not in this build"))
                continue
            if error:
                broken.append(name)
                url_rows.append((name, None, None, f"command failed: {error}"))
                continue
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            url_counts_[name] = {"fixture": fixture, "sha256": digest, **got}
            before = expected_urls.get(name)
            if before is None:
                url_rows.append((name, got, None, "no baseline"))
                continue
            change, status = compare_url(name, got, before, before["sha256"] == digest, failed)
            if before["sha256"] != digest:
                status += " (fixture bytes differ from the baseline's)"
            url_rows.append((name, got, change, status))
    finally:
        server.close()

    lines = ["| Case | Instructions | Change | Total | Total change | Status |",
             "|---|---:|---:|---:|---:|---|"]
    for name, count, change, status, total, total_change in rows:
        lines.append(f"| {name} | {'' if count is None else f'{count:,}'} | "
                     f"{'' if change is None else f'{change:+.2%}'} | "
                     f"{'' if total is None else f'{total:,}'} | "
                     f"{'' if total_change is None else f'{total_change:+.2%}'} | {status} |")
    lines.append("\nEvery case counts the instructions beyond start-up; its total counts the whole command, "
                 "start-up included, against the baseline's start-up plus the case's; a case above the tolerance "
                 "passes only as work moved out of start-up, with start-up lower and the total no higher.")
    lines += ["", "| URL case | Requests | Rounds | Bytes received | Change | Status |",
              "|---|---:|---:|---:|---|---|"]
    for name, got, change, status in url_rows:
        lines.append(f"| {name} | {'' if got is None else got['requests']} | "
                     f"{'' if got is None else got['rounds']} | "
                     f"{'' if got is None else format(got['bytes'], ',')} | {change or ''} | {status} |")
    lines.append(f"\nURL cases read from a local range server (each response head held back "
                 f"{RESPONSE_DELAY * 1000:.0f} ms); no request or round more and {URL_BYTES_TOLERANCE:.0%} "
                 f"of bytes above the baseline, or {URL_SLACK} more and {URL_BYTES_TOLERANCE_OTHER:.0%} when the "
                 f"fixture's bytes differ; {', '.join(f'{n} {k} more rounds' for n, k in URL_ROUNDS_SLACK.items())} "
                 f"(its rounds vary from run to run).")
    if tolerance is not None:
        lines.append(f"Tolerance: {tolerance:.1%} above the baseline.")
    for tool, found in tools.items():
        recorded = (baseline or {}).get("tools", {}).get(tool)
        if baseline and recorded != found:
            lines.append(f"{tool} differs from the baseline's: {found} (baseline: {recorded}); "
                         "a difference may come from the tool, not the code.")
    print("\n".join(lines))

    if args.write:
        args.write.write_text(json.dumps({"tolerance": tolerance, "tools": tools, "startup": startup,
                                          "cases": counts, "url_cases": url_counts_}, indent=1) + "\n")
    if broken:
        print(f"cases that did not run: {', '.join(broken)}", file=sys.stderr)
        return 1
    if baseline is None:
        print("no baseline: report only", file=sys.stderr)
        return 0
    if failed:
        print(f"beyond the baseline's tolerance: {', '.join(failed)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
