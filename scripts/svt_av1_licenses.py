"""Check the scoped SVT-AV1 native licence and retained notice bytes.

The prefix must not be modified concurrently during this check.
"""

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import runpy
import stat
import sys


ROOT = Path(__file__).resolve().parents[1]
POLICY = ROOT / 'scripts/svt-av1-notices.json'
RECIPE = ROOT / 'scripts/encoders/svt-av1/build.py'


class LicenseError(ValueError):
    pass


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise LicenseError('duplicate native licence JSON key')
        result[key] = value
    return result


def read_json(path):
    try:
        with path.open('rb') as source:
            data = source.read(256 * 1024 + 1)
        if len(data) > 256 * 1024:
            raise LicenseError('native licence JSON exceeds its size limit')
        value = json.loads(data.decode('utf-8'), object_pairs_hook=unique_object)
    except LicenseError:
        raise
    except (OSError, ValueError, RecursionError) as error:
        raise LicenseError('missing or invalid native licence JSON') from error
    if not isinstance(value, dict):
        raise LicenseError('native licence JSON must be an object')
    return value


def regular_file(prefix, relative):
    parts = PurePosixPath(relative).parts
    if (not parts or PurePosixPath(relative).is_absolute()
            or '..' in parts or PurePosixPath(relative).as_posix() != relative):
        raise LicenseError('unsafe retained-file path')
    path = prefix
    try:
        for index, part in enumerate(parts):
            path = path / part
            mode = path.lstat().st_mode
            if index == len(parts) - 1:
                valid = stat.S_ISREG(mode)
            else:
                valid = stat.S_ISDIR(mode)
            if not valid:
                raise LicenseError('retained paths must be regular files and directories')
    except OSError as error:
        raise LicenseError('missing or unreadable retained file') from error
    return path


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def check(prefix, target):
    recipe = runpy.run_path(str(RECIPE))
    if target not in recipe['TARGETS']:
        raise LicenseError('unsupported native target')
    if not prefix.is_absolute() or prefix.is_symlink() or not prefix.is_dir():
        raise LicenseError('prefix must be an absolute, existing, nonsymlink directory')
    prefix = prefix.resolve()
    policy = read_json(POLICY)
    if (set(policy) != {'archive_sha256', 'notices'}
            or policy['archive_sha256'] != recipe['ARCHIVE_SHA256']):
        raise LicenseError('review native notice fingerprints for the recipe archive')
    notices = policy['notices']
    if (not isinstance(notices, dict) or set(notices) != set(recipe['NOTICES'])
            or any(not isinstance(value, str) or not re.fullmatch('[a-f0-9]{64}', value)
                   for value in notices.values())):
        raise LicenseError('review the complete native notice fingerprint roster')
    manifest = read_json(regular_file(prefix, 'manifest.json'))
    if (manifest.get('target') != target or manifest.get('version') != recipe['SVT_VERSION']
            or manifest.get('archive_sha256') != recipe['ARCHIVE_SHA256']):
        raise LicenseError('native manifest identity differs from the requested pinned recipe')
    regular_file(prefix, 'include/svt-av1/EbSvtAv1Enc.h')
    headers = sorted(path.relative_to(prefix).as_posix()
                     for path in (prefix / 'include/svt-av1').rglob('*.h'))
    paths = ['lib/libSvtAv1Enc.a', 'bin/svt-av1-version', *headers]
    notice_paths = {f'share/licenses/svt-av1/{name}': notices[source]
                    for source, name in recipe['NOTICES'].items()}
    paths.extend(notice_paths)
    hashes = manifest.get('sha256')
    if (not isinstance(hashes, dict) or set(hashes) != set(paths)
            or any(not isinstance(value, str) or not re.fullmatch('[a-f0-9]{64}', value)
                   for value in hashes.values())):
        raise LicenseError('native manifest retained-file roster is incomplete or invalid')
    for relative in paths:
        actual = digest(regular_file(prefix, relative))
        if relative in notice_paths and actual != notice_paths[relative]:
            raise LicenseError('native notice bytes differ from the approved upstream text')
        if actual != hashes[relative]:
            raise LicenseError('retained file differs from its native manifest hash')
    if ({path.name for path in (prefix / 'share/licenses/svt-av1').iterdir()}
            != set(recipe['NOTICES'].values())):
        raise LicenseError('native notice directory differs from the approved roster')
    return {
        'component': 'SVT-AV1', 'license': 'BSD-3-Clause-Clear', 'target': target,
        'version': recipe['SVT_VERSION'], 'archive_sha256': recipe['ARCHIVE_SHA256'],
        'notices': notice_paths,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, required=True)
    parser.add_argument('--target', required=True)
    options = parser.parse_args(argv)
    try:
        record = check(options.prefix, options.target)
    except (LicenseError, OSError) as error:
        message = str(error) if isinstance(error, LicenseError) else 'unreadable native installation'
        print(f'error: {message}', file=sys.stderr)
        return 1
    print(json.dumps(record, sort_keys=True))
    return 0


if __name__ == '__main__':
    sys.exit(main())
