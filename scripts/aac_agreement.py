#!/usr/bin/env python3
"""The AAC-LC agreement bars of docs/design/audio-decode.md ("AAC-LC bars",
bars 2 to 5, and "AAC-LC sets"), run by .github/workflows/audio-conformance.yml.

  aac_agreement.py prepare --ffmpeg F --work DIR [--conformance DIR]
      Writes the generated set (DIR/gen/*.m4a and *.mka, DIR/set.json) with
      the pinned ffmpeg's aac encoder, then for every MP4 input (generated,
      and the LC streams of the conformance package's compressedMp4/) the
      reference `ffmpeg -ignore_editlist 1 -i IN -map 0:a:0 -f f32le`
      (DIR/ref/*.f32), and DIR/agreement.tsv, the list the decoder test reads
      (crates/coreav-aac/tests/conformance.rs, `ffmpeg_agreement`).
  aac_agreement.py judge --coreav C --ffmpeg F --ffprobe P --work DIR --out OUT.json
      Reads the decoder test's DIR/decoder.jsonl (bar 2), runs
      scripts/decode_audio_check.py (bar 3), compares the Matroska inputs
      (bar 4) and the thread counts (bar 5), writes OUT.json and exits 1 when
      a gating bar is missed.

Uses the Python standard library only; ffmpeg and ffprobe are black boxes.
"""
import argparse
import array
import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SECONDS = 10
RATES = (8000, 11025, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000)
TOOLS = (('is-off', ('-aac_is', '0')), ('ms-forced', ('-aac_ms', '1')), ('tns-off', ('-aac_tns', '0')),
         ('fast-coder', ('-aac_coder', 'fast')), ('32k', ('-b:a', '32k')), ('256k', ('-b:a', '256k')))
LAYOUTS = (('3.0', 3), ('4.0', 4), ('5.0', 5), ('5.1', 6), ('6.1', 7), ('7.1', 8))
PNS_OFF = ('-aac_pns', '0')
# Twice full accuracy (the MP3 agreement tolerance) and the level rule.
MAX_RMS = 2 * 2.0 ** -15 / 12 ** 0.5
MAX_ABS = 2 * 2.0 ** -14
MAX_NOISE_DB = 0.1
MIN_NOISE_BLOCKS = 100
TIMEOUT = 600


def source(kind, rate, channels, layout):
    """A lavfi source of SECONDS seconds; each channel its own frequency."""
    if kind == 'noise':
        parts = ''.join(f'anoisesrc=c=pink:a=0.25:r={rate}:d={SECONDS}:seed={c + 1}[n{c}];' for c in range(channels))
        if channels == 1:
            return parts.replace('[n0];', '')
        return parts + ''.join(f'[n{c}]' for c in range(channels)) + f'amerge=inputs={channels}'
    def expression(c):
        k = 1 + 0.07 * c
        if kind == 'tones':
            return (f'0.25*(sin(2*PI*{220 * k:.3f}*t)+sin(2*PI*{1000 * k:.3f}*t)+sin(2*PI*{5000 * k:.3f}*t))'
                    '*(0.75+0.25*sin(2*PI*3*t))')
        if kind == 'clicks':
            return f'0.1*sin(2*PI*{440 * k:.3f}*t)+if(lt(mod(t+{0.01 * c:.2f},0.25),0.001),0.9,0)'
        if kind == 'tones+clicks':
            return (f'0.2*(sin(2*PI*{220 * k:.3f}*t)+sin(2*PI*{1000 * k:.3f}*t))'
                    f'+if(lt(mod(t+{0.01 * c:.2f},0.25),0.001),0.5,0)')
        return f'0.5*sin(2*PI*({50 * k:.3f}*t+{(16000 - 50) / (2 * SECONDS):.3f}*t*t))'
    exprs = '|'.join(expression(c) for c in range(channels))
    return f"aevalsrc='{exprs}':s={rate}:d={SECONDS}:c={layout}"


