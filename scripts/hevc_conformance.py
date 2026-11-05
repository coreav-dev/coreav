#!/usr/bin/env python3
"""Bounded, data-only HEVC acquisition and milestone-1 census. No timing runs."""

import argparse
import bisect
import collections
import contextlib
import hashlib
import functools
import json
import os
import pathlib
import re
import resource
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin
import test_oracle
import hevc_oracle_input
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

HEVC_DECODE_INPUT = ('-export_side_data', '+enhancements')

REGISTRATION = ROOT / 'bench/hevc-conformance/archives.json'
ROSTER = ROOT / 'docs/measurements/hevc-native/roster.json'
ARCHIVE_CAP = 16 << 20
BITSTREAM_CAP = 64 << 20
MEMBER_CAP = 8 << 30
EXPANDED_CAP = 16 << 30
TOTAL_EXPANDED_CAP = 64 << 30
TEXT_CAP = 256 << 10
CAPTURE_CAP = 64 << 20
TIMEOUT = 300
BITSTREAM_EXTENSIONS = {'.bit', '.bin', '.265', '.h265', '.hevc'}
TERMINAL_PACKETS = {
    'DELTAQP_A': ('1274e3e308dfe684b38a88f8b3a2845b84f57492b0b02aaecbe5dc29ca8ed532', 12),
    'NUT_A': ('d87dcae6353a680ff1c816395b578afae3ed9f1a88b56b07a24e62333e0621b7', 11),
}
EXCLUSION_IDENTITIES = {
    'SAODBLK_A': 'dab095100e4cea72b8e871d01bf39bb5027b460be5c495ea3374454f4aafe507',
    'SAODBLK_B': '8f3d22ccfffe230c034f1fbcc996088282cea52e2d3e22455c395a2fbb0b91ae',
}
EXCLUSION_TRIAL_IDENTITIES = {
    'SAODBLK_A': '51f758e82dbd09a765c2842bf54d7bb67482e32ab07ac2c4bcf0e148555da3ed',
    'SAODBLK_B': '32f18420053bd1e51d698b8b8266c77accf63d8482d13188bb7b31d8b2d81671',
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum=0):
    return type(value) is int and value >= minimum


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON key')
        result[key] = value
    return result


def read_json(path):
    return json.loads(pathlib.Path(path).read_text(), object_pairs_hook=unique_object)


def write_json(path, value):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.part')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def digest(path):
    result = hashlib.sha256()
    with pathlib.Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            result.update(block)
    return result.hexdigest()


def registration(path=REGISTRATION):
    data = read_json(path)
    cases = data['archives']
    require(data['version'] == 1 and len(cases) == data['archive_count'] == 147,
            'registration must account for all 147 archives')
    require(len({c['archive'].casefold() for c in cases}) == 147
            and len({c['test'] for c in cases}) == 147, 'duplicate archive or table identity')
    require(sum(c['bytes'] for c in cases) == data['compressed_bytes'], 'registered byte total differs')
    for case in cases:
        require(re.fullmatch(r'[A-Za-z0-9_]+\.zip', case['archive'])
                and integer(case['bytes'], 1) and case['bytes'] <= ARCHIVE_CAP
                and case['table_page'] in range(91, 98), 'invalid archive registration')
        if 'archive_sha256' in case:
            require(re.fullmatch('[0-9a-f]{64}', case['archive_sha256']) is not None,
                    'invalid registered archive SHA-256')
    return data


