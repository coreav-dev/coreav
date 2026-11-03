#!/usr/bin/env python3
"""Build the standalone reconstruction decoder from the distribution's archive pin."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import sys
import tarfile
import tempfile


ROOT = Path(__file__).resolve().parents[1]
PIN = ROOT / 'packages/cli/scripts/codecs.sh'
SPEC = importlib.util.spec_from_file_location('dav1d_oracle_process',
                                            ROOT / 'scripts/encoders/svt-av1/build.py')
PROCESS = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PROCESS
SPEC.loader.exec_module(PROCESS)


def digest(path):
    result = hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda: source.read(1 << 20), b''):
            result.update(block)
    return result.hexdigest()


def pin():
    values = re.findall(r'^(DAV1D_(?:VERSION|SHA256))=(\S+)$', PIN.read_text(), re.MULTILINE)
    if (len(values) != 2 or len(dict(values)) != 2
            or re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', dict(values).get('DAV1D_VERSION', '')) is None
            or re.fullmatch(r'[0-9a-f]{64}', dict(values).get('DAV1D_SHA256', '')) is None):
        raise ValueError('expected one dav1d version and archive digest in the distribution pin')
    return dict(values)


def extract(archive, destination, expected):
    if digest(archive) != expected['DAV1D_SHA256']:
        raise ValueError('dav1d archive differs from the distribution pin')
    root = 'dav1d-' + expected['DAV1D_VERSION']
    with tarfile.open(archive, 'r:xz') as source:
        members = source.getmembers()
        seen = set()
        total = 0
        for member in members:
            path = PurePosixPath(member.name)
            total += member.size
            if (not path.parts or path.parts[0] != root or path.is_absolute()
                    or '..' in path.parts or path in seen
                    or member.name.rstrip('/') != path.as_posix()
                    or not (member.type == tarfile.REGTYPE or member.isdir())
                    or member.size < 0 or total > (256 << 20)):
                raise ValueError('dav1d archive has an unsafe or excessive entry')
            seen.add(path)
        for member in members:
            output = destination.joinpath(*PurePosixPath(member.name).parts)
            if member.isdir():
                output.mkdir(parents=True, exist_ok=True)
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as content, output.open('xb') as target:
                    shutil.copyfileobj(content, target)
                output.chmod(member.mode & 0o777)
    return destination / root


def command_environment():
    environment = {key: value for key, value in os.environ.items()
                   if key in ('PATH', 'HOME', 'TMPDIR', 'SSL_CERT_FILE', 'SSL_CERT_DIR',
                              'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
                              'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    environment.update(CC='cc', CFLAGS='', CPPFLAGS='', LDFLAGS='', LANG='C.UTF-8', LC_ALL='C.UTF-8')
    return environment


def identity(binary, version, runner=PROCESS.run):
    binary = binary.resolve(strict=True)
    before = digest(binary)
    record = {}
    command = [str(binary), '--version']
    runner(command, command_environment(), timeout=30, record=record)
    text = (record.get('stdout', '') + record.get('stderr', '')).strip()
    if (record.get('exit_code') != 0 or record.get('timed_out') is not False
            or text != version or digest(binary) != before):
        raise ValueError('standalone dav1d identity differs from the pinned version')
    return {'path': str(binary), 'sha256': before, 'version': text,
            'command': command, 'process': record}


def build(prefix, archive=None, jobs=2, runner=PROCESS.run):
    if (platform.system(), platform.machine()) != ('Linux', 'x86_64'):
        raise ValueError('reconstruction oracle acquisition requires Linux x86-64')
    if jobs not in (1, 2):
        raise ValueError('decoder build jobs must be one or two')
    prefix = Path(prefix).absolute()
    if os.path.lexists(prefix) or not prefix.parent.is_dir():
        raise ValueError('decoder prefix must be newly absent beneath an existing directory')
    expected = pin()
    inputs = {'pin_sha256': digest(PIN), 'recipe_sha256': digest(__file__),
              'process_recipe_sha256': digest(PROCESS.__file__), **expected}
    environment = command_environment()
    commands = []

    def run(command, timeout):
        record = {'command': [str(part) for part in command]}
        commands.append(record)
        try:
            runner(record['command'], environment, timeout=timeout, record=record)
        except PROCESS.BuildError:
            raise ValueError('opaque decoder build command failed') from None
        if record.get('exit_code') != 0 or record.get('timed_out') is not False:
            raise ValueError('opaque decoder build command did not complete successfully')
        return record['stdout']

    with tempfile.TemporaryDirectory(prefix='.dav1d-oracle-', dir=prefix.parent) as temporary:
        work = Path(temporary)
        if archive is None:
            archive = work / 'source.tar.xz'
            version = expected['DAV1D_VERSION']
            run(['curl', '-fLsS', '--max-time', '60', '--max-filesize', str(32 << 20),
                 f'https://downloads.videolan.org/pub/videolan/dav1d/{version}/dav1d-{version}.tar.xz',
                 '-o', archive], 65)
        source = extract(Path(archive), work, expected)
        for program in ('cc', 'meson', 'ninja', 'nasm'):
            run([program, '--version'], 30)
        directory = work / 'build'
        run(['meson', 'setup', directory, source, '--buildtype', 'release',
             '--default-library', 'static', '-Denable_tools=true', '-Denable_tests=false',
             '-Denable_examples=false', '-Denable_docs=false'], 60)
        run(['ninja', '-j' + str(jobs), '-C', directory], 180)
        stage = work / 'install'
        (stage / 'bin').mkdir(parents=True)
        binary = stage / 'bin/dav1d'
        shutil.copy2(directory / 'tools/dav1d', binary)
        executable = identity(binary, expected['DAV1D_VERSION'], runner)
        notices = stage / 'licenses'
        notices.mkdir()
        shutil.copy2(source / 'COPYING', notices / 'dav1d-COPYING')
        shutil.copy2(source / 'doc/PATENTS', notices / 'dav1d-PATENTS')
        notice = run(['awk', '!/^;\\*/ { exit } { print }', source / 'src/ext/x86/x86inc.asm'], 30)
        if not notice.strip():
            raise ValueError('dav1d assembly notice is missing')
        (notices / 'dav1d-x86inc-LICENSE').write_text(notice)
        if inputs != {'pin_sha256': digest(PIN), 'recipe_sha256': digest(__file__),
                      'process_recipe_sha256': digest(PROCESS.__file__), **pin()}:
            raise ValueError('decoder build inputs changed during acquisition')
        for command in commands:
            if command['command'][1:] != ['--version']:
                for channel in ('stdout', 'stderr'):
                    content = command.pop(channel).encode()
                    command[channel + '_sha256'] = hashlib.sha256(content).hexdigest()
                    command[channel + '_bytes'] = len(content)
        manifest = {'inputs': inputs, 'executable': executable, 'commands': commands,
                    'published_path': str(prefix / 'bin/dav1d'),
                    'notices': {path.name: digest(path) for path in sorted(notices.iterdir())}}
        (stage / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        owner = stage.stat()
        try:
            PROCESS.publish(stage, prefix)
            final = identity(prefix / 'bin/dav1d', expected['DAV1D_VERSION'], runner)
            if final['sha256'] != executable['sha256']:
                raise ValueError('decoder executable changed during publication')
        except BaseException:
            try:
                published = prefix.lstat()
            except FileNotFoundError:
                pass
            else:
                if os.path.samestat(owner, published):
                    shutil.rmtree(prefix)
            raise
    return final


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, required=True)
    parser.add_argument('--archive', type=Path)
    parser.add_argument('--jobs', type=int, choices=(1, 2), default=2)
    options = parser.parse_args(argv)
    try:
        with PROCESS.cancellation():
            result = build(options.prefix, options.archive, options.jobs)
    except (OSError, ValueError, tarfile.TarError, PROCESS.BuildError) as error:
        print(f'dav1d oracle acquisition failed: {error}', file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    sys.exit(main())
