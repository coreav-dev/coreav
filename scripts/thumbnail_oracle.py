#!/usr/bin/env python3
"""The decode and thumbnail oracles (docs/design/decode.md, bar 1;
docs/design/thumbnails.md, bars I1-I3), shared by the fixtures gate in CI
and the corpus runs of the thumbnail spike (docs/spikes/thumbnail-bench/).

For one file: thumbnails in keyframe and exact mode against FFmpeg's of the
same frame and size (PSNR over 8-bit RGB), CoreAV's JPEG and FFmpeg's
against the same lossless reference, and the first 120 packets through
CoreAV's decoder (`decode-oracle`) against FFmpeg's framemd5 of the same
stream.

    thumbnail_oracle.py --coreav PATH --decode-oracle PATH FILE...
    thumbnail_oracle.py --coreav PATH --decode-oracle PATH --fixtures DIR [--report OUT]

With --fixtures, every MP4, MOV, Matroska and WebM file of DIR with a video
stream on the default decode allowlist is checked against the fixture bars:
every compared frame equal, every image at least FLOOR dB; exit 1 when one
is not. FFmpeg (ffmpeg, ffprobe) is the black-box oracle. Uses only the
Python standard library.
"""
import argparse
import json
import math
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

WIDTH = 320
MP4_EXTS = ('mp4', 'mov', 'm4v', '3gp', 'm4a', 'm4b')
PACKETS = 120
TIMEOUT = 600
# The environment of every tool this runs; a runner that holds credentials
# sets one without them.
ENV = None
# Extra arguments of every `decode-oracle` run (a larger memory budget for a
# re-run on a larger host).
DECODE_ORACLE_ARGS = []
# The fixtures gate's image floor: gross errors only (a wrong frame, matrix,
# range or rotation costs far more); the corpus bars are in
# docs/design/thumbnails.md.
FLOOR = 30.0
# Refusals of malformed fixtures, by design.
REFUSED = ('truncated_structure', 'invalid_structure')
# Fixtures whose images are not compared, and why (frames still are).
IMAGES_NOT_COMPARED = {
    'size_inband_sps.mp4': "FFmpeg's H.264 decoder labels frames with other packets' timestamps when "
                           "in-band parameter sets change the frame size (docs/design/thumbnails.md, "
                           "'Limitations')",
}
ALLOWED = ('h264', 'hevc', 'av1', 'vp9', 'vp8', 'prores')
# The codecs a backend other than the fallback decodes.
BACKEND_CODECS = {'dav1d': ('av1',), 'libvpx': ('vp9', 'vp8')}


def default_video(path, ffprobe='ffprobe'):
    """The index of the video stream FFmpeg (and CoreAV) use by default: the
    highest resolution, the first on a tie; attached pictures left out."""
    r = subprocess.run([ffprobe, '-v', 'error', '-show_entries',
                        'stream=index,codec_type,width,height:stream_disposition=attached_pic', '-of', 'json',
                        str(path)], capture_output=True, text=True, timeout=120, check=False, env=ENV)
    try:
        streams = json.loads(r.stdout or '{}').get('streams', [])
    except ValueError:
        return None
    best = None
    for s in streams:
        if s.get('codec_type') != 'video' or s.get('disposition', {}).get('attached_pic'):
            continue
        area = (s.get('width') or 0) * (s.get('height') or 0)
        if best is None or area > best[0]:
            best = (area, s['index'])
    return best[1] if best else None


