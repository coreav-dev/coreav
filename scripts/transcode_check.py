#!/usr/bin/env python3
"""Bars B1 to B4 of `coreav transcode` (docs/design/transcode.md, "Bars").

    transcode_check.py --coreav PATH --arm NAME=ARGS [--arm ...] --fixtures DIR --out DIR
    transcode_check.py --coreav PATH --arm NAME=ARGS [--arm ...] --corpus --work DIR --out DIR
    transcode_check.py --summarize --out DIR
    transcode_check.py --merge SHARD_EVIDENCE... --out DIR

`--arm ffmpeg="--encoder ffmpeg --ffmpeg /opt/coreav-oracle-bin/ffmpeg"` and
`--arm x264="--encoder x264 --x264 /path/x264"` are the two arms of the note;
the words after NAME are added to every `coreav transcode` command of that
arm. For each input, every arm runs

    coreav transcode IN OUT.mp4 --preset veryfast --crf 23 [--scale -2:360] ARGS --force

and the baseline is the note's command, its `-map` options naming the
streams the first successful arm's summary lists as written (the video
first, then the audio, as the note writes it; streams are matched by kind
and order, never by output index):

    ffmpeg -nostdin -noautorotate -flags +unaligned -i IN -map 0:<video> [-map 0:<audio>]... -c:v libx264 \\
           -preset veryfast -crf 23 [-vf scale=-2:360] -c:a copy -fps_mode passthrough -y OUT.mp4

Before the run, each arm transcodes `--preflight` (default
/opt/coreav-fixtures/basic.mp4); the run stops when one does not, so a
misconfigured arm cannot pass as refusals.

The written streams are found in CoreAV's output from its summary: the kept
streams (the encoded video, `audio`, `other`) in source order, so the
encoded stream keeps its place; every check below names it by its absolute
output index. In the baseline's output the video is first, then the audio.

Then, for each arm's output (and the baseline's, for information):

- B1: `ffprobe -v error -select_streams <encoded stream> -show_frames OUT`
  exits 0 with nothing on standard error; `ffprobe` exits 0 and lists every
  written stream with a codec: 1 + the other video streams copied, and the
  audio streams written (streams FFmpeg adds of its own are listed, not a
  failure).
  With `--decode-oracle PATH`, a `tools/decode-oracle`-style program decodes
  the output's video (`PATH OUT --packets N [--decode-oracle-args]`) and its
  frames' MD5s must equal FFmpeg's `framemd5` of the same stream; outputs
  outside 8-bit 4:2:0 (`yuv420p`, `yuvj420p`) are counted apart by profile.
- B2: the output's video frames (`ffprobe -show_frames`,
  `best_effort_timestamp` as an exact rational) against the source's
  presented frames of the same stream: equal count; every time relative to
  the first within half a tick of the source's time base, exact when the
  output keeps that time base, a source time that repeats the one before
  expected one tick after the frame before while that is below the next
  source time (not expected otherwise); the first video frame's time minus the first
  audio packet's within 1 ms of the source's (the first written audio
  stream; every stream's offset is recorded).
- B3: every written audio stream's packets (`ffprobe -show_packets
  -show_data_hash MD5`) equal the source stream's in count and data, and the
  same stream of `coreav remux IN` in timestamps and durations (exact
  rationals), and the audio streams written are the ones `coreav remux IN`
  keeps. Where `coreav remux IN` cannot write the source (a video codec MP4
  cannot carry), `coreav remux --streams <the source's audio> IN`. A file
  counts as one with audio when that remux keeps an audio stream, for arms
  that fail and arms that succeed alike.
- B4: PSNR, the `average` of FFmpeg's `psnr` filter, pairing the n-th
  presented frame of each input: both inputs `-noautorotate -reinit_filter
  0`, each through `scale=W:H` at the output's size and then
  `settb=1,setpts=N`; a failure when the two frame counts or the compared
  count differ, and a comparison that crashes or reports no value fails
  that file's B4. Q1 against the source as FFmpeg decodes it; Q2 against
  the baseline's output; the video streams' size ratio.
- Fixtures only: `--bitrate 1000` against `-b:v 1M` on fixtures whose video
  is at least 5 s long; video bit rate within 10%.

Modes. `--fixtures DIR`: every MP4, MOV, Matroska and WebM file of DIR
whose default video stream's codec is on the default decode allowlist; the
fixture bars (100%; every fixture at least -0.50 dB of Q1 and 30 dB of Q2).
`--corpus`: corpus v1 `real` files tagged c:isobmff, c:matroska or c:webm,
classed by the codec of their default video stream (the highest
resolution, the first on a tie), taken per class in the order of
sha256("coreav-transcode-v1:" + sha256): files whose video is longer than
120 s are left out, at most 200 files a class, and a file that would take
the class past 1.2e11 pixel-frames (width x height x video packets, from
`ffprobe` before any run) is skipped and counted "over budget". The scaled
configuration takes the first 100 selected files, over all classes in the
order of the same hash, whose video is taller than 360 rows. The selection is computed once, before any
run, into OUT/selection.json, and reused afterwards. Files are fetched from
R2 (tools/stage3-report/r2pool.py; R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY,
R2_ENDPOINT, never printed nor passed to the tools), checked against their
SHA-256 and deleted after use (`--keep-files` keeps them); a file that
cannot be fetched stops the selection (it is never frozen in as left out).
Selections, results and logs name corpus files by SHA-256 only and hold no
object key; FFmpeg's failure text is kept only from the same
command run again at `-loglevel error`, without the input's metadata.

Refusals by design (exit 1 with a `coreav.error/1` object whose kind is
`unsupported_feature`, `unsupported_container`, `truncated_structure` or
`invalid_structure`, and whose message does not name the arm's setup, such
as "no x264 program") are counted apart by reason; any other failure counts
against every bar. The baseline still runs on a refused file (the default
video stream and the audio streams `coreav remux` keeps), and refusals of
files whose baseline succeeds are listed by file. A file whose baseline
fails to run is counted apart for B4 ("baseline failed"), and judged on B1
to B3. The baseline's own B2 and B3 misses are counted and listed.

`--shard K/N` (with `--fixtures`) runs the K-th of every N inputs in name
order, K from 1. A shard judges no bar: it writes its results and, with
`--compat`, its OUT/evidence, and exits 1 only when an input has no complete
result. `--merge` takes the evidence directories of shards 1 to N of one
run, each once, with the same command lines, arms and fixtures, and
summarizes and judges their inputs as one run, writing OUT/run.json,
OUT/summary.json and OUT/evidence as one run does; the programs' SHA-256
are listed by shard when the shards' builds differ.

Output: OUT/results/<config>/<file>.json per input (a run that stops
resumes: inputs with a complete result are skipped), OUT/summary.json with
per-class and overall verdicts per bar, configuration and arm (and the
baseline's own B1 to B3, for information), and a table on standard output.
B2 and B3: fixtures 100%; corpus 99% over all classes together ("all",
`met_with_every_class`) and 95% in every class. Only the run's inputs are
summarized: one without a complete result is counted as missing, and its
class gets no verdict. The exit status is 1 when an arm misses a bar, a class or an arm has no judged file, a class
of the selection is empty, or an input is missing. Uses only the
Python standard library.
"""
import argparse
import bisect
import collections
import concurrent.futures
import hashlib
import io
import json
import math
import os
import pathlib
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from fractions import Fraction

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
MANIFEST = ROOT / 'corpus' / 'v1' / 'manifest.json'

CLASSES = ('h264', 'hevc', 'av1', 'vp9', 'vp8', 'prores')
CONTAINER_TAGS = ('c:isobmff', 'c:matroska', 'c:webm')
MP4_EXTS = ('mp4', 'mov', 'm4v', '3gp', '3g2', 'm4a', 'm4b')
FIXTURE_EXTS = MP4_EXTS + ('mkv', 'webm')
# Refusals by design (docs/design/transcode.md, "Bars"): only the input side reports these kinds.
REFUSED_KINDS = ('unsupported_feature', 'unsupported_container', 'truncated_structure', 'invalid_structure')
# Messages of those kinds that come from the arm's setup (crates/coreav-transcode/src/pipeline.rs,
# crates/coreav-cli/src/transcode.rs): failures, never refusals. The messages that name the encoder for a
# format it does not take ("the encoder takes planar ...") are refusals of the input and stay so.
MISCONFIGURED = re.compile(r'\bno (?:x264|ffmpeg|encoder) program\b|path of the running coreav command')

# The pre-registered parameters (docs/design/transcode.md, "Bars").
SALT = 'coreav-transcode-v1:'
PRESET = 'veryfast'
CRF = 23
MAX_SECONDS = 120
MAX_FILES = 200
BUDGET = 1.2e11
SCALE = '-2:360'
SCALED_FILES = 100
SCALED_TALLER_THAN = 360
BITRATE_KBPS = 1000          # `--bitrate 1000` against `-b:v 1M`
BITRATE_MIN_SECONDS = 5
BITRATE_TOLERANCE = 0.10
AV_OFFSET = Fraction(1, 1000)
Q1_MEDIAN = -0.10
Q1_FLOOR = -0.50
Q2_MEDIAN = 35.0
Q2_FLOOR = 30.0
SHARE = 0.98
SIZE_RATIO = (0.98, 1.02)
CORPUS_SHARE = 0.99           # B2, B3 on the corpus, over all classes together
CLASS_SHARE = 0.95            # and in every class
RESAMPLES = 10_000
SEED = 518

TIMEOUT = 7200
PREFLIGHT = '/opt/coreav-fixtures/basic.mp4'
CONFIGS = ('default', 'scaled')

# B9 (docs/design/transcode.md): `coreav ffmpeg` command lines, IN and OUT.mp4 substituted, each against FFmpeg
# running the same command line. The first nine as `commands-v1` has them; the rest cover each accepted option.
COMPAT_COMMANDS = {
    'x01': '-i IN -c:v libx264 OUT.mp4',
    'x02': '-i IN -c:v libx264 -crf 19 OUT.mp4',
    'x03': '-i IN -crf 0 -c:v libx264 OUT.mp4',
    'x04': '-y -i IN -c:v libx264 -crf 24 OUT.mp4',
    'x05': '-i IN -c:v libx264 -crf 23 -preset medium -pix_fmt yuv420p OUT.mp4',
    'x06': '-i IN -vcodec libx264 -f mp4 OUT.mp4',
    'x07': '-i IN -vf scale=1920:-2 -vcodec libx264 -crf 20 OUT.mp4',
    'x08': '-y -i IN -c:a copy -c:v libx264 -pix_fmt yuv420p OUT.mp4',
    'x09': '-i IN -c:v libx264 -crf 1 OUT.mp4',
    'x10': '-noautorotate -i IN -c:v libx264 -preset veryfast -b:v 1M -c:a copy -movflags +faststart OUT.mp4',
    'x11': '-i IN -c copy -c:v libx264 -preset veryfast -s 320x240 -threads 2 -fps_mode passthrough OUT.mp4',
    'x12': '-i IN -an -c:v libx264 -preset veryfast OUT.mp4',
}
# Fixtures that miss B2 with `coreav transcode` as well (docs/measurements/transcode.md): B9 sets their B2 apart.
B2_KNOWN_MISSES = {'size_inband_sps.mp4'}
EXIT_UNSUPPORTED = 4
# A file `coreav ffmpeg` rejects (exit status 1, docs/design/compat-cli.md) is set apart as `coreav transcode`'s runs
# set a rejected file apart, when the message names one of these defects of the file.
REJECTED_DEFECTS = ('truncated structure', 'invalid structure')

