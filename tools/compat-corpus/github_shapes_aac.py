"""Bind measured AAC PCM outputs to the registered decoder qualification."""
import json
from pathlib import Path
import re
import subprocess
import tempfile

import aac_pcm_eligibility as qualification


COMPATIBLE = 'compatible_aac'
DIAGNOSTIC_BUILD = ['cargo', 'build', '--locked', '--release', '-p', 'coreav-aac',
                    '--features', 'diagnostics', '--example', 'pns_diagnostic']
PCM = {'pcm_f32le', 'pcm_s16le'}
MAPPING = re.compile(r'^\s*Stream #0:\d+(?:\[[^]]*\])?(?: \([^)]*\))? -> #0:0 '
                     r'\(aac \(native\) -> (pcm_f32le|pcm_s16le) \(native\)\)\s*$', re.M)


def decodes_aac(reference):
    return bool(re.search(r'^\s*Stream #.* -> #\d+:\d+ \(aac \(',
                          (reference / 'command.stderr').read_text(), re.M))


def noise_count(metadata):
    frames = metadata['frames']
    if metadata['schema'] != 'coreav.aac-pns-diagnostic/1' or type(metadata['packets']) is not int or not (
            0 < metadata['packets'] == len(frames) <= 16384) or metadata['channels'] not in (1, 2):
        raise ValueError('incomplete diagnostic scan')
    count = 0
    for frame in frames:
        if len(frame['channels']) != metadata['channels']:
            raise ValueError('incomplete diagnostic channels')
        for channel in frame['channels']:
            if not isinstance(channel['noise'], list):
                raise ValueError('incomplete diagnostic noise')
            count += len(channel['noise'])
    return count


def exact_without_pns(command, base, source, programs, scratch):
    audio = [s for s in (source or {}).get('streams', []) if s.get('codec_type') == 'audio']
    if len(command.sources) != 1 or len(audio) != 1 or audio[0].get('codec_name') != 'aac':
        return False
    try:
        with tempfile.TemporaryDirectory(prefix='shape-aac-scan-', dir=scratch) as temporary:
            directory = Path(temporary)
            scan = qualification.run([programs['$AAC_DIAGNOSTIC'],
                base / 'reference' / command.sources[0][0], directory])
            if scan.returncode:
                return False
            metadata = json.loads((directory / 'metadata.json').read_text())
            return noise_count(metadata) == 0
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError):
        return False


def output_format(command, source):
    if command.tool != 'ffmpeg' or len(command.sources) != 1 or len(command.outputs) != 1 or command.stdin:
        return None
    audio = [s for s in (source or {}).get('streams', []) if s.get('codec_type') == 'audio']
    if len(audio) != 1 or audio[0].get('codec_name') != 'aac':
        return None
    prefix = list(command.args[:command.args.index('-i')])
    if prefix not in ([], ['-threads', '1']):
        return None
    options = command.output_options[0]
    values = {}
    at = 0
    while at < len(options):
        key = options[at]
        if key in values or key not in ('-vn', '-c:a', '-f', '-ar', '-ac'):
            return None
        at += 1
        if key == '-vn':
            values[key] = True
        else:
            if at == len(options):
                return None
            values[key] = options[at]
            at += 1
    for key, field in (('-ar', 'sample_rate'), ('-ac', 'channels')):
        if key in values and int(values[key]) != int(audio[0][field]):
            return None
    form = values.get('-f', 'wav' if command.outputs[0].endswith('.wav') else None)
    codec = {'f32le': 'pcm_f32le', 's16le': 'pcm_s16le', 'wav': 'pcm_s16le'}.get(form)
    if codec is None or values.get('-c:a', codec) != codec:
        return None
    return form


def compare(command, base, source, mapping, verdict, programs, scratch):
    reference = base / 'reference'
    candidate = base / 'candidate'
    audio = [s for s in (source or {}).get('streams', []) if s.get('codec_type') == 'audio']
    if len(audio) != 1 or audio[0].get('codec_name') != 'aac':
        return 'different'
    form = output_format(command, source)
    exact = verdict == ('verified', 'exact decoded output')
    if not exact and verdict != ('failed', 'decoded samples'):
        return 'different'
    try:
        with tempfile.TemporaryDirectory(prefix='shape-aac-', dir=scratch) as temporary:
            work = Path(temporary)
            diagnostic = work / 'diagnostic'
            diagnostic.mkdir()
            input_path = reference / command.sources[0][0]
            scan = qualification.run([programs['$AAC_DIAGNOSTIC'], input_path, diagnostic])
            if scan.returncode:
                return 'different'
            metadata = json.loads((diagnostic / 'metadata.json').read_text())
            noise = noise_count(metadata)
            if exact and not noise:
                return 'counted'
            if form is None or metadata['unsafe']:
                return 'different'
            codec = 'pcm_f32le' if form == 'f32le' else 'pcm_s16le'
            log = (reference / 'command.stderr').read_text()
            if mapping != {(0, 0): codec} or MAPPING.findall(log) != [codec]:
                return 'different'
            if command.outputs[0] != '-' and any((folder / 'stdout').read_bytes() for folder in (reference, candidate)):
                return 'different'
            oracle = qualification.test_oracle.require(programs['$FFMPEG'], programs['$FFPROBE'])
            pns = qualification.pns_check(oracle, input_path, diagnostic, metadata) if noise else None
            capture = {}
            row = qualification.check_output(programs['$COREAV'], oracle, input_path, work, metadata, pns, capture)
            if row['status'] != 'pass' or set(capture) != {'f32', 's16'}:
                return 'different'
            name = 'stdout' if command.outputs[0] == '-' else command.outputs[0]
            actual, expected = [(folder / name).read_bytes() for folder in (candidate, reference)]
            fmt = 'f32' if form == 'f32le' else 's16'
            ours, theirs = capture[fmt]
            if form == 'wav':
                qualification.compare_wav(actual, expected, ours, theirs, row['channels'], row['rate'])
            elif (actual, expected) != (ours, theirs):
                return 'different'
            return COMPATIBLE
    except (ValueError, OSError, KeyError, TypeError, IndexError, subprocess.SubprocessError):
        return 'different'
