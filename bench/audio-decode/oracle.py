#!/usr/bin/env python3
"""Run the maintained Opus oracle and retain only closed error receipts."""
import argparse
import importlib.util
import json
import math
import pathlib
import tempfile
import types

import receipts


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError


def failure(error):
    return {'schema': 'opus-oracle/1', 'status': 'error', 'passed': None, 'detail': None, 'error': error}


def load_reader(repo):
    spec = importlib.util.spec_from_file_location('opus_capacity_reader', repo / 'scripts/decode_audio_check.py')
    reader = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(reader)
    return reader


def judge(record, media, work, repo, tools, timeout, *, execute=receipts.invoke):
    operation, tool = 'oracle.judge', 'harness'
    try:
        import agreement
        media, work, repo = pathlib.Path(media), pathlib.Path(work), pathlib.Path(repo)
        source = (media / record['file']).resolve()
        if media.resolve() not in source.parents or not math.isfinite(timeout) or timeout <= 0:
            raise ValueError
        channels = record['channels']
        if type(channels) is not int or not 1 <= channels <= 255:
            raise ValueError
        operation, tool = 'oracle.probe', 'ffprobe'
        done = execute(operation, tool, [tools.ffprobe, '-v', 'error', '-show_entries',
                                        'stream=index,codec_name,channels,initial_padding', '-of', 'json', source], timeout)
        streams = receipts.decode_json(done.stdout)['streams']
        stream = next((s for s in streams if s.get('codec_name') == 'opus'), None)
        if (stream is None or type(stream.get('index')) is not int or stream['index'] < 0
                or int(stream['channels']) != channels):
            raise ValueError
        selected = record.get('selected_stream')
        if selected is not None and (selected.get('index') != stream['index'] or int(selected['channels']) != channels):
            raise ValueError
        operation, tool = 'oracle.reader', 'harness'
        dc = load_reader(repo)
        entries = dc.known_divergences()
        operation, tool = 'oracle.model', 'harness'
        with tempfile.TemporaryDirectory(prefix='oracle-', dir=work) as temp:
            passed, detail = agreement.judge(source, stream['index'], channels, stream, pathlib.Path(temp),
                                              tools, dc, entries, execute, timeout)
        result = {'schema': 'opus-oracle/1', 'status': 'ok', 'passed': passed, 'detail': detail, 'error': None}
        if not validate(result):
            raise ValueError
        return result
    except BaseException as error:
        return failure(receipts.exception_receipt(error, operation, tool))


def natural(value):
    return type(value) is int and 0 <= value <= receipts.COUNT_LIMIT


def quality(value):
    return value is None or (type(value) in (int, float) and math.isfinite(value))


def validate(value):
    try:
        return _validate(value)
    except (ValueError, TypeError, KeyError):
        return False


def _validate(value):
    if (not isinstance(value, dict) or set(value) != {'schema', 'status', 'passed', 'detail', 'error'}
            or value['schema'] != 'opus-oracle/1'):
        return False
    if value['status'] == 'error':
        return (value['passed'] is None and value['detail'] is None and receipts.validate(value['error'])
                and value['error']['outcome'] not in ('ok', 'disagree'))
    if value['status'] != 'ok' or type(value['passed']) is not bool or value['error'] is not None:
        return False
    detail = value['detail']
    required = {'samples', 'length_ok', 'default_length_ok', 'compare_passed', 'quality'}
    model = {'applied', 'model', 'ffmpeg_expected', 'full', 'entries_count'}
    if not isinstance(detail, dict) or not required <= set(detail) or set(detail) - required - model - {'default'}:
        return False
    samples = detail['samples']
    if (not isinstance(samples, dict) or set(samples) != {'ours', 'ref', 'default'}
            or not all(natural(n) for n in samples.values())
            or not all(type(detail[k]) is bool for k in ('length_ok', 'default_length_ok', 'compare_passed'))
            or not quality(detail['quality']) or value['passed'] != (detail['length_ok'] and detail['compare_passed'])):
        return False
    if detail['compare_passed'] and (detail['quality'] is None or samples['ours'] == 0):
        return False
    if set(detail) & model:
        if (not model <= set(detail) or type(detail['applied']) is not bool
                or not all(natural(detail[k]) for k in model - {'applied'})):
            return False
    if 'default' in detail:
        default = detail['default']
        if (not isinstance(default, dict) or set(default) != {'snr_db', 'max_steps', 'compare_passed', 'quality'}
                or not natural(default['max_steps']) or type(default['compare_passed']) is not bool
                or not quality(default['quality'])
                or not (default['snr_db'] in ('Infinity', '-Infinity') or quality(default['snr_db']))):
            return False
    return True


def read_receipt(path):
    try:
        with pathlib.Path(path).open('rb') as source:
            data = source.read(receipts.BUFFER_LIMIT + 1)
        if len(data) > receipts.BUFFER_LIMIT:
            raise ValueError
        value = receipts.decode_json(data)
        if not validate(value):
            raise ValueError
        return value
    except (OSError, ValueError, TypeError):
        return failure(receipts.receipt('oracle.receipt', 'harness', 'protocol_error'))


def main():
    parser = Parser(description=__doc__)
    for name in ('record', 'media', 'work', 'repo', 'out', 'coreav', 'ffmpeg', 'ffprobe', 'opus-compare'):
        parser.add_argument('--' + name, type=pathlib.Path, required=True)
    parser.add_argument('--timeout', type=float, required=True)
    try:
        args = parser.parse_args()
    except ValueError:
        print(json.dumps(failure(receipts.receipt('oracle.receipt', 'harness', 'protocol_error'))))
        return 2
    try:
        if args.repo.resolve() != pathlib.Path(__file__).resolve().parents[2]:
            raise ValueError
        record = receipts.decode_json(args.record.read_bytes())
        result = judge(record, args.media, args.work, args.repo, args, args.timeout)
    except BaseException as error:
        result = failure(receipts.exception_receipt(error, 'oracle.receipt', 'harness'))
    try:
        receipts.atomic_write(args.out, result)
    except OSError:
        return 1
    return 0 if result['status'] == 'ok' else 1


if __name__ == '__main__':
    raise SystemExit(main())