ENV = None          # the tools' environment: no credentials
_print = threading.Lock()


def log(*a):
    with _print:
        print(*a, flush=True)


# --- processes and probes ------------------------------------------------------------------------------------

def child_env(ffmpeg=None):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('R2_', 'AWS_', 'COREAV_GCP_', 'CORPUS_R2'))}
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    # A refused `coreav ffmpeg` command stays refused (exit status 4) instead of running FFmpeg in its place: the
    # fallback names no program. Not strict-safe mode, which would also refuse the FFmpeg decoder and encoder the arms
    # run (docs/design/compat-cli.md, "Strict-safe mode").
    env['COREAV_FALLBACK'] = '/nonexistent/coreav-fallback'
    env.pop('COREAV_STRICT_SAFE', None)
    if ffmpeg:
        env['COREAV_FFMPEG'] = ffmpeg
    return env


def run(cmd, timeout=TIMEOUT, stdout=subprocess.PIPE):
    try:
        return subprocess.run([str(c) for c in cmd], stdout=stdout, stderr=subprocess.PIPE, timeout=timeout,
                              check=False, env=ENV)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(cmd, -999, b'', b'timeout')


def jrun(cmd, timeout=TIMEOUT):
    r = run(cmd, timeout)
    try:
        return r, json.loads(r.stdout or b'{}')
    except ValueError:
        return r, None


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        while chunk := f.read(1 << 22):
            h.update(chunk)
    return h.hexdigest()


def rational(value, base):
    """An integer timestamp in a time base, as an exact Fraction; None when absent."""
    if value in (None, 'N/A', ''):
        return None
    return Fraction(int(value)) * Fraction(str(base))


def fstr(x):
    """A Fraction as text for the results ('num/den')."""
    return None if x is None else f'{x.numerator}/{x.denominator}'


def default_video(streams):
    """The stream FFmpeg and CoreAV take by default: the highest resolution, the first on a tie; attached pictures
    left out."""
    best = None
    for s in streams:
        if s.get('codec_type') != 'video' or (s.get('disposition') or {}).get('attached_pic'):
            continue
        area = (s.get('width') or 0) * (s.get('height') or 0)
        if best is None or area > best[0]:
            best = (area, s)
    return best[1] if best else None


def streams_of(ffprobe, path):
    r, d = jrun([ffprobe, '-v', 'error', '-show_entries',
                 'stream=index,codec_type,codec_name,profile,width,height,pix_fmt,time_base'
                 ':stream_disposition=attached_pic', '-of', 'json', path], 600)
    if d is None or r.returncode != 0:
        return None
    return d.get('streams', [])


def probe(ffprobe, path):
    """What the selection needs, from ffprobe, before any run: the default video stream, its size, its packets
    (the frame count of the pixel-frame budget) and its length (the packets' span, exact)."""
    streams = streams_of(ffprobe, path)
    if streams is None:
        return {'status': 'probe_failed'}
    v = default_video(streams)
    if v is None:
        return {'status': 'no_video'}
    r = run([ffprobe, '-v', 'error', '-select_streams', str(v['index']), '-show_entries', 'packet=pts,duration',
             '-of', 'csv=p=0', path], 1800)
    if r.returncode != 0:
        return {'status': 'probe_failed'}
    tb = v.get('time_base') or '1/1'
    first = end = None
    packets = 0
    for line in r.stdout.decode(errors='replace').splitlines():
        parts = line.strip().split(',')
        if not parts or not parts[0]:
            continue
        packets += 1
        try:
            pts = int(parts[0])
        except ValueError:
            continue
        try:
            dur = int(parts[1]) if len(parts) > 1 and parts[1] not in ('', 'N/A') else 0
        except ValueError:
            dur = 0
        first = pts if first is None else min(first, pts)
        end = pts + dur if end is None else max(end, pts + dur)
    seconds = float((end - first) * Fraction(tb)) if first is not None else None
    w, h = v.get('width') or 0, v.get('height') or 0
    return {'status': 'ok', 'stream_index': v['index'], 'codec': v.get('codec_name'), 'width': w, 'height': h,
            'pix_fmt': v.get('pix_fmt'), 'packets': packets, 'seconds': seconds, 'pixel_frames': w * h * packets,
            'audio': sum(1 for s in streams if s.get('codec_type') == 'audio')}


def frames(ffprobe, path, select):
    """(time base, best_effort_timestamps in presentation order) of one video stream, or (None, error)."""
    r, d = jrun([ffprobe, '-v', 'error', '-select_streams', select, '-show_entries',
                 'frame=best_effort_timestamp:stream=time_base', '-of', 'json', path])
    if d is None or r.returncode != 0 or not d.get('streams'):
        return None, f'ffprobe exit {r.returncode}: {r.stderr.decode(errors="replace")[-200:]}'
    ts = [f.get('best_effort_timestamp') for f in d.get('frames', [])]
    return d['streams'][0].get('time_base'), [None if t in (None, 'N/A') else int(t) for t in ts]


def packets(ffprobe, path, select, data_hash=False):
    """(time base, packets) of one stream: pts, dts, duration, size and, with data_hash, MD5 of the data."""
    cmd = [ffprobe, '-v', 'error', '-select_streams', select, '-show_packets']
    entries = 'packet=pts,dts,duration,size'
    if data_hash:
        cmd += ['-show_data_hash', 'MD5']
        entries += ',data_hash'
    r, d = jrun(cmd + ['-show_entries', entries + ':stream=time_base', '-of', 'json', path])
    if d is None or r.returncode != 0 or not d.get('streams'):
        return None, f'ffprobe exit {r.returncode}: {r.stderr.decode(errors="replace")[-200:]}'
    return d['streams'][0].get('time_base'), d.get('packets', [])


# --- the pure comparisons (scripts/transcode_check_selftest.py) ------------------------------------------------

def moved_repeats(times):
    """The source's frame times as an MP4 holds them, whose decode times increase (docs/design/transcode.md,
    "Which frames"): a time that repeats the one before (Matroska's millisecond times can give several frames
    one time) is one tick after the frame before when that is below the source's next time (any time after the
    last one), and the frame is not expected otherwise; a time below the one before is left as it is. Returns
    the times, how many moved, and how many are not expected."""
    later = sorted(set(times))
    out, last, moved, dropped = [], None, 0, 0
    for t in times:
        if last is not None and t >= last and out and t <= out[-1]:
            nxt = bisect.bisect_right(later, t)
            if nxt == len(later) or out[-1] + 1 < later[nxt]:
                out.append(out[-1] + 1)
                moved += 1
            else:
                dropped += 1
        else:
            out.append(t)
        last = t
    return out, moved, dropped


def compare_frames(src_tb, src, out_tb, out):
    """B2's frame part: equal counts, and each time relative to the first within half a tick of the source's
    time base; exact when the output keeps that time base. A source time that repeats is expected moved
    (moved_repeats)."""
    res = {'source_frames': len(src), 'output_frames': len(out)}
    if not src or not out:
        return dict(res, ok=False, reason='no frames')
    if None in src or None in out:
        return dict(res, ok=False, reason='a frame without a timestamp')
    src, moved, dropped = moved_repeats(src)
    if moved or dropped:
        res.update(moved=moved, not_expected=dropped)
    if len(src) != len(out):
        return dict(res, ok=False, reason='frame counts differ')
    stb, otb = Fraction(str(src_tb)), Fraction(str(out_tb))
    tolerance = Fraction(0) if stb == otb else stb / 2
    worst, at = Fraction(0), None
    for i, (s, o) in enumerate(zip(src, out)):
        err = abs((o - out[0]) * otb - (s - src[0]) * stb)
        if err > worst:
            worst, at = err, i
    res.update(tolerance=fstr(tolerance), worst=fstr(worst), worst_seconds=float(worst), worst_frame=at)
    if worst > tolerance:
        return dict(res, ok=False, reason='a timestamp differs')
    return dict(res, ok=True)


def compare_offset(src_video, src_audio, out_video, out_audio):
    """B2's A/V part: (first video frame - first audio packet) of the output against the source's, within 1 ms."""
    if None in (src_video, src_audio, out_video, out_audio):
        return {'ok': False, 'reason': 'a first time is missing'}
    d = (out_video - out_audio) - (src_video - src_audio)
    return {'ok': abs(d) <= AV_OFFSET, 'difference': fstr(d), 'difference_seconds': float(d)}


def compare_audio(src, out, remux):
    """B3 for one stream: each argument is (time base, packets). Count and data against the source; pts, dts and
    duration as exact rationals against `coreav remux`'s same stream."""
    (_, sp), (otb, op) = src, out
    res = {'source_packets': len(sp), 'output_packets': len(op)}
    if len(sp) != len(op):
        return dict(res, ok=False, reason='packet counts differ')
    bad = next((i for i, (a, b) in enumerate(zip(sp, op)) if a.get('data_hash') != b.get('data_hash')
                or a.get('data_hash') is None), None)
    if bad is not None:
        return dict(res, ok=False, reason='packet data differs', packet=bad)
    if remux is None:
        return dict(res, ok=False, reason='no coreav remux stream to compare timestamps with')
    rtb, rp = remux
    if len(rp) != len(op):
        return dict(res, ok=False, reason='packet count differs from coreav remux', remux_packets=len(rp))
    for i, (a, b) in enumerate(zip(op, rp)):
        for key in ('pts', 'dts', 'duration'):
            if rational(a.get(key), otb) != rational(b.get(key), rtb):
                return dict(res, ok=False, reason=f'{key} differs from coreav remux', packet=i,
                            output=fstr(rational(a.get(key), otb)), remux=fstr(rational(b.get(key), rtb)))
    return dict(res, ok=True)


def first_time(tb, values):
    for v in values:
        if v is not None:
            return rational(v, tb)
    return None


def salted(sha, salt=SALT):
    return hashlib.sha256((salt + sha).encode()).hexdigest()


def select_class(candidates, probe_of, cls, max_files=MAX_FILES, budget=BUDGET, min_seconds=None,
                 max_seconds=MAX_SECONDS, salt=SALT, probe_rest=False):
    """The budgeted selection of one class: candidates (manifest entries) in salted order, each probed by
    `probe_of(entry)`; returns (selected [(entry, probe)], apart Counter, eligible-but-not-taken
    [(entry, probe)]). A file whose default video is another allowlisted codec belongs to that class and is not
    counted here. Probing stops at max_files selected unless probe_rest (the speed run's top-up needs every
    eligible file)."""
    selected, rest, apart = [], [], collections.Counter()
    used = 0
    for entry in sorted(candidates, key=lambda e: salted(e['sha256'], salt)):
        if len(selected) >= max_files and not probe_rest:
            break
        p = probe_of(entry)
        if p.get('status') != 'ok':
            apart[p.get('status', 'probe_failed')] += 1
            continue
        if p['codec'] != cls:
            if p['codec'] not in CLASSES:
                apart[f'not_allowlisted:{p["codec"]}'] += 1
            continue
        if p['seconds'] is None or p['seconds'] > max_seconds:
            apart[f'video longer than {max_seconds} s'] += 1
            continue
        if min_seconds is not None and p['seconds'] < min_seconds:
            apart[f'video shorter than {min_seconds} s'] += 1
            continue
        if len(selected) >= max_files:
            rest.append((entry, p))
            continue
        if used + p['pixel_frames'] > budget:
            apart['over budget'] += 1
            rest.append((entry, p))
            continue
        used += p['pixel_frames']
        selected.append((entry, p))
    return selected, apart, rest


