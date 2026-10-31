#!/usr/bin/env python3
"""The HE-AAC bars of docs/design/audio-decode.md ("HE-AAC bars", "HE-AAC
sets"), run by .github/workflows/audio-conformance.yml.

  he_aac_agreement.py sources --ffmpeg F --out DIR
      Writes the generated set's sources (DIR/src-*.wav) with the pinned
      ffmpeg, and the list of encodes (DIR/encodes.json).
  he_aac_agreement.py encode --dir DIR
      On macOS: encodes each source with Apple's encoder (afconvert) into
      DIR/gen/*.m4a and records the runner and each encode (DIR/encoded.json).
  he_aac_agreement.py select --ffmpeg F --ffprobe P --work DIR
      Selects the corpus set from the corpus store's manifests (the R2_*
      variables; needs boto3) and copies its files into DIR/corpus/ under
      neutral names. Nothing it writes names a file.
  he_aac_agreement.py prepare --ffmpeg F --ffprobe P --work DIR --conformance DIR --generated DIR
      Writes each input's variants (Matroska copy, implicit signalling),
      the oracle's decode of every input (DIR/ref/*.f32), what the oracle
      states of each (DIR/set.json), and DIR/agreement.tsv, the list the
      decoder test reads: input, the oracle's decode, the ISO references.
  he_aac_agreement.py judge --coreav C --ffmpeg F --ffprobe P --work DIR --out OUT.json
      Reads the decoder test's DIR/decoder.jsonl (bars 1, 2, 4 and 5), runs
      scripts/decode_audio_check.py (bar 3) and the thread counts (bar 6),
      writes OUT.json and exits 1 when a gating bar is missed.

Standard library only, except select (boto3); ffmpeg, ffprobe and
afconvert are black boxes.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import platform
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'scripts'))
from aac_agreement import source  # noqa: E402
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT  # noqa: E402

# "HE-AAC bars": the tolerance against an ISO reference, and twice it
# against FFmpeg.
REF_RMS = 2.0 ** -15 / 12 ** 0.5
REF_ABS = 2.0 ** -12
MAX_RMS = 2 * REF_RMS
MAX_ABS = 2 * REF_ABS
MAX_NOISE_DB = 0.1
MIN_NOISE_BLOCKS = 100
TIMEOUT = 600
RATES = (32000, 44100, 48000)
KINDS = ('tones', 'noise', 'clicks', 'sweep')
CHANNELS = {'mono': 1, 'stereo': 2, '5.1': 6}
CORPORA = ('v1', 'v1-fmp4-large', 'v1-ts-audio')
CORPUS_EXTENSIONS = ('mp4', 'm4a', 'm4b', 'mov', '3gp', 'mkv', 'mka')
HE_PROFILES = ('HE-AAC', 'HE-AACv2')
MATROSKA = ('.mkv', '.mka')
# "HE-AAC bars", bar 2: the only waivers, from the comparison with FFmpeg's
# samples and length alone, with each stream's Matroska copy and implicit
# variant.
WAIVED = frozenset([
    'al_sbr_i_32_1_new', 'al_sbr_i_44_1_new', 'al_sbr_i_48_1_new',
    'al960_sbr_i_32_1', 'al960_sbr_i_32_2', 'al960_sbr_i_44_1', 'al960_sbr_i_44_2', 'al960_sbr_i_48_1',
    'al960_sbr_i_48_2', 'al960_sbr_ps_04', 'al_sbr_cm_48_5'])
SAMPLE_RATES = (96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350)


def encodes():
    """The generated set ("HE-AAC sets", "Generated"), one dict per encode."""
    out = []
    for kind in KINDS:
        for rate in RATES:
            for layout in ('mono', 'stereo'):
                out.append({'name': f'he-{kind}-{rate}-{layout}', 'group': 'he-aac', 'kind': kind, 'rate': rate,
                            'layout': layout, 'codec': 'aach', 'bitrate': None})
    for kind in KINDS:
        for rate in RATES:
            out.append({'name': f'he2-{kind}-{rate}-stereo', 'group': 'he-aac-v2', 'kind': kind, 'rate': rate,
                        'layout': 'stereo', 'codec': 'aacp', 'bitrate': None})
    for kind in ('tones', 'clicks'):
        for codec in ('aach', 'aacp'):
            for bitrate in (24000, 64000):
                out.append({'name': f'rate-{codec}-{kind}-{bitrate // 1000}k', 'group': 'rates', 'kind': kind,
                            'rate': 48000, 'layout': 'stereo', 'codec': codec, 'bitrate': bitrate})
    out.append({'name': 'layout-5.1', 'group': 'layouts', 'kind': 'tones+clicks', 'rate': 48000, 'layout': '5.1',
                'codec': 'aach', 'bitrate': None})
    return out


def source_name(e):
    return f"src-{e['kind']}-{e['rate']}-{e['layout']}.wav"


def run(args, timeout=TIMEOUT, **kw):
    return subprocess.run([str(a) for a in args], capture_output=True, timeout=timeout, **kw)


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def sources(a):
    a.out.mkdir(parents=True, exist_ok=True)
    plan = encodes()
    written = {}
    for e in plan:
        name = source_name(e)
        if name in written:
            continue
        lavfi = source(e['kind'], e['rate'], CHANNELS[e['layout']], e['layout'])
        made = run([a.ffmpeg, '-v', 'error', '-y', '-f', 'lavfi', '-i', lavfi, '-c:a', 'pcm_s16le', a.out / name])
        if made.returncode != 0:
            raise SystemExit(f"{name}: {made.stderr.decode(errors='replace')[-300:]}")
        written[name] = sha256(a.out / name)
    (a.out / 'encodes.json').write_text(json.dumps({'encodes': plan, 'sources': written}, indent=1) + '\n')
    print(f'{len(written)} sources for {len(plan)} encodes')


def encode(a):
    plan = json.loads((a.dir / 'encodes.json').read_text())
    gen = a.dir / 'gen'
    gen.mkdir(exist_ok=True)
    results = []
    for e in plan['encodes']:
        out = gen / f"{e['name']}.m4a"
        args = ['afconvert', '-f', 'm4af', '-d', e['codec']]
        if e['bitrate']:
            args += ['-b', str(e['bitrate'])]
        done = run(args + [a.dir / source_name(e), out])
        ok = done.returncode == 0 and out.exists() and out.stat().st_size > 0
        results.append({**e, 'status': 'encoded' if ok else 'not encoded', 'sha256': sha256(out) if ok else None,
                        'error': None if ok else (done.stderr or done.stdout).decode(errors='replace')[-300:]})
        if not ok and out.exists():
            out.unlink()
    runner = {'macos': ' '.join(run(['sw_vers']).stdout.decode().split()), 'machine': platform.machine(),
              'image': os.environ.get('ImageOS'), 'image_version': os.environ.get('ImageVersion')}
    (a.dir / 'encoded.json').write_text(json.dumps({'runner': runner, 'encodes': results}, indent=1) + '\n')
    print(f"{sum(r['status'] == 'encoded' for r in results)} of {len(results)} encoded on {runner['macos']}")


class Bits:
    def __init__(self, data):
        self.data, self.at = data, 0

    def read(self, n):
        value = 0
        for _ in range(n):
            byte = self.at >> 3
            bit = (self.data[byte] >> (7 - (self.at & 7))) & 1 if byte < len(self.data) else 0
            value, self.at = value << 1 | bit, self.at + 1
        return value

    def left(self):
        return len(self.data) * 8 - self.at


def object_type(bits):
    aot = bits.read(5)
    return 32 + bits.read(6) if aot == 31 else aot


def sample_rate(bits):
    index = bits.read(4)
    if index == 15:
        return bits.read(24)
    return SAMPLE_RATES[index] if index < len(SAMPLE_RATES) else None


def skip_program(bits):
    """Reads past a program_config_element as crates/coreav-aac's
    configuration parser does; byte alignment counts from the start."""
    bits.read(10)
    front, side, back, lfe, assoc, cc = (bits.read(n) for n in (4, 4, 4, 2, 3, 4))
    for _ in range(2):
        if bits.read(1):
            bits.read(4)
    if bits.read(1):
        bits.read(3)
    bits.read(5 * (front + side + back) + 4 * lfe + 4 * assoc + 5 * cc)
    bits.read(-bits.at % 8)
    bits.read(8 * bits.read(8))


def signalling(asc):
    """The SBR and PS signalling of an AudioSpecificConfig: hierarchical
    (object type 5 or 29 first), backward-compatible (the 0x2b7 and 0x548
    extensions after the core's configuration) or none (SBR, if any, is
    implicit), with the core and extension rates."""
    bits = Bits(asc)
    first = object_type(bits)
    core = sample_rate(bits)
    channel_configuration = bits.read(4)
    sbr, ps, extension, aot = 'none', False, None, first
    if first in (5, 29):
        sbr, ps = 'hierarchical', first == 29
        extension = sample_rate(bits)
        aot = object_type(bits)
    frame = None
    if aot == 2:
        frame = 960 if bits.read(1) else 1024
        if bits.read(1):
            bits.read(14)
        has_extension = bits.read(1)
        if channel_configuration == 0:
            skip_program(bits)
        if has_extension:
            bits.read(1)
        if sbr == 'none' and bits.left() >= 16 and bits.read(11) == 0x2b7:
            if object_type(bits) == 5 and bits.read(1):
                sbr = 'compatible'
                extension = sample_rate(bits)
                if bits.left() >= 12 and bits.read(11) == 0x548:
                    ps = bool(bits.read(1))
    return {'object_type': first, 'core_rate': core, 'channel_configuration': channel_configuration,
            'frame': frame, 'sbr': sbr, 'ps': ps, 'extension_rate': extension,
            'downsampled': extension is not None and extension == core}


def has_implicit_variant(sig):
    """ADTS carries neither frames of 960 samples nor the signalling of
    downsampled SBR, so those round trips change the stream ("HE-AAC sets")."""
    return sig['frame'] == 1024 and not sig['downsampled']


def reader_options(path):
    """-ignore_editlist is an option of the MP4/MOV reader alone; the Matroska
    reader refuses it and the command exits with 8. -flags2 +skip_manual is a
    decoder flag and applies to both."""
    return [] if pathlib.Path(path).suffix in MATROSKA else ['-ignore_editlist', '1']


def container(path):
    return 'matroska' if pathlib.Path(path).suffix in MATROSKA else 'mp4'


def probe(ffprobe, path):
    """What the oracle states of an input's first audio stream, with the
    samples its packets' durations state at the output rate. Matroska states
    durations in milliseconds, which do not hold an AAC frame exactly, so
    there the samples are the packets times the frame length at the output
    rate (\"HE-AAC bars\", \"Length\")."""
    out = run([ffprobe, '-v', 'error', *reader_options(path), '-select_streams', 'a:0', '-show_entries',
               'packet=duration:stream=codec_name,profile,time_base,sample_rate,channels,extradata', '-show_data',
               '-of', 'json', path])
    try:
        info = json.loads(out.stdout)
        stream = info['streams'][0]
        num, den = (int(x) for x in stream['time_base'].split('/'))
        rate, packets = int(stream['sample_rate']), len(info.get('packets', []))
        stated = round(sum(int(p.get('duration', 0)) for p in info.get('packets', [])) * num * rate / den)
        extradata = ''.join(line.split(':', 1)[1][:40].replace(' ', '')
                            for line in stream.get('extradata', '').splitlines() if ':' in line)
        if container(path) == 'matroska':
            sig = signalling(bytes.fromhex(extradata)) if extradata else None
            stated = packets * sig['frame'] * rate // sig['core_rate'] if sig and sig['frame'] else None
        return {'codec': stream.get('codec_name'), 'profile': stream.get('profile'), 'packets': packets,
                'stated_samples': stated, 'asc': extradata}
    except (ValueError, KeyError, IndexError):
        return {'codec': None, 'profile': None, 'packets': None, 'stated_samples': None, 'asc': None}


def wav_format(blob):
    """(rate, channels, channel mask) of a WAV header; a WAV without an
    extensible format has its channel count's default mask."""
    at = 12
    while at + 8 <= len(blob):
        kind, size = blob[at:at + 4], int.from_bytes(blob[at + 4:at + 8], 'little')
        if kind == b'fmt ':
            fmt = blob[at + 8:at + 8 + size]
            tag = int.from_bytes(fmt[0:2], 'little')
            channels = int.from_bytes(fmt[2:4], 'little')
            mask = int.from_bytes(fmt[20:24], 'little') if tag == 0xfffe and size >= 24 else {1: 0x4, 2: 0x3}.get(channels, 0)
            return int.from_bytes(fmt[4:8], 'little'), channels, mask
        at += 8 + size + (size & 1)
    raise ValueError('no fmt chunk')


def bar2_decode(ffmpeg, path, out):
    """Bar 2's oracle decode: every packet's samples, no edit list applied
    and no codec delay removed."""
    return [ffmpeg, '-v', 'warning', '-y', *reader_options(path), '-flags2', '+skip_manual', *AUDIO_DECODE_INPUT, '-i', path,
            '-map', '0:a:0', '-f', 'f32le', out]


def oracle(a, path, out):
    """The oracle's decode of every packet into `out`, and the format of its WAV."""
    dec = run(bar2_decode(a.ffmpeg, path, out))
    head = run([a.ffmpeg, '-v', 'error', *reader_options(path), *AUDIO_DECODE_INPUT, '-i', path, '-map', '0:a:0',
                '-t', '0.1', '-f', 'wav', '-'])
    try:
        rate, channels, mask = wav_format(head.stdout)
    except ValueError:
        rate = channels = mask = None
    stderr = dec.stderr.decode(errors='replace')
    size = pathlib.Path(out).stat().st_size if pathlib.Path(out).exists() else 0
    return {'returncode': dec.returncode, 'warnings': bool(stderr.strip()), 'stderr': stderr[-300:],
            'rate': rate, 'channels': channels, 'mask': mask,
            'samples': size // 4 // channels if channels else None}


def variants(a, path, sig, folder, stem):
    """The Matroska copy of an MP4 input and, where the round trip keeps the
    stream, its implicit-signalling variant."""
    made = {}
    mka = folder / f'{stem}.mka'
    if run([a.ffmpeg, '-v', 'error', '-y', '-i', path, '-map', '0:a:0', '-c', 'copy', mka]).returncode == 0:
        made['mka'] = mka
    if has_implicit_variant(sig):
        adts, implicit = folder / f'{stem}.adts', folder / f'{stem}.implicit.m4a'
        first = run([a.ffmpeg, '-v', 'error', '-y', '-i', path, '-map', '0:a:0', '-c', 'copy', '-f', 'adts', adts])
        if first.returncode == 0 and run([a.ffmpeg, '-v', 'error', '-y', '-i', adts, '-c', 'copy', implicit]).returncode == 0:
            made['implicit'] = implicit
        adts.unlink(missing_ok=True)
    return made


def references(conformance, name):
    """The ISO references of a conformance stream for bar 1: the
    high-quality SBR one, or for the PS streams and their _new copies the
    unrestricted PS one; one file, or one per channel."""
    folder = conformance / 'referencesWav' / 'add-opt'
    if name.startswith('al_sbr_ps_'):
        stem = 'al_sbr_ps_' + name[len('al_sbr_ps_'):].removesuffix('_new') + '_ur'
    elif name.startswith('al_sbr_'):
        stem = 'al_sbr_hq_' + name[len('al_sbr_'):]
    else:
        return []
    single = folder / f'{stem}.wav'
    if single.exists():
        return [single]
    return sorted(p for p in folder.glob(f'{stem}_*.wav')
                  if len(p.stem) == len(stem) + 4 and p.stem[len(stem) + 2:].isdigit())


def prepare(a):
    work = a.work
    for sub in ('var', 'ref'):
        (work / sub).mkdir(parents=True, exist_ok=True)
    inputs = [{'key': f'conf-{f.stem}', 'set': 'conformance', 'path': f, 'references': references(a.conformance, f.stem)}
              for f in sorted((a.conformance / 'compressedMp4').glob('al*_sbr_*.mp4'))]
    encoded = json.loads((a.generated / 'encoded.json').read_text())
    inputs += [{'key': f"gen-{e['name']}", 'set': 'generated', 'path': a.generated / 'gen' / f"{e['name']}.m4a",
                'references': []} for e in encoded['encodes'] if e['status'] == 'encoded']
    corpus = work / 'corpus'
    inputs += [{'key': f'corpus-{f.stem}', 'set': 'corpus', 'path': f, 'references': []}
               for f in (sorted(corpus.glob('c*.*')) if corpus.exists() else [])]
    rows, tsv = [], []
    for i in inputs:
        stated = probe(a.ffprobe, i['path'])
        sig = signalling(bytes.fromhex(stated['asc'])) if stated['asc'] else None
        matroska = i['path'].suffix in MATROSKA
        row = {'key': i['key'], 'set': i['set'], 'path': str(i['path']), 'variant': 'matroska' if matroska else 'mp4',
               'of': None, 'references': [str(r) for r in i['references']], **stated, 'signalling': sig}
        made = {}
        if i['set'] != 'corpus' and sig:
            made = variants(a, i['path'], sig, work / 'var', i['key'])
        elif matroska:
            copy = work / 'var' / f"{i['key']}.mp4"
            done = run([a.ffmpeg, '-v', 'error', '-y', '-i', i['path'], '-map', '0:a:0', '-c', 'copy', copy])
            if done.returncode == 0:
                made = {'copy': copy}
            else:
                row['copy_failed'] = done.stderr.decode(errors='replace')[-300:]
        derived = []
        for kind, path in made.items():
            more = probe(a.ffprobe, path)
            derived.append({**row, 'key': f"{i['key']}.{kind}", 'path': str(path), 'variant': kind, 'of': i['key'],
                            'references': [], **more,
                            'signalling': signalling(bytes.fromhex(more['asc'])) if more['asc'] else None})
        for r in [row] + derived:
            ref = work / 'ref' / f"{r['key']}.f32"
            r['oracle'] = oracle(a, r['path'], ref)
            r['container'] = container(r['path'])
            tsv.append('\t'.join([r['path'], str(ref) if r['oracle']['returncode'] == 0 else '',
                                  ';'.join(r['references'])]))
            rows.append(r)
    (work / 'agreement.tsv').write_text('\n'.join(tsv) + '\n')
    version = run([a.ffmpeg, '-version']).stdout.decode().splitlines()[0]
    (work / 'set.json').write_text(json.dumps({
        'ffmpeg': version, 'encoder': encoded['runner'],
        'not_encoded': [e['name'] for e in encoded['encodes'] if e['status'] != 'encoded'], 'inputs': rows},
        indent=1) + '\n')
    counts = {s: sum(r['set'] == s and r['of'] is None for r in rows) for s in ('conformance', 'generated', 'corpus')}
    decoded = sum(r['oracle']['returncode'] == 0 for r in rows)
    print(f'{len(rows)} inputs with variants (originals: {counts}), {decoded} decoded by the oracle')


def candidate(row):
    """The manifest half of the corpus rule ("HE-AAC sets", "Corpus"): not
    held out, and an extension of the containers the pipeline reads. The
    other half is the oracle's profile of the first audio stream."""
    return row.get('holdout') is False and row.get('ext') in CORPUS_EXTENSIONS


def clean_decode(ffmpeg, path, scratch):
    """The last condition of the corpus rule: bar 2's oracle decode exits
    with 0 and prints nothing at -v warning."""
    try:
        done = run(bar2_decode(ffmpeg, path, scratch))
    except subprocess.TimeoutExpired:
        return False
    finally:
        scratch.unlink(missing_ok=True)
    return done.returncode == 0 and not done.stderr.strip()


def dropped_classes(files):
    """The container classes the corpus selection empties (\"HE-AAC sets\",
    \"Corpus\"): files is one (class, answered, he_aac, clean) per candidate.
    A class is dropped when ffprobe answers for none of its candidates, or
    when it has HE-AAC files and none of them decodes cleanly."""
    dropped = []
    for cls in sorted({f[0] for f in files}):
        mine = [f for f in files if f[0] == cls]
        if not any(f[1] for f in mine) or (any(f[2] for f in mine) and not any(f[3] for f in mine)):
            dropped.append(cls)
    return dropped


def select(a):
    import boto3
    s3 = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'], aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'],
                      aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'], region_name='auto')
    bucket = os.environ['R2_BUCKET']
    candidates = []
    for corpus in CORPORA:
        manifest = json.loads(s3.get_object(Bucket=bucket, Key=f'corpus/{corpus}/manifest.json')['Body'].read())
        for row in manifest['files']:
            if candidate(row):
                key = row['key'] if row['key'].startswith('corpus/') else f"corpus/{corpus}/{row['key']}"
                candidates.append((row['sha256'], row['ext'], key))
    candidates.sort()

    def profile(c):
        url = s3.generate_presigned_url('get_object', Params={'Bucket': bucket, 'Key': c[2]}, ExpiresIn=3600)
        try:
            out = run([a.ffprobe, '-v', 'error', '-rw_timeout', '30000000', '-select_streams', 'a:0',
                       '-show_entries', 'stream=codec_name,profile', '-of', 'json', url], timeout=180)
            stream = (json.loads(out.stdout).get('streams') or [{}])[0]
            return stream.get('codec_name'), stream.get('profile')
        except (subprocess.TimeoutExpired, ValueError):
            return None, None

    with concurrent.futures.ThreadPoolExecutor(16) as pool:
        profiles = list(pool.map(profile, candidates))
    folder = a.work / 'corpus'
    folder.mkdir(parents=True, exist_ok=True)
    files, kept = [], 0
    for (_, ext, key), (codec, prof) in zip(candidates, profiles):
        he_aac, clean = codec == 'aac' and prof in HE_PROFILES, False
        if he_aac:
            path = folder / f'candidate.{ext}'
            s3.download_file(bucket, key, str(path))
            clean = clean_decode(a.ffmpeg, path, folder / 'candidate.f32')
            if clean:
                kept += 1
                path.rename(folder / f'c{kept:03d}.{ext}')
            else:
                path.unlink()
        files.append((container(f'x.{ext}'), codec is not None, he_aac, clean))
    summary = {'candidates': len(candidates), 'unanswered': sum(not f[1] for f in files),
               'he_aac': sum(f[2] for f in files), 'not_decoded_cleanly': sum(f[2] and not f[3] for f in files),
               'selected': kept, 'profiles': {p: sum(prof == p for _, prof in profiles) for p in HE_PROFILES},
               'classes': {cls: {'candidates': sum(f[0] == cls for f in files),
                                 'he_aac': sum(f[0] == cls and f[2] for f in files),
                                 'selected': sum(f[0] == cls and f[3] for f in files)}
                           for cls in ('mp4', 'matroska')},
               'dropped_classes': dropped_classes(files)}
    (a.work / 'corpus.json').write_text(json.dumps(summary, indent=1) + '\n')
    print(json.dumps(summary))
    if summary['dropped_classes']:
        sys.exit('the corpus selection dropped every file of: ' + ', '.join(summary['dropped_classes']))


def level_only(r):
    """Frames whose output blocks are compared by level only (noise bands,
    intensity under an all-ones mask); unknown counts as some."""
    return r.get('noise_frames', 1) + r.get('intensity_all_ms_frames', 1)


def within(pair, rms, worst):
    return pair is not None and pair[0] <= rms and pair[1] <= worst


def bar1_verdict(r):
    if r is None:
        return 'missing'
    if r['status'] != 'decoded':
        return 'FAIL: ' + (r.get('reason') or r['status'])
    return 'pass' if within((r.get('reference') or {}).get('coreav'), REF_RMS, REF_ABS) else 'FAIL: samples'


def waived(ref):
    """Whether an input is one of bar 2's named waivers (or a variant of one)."""
    return ref['set'] == 'conformance' and ref['key'].removeprefix('conf-').split('.')[0] in WAIVED


def bar2_verdict(r, ref):
    """Bar 2 on the decoder test's row r and the oracle's statement ref of
    one input. A waived stream leaves only the comparison with FFmpeg's
    samples and length; the rest still gates."""
    if r is None:
        return 'missing'
    if r['status'] != 'decoded':
        return 'FAIL: ' + (r.get('reason') or r['status'])
    o = ref['oracle']
    excused = waived(ref)
    n = r['packets'] * r['frame']
    gating, compared = set(), set()
    if r['damaged']:
        gating.add('damaged frames')
    if r['length'] != n:
        gating.add('lengths')
    if (r['rate'], r['channels']) != (o['rate'], o['channels']) or (not r.get('pce') and r['mask'] != o['mask']):
        gating.add('format')
    if o['returncode'] != 0 or o['warnings']:
        compared.add('ffmpeg error')
    theirs = ref.get('stated_samples')
    if (theirs is None or theirs > n or r['reference_length'] != theirs
            or r['reference_values'] != theirs * r['channels']):
        compared.add('ffmpeg length')
    for c in r['per_channel']:
        if c['rms'] > MAX_RMS or c['max'] > MAX_ABS:
            compared.add('samples')
        if level_only(r) and (c['noise_db'] is None or
                              (c['noise_blocks'] >= MIN_NOISE_BLOCKS and c['noise_db'] > MAX_NOISE_DB)):
            compared.add('noise level')
    failed = gating | (set() if excused else compared)
    if failed:
        return 'FAIL: ' + ', '.join(sorted(failed))
    return 'pass (waived: samples not compared)' if excused else 'pass'


def passed(row):
    return row.get('verdict', '').startswith('pass') or row.get('pass') is True


def same_decode(r, other):
    return (r is not None and other is not None and r['status'] == other['status'] == 'decoded'
            and r['sha256'] == other['sha256'] and (r['rate'], r['channels']) == (other['rate'], other['channels']))


def judge_rows(meta, decoded):
    """Bars 1, 2, 4 and 5: the oracle's statements against the decoder
    test's rows (keyed by input path)."""
    by_key = {r['key']: r for r in meta['inputs']}
    bar1, bar2, bar4, bar5 = [], [], [], []
    for ref in meta['inputs']:
        r = decoded.get(ref['path'])
        if ref['references']:
            bar1.append({'input': ref['key'], 'verdict': bar1_verdict(r), 'reference': (r or {}).get('reference')})
        if ref['variant'] != 'copy':
            row = {'input': ref['key'], 'set': ref['set'], 'verdict': bar2_verdict(r, ref)}
            if r and r['status'] == 'decoded':
                row |= {'rms': max((c['rms'] for c in r['per_channel']), default=0),
                        'max': max((c['max'] for c in r['per_channel']), default=0),
                        'noise_frames': r.get('noise_frames'), 'rate': r['rate'], 'channels': r['channels']}
            if ref['oracle']['warnings'] or ref['oracle']['returncode']:
                row['ffmpeg_stderr'] = ref['oracle']['stderr']
            bar2.append(row)
        if ref['variant'] == 'implicit':
            o = ref['oracle']
            ok = same_decode(r, decoded.get(by_key[ref['of']]['path'])) and (o['rate'], o['channels']) == (
                r['rate'], r['channels'])
            bar4.append({'input': ref['key'], 'pass': ok})
        if ref['variant'] == 'mka':
            bar5.append({'input': ref['key'], 'pass': same_decode(r, decoded.get(by_key[ref['of']]['path']))})
        if ref['variant'] == 'copy':
            bar5.append({'input': ref['of'], 'pass': same_decode(decoded.get(by_key[ref['of']]['path']), r)})
        if ref['variant'] == 'matroska' and 'copy_failed' in ref:
            bar5.append({'input': ref['key'], 'pass': False, 'reason': 'not copied into MP4'})
    return bar1, bar2, bar4, bar5


def bar3_pass(res, samples):
    """Bar 3 on one result of scripts/decode_audio_check.py: the length and
    the timeline always; the samples where they are compared."""
    if res.get('status') != 'ok' or not res.get('timeline') or not res.get('length'):
        return False
    return bool(res.get('model')) or not samples


def summary_of(rows):
    return {'counted': len(rows), 'passed': sum(passed(r) for r in rows)}


def verdict(out):
    bars = [out[f'bar{n}'] for n in range(1, 7)]
    return 'met' if all(b['counted'] and b['passed'] == b['counted'] for b in bars) else 'missed'


def judge(a):
    work = a.work
    meta = json.loads((work / 'set.json').read_text())
    decoded = {}
    for line in (work / 'decoder.jsonl').read_text().splitlines():
        r = json.loads(line)
        decoded[r['input']] = r
    bar1, bar2, bar4, bar5 = judge_rows(meta, decoded)
    out = {'ffmpeg': meta['ffmpeg'], 'encoder': meta['encoder'], 'not_encoded': meta['not_encoded']}
    out['bar1'] = summary_of(bar1) | {'results': bar1}
    out['bar2'] = summary_of(bar2) | {
        'samples_not_compared': [[r['input'], r['verdict']] for r in bar2 if r['verdict'].startswith('pass (')],
        'largest_difference': max((r['max'] for r in bar2 if r['verdict'] == 'pass'), default=0),
        'largest_rms': max((r['rms'] for r in bar2 if r['verdict'] == 'pass'), default=0),
        'results': bar2}
    # Bar 3: the generated and corpus MP4 files.
    mp4s = [r for r in meta['inputs'] if r['set'] in ('generated', 'corpus') and r['variant'] == 'mp4']
    results = []
    if mp4s:
        run([sys.executable, '-I', ROOT / 'scripts' / 'decode_audio_check.py', '--coreav', a.coreav,
             '--ffmpeg', a.ffmpeg, '--ffprobe', a.ffprobe, '--tolerance', MAX_RMS, MAX_ABS,
             '--out', work / 'timeline.json', *[r['path'] for r in mp4s]], timeout=TIMEOUT * len(mp4s))
        results = json.loads((work / 'timeline.json').read_text())['results']
    bar3 = []
    for r, res in zip(mp4s, results):
        d = decoded.get(r['path'])
        samples = d is not None and level_only(d) == 0
        bar3.append({'input': r['key'], 'samples_compared': samples, 'pass': bar3_pass(res, samples),
                     'samples_within': bool(res.get('model')), 'ffmpeg': res.get('ffmpeg')})
    out['bar3'] = summary_of(bar3) | {
        'ffmpeg_default': {v: sum(r['ffmpeg'] == v for r in bar3) for v in ('equal', 'covered', 'uncovered')},
        'results': bar3}
    out['bar4'] = summary_of(bar4) | {'results': bar4}
    out['bar5'] = summary_of(bar5) | {'results': bar5}
    # Bar 6: thread counts on the generated and corpus sets, both containers.
    bar6 = []
    for r in meta['inputs']:
        if r['set'] in ('generated', 'corpus') and r['variant'] != 'implicit':
            outs = [run([a.coreav, 'decode-audio', '--format', 'f32', '--threads', t, r['path'], '-']) for t in ('1', '8')]
            bar6.append({'input': r['key'],
                         'pass': all(o.returncode == 0 for o in outs) and outs[0].stdout == outs[1].stdout})
    out['bar6'] = summary_of(bar6) | {'results': bar6}
    out['verdict'] = verdict(out)
    a.out.write_text(json.dumps(out, indent=1) + '\n')
    print(json.dumps({k: {kk: vv for kk, vv in v.items() if kk != 'results'} if isinstance(v, dict) else v
                      for k, v in out.items()}, indent=1))
    return 0 if out['verdict'] == 'met' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest='command', required=True)
    s = sub.add_parser('sources')
    s.add_argument('--ffmpeg', required=True)
    s.add_argument('--out', type=pathlib.Path, required=True)
    e = sub.add_parser('encode')
    e.add_argument('--dir', type=pathlib.Path, required=True)
    c = sub.add_parser('select')
    for name in ('--ffmpeg', '--ffprobe'):
        c.add_argument(name, required=True)
    c.add_argument('--work', type=pathlib.Path, required=True)
    p = sub.add_parser('prepare')
    for name in ('--ffmpeg', '--ffprobe'):
        p.add_argument(name, required=True)
    for name in ('--work', '--conformance', '--generated'):
        p.add_argument(name, type=pathlib.Path, required=True)
    j = sub.add_parser('judge')
    for name in ('--coreav', '--ffmpeg', '--ffprobe'):
        j.add_argument(name, required=True)
    j.add_argument('--work', type=pathlib.Path, required=True)
    j.add_argument('--out', type=pathlib.Path, required=True)
    a = parser.parse_args()
    return {'sources': sources, 'encode': encode, 'select': select, 'prepare': prepare, 'judge': judge}[a.command](a)


if __name__ == '__main__':
    sys.exit(main() or 0)
