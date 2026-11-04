#!/usr/bin/env python3
"""Bounded, data-only HEVC acquisition and milestone-1 census. No timing runs."""

import argparse
import collections
import contextlib
import hashlib
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
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

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


def child_limits():
    resource.setrlimit(resource.RLIMIT_AS, (6 << 30, 6 << 30))
    resource.setrlimit(resource.RLIMIT_FSIZE, (CAPTURE_CAP, CAPTURE_CAP))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    resource.setrlimit(resource.RLIMIT_CPU, (TIMEOUT, TIMEOUT))


def child(command, directory, label):
    require(sys.platform == 'linux', 'hostile subprocesses require Linux limits')
    directory.mkdir(parents=True, exist_ok=True)
    stdout, stderr = directory / f'{label}.stdout', directory / f'{label}.stderr'
    environment = {k: v for k, v in os.environ.items() if k in ('PATH', 'LANG', 'TMPDIR')}
    environment['LC_ALL'] = 'C'
    with stdout.open('wb') as out, stderr.open('wb') as err:
        process = subprocess.Popen(list(map(str, command)), stdout=out, stderr=err,
                                   env=environment, start_new_session=True, preexec_fn=child_limits)
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


def probe(binary, path, directory, frames=False):
    entries = ('frame=width,height,pix_fmt,pict_type,key_frame,pkt_pos' if frames else
               'packet=pos,size:stream=index,codec_name,profile,width,height,coded_width,coded_height,pix_fmt,field_order')
    command = [binary, '-v', 'error', '-f', 'hevc', '-select_streams', 'v:0',
               '-show_frames' if frames else '-show_packets', '-show_entries', entries,
               '-of', 'json', path]
    code, out, err = child(command, directory, 'frames' if frames else 'packets')
    require(code == 0 and not err.read_bytes().strip(), 'oracle failed or reported decode errors')
    return read_json(out)


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


def compare(case, inventory, summary, pictures, outputs, packets, frames, reference_confirmation=None):
    require(frames, 'oracle returned no output pictures')
    boundaries = packets.get('packets', [])
    require(len(boundaries) == len(pictures), 'independent access-unit count differs')
    for picture, packet in zip(pictures, boundaries):
        start, size = int(packet['pos']), int(packet['size'])
        require(start <= picture['offset'] < start + size, 'first-slice boundary outside oracle access unit')
    streams = packets.get('streams', [])
    require(len(streams) == 1 and streams[0].get('codec_name') == 'hevc', 'missing oracle stream metadata')
    require((streams[0].get('coded_width'), streams[0].get('coded_height'))
            == (pictures[0]['coded_width'], pictures[0]['coded_height']), 'independent coded geometry differs')
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
    return dict(coded_pictures=summary['pictures'], output_pictures=len(frames),
                published_counts=counts, reference_layout='whole_output_yuv_file',
                reference_count=reference_count,
                packed_output_bytes=output_bytes, reference_yuv_bytes=sorted(sizes),
                reference_packing='unverified until binary-oracle file MD5 agrees',
                native_formats=sorted(native_formats), output_order_checked_by_packet_positions=order_checked)