def bootstrap(values, resamples=RESAMPLES, seed=SEED):
    """(median, low, high): the median and its bootstrap 95% interval (percentile method)."""
    import random
    if not values:
        return None, None, None
    rng = random.Random(seed)
    meds = sorted(statistics.median(rng.choices(values, k=len(values))) for _ in range(resamples))
    return statistics.median(values), meds[int(0.025 * resamples)], meds[int(0.975 * resamples) - 1]


def parse_psnr(stderr):
    """(average, [compared, distorted, reference] frame counts) from `ffmpeg -v verbose` with three null outputs."""
    avg = None
    m = re.findall(r'PSNR y:\S+ .*?average:(\S+)', stderr)
    if m:
        avg = float('inf') if m[-1] == 'inf' else float(m[-1])
    counts = {}
    for k, n in re.findall(r'\[out#(\d+)/null[^\]]*\]\s+Output stream #\d+:\d+ \(video\): (\d+) frames encoded',
                           stderr):
        counts[int(k)] = int(n)
    return avg, [counts.get(i) for i in range(3)]


def error_of(r):
    text = r.stderr.decode(errors='replace').strip()
    for candidate in (text, text.splitlines()[-1] if text else ''):
        try:
            d = json.loads(candidate)
            if isinstance(d, dict):
                return d
        except ValueError:
            pass
    return {'raw': text[-400:], 'exit': r.returncode}


# --- one input -------------------------------------------------------------------------------------------------

def ffmpeg_error_text(cmd):
    """The error text of a failed FFmpeg command, from the same command run again at `-loglevel error`: the
    first run's log holds the banner and the input's metadata, which the results must not keep."""
    r = run([cmd[0], '-loglevel', 'error'] + [c for c in cmd[1:] if c not in ('-v', 'verbose')])
    return r.stderr.decode(errors='replace')[-400:]


def psnr(ffmpeg, dist, dist_index, ref, ref_index, width, height):
    """PSNR of stream `dist_index` of `dist` against stream `ref_index` of `ref`: both inputs through `scale=W:H`
    at the output's size, then `settb=1,setpts=N`. The counts of the frames reaching `psnr` from each side and of
    the frames compared come from three null outputs."""
    chain = f'scale={width}:{height},settb=1,setpts=N'
    graph = (f'[0:{dist_index}]{chain},split[d][dc];[1:{ref_index}]{chain},split[r][rc];'
             '[d][r]psnr=shortest=1[o]')
    # -reinit_filter 0: a stream that changes size mid-way goes on through `scale` instead of reconfiguring the
    # graph, which would drop the frames in flight on the other branch.
    cmd = [ffmpeg, '-nostdin', '-hide_banner', '-nostats', '-v', 'verbose', '-noautorotate', '-reinit_filter', '0',
           '-flags', '+unaligned', '-i', dist, '-noautorotate', '-reinit_filter', '0', '-flags', '+unaligned', '-i', ref, '-filter_complex', graph]
    for label in ('[o]', '[dc]', '[rc]'):
        cmd += ['-map', label, '-fps_mode', 'passthrough', '-f', 'null', '-']
    r = run(cmd)
    avg, (compared, nd, nr) = parse_psnr(r.stderr.decode(errors='replace'))
    res = {'average': avg, 'compared': compared, 'frames': nd, 'reference_frames': nr}
    if r.returncode != 0 or avg is None:
        # A comparison that crashes or reports no value fails the file's B4.
        return dict(res, ok=False, reason=f'ffmpeg exit {r.returncode}: {ffmpeg_error_text(cmd)}')
    if not (compared == nd == nr):
        return dict(res, ok=False, reason='frame counts differ')
    return dict(res, ok=True)


def native_conformance(args, out, index, profile, pix_fmt, npackets):
    """The native decoder's frames of output stream `index` against FFmpeg's framemd5 of it; 8-bit 4:2:0 only
    (`yuvj420p` is 8-bit 4:2:0 with the full range flag)."""
    if pix_fmt not in ('yuv420p', 'yuvj420p'):
        return {'status': 'apart', 'reason': f'profile {profile} {pix_fmt}'}
    cmd = [args.decode_oracle, out, '--packets', str(npackets)] + shlex.split(args.decode_oracle_args or '')
    r, ours = jrun(cmd)
    if ours is None or 'frames' not in ours:
        return {'status': 'failed', 'ok': False,
                'error': (ours or {}).get('error') or r.stdout[-300:].decode(errors='replace')}
    # The FFmpeg fallback would compare FFmpeg with itself: the oracle must have run CoreAV's own decoder.
    if str(ours.get('decoder', '')).startswith('ffmpeg'):
        return {'status': 'failed', 'ok': False, 'error': f'the decode oracle ran {ours.get("decoder")}, not the native decoder'}
    if ours.get('stream_index') not in (None, index):
        return {'status': 'apart', 'reason': f'the decode oracle took output stream {ours.get("stream_index")}'}
    # -noautorotate: framemd5 hashes the decoded frames, before any turn by the track's matrix; the edit list
    # ignored, as the decode oracle decodes every packet.
    f = run([args.ffmpeg, '-v', 'error', '-noautorotate', '-ignore_editlist', '1', '-i', out, '-map', f'0:{index}',
             '-fps_mode', 'passthrough', '-noautoscale', '-f', 'framemd5', '-'])
    theirs = [line.split(',')[-1].strip() for line in f.stdout.decode(errors='replace').splitlines()
              if line and not line.startswith('#')]
    mine = [fr['md5'] for fr in ours['frames']]
    equal = sum(1 for a, b in zip(mine, theirs) if a == b)
    return {'status': 'ok', 'ok': len(mine) == len(theirs) and equal == len(mine), 'frames': len(mine),
            'ffmpeg_frames': len(theirs), 'equal': equal, 'decoder': ours.get('decoder')}


def output_layout(summary):
    """Where the written streams are in CoreAV's output: the kept streams (the encoded video, the audio, the
    other streams) in source order, so the encoded stream keeps its place. Returns the output index of the
    encoded video, of each written audio stream (in the summary's order), and how many video streams the output
    has (the encoded one and the other video streams copied)."""
    v = summary['video']['stream_index']
    audio = [a['stream_index'] for a in summary.get('audio', [])]
    other = summary.get('other') or []
    order = sorted([v] + audio + [o['stream_index'] for o in other])
    pos = {s: i for i, s in enumerate(order)}
    return {'video': pos[v], 'audio': [pos[a] for a in audio],
            'videos': 1 + sum(1 for o in other if o.get('kind') == 'video'), 'streams': len(order)}


def baseline_layout(audio):
    """The baseline's output: `-map` order, the video first."""
    return {'video': 0, 'audio': list(range(1, 1 + len(audio))), 'videos': 1, 'streams': 1 + len(audio)}


class Source:
    """The source facts every output is compared with, read once per input.
    `kinds` are the stream kinds of `coreav remux IN` whose packets are read."""

    kinds = ('audio',)

    def __init__(self, args, path):
        self.args, self.path = args, path
        self.streams = streams_of(args.ffprobe, path) or []
        self._frames, self._audio, self._remux = {}, {}, None
        self.remux_streams = None
        self.remux_audio = None

    def video_frames(self, index):
        if index not in self._frames:
            self._frames[index] = frames(self.args.ffprobe, self.path, str(index))
        return self._frames[index]

    def audio(self, index):
        if index not in self._audio:
            self._audio[index] = packets(self.args.ffprobe, self.path, str(index), data_hash=True)
        return self._audio[index]

    def has_audio(self):
        """Whether the file counts as one "with audio" for B3: `coreav remux IN` keeps an audio stream (or, when
        the remux cannot run, the source has one). The same rule for arms that succeed and arms that fail."""
        if self.remux_audio is not None:
            return bool(self.remux_audio)
        return any(s.get('codec_type') == 'audio' for s in self.streams)

    def remux(self, tmp):
        """{source stream index: (time base, packets)} of `coreav remux IN`, or an error text; `remux_audio` the
        audio streams it keeps. Where it cannot write the source (a video codec MP4 cannot carry, such as ProRes),
        `coreav remux --streams <the source's audio streams> IN`, recorded as `remux_streams`; when that is refused
        because MP4 cannot carry one of them (`carried_none`), each audio stream alone (`each audio`), keeping
        those it writes."""
        if self._remux is None:
            out = tmp / 'remux.mp4'
            r, summary = jrun([self.args.coreav, 'remux', self.path, out, '--force'])
            self.remux_streams = 'all'
            source_audio = [s['index'] for s in self.streams if s.get('codec_type') == 'audio']
            if (r.returncode != 0 or not summary) and source_audio:
                r, summary = jrun([self.args.coreav, 'remux', '--streams', ','.join(str(a) for a in source_audio),
                                   self.path, out, '--force'])
                self.remux_streams = 'audio'
            if (r.returncode != 0 and self.remux_streams == 'audio'
                    and error_of(r).get('kind') == 'unsupported_feature' and carried_none(error_of(r))):
                # MP4 cannot carry one of the source's audio streams (Vorbis in WebM, PCM in MOV), and the remux
                # refuses the whole selection: each stream alone, keeping those MP4 carries.
                self.remux_streams = 'each audio'
                self._remux = {}
                for a in source_audio:
                    r, summary = jrun([self.args.coreav, 'remux', '--streams', str(a), self.path, out, '--force'])
                    if r.returncode == 0 and summary:
                        for i, s in enumerate(summary.get('streams', [])):
                            if s.get('kind') in self.kinds:
                                self._remux[s['stream_index']] = packets(self.args.ffprobe, out, str(i))
                    elif not carried_none(error_of(r)):
                        self._remux = f'coreav remux --streams {a} failed: {json.dumps(error_of(r))[:300]}'
                        break
                if isinstance(self._remux, dict):
                    self.remux_audio = sorted(self._remux)
            elif r.returncode != 0 or not summary:
                self._remux = f'coreav remux failed: {json.dumps(error_of(r))[:300]}'
            else:
                self._remux = {}
                for i, s in enumerate(summary.get('streams', [])):
                    if s.get('kind') in self.kinds:
                        self._remux[s['stream_index']] = packets(self.args.ffprobe, out, str(i))
                self.remux_audio = sorted(self._remux)
            if out.exists():
                out.unlink()
        return self._remux


# What `coreav remux` says when it does not copy an audio stream's codec to MP4 (coreav-isobmff-write's remux and
# Matroska remux): only these refusals count as "MP4 does not carry it" for B3; any other refusal is a failure.
NOT_CARRIED = re.compile(r'no standard sample entry|(is|are) not copied to MP4|MP4 cannot carry this codec'
                         r'|audio other than AAC, MP3 or Opus')


def carried_none(err):
    return err.get('kind') == 'unsupported_feature' and bool(NOT_CARRIED.search(str(err.get('message') or '')))


def misconfigured(err):
    """A refusal kind that comes from the arm's setup, not from the input: no encoder program, or no path to
    the running command. Counted as a failure, never as a refusal."""
    return bool(MISCONFIGURED.search(str(err.get('message') or '')))


