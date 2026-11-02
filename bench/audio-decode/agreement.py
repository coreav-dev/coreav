"""The Opus agreement and MP4 presentation model, with an injected executor."""
import contextlib
import math
import os
import pathlib
import re
import struct
import sys
import tempfile
import types

import numpy as np

import receipts

sys.path.append(str(pathlib.Path(__file__).resolve().parents[2] / 'scripts'))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

CHUNK = 1 << 22


def malformed(operation, tool):
    raise receipts.CommandFailure(receipts.receipt(operation, tool, 'protocol_error'))


@contextlib.contextmanager
def stage(operation, tool):
    try:
        yield
    except receipts.CommandFailure:
        raise
    except BaseException as error:
        raise receipts.CommandFailure(receipts.exception_receipt(error, operation, tool)) from None


def to_file(execute, operation, tool, args, dst, timeout, wav=False, f32=False):
    with stage(operation, tool):
        with tempfile.TemporaryDirectory(dir=dst.parent) as capture:
            source = pathlib.Path(capture) / 'stdout'
            execute(operation, tool, args, timeout, stdout_path=source)
            with source.open('rb') as incoming, dst.open('wb') as out:
                if wav:
                    head = incoming.read(12)
                    if len(head) != 12 or head[:4] != b'RIFF' or head[8:12] != b'WAVE':
                        malformed(operation, tool)
                    while True:
                        chunk = incoming.read(8)
                        if len(chunk) != 8:
                            malformed(operation, tool)
                        name, length = chunk[:4], struct.unpack('<I', chunk[4:])[0]
                        if name == b'data':
                            break
                        if length > source.stat().st_size - incoming.tell():
                            malformed(operation, tool)
                        incoming.seek(length + length % 2, 1)
                written = 0
                rest = b''
                while block := incoming.read(CHUNK):
                    if f32:
                        block = rest + block
                        cut = len(block) // 4 * 4
                        rest = block[cut:]
                        x = np.frombuffer(block[:cut], '<f4')
                        if not np.isfinite(x).all():
                            malformed(operation, tool)
                        data = np.clip(np.rint(x * 32768.0), -32768, 32767).astype('<i2').tobytes()
                    else:
                        data = block
                    out.write(data)
                    written += len(data)
                if rest or written % 2:
                    malformed(operation, tool)
                if wav and length not in (0xffffffff, 0) and written != length:
                    malformed(operation, tool)
        return written


def compare(execute, operation, tool, ref, test, channels, tmp, timeout):
    with stage(operation, 'opus_compare'):
        def one(a, b, stereo):
            if not stereo:
                dual = tmp / 'dual.sw'
                data = np.memmap(a, dtype='<i2', mode='r')
                with open(dual, 'wb') as out:
                    for at in range(0, len(data), CHUNK):
                        out.write(np.repeat(data[at:at + CHUNK], 2).tobytes())
                del data
                a = dual
            done = execute(operation, 'opus_compare', [tool, *(['-s'] if stereo else []), '-r', '48000', a, b], timeout)
            (tmp / 'dual.sw').unlink(missing_ok=True)
            text = (done.stdout + done.stderr).decode('ascii', errors='replace')
            metrics = re.findall(r'quality metric: (-?\d+(?:\.\d+)?)', text)
            passes = re.search(r'\bPASSES\b', text) is not None
            fails = re.search(r'\bFAILS\b', text) is not None
            if (len(metrics) != 1 or passes == fails or done.returncode not in (0, 1)
                    or (done.returncode == 1 and not fails)):
                raise receipts.CommandFailure({**done.receipt, 'outcome': 'protocol_error'})
            quality = float(metrics[0])
            if not math.isfinite(quality):
                malformed(operation, 'opus_compare')
            return passes, quality

        if channels <= 2:
            return one(ref, test, channels == 2)
        results = []
        for channel in range(channels):
            a, b = tmp / f'ref{channel}.sw', tmp / f'test{channel}.sw'
            for src, dst in ((ref, a), (test, b)):
                data = np.memmap(src, dtype='<i2', mode='r').reshape(-1, channels)
                with open(dst, 'wb') as out:
                    for at in range(0, len(data), CHUNK):
                        out.write(np.ascontiguousarray(data[at:at + CHUNK, channel]).tobytes())
                del data
            results.append(one(a, b, False))
            a.unlink()
            b.unlink()
        return all(p for p, _ in results), min(q for _, q in results)