def registration_identity(data):
    identity = {key: value for key, value in data.items() if key != 'status'}
    identity['archives'] = [{key: value for key, value in case.items()
                             if key not in ('archive_sha256', 'extracted', 'references')}
                            for case in data['archives']]
    return hashlib.sha256(json.dumps(identity, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def known_exclusions():
    path = ROOT / 'bench/hevc-conformance/known-exclusions.json'
    data = read_json(path)
    require(data['version'] == 1 and set(data['exclusions']) == set(EXCLUSION_IDENTITIES),
            'only SAODBLK_A and SAODBLK_B have registered maintainer exclusions')
    for test, item in data['exclusions'].items():
        require(item['bitstream_sha256'] == EXCLUSION_IDENTITIES[test]
                and item['decision'] == 'excluded by maintainer decision, 2026-11-04'
                and all(re.fullmatch('[0-9a-f]{32}', item[k]) for k in
                        ('published_md5', 'observed_oracle_md5'))
                and item['published_md5'] != item['observed_oracle_md5']
                and re.fullmatch('[0-9a-f]{64}', item['archive_sha256']) is not None
                and item.get('investigation') and item.get('layout_registration_sha256_at_trial'),
                'maintainer exclusion identity, decision or retained mismatch evidence differs')
        history = item.get('historical_trial')
        require(isinstance(history, dict) and hashlib.sha256(json.dumps(history, sort_keys=True,
                separators=(',', ':')).encode()).hexdigest() == EXCLUSION_TRIAL_IDENTITIES[test],
                'historical exclusion trial differs from the retained normalized receipt')
        attempts = history['reference_attempts']
        require(history['kind'] == 'historical' and history['trial_head'] == item['trial_head']
                and history['original_input_sha256'] == item['bitstream_sha256'] and len(attempts) == 1,
                'historical exclusion trial input or provenance differs from exclusion metadata')
        attempt = attempts[0]
        require(attempt['candidate'] == 'C1' and attempt['layout'] == 'whole_output_yuv_file'
                and attempt['expected_md5'] == item['published_md5']
                and attempt['observed_md5'] == item['observed_oracle_md5']
                and attempt['layout_registration'] == dict(candidate='C1', document=item['layout_registration'],
                    sha256=item['layout_registration_sha256_at_trial'])
                and attempt['status'] == 'md5_mismatch' and attempt['exit_code'] == 0
                and attempt['progress_complete'] is True and attempt['count_matches_frame_metadata'] is True
                and attempt['output_pictures'] == attempt['frame_metadata_count'] == 500,
                'historical exclusion trial result differs from exclusion metadata')
    return data['exclusions']


def download(url, destination, cap, expected_size=None):
    parsed = urllib.parse.urlsplit(url)
    require(parsed.scheme == 'https' and parsed.hostname == 'www.itu.int'
            and parsed.path.startswith('/wftp3/av-arch/jctvc-site/bitstream_exchange/draft_conformance/HEVC_v1/'),
            'only official HEVC_v1 data URLs are allowed')
    destination = pathlib.Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + '.part')
    deadline = time.monotonic() + TIMEOUT
    try:
        with urllib.request.urlopen(url, timeout=60) as response, temporary.open('wb') as out:
            require(response.url == url, 'unexpected data download redirect')
            advertised = response.headers.get('Content-Length')
            if advertised is not None:
                require(int(advertised) <= cap, 'download exceeds byte cap')
            total = 0
            while block := response.read(min(1 << 20, cap - total + 1)):
                require(time.monotonic() < deadline, 'download deadline reached')
                total += len(block)
                require(total <= cap, 'download exceeds byte cap')
                out.write(block)
            require(expected_size is None or total == expected_size, 'download byte count differs')
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def listing(data, directory):
    path = directory / 'listing.html'
    download(data['listing_url'], path, 1 << 20)
    require(digest(path) == data['listing_sha256'], 'official listing changed; reconcile before acquisition')
    found = re.findall(r'(\d+) <A HREF="[^"]+\.zip">([^<]+)</A>', path.read_text())
    require({name: int(size) for size, name in found}
            == {case['archive']: case['bytes'] for case in data['archives']}, 'listing membership differs')


def member_name(value):
    require(isinstance(value, str) and value and '\\' not in value and '\x00' not in value,
            'unsafe archive member name')
    path = pathlib.PurePosixPath(value)
    require(not path.is_absolute() and '..' not in path.parts and ':' not in value,
            'unsafe archive member path')
    return path


def extracted_data(inventory):
    return [{key: member[key] for key in ('name', 'bytes', 'role', 'sha256')}
            for member in inventory['members'] if 'sha256' in member]


def md5_records(text, source):
    result = []
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    bare = len(lines) == 1 and re.fullmatch(r'[0-9a-fA-F]{32}', lines[0])
    if bare:
        name = pathlib.PurePosixPath(source).name.lower()
        target = None
        layout = 'whole_output_yuv_file' if name.endswith(('.yuv.md5', '_yuv.md5')) else 'unresolved'
        for suffix in BITSTREAM_EXTENSIONS:
            if name.endswith(suffix + '.md5'):
                target = source[:-4]
                layout = 'bitstream_file'
                break
        return [dict(source=source, target=target, md5=lines[0].lower(), layout=layout,
                     syntax='bare_digest', digest_count=1,
                     interpretation='named bitstream digest' if target else
                     ('named YUV digest attachment' if layout == 'whole_output_yuv_file' else
                      'bare digest requires a published package role before use'))]
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        match = re.fullmatch(r'([0-9a-fA-F]{32})[ \t]+\*?(.+)', line.strip())
        if match:
            value, target = match.groups()
            syntax = 'md5sum'
        else:
            match = re.fullmatch(r'MD5 \((.+)\) = ([0-9a-fA-F]{32})', line.strip())
            require(match is not None, 'unrecognized published MD5 layout')
            target, value = match.groups()
            syntax = 'bsd_md5'
        require('\x00' not in target and '\\' not in target and ':' not in target
                and '..' not in pathlib.PurePosixPath(target).parts, 'unsafe published hash target')
        suffix = pathlib.PurePosixPath(target).suffix.lower()
        layout = 'whole_output_yuv_file' if suffix == '.yuv' else (
            'bitstream_file' if suffix in BITSTREAM_EXTENSIONS else 'unresolved')
        result.append(dict(source=source, target=target, md5=value.lower(), layout=layout,
                           syntax=syntax, digest_count=1))
    require(result, 'empty published MD5 file')
    return result


def referenced_member(reference, members):
    if reference['target'] is None:
        candidates = [m for m in members if m['role'] == 'reference_yuv' and m['md5'] == reference['md5']]
        require(len(candidates) <= 1, 'ambiguous published bare output hash target')
        return candidates[0] if candidates else None
    target = pathlib.PurePosixPath(reference['target'])
    if target.is_absolute():
        return None
    relative = pathlib.PurePosixPath(reference['source']).parent / target
    candidates = [m for m in members if m['name'].casefold() in (str(target).casefold(), str(relative).casefold())]
    if not candidates and len(target.parts) == 1:
        candidates = [m for m in members if pathlib.PurePosixPath(m['name']).name.casefold() == str(target).casefold()]
    require(len(candidates) <= 1, 'ambiguous published hash target')
    return candidates[0] if candidates else None


def metadata_text(content):
    content = bytes(content)
    for bom, encoding in [(b'\xff\xfe\x00\x00', 'utf-32'), (b'\x00\x00\xfe\xff', 'utf-32'),
                          (b'\xff\xfe', 'utf-16'), (b'\xfe\xff', 'utf-16'),
                          (b'\xef\xbb\xbf', 'utf-8-sig')]:
        if content.startswith(bom):
            return content.decode(encoding), encoding, 'BOM'
    declaration = re.search(rb'(?:^|\n)[ \t]*(?:#[ \t]*)?(?:encoding|charset)[ \t]*[:=][ \t]*'
                            rb'[\"\']?([A-Za-z0-9_-]+)', content[:1024], re.I)
    if declaration:
        name = declaration[1].decode('ascii').lower().replace('_', '-')
        encodings = {'utf-8': 'utf-8', 'utf8': 'utf-8', 'us-ascii': 'ascii', 'ascii': 'ascii',
                     'utf-16le': 'utf-16-le', 'utf-16be': 'utf-16-be',
                     'iso-8859-1': 'iso-8859-1', 'latin1': 'iso-8859-1',
                     'windows-1252': 'cp1252', 'cp1252': 'cp1252'}
        require(name in encodings, 'unsupported declared metadata encoding')
        encoding = encodings[name]
        return content.decode(encoding), encoding, 'declared'
    require(b'\x00' not in content, 'binary metadata without a text encoding')
    try:
        return content.decode('utf-8'), 'utf-8', 'strict UTF-8'
    except UnicodeDecodeError:
        try:
            return content.decode('cp1252'), 'cp1252', 'fallback; no BOM or declaration'
        except UnicodeDecodeError:
            return content.decode('iso-8859-1'), 'iso-8859-1', 'fallback; no BOM or declaration'


def archive_inventory(path, directory):
    directory.mkdir(parents=True, exist_ok=True)
    members, hashes, descriptions = [], [], []
    with zipfile.ZipFile(path) as archive:
        infos = archive.infolist()
        require(len(infos) <= 512 and sum(i.file_size for i in infos) <= EXPANDED_CAP,
                'archive expansion exceeds registered limits')
        names = set()
        for info in infos:
            name = member_name(info.filename)
            require(info.filename.casefold() not in names, 'duplicate archive member')
            names.add(info.filename.casefold())
            require(not info.flag_bits & 1 and not stat.S_ISLNK(info.external_attr >> 16)
                    and info.file_size <= MEMBER_CAP
                    and info.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED),
                    'encrypted, linked, oversized or unsupported-compression archive member')
            if info.is_dir():
                continue
            suffix = name.suffix.lower()
            role = 'not_read'
            if suffix in BITSTREAM_EXTENSIONS:
                role = 'bitstream'
                require(info.file_size <= BITSTREAM_CAP, 'bitstream exceeds registered byte cap')
            elif suffix == '.yuv':
                role = 'reference_yuv'
            elif suffix == '.md5' or re.search(r'_md5\.txt$', name.name, re.I):
                role = 'reference_hash'
                require(info.file_size <= TEXT_CAP, 'hash text exceeds registered byte cap')
            elif suffix == '.txt' and not re.search(r'trace|log|decod|encod|cfg', name.name, re.I):
                if info.file_size <= TEXT_CAP:
                    role = 'description'
            if role in ('reference_hash', 'description') and name.parts[0] == '__MACOSX' and name.name.startswith('._'):
                role = 'archive_metadata'
            record = dict(name=info.filename, bytes=info.file_size, role=role)
            if role == 'not_read':
                members.append(record)
                continue
            sha, md5 = hashlib.sha256(), hashlib.md5()
            total = 0
            content = bytearray()
            output = None
            if role == 'bitstream':
                output = (directory / f'{len(members):03}{suffix}').open('wb')
            try:
                with archive.open(info) as stream:
                    while block := stream.read(1 << 20):
                        total += len(block)
                        require(total <= info.file_size, 'member exceeds declared size')
                        sha.update(block)
                        md5.update(block)
                        if output is not None:
                            output.write(block)
                        if role in ('reference_hash', 'description'):
                            content.extend(block)
            finally:
                if output is not None:
                    output.close()
            require(total == info.file_size, 'truncated archive member')
            record.update(sha256=sha.hexdigest(), md5=md5.hexdigest())
            if role == 'bitstream':
                record['path'] = f'{len(members):03}{suffix}'
            elif role == 'reference_hash':
                text, encoding, basis = metadata_text(content)
                record.update(encoding=encoding, encoding_basis=basis)
                records = md5_records(text, info.filename)
                record['digest_count'] = len(records)
                hashes.extend(records)
            elif role == 'description':
                text, encoding, basis = metadata_text(content)
                record.update(encoding=encoding, encoding_basis=basis)
                descriptions.append(dict(source=info.filename, text=text, encoding=encoding, encoding_basis=basis))
            members.append(record)
    bitstreams = [m for m in members if m['role'] == 'bitstream']
    require(bitstreams, 'archive has no recognized bitstream')
    for reference in hashes:
        if reference['syntax'] == 'bare_digest' and reference['layout'] == 'unresolved':
            source = reference['source']
            stem = source[:-8] if source.lower().endswith('_md5.txt') else source[:-4]
            matching = [m for m in bitstreams if str(pathlib.PurePosixPath(m['name']).with_suffix('')).casefold()
                        == stem.casefold()]
            if len(bitstreams) == len(matching) == 1 and descriptions:
                reference.update(layout='whole_output_yuv_file', stream_member=matching[0]['name'],
                                 interpretation='H.265.1 (10/2018) 6.5.2 decoded-picture hash attachment',
                                 role_basis='sole bitstream with matching attachment basename and short description; '
                                            'single digest, fixed whole-output packing requires exact confirmation')
        target = referenced_member(reference, members)
        if target and 'md5' in target:
            require(target['md5'] == reference['md5'], 'published hash differs from archived data')
            if reference['syntax'] == 'bare_digest' and target['role'] == 'reference_yuv':
                reference.update(layout='whole_output_yuv_file', role_basis='digest of archived published YUV bytes')
        reference['archived_bytes'] = target['bytes'] if target else None
        reference['resolved_member'] = target['name'] if target else None
    referenced = {r['resolved_member'].casefold() for r in hashes if r['resolved_member']}
    for member in members:
        if member['role'] == 'reference_yuv' and member['name'].casefold() not in referenced:
            hashes.append(dict(source=member['name'], target=member['name'], md5=member['md5'],
                               layout='whole_output_yuv_file', archived_bytes=member['bytes'],
                               derived_from_published_yuv=True))
    return dict(archive_sha256=digest(path), members=members, references=hashes,
                descriptions=descriptions)