def transcode(args, arm_args, path, out, config, bitrate=False):
    cmd = [args.coreav, 'transcode', path, out, '--preset', PRESET]
    cmd += ['--bitrate', str(BITRATE_KBPS)] if bitrate else ['--crf', str(CRF)]
    if config == 'scaled':
        cmd += ['--scale', SCALE]
    cmd += arm_args + ['--force']
    t = time.monotonic()
    r = run(cmd)
    res = {'exit': r.returncode, 'seconds': round(time.monotonic() - t, 3)}
    if r.returncode == 0:
        try:
            summary = json.loads(r.stdout)
        except ValueError:
            return dict(res, status='failed', reason='exit 0 without a JSON summary')
        if summary.get('schema') != 'coreav.transcode/1' or not os.path.isfile(out):
            return dict(res, status='failed', reason='no coreav.transcode/1 summary or no output', summary=summary)
        return dict(res, status='ok', summary=summary)
    err = error_of(r)
    if r.returncode == 1 and err.get('schema') == 'coreav.error/1' and err.get('kind') in REFUSED_KINDS:
        if misconfigured(err):
            return dict(res, status='failed', reason='the arm is misconfigured', error=err)
        return dict(res, status='refused', reason=err.get('kind'), error=err)
    return dict(res, status='failed', reason=f'exit {r.returncode}' + (f' ({err.get("kind")})' if 'kind' in err else ''),
                error=err)


def baseline_cmd(args, path, out, video, audio, config, bitrate=False):
    cmd = [args.ffmpeg, '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', path, '-map', f'0:{video}']
    for a in audio:
        cmd += ['-map', f'0:{a}']
    cmd += ['-c:v', 'libx264', '-preset', PRESET]
    cmd += ['-b:v', '1M'] if bitrate else ['-crf', str(CRF)]
    if config == 'scaled':
        cmd += ['-vf', f'scale={SCALE}']
    return cmd + ['-c:a', 'copy', '-fps_mode', 'passthrough', '-y', out]


def baseline(args, path, out, video, audio, config, bitrate=False):
    cmd = baseline_cmd(args, path, out, video, audio, config, bitrate)
    t = time.monotonic()
    r = run(cmd)
    res = {'exit': r.returncode, 'seconds': round(time.monotonic() - t, 3)}
    if r.returncode != 0 or not os.path.isfile(out):
        return dict(res, status='failed', reason=ffmpeg_error_text(cmd))
    return dict(res, status='ok')


def video_bytes_and_rate(args, out, index):
    tb, pk = packets(args.ffprobe, out, str(index))
    if tb is None:
        return None, None
    size = sum(int(p.get('size') or 0) for p in pk)
    ts = [(int(p['pts']), int(p.get('duration') or 0)) for p in pk if p.get('pts') not in (None, 'N/A')]
    if not ts:
        return size, None
    span = (max(a + b for a, b in ts) - min(a for a, _ in ts)) * Fraction(tb)
    return size, (size * 8 / float(span)) if span > 0 else None


def attached_picture(stream):
    """Whether ffprobe lists `stream` as an attached picture: a cover image that the movie's metadata carries (an
    `ilst` `covr` item in the copied `udta`), not a track."""
    return (stream.get('disposition') or {}).get('attached_pic') == 1


def streams_read(ostreams, layout, audio):
    """B1's stream half, from ffprobe's `streams` of the output: every stream read, and the written ones there
    with a codec. FFmpeg adds streams of its own (a chapter text track, a timecode track, an attached picture for
    a cover image in the copied metadata): listed and read, not counted as written tracks."""
    kinds = [s.get('codec_type') for s in ostreams]
    tracks = [s for s in ostreams if not attached_picture(s)]
    ov = next((s for s in tracks if s.get('index') == layout['video']), {})
    return (all(kinds) and sum(1 for s in tracks if s.get('codec_type') == 'video') == layout['videos']
            and kinds.count('audio') == audio and ov.get('codec_type') == 'video'
            and all(s.get('codec_name') for s in ostreams if s.get('codec_type') in ('video', 'audio'))
            and all(next((s for s in ostreams if s.get('index') == a), {}).get('codec_type') == 'audio'
                    for a in layout['audio']))


def judge_output(args, src, out, layout, video_index, audio_indices, tmp, expected_audio=None, extra_audio=0):
    """B1 to B3 of one output file, and what B4 needs of it. `layout` places the written streams in the output
    (output_layout, baseline_layout); video_index and audio_indices are the source streams they come from.
    `expected_audio`: the source audio streams B3 expects written, by default those `coreav remux IN` keeps.
    `extra_audio`: the audio streams written beside audio_indices that are not their packets (encoded, say), which B1
    counts and B3 leaves to the caller."""
    res = {'layout': layout}
    vi = layout['video']
    # B1
    r = run([args.ffprobe, '-v', 'error', '-select_streams', str(vi), '-show_frames', out], stdout=subprocess.DEVNULL)
    clean = r.returncode == 0 and not r.stderr.strip()
    r2, d = jrun([args.ffprobe, '-v', 'error', '-show_entries',
                  'stream=index,codec_type,codec_name,profile,width,height,pix_fmt,time_base'
                  ':stream_disposition=attached_pic', '-of', 'json', out])
    ostreams = (d or {}).get('streams', [])
    kinds = [s.get('codec_type') for s in ostreams]
    ov = next((s for s in ostreams if s.get('index') == vi), {})
    probed = r2.returncode == 0 and streams_read(ostreams, layout, len(audio_indices) + extra_audio)
    res['B1'] = {'ok': clean and probed, 'decode_clean': clean,
                 'decode_messages': r.stderr.decode(errors='replace')[-300:], 'ffprobe_ok': probed, 'streams': kinds}
    res['video'] = {k: ov.get(k) for k in ('codec_name', 'profile', 'width', 'height', 'pix_fmt', 'time_base')}
    size, rate = video_bytes_and_rate(args, out, vi)
    res['video']['bytes'], res['video']['bit_rate'] = size, rate
    if args.decode_oracle:
        _, pk = packets(args.ffprobe, out, str(vi))
        res['B1']['native'] = native_conformance(args, out, vi, ov.get('profile'), ov.get('pix_fmt'),
                                                 len(pk) if isinstance(pk, list) else 0)
    # B2
    stb, sframes = src.video_frames(video_index)
    otb, oframes = frames(args.ffprobe, out, str(vi))
    if stb is None or otb is None:
        res['B2'] = {'ok': False, 'reason': sframes if stb is None else oframes}
    else:
        b2 = compare_frames(stb, sframes, otb, oframes)
        offsets = []
        for a, oa in zip(audio_indices, layout['audio']):
            satb, sap = src.audio(a)
            oatb, oap = packets(args.ffprobe, out, str(oa))
            if satb is None or oatb is None:
                offsets.append({'ok': False, 'reason': 'audio packets not read'})
                continue
            offsets.append(compare_offset(first_time(stb, sframes), first_time(satb, [p.get('pts') for p in sap]),
                                          first_time(otb, oframes), first_time(oatb, [p.get('pts') for p in oap])))
        b2['av_offsets'] = offsets
        if offsets and not offsets[0]['ok']:
            b2 = dict(b2, ok=False, reason=(b2.get('reason') or 'A/V offset differs'))
        res['B2'] = b2
    # B3: judged on every file with audio (Source.has_audio) and every output that wrote some.
    if audio_indices or src.has_audio():
        remux = src.remux(tmp)
        per = []
        for a, oa in zip(audio_indices, layout['audio']):
            s = src.audio(a)
            o = packets(args.ffprobe, out, str(oa), data_hash=True)
            if s[0] is None or o[0] is None:
                per.append({'ok': False, 'reason': 'packets not read'})
                continue
            rm = remux.get(a) if isinstance(remux, dict) else None
            c = compare_audio(s, o, rm if rm and rm[0] is not None else None)
            if isinstance(remux, str):
                c['remux_error'] = remux
            per.append(dict(c, stream_index=a))
        expected = src.remux_audio if expected_audio is None else sorted(expected_audio)
        same_set = expected is not None and sorted(audio_indices) == expected
        res['B3'] = {'ok': same_set and all(p['ok'] for p in per), 'streams': per,
                     'kept_audio': sorted(audio_indices), 'expected_audio': expected,
                     'remux_streams': src.remux_streams}
        if not same_set:
            res['B3']['reason'] = 'the audio streams kept differ from those expected'
    return res


def check_input(args, path, config, cls, tmp):
    """Every arm and the baseline on one input in one configuration."""
    src = Source(args, path)
    src.remux(tmp)
    res = {'config': config, 'class': cls, 'arms': {}, 'has_audio': src.has_audio(),
           'remux_audio': src.remux_audio}
    summary = None
    for name, arm_args in args.arms:
        out = tmp / f'{name}.mp4'
        res['arms'][name] = transcode(args, arm_args, path, out, config)
        if res['arms'][name]['status'] == 'ok' and summary is None:
            summary = res['arms'][name]['summary']
    if summary is None:
        # No arm wrote an output. The baseline still runs, on the default video stream and the audio streams
        # `coreav remux` keeps, so that refusals of files FFmpeg transcodes are listed.
        v = default_video(src.streams)
        if v is not None:
            base = tmp / 'baseline.mp4'
            res['baseline'] = dict(baseline(args, path, base, v['index'], src.remux_audio or [], config),
                                   maps_from='probe')
            base.unlink(missing_ok=True)
        res['complete'] = True
        return res
    video = summary['video']['stream_index']
    audio = [a['stream_index'] for a in summary.get('audio', [])]
    res['maps'] = {'video': video, 'audio': audio}
    if audio != sorted(audio):
        res['maps']['note'] = 'the summary does not list the audio streams in source order'
    v = next((s for s in src.streams if s.get('index') == video), {})
    res['source'] = {k: v.get(k) for k in ('codec_name', 'profile', 'width', 'height', 'pix_fmt', 'time_base')}
    base = tmp / 'baseline.mp4'
    res['baseline'] = baseline(args, path, base, video, audio, config)
    if res['baseline']['status'] == 'ok':
        res['baseline'].update(judge_output(args, src, base, baseline_layout(audio), video, audio, tmp))
    for name, _ in args.arms:
        arm = res['arms'][name]
        if arm['status'] != 'ok':
            continue
        out = tmp / f'{name}.mp4'
        s = arm['summary']
        a_audio = [a['stream_index'] for a in s.get('audio', [])]
        if s['video']['stream_index'] != video or a_audio != audio:
            arm['maps_differ'] = True
        layout = output_layout(s)
        arm.update(judge_output(args, src, out, layout, s['video']['stream_index'], a_audio, tmp))
        w, h = arm['video'].get('width'), arm['video'].get('height')
        if not (w and h):
            arm['Q1'] = {'ok': False, 'reason': 'no output size'}
            arm['Q2'] = {'ok': False, 'reason': 'no output size'}
        else:
            q1 = psnr(args.ffmpeg, out, layout['video'], path, video, w, h)
            arm['Q1_coreav'] = q1
            if res['baseline']['status'] == 'ok':
                bq = res['baseline'].get('Q1')
                if bq is None:
                    bq = res['baseline']['Q1'] = psnr(args.ffmpeg, base, 0, path, video,
                                                      res['baseline']['video'].get('width'),
                                                      res['baseline']['video'].get('height'))
                if q1['ok'] and bq['ok']:
                    a, b = q1['average'], bq['average']
                    arm['Q1'] = {'ok': True, 'delta': 0.0 if a == b else a - b}
                else:
                    arm['Q1'] = {'ok': False, 'reason': 'a PSNR failed'}
                arm['Q2'] = psnr(args.ffmpeg, out, layout['video'], base, 0, w, h)
                bs, cs = res['baseline']['video'].get('bytes'), arm['video'].get('bytes')
                arm['size_ratio'] = cs / bs if bs and cs else None
        if args.fixtures and config == 'default':
            arm['bitrate'] = bitrate_check(args, name, path, tmp, video, audio)
        out.unlink(missing_ok=True)
    base.unlink(missing_ok=True)
    res['complete'] = True
    return res