def stats(ref, test, frames, channels):
    a = np.memmap(ref, dtype='<i2', mode='r')
    b = np.memmap(test, dtype='<i2', mode='r')
    n = frames * channels
    signal = noise = 0.0
    largest = 0
    for at in range(0, n, CHUNK):
        x = a[at:min(at + CHUNK, n)].astype(np.int64)
        d = x - b[at:min(at + CHUNK, n)].astype(np.int64)
        signal += float(np.dot(x, x))
        noise += float(np.dot(d, d))
        largest = max(largest, int(np.abs(d).max()) if len(d) else 0)
    snr = math.inf if noise == 0 else (10 * math.log10(signal / noise) if signal else -math.inf)
    return snr, largest


def truncate_copy(src, dst, size):
    with open(src, 'rb') as a, open(dst, 'wb') as b:
        left = size
        while left > 0 and (block := a.read(min(CHUNK, left))):
            b.write(block)
            left -= len(block)
    if left:
        malformed('oracle.model', 'harness')


def mp4_model(path, index, channels, stream, tmp, tools, dc, entries, execute, timeout):
    def reader(args):
        done = execute('oracle.reader', 'ffprobe', args, timeout)
        return types.SimpleNamespace(stdout=done.stdout, returncode=done.returncode)

    with stage('oracle.reader', 'ffprobe'):
        ref = dc.reference(path, types.SimpleNamespace(ffprobe=tools.ffprobe), executor=reader)
        if ref is None or ref['index'] != index:
            malformed('oracle.reader', 'ffprobe')
    frame = 2 * channels
    full_bytes = to_file(execute, 'oracle.full_decode', 'ffmpeg',
                         [tools.ffmpeg, '-v', 'error', '-nostdin', '-ignore_editlist', '1',
                          '-request_sample_fmt', 'flt', '-c:a', 'libopus', *AUDIO_DECODE_INPUT, '-i', path,
                          '-map', f'0:{index}', '-f', 'f32le', '-'], tmp / 'full.sw', timeout, f32=True)
    if full_bytes % frame:
        malformed('oracle.full_decode', 'ffmpeg')
    total = full_bytes // frame
    skip = int(stream.get('initial_padding') or 0)
    plan = None if ref['plan'] is None else [
        p if p[0] == 'silence' else ('media', p[1] - skip, p[2] - skip) for p in ref['plan']]
    edges = [e - skip for e in ref['edges']]
    if edges and edges[-1] < total:
        edges[-1] = total
    pieces = dc.effective(plan, total)
    with open(tmp / 'full.sw', 'rb') as full, open(tmp / 'model.sw', 'wb') as model:
        for piece in pieces:
            if piece[0] == 'silence':
                left = piece[1] * frame
                while left:
                    n = min(left, CHUNK)
                    model.write(bytes(n))
                    left -= n
            else:
                full.seek(piece[1] * frame)
                left = (piece[2] - piece[1]) * frame
                while left > 0 and (block := full.read(min(CHUNK, left))):
                    model.write(block)
                    left -= len(block)
                if left:
                    malformed('oracle.model', 'harness')
    model_samples = sum(dc.length(p) for p in pieces)
    applied = []
    if plan is None:
        expected = total
    else:
        written, applied = dc.ffmpeg_ops({'plan': plan, 'total': total, 'edges': edges, 'lossy': True}, entries)
        expected = sum(n for piece_ops, _ in written for _, _, n in piece_ops)
    (tmp / 'full.sw').unlink()
    return {'applied': plan is not None, 'model': model_samples, 'ffmpeg_expected': expected, 'full': total,
            'entries_count': len(applied)}