def acquire(args):
    require(sys.platform == 'linux', 'untrusted conformance media require Linux')
    data = registration(args.registration)
    directory = args.dest
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    listing(data, directory)
    selected = set(args.archive or [c['archive'] for c in data['archives']])
    require(selected <= {c['archive'] for c in data['archives']}, 'unregistered archive selection')
    records = []
    expanded_bytes = 0
    for case in data['archives']:
        if case['archive'] not in selected:
            continue
        entry = dict(archive=case['archive'], test=case['test'])
        try:
            path = directory / case['archive']
            if not path.exists():
                download(data['listing_url'] + case['archive'], path, ARCHIVE_CAP, case['bytes'])
            require(path.stat().st_size == case['bytes'], 'cached archive size differs')
            if 'archive_sha256' in case:
                require(digest(path) == case['archive_sha256'], 'registered archive SHA-256 differs')
            with zipfile.ZipFile(path) as archive:
                expanded_bytes += sum(info.file_size for info in archive.infolist())
            require(expanded_bytes <= TOTAL_EXPANDED_CAP, 'aggregate archive expansion exceeds census cap')
            inventory = archive_inventory(path, directory / case['test'])
            if 'extracted' in case:
                require(extracted_data(inventory) == case['extracted'],
                        'registered extracted data differ')
            write_json(directory / case['test'] / 'inventory.json', inventory)
            entry.update(status='inventoried', archive_sha256=inventory['archive_sha256'],
                         inventory_sha256=digest(directory / case['test'] / 'inventory.json'))
        except (ValueError, OSError, zipfile.BadZipFile, UnicodeError) as error:
            entry.update(status='error', reason=str(error))
        records.append(entry)
        write_json(directory / 'acquisition.json', dict(registration_sha256=digest(args.registration),
                   registration_identity_sha256=registration_identity(data),
                   expanded_bytes_reserved=expanded_bytes,
                   complete=len(records) == 147 and all(r['status'] == 'inventoried' for r in records),
                   archives=records))
    require(len(records) == len(selected) and all(r['status'] == 'inventoried' for r in records),
            'acquisition failed; inspect acquisition.json')


def manifest(args):
    data = registration(args.registration)
    acquisition = read_json(args.data / 'acquisition.json')
    require(acquisition['complete'] and acquisition['registration_identity_sha256'] == registration_identity(data)
            and len(acquisition['archives']) == 147, 'complete SHA-bound acquisition is required')
    require({r['archive'] for r in acquisition['archives']} == {c['archive'] for c in data['archives']},
            'acquisition membership differs')
    for case in data['archives']:
        directory = args.data / case['test']
        receipt = next(r for r in acquisition['archives'] if r['archive'] == case['archive'])
        require(receipt['status'] == 'inventoried' and digest(directory / 'inventory.json') == receipt['inventory_sha256']
                and digest(args.data / case['archive']) == receipt['archive_sha256'], 'acquisition receipt differs')
        inventory = read_json(directory / 'inventory.json')
        require(inventory['archive_sha256'] == receipt['archive_sha256'], 'inventory archive SHA-256 differs')
        case['archive_sha256'] = receipt['archive_sha256']
        case['extracted'] = extracted_data(inventory)
        case['references'] = inventory['references']
    data['status'] = 'acquired archive and extracted data hashes pinned; header census is separate'
    write_json(args.out, data)


def child_limits(file_limit=CAPTURE_CAP):
    resource.setrlimit(resource.RLIMIT_AS, (6 << 30, 6 << 30))
    resource.setrlimit(resource.RLIMIT_FSIZE, (file_limit, file_limit))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    resource.setrlimit(resource.RLIMIT_CPU, (TIMEOUT, TIMEOUT))


def child(command, directory, label, *, file_limit=CAPTURE_CAP):
    require(sys.platform == 'linux', 'hostile subprocesses require Linux limits')
    require(integer(file_limit) and CAPTURE_CAP <= file_limit <= 4 << 30, 'invalid subprocess file limit')
    directory.mkdir(parents=True, exist_ok=True)
    stdout, stderr = directory / f'{label}.stdout', directory / f'{label}.stderr'
    environment = {k: v for k, v in os.environ.items() if k in ('PATH', 'LANG', 'TMPDIR')}
    environment['LC_ALL'] = 'C'
    with stdout.open('wb') as out, stderr.open('wb') as err:
        process = subprocess.Popen(list(map(str, command)), stdout=out, stderr=err,
                                   env=environment, start_new_session=True,
                                   preexec_fn=functools.partial(child_limits, file_limit))
        try:
            code = process.wait(timeout=TIMEOUT)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
    require(stdout.stat().st_size < CAPTURE_CAP and stderr.stat().st_size < CAPTURE_CAP,
            'subprocess capture limit reached')
    return code, stdout, stderr


def annex_b(data):
    starts = iter(re.finditer(b'\x00\x00\x01', data))
    marker = next(starts, None)
    require(marker is not None and not any(data[:marker.start()]), 'missing Annex B start code')
    result = []
    while marker is not None:
        require(len(result) < 1_000_000, 'NAL count exceeds census cap')
        start = marker.end()
        following = next(starts, None)
        end = following.start() if following else len(data)
        while end > start and data[end - 1] == 0:
            end -= 1
        nal = data[start:end]
        require(len(nal) >= 2 and not nal[0] & 0x80 and nal[1] & 7, 'invalid NAL header')
        result.append(dict(index=len(result), offset=start, size=len(nal), nal_type=(nal[0] >> 1) & 63,
                           layer_id=((nal[0] & 1) << 5) | (nal[1] >> 3), temporal_id=(nal[1] & 7) - 1))
        marker = following
    return result


def native_rows(path, data):
    rows = [json.loads(line, object_pairs_hook=unique_object) for line in path.read_text().splitlines()]
    require(rows and rows[-1].get('kind') == 'summary' and rows[-1].get('complete') is True,
            'native parser did not reach EOF')
    summary = rows[-1]
    require(all(integer(summary.get(k)) for k in ('bytes', 'nals', 'pictures', 'output_pictures')),
            'invalid native summary counts')
    require(all(r.get('kind') in ('nal', 'output') for r in rows[:-1]), 'unexpected native record')
    nals = [r for r in rows[:-1] if r['kind'] == 'nal']
    outputs = [r for r in rows[:-1] if r['kind'] == 'output']
    expected = annex_b(data)
    require(len(nals) == summary.get('nals') == len(expected) and summary.get('bytes') == len(data),
            'native NAL or byte count differs')
    pictures = []
    for mine, independent in zip(nals, expected):
        require(mine.get('kind') == 'nal' and all(type(mine.get(k)) is int and mine[k] == v
                for k, v in independent.items()), 'native NAL framing differs')
        if mine['nal_type'] < 32:
            require(type(mine.get('first_slice_segment_in_pic_flag')) is bool
                    and type(mine.get('dependent_slice_segment_flag')) is bool,
                    'missing first-slice or dependent-segment indication')
            if mine['first_slice_segment_in_pic_flag']:
                require(type(mine.get('picture_index')) is int and mine['picture_index'] == len(pictures),
                        'native picture index differs')
                require(all(integer(mine.get(k), 1) for k in
                            ('coded_width', 'coded_height', 'width', 'height'))
                        and mine.get('bit_depth') in (8, 10) and integer(mine.get('profile_idc'))
                        and mine['profile_idc'] <= 31
                        and all(type(mine.get(k)) is bool for k in ('tiles', 'wpp')), 'invalid native picture format')
                pictures.append(dict(mine, all_intra=True, filters_disabled=True, dependent_segments=False))
            require(pictures and mine.get('picture_index') == len(pictures) - 1,
                    'slice is not attached to the current picture')
            picture = pictures[-1]
            if mine.get('dependent_slice_segment_flag') is True:
                picture['dependent_segments'] = True
            else:
                require(type(mine.get('slice_type')) is int and mine['slice_type'] in (0, 1, 2)
                        and isinstance(mine.get('sao'), list) and len(mine['sao']) == 2
                        and all(type(flag) is bool for flag in mine['sao'])
                        and type(mine.get('deblocking_disabled')) is bool, 'missing effective independent-slice flags')
                picture['all_intra'] &= mine.get('slice_type') == 2
                picture['filters_disabled'] &= mine.get('sao') == [False, False] and mine.get('deblocking_disabled') is True
    require(len(pictures) == summary.get('pictures') and pictures, 'native coded-picture count differs')
    require(integer(summary.get('output_pictures')) and len(outputs) == summary['output_pictures'],
            'native output-picture count differs')
    seen = set()
    for output in outputs:
        index = output.get('picture_index')
        require(integer(index) and index < len(pictures) and index not in seen,
                'invalid or repeated native output picture')
        seen.add(index)
        require(type(output.get('poc')) is int and all(output.get(k) == pictures[index].get(k)
                for k in ('width', 'height', 'bit_depth')), 'native output format differs')
    return summary, pictures, outputs


def stderr_line(raw, stream, stage, kind='diagnostic', exit_code=None, preparation=None):
    line = raw.removesuffix('\n').removesuffix('\r')
    prefix = re.fullmatch(r'(\[hevc @ 0x[0-9a-fA-F]+\] )(.*)', line)
    result = dict(raw_line=raw, context_prefix=prefix[1] if prefix else '',
                  message=prefix[2] if prefix else line, stream=stream, stage=stage,
                  kind=kind, exit_code=exit_code, classification='unclassified')
    if preparation is not None:
        result.update(original_input_sha256=preparation['original_sha256'],
                      oracle_input_sha256=preparation['prepared_sha256'])
    return result


