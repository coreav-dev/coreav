#!/usr/bin/env python3
"""Check the standalone macOS oracle pin and verify installed executables."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_pin

ROOT = Path(__file__).resolve().parent.parent
PIN = 'scripts/oracle-ffmpeg-macos.env'
OWN = ('scripts/oracle_pin_macos.py', 'scripts/tests/test_oracle_pin_macos.py')


def load(root=ROOT):
    pin = {}
    for line in (root / PIN).read_text().splitlines():
        if not line or line.startswith('#'):
            continue
        match = oracle_pin.LINE.fullmatch(line)
        if not match or match[1] in pin:
            raise SystemExit(f'{PIN}: expected unique NAME=value lines')
        pin[match[1]] = match[2]
    return pin


def problems(root=ROOT):
    pin = load(root)
    if pin['MACOS_BUILD'].split('-', 1)[0] != oracle_pin.load(root)['VERSION']:
        yield f'{PIN}: the macOS build must use the shared release version'
    values = [re.compile(re.escape(value)) for value in pin.values()]
    other_pin = re.compile(r'ffmpeg\.martin-riedl\.de/download/|\bMACOS_BUILD\s*=', re.I)
    for relative in oracle_pin.tracked(root):
        if relative == PIN or relative in OWN or relative.startswith('docs/'):
            continue
        try:
            lines = (root / relative).read_text().splitlines()
        except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
            continue
        for number, line in enumerate(lines, 1):
            if other_pin.search(line) or any(value.search(line) for value in values):
                yield f'{relative}:{number}: a macOS oracle pin; read it from {PIN} instead'


def verify_bin(directory, root=ROOT):
    pin = load(root)
    build = pin['MACOS_BUILD']
    if build.split('-', 1)[0] != oracle_pin.load(root)['VERSION']:
        yield f'{PIN}: the macOS build must use the shared release version'
        return
    verified = 0
    for name in ('ffmpeg', 'ffprobe'):
        binary = directory.resolve() / name
        try:
            expected = pin[f'MACOS_{name.upper()}_BINARY_SHA256']
            if hashlib.sha256(binary.read_bytes()).hexdigest() != expected:
                yield f'{binary}: binary SHA-256 mismatch against {PIN}'
                continue
            architecture = subprocess.run(['lipo', '-archs', str(binary)], check=True,
                                          capture_output=True, text=True, timeout=10)
            if architecture.stdout.strip() != 'arm64':
                yield f'{binary}: expected only the arm64 architecture'
                continue
            subprocess.run(['codesign', '--verify', '--strict', '-R',
                            '=anchor apple generic and certificate leaf[subject.OU] = "' +
                            pin['MACOS_SIGNING_TEAM'] + '"', str(binary)], check=True,
                           capture_output=True, text=True, timeout=10)
            result = subprocess.run([str(binary), '-version'], check=True, capture_output=True,
                                    text=True, timeout=10)
        except (OSError, subprocess.SubprocessError) as error:
            yield f'{binary}: cannot verify the macOS oracle: {error}'
            continue
        fields = result.stdout.splitlines()[0].split() if result.stdout else []
        if len(fields) < 3 or fields[:3] != [name, 'version', build]:
            yield f'{binary}: expected the {build} oracle from {PIN}'
            continue
        verified += 1
    if verified != 2:
        return
    binary = directory.resolve() / 'ffmpeg'
    try:
        result = subprocess.run([str(binary), '-hide_banner', '-encoders'], check=True,
                                capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError) as error:
        yield f'{binary}: cannot list the oracle encoders: {error}'
        return
    if not any(re.fullmatch(r'\s*V[A-Z.]{5}\s+h264_videotoolbox\s+.+', line)
               for line in result.stdout.splitlines()):
        yield f'{binary}: the pinned oracle is missing h264_videotoolbox'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--check', action='store_true')
    mode.add_argument('--verify-bin', type=Path)
    parser.add_argument('--root', type=Path, default=ROOT)
    args = parser.parse_args()
    found = list(verify_bin(args.verify_bin, args.root) if args.verify_bin else problems(args.root))
    for problem in found:
        print(problem, file=sys.stderr)
    return bool(found)


if __name__ == '__main__':
    sys.exit(main())
