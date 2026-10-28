"""`--exact` thumbnails with disposable packets skipped against the same
thumbnails decoding every packet (docs/design/thumbnails.md, "Frames
nothing depends on"): byte-identical PNGs, on every H.264 and HEVC file.

    exact_skip_check.py --decode-oracle PATH (--fixtures DIR | FILE...)
        [--times N] [--matroska] [--report OUT]

Each file is checked at `T` (the measurements' 40% of the presented
duration) or, with `--times N`, at N times spread over the file; with
`--matroska`, also a Matroska copy of it (stream copy by FFmpeg). Prints
counts only. Exits 1 when an image differs; when a run fails other than
alike in both modes and as a known refusal (a malformed container, a time
past the end, no keyframe before the time); when a Matroska copy cannot
be made; or when, for a codec and container checked, no run skipped any
packet, which would mean skipping never happened there.
"""
import argparse
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import thumbnail_oracle  # noqa: E402

CODECS = ('h264', 'hevc')
# The messages of refusals that are a file's, not the check's: the
# container parsers' structure errors, a time past the end, no keyframe.
KNOWN_REFUSALS = ('truncated structure at offset', "invalid '", 'invalid element 0x', 'invalid sample table',
                  'the time is at or after the end of the video',
                  'no keyframe before the frame at that time')


def codec_of(path, ffprobe):
    stream = thumbnail_oracle.default_video(path, ffprobe)
    if stream is None:
        return None
    r = thumbnail_oracle.run([ffprobe, '-v', 'error', '-select_streams', str(stream),
                              '-show_entries', 'stream=codec_name', '-of', 'csv=p=0', str(path)])
    return r.stdout.decode().strip()


def times_of(path, ffprobe, count):
    t = thumbnail_oracle.at_time(path, ffprobe)
    if t is None:
        return []
    if count <= 1:
        return [t]
    duration = t / 0.4
    return [int(duration * (i + 0.5) / count * 1000) / 1000 for i in range(count)]


def one(decode_oracle, path, t):
    with tempfile.TemporaryDirectory() as tmp:
        runs = {}
        for skip in ('on', 'off'):
            out = pathlib.Path(tmp) / f'{skip}.png'
            r = thumbnail_oracle.run([decode_oracle, 'thumbnail', str(path), '--at', f'{t:.3f}',
                                      '--skip', skip, '-o', str(out)])
            try:
                summary = json.loads(r.stdout)
            except ValueError:
                summary = {'error': r.stdout.decode(errors='replace')[-200:] or 'no output'}
            if 'error' in summary or not out.is_file():
                runs[skip] = {'error': str(summary.get('error'))}
            else:
                runs[skip] = {**summary, 'image': out.read_bytes()}
    on, off = runs['on'], runs['off']
    if 'error' in on or 'error' in off:
        alike = on.get('error') == off.get('error')
        known = alike and any(on['error'].startswith(k) or f': {k}' in on['error'] for k in KNOWN_REFUSALS)
        return {'status': 'refused' if known else 'failed', 'on': on.get('error'), 'off': off.get('error')}
    same = on['image'] == off['image'] and on['frame'] == off['frame']
    return {'status': 'same' if same else 'differs',
            'packets_on': on['packets_decoded'], 'packets_off': off['packets_decoded']}


def check(decode_oracle, path, ffprobe, times=1):
    """The result of `path` at its times; the first that is not the same
    decides it."""
    results = [one(decode_oracle, path, t) for t in times_of(path, ffprobe, times)]
    if not results:
        return {'status': 'no_duration'}
    worst = next((r for r in results if r['status'] in ('differs', 'failed')), None)
    out = dict(worst or next((r for r in results if r['status'] == 'refused'), results[0]))
    out['checked'] = len(results)
    out['packets_on'] = sum(r.get('packets_on', 0) for r in results)
    out['packets_off'] = sum(r.get('packets_off', 0) for r in results)
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('--decode-oracle', required=True)
    p.add_argument('--ffmpeg', default=shutil.which('ffmpeg'))
    p.add_argument('--ffprobe', default=shutil.which('ffprobe'))
    p.add_argument('--fixtures', type=pathlib.Path)
    p.add_argument('--times', type=int, default=1)
    p.add_argument('--matroska', action='store_true')
    p.add_argument('--report', type=pathlib.Path)
    p.add_argument('files', nargs='*', type=pathlib.Path)
    a = p.parse_args()
    files = list(a.files)
    if a.fixtures:
        files += sorted(f for f in a.fixtures.iterdir()
                        if f.suffix.lower().lstrip('.') in thumbnail_oracle.MP4_EXTS + ('mkv', 'webm'))
    results = []
    with tempfile.TemporaryDirectory() as copies:
        for path in files:
            codec = codec_of(path, a.ffprobe)
            if codec not in CODECS:
                continue
            targets = [path]
            if a.matroska and path.suffix.lower() != '.mkv':
                copy = pathlib.Path(copies) / f'{path.stem}.mkv'
                done = subprocess.run([a.ffmpeg, '-v', 'error', '-y', '-i', str(path), '-map', '0:v:0',
                                       '-c', 'copy', str(copy)], capture_output=True, timeout=120, check=False)
                if done.returncode == 0:
                    targets.append(copy)
                else:
                    results.append({'status': 'failed', 'file': copy.name, 'codec': codec,
                                    'error': 'the Matroska copy could not be made'})
            for target in targets:
                result = check(a.decode_oracle, target, a.ffprobe, a.times)
                result['file'] = target.name
                result['codec'] = codec
                results.append(result)
    counts = {}
    for r in results:
        counts[r['status']] = counts.get(r['status'], 0) + 1
    on = sum(r.get('packets_on', 0) for r in results)
    off = sum(r.get('packets_off', 0) for r in results)
    print(json.dumps({'files': len(results), 'by_status': counts, 'packets_decoded_on': on,
                      'packets_decoded_off': off}))
    if a.report:
        a.report.write_text(json.dumps(results, indent=1))
    # Skipping must have happened for each codec and container checked.
    groups = {}
    for r in results:
        key = (r['codec'], pathlib.Path(r['file']).suffix.lower() == '.mkv')
        g = groups.setdefault(key, [0, 0])
        g[0] += r.get('packets_on', 0)
        g[1] += r.get('packets_off', 0)
    unskipped = sorted(f"{codec} {'mkv' if mkv else 'mp4/mov'}" for (codec, mkv), (o, f) in groups.items()
                       if f > 0 and o >= f)
    if unskipped:
        print(json.dumps({'no_packet_skipped_for': unskipped}))
    bad = counts.get('differs') or counts.get('failed') or not results or on >= off or unskipped
    return 1 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