def stderr_lines(path):
    return path.read_bytes().decode('utf-8', errors='surrogateescape').splitlines(keepends=True)


def progress_line(item):
    key, separator, _ = item['message'].partition('=')
    return not item['context_prefix'] and bool(separator) and (key in
            {'frame', 'fps', 'bitrate', 'total_size', 'out_time_us', 'out_time_ms', 'out_time',
             'dup_frames', 'drop_frames', 'speed', 'progress'}
            or re.fullmatch(r'stream_\d+_\d+_q', key) is not None)


def probe(binary, path, directory, frames=False, *, diagnostics=None, stream=None, preparation=None):
    entries = ('frame=width,height,pix_fmt,pict_type,key_frame,pkt_pos' if frames else
               'packet=pos,size:stream=index,codec_name,profile,width,height,coded_width,coded_height,pix_fmt,field_order')
    command = [binary, '-v', 'repeat+error', '-flags', 'unaligned', *HEVC_DECODE_INPUT,
               '-f', 'hevc', '-select_streams', 'v:0',
               '-show_frames' if frames else '-show_packets', '-show_entries', entries,
               '-of', 'json', path]
    code, out, err = child(command, directory, 'frames' if frames else 'packets')
    if diagnostics is not None:
        require(isinstance(stream, str) and stream, 'diagnostic collection requires a stream identity')
        diagnostics.extend(stderr_line(raw, stream, 'frames' if frames else 'packets', exit_code=code,
                                       preparation=preparation)
                           for raw in stderr_lines(err))
    require(code == 0, 'oracle failed')
    return read_json(out)


def account_packets(case, member, data, native_path, packets, pictures):
    independent = annex_b(data)
    native = [row for row in (json.loads(line, object_pairs_hook=unique_object)
                              for line in native_path.read_text().splitlines()) if row.get('kind') == 'nal']
    require(len(native) == len(independent) and all(all(type(mine.get(k)) is int and mine[k] == v
            for k, v in nal.items()) for mine, nal in zip(native, independent)),
            'packet proof requires independently matching native NAL framing')
    require(hashlib.sha256(data).hexdigest() == member['sha256'], 'packet proof bitstream identity differs')
    boundaries = packets.get('packets', [])
    require(boundaries, 'oracle returned no access units')
    spans, end = [], 0
    for packet in boundaries:
        start, size = int(packet['pos']), int(packet['size'])
        require(start == end and size > 0 and start + size <= len(data),
                'oracle packet spans are incomplete, overlapping or outside the bitstream')
        spans.append((start, start + size))
        end = start + size
    require(end == len(data), 'oracle packet spans omit the bitstream tail')
    starts = [start for start, _ in spans]
    members = [[] for _ in spans]
    for nal in independent:
        index = bisect.bisect_right(starts, nal['offset']) - 1
        require(index >= 0 and nal['offset'] + nal['size'] <= spans[index][1],
                'NAL crosses an oracle packet boundary')
        members[index].append(nal)
    picture_packets, auxiliary = [], []
    cursor = 0
    for index, (packet, nals) in enumerate(zip(boundaries, members)):
        require(nals, 'oracle packet contains no independently framed NAL')
        first = [n for n in nals if n['nal_type'] < 32
                 and native[n['index']].get('first_slice_segment_in_pic_flag') is True]
        if first:
            require(len(first) == 1 and cursor < len(pictures)
                    and first[0]['offset'] == pictures[cursor]['offset'],
                    'oracle packet does not contain exactly one next coded picture')
            picture_packets.append(packet)
            cursor += 1
            continue
        require(case['test'] in TERMINAL_PACKETS
                and TERMINAL_PACKETS[case['test']][0] == member['sha256']
                and index == len(boundaries) - 1
                and int(packet['size']) == TERMINAL_PACKETS[case['test']][1]
                and all(n['nal_type'] in (36, 37) for n in nals),
                'non-picture oracle packet is not a registered terminal EOS/EOB packet')
        auxiliary.append(dict(packet_index=index, pos=int(packet['pos']), size=int(packet['size']),
                              nal_indices=[n['index'] for n in nals], nal_types=[n['nal_type'] for n in nals],
                              contains_slice=False, terminal=True, independent_annex_b=True,
                              native_nals_agree=True, bitstream_sha256=member['sha256']))
    require(cursor == len(pictures), 'oracle packet map omitted a coded picture')
    return dict(picture_packets=picture_packets, auxiliary_packets=auxiliary)


def classify_diagnostics(case, member, inventory, summary, comparison, confirmation, diagnostics, accounting):
    references = [r for r in inventory['references'] if r['layout'] == 'whole_output_yuv_file']
    preparation = confirmation.get('oracle_preparation')
    oracle_sha = member['sha256']
    if preparation is not None:
        require(case['test'] == hevc_oracle_input.STREAM
                and member['sha256'] == hevc_oracle_input.STREAM_SHA256
                and preparation.get('original_sha256') == member['sha256']
                and preparation.get('prepared_sha256') == hevc_oracle_input.PREPARED_SHA256
                and preparation.get('method') == 'stable_leading_parameter_sets'
                and preparation.get('document') == hevc_oracle_input.DOCUMENT
                and preparation.get('document_sha256') == digest(ROOT / hevc_oracle_input.DOCUMENT)
                and all(preparation.get(k) is True for k in
                        ('every_nal_byte_preserved', 'within_type_order_preserved', 'suffix_unchanged'))
                and preparation.get('published_output_count') == case.get('published_output_count')
                and case['published_output_count']['value'] == 6,
                'oracle preparation is not the registered one-stream byte-preserving proof')
        oracle_sha = preparation['prepared_sha256']
    require(len(references) == 1 and confirmation.get('complete') is True
            and type(confirmation.get('exit_code')) is int and confirmation['exit_code'] == 0
            and confirmation.get('progress_complete') is True
            and confirmation.get('layout') == 'whole_output_yuv_file'
            and confirmation.get('md5') == references[0].get('md5')
            and confirmation.get('bitstream_sha256') == oracle_sha
            and summary.get('complete') is True
            and confirmation.get('output_pictures') == summary.get('output_pictures')
            == comparison.get('output_pictures') == comparison['reference_count'].get('value')
            and all(integer(value, 1) for value in (confirmation.get('output_pictures'),
                summary.get('output_pictures'), comparison.get('output_pictures'),
                comparison['reference_count'].get('value'))),
            'oracle diagnostics require complete native/count/published-MD5 proof')
    proof = dict(bitstream_sha256=member['sha256'], exit_code=0, progress_complete=True,
                 oracle_input_sha256=oracle_sha,
                 native_output_pictures=summary['output_pictures'],
                 reference_count=comparison['reference_count'], layout=confirmation['layout'],
                 pixel_format=confirmation['pixel_format'], published_md5=references[0]['md5'],
                 md5_source=references[0]['source'], oracle_md5=confirmation['md5'],
                 auxiliary_packets=accounting['auxiliary_packets'])
    pending = []
    for item in diagnostics:
        require(item['stream'] == case['test'] and item['stage'] in ('packets', 'frames', 'reference-md5')
                and type(item.get('exit_code')) is int and item['exit_code'] == 0,
                'oracle diagnostic stream or stage differs')
        require(stderr_line(item['raw_line'], case['test'], item['stage'], item['kind'], item['exit_code'],
                            preparation)
                == {key: value for key, value in item.items() if key not in ('proof',)},
                'oracle diagnostic raw line differs from its parsed message')
        classification = ('validated_progress' if item['kind'] == 'progress'
                          and item['stage'] == 'reference-md5' and progress_line(item)
                          else 'complete_published_reference')
        pending.append((item, classification, proof))
    for item, classification, item_proof in pending:
        item.update(classification=classification, proof=item_proof,
                    original_input_sha256=member['sha256'], oracle_input_sha256=oracle_sha)
    return diagnostics


def published_counts(case, inventory):
    counts = []
    if 'published_output_count' in case:
        count = case['published_output_count']
        method = 'h265_1_prose_count' if count.get('source', '').startswith('H.265.1 ') else 'published_output_count'
        counts.append(dict(count, scope='output', method=method, published=True))
    for description in inventory['descriptions']:
        for match in re.finditer(r'\bcontains?\s+(\d+|one|two|three)\s+(?:pictures?|frames?)\b',
                                 description['text'], re.I):
            token = match[1].lower()
            value = int(token) if token.isdecimal() else {'one': 1, 'two': 2, 'three': 3}[token]
            counts.append(dict(value=value, source=description['source'], text=match[0],
                               scope='coded', interpretation='total described pictures, not assumed to be output count'))
    return counts