def compat_words(key, path, out):
    return [str(path) if w == 'IN' else str(out) if w == 'OUT.mp4' else w for w in shlex.split(COMPAT_COMMANDS[key])]


def compat_run(args, arm_env, key, path, out):
    """`coreav ffmpeg` with one B9 command line: 'ok', 'refused' (exit status 4, or exit status 1 naming a defect of
    the file as REJECTED_DEFECTS lists, nothing written either way) or 'failed'."""
    env = dict(ENV, **arm_env)
    t = time.monotonic()
    try:
        r = subprocess.run([args.coreav, 'ffmpeg'] + compat_words(key, path, out), stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=TIMEOUT, check=False, env=env)
    except subprocess.TimeoutExpired:
        r = subprocess.CompletedProcess([], -999, b'', b'timeout')
    res = {'exit': r.returncode, 'seconds': round(time.monotonic() - t, 3),
           'stderr': r.stderr.decode(errors='replace')[-400:]}
    if r.returncode == 0 and os.path.isfile(out) and not r.stdout:
        return dict(res, status='ok')
    if r.returncode == EXIT_UNSUPPORTED and not os.path.exists(out):
        return dict(res, status='refused', reason=(res['stderr'].strip().splitlines() or [''])[-1])
    if r.returncode == 1 and not os.path.exists(out) and any(d in res['stderr'] for d in REJECTED_DEFECTS):
        return dict(res, status='refused', reason='rejected: ' + (res['stderr'].strip().splitlines() or [''])[-1])
    return dict(res, status='failed', reason=f'exit {r.returncode}' + (', output left' if os.path.exists(out) else ''))


def written(args, out):
    """The output's streams in order, (kind, codec), and its video and audio stream indices."""
    _, d = jrun([args.ffprobe, '-v', 'error', '-show_entries', 'stream=index,codec_type,codec_name', '-of', 'json',
                 out])
    streams = (d or {}).get('streams', [])
    kinds = [(s.get('codec_type'), s.get('codec_name')) for s in streams]
    video = next((s['index'] for s in streams if s.get('codec_type') == 'video'), None)
    return kinds, video, [s['index'] for s in streams if s.get('codec_type') == 'audio']


def audio_origin(args, src, out, index):
    """The source audio stream whose packets an output audio stream carries (same data, packet by packet)."""
    _, ours = packets(args.ffprobe, out, str(index), data_hash=True)
    mine = [p.get('data_hash') for p in ours or []]
    for s in src.streams:
        if s.get('codec_type') == 'audio':
            _, theirs = src.audio(s['index'])
            if mine and [p.get('data_hash') for p in theirs or []] == mine:
                return s['index']
    return None


def check_compat(args, path, key, cls, tmp):
    """B9: one `coreav ffmpeg` command line on one fixture, each arm against FFmpeg running the same line."""
    src = Source(args, path)
    src.remux(tmp)
    res = {'config': key, 'class': cls, 'command': COMPAT_COMMANDS[key], 'arms': {}, 'has_audio': False,
           'b2_known_miss': os.path.basename(path) in B2_KNOWN_MISSES}
    base = tmp / 'baseline.mp4'
    cmd = [args.ffmpeg, '-nostdin'] + compat_words(key, path, base)
    t = time.monotonic()
    r = run(cmd)
    res['baseline'] = {'exit': r.returncode, 'seconds': round(time.monotonic() - t, 3),
                       'status': 'ok' if r.returncode == 0 and base.is_file() else 'failed'}
    if res['baseline']['status'] != 'ok':
        res['baseline']['reason'] = r.stderr.decode(errors='replace')[-300:]
    else:
        kinds, bv, ba = written(args, base)
        res['baseline'].update(streams=kinds, audio_from=[audio_origin(args, src, base, a) for a in ba])
        res['has_audio'] = bool(ba)
    for name, arm_env in args.arms:
        out = tmp / f'{name}.mp4'
        arm = res['arms'][name] = compat_run(args, dict(arm_env), key, path, out)
        if arm['status'] != 'ok':
            continue
        kinds, vi, ai = written(args, out)
        arm['streams'] = kinds
        src_audio = [audio_origin(args, src, out, a) for a in ai]
        if vi is None or None in src_audio:
            arm.update(status='failed', reason='no video stream, or an audio stream not from the source')
            out.unlink(missing_ok=True)
            continue
        layout = {'video': vi, 'audio': ai, 'videos': sum(1 for k, _ in kinds if k == 'video'),
                  'streams': len(kinds)}
        video = default_video(src.streams)['index']
        expected = res['baseline'].get('audio_from') if res['baseline']['status'] == 'ok' else None
        arm.update(judge_output(args, src, out, layout, video, src_audio, tmp,
                                expected_audio=expected if expected is not None else []))
        if res['baseline']['status'] == 'ok' and kinds != res['baseline']['streams']:
            arm['B1'] = dict(arm['B1'], ok=False, reason='the streams written differ from FFmpeg\'s')
        elif res['baseline']['status'] == 'ok' and (display_matrix(args, out, vi)
                                                    != display_matrix(args, base, written(args, base)[1])):
            arm['B1'] = dict(arm['B1'], ok=False, reason='the display matrix differs from FFmpeg\'s')
        w, h = arm['video'].get('width'), arm['video'].get('height')
        if res['baseline']['status'] != 'ok' or not (w and h):
            arm['Q1'] = arm['Q2'] = {'ok': False, 'reason': 'no baseline or no output size'}
        else:
            q1 = psnr(args.ffmpeg, out, vi, path, video, w, h)
            bv = written(args, base)[1]
            bq = res['baseline'].get('Q1')
            if bq is None:
                bw, bh = probe_size(args, base, bv)
                bq = res['baseline']['Q1'] = psnr(args.ffmpeg, base, bv, path, video, bw, bh)
            arm['Q1'] = ({'ok': True, 'delta': 0.0 if q1['average'] == bq['average'] else q1['average'] - bq['average']}
                         if q1['ok'] and bq['ok'] else {'ok': False, 'reason': 'a PSNR failed'})
            arm['Q2'] = psnr(args.ffmpeg, out, vi, base, bv, w, h)
            bs, cs = video_bytes_and_rate(args, base, bv)[0], arm['video'].get('bytes')
            arm['size_ratio'] = cs / bs if bs and cs else None
        out.unlink(missing_ok=True)
    base.unlink(missing_ok=True)
    res['complete'] = True
    return res


def display_matrix(args, path, index):
    """The display matrices `ffprobe` lists for a stream's side data (none for the identity)."""
    _, d = jrun([args.ffprobe, '-v', 'error', '-select_streams', str(index), '-show_entries',
                 'stream_side_data=displaymatrix', '-of', 'json', path])
    s = ((d or {}).get('streams') or [{}])[0]
    return [x.get('displaymatrix') for x in s.get('side_data_list', [])]


def export_evidence(args, out, run_info):
    """B9's per-call evidence with absolute paths taken out, in OUT/evidence: calls.jsonl (one row per command
    line, file and arm, the baseline included), manifest.json (the command lines, the arms, the programs' and the
    fixtures' SHA-256) and results.tar.gz (run.json, summary.json and results/, from which `--summarize` recomputes
    the summary)."""
    dirs = [args.fixtures, args.out, args.tmp, os.path.dirname(os.path.abspath(args.coreav)),
            os.path.dirname(shutil.which(args.ffmpeg) or args.ffmpeg),
            os.path.dirname(shutil.which(args.ffprobe) or args.ffprobe), os.path.expanduser('~'),
            tempfile.gettempdir()]
    dirs += [os.path.dirname(v) for _, words in args.arms for _, v in words if os.path.isabs(v)]
    roots = sorted({str(pathlib.Path(d).resolve()) for d in dirs if d} | {os.path.abspath(d) for d in dirs if d},
                   key=len, reverse=True)

    def clean(text):
        for r in roots:
            text = text.replace(r + os.sep, '').replace(r, '')
        return text

    out = pathlib.Path(out)
    ev = out / 'evidence'
    ev.mkdir(parents=True, exist_ok=True)
    rows = []
    with tarfile.open(ev / 'results.tar.gz', 'w:gz') as tar:
        for p in [p for p in (out / 'run.json', out / 'summary.json') if p.exists()] + \
                sorted((out / 'results').rglob('*.json')):
            data = clean(p.read_text()).encode()
            info = tarfile.TarInfo(str(p.relative_to(out)))
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
            if p.parent.parent.name != 'results':
                continue
            r = json.loads(p.read_text())
            for arm, a in [('baseline', r.get('baseline', {}))] + list(r.get('arms', {}).items()):
                rows.append({'config': r.get('config'), 'file': r.get('file'), 'class': r.get('class'), 'arm': arm,
                             'status': a.get('status'), 'exit': a.get('exit'),
                             'reason': clean(str(a.get('reason') or '')) or None,
                             **{b: (a.get(b) or {}).get('ok') for b in ('B1', 'B2', 'B3')},
                             'Q1_delta_db': (a.get('Q1') or {}).get('delta'),
                             'Q2_db': (a.get('Q2') or {}).get('average'), 'size_ratio': a.get('size_ratio'),
                             'b2_known_miss': r.get('b2_known_miss')})
    (ev / 'calls.jsonl').write_text(''.join(json.dumps(x, sort_keys=True) + '\n' for x in rows))
    fixtures = sorted(p for p in pathlib.Path(args.fixtures).iterdir() if p.is_file())
    _write(ev / 'manifest.json', {
        'commit': os.environ.get('GITHUB_SHA'), 'commands': COMPAT_COMMANDS,
        'arms': {n: [f'{var}={os.path.basename(v)}' for var, v in w] for n, w in args.arms},
        'tools': run_info['tools'], 'ffmpeg_version': run_info['ffmpeg_version'],
        'fixtures': {p.name: sha256_file(p) for p in fixtures}})


def probe_size(args, path, index):
    _, d = jrun([args.ffprobe, '-v', 'error', '-select_streams', str(index), '-show_entries', 'stream=width,height',
                 '-of', 'json', path])
    s = ((d or {}).get('streams') or [{}])[0]
    return s.get('width'), s.get('height')