def judge(path, index, channels, stream, tmp, tools, dc, entries, execute, timeout):
    frame = 2 * channels
    ff = [tools.ffmpeg, '-v', 'error', '-nostdin']
    ref_bytes = to_file(execute, 'oracle.reference_decode', 'ffmpeg',
                        [*ff, '-request_sample_fmt', 'flt', '-c:a', 'libopus', *AUDIO_DECODE_INPUT, '-i', path,
                         '-map', f'0:{index}', '-f', 'f32le', '-'], tmp / 'ref.sw', timeout, f32=True)
    def_bytes = to_file(execute, 'oracle.default_decode', 'ffmpeg',
                        [*ff, *AUDIO_DECODE_INPUT, '-i', path, '-map', f'0:{index}', '-f', 's16le', '-'],
                        tmp / 'def.sw', timeout)
    ours_bytes = to_file(execute, 'oracle.coreav_decode', 'coreav',
                         [tools.coreav, 'decode-audio', '--threads', '1', '--stream', index, '--format', 's16', path, '-'],
                         tmp / 'ours.sw', timeout, wav=True)
    for operation, size, tool in (('oracle.reference_decode', ref_bytes, 'ffmpeg'),
                                  ('oracle.default_decode', def_bytes, 'ffmpeg'),
                                  ('oracle.coreav_decode', ours_bytes, 'coreav')):
        if size % frame:
            malformed(operation, tool)
    n_ours, n_ref, n_def = ours_bytes // frame, ref_bytes // frame, def_bytes // frame
    out = {'samples': {'ours': n_ours, 'ref': n_ref, 'default': n_def}}
    target = tmp / 'ref.sw'
    if path.suffix in ('.mp4', '.mov', '.m4a'):
        model = mp4_model(path, index, channels, stream, tmp, tools, dc, entries, execute, timeout)
        out.update(model)
        out['length_ok'] = n_ours == model['model'] and n_ref == model['ffmpeg_expected']
        out['default_length_ok'] = n_def == model['ffmpeg_expected']
        target = tmp / 'model.sw'
    else:
        out['length_ok'] = n_ours == n_ref
        out['default_length_ok'] = n_def == n_ref
    common = min(n_ours, target.stat().st_size // frame)
    if common * frame != target.stat().st_size:
        os.truncate(target, common * frame)
    test = tmp / 'ours.sw'
    if common * frame != ours_bytes:
        truncate_copy(tmp / 'ours.sw', tmp / 'ours.cut.sw', common * frame)
        test = tmp / 'ours.cut.sw'
    passed, quality = compare(execute, 'oracle.compare', tools.opus_compare, target, test, channels, tmp, timeout) if common else (False, None)
    out.update(compare_passed=passed, quality=quality)
    for name in ('ref.sw', 'model.sw', 'ours.cut.sw'):
        (tmp / name).unlink(missing_ok=True)
    shared = min(n_ours, n_def)
    if shared:
        snr, largest = stats(tmp / 'def.sw', tmp / 'ours.sw', shared, channels)
        os.truncate(tmp / 'def.sw', shared * frame)
        os.truncate(tmp / 'ours.sw', shared * frame)
        d_pass, d_quality = compare(execute, 'oracle.default_compare', tools.opus_compare, tmp / 'def.sw',
                                   tmp / 'ours.sw', channels, tmp, timeout)
        out['default'] = {'snr_db': snr if math.isfinite(snr) else 'Infinity' if snr > 0 else '-Infinity',
                          'max_steps': largest, 'compare_passed': d_pass, 'quality': d_quality}
    return bool(out['length_ok'] and passed), out
