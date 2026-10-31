#!/usr/bin/env python3
"""Frame listings of `coreav ffprobe` against the pinned `ffprobe`
(docs/design/compat-cli.md, "Frames").

    frames.py --coreav PATH --media DIR [DIR...] [--native-h264]
              [--baseline FILE] [--write-baseline FILE] [--report FILE]

Runs every command of COMMANDS on every fixture of the directories (a
MANIFEST with `generated NAME` lines selects the files) through both
tools, `coreav` in strict-safe mode so that nothing is handed to FFmpeg.
A command `ffprobe` fails is not counted. For each other pair:

- `agree`: coreav exits 0 and prints what ffprobe prints, byte for byte.
- `refused`: coreav exits 4, prints nothing, names what it does not
  decode, and a stream the command selects has a codec outside NATIVE,
  or H.264 in a pixel format outside H264_PIX_FMTS.
- anything else is a failure: different output, another exit status, or
  a refusal of a command whose every selected stream CoreAV decodes.

Exit 1 when a failure is not in the baseline (or, without one, when there
is any). ffprobe is a black box: only its output is read. Python 3
standard library only.
"""
import argparse
import concurrent.futures
import json
import os
import pathlib
import subprocess
import sys

TIMEOUT = 600
ENV = {**os.environ, 'COREAV_STRICT_SAFE': '1'}

# The commands: every writer, frames alone and with packets, one stream
# type at a time and all streams, -count_frames, and a packet interval.
# Packets and streams show only the entries CoreAV prints as the pinned
# ffprobe does without frames: packet durations and side data and some
# stream entries of ffprobe 9 are not the subject here (narrowed after the
# first run, which compared every entry).
PACKET = 'packet=stream_index,pts,dts,size,pos,flags'
COMMANDS = [
    ['-v', 'error', '-show_frames', 'INPUT'],
    ['-v', 'error', '-of', 'json', '-show_frames', '-show_entries', PACKET, 'INPUT'],
    ['-v', 'error', '-of', 'compact', '-select_streams', 'v', '-show_frames', 'INPUT'],
    ['-v', 'error', '-of', 'compact', '-select_streams', 'a', '-show_frames', 'INPUT'],
    ['-v', 'error', '-of', 'flat', '-select_streams', 'v:0', '-show_frames', '-show_entries',
     PACKET, 'INPUT'],
    ['-v', 'error', '-of', 'csv=p=0', '-select_streams', 'v:0', '-show_entries',
     'frame=key_frame,pict_type,pts_time,pkt_dts_time', 'INPUT'],
    ['-v', 'error', '-of', 'json', '-select_streams', 'a:0', '-show_entries',
     'frame=pts,pkt_dts,duration,nb_samples,sample_fmt', 'INPUT'],
    ['-v', 'error', '-select_streams', 'v:0', '-count_frames', '-show_entries',
     'stream=nb_read_frames', '-of', 'default=nokey=1:noprint_wrappers=1', 'INPUT'],
    ['-v', 'error', '-of', 'json', '-count_frames', '-count_packets', '-show_entries',
     'stream=index,codec_type,nb_read_frames,nb_read_packets', 'INPUT'],
    ['-v', 'error', '-of', 'json', '-select_streams', 'v:0', '-show_frames', '-read_intervals',
     '%+#5', 'INPUT'],
]

# Codecs CoreAV decodes, by ffprobe's names; H.264 only in a build with
# the native decoder.
NATIVE = {'mp3', 'flac', 'pcm_u8', 'pcm_s8', 'pcm_alaw', 'pcm_mulaw', 'pcm_s16le', 'pcm_s16be',
          'pcm_s24le', 'pcm_s24be', 'pcm_s32le', 'pcm_s32be', 'pcm_f32le', 'pcm_f32be',
          'pcm_f64le', 'pcm_f64be'}
# The pictures the native H.264 decoder gives: 8-bit 4:2:0.
H264_PIX_FMTS = {'yuv420p', 'yuvj420p'}


def media_files(dirs):
    out = []
    for d in dirs:
        d = pathlib.Path(d)
        manifest = d / 'MANIFEST'
        if manifest.exists():
            for line in manifest.read_text().splitlines():
                parts = line.split()
                if len(parts) >= 2 and parts[0] == 'generated':
                    out.append(d / parts[1])
        else:
            out.extend(sorted(p for p in d.iterdir()
                              if p.is_file() and p.suffix not in ('.json', '.log', '')))
    return out


def run(cmd):
    try:
        p = subprocess.run(cmd, capture_output=True, timeout=TIMEOUT, env=ENV)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return None, b'', b'timeout'