def bitrate_check(args, name, path, tmp, video, audio):
    stb, sframes = Source(args, path).video_frames(video)
    first, last = first_time(stb, sframes), first_time(stb, list(reversed(sframes or [])))
    if first is None or last is None or float(last - first) < BITRATE_MIN_SECONDS:
        return {'status': 'apart', 'reason': f'video shorter than {BITRATE_MIN_SECONDS} s'}
    arm_args = dict(args.arms)[name]
    out, base = tmp / f'{name}.br.mp4', tmp / 'baseline.br.mp4'
    try:
        t = transcode(args, arm_args, path, out, 'default', bitrate=True)
        if t['status'] != 'ok':
            return {'status': 'failed', 'ok': False, 'coreav': {k: t.get(k) for k in ('status', 'reason', 'error')}}
        if not base.exists():
            b = baseline(args, path, base, video, audio, 'default', bitrate=True)
            if b['status'] != 'ok':
                return {'status': 'baseline_failed'}
        _, ours = video_bytes_and_rate(args, out, output_layout(t['summary'])['video'])
        _, theirs = video_bytes_and_rate(args, base, 0)
        if not ours or not theirs:
            return {'status': 'failed', 'ok': False, 'reason': 'no bit rate'}
        ratio = ours / theirs
        return {'status': 'ok', 'ok': abs(ratio - 1) <= BITRATE_TOLERANCE, 'coreav_bps': round(ours),
                'baseline_bps': round(theirs), 'ratio': ratio}
    finally:
        out.unlink(missing_ok=True)


# --- inputs ----------------------------------------------------------------------------------------------------

def fixture_inputs(args):
    items = []
    for path in sorted(pathlib.Path(args.fixtures).iterdir()):
        if path.suffix.lstrip('.').lower() not in FIXTURE_EXTS:
            continue
        p = probe(args.ffprobe, path)
        if p.get('status') != 'ok' or p['codec'] not in CLASSES:
            continue
        # The scaled configuration is a corpus run (docs/design/transcode.md); on fixtures only for debugging.
        configs = ['default'] + (['scaled'] if args.debug_scale_all else [])
        if args.compat:
            configs = list(COMPAT_COMMANDS)
        items.append({'key': path.name, 'path': str(path), 'class': p['codec'], 'configs': configs})
    return items


def manifest_candidates(classes, holdout=None):
    files = json.loads(MANIFEST.read_text())['files']
    by_class = collections.defaultdict(list)
    for e in files:
        if e['set'] != 'real' or not any(t in e.get('tags', ()) for t in CONTAINER_TAGS):
            continue
        if holdout is not None and bool(e.get('holdout')) != holdout:
            continue
        for t in e.get('tags', ()):
            if t.startswith('v:') and t[2:] in classes:
                by_class[t[2:]].append(e)
    return by_class


def r2_credentials():
    creds = {'access': os.environ.get('R2_ACCESS_KEY_ID'), 'secret': os.environ.get('R2_SECRET_ACCESS_KEY'),
             'endpoint': os.environ.get('R2_ENDPOINT')}
    if not all(creds.values()):
        raise SystemExit('R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_ENDPOINT are required to fetch corpus files')
    return creds


_keys = None


def object_key(sha):
    """The R2 object of a corpus file, from the manifest: selections and results name files by SHA-256 only and
    keep no object key, so nothing written for the evidence holds one."""
    global _keys
    if _keys is None:
        _keys = {e['sha256']: e['key'] for e in json.loads(MANIFEST.read_text())['files']}
    return _keys[sha]


def download(entry, dest, part=8 << 20, workers=8):
    """One corpus object, by parallel ranged GETs, checked against its SHA-256. True when the file is right."""
    sys.path.insert(0, str(ROOT / 'tools' / 'stage3-report'))
    import r2pool  # noqa: E402
    dest = pathlib.Path(dest)
    key = object_key(entry['sha256'])
    if dest.exists() and dest.stat().st_size == entry['size'] and sha256_file(dest) == entry['sha256']:
        return True
    size = entry['size']
    pool = r2pool.Pool(r2_credentials(), workers, r2pool.Limits(requests=20000, bytes_=2 * size + (64 << 20)),
                       timeout=300)
    tmp = dest.with_name(dest.name + '.part')
    try:
        _, etag = pool.head(key)
        fd = os.open(tmp, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o600)
        try:
            os.ftruncate(fd, size)
            parts = [(o, min(part, size - o)) for o in range(0, size, part)]
            with concurrent.futures.ThreadPoolExecutor(workers) as ex:
                got = sum(ex.map(lambda p: pool.get(key, size, etag, p[0], p[1],
                                                    sink=lambda pos, c: os.pwrite(fd, c, pos)), parts))
        finally:
            os.close(fd)
    except Exception as e:  # noqa: BLE001 - network faults are reported without URLs or keys
        tmp.unlink(missing_ok=True)
        log(f'download failed for {entry["sha256"][:12]}: {type(e).__name__}')
        return False
    finally:
        pool.close()
    if got != size or sha256_file(tmp) != entry['sha256']:
        tmp.unlink(missing_ok=True)
        return False
    tmp.rename(dest)
    return True


def local_path(work, entry):
    return pathlib.Path(work) / 'files' / f'{entry["sha256"]}.{entry["ext"]}'


def make_prober(args, keep):
    """probe_of(entry) with a cache in WORK/probe/: downloads the file when needed, deletes it again unless keep."""
    cache = pathlib.Path(args.work) / 'probe'
    cache.mkdir(parents=True, exist_ok=True)

    def probe_of(entry):
        f = cache / f'{entry["sha256"]}.json'
        if f.exists():
            return json.loads(f.read_text())
        path = local_path(args.work, entry)
        path.parent.mkdir(parents=True, exist_ok=True)
        had = path.exists()
        if not download(entry, path):
            # Never frozen into a selection: a file that could not be fetched would be left out for good.
            raise SystemExit(f'selection stopped: {entry["sha256"][:12]} could not be fetched; run it again')
        p = probe(args.ffprobe, path)
        f.write_text(json.dumps(p))
        if not keep and not had:
            path.unlink(missing_ok=True)
        return p
    return probe_of


def corpus_selection(args):
    """OUT/selection.json: computed once, before any run, then reused."""
    sel_path = pathlib.Path(args.out) / 'selection.json'
    params = {'salt': SALT, 'max_files': args.max_files, 'budget': BUDGET, 'max_seconds': MAX_SECONDS,
              'scaled_files': SCALED_FILES, 'scaled_taller_than': SCALED_TALLER_THAN, 'scaled_over': 'all classes',
              'classes': list(args.classes)}
    if sel_path.exists():
        sel = json.loads(sel_path.read_text())
        if sel['params'] != params:
            raise SystemExit('OUT/selection.json was made with other parameters; use a new --out')
        return sel
    probe_of = make_prober(args, args.keep_files)
    by_class = manifest_candidates(args.classes)
    sel = {'params': params, 'official': args.max_files == MAX_FILES and set(args.classes) == set(CLASSES),
           'classes': {}}
    taller = []
    for cls in args.classes:
        chosen, apart, _ = select_class(by_class.get(cls, []), probe_of, cls, max_files=args.max_files)
        taller += [e['sha256'] for e, p in chosen if p['height'] > SCALED_TALLER_THAN]
        sel['classes'][cls] = {
            'files': [{'sha256': e['sha256'], 'ext': e['ext'], 'size': e['size'],
                       'holdout': e.get('holdout', False), 'pixel_frames': p['pixel_frames'],
                       'seconds': p['seconds'], 'height': p['height']} for e, p in chosen],
            'pixel_frames': sum(p['pixel_frames'] for _, p in chosen), 'apart': dict(apart)}
        log(f'selection {cls}: {len(chosen)} files, '
            f'{sel["classes"][cls]["pixel_frames"]:.3g} pixel-frames, apart {dict(apart)}')
    # The scaled configuration: the first 100 selected files over all classes, in the order of the same hash,
    # whose video is taller than 360 rows.
    sel['scaled'] = sorted(taller, key=salted)[:SCALED_FILES]
    log(f'scaled configuration: {len(sel["scaled"])} files')
    tmp = sel_path.with_suffix('.part')
    tmp.write_text(json.dumps(sel, indent=1))
    tmp.rename(sel_path)
    return sel


def corpus_inputs(args, sel):
    items = []
    scaled = set(sel['scaled'])
    for cls, c in sel['classes'].items():
        for f in c['files']:
            items.append({'key': f['sha256'], 'entry': f, 'class': cls,
                          'configs': ['default'] + (['scaled'] if f['sha256'] in scaled or args.debug_scale_all
                                                    else [])})
    return items


def result_path(out, config, key):
    return pathlib.Path(out) / 'results' / config / f'{key}.json'


def process(args, item):
    todo = [c for c in item['configs'] if not _complete(result_path(args.out, c, item['key']))]
    if not todo:
        return
    path = pathlib.Path(item['path']) if 'path' in item else local_path(args.work, item['entry'])
    fetched = False
    if 'entry' in item and not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        if not download(item['entry'], path):
            for c in todo:
                _write(result_path(args.out, c, item['key']),
                       {'config': c, 'class': item['class'], 'download_failed': True, 'complete': False})
            return
        fetched = True
    try:
        for config in todo:
            with tempfile.TemporaryDirectory(dir=args.tmp) as d:
                t = time.monotonic()
                check = check_compat if args.compat else check_input
                res = check(args, path, config, item['class'], pathlib.Path(d))
                res['seconds'] = round(time.monotonic() - t, 1)
                res['file'] = item['key']
                _write(result_path(args.out, config, item['key']), res)
                log(f'{config} {item["class"]} {item["key"][:40]}: {brief(res)}')
    finally:
        if fetched and not args.keep_files:
            path.unlink(missing_ok=True)


def brief(res):
    parts = []
    for name, a in res['arms'].items():
        if a['status'] != 'ok':
            parts.append(f'{name} {a["status"]} ({a.get("reason")})')
            continue
        bits = [f'B{n}={"ok" if (a.get(f"B{n}") or {"ok": True})["ok"] else "FAIL"}' for n in (1, 2, 3)]
        q1 = a.get('Q1', {})
        q2 = a.get('Q2', {})
        bits.append(f'dPSNR={q1.get("delta"):.3f}' if q1.get('ok') else 'Q1=FAIL')
        bits.append(f'Q2={q2.get("average")}' if q2.get('ok') else 'Q2=FAIL')
        if a.get('size_ratio'):
            bits.append(f'size={a["size_ratio"]:.4f}')
        if a.get('bitrate', {}).get('status') == 'ok':
            bits.append(f'br={a["bitrate"]["ratio"]:.4f}')
        parts.append(f'{name} ' + ' '.join(bits))
    if res.get('baseline', {}).get('status') == 'failed':
        parts.append('baseline failed')
    return '; '.join(parts)


def _complete(path):
    try:
        return json.loads(path.read_text()).get('complete', False)
    except (OSError, ValueError):
        return False


def _write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + '.part')
    tmp.write_text(json.dumps(value, indent=1, default=str))
    tmp.rename(path)


# --- the summary -----------------------------------------------------------------------------------------------

def share(values, floor):
    return sum(1 for v in values if v >= floor) / len(values) if values else None


