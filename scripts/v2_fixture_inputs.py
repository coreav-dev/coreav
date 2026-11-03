"""Registered supplemental file assignments, after the legacy V2 traversal."""
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import shutil

ROOT = Path(__file__).resolve().parents[1]
REGISTRATION = ROOT / 'tools/compat-corpus/v2-fixtures.json'
COMMANDS = ROOT / 'tools/compat-corpus/commands-0.1.0/coverage.json'
GENERATOR = ROOT / 'scripts/gen-v2-fixtures.py'
LEGACY = ('basic.mp4', 'mkv_h264_aac.mkv', 'webm_vp9_opus.webm', 'ts_h264_aac.ts', 'audio_aac.m4a')


def identity(entry):
    value = {key: entry[key] for key in ('args', 'output')}
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def registration():
    document = json.loads(REGISTRATION.read_text())
    raw = COMMANDS.read_bytes()
    if document['schema'] != 'coreav.v2-fixture-inputs/1' or hashlib.sha256(raw).hexdigest() != document['commands_sha256']:
        raise ValueError('supplemental V2 fixtures require the unchanged registered command set')
    commands = json.loads(raw)['commands']
    indices, identities = set(), set()
    for row in document['cases']:
        entry = commands[row['index']]
        key = identity(entry)
        if entry['class'] != 'media' or key != row['identity_sha256'] or row['index'] in indices or key in identities:
            raise ValueError('supplemental V2 case identity or uniqueness differs')
        indices.add(row['index'])
        identities.add(key)
        count = len(set(a for a in entry['args'] if re.fullmatch(r'INPUT[23]?', a)))
        for assignment in row['assignments']:
            if len(assignment) != count or any(Path(name).name != name for name in assignment):
                raise ValueError('supplemental V2 assignment must bind each placeholder to one filename')
    return document


DOCUMENT = registration()
ASSIGNMENTS = {row['identity_sha256']: tuple(tuple(a) for a in row['assignments']) for row in DOCUMENT['cases']}
DEPENDENCIES = DOCUMENT['dependencies']
SPARSE = frozenset(DOCUMENT['sparse'])
FILES = tuple(sorted({name for row in DOCUMENT['cases'] for a in row['assignments'] for name in a
                      if name not in LEGACY} | {name for names in DEPENDENCIES.values() for name in names}))


def assignments(entry):
    return ASSIGNMENTS.get(identity(entry), ())


def sparse_copy(source, destination):
    if not hasattr(os, 'SEEK_DATA') or not hasattr(os, 'SEEK_HOLE'):
        raise RuntimeError('sparse V2 fixture staging requires filesystem extent support')
    with source.open('rb') as src, destination.open('wb') as dst:
        size = os.fstat(src.fileno()).st_size
        dst.truncate(size)
        offset = 0
        while offset < size:
            try:
                start = os.lseek(src.fileno(), offset, os.SEEK_DATA)
                end = min(size, os.lseek(src.fileno(), start, os.SEEK_HOLE))
            except OSError as error:
                if error.errno == errno.ENXIO:
                    break
                raise RuntimeError('sparse V2 fixture staging requires filesystem extent support') from error
            if not offset <= start < end:
                raise RuntimeError('sparse V2 fixture extent did not advance')
            src.seek(start)
            dst.seek(start)
            while start < end:
                data = src.read(min(1 << 20, end - start))
                if not data:
                    raise RuntimeError('sparse V2 fixture changed during staging')
                if data.strip(b'\0'):
                    dst.write(data)
                else:
                    dst.seek(len(data), os.SEEK_CUR)
                start += len(data)
            offset = end


def stage(name, fixtures, folder):
    for item in (name, *DEPENDENCIES.get(name, ())):
        source, destination = fixtures / item, folder / item
        if item in SPARSE:
            sparse_copy(source, destination)
        else:
            shutil.copyfile(source, destination)


def provenance():
    return {path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in (REGISTRATION, GENERATOR, Path(__file__))}