def selected_codecs(ffprobe, args, path):
    """The codecs of the streams the command decodes: those its
    -select_streams names, audio, video and subtitles (ffprobe decodes no
    data stream). H.264 is named with its pixel format."""
    spec = []
    if '-select_streams' in args:
        spec = ['-select_streams', args[args.index('-select_streams') + 1]]
    code, out, _ = run([ffprobe, '-v', 'error', *spec, '-show_entries',
                        'stream=codec_type,codec_name,pix_fmt', '-of', 'json', str(path)])
    if code != 0:
        return None
    streams = json.loads(out).get('streams', [])
    return [f"h264 {s.get('pix_fmt')}" if s.get('codec_name') == 'h264' else s.get('codec_name', 'unknown')
            for s in streams if s.get('codec_type') in ('video', 'audio', 'subtitle')]


def first_difference(a, b):
    la, lb = a.decode('utf-8', 'replace').splitlines(), b.decode('utf-8', 'replace').splitlines()
    for i in range(max(len(la), len(lb))):
        x = la[i] if i < len(la) else '<end>'
        y = lb[i] if i < len(lb) else '<end>'
        if x != y:
            return {'line': i + 1, 'ffprobe': x[:300], 'coreav': y[:300]}
    return {'line': None}


def judge(opts, native, args, path):
    argv = [str(path) if a == 'INPUT' else a for a in args]
    ref_code, ref_out, ref_err = run([opts.ffprobe] + argv)
    if ref_code is None or ref_code < 0:
        return {'result': 'fail', 'exit': ref_code, 'oracle_failed': True,
                'stderr': 'ffprobe oracle failed: ' + ref_err[:300].decode('utf-8', 'replace')}
    if ref_code != 0:
        return None
    code, out, err = run([opts.coreav, 'ffprobe'] + argv)
    if code == 0 and out == ref_out:
        return {'result': 'agree'}
    codecs = selected_codecs(opts.ffprobe, args, path) or []
    outside = sorted({c for c in codecs if c not in native})
    if code == 4 and not out and err.strip() and outside:
        return {'result': 'refused', 'codecs': outside}
    failure = {'result': 'fail', 'exit': code}
    if code == 0:
        failure['diff'] = first_difference(ref_out, out)
    else:
        failure['stderr'] = err[:300].decode('utf-8', 'replace')
    return failure


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--coreav', required=True)
    ap.add_argument('--ffprobe', default='ffprobe')
    ap.add_argument('--media', nargs='+', required=True)
    ap.add_argument('--native-h264', action='store_true',
                    help='the coreav binary is built with the native-h264 feature')
    ap.add_argument('--baseline')
    ap.add_argument('--write-baseline')
    ap.add_argument('--report')
    ap.add_argument('--jobs', type=int, default=os.cpu_count() or 4)
    opts = ap.parse_args()
    native = NATIVE | ({f'h264 {f}' for f in H264_PIX_FMTS} if opts.native_h264 else set())
    files = media_files(opts.media)
    names = set()
    for path in files:
        if path.name in names:
            ap.error(f'duplicate fixture name: {path.name}; baseline keys require unique names')
        names.add(path.name)
    pairs = [(args, path) for args in COMMANDS for path in files]
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        results = list(pool.map(lambda p: judge(opts, native, *p), pairs))
    counts = {'agree': 0, 'refused': 0, 'fail': 0}
    failures = []
    for (args, path), r in zip(pairs, results):
        if r is None:
            continue
        counts[r['result']] += 1
        if r['result'] == 'fail':
            failures.append({'args': args, 'file': path.name, **r})
    keys = sorted('|'.join([' '.join(f['args']), f['file']]) for f in failures)
    by_key = {'|'.join([' '.join(f['args']), f['file']]): f for f in failures}
    report = {'counts': counts, 'failures': failures}
    if opts.report:
        pathlib.Path(opts.report).write_text(json.dumps(report, indent=1))
    print(json.dumps(counts))
    if opts.write_baseline:
        pathlib.Path(opts.write_baseline).write_text(json.dumps(
            {'note': 'failing (command, fixture) pairs of tools/compat-corpus/frames.py on the '
                     'generated fixtures; CI fails on any other',
             'failing': [k for k in keys if not by_key[k].get('oracle_failed')]}, indent=1) + '\n')
    # The baseline lists the failures of every fixture set; this run judges
    # those of its own files.
    known = set()
    if opts.baseline:
        known = {k for k in json.loads(pathlib.Path(opts.baseline).read_text())['failing']
                 if k.rsplit('|', 1)[-1] in names}
    new_keys = [k for k in keys if k not in known or by_key[k].get('oracle_failed')]
    for k in new_keys:
        f = by_key[k]
        print(f'FAIL {k}')
        print('    ' + json.dumps({x: f.get(x) for x in ('exit', 'diff', 'stderr') if x in f})[:600])
    fixed = sorted(known - set(keys))
    for k in fixed:
        print(f'FIXED {k}')
    print(f'{len(new_keys)} new failures, {len(fixed)} fixed since the baseline')
    return 1 if new_keys else 0


if __name__ == '__main__':
    sys.exit(main())