def summarize(out, fixtures, expected, arms, compat=False):
    """Per configuration, arm and class (and over all classes): each bar's counts and verdict, over the inputs
    of the run only (`expected`: {config: {file: class}}). An input without a complete result is counted as
    missing, and a class with one missing gets no verdict."""
    rows = collections.defaultdict(list)
    missing = collections.defaultdict(collections.Counter)
    for config, files in expected.items():
        for key, cls in files.items():
            f = result_path(out, config, key)
            r = json.loads(f.read_text()) if f.exists() else {}
            if not r.get('complete'):
                missing[config][cls] += 1
                missing[config]['all'] += 1
                continue
            rows[config].append(r)
    report = {'mode': 'fixtures' if fixtures else 'corpus', 'configs': {},
              'missing': {c: dict(m) for c, m in missing.items() if m}}
    for config, files in expected.items():
        results = rows[config]
        classes = sorted(set(files.values()), key=CLASSES.index) + ['all']
        cfg = report['configs'][config] = {}
        # The baseline's own B1 to B3, for information: it is the command the note times against.
        for arm in list(arms) + ['baseline']:
            per = cfg[arm] = {}
            groups = {c: [r for r in results if c == 'all' or r['class'] == c] for c in classes}
            for cls, rs in groups.items():
                # Fixtures: 100%. Corpus: 99% over all classes together, and 95% in every class.
                bar = 1.0 if fixtures else (CORPUS_SHARE if cls == 'all' else CLASS_SHARE)
                per[cls] = class_verdict(rs, arm, bar, fixtures, compat)
                if missing[config][cls]:
                    per[cls]['missing'] = missing[config][cls]
                    for v in per[cls].values():
                        if isinstance(v, dict) and 'met' in v:
                            v['met'] = None
            if arm != 'baseline' and not fixtures:
                every = [v for c, v in per.items() if c != 'all']
                for b in ('B2', 'B3'):
                    pooled = per['all'][b]
                    pooled['met_with_every_class'] = (None if pooled['met'] is None
                                                      or any(v[b]['met'] is None for v in every) else
                                                      pooled['met'] and all(v[b]['met'] for v in every))
        # Refusals of files whose baseline succeeded, listed by file; the baseline's own B2 and B3 misses.
        report.setdefault('refused_with_baseline_ok', {})[config] = sorted(
            {(r['file'], a.get('reason'), str((a.get('error') or {}).get('message'))[:120])
             for r in results for a in r.get('arms', {}).values()
             if a.get('status') == 'refused' and r.get('baseline', {}).get('status') == 'ok'})
        report.setdefault('baseline_misses', {})[config] = {
            b: sorted(r['file'] for r in results if r.get('baseline', {}).get('status') == 'ok'
                      and r['baseline'].get(b) and not r['baseline'][b]['ok']) for b in ('B2', 'B3')}
    return report


def class_verdict(rs, arm, b23_bar, fixtures, compat=False):
    apart = collections.Counter()
    b1 = []
    native = []
    native_apart = collections.Counter()
    b2, b3, q1, q2, sizes, br = [], [], [], [], [], []
    files_below = []
    for r in rs:
        if r.get('download_failed'):
            apart['download failed'] += 1
            continue
        a = r.get('baseline') if arm == 'baseline' else r.get('arms', {}).get(arm)
        if a is None or a.get('maps_from') == 'probe' or (arm == 'baseline' and 'command' in r):
            # A baseline row of a file CoreAV refused, or of a B9 command line (FFmpeg's own run, not judged): only
            # the list of refusals uses it.
            continue
        if a['status'] == 'refused':
            apart[f'refused: {a["reason"]}: {str((a.get("error") or {}).get("message"))[:60]}'] += 1
            continue
        if a['status'] != 'ok':
            # Any other failure counts against every bar.
            b1.append(False)
            b2.append(False)
            if r.get('has_audio'):
                b3.append(False)
            q1.append(-math.inf)
            q2.append(-math.inf)
            files_below.append((r['file'], 'failed'))
            continue
        b1.append(a['B1']['ok'])
        n = a['B1'].get('native')
        if n:
            if n['status'] == 'apart':
                native_apart[n['reason']] += 1
            else:
                native.append(n.get('ok', False))
        if r.get('b2_known_miss'):
            apart['B2 set apart: a known miss of coreav transcode'] += 1
        else:
            b2.append(a['B2']['ok'])
        if 'B3' in a:
            b3.append(a['B3']['ok'])
        if arm == 'baseline':
            pass
        elif r.get('baseline', {}).get('status') != 'ok' and not compat:
            # B9 compares with FFmpeg running the same command line, so there a missing comparison fails Q1 and Q2.
            apart['B4: baseline failed'] += 1
        else:
            d = a['Q1'].get('delta') if a.get('Q1', {}).get('ok') else -math.inf
            p = a['Q2'].get('average') if a.get('Q2', {}).get('ok') else -math.inf
            q1.append(d)
            q2.append(p)
            if a.get('size_ratio') is not None:
                sizes.append(a['size_ratio'])
            if d < Q1_FLOOR or p < Q2_FLOOR:
                files_below.append((r['file'], f'dPSNR {d}, Q2 {p}'))
        b = a.get('bitrate')
        if b and b.get('status') in ('ok', 'failed'):
            br.append(b.get('ok', False))
            if not b.get('ok'):
                files_below.append((r['file'], f'bit rate ratio {b.get("ratio")}'))

    def frac(xs):
        return {'pass': sum(xs), 'judged': len(xs), 'share': (sum(xs) / len(xs)) if xs else None}

    def met(xs, bar):
        return None if not xs else (sum(xs) / len(xs)) >= bar

    v = {'files': len(rs), 'apart': dict(apart)}
    v['B1'] = dict(frac(b1), met=met(b1, 1.0))
    v['B1_native'] = dict(frac(native), met=met(native, 1.0), apart=dict(native_apart))
    v['B2'] = dict(frac(b2), bar=b23_bar, met=met(b2, b23_bar))
    v['B3'] = dict(frac(b3), bar=b23_bar, met=met(b3, b23_bar))
    q1m = statistics.median(q1) if q1 else None
    q2m = statistics.median(q2) if q2 else None
    sm = statistics.median(sizes) if sizes else None
    v['Q1'] = {'judged': len(q1), 'median_delta_db': q1m, 'share_at_least_-0.50': share(q1, Q1_FLOOR),
               'median_size_ratio': sm,
               'met': None if not q1 else (q1m >= Q1_MEDIAN and share(q1, Q1_FLOOR) >= SHARE
                                           and sm is not None and SIZE_RATIO[0] <= sm <= SIZE_RATIO[1])}
    v['Q2'] = {'judged': len(q2), 'median_db': q2m, 'share_at_least_30': share(q2, Q2_FLOOR),
               'met': None if not q2 else (q2m >= Q2_MEDIAN and share(q2, Q2_FLOOR) >= SHARE)}
    if compat:
        # B9: Q1 and Q2 for every fixture a command line runs on; the medians and the size ratio are B4's.
        v['Q1']['met'] = share(q1, Q1_FLOOR) == 1.0 if q1 else None
        v['Q2']['met'] = share(q2, Q2_FLOOR) == 1.0 if q2 else None
        v['below_floor'] = files_below
    elif fixtures:
        # Fixtures: Q1 and Q2 for every fixture, and the bit-rate check.
        v['Q1']['met'] = v['Q1']['met'] and share(q1, Q1_FLOOR) == 1.0 if q1 else None
        v['Q2']['met'] = v['Q2']['met'] and share(q2, Q2_FLOOR) == 1.0 if q2 else None
        v['bitrate'] = dict(frac(br), met=met(br, 1.0))
        v['below_floor'] = files_below
    return v


def table(report):
    lines = []
    for config, rows in report.get('refused_with_baseline_ok', {}).items():
        for row in rows:
            lines.append(f'refused by CoreAV, transcoded by the baseline ({config}): {row[0]} {row[1]}: {row[2]}')
    for config, misses in report.get('baseline_misses', {}).items():
        for b, files in misses.items():
            if files:
                lines.append(f'the baseline misses {b} ({config}, {len(files)}): {" ".join(files)}')
    for config, arms in report['configs'].items():
        for arm, classes in arms.items():
            lines.append(f'== {config} / {arm}')
            lines.append(f'{"class":8} {"files":>5} {"B1":>9} {"B2":>9} {"B3":>9} {"dPSNR":>8} {">=-0.5":>7} '
                         f'{"size":>7} {"Q2":>7} {">=30":>6}  apart')
            for cls in list(CLASSES) + ['all']:
                v = classes.get(cls)
                if not v:
                    continue

                def f(b):
                    return f'{b["pass"]}/{b["judged"]}' + ('' if b['met'] is None else ('+' if b['met'] else '!'))

                def n(x, d=3):
                    return '-' if x is None else f'{x:.{d}f}'
                q1, q2 = v['Q1'], v['Q2']
                lines.append(f'{cls:8} {v["files"]:>5} {f(v["B1"]):>9} {f(v["B2"]):>9} {f(v["B3"]):>9} '
                             f'{n(q1["median_delta_db"]):>8} {n(q1["share_at_least_-0.50"], 2):>7} '
                             f'{n(q1["median_size_ratio"], 4):>7} {n(q2["median_db"], 2):>7} '
                             f'{n(q2["share_at_least_30"], 2):>6}  {v["apart"] or ""}')
    return '\n'.join(lines)


def tool_hashes(args):
    """SHA-256 of every program the run used: coreav, ffmpeg, ffprobe and the arms' encoder programs."""
    files = {'coreav': args.coreav, 'ffmpeg': args.ffmpeg, 'ffprobe': args.ffprobe}
    for name, words in args.arms:
        if args.compat:
            for var, value in words:
                if var in ('COREAV_X264', 'COREAV_FFMPEG') and value:
                    files[f'{name} {var}'] = value
            continue
        for i, w in enumerate(words[:-1]):
            if w in ('--x264', '--ffmpeg'):
                files[f'{name}{w}'] = words[i + 1]
    if args.decode_oracle:
        files['decode-oracle'] = args.decode_oracle
    out = {}
    for k, p in files.items():
        p = shutil.which(p) or p
        out[k] = {'path': os.path.basename(p), 'sha256': sha256_file(p) if os.path.isfile(p) else None}
    return out