def compare_headers(summary, pictures, outputs, packets, frames, *, packet_accounting=None):
    require(frames, 'oracle returned no output pictures')
    boundaries = (packet_accounting['picture_packets'] if packet_accounting is not None
                  else packets.get('packets', []))
    require(len(boundaries) == len(pictures), 'independent access-unit count differs')
    for picture, packet in zip(pictures, boundaries):
        start, size = int(packet['pos']), int(packet['size'])
        require(start <= picture['offset'] < start + size, 'first-slice boundary outside oracle access unit')
    streams = packets.get('streams', [])
    require(len(streams) == 1 and streams[0].get('codec_name') == 'hevc', 'missing oracle stream metadata')
    require((streams[0].get('coded_width'), streams[0].get('coded_height'))
            in {(p['coded_width'], p['coded_height']) for p in pictures},
            'independent coded geometry differs')
    require(len(frames) == summary['output_pictures'] == len(outputs), 'independent output count differs')
    native_formats = {(p['width'], p['height'], p['bit_depth']) for p in pictures}
    order_checked = all(str(frame.get('pkt_pos', '')).isdecimal() for frame in frames)
    for output, frame in zip(outputs, frames):
        depth = {'yuv420p': 8, 'yuvj420p': 8, 'yuv420p10le': 10}.get(frame.get('pix_fmt'))
        require((frame.get('width'), frame.get('height'), depth)
                == (output['width'], output['height'], output['bit_depth']),
                'independent output geometry or depth differs')
        if order_checked:
            packet = boundaries[output['picture_index']]
            require(int(frame['pkt_pos']) == int(packet['pos']), 'independent output order differs')
    return dict(coded_pictures=summary['pictures'], output_pictures=len(frames),
                native_formats=sorted(native_formats), output_order_checked_by_packet_positions=order_checked,
                oracle_auxiliary_packets=packet_accounting['auxiliary_packets'] if packet_accounting else [])