def encodes():
    """(name, group, lavfi source, channels, encoder options) of the set."""
    out = []
    for rate in RATES:
        for channels, layout in ((1, 'mono'), (2, 'stereo')):
            for kind in ('tones', 'noise', 'clicks', 'sweep'):
                out.append((f'rates-{kind}-{rate}-{layout}', 'rates', source(kind, rate, channels, layout),
                            channels, PNS_OFF))
    for rate in RATES:
        for kind in ('noise', 'tones'):
            out.append((f'pns-{kind}-{rate}-stereo', 'pns', source(kind, rate, 2, 'stereo'), 2, ()))
    for kind in ('tones', 'noise', 'clicks', 'sweep'):
        for name, options in TOOLS:
            out.append((f'tools-{kind}-{name}', 'tools', source(kind, 48000, 2, 'stereo'), 2,
                        PNS_OFF + options))
    for layout, channels in LAYOUTS:
        for pce in (False, True):
            out.append((f'layouts-{layout}{"-pce" if pce else ""}', 'layouts',
                        source('tones+clicks', 48000, channels, layout), channels,
                        PNS_OFF + (('-aac_pce', '1') if pce else ())))
    return out


def run(args, **kw):
    return subprocess.run([str(a) for a in args], capture_output=True, timeout=TIMEOUT, **kw)


def prepare(a):
    work = a.work
    for sub in ('gen', 'ref'):
        (work / sub).mkdir(parents=True, exist_ok=True)
    inputs, tsv = [], []
    for name, group, src, channels, options in encodes():
        m4a, mka = work / 'gen' / f'{name}.m4a', work / 'gen' / f'{name}.mka'
        enc = run([a.ffmpeg, '-v', 'error', '-y', '-f', 'lavfi', '-i', src, '-c:a', 'aac', *options, m4a])
        record = {'name': name, 'group': group, 'channels': channels, 'options': list(options)}
        if enc.returncode != 0:
            inputs.append({**record, 'status': 'not encoded', 'error': enc.stderr.decode(errors='replace')[-300:]})
            continue
        cp = run([a.ffmpeg, '-v', 'error', '-y', '-i', m4a, '-map', '0:a', '-c', 'copy', mka])
        inputs.append({**record, 'status': 'encoded', 'mka': cp.returncode == 0})
    mp4s = [(f"gen-{r['name']}", work / 'gen' / f"{r['name']}.m4a") for r in inputs if r['status'] == 'encoded']
    if a.conformance:
        for f in sorted((a.conformance / 'compressedMp4').glob('al*.mp4')):
            mp4s.append((f'conf-{f.stem}', f))
    refs = {}
    for key, path in mp4s:
        ref = work / 'ref' / f'{key}.f32'
        dec = run([a.ffmpeg, '-v', 'error', '-ignore_editlist', '1', '-i', path, '-map', '0:a:0', '-f', 'f32le', ref])
        refs[str(path)] = {'key': key, 'returncode': dec.returncode, 'errors': bool(dec.stderr.strip()),
                           'stderr': dec.stderr.decode(errors='replace')[-300:]}
        if dec.returncode == 0:
            tsv.append(f'{path}\t{ref}')
    (work / 'agreement.tsv').write_text('\n'.join(tsv) + '\n')
    version = run([a.ffmpeg, '-version']).stdout.decode().splitlines()[0]
    (work / 'set.json').write_text(json.dumps({'ffmpeg': version, 'inputs': inputs, 'references': refs}, indent=1))
    print(f"{sum(r['status'] == 'encoded' for r in inputs)} of {len(inputs)} encodes, {len(tsv)} references")


def wav_floats(blob):
    """(channels, float samples) of a WAV file of IEEE floats."""
    at, channels = 12, 0
    while at + 8 <= len(blob):
        kind, size = blob[at:at + 4], int.from_bytes(blob[at + 4:at + 8], 'little')
        if kind == b'fmt ':
            channels = int.from_bytes(blob[at + 10:at + 12], 'little')
        if kind == b'data':
            samples = array.array('f')
            samples.frombytes(blob[at + 8:at + 8 + size - size % 4])
            return channels, samples
        at += 8 + size + (size & 1)
    raise ValueError('no data chunk')