def parse_arms(values, parser, compat=False):
    """NAME=ARGS: the words added to every `coreav transcode` of the arm; with --compat, VAR=VALUE words, the
    environment of every `coreav ffmpeg` of the arm (which encoder program it finds)."""
    arms = []
    for v in values or []:
        name, _, rest = v.partition('=')
        if not re.fullmatch(r'[A-Za-z0-9_-]+', name) or name == 'baseline' or name in dict(arms):
            parser.error(f'--arm {v!r}: NAME=ARGS, NAME distinct, of letters, digits, - and _')
        words = shlex.split(rest)
        if compat:
            if not all(re.fullmatch(r'[A-Z][A-Z0-9_]*=.*', w) for w in words):
                parser.error(f'--arm {v!r}: with --compat, ARGS are VAR=VALUE environment words')
            words = [tuple(w.split('=', 1)) for w in words]
        arms.append((name, words))
    return arms


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('--coreav')
    p.add_argument('--arm', action='append', metavar='NAME=ARGS')
    p.add_argument('--ffmpeg', default=shutil.which('ffmpeg'))
    p.add_argument('--ffprobe', default=shutil.which('ffprobe'))
    p.add_argument('--decode-oracle', help='a tools/decode-oracle-style program for B1 decoder conformance')
    p.add_argument('--decode-oracle-args', default='', help='extra words for it (for example "--backend native")')
    mode = p.add_mutually_exclusive_group(required=True)
    mode.add_argument('--fixtures', metavar='DIR')
    mode.add_argument('--corpus', action='store_true')
    mode.add_argument('--summarize', action='store_true', help='only summarize OUT/results')
    mode.add_argument('--merge', nargs='+', metavar='SHARD_EVIDENCE',
                      help='judge the evidence directories of shards 1 to N of one --compat run as one run')
    p.add_argument('--out', required=True)
    p.add_argument('--work', help='corpus downloads (default OUT/work)')
    p.add_argument('--tmp', default=None, help='where outputs are written (tmpfs if it is large enough)')
    p.add_argument('--jobs', type=int, default=1)
    p.add_argument('--shard', metavar='K/N', help='with --fixtures: the K-th of every N inputs in name order')
    p.add_argument('--classes', default=','.join(CLASSES))
    p.add_argument('--keep-files', action='store_true')
    p.add_argument('--only', help='comma-separated file names or SHA-256 prefixes')
    p.add_argument('--max-files', type=int, default=MAX_FILES,
                   help=f'debug: files a class (default {MAX_FILES}; a selection with another value is not official)')
    p.add_argument('--preflight', default=PREFLIGHT,
                   help=f'the fixture each arm must transcode before the run (default {PREFLIGHT})')
    p.add_argument('--compat', action='store_true',
                   help='with --fixtures: bar B9, the coreav ffmpeg command lines of COMPAT_COMMANDS')
    p.add_argument('--debug-scale-all', action='store_true',
                   help='debug: run the scaled configuration on every input, whatever its height')
    args = p.parse_args()
    args.classes = [c for c in args.classes.split(',') if c]
    if not set(args.classes) <= set(CLASSES):
        p.error(f'--classes: {CLASSES}')
    if args.summarize:
        info = json.loads((pathlib.Path(args.out) / 'run.json').read_text())
        rep = summarize(args.out, info['mode'] == 'fixtures', info['expected'], list(info['arms']),
                        info.get('compat') is not None)
        rep['run'] = info
        print(table(rep))
        return verdict_status(rep)
    if args.merge:
        return merge(args)
    shard = re.fullmatch(r'([1-9][0-9]*)/([1-9][0-9]*)', args.shard or '')
    if args.shard and (not shard or int(shard[1]) > int(shard[2]) or not args.fixtures):
        p.error('--shard K/N: 1 <= K <= N, with --fixtures')
    args.work = args.work or str(pathlib.Path(args.out) / 'work')
    if args.compat and not args.fixtures:
        p.error('--compat runs on --fixtures')
    args.arms = parse_arms(args.arm, p, args.compat)
    if not args.coreav or not args.arms:
        p.error('--coreav and at least one --arm are required')
    if not args.ffmpeg or not args.ffprobe:
        p.error('ffmpeg and ffprobe are required (--ffmpeg, --ffprobe)')
    global ENV
    ENV = child_env(args.ffmpeg)
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    run_info = {'mode': 'fixtures' if args.fixtures else 'corpus', 'preset': PRESET, 'crf': CRF,
                'compat': COMPAT_COMMANDS if args.compat else None,
                'arms': {n: a for n, a in args.arms}, 'tools': tool_hashes(args),
                'ffmpeg_version': run([args.ffmpeg, '-version']).stdout.decode(errors='replace').splitlines()[:1],
                'debug_scale_all': args.debug_scale_all, 'max_files': args.max_files}
    preflight(args)
    if args.fixtures:
        items = fixture_inputs(args)
    else:
        items = corpus_inputs(args, corpus_selection(args))
    items = [i for i in items if i['class'] in args.classes]
    if args.only:
        keys = args.only.split(',')
        items = [i for i in items if any(i['key'].startswith(k) for k in keys)]
        run_info['only'] = keys
    if shard:
        k, n = int(shard[1]), int(shard[2])
        items = items[k - 1::n]
        run_info['shard'] = [k, n]
    expected = {}
    for i in items:
        for c in i['configs']:
            expected.setdefault(c, {})[i['key']] = i['class']
    if not args.compat:
        expected.setdefault('default', {})
    if not args.fixtures and not args.only:
        # A class of the selection without files has no judged file: it is reported, and fails the run.
        run_info['empty_classes'] = [c for c in args.classes if c not in expected['default'].values()]
    run_info['expected'] = expected
    _write(out / 'run.json', run_info)
    log(f'{len(items)} inputs, {sum(len(i["configs"]) for i in items)} runs')
    with concurrent.futures.ThreadPoolExecutor(max(1, args.jobs)) as ex:
        for fut in [ex.submit(process, args, i) for i in items]:
            fut.result()
    if shard:
        if args.compat:
            export_evidence(args, out, run_info)
        missing = [f'{c} {key}' for c, files in expected.items() for key in files
                   if not _complete(result_path(out, c, key))]
        for m in missing:
            print('FAIL: no complete result:', m)
        log(f'shard {args.shard}: {len(items)} inputs; --merge judges the bars over every shard')
        return 1 if missing else 0
    rep = summarize(args.out, bool(args.fixtures), expected, [n for n, _ in args.arms], args.compat)
    rep['run'] = run_info
    if not args.fixtures:
        rep['selection_apart'] = {c: v['apart'] for c, v in corpus_selection(args)['classes'].items()}
    _write(out / 'summary.json', rep)
    if args.compat:
        export_evidence(args, out, run_info)
    print(table(rep))
    return verdict_status(rep)


def merge(args):
    """--merge: the evidence of shards 1 to N of one run, summarized and judged as one run."""
    out = pathlib.Path(args.out)
    if (out / 'results').exists():
        raise SystemExit(f'--merge: {out} already holds results')
    infos, manifests, rows = {}, [], []
    for d in map(pathlib.Path, args.merge):
        manifests.append(json.loads((d / 'manifest.json').read_text()))
        rows += [json.loads(line) for line in (d / 'calls.jsonl').read_text().splitlines()]
        with tarfile.open(d / 'results.tar.gz') as tar:
            for m in tar.getmembers():
                parts = pathlib.PurePosixPath(m.name).parts
                if not m.isfile() or '..' in parts or not (parts == ('run.json',) or
                                                           (len(parts) == 3 and parts[0] == 'results')):
                    raise SystemExit(f'--merge: {d}: unexpected member {m.name!r}')
                data = tar.extractfile(m).read()
                if parts == ('run.json',):
                    info = json.loads(data)
                    k = (info.get('shard') or [None])[0]
                    if k is None or k in infos:
                        raise SystemExit(f'--merge: {d} is no shard, or a shard given twice')
                    infos[k] = info
                    continue
                path = out.joinpath(*parts)
                if path.exists():
                    raise SystemExit(f'--merge: {m.name} is in two shards')
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
    counts = {info['shard'][1] for info in infos.values()}
    if len(counts) != 1 or sorted(infos) != list(range(1, counts.pop() + 1)):
        raise SystemExit(f'--merge: shards {sorted(infos)} are not shards 1 to N of one run, each once')

    fixtures = [m.get('fixtures') or {} for m in manifests]
    differ = sorted(n for n in set().union(*fixtures) if any(f.get(n) != fixtures[0].get(n) for f in fixtures))
    if differ:
        raise SystemExit(f'--merge: the shards ran on different bytes of {", ".join(differ)}: '
                         'give every shard the one fixture set of the run')

    def common(d):
        return {k: v for k, v in d.items() if k not in ('shard', 'expected', 'tools')}
    if any(common(i) != common(infos[1]) for i in infos.values()) or \
            any(common(m) != common(manifests[0]) for m in manifests):
        raise SystemExit('--merge: the shards ran different command lines, arms or fixtures')
    tools = [infos[k]['tools'] for k in sorted(infos)]
    expected = {}
    for k in sorted(infos):
        for config, files in infos[k]['expected'].items():
            if set(files) & set(expected.get(config, {})):
                raise SystemExit(f'--merge: {config}: an input is in two shards')
            expected.setdefault(config, {}).update(files)
    run_info = common(infos[1]) | {'tools': tools[0] if all(t == tools[0] for t in tools) else tools,
                                   'expected': expected}
    _write(out / 'run.json', run_info)
    rep = summarize(out, run_info['mode'] == 'fixtures', expected, list(run_info['arms']),
                    run_info.get('compat') is not None)
    rep['run'] = run_info
    _write(out / 'summary.json', rep)
    ev = out / 'evidence'
    ev.mkdir(parents=True, exist_ok=True)
    with tarfile.open(ev / 'results.tar.gz', 'w:gz') as tar:
        for p in [out / 'run.json', out / 'summary.json'] + sorted((out / 'results').rglob('*.json')):
            tar.add(p, arcname=str(p.relative_to(out)))
    rows.sort(key=lambda r: (r['config'] or '', r['file'] or ''))
    (ev / 'calls.jsonl').write_text(''.join(json.dumps(x, sort_keys=True) + '\n' for x in rows))
    _write(ev / 'manifest.json', manifests[0] | {'tools': run_info['tools']})
    print(table(rep))
    return verdict_status(rep)


def verdict_status(rep):
    """0 when every arm meets every bar in every class; 1 when a bar is missed, a class or an arm has no judged
    file, a class of the selection is empty, or an input has no complete result."""
    problems = []
    if rep.get('missing'):
        problems.append(f'inputs without a complete result: {rep["missing"]}')
    for c in (rep.get('run') or {}).get('empty_classes', []):
        problems.append(f'class {c} has no selected file')
    compat = (rep.get('run') or {}).get('compat') is not None
    judged = collections.Counter()
    for config, arms in rep['configs'].items():
        for arm, classes in arms.items():
            if arm == 'baseline':
                continue
            for cls, v in classes.items():
                judged[arm] += v['B1']['judged']
                # B9: a command line every fixture refuses is listed, not a failure; an arm that runs none fails.
                if v['B1']['judged'] == 0 and not compat:
                    problems.append(f'{config} {arm} {cls}: no judged file')
                for bar in ('B1', 'B1_native', 'B2', 'B3', 'Q1', 'Q2', 'bitrate'):
                    if (v.get(bar) or {}).get('met') is False:
                        problems.append(f'{config} {arm} {cls}: {bar} missed')
                for bar in ('B2', 'B3'):
                    if (v.get(bar) or {}).get('met_with_every_class') is False:
                        problems.append(f'{config} {arm}: {bar} missed in a class (95%)')
    if compat:
        problems += [f'{arm}: no command line ran on any fixture' for arm, n in judged.items() if n == 0]
    for p in problems:
        print('FAIL:', p)
    return 1 if problems else 0


def preflight(args):
    """Each arm transcodes one fixture before the run; a misconfigured arm (no encoder program, a wrong path)
    stops the run instead of turning every input into a refusal or a failure."""
    if not args.preflight or not os.path.isfile(args.preflight):
        raise SystemExit('--preflight names no file: each arm must be checked on one fixture before a run')
    with tempfile.TemporaryDirectory(dir=args.tmp) as d:
        for name, arm_args in args.arms:
            out = pathlib.Path(d) / f'{name}.mp4'
            t = (compat_run(args, dict(arm_args), 'x12', args.preflight, out) if args.compat
                 else transcode(args, arm_args, args.preflight, out, 'default'))
            if t['status'] != 'ok':
                raise SystemExit(f'arm {name} does not transcode {os.path.basename(args.preflight)}: '
                                 f'{t["status"]} {t.get("reason")} {json.dumps(t.get("error"))[:300]}')
    log(f'preflight: every arm transcodes {os.path.basename(args.preflight)}')


if __name__ == '__main__':
    sys.exit(main())