def compare(case, inventory, summary, pictures, outputs, packets, frames, reference_confirmation=None,
            *, packet_accounting=None):
    headers = compare_headers(summary, pictures, outputs, packets, frames, packet_accounting=packet_accounting)
    counts = published_counts(case, inventory)
    for count in counts:
        require(count['value'] == (len(frames) if count['scope'] == 'output' else summary['pictures']),
                'published count differs from parsed pictures or oracle output')
    references = [r for r in inventory['references'] if r['layout'] == 'whole_output_yuv_file']
    require(references, 'missing published output hash; HM is not an allowed replacement')
    require(len(references) == 1, 'multiple output files need an explicit published stream mapping')
    if reference_confirmation is not None:
        require(reference_confirmation.get('complete') is True
                and reference_confirmation.get('layout') == 'whole_output_yuv_file'
                and reference_confirmation.get('md5') == references[0]['md5']
                and reference_confirmation.get('output_pictures') == len(frames),
                'published output hash confirmation is missing, incomplete or for different pictures')
    frame_sizes = [(f['width'] * f['height'] + 2 * ((f['width'] + 1) // 2) * ((f['height'] + 1) // 2))
                   * (2 if f['pix_fmt'] == 'yuv420p10le' else 1) for f in frames]
    output_bytes = sum(frame_sizes)
    sizes = {r['archived_bytes'] for r in references if r['archived_bytes'] is not None}
    if sizes:
        require(sizes == {output_bytes}, 'published YUV size differs from oracle output layout')
        if len(set(frame_sizes)) == 1:
            counts.append(dict(scope='output', value=output_bytes // frame_sizes[0],
                               source=references[0].get('resolved_member') or references[0]['target'],
                               method='archived_yuv_length', published=True,
                               interpretation='published YUV bytes / fixed packed picture bytes'))
    reference_count = next((dict(count) for method in
                            ('published_output_count', 'archived_yuv_length', 'h265_1_prose_count')
                            for count in counts if count['scope'] == 'output' and count['method'] == method), None)
    if reference_count is None:
        require(reference_confirmation is not None,
                'no independently published output count or hash-validated oracle count')
        reference_count = dict(scope='output', value=reference_confirmation['output_pictures'],
                               method='hash_validated_oracle_count', published=False,
                               source=references[0]['source'], md5=references[0]['md5'],
                               oracle_build=reference_confirmation['oracle_build'],
                               interpretation='pinned FFmpeg full output count after exact published whole-output MD5 agreement')
    require(reference_count['value'] == summary['output_pictures'], 'reference count differs from native simulator')
    return dict(headers, published_counts=counts, reference_layout='whole_output_yuv_file',
                reference_count=reference_count,
                packed_output_bytes=output_bytes, reference_yuv_bytes=sorted(sizes),
                reference_packing='unverified until binary-oracle file MD5 agrees')


def reference_progress(collected):
    progress, block = [], {}
    for item in collected:
        if item['kind'] == 'diagnostic':
            continue
        key, separator, value = item['message'].partition('=')
        require(separator and key not in block and progress_line(item),
                'reference-layout oracle reported errors or invalid progress')
        block[key] = value
        if key == 'progress':
            progress.append(block)
            block = {}
    require(progress and not block and progress[-1].get('progress') == 'end'
            and all(p.get('progress') == 'continue' for p in progress[:-1]),
            'reference-layout oracle did not confirm complete output')
    for record in progress:
        require(record.get('frame', '').isdecimal()
                and record.get('dup_frames') == record.get('drop_frames') == '0',
                'reference-layout oracle omitted frame counts or duplicated/dropped frames')
    require(all(int(a['frame']) <= int(b['frame']) for a, b in zip(progress, progress[1:])),
            'reference-layout oracle progress frame count decreased')
    return progress


def confirm_reference(binary, path, directory, frames, references, *, diagnostics=None, stream=None,
                      attempts=None, layout_registration=None, preparation=None):
    require(len(references) == 1 and references[0].get('layout') == 'whole_output_yuv_file'
            and re.fullmatch('[0-9a-f]{32}', references[0].get('md5', '')) is not None,
            'one published whole-output MD5 is required before confirming a reference count')
    formats = {frame['pix_fmt'] for frame in frames}
    require(len(formats) == 1, 'changing output depth needs an explicitly registered file packing')
    command = [binary, '-v', 'repeat+error', '-noautorotate', '-flags', 'unaligned', '-f', 'hevc', '-c:v', 'hevc',
               *HEVC_DECODE_INPUT, *AUDIO_DECODE_INPUT, '-i', path,
               '-map', '0:v:0', '-fps_mode', 'passthrough', '-noautoscale', '-c:v', 'rawvideo',
               '-pix_fmt', '+' + next(iter(formats)), '-progress', 'pipe:2', '-f', 'md5', '-']
    attempt = dict(candidate=layout_registration['candidate'] if layout_registration else 'canonical_visible_planar',
                   layout='whole_output_yuv_file', packing='visible planar Y,Cb,Cr; 10-bit little-endian 16-bit',
                   pixel_format=next(iter(formats)), command=list(map(str, command)),
                   layout_registration=layout_registration, expected_md5=references[0]['md5'], observed_md5=None,
                   exit_code=None, progress_complete=False, output_pictures=None,
                   frame_metadata_count=len(frames), count_matches_frame_metadata=False, status='incomplete')
    if attempts is not None:
        attempts.append(attempt)
    if preparation is not None:
        attempt.update(oracle_preparation=preparation,
                       original_input_sha256=preparation['original_sha256'],
                       oracle_input_sha256=preparation['prepared_sha256'])
    code, out, err = child(command, directory, 'reference-md5')
    match = re.fullmatch(r'MD5=([0-9a-f]{32})\s*', out.read_text())
    attempt.update(exit_code=code, observed_md5=match[1] if match else None,
                   stdout_capture=out.name, stderr_capture=err.name)
    if diagnostics is not None:
        require(isinstance(stream, str) and stream, 'diagnostic collection requires a stream identity')
    lines = stderr_lines(err)
    collected = []
    for raw in lines:
        item = stderr_line(raw, stream, 'reference-md5', exit_code=code, preparation=preparation)
        if progress_line(item):
            item['kind'] = 'progress'
        collected.append(item)
    if diagnostics is not None:
        diagnostics.extend(collected)
    observed_frames = [item['message'].partition('=')[2] for item in collected
                       if item['kind'] == 'progress' and item['message'].startswith('frame=')]
    if observed_frames and observed_frames[-1].isdecimal():
        attempt['output_pictures'] = int(observed_frames[-1])
    try:
        observed_progress = reference_progress(collected)
    except ValueError:
        pass
    else:
        attempt.update(progress_complete=True, progress=observed_progress,
                       count_matches_frame_metadata=int(observed_progress[-1]['frame']) == len(frames))
    if match and match[1] != references[0]['md5']:
        attempt['status'] = 'md5_mismatch'
    require(code == 0, 'reference-layout oracle failed')
    require(match is not None and {r['md5'] for r in references} == {match[1]},
            'published output MD5 differs; investigate without substituting an oracle hash')
    progress = reference_progress(collected)
    require(int(progress[-1]['frame']) == len(frames) > 0,
            'hash-validated output count differs from complete oracle frame metadata')
    result = dict(layout='whole_output_yuv_file', packing='visible planar Y,Cb,Cr; 10-bit little-endian 16-bit',
                md5=match[1], pixel_format=next(iter(formats)), complete=True,
                output_pictures=int(progress[-1]['frame']),
                oracle_build=oracle_pin.load()['BUILD'])
    if diagnostics is not None:
        result.update(exit_code=code, progress_complete=True, bitstream_sha256=digest(path))
    if preparation is not None:
        result['oracle_preparation'] = preparation
    attempt['status'] = 'confirmed'
    return result


def frame_hashes(text, frames, references=None):
    lines = text.splitlines()
    require('#hash: MD5' in lines and '#codec_id 0: rawvideo' in lines
            and '#media_type 0: video' in lines, 'oracle_frame_hash_format')
    packets = []
    for line in lines:
        if not line.strip() or line.startswith('#'):
            continue
        columns = [column.strip() for column in line.split(',')]
        require(len(columns) == 6 and columns[0] == '0'
                and all(re.fullmatch(r'-?\d+', item) for item in columns[1:4])
                and int(columns[3]) > 0 and columns[4].isdecimal()
                and re.fullmatch('[0-9a-f]{32}', columns[5]), 'oracle_frame_hash_row')
        packets.append(dict(bytes=int(columns[4]), md5=columns[5]))
    require(len(packets) == len(frames) > 0
            and (references is None or len(references) == len(frames)), 'oracle_reference_count')
    for index, (packet, frame) in enumerate(zip(packets, frames)):
        width, height = frame['width'], frame['height']
        require(integer(width, 1) and integer(height, 1)
                and frame['pix_fmt'] in ('yuv420p', 'yuvj420p', 'yuv420p10le'), 'oracle_frame_format')
        size = (width * height + 2 * ((width + 1) // 2) * ((height + 1) // 2)) * (
            2 if frame['pix_fmt'] == 'yuv420p10le' else 1)
        require(packet['bytes'] == size, 'oracle_reference_picture')
        if references is not None:
            reference = references[index]
            require(reference['output_index'] == index and packet['bytes'] == reference['bytes']
                    and packet['md5'] == reference['md5'], 'oracle_reference_picture')
    return packets


def oracle_frames(binary, path, directory, frames, stream, references=None, *, first_picture=False):
    formats = {frame['pix_fmt'] for frame in frames}
    require(len(formats) == 1, 'oracle_changing_packing')
    command = [binary, '-v', 'repeat+error', '-noautorotate', '-flags', 'unaligned',
               *([] if first_picture else ['-f', 'hevc']), '-c:v', 'hevc',
               *HEVC_DECODE_INPUT, *AUDIO_DECODE_INPUT, '-i', path,
               '-map', '0:v:0', '-fps_mode', 'passthrough', '-noautoscale', '-c:v', 'rawvideo',
               '-pix_fmt', '+' + next(iter(formats)), *(['-frames:v', '1'] if first_picture else []),
               '-progress', 'pipe:2', '-f', 'framemd5', '-']
    label = 'source-first-frame' if first_picture else 'oracle-frames'
    code, stdout, stderr = child(command, directory, label)
    require(code == 0, 'oracle_failed')
    collected = []
    for raw in stderr_lines(stderr):
        item = stderr_line(raw, stream, 'intra-frames', exit_code=code)
        if progress_line(item):
            item['kind'] = 'progress'
        collected.append(item)
    progress = reference_progress(collected)
    require(int(progress[-1]['frame']) == len(frames), 'oracle_incomplete_output')
    result = frame_hashes(stdout.read_text(), frames, references)
    write_json(directory / (label + '.json'), dict(complete=True, command=list(map(str, command)),
               frames=result, progress=progress, diagnostic_lines=sum(
                   row['kind'] == 'diagnostic' for row in collected),
               diagnostic_policy='exit_completeness_and_published_reference_only'))
    return result


def anchored_reference_frames(binary, path, directory, frames, outputs, confirmation,
                              inventory, stream, stream_sha256):
    references = [r for r in inventory['references'] if r['layout'] == 'whole_output_yuv_file']
    require(len(references) == 1 and confirmation.get('complete') is True
            and confirmation.get('exit_code') == 0 and confirmation.get('progress_complete') is True
            and confirmation.get('output_pictures') == len(frames) == len(outputs) > 0
            and re.fullmatch('[0-9a-f]{32}', references[0].get('md5', '')) is not None
            and confirmation.get('md5') == references[0]['md5']
            and confirmation.get('bitstream_sha256') == digest(path), 'picture_reference_anchor')
    source = next((m for m in inventory['members'] if m['name'] == references[0]['source']), None)
    require(source is not None and source['role'] == 'reference_hash', 'picture_reference_source')
    packets = oracle_frames(binary, path, directory, frames, stream)
    binary_sha256 = digest(binary)
    return [dict(packet, output_index=index, picture_index=output['picture_index'],
                 stream=stream, stream_sha256=stream_sha256, anchoring_md5=confirmation['md5'],
                 source=source['name'], source_sha256=source['sha256'],
                 source_scope='oracle_picture_anchored_to_published_whole_output',
                 oracle_build=confirmation['oracle_build'], oracle_sha256=binary_sha256,
                 oracle_input_sha256=confirmation['bitstream_sha256'])
            for index, (packet, output) in enumerate(zip(packets, outputs))]


def reference_frames(archive_path, inventory, frames, confirmation=None):
    yuv = [m for m in inventory['members'] if m['role'] == 'reference_yuv']
    if not yuv:
        if len(frames) == 1:
            references = [r for r in inventory['references'] if r['layout'] == 'whole_output_yuv_file']
            require(len(references) == 1 and confirmation is not None
                    and confirmation.get('complete') is True
                    and confirmation.get('output_pictures') == 1
                    and confirmation.get('md5') == references[0]['md5'],
                    'single-picture reference needs complete published whole-output hash confirmation')
            reference = references[0]
            source = next((m for m in inventory['members'] if m['name'] == reference['source']), None)
            require(source is not None and source['role'] == 'reference_hash', 'missing published hash attachment')
            frame = frames[0]
            size = (frame['width'] * frame['height']
                    + 2 * ((frame['width'] + 1) // 2) * ((frame['height'] + 1) // 2)) * (
                        2 if frame['pix_fmt'] == 'yuv420p10le' else 1)
            return [dict(output_index=0, bytes=size, md5=reference['md5'], source=source['name'],
                         source_sha256=source['sha256'], source_scope='published_whole_output_single_picture')]
        return []
    require(len(yuv) == 1, 'multiple published YUV files need an explicit stream mapping')
    result = []
    with zipfile.ZipFile(archive_path) as archive, archive.open(yuv[0]['name']) as stream:
        whole = hashlib.sha256()
        for index, frame in enumerate(frames):
            width, height = frame['width'], frame['height']
            size = (width * height + 2 * ((width + 1) // 2) * ((height + 1) // 2)) * (
                2 if frame['pix_fmt'] == 'yuv420p10le' else 1)
            remaining, md5 = size, hashlib.md5()
            while remaining:
                block = stream.read(min(1 << 20, remaining))
                require(block, 'published YUV ends before the last output picture')
                remaining -= len(block)
                md5.update(block)
                whole.update(block)
            result.append(dict(output_index=index, bytes=size, md5=md5.hexdigest(), source_scope='published_yuv_picture',
                               source=yuv[0]['name'], source_sha256=yuv[0]['sha256']))
        require(not stream.read(1) and whole.hexdigest() == yuv[0]['sha256'],
                'published YUV has unexplained trailing data or a changed hash')
    return result


def coverage(pictures, outputs, references):
    result = []
    for output, reference in zip(outputs, references):
        picture = pictures[output['picture_index']]
        if not picture['all_intra']:
            continue
        result.append(dict(picture_index=output['picture_index'], poc=output['poc'],
                           bit_depth=picture['bit_depth'], gate='G2' if picture['filters_disabled'] else 'G3',
                           tiles=picture.get('tiles'), wpp=picture.get('wpp'),
                           dependent_segments=picture['dependent_segments'], reference=reference))
    return result


def census_result(records, registration_sha256, parser_sha256, oracle, synthetic=None):
    groups = collections.defaultdict(list)
    candidates = []
    count_sources = []
    for record in records:
        for stream in record['streams']:
            groups[stream['sha256']].append(dict(test=record['test'], name=stream['name']))
            candidates.extend(dict(test=record['test'], stream_sha256=stream['sha256'],
                                   source_kind='published_conformance', **item)
                              for item in stream.get('intra_candidates', []))
            count_sources.append(dict(test=record['test'], name=stream['name'], stream_sha256=stream['sha256'],
                                      source_kind='published_conformance',
                                      status=stream.get('status', 'not_run'),
                                      reference_count=stream.get('reference_count'),
                                      exclusion=stream.get('exclusion'),
                                      reason=record.get('reason') if record['status'] == 'blocked' else stream.get('reason')))
    synthetic_streams = synthetic.get('streams', []) if synthetic else []
    for stream in synthetic_streams:
        count_sources.append(dict(test=stream['test'], name=stream['test'], stream_sha256=stream['sha256'],
                                  source_kind='synthetic', status=stream.get('status', 'not_run'),
                                  reference_count=stream.get('reference_count'), reason=stream.get('reason')))
        if synthetic.get('complete') is True:
            candidates.extend(dict(test=stream['test'], stream_sha256=stream['sha256'],
                                   source_kind='synthetic', **item)
                              for item in stream.get('intra_candidates', []))
    missing = [dict(gate=gate, bit_depth=depth) for gate in ('G2', 'G3') for depth in (8, 10)
               if not any(c['gate'] == gate and c['bit_depth'] == depth for c in candidates)]
    return dict(registration_sha256=registration_sha256, parser_sha256=parser_sha256, oracle=oracle,
                complete=len(records) == 147 and all(r['status'] == 'accounted' for r in records) and not missing
                         and (synthetic is None or synthetic.get('complete') is True),
                archives=records, distinct_bitstreams=len(groups),
                synthetic_census=synthetic, synthetic_bitstreams=len(synthetic_streams),
                duplicate_bitstreams={k: v for k, v in groups.items() if len(v) > 1},
                reference_count_sources=count_sources,
                reference_count_methods=dict(collections.Counter(row['reference_count']['method']
                    if row['reference_count'] else ('expected_refusal' if row['status'] == 'refused'
                        else 'maintainer_exclusion' if row['status'] == 'excluded' else 'blocked')
                    for row in count_sources)),
                intra_candidates=candidates, missing_intra_sets=missing,
                later_effective_tool_coverage='header flags identify candidates, not demonstrated pixel effects')


def run(args):
    data = registration(args.registration)
    exclusions = known_exclusions() if any(c['test'] in EXCLUSION_IDENTITIES for c in data['archives']) else {}
    exclusions_sha = digest(ROOT / 'bench/hevc-conformance/known-exclusions.json') if exclusions else None
    require(all('archive_sha256' in c and 'extracted' in c for c in data['archives']),
            'run requires the SHA-bound manifest produced by the manifest command')
    require(not list(test_oracle.verify_bin(args.oracle_bin)), 'pinned oracle binaries are required')
    oracle = {name: digest(args.oracle_bin / name) for name in ('ffmpeg', 'ffprobe')}
    oracle['pin_sha256'] = digest(ROOT / oracle_pin.PIN)
    acquisition = read_json(args.data / 'acquisition.json')
    require(acquisition['registration_identity_sha256'] == registration_identity(data)
            and acquisition['complete'], 'complete SHA-bound acquisition is required')
    require({r['archive'] for r in acquisition['archives']} == {c['archive'] for c in data['archives']},
            'acquisition omitted an archive')
    synthetic = None
    if getattr(args, 'synthetic_data', None) is not None:
        import hevc_synthetic
        synthetic = hevc_synthetic.inspect(args.synthetic_data, args.out / 'synthetic', args.parser,
                                           args.oracle_bin, sys.modules[__name__])
    records = []
    for case in data['archives']:
        result = dict(archive=case['archive'], test=case['test'], streams=[])
        try:
            directory = args.data / case['test']
            inventory = read_json(directory / 'inventory.json')
            receipt = next(r for r in acquisition['archives'] if r['archive'] == case['archive'])
            require(digest(directory / 'inventory.json') == receipt['inventory_sha256'],
                    'inventory SHA-256 differs from acquisition')
            require(digest(args.data / case['archive']) == inventory['archive_sha256']
                    == receipt['archive_sha256'], 'archive SHA-256 differs from acquisition')
            if 'archive_sha256' in case:
                require(inventory['archive_sha256'] == case['archive_sha256'], 'registered archive SHA-256 differs')
            require(extracted_data(inventory) == case['extracted'], 'registered extracted data differ')
            bitstreams = [m for m in inventory['members'] if m['role'] == 'bitstream']
            result['streams'] = [dict(name=m['name'], sha256=m['sha256'], status='not_run') for m in bitstreams]
            for member, record in zip(bitstreams, result['streams']):
                path = directory / member['path']
                require(digest(path) == member['sha256'], 'extracted bitstream SHA-256 differs')
                output = args.out / case['test'] / member['sha256']
                code, native, _ = child([args.parser, path], output, 'native')
                if 'expected_refusal' in case:
                    rows = [json.loads(line, object_pairs_hook=unique_object)
                            for line in native.read_text().splitlines()]
                    require(code != 0 and rows and rows[-1].get('kind') == 'refusal'
                            and rows[-1].get('category') == 'unsupported'
                            and rows[-1].get('reason') == case['expected_refusal'], 'named refusal differs')
                    record.update(status='refused', reason=case['expected_refusal'])
                    continue
                require(code == 0, 'native parser failed')
                summary, pictures, outputs = native_rows(native, path.read_bytes())
                if case['test'] in exclusions:
                    exclusion = exclusions[case['test']]
                    references = [r for r in inventory['references'] if r['layout'] == 'whole_output_yuv_file']
                    require(member['sha256'] == exclusion['bitstream_sha256']
                            and inventory['archive_sha256'] == exclusion['archive_sha256']
                            and len(references) == 1 and references[0]['md5'] == exclusion['published_md5'],
                            'maintainer exclusion does not match the pinned archive, stream and reference')
                    record.update(status='excluded', reason=exclusion['decision'], exclusion=exclusion,
                                  native_headers_complete=True, native_summary=summary)
                    continue
                oracle_path, preparation = path, None
                if case['test'] == 'VPSSPSPPS_A':
                    oracle_path, preparation = hevc_oracle_input.prepare(case, path, output,
                                                                         sys.modules[__name__])
                    record['oracle_preparation'] = preparation
                diagnostics = record['oracle_stderr'] = []
                packets = probe(args.oracle_bin / 'ffprobe', oracle_path, output,
                                diagnostics=diagnostics, stream=case['test'], preparation=preparation)
                frames = probe(args.oracle_bin / 'ffprobe', oracle_path, output, frames=True,
                               diagnostics=diagnostics, stream=case['test'], preparation=preparation)['frames']
                attempts = record['reference_attempts'] = []
                confirmation = confirm_reference(args.oracle_bin / 'ffmpeg', oracle_path, output, frames,
                    [r for r in inventory['references'] if r['layout'] == 'whole_output_yuv_file'],
                    diagnostics=diagnostics, stream=case['test'], attempts=attempts,
                    preparation=preparation)
                record['reference_packing'] = confirmation
                accounting = account_packets(case, member, path.read_bytes(), native, packets, pictures)
                record['oracle_auxiliary_packets'] = accounting['auxiliary_packets']
                comparison = compare(case, inventory, summary, pictures, outputs, packets, frames,
                                     confirmation, packet_accounting=accounting)
                classify_diagnostics(case, member, inventory, summary, comparison, confirmation, diagnostics, accounting)
                record.update(comparison)
                record['reference_packing'] = confirmation
                record['reference_frames'] = reference_frames(args.data / case['archive'], inventory, frames, confirmation)
                if not record['reference_frames'] and any(
                        picture['all_intra'] for picture in pictures):
                    record['reference_frames'] = anchored_reference_frames(
                        args.oracle_bin / 'ffmpeg', oracle_path, output, frames, outputs,
                        confirmation, inventory, case['test'], member['sha256'])
                record['intra_candidates'] = coverage(pictures, outputs, record['reference_frames'])
                record['status'] = 'headers_checked'
                record['tools'] = summary.get('tools', {})
            require(result['streams'], 'archive omitted all bitstreams')
            result['status'] = 'accounted'
        except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
            result.update(status='blocked', reason=str(error))
        records.append(result)
        result = census_result(records, digest(args.registration), digest(args.parser), oracle, synthetic)
        result['known_exclusions_sha256'] = exclusions_sha
        write_json(args.out / 'census.json', result)
    require(result['complete'], 'G1 blocked; inspect census.json')


def roster(path=ROSTER):
    data = read_json(path)
    files = data['files']
    require(data['counts'] == dict(files=54, development=26, held_out_partition=28, b5=8)
            and len(files) == len({f['sha256'] for f in files}) == 54, 'roster membership differs')
    require(collections.Counter(f['partition'] for f in files)
            == {'development': 26, 'held-out-partition': 28}, 'roster partitions differ')
    require(sum('b5' in f['sets'] for f in files) == 8, 'B5 roster differs')
    for source in data['sources']:
        require(digest(ROOT / source['path']) == source['sha256'], 'registered roster source changed')
    for item in files:
        require(re.fullmatch('[0-9a-f]{64}', item['sha256']) and integer(item['size'], 1)
                and item['ext'] in ('mp4', 'mkv', 'mov'), 'invalid registered corpus file')
    return files


def fetch_corpus(args, inspect=False):
    require(sys.platform == 'linux', 'hostile corpus files require Linux')
    sys.path.insert(0, str(ROOT / 'tools/compat-corpus'))
    import round_files
    files = roster()
    if inspect:
        require(not list(test_oracle.verify_bin(args.oracle_bin)), 'pinned oracle binaries are required')
    pool = round_files.pool()
    from r2pool import Limits
    pool.limits = Limits(requests=4000, bytes_=32 << 30)
    args.dest.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        required_space = (max(f['size'] for f in files) if inspect else sum(f['size'] for f in files)) + (128 << 20)
        require(shutil.disk_usage(args.dest).free >= required_space, 'insufficient acquisition disk space')
        entries = {}
        for corpus_name in ('v1', 'v1-ts-audio', 'v2-heldout'):
            if args.manifests:
                path = args.manifests / 'corpus' / corpus_name / 'manifest.json'
                require(path.stat().st_size <= 32 << 20, 'manifest exceeds byte cap')
                manifest = read_json(path)
            else:
                key = f'corpus/{corpus_name}/manifest.json'
                size, etag = pool.head(key)
                require(size <= 32 << 20, 'manifest exceeds byte cap')
                manifest = json.loads(pool.get(key, size, etag), object_pairs_hook=unique_object)
            entries_list = manifest['files'] if isinstance(manifest, dict) else manifest
            for entry in entries_list:
                sha = entry.get('sha256')
                if sha not in {f['sha256'] for f in files}:
                    continue
                registered = next(f for f in files if f['sha256'] == sha)
                require(entry.get('size') == registered['size'] and entry.get('ext') == registered['ext'],
                        'corpus metadata differs from registered SHA-bound roster')
                key = entry['key']
                require(isinstance(key, str) and key and '..' not in key.split('/') and '\\' not in key,
                        'unsafe corpus object key')
                if not key.startswith('corpus/'):
                    key = f'corpus/{corpus_name}/{key}'
                entries.setdefault(sha, dict(registered, key=key))
        require(set(entries) == {f['sha256'] for f in files}, 'corpus manifests omit registered files')
        context = tempfile.TemporaryDirectory(prefix='hevc-', dir=args.dest) if inspect else contextlib.nullcontext(args.dest)
        with context as destination:
            destination = pathlib.Path(destination)
            records = [dict(item, status='not_run') for item in files]
            if inspect:
                write_json(args.out / 'corpus-census.json', corpus_result(records))
            for index, item in enumerate(files):
                if inspect:
                    require(shutil.disk_usage(destination).free >= item['size'] + (128 << 20),
                            'insufficient per-file acquisition disk space')
                path = round_files.fetch_one(pool, entries[item['sha256']], destination)
                if inspect:
                    try:
                        options = dict(native=args.native, parser=args.parser,
                                       max_memory_bytes=args.max_memory_bytes, max_bins=args.max_bins) if getattr(args, 'native', None) else None
                        records[index] = corpus_record(item, path, args.out, args.oracle_bin, intra=options)
                        write_json(args.out / 'corpus-census.json', corpus_result(records))
                    finally:
                        path.unlink(missing_ok=True)
            if inspect:
                require(all(r['status'] == 'inspected' for r in records), 'corpus metadata census blocked')
    finally:
        pool.close()


def corpus_record(item, path, directory, oracle_bin, *, intra=None):
    record = dict(item)
    try:
        require(path.stat().st_size == item['size'] and digest(path) == item['sha256'],
                'corpus size or SHA-256 differs')
        command = [oracle_bin / 'ffprobe', '-v', 'error', '-show_format', '-show_streams', '-of', 'json', path]
        code, out, err = child(command, directory / item['sha256'], 'metadata')
        require(code == 0 and not err.read_bytes().strip(), 'corpus metadata oracle failed')
        meta = read_json(out)
        videos = [s for s in meta.get('streams', []) if s.get('codec_type') == 'video']
        require(videos and videos[0].get('codec_name') == 'hevc', 'selected video is not HEVC')
        stream = videos[0]
        record.update(status='inspected', container=meta.get('format', {}).get('format_name'),
                      profile=stream.get('profile'), width=stream.get('width'),
                      height=stream.get('height'), pixel_format=stream.get('pix_fmt'),
                      bit_depth={'yuv420p': 8, 'yuvj420p': 8, 'yuv420p10le': 10}.get(stream.get('pix_fmt')),
                      field_order=stream.get('field_order'))
        require(record['container'] and record['profile'] and record['pixel_format'],
                'incomplete profile/container census')
        record['admission'] = 'requires native parameter-set validation'
        if intra and item['partition'] == 'development':
            import hevc_intra
            record['intra'] = hevc_intra.development_picture(item, path, directory / item['sha256'], oracle_bin, **intra)
            record['admission'] = 'first independent IRAP validated; full decode not established'
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        record.update(status='blocked', reason=type(error).__name__)
        report = directory / item['sha256'] / 'intra.json'
        if intra and item['partition'] == 'development' and report.exists():
            record['intra'] = read_json(report)
    return record


def corpus_result(records):
    counts = collections.Counter((r['partition'], r.get('profile'), r.get('bit_depth'), r['status']) for r in records)
    result = dict(roster_sha256=digest(ROSTER),
                complete=len(records) == 54 and all(r['status'] == 'inspected' for r in records), files=records,
                counts=[dict(partition=k[0], profile=k[1], bit_depth=k[2], status=k[3], files=count)
                        for k, count in counts.items()])
    selected = [r for r in records if 'intra' in r]
    if selected:
        result['development_intra'] = dict(expected=26, completed=sum(r['intra'].get('complete') is True for r in selected))
        stages = ('extract_first_picture', 'headers', 'oracle', 'reconstruction', 'comparison', 'complete')
        result['development_intra']['stages'] = {stage: sum(r['intra'].get('stage') == stage for r in selected) for stage in stages}
        refusals = collections.Counter()
        for record in selected:
            refusal = record['intra'].get('native_refusal', {})
            reason = refusal.get('reason')
            if (refusal.get('kind') == 'refusal' and refusal.get('category') in ('invalid_data', 'limit', 'unsupported')
                    and isinstance(reason, str) and re.fullmatch('[a-z0-9_]{1,80}', reason)):
                refusals[reason] += 1
        result['development_intra']['native_refusals'] = dict(refusals)
        result['complete'] &= len(selected) == 26 and result['development_intra']['completed'] == 26
    return result


def corpus(args):
    require(not list(test_oracle.verify_bin(args.oracle_bin)), 'pinned oracle binaries are required')
    files = roster()
    records = [dict(item, status='not_run') for item in files]
    for index, item in enumerate(files):
        path = args.data / f"{item['sha256']}.{item['ext']}"
        records[index] = corpus_record(item, path, args.out, args.oracle_bin)
        write_json(args.out / 'corpus-census.json', corpus_result(records))
    require(all(r['status'] == 'inspected' for r in records), 'corpus metadata census blocked')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest='mode', required=True)
    acquire_parser = modes.add_parser('acquire')
    acquire_parser.add_argument('--dest', required=True, type=pathlib.Path)
    acquire_parser.add_argument('--archive', action='append')
    acquire_parser.add_argument('--registration', type=pathlib.Path, default=REGISTRATION)
    manifest_parser = modes.add_parser('manifest')
    manifest_parser.add_argument('--data', required=True, type=pathlib.Path)
    manifest_parser.add_argument('--out', required=True, type=pathlib.Path)
    manifest_parser.add_argument('--registration', type=pathlib.Path, default=REGISTRATION)
    run_parser = modes.add_parser('run')
    run_parser.add_argument('--registration', type=pathlib.Path, default=REGISTRATION)
    run_parser.add_argument('--parser', required=True, type=pathlib.Path)
    run_parser.add_argument('--synthetic-data', type=pathlib.Path)
    for command in (run_parser, modes.add_parser('corpus')):
        command.add_argument('--data', required=True, type=pathlib.Path)
        command.add_argument('--out', required=True, type=pathlib.Path)
        command.add_argument('--oracle-bin', required=True, type=pathlib.Path)
    fetch_parser = modes.add_parser('fetch-corpus')
    fetch_parser.add_argument('--dest', required=True, type=pathlib.Path)
    fetch_parser.add_argument('--manifests', type=pathlib.Path)
    online_parser = modes.add_parser('corpus-online')
    online_parser.add_argument('--dest', required=True, type=pathlib.Path)
    online_parser.add_argument('--out', required=True, type=pathlib.Path)
    online_parser.add_argument('--oracle-bin', required=True, type=pathlib.Path)
    online_parser.add_argument('--manifests', type=pathlib.Path)
    online_parser.add_argument('--native', type=pathlib.Path)
    online_parser.add_argument('--parser', type=pathlib.Path)
    online_parser.add_argument('--max-memory-bytes', type=int, default=256 << 20)
    online_parser.add_argument('--max-bins', type=int, default=1 << 28)
    args = parser.parse_args()
    try:
        {'acquire': acquire, 'manifest': manifest, 'run': run,
         'fetch-corpus': fetch_corpus, 'corpus': corpus,
         'corpus-online': lambda options: fetch_corpus(options, inspect=True)}[args.mode](args)
    except (ValueError, OSError, KeyError, TypeError, zipfile.BadZipFile) as error:
        print(f'hevc_conformance.py: {type(error).__name__}: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