def confirm_reference(binary, path, directory, frames, references):
    require(len(references) == 1 and references[0].get('layout') == 'whole_output_yuv_file'
            and re.fullmatch('[0-9a-f]{32}', references[0].get('md5', '')) is not None,
            'one published whole-output MD5 is required before confirming a reference count')
    formats = {frame['pix_fmt'] for frame in frames}
    require(len(formats) == 1, 'changing output depth needs an explicitly registered file packing')
    command = [binary, '-v', 'error', '-noautorotate', '-flags', 'unaligned', '-f', 'hevc', '-c:v', 'hevc',
               *AUDIO_DECODE_INPUT, '-i', path,
               '-map', '0:v:0', '-fps_mode', 'passthrough', '-noautoscale', '-c:v', 'rawvideo',
               '-pix_fmt', '+' + next(iter(formats)), '-progress', 'pipe:2', '-f', 'md5', '-']
    code, out, err = child(command, directory, 'reference-md5')
    require(code == 0, 'reference-layout oracle failed')
    match = re.fullmatch(r'MD5=([0-9a-f]{32})\s*', out.read_text())
    require(match is not None and {r['md5'] for r in references} == {match[1]},
            'published output MD5 differs; investigate without substituting an oracle hash')
    progress, block = [], {}
    for line in err.read_text().splitlines():
        key, separator, value = line.partition('=')
        require(separator and key not in block and (key in
                {'frame', 'fps', 'bitrate', 'total_size', 'out_time_us', 'out_time_ms', 'out_time',
                 'dup_frames', 'drop_frames', 'speed', 'progress'} or re.fullmatch(r'stream_\d+_\d+_q', key)),
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
    require(int(progress[-1]['frame']) == len(frames) > 0,
            'hash-validated output count differs from complete oracle frame metadata')
    return dict(layout='whole_output_yuv_file', packing='visible planar Y,Cb,Cr; 10-bit little-endian 16-bit',
                md5=match[1], pixel_format=next(iter(formats)), complete=True,
                output_pictures=int(progress[-1]['frame']),
                oracle_build=oracle_pin.load()['BUILD'])


def reference_frames(archive_path, inventory, frames):
    yuv = [m for m in inventory['members'] if m['role'] == 'reference_yuv']
    if not yuv:
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
            result.append(dict(output_index=index, bytes=size, md5=md5.hexdigest(),
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


def census_result(records, registration_sha256, parser_sha256, oracle):
    groups = collections.defaultdict(list)
    candidates = []
    count_sources = []
    for record in records:
        for stream in record['streams']:
            groups[stream['sha256']].append(dict(test=record['test'], name=stream['name']))
            candidates.extend(dict(test=record['test'], stream_sha256=stream['sha256'], **item)
                              for item in stream.get('intra_candidates', []))
            count_sources.append(dict(test=record['test'], name=stream['name'], stream_sha256=stream['sha256'],
                                      status=stream.get('status', 'not_run'),
                                      reference_count=stream.get('reference_count'),
                                      reason=record.get('reason') if record['status'] == 'blocked' else stream.get('reason')))
    missing = [dict(gate=gate, bit_depth=depth) for gate in ('G2', 'G3') for depth in (8, 10)
               if not any(c['gate'] == gate and c['bit_depth'] == depth for c in candidates)]
    return dict(registration_sha256=registration_sha256, parser_sha256=parser_sha256, oracle=oracle,
                complete=len(records) == 147 and all(r['status'] == 'accounted' for r in records) and not missing,
                archives=records, distinct_bitstreams=len(groups),
                duplicate_bitstreams={k: v for k, v in groups.items() if len(v) > 1},
                reference_count_sources=count_sources,
                reference_count_methods=dict(collections.Counter(row['reference_count']['method']
                    if row['reference_count'] else ('expected_refusal' if row['status'] == 'refused' else 'blocked')
                    for row in count_sources)),
                intra_candidates=candidates, missing_intra_sets=missing,
                later_effective_tool_coverage='header flags identify candidates, not demonstrated pixel effects')


def run(args):
    data = registration(args.registration)
    require(all('archive_sha256' in c and 'extracted' in c for c in data['archives']),
            'run requires the SHA-bound manifest produced by the manifest command')
    require(not list(oracle_pin.verify_bin(args.oracle_bin)), 'pinned oracle binaries are required')
    oracle = {name: digest(args.oracle_bin / name) for name in ('ffmpeg', 'ffprobe')}
    oracle['pin_sha256'] = digest(ROOT / oracle_pin.PIN)
    acquisition = read_json(args.data / 'acquisition.json')
    require(acquisition['registration_identity_sha256'] == registration_identity(data)
            and acquisition['complete'], 'complete SHA-bound acquisition is required')
    require({r['archive'] for r in acquisition['archives']} == {c['archive'] for c in data['archives']},
            'acquisition omitted an archive')
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
                packets = probe(args.oracle_bin / 'ffprobe', path, output)
                frames = probe(args.oracle_bin / 'ffprobe', path, output, frames=True)['frames']
                confirmation = confirm_reference(args.oracle_bin / 'ffmpeg', path, output, frames,
                    [r for r in inventory['references'] if r['layout'] == 'whole_output_yuv_file'])
                record.update(compare(case, inventory, summary, pictures, outputs, packets, frames,
                                      confirmation))
                record['reference_packing'] = confirmation
                record['reference_frames'] = reference_frames(args.data / case['archive'], inventory, frames)
                record['intra_candidates'] = coverage(pictures, outputs, record['reference_frames'])
                record['status'] = 'headers_checked'
                record['tools'] = summary.get('tools', {})
            require(result['streams'], 'archive omitted all bitstreams')
            result['status'] = 'accounted'
        except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
            result.update(status='blocked', reason=str(error))
        records.append(result)
        result = census_result(records, digest(args.registration), digest(args.parser), oracle)
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
        require(not list(oracle_pin.verify_bin(args.oracle_bin)), 'pinned oracle binaries are required')
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
                        records[index] = corpus_record(item, path, args.out, args.oracle_bin)
                        write_json(args.out / 'corpus-census.json', corpus_result(records))
                    finally:
                        path.unlink(missing_ok=True)
            if inspect:
                require(all(r['status'] == 'inspected' for r in records), 'corpus metadata census blocked')
    finally:
        pool.close()


def corpus_record(item, path, directory, oracle_bin):
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
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        record.update(status='blocked', reason=type(error).__name__)
    return record


def corpus_result(records):
    counts = collections.Counter((r['partition'], r.get('profile'), r.get('bit_depth'), r['status']) for r in records)
    return dict(roster_sha256=digest(ROSTER),
                complete=len(records) == 54 and all(r['status'] == 'inspected' for r in records), files=records,
                counts=[dict(partition=k[0], profile=k[1], bit_depth=k[2], status=k[3], files=count)
                        for k, count in counts.items()])


def corpus(args):
    require(not list(oracle_pin.verify_bin(args.oracle_bin)), 'pinned oracle binaries are required')
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
