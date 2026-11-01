#!/usr/bin/env python3
"""Generates `crates/coreav-cli/src/fallback/names.rs`: the public names the
FFmpeg fallback's log keeps (`docs/design/compat-cli.md`, "Fallback to
FFmpeg"). Every list comes from the pinned FFmpeg binaries' own listings
(`ffmpeg -h full`, `ffprobe -h full`, `ffmpeg -encoders`, `ffmpeg -formats`,
`ffprobe -sections`), run as black boxes, except the keys `coreav ffprobe`
prints, which are the entry keys of its own source
(`crates/coreav-cli/src/ffprobe/`). Nothing is taken from a logged command.

    scripts/install-oracle-ffmpeg.sh DIR
    python3 -I scripts/fallback_names.py DIR
    python3 -I scripts/fallback_names.py --check DIR

The vocabulary's options come from `tools/compat-corpus/classify.py` and
override the listing, which shows some flags with an argument name
(`-hide_banner`) and leaves `-i` out; `LISTED_WITHOUT_ARGUMENT` names the
options it shows with none though they take one. Python 3.11 standard
library only.
"""
import argparse
import difflib
import importlib.util
import pathlib
import re
import shlex
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin  # noqa: E402

OUT = ROOT / 'crates/coreav-cli/src/fallback/names.rs'
SOURCE = ROOT / 'crates/coreav-cli/src/ffprobe'
PIN = oracle_pin.load()

# Option sections left out, whose options the log writes as `-OPTION`:
# libaom-av1's private options hold a word the repository's public text
# does not use (docs/design/compat-cli.md names no such option).
SKIPPED = ('libaom-av1 encoder AVOptions:',)
# The options whose values the log keeps by name (the design note's table).
NAMED_VALUES = ('movflags', 'fflags', 'avoid_negative_ts', 'hls_playlist_type')
# ffprobe's options that take a value though the listing shows no argument
# name for them (`-data_dump_format   set data dump format (available formats
# are: xxd, base64)`), which the listing alone reads as a flag.
LISTED_WITHOUT_ARGUMENT = {'-data_dump_format'}

PROGRAM = re.compile(r'\A(-[^\s\[]+)(?:\[:<\w+>\])?(?: (<[^>]*>))?(?:\s{2,}|\Z)')
AVOPTION = re.compile(r'\A  (-\S+)\s+<\S+>')
CONSTANT = re.compile(r'\A     (\S+)\s+(?:(-?\d+)\s+)?[A-Z.]{11}')
ENCODER = re.compile(r'\A [VAS][F.][S.][X.][B.][D.] (\S+)')
FORMAT = re.compile(r'\A [D ][E ][d ] (\S+)')
SECTION = re.compile(r'\A[W.][A.][V.][T.]\s+(\S+)\Z')
KEY = re.compile(r'\(\s*"([a-z][a-z0-9_]*)",')
BOOLEAN_VALUE = re.compile(r'=>\s*\(\s*"[^"]*",\s*(?:true|false)\s*\)')


