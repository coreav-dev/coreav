"""Prove complete PNS-free AAC fixtures without changing output predicates."""
import hashlib
import json
from pathlib import Path
import re
import subprocess

from github_shapes_aac import noise_count


class FixtureError(ValueError):
    pass


def sha(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def scan_fixture(source, folder, programs, env, execute):
    source, folder = Path(source).resolve(), Path(folder).resolve()
    folder.mkdir(parents=True, exist_ok=False)
    receipts = {}

    def capture(program, args, tag):
        receipt = execute([programs[program], *args], folder, env, 120, tag)
        if receipt['code'] or receipt['timed_out']:
            raise FixtureError('carriage' if tag == 'carriage' else 'scan_failed')
        normalized = dict(receipt)
        normalized['args'] = [program, *['$FIXTURE' if arg == str(source) else arg.replace(str(folder), '$SCAN')
                                       for arg in receipt['args'][1:]]]
        for stream in ('stdout', 'stderr'):
            normalized[stream + '_sha256'] = sha(folder / receipt[stream])
        receipts[tag] = normalized
        return folder / receipt['stdout']

    def probe(path, tag):
        output = capture('$FFPROBE', ['-v', 'error', '-select_streams', 'a', '-count_packets',
            '-show_streams', '-show_packets', '-show_data_hash', 'sha256', '-of', 'json', str(path)], tag)
        document = json.loads(output.read_text())
        streams, packets = document['streams'], document['packets']
        if len(streams) != 1 or not isinstance(packets, list) or not 0 < len(packets) <= 16384:
            raise FixtureError('configuration')
        stream = streams[0]
        if stream['codec_type'] != 'audio' or stream['codec_name'] != 'aac' or stream['profile'] != 'LC':
            raise FixtureError('configuration')
        if type(stream['channels']) is not int or stream['channels'] not in (1, 2):
            raise FixtureError('configuration')
        if not str(stream['sample_rate']).isdecimal() or not 0 < int(stream['sample_rate']) <= 192000:
            raise FixtureError('configuration')
        if not str(stream['nb_read_packets']).isdecimal() or int(stream['nb_read_packets']) != len(packets):
            raise FixtureError('packet_count')
        hashes = [packet['data_hash'] for packet in packets]
        config = stream['extradata_hash']
        if any(not isinstance(value, str) or not re.fullmatch(r'SHA256:[0-9a-f]{64}', value)
               for value in [config, *hashes]):
            raise FixtureError('packet_binding')
        if any(packet['stream_index'] != stream['index'] or not str(packet['size']).isdecimal()
               or not 0 < int(packet['size']) <= 16 << 20 for packet in packets):
            raise FixtureError('packet_binding')
        return {'rate': int(stream['sample_rate']), 'channels': stream['channels'],
                'configuration': config, 'packets': [(p['data_hash'], int(p['size'])) for p in packets]}

    try:
        original = probe(source, 'fixture-probe')
        scanned = source
        if source.suffix == '.mkv':
            scanned = folder / 'carriage.mp4'
            capture('$FFMPEG', ['-nostdin', '-v', 'error', '-i', str(source), '-map', '0:a:0',
                '-c:a', 'copy', '-f', 'mp4', str(scanned)], 'carriage')
            carried = probe(scanned, 'carriage-probe')
            if carried['configuration'] != original['configuration'] or any(
                    carried[key] != original[key] for key in ('rate', 'channels')):
                raise FixtureError('configuration')
            if carried['packets'] != original['packets']:
                raise FixtureError('packet_binding')
        diagnostic = folder / 'diagnostic'
        diagnostic.mkdir()
        capture('$AAC_DIAGNOSTIC', [str(scanned), str(diagnostic)], 'diagnostic')
        metadata_path = diagnostic / 'metadata.json'
        metadata = json.loads(metadata_path.read_text())
        try:
            bands = noise_count(metadata)
        except (ValueError, TypeError, KeyError):
            raise FixtureError('scan_incomplete') from None
        if bands:
            raise FixtureError('pns_present')
        if metadata['packets'] != len(original['packets']):
            raise FixtureError('packet_count')
        if any(type(metadata[key]) is not int for key in ('rate', 'channels')) or (
                metadata['rate'] != original['rate'] or metadata['channels'] != original['channels']):
            raise FixtureError('configuration')
        end = 0
        with scanned.open('rb') as media:
            length = scanned.stat().st_size
            for frame, (packet_hash, packet_size) in zip(metadata['frames'], original['packets']):
                offset, size = frame['offset'], frame['size']
                if type(offset) is not int or type(size) is not int or not (
                        end <= offset and size == packet_size and offset + size <= length):
                    raise FixtureError('packet_binding')
                media.seek(offset)
                if 'SHA256:' + hashlib.sha256(media.read(size)).hexdigest() != packet_hash:
                    raise FixtureError('packet_binding')
                end = offset + size
        result = {'fixture_sha256': sha(source), 'scanned_sha256': sha(scanned),
                  'metadata_sha256': sha(metadata_path), 'packets': metadata['packets'],
                  'packet_hashes_sha256': hashlib.sha256(json.dumps(original['packets']).encode()).hexdigest(),
                  'extradata_hash': original['configuration'], 'pns_bands': bands, 'receipts': receipts}
        (folder / 'gate.json').write_text(json.dumps(result, sort_keys=True, indent=2) + '\n')
        return result
    except FixtureError:
        raise
    except (ValueError, OSError, KeyError, TypeError, IndexError, subprocess.SubprocessError):
        raise FixtureError('scan_failed') from None