def at_time(path, ffprobe='ffprobe'):
    """T: 40% of the first video stream's presented duration, rounded down
    to the millisecond: from the file's start time (where `ffmpeg -ss` and
    `coreav thumbnail --at` count from) to the end of its last video packet.
    (`ffprobe`'s duration field is not the length for a file whose start
    time is not 0.)"""
    stream = default_video(path, ffprobe)
    if stream is None:
        return None
    r = subprocess.run([ffprobe, '-v', 'error', '-select_streams', str(stream), '-show_entries',
                        'packet=pts_time,duration_time:format=start_time', '-of', 'json', str(path)],
                       capture_output=True, text=True, timeout=600, check=False, env=ENV)
    try:
        d = json.loads(r.stdout or '{}')
    except ValueError:
        return None
    try:
        start = float(d.get('format', {}).get('start_time', 0) or 0)
    except (TypeError, ValueError):
        start = 0.0
    end = None
    for p in d.get('packets', []):
        try:
            t = float(p['pts_time'])
        except (KeyError, TypeError, ValueError):
            continue
        try:
            t += float(p.get('duration_time') or 0)
        except (TypeError, ValueError):
            pass
        end = t if end is None else max(end, t)
    if end is None:
        return None
    length = end - start
    if not math.isfinite(length) or length <= 0:
        return None
    return math.floor(length * 0.4 * 1000) / 1000


def floor_us(rational):
    """A coreav.thumbnail/1 rational, rounded down to the microsecond, as text."""
    num, den = rational['num'], rational['den']
    micros = (num * 1_000_000) // den
    sign = '-' if micros < 0 else ''
    micros = abs(micros)
    return f'{sign}{micros // 1_000_000}.{micros % 1_000_000:06d}'


def psnr(a, b):
    """PSNR over every 8-bit sample of two equal-size images, or None."""
    if not a or len(a) != len(b):
        return None
    total = 0
    for x, y in zip(a, b):
        d = x - y
        total += d * d
    if total == 0:
        return 99.0
    return 10 * math.log10(255 * 255 * len(a) / total)