def level_only(r):
    """Frames compared by level only: noise bands, and intensity bands under
    an ms_mask_present of 2 (docs/design/audio-decode.md, "AAC-LC bars")."""
    return r.get('noise_frames', 1) + r.get('intensity_all_ms_frames', 1)


def bar2_verdict(r, ref):
    if ref['returncode'] != 0 or ref['errors']:
        return 'ffmpeg error'
    if r is None:
        return 'missing'
    if r['status'] == 'refused':
        return 'refused'
    n = r['packets'] * r['frame']
    problems = []
    if r['damaged']:
        problems.append('damaged frames')
    if r['length'] != n or r['reference_length'] != n or r['reference_values'] != n * r['channels']:
        problems.append('lengths')
    for c in r['per_channel']:
        if c['rms'] > MAX_RMS or c['max'] > MAX_ABS:
            problems.append('samples')
        if level_only(r) and (c['noise_db'] is None or
                                  (c['noise_blocks'] >= MIN_NOISE_BLOCKS and c['noise_db'] > MAX_NOISE_DB)):
            problems.append('noise level')
    return 'pass' if not problems else 'FAIL: ' + ', '.join(sorted(set(problems)))


def judge(a):
    work = a.work
    meta = json.loads((work / 'set.json').read_text())
    decoded = {}
    for line in (work / 'decoder.jsonl').read_text().splitlines():
        r = json.loads(line)
        decoded[r['input']] = r
    out = {'ffmpeg': meta['ffmpeg'], 'not_encoded': [r['name'] for r in meta['inputs'] if r['status'] != 'encoded']}
    # Bar 2.
    bar2 = []
    for path, ref in meta['references'].items():
        r = decoded.get(path)
        row = {'input': ref['key'], 'verdict': bar2_verdict(r, ref)}
        if r and r['status'] == 'decoded':
            row |= {k: r[k] for k in ('channels', 'frame', 'packets', 'noise_frames', 'intensity_all_ms_frames',
                                      'pce', 'matched')}
            row['rms'] = max(c['rms'] for c in r['per_channel'])
            row['max'] = max(c['max'] for c in r['per_channel'])
            row['noise'] = [[c['noise_db'], c['noise_blocks'], c['noise_block_db']] for c in r['per_channel']]
        elif r:
            row['reason'] = r.get('reason')
        if ref['returncode'] != 0 or ref['errors']:
            row['ffmpeg_stderr'] = ref['stderr']
        bar2.append(row)
    counted = [r for r in bar2 if r['verdict'] == 'pass' or r['verdict'].startswith('FAIL') or r['verdict'] == 'missing']
    out['bar2'] = {'counted': len(counted), 'passed': sum(r['verdict'] == 'pass' for r in counted),
                   'refused': [[r['input'], r.get('reason')] for r in bar2 if r['verdict'] == 'refused'],
                   'ffmpeg_errors': [r['input'] for r in bar2 if r['verdict'] == 'ffmpeg error'],
                   'largest_difference': max((r['max'] for r in counted if 'max' in r), default=0),
                   'largest_rms': max((r['rms'] for r in counted if 'rms' in r), default=0),
                   'results': bar2}
    # Bar 3: the generated MP4 inputs, gating where no frame codes noise.
    gen = [r for r in meta['inputs'] if r['status'] == 'encoded']
    files = [work / 'gen' / f"{r['name']}.m4a" for r in gen]
    check = run([sys.executable, '-I', ROOT / 'scripts' / 'decode_audio_check.py', '--coreav', a.coreav,
                 '--ffmpeg', a.ffmpeg, '--ffprobe', a.ffprobe, '--out', work / 'edit-lists.json', *files])
    results = json.loads((work / 'edit-lists.json').read_text())['results']
    bar3 = []
    for r, f, res in zip(gen, files, results):
        d = decoded.get(str(f))
        noisy = d is None or level_only(d) != 0
        ok = res.get('status') == 'ok' and res.get('model') and res.get('timeline')
        clean = res.get('status') == 'ok' and not res.get('decode_errors')
        bar3.append({'input': r['name'], 'gating': not noisy, 'status': res.get('status'),
                     'clean': clean, 'model': res.get('model'), 'timeline': res.get('timeline'),
                     'ffmpeg': res.get('ffmpeg'), 'entries': res.get('entries', []), 'pass': bool(ok)})
    gating = [r for r in bar3 if r['gating']]
    out['bar3'] = {'gating': len(gating), 'passed': sum(r['pass'] for r in gating),
                   'reported': [[r['input'], r['pass']] for r in bar3 if not r['gating']],
                   'ffmpeg_default': {v: sum(r['ffmpeg'] == v for r in bar3) for v in ('equal', 'covered', 'uncovered')},
                   'check_exit': check.returncode, 'results': bar3}
    # Bars 4 and 5.
    bar4, bar5 = [], []
    for r in gen:
        for ext in ('m4a', 'mka'):
            if ext == 'mka' and not r['mka']:
                continue
            f = work / 'gen' / f"{r['name']}.{ext}"
            outs = [run([a.coreav, 'decode-audio', '--format', 'f32', '--threads', t, f, '-']) for t in ('1', '8')]
            same = all(o.returncode == 0 for o in outs) and outs[0].stdout == outs[1].stdout
            bar5.append({'input': f.name, 'same': same,
                         'sha256': hashlib.sha256(outs[0].stdout).hexdigest() if same else None})
            if ext != 'mka' or outs[0].returncode != 0:
                continue
            channels, ours = wav_floats(outs[0].stdout)
            ff = run([a.ffmpeg, '-v', 'error', '-i', f, '-map', '0:a:0', '-f', 'f32le', '-'])
            theirs = array.array('f')
            theirs.frombytes(ff.stdout[:len(ff.stdout) - len(ff.stdout) % 4])
            n = min(len(ours), len(theirs)) // channels * channels
            sq, worst = 0.0, 0.0
            for x, y in zip(ours[:n], theirs[:n]):
                d = abs(x - y)
                sq += d * d
                worst = max(worst, d)
            d = decoded.get(str(work / 'gen' / f"{r['name']}.m4a")) or {}
            bar4.append({'input': f.name, 'level_only_frames': level_only(d) if d else None, 'length': len(ours) // channels,
                         'ffmpeg_length': len(theirs) // channels, 'rms': (sq / max(n, 1)) ** 0.5, 'max': worst})
    out['bar4'] = bar4
    out['bar5'] = {'inputs': len(bar5), 'same': sum(r['same'] for r in bar5), 'results': bar5}
    bad = (out['bar2']['passed'] != out['bar2']['counted'] or out['bar3']['passed'] != out['bar3']['gating']
           or out['bar5']['same'] != out['bar5']['inputs'])
    out['verdict'] = 'missed' if bad else 'met'
    a.out.write_text(json.dumps(out, indent=1) + '\n')
    summary = {k: {kk: vv for kk, vv in v.items() if kk != 'results'} if isinstance(v, dict) else v
               for k, v in out.items() if k != 'bar4'}
    summary['bar4'] = {'inputs': len(bar4), 'largest_rms_without_noise': max(
        (r['rms'] for r in bar4 if r['level_only_frames'] == 0), default=0)}
    print(json.dumps(summary, indent=1))
    return 1 if bad else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('prepare')
    p.add_argument('--ffmpeg', required=True)
    p.add_argument('--work', type=pathlib.Path, required=True)
    p.add_argument('--conformance', type=pathlib.Path)
    j = sub.add_parser('judge')
    for name in ('--coreav', '--ffmpeg', '--ffprobe'):
        j.add_argument(name, required=True)
    j.add_argument('--work', type=pathlib.Path, required=True)
    j.add_argument('--out', type=pathlib.Path, required=True)
    a = parser.parse_args()
    return prepare(a) if a.command == 'prepare' else judge(a)


if __name__ == '__main__':
    sys.exit(main() or 0)
