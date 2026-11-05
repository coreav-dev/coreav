"""Resolve and qualify the immutable oracle selection of one test process."""
import argparse
import ast
from dataclasses import dataclass
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading

sys.path.insert(0, str(Path(__file__).resolve().parent))

import oracle_pin
import oracle_pin_macos


@dataclass(frozen=True)
class Oracle:
    ffmpeg: Path
    ffprobe: Path


class QualificationError(RuntimeError):
    pass


_lock = threading.Lock()
_selection = None
_result = None
SUPPORT = ('test_oracle.py', 'oracle_pin.py', 'oracle_pin_macos.py',
           'oracle-ffmpeg.env', 'oracle-ffmpeg-macos.env', 'oracle-audio-args.txt')


def bundle(root, entries):
    source = Path(__file__).resolve().parent
    target = Path(root) / 'scripts'
    target.mkdir(parents=True, exist_ok=True)
    pending = [*entries, *(name for name in SUPPORT if name.endswith('.py'))]
    copied = set()
    while pending:
        name = pending.pop()
        if name in copied:
            continue
        copied.add(name)
        path = source / name
        shutil.copyfile(path, target / name)
        for node in ast.walk(ast.parse(path.read_text())):
            modules = [item.name for item in node.names] if isinstance(node, ast.Import) else \
                [node.module] if isinstance(node, ast.ImportFrom) and node.level == 0 else []
            for module in modules:
                dependency = module.split('.')[0] + '.py'
                if (source / dependency).is_file():
                    pending.append(dependency)
    for name in SUPPORT:
        if not name.endswith('.py'):
            shutil.copyfile(source / name, target / name)
    if 'composed_audio.py' in copied:
        for relative in ('docs/design/composed-opus-aac-v2.json', 'docs/design/audio-decode.md',
                         'crates/coreav-audio/src/convert.rs', 'scripts/composed_audio_fixtures.py'):
            destination = Path(root) / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source.parent / relative, destination)


def _path(value):
    value = os.fspath(value)
    if Path(value).name == value:
        value = shutil.which(value) or value
    return Path(value).absolute()


def require(ffmpeg=None, ffprobe=None):
    global _selection, _result
    selected = ffmpeg if ffmpeg is not None else os.environ.get('COREAV_FFMPEG')
    if selected is None:
        selected = shutil.which('ffmpeg') or '/opt/coreav-oracle-bin/ffmpeg'
    selected = _path(selected)
    probe = ffprobe if ffprobe is not None else os.environ.get('COREAV_FFPROBE')
    probe = _path(probe) if probe is not None else selected.with_name('ffprobe')
    requested = (selected.resolve(), probe.resolve())
    with _lock:
        if _result is not None:
            if requested != _selection:
                raise QualificationError('stage=oracle_qualification: selection changed within process')
            if isinstance(_result, QualificationError):
                raise _result
            return _result
        _selection = requested
        try:
            directory = selected.parent.resolve()
            if requested != ((directory / 'ffmpeg').resolve(), (directory / 'ffprobe').resolve()):
                raise QualificationError('selected executables differ from the installed oracle pair')
            if sys.platform not in ('linux', 'darwin'):
                raise QualificationError('no registered oracle for this platform')
            verifier = oracle_pin_macos if sys.platform == 'darwin' else oracle_pin
            problems = tuple(verifier.verify_bin(directory))
            if problems:
                raise QualificationError('; '.join(problems))
            if any('\n' in str(path) or '\r' in str(path) for path in requested):
                raise QualificationError('oracle paths contain a line separator')
            _result = Oracle(*requested)
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, SystemExit) as error:
            _result = QualificationError(f'stage=oracle_qualification: {error}')
        if isinstance(_result, QualificationError):
            raise _result
    return _result


def verify_bin(directory, root=oracle_pin.ROOT):
    try:
        if Path(root).resolve() != oracle_pin.ROOT:
            raise QualificationError('stage=oracle_qualification: alternate pin root')
        require(Path(directory) / 'ffmpeg', Path(directory) / 'ffprobe')
    except QualificationError as error:
        return (str(error),)
    return ()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--resolve', action='store_true', required=True)
    parser.add_argument('--ffmpeg')
    parser.add_argument('--ffprobe')
    args = parser.parse_args()
    try:
        oracle = require(args.ffmpeg, args.ffprobe)
    except QualificationError as error:
        print(error, file=sys.stderr)
        return 1
    print(oracle.ffmpeg)
    print(oracle.ffprobe)
    return 0


if __name__ == '__main__':
    sys.exit(main())