def rgb(path, ffmpeg='ffmpeg'):
    r = subprocess.run([ffmpeg, '-v', 'error', '-i', str(path), '-fps_mode', 'passthrough', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'],
                       capture_output=True, timeout=120, check=False, env=ENV)
    return r.stdout


def run(cmd):
    return subprocess.run(cmd, capture_output=True, timeout=TIMEOUT, check=False, env=ENV)


def error_of(r):
    text = r.stderr.decode(errors='replace').strip()
    try:
        return json.loads(text.splitlines()[-1])
    except (ValueError, IndexError):
        return text[-300:]


def thumb(coreav, path, t, out, exact=False):
    r = run([coreav, 'thumbnail', str(path), '--at', f'{t:.3f}', '--width', str(WIDTH), '-o', str(out),
             '--force'] + (['--exact'] if exact else []))
    if r.returncode != 0:
        return None, error_of(r)
    return json.loads(r.stdout), None


def ffmpeg_time(ffprobe, path, summary):
    """FFmpeg's `-ss` time for the frame CoreAV chose, rounded down to the
    microsecond: the pts `ffprobe` lists for the packet that holds the frame's
    bytes, exactly from its integer pts and time base, less the file's start
    time, from which `-ss` counts; CoreAV's own time when `ffprobe` lists no
    such packet. In MP4/MOV that packet starts at the frame's byte offset;
    elsewhere it is the stream's last packet that starts before it, as a
    Matroska block or an MPEG-TS PES starts with a header. So FFmpeg's
    timeline may differ from CoreAV's: by known divergence KD-EDIT-SNAP in
    MP4, and by the later start FFmpeg 9.0.2 gives a Matroska file whose
    audio has a `CodecDelay`."""
    from fractions import Fraction
    mp4 = path.suffix.lstrip('.').lower() in MP4_EXTS
    r = run([ffprobe, '-v', 'error', '-select_streams', str(summary.get('stream_index')), '-show_entries',
             'packet=pos,pts,flags:stream=time_base:format=start_time', '-of', 'json', str(path)])
    try:
        d = json.loads(r.stdout)
    except ValueError:
        d = {}
    offset, held = int(summary['packet']['offset']), None
    for p in d.get('packets', []):
        pos = str(p.get('pos'))
        if not pos.isdigit():
            continue
        if int(pos) == offset:
            held = p
            break
        if not mp4 and int(pos) < offset and (held is None or int(pos) > int(held['pos'])):
            held = p
    if held is None or held.get('pts') in (None, 'N/A'):
        return floor_us(summary['time']['frame'])
    if 'D' in str(held.get('flags', '')):
        # FFmpeg discards the packet's frame (known divergence
        # KD-EDIT-SNAP: CoreAV presents it): no reference exists.
        return None
    base = (d.get('streams') or [{}])[0].get('time_base', '1/1')
    start = Fraction(str(d.get('format', {}).get('start_time', '0') or '0'))
    t = Fraction(int(held['pts'])) * Fraction(base) - start
    return floor_us({'num': t.numerator, 'den': t.denominator})


def reference(ffmpeg, path, summary, out, extra=('-pix_fmt', 'rgb24'), ffprobe='ffprobe'):
    """FFmpeg's image of the frame CoreAV chose, at the same size: True when
    written, False when FFmpeg failed, None when FFmpeg discards that
    frame. FFmpeg seeks its input to the keyframe CoreAV decoded from and,
    for a frame after it, skips to the frame with an output seek, so that
    both decode the same packets (FFmpeg's own seek can land on a frame
    that does not start decoding, in a file without a true sync table)."""
    from fractions import Fraction
    w, h = summary['output']['width'], summary['output']['height']
    at = ffmpeg_time(ffprobe, path, summary)
    if at is None:
        return None
    frame, start = summary['time']['frame'], summary['time'].get('decode_start') or summary['time']['frame']
    gap = Fraction(frame['num'], frame['den']) - Fraction(start['num'], start['den'])
    seek = ['-ss', at, '-i', str(path)]
    if gap > 0:
        key = Fraction(at) - gap
        if key > 0:
            seek = ['-ss', floor_us({'num': key.numerator, 'den': key.denominator}), '-i', str(path),
                    '-ss', floor_us({'num': gap.numerator, 'den': gap.denominator})]
        else:
            # The keyframe is at or before the file's start: decode from the
            # start and skip to the frame.
            seek = ['-i', str(path), '-ss', at]
    # Exact cropping, as in framemd5 below.
    r = run([ffmpeg, '-v', 'error', '-y', '-flags', 'unaligned', *seek, '-map', f'0:{summary["stream_index"]}', '-frames:v', '1',
             '-vf', f'scale={w}:{h}', *extra, str(out)])
    return r.returncode == 0 and out.exists()


def images(coreav, ffmpeg, ffprobe, path, t, tmp):
    out = {}
    key, err = thumb(coreav, path, t, tmp / 'k.png')
    if err is not None:
        return {'status': 'refused', 'error': err}
    out['frame'] = key['time']['frame']
    out['size'] = [key['output']['width'], key['output']['height']]
    out['backend'] = key['decoder']['backend']
    out['keyframe'] = key['keyframe']
    made = reference(ffmpeg, path, key, tmp / 'r.png', ffprobe=ffprobe)
    if made is None:
        return {'status': 'ffmpeg_discards', **out}
    if not made:
        return {'status': 'ffmpeg_failed', **out}
    ref = rgb(tmp / 'r.png', ffmpeg)
    out['I1'] = psnr(rgb(tmp / 'k.png', ffmpeg), ref)
    jpg, err = thumb(coreav, path, t, tmp / 'k.jpg')
    if err is not None:
        out['I3_error'] = err
    elif not reference(ffmpeg, path, key, tmp / 'f.jpg', extra=(), ffprobe=ffprobe):
        out['I3_error'] = 'ffmpeg failed'
    else:
        out['I3_ours'] = psnr(rgb(tmp / 'k.jpg', ffmpeg), ref)
        out['I3_ffmpeg'] = psnr(rgb(tmp / 'f.jpg', ffmpeg), ref)
        out['I3_bytes'] = [os.path.getsize(tmp / 'k.jpg'), os.path.getsize(tmp / 'f.jpg')]
    exact, err = thumb(coreav, path, t, tmp / 'x.png', exact=True)
    if err is not None:
        out['I2_error'] = err
    else:
        made = reference(ffmpeg, path, exact, tmp / 'rx.png', ffprobe=ffprobe)
        if made:
            out['I2'] = psnr(rgb(tmp / 'x.png', ffmpeg), rgb(tmp / 'rx.png', ffmpeg))
            out['exact_frame'] = exact['time']['frame']
        else:
            out['I2_ffmpeg'] = 'failed' if made is False else 'discards'
    out['status'] = 'ok'
    return out


def framemd5(ffmpeg, ffprobe, path, ext, stream_index, n, backend):
    """FFmpeg's MD5s of the first `n` frames of the stream."""
    # -noautorotate: framemd5 hashes decoded frames, before any rotation.
    # -noautoscale: frames keep their size when the stream changes it.
    cmd = [ffmpeg, '-v', 'error', '-noautorotate']
    # CoreAV's frame model drops alpha planes (docs/design/decode.md):
    # compare the colour planes, in the decoder's format without alpha.
    probe = run([ffprobe, '-v', 'error', '-select_streams', str(stream_index), '-show_entries',
                 'stream=pix_fmt', '-of', 'csv=p=0', str(path)]).stdout.decode().strip()
    codec = run([ffprobe, '-v', 'error', '-select_streams', str(stream_index), '-show_entries',
                 'stream=codec_name', '-of', 'csv=p=0', str(path)]).stdout.decode().strip()
    alpha = None
    if probe.startswith('yuva'):
        alpha = 'yuv' + probe[4:]
    elif probe.startswith('gbrap'):
        alpha = 'gbrp' + probe[5:]
    if ext in MP4_EXTS:
        cmd += ['-ignore_editlist', '1']
    if backend == 'dav1d':
        # The same decoder (docs/design/decode.md, bar 1).
        cmd += ['-c:v', 'libdav1d']
    elif backend == 'libvpx':
        # FFmpeg's own VP8 or VP9 decoder, never its libvpx wrapper: an
        # independent decoder of the same exactly specified process.
        cmd += ['-c:v', 'vp8' if codec == 'vp8' else 'vp9']
    # Crop exactly as the bitstream says: by default FFmpeg 7 skips a left crop
    # that would break its planes' alignment (docs/design/decode.md).
    cmd += ['-flags', 'unaligned']
    cmd += ['-i', str(path), '-map', f'0:{stream_index}', '-fps_mode', 'passthrough', '-frames:v', str(n)]
    cmd += ['-pix_fmt', alpha] if alpha else []
    cmd += ['-noautoscale', '-f', 'framemd5', '-']
    r = run(cmd)
    return [line.split(',')[-1].strip() for line in r.stdout.decode().splitlines()
            if line and not line.startswith('#')]


def frames(decode_oracle, ffmpeg, ffprobe, path, ext, backend='ffmpeg'):
    r = run([decode_oracle, str(path), '--packets', str(PACKETS), '--backend', backend, *DECODE_ORACLE_ARGS])
    try:
        ours = json.loads(r.stdout)
    except ValueError:
        ours = {'error': r.stdout.decode(errors='replace')[-300:]}
    if 'error' in ours:
        # Bar 1: a stream CoreAV fails to decode disagrees in full, on
        # FFmpeg's first PACKETS frames of it (its default stream).
        stream_index = default_video(path, ffprobe)
        theirs = [] if stream_index is None else framemd5(ffmpeg, ffprobe, path, ext, stream_index,
                                                           PACKETS, backend)
        return {'status': 'decode_failed', 'error': ours['error'], 'ffmpeg_frames': len(theirs)}
    cutoff = ours['cutoff']
    mine = [f for f in ours['frames'] if cutoff is None or (f['pts'] is not None and f['pts'] < cutoff)]
    n = len(mine)
    if n == 0:
        theirs = framemd5(ffmpeg, ffprobe, path, ext, ours['stream_index'], PACKETS, backend)
        return {'status': 'no_frames', 'fed': ours['fed'], 'ffmpeg_frames': len(theirs)}
    theirs = framemd5(ffmpeg, ffprobe, path, ext, ours['stream_index'], n, backend)
    equal = sum(1 for a, b in zip(mine, theirs) if a['md5'] == b)
    return {'status': 'ok', 'compared': n, 'equal': equal, 'ffmpeg_frames': len(theirs),
            'agree': equal == n and len(theirs) >= n, 'format': mine[0]['format'], 'decoder': ours['decoder']}


def check(path, ext, coreav, decode_oracle, ffmpeg, ffprobe, backend='ffmpeg', frames_too=True):
    with tempfile.TemporaryDirectory(dir='/dev/shm' if os.path.isdir('/dev/shm') else None) as d:
        tmp = pathlib.Path(d)
        t = at_time(path, ffprobe)
        result = {'t': t}
        if t is None:
            result['images'] = {'status': 'no_duration'}
        else:
            result['images'] = images(coreav, ffmpeg, ffprobe, path, t, tmp)
        if frames_too:
            result['frames'] = frames(decode_oracle, ffmpeg, ffprobe, path, ext, backend)
        return result


def video_codec(path, ffprobe):
    stream = default_video(path, ffprobe)
    if stream is None:
        return None
    r = run([ffprobe, '-v', 'error', '-select_streams', str(stream), '-show_entries', 'stream=codec_name',
             '-of', 'csv=p=0', str(path)])
    return r.stdout.decode().strip() or None


def fixtures(args):
    report = []
    failed = []
    for path in sorted(pathlib.Path(args.fixtures).iterdir()):
        ext = path.suffix.lstrip('.').lower()
        if ext not in MP4_EXTS + ('mkv', 'webm'):
            continue
        codec = video_codec(path, args.ffprobe)
        if codec not in BACKEND_CODECS.get(args.backend, ALLOWED):
            continue
        result = check(path, ext, args.coreav, args.decode_oracle, args.ffmpeg, args.ffprobe, args.backend)
        result['file'] = path.name
        result['codec'] = codec
        report.append(result)
        images, frames_ = result.get('images', {}), result.get('frames', {})
        problems = []
        if frames_.get('status') != 'ok' or not frames_.get('agree'):
            problems.append(f'frames: {frames_}')
        if path.name in IMAGES_NOT_COMPARED:
            print(f'images not compared {path.name}: {IMAGES_NOT_COMPARED[path.name]}', flush=True)
        elif images.get('status') == 'ok':
            for key in ('I1', 'I2'):
                if images.get(key) is not None and images[key] < FLOOR:
                    problems.append(f'{key} {images[key]:.2f} dB')
        elif images.get('status') == 'refused' and isinstance(images.get('error'), dict) \
                and images['error'].get('kind') in REFUSED:
            print(f'refused {path.name}: {images["error"].get("kind")}', flush=True)
            continue
        elif images.get('status') not in ('ffmpeg_failed', 'ffmpeg_discards'):
            problems.append(f'images: {images}')
        status = 'ok' if not problems else 'FAIL'
        print(f'{status} {path.name} {codec} I1={images.get("I1")} I2={images.get("I2")} '
              f'frames={frames_.get("equal")}/{frames_.get("compared")}', flush=True)
        if problems:
            failed.append({'file': path.name, 'problems': problems})
    if args.report:
        pathlib.Path(args.report).write_text(json.dumps({'files': report, 'failed': failed}, indent=1))
    print(json.dumps({'checked': len(report), 'failed': failed}, indent=1))
    return 1 if failed or not report else 0


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[1])
    p.add_argument('--coreav', required=True)
    p.add_argument('--decode-oracle', required=True)
    p.add_argument('--ffmpeg', default=shutil.which('ffmpeg'))
    p.add_argument('--ffprobe', default=shutil.which('ffprobe'))
    p.add_argument('--backend', default='ffmpeg')
    p.add_argument('--fixtures')
    p.add_argument('--report')
    p.add_argument('files', nargs='*')
    a = p.parse_args()
    if a.fixtures:
        return fixtures(a)
    for f in a.files:
        path = pathlib.Path(f)
        print(json.dumps(check(path, path.suffix.lstrip('.').lower(), a.coreav, a.decode_oracle, a.ffmpeg,
                               a.ffprobe, a.backend)))
    return 0


if __name__ == '__main__':
    sys.exit(main())