def classify():
    spec = importlib.util.spec_from_file_location(
        'classify', ROOT / 'tools/compat-corpus/classify.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(program, *args):
    return subprocess.run([str(program), '-hide_banner', *args], check=True,
                          capture_output=True, text=True).stdout


def options(listing, values, flags):
    """{name: takes a value}, and {option: its named constants}. A program
    option's form wins over an AVOption of the same name (`-y` of a
    filter), and the vocabulary's over both."""
    program, avoptions, constants, current, skipping = {}, {}, {}, None, False
    for line in listing.splitlines():
        if line.endswith('AVOptions:'):
            skipping = line in SKIPPED
        if skipping:
            continue
        if m := PROGRAM.match(line):
            program[m.group(1)] = m.group(2) is not None
            current = None
        elif m := AVOPTION.match(line):
            current = m.group(1)
            avoptions[current] = True
        elif current and (m := CONSTANT.match(line)):
            constants.setdefault(current, set()).add(m.group(1))
            if m.group(2):
                constants[current].add(m.group(2))
        elif not line.startswith('     '):
            current = None
    found = avoptions | program
    found |= {name: True for name in values} | {name: False for name in flags}
    return found, constants


def rust_names(name, names):
    bad = sorted(n for n in names if not re.fullmatch(r'[^\s"\\]+', n))
    if bad or not names:
        sys.exit(f'fallback_names.py: {name} cannot hold {bad or "no names"}')
    rows = '\n'.join(sorted(names))
    return f'pub const {name}: &str = "\\\n{rows}";\n'


def rust_options(tool, name, found):
    return (
        f'/// `{tool}`\'s options that take a value.\n'
        + rust_names(f'{name}_VALUES', [o for o, v in found.items() if v])
        + f'\n/// `{tool}`\'s options that take none.\n'
        + rust_names(f'{name}_FLAGS', [o for o, v in found.items() if not v]))


def generate(bin_dir):
    problems = list(oracle_pin.verify_bin(bin_dir))
    if problems:
        sys.exit('\n'.join(problems))
    ffmpeg, ffprobe = bin_dir / 'ffmpeg', bin_dir / 'ffprobe'
    vocabulary = classify()
    ffmpeg_options, constants = options(
        run(ffmpeg, '-h', 'full'),
        vocabulary.FFMPEG_VALUE | set(vocabulary.FFMPEG_CODEC), vocabulary.FFMPEG_FLAG)
    ffprobe_options, _ = options(
        run(ffprobe, '-h', 'full'), vocabulary.FFPROBE_VALUE | LISTED_WITHOUT_ARGUMENT,
        vocabulary.FFPROBE_FLAG)
    encoders = {m.group(1) for line in run(ffmpeg, '-encoders').splitlines()
                if (m := ENCODER.match(line))}
    formats = set()
    for line in run(ffmpeg, '-formats').splitlines():
        if m := FORMAT.match(line):
            formats.add(m.group(1))
            formats.update(m.group(1).split(','))
    sections = set()
    for line in run(ffprobe, '-sections').splitlines():
        if m := SECTION.match(line):
            sections.update(m.group(1).split('/'))
    keys = set()
    for path in sorted(SOURCE.rglob('*.rs')):
        keys.update(KEY.findall(BOOLEAN_VALUE.sub('', path.read_text())))
    keys -= sections
    out = [
        f'//! Generated by `scripts/fallback_names.py` from the listings of FFmpeg\n'
        f'//! {PIN["VERSION"]} (`-h full`, `-encoders`, `-formats`, `ffprobe -sections`)\n'
        f'//! and the entry keys of `coreav ffprobe`; do not edit. Each list is\n'
        f'//! one string, a name per line: an array of `&str` would cost the\n'
        f'//! binary a relocation per name at every start-up.\n\n',
        rust_options('ffmpeg', 'FFMPEG', ffmpeg_options),
        '\n',
        rust_options('ffprobe', 'FFPROBE', ffprobe_options),
        '\n/// Encoder names.\n',
        rust_names('ENCODERS', encoders),
        '\n/// Format names, whole and each comma-separated part.\n',
        rust_names('FORMATS', formats),
    ]
    for option in NAMED_VALUES:
        out.append(f'\n/// The values `-{option}` names.\n')
        out.append(rust_names(option.upper(), constants[f'-{option}']))
    out.append('\n/// `ffprobe`\'s section names, both forms.\n')
    out.append(rust_names('SECTIONS', sections))
    out.append('\n/// The keys `coreav ffprobe` prints outside the tags.\n')
    out.append(rust_names('KEYS', keys))
    return subprocess.run(['rustfmt', '--edition', '2024'], input=''.join(out),
                          capture_output=True, text=True, check=True, cwd=ROOT).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--check', action='store_true')
    parser.add_argument('directory', type=pathlib.Path, help='the pinned ffmpeg and ffprobe')
    args = parser.parse_args()
    generated = generate(args.directory)
    if args.check:
        current = OUT.read_text() if OUT.exists() else ''
        if current == generated:
            return 0
        diff = difflib.unified_diff(current.splitlines(), generated.splitlines(),
                                    fromfile=str(OUT.relative_to(ROOT)), tofile='generated',
                                    lineterm='')
        print('\n'.join(diff), file=sys.stderr)
        directory = shlex.quote(str(args.directory))
        print(f'fallback_names.py: {OUT.relative_to(ROOT)} is not the generator\'s output; '
              f'regenerate it on Linux x86-64: scripts/install-oracle-ffmpeg.sh {directory} '
              f'&& python3 -I scripts/fallback_names.py {directory}', file=sys.stderr)
        return 1
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(generated)
    return 0


if __name__ == '__main__':
    sys.exit(main())
