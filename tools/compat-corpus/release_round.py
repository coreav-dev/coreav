"""What every tool of the 0.1.0 round shares (docs/design/compat-cli.md,
"The 0.1.0 bars, pre-registered"; docs/measurements/compat-0.1.0/README.md):
the commands of `commands-0.1.0`, the container family of a file, the keys
the set prints, the environment both tools run in and the verdict of the
fallback log. Python 3 standard library only.
"""
import json
import os
import pathlib
import shutil

HERE = pathlib.Path(__file__).resolve().parent
SET = HERE / 'commands-0.1.0' / 'commands.json'

# The container families of the set, by the names its `containers` use, and
# the extensions of each, as the draw's coverage rule lists them
# (docs/design/compat-cli.md, "The held-out draw").
FAMILIES = {
    'mp4-mov': ('mp4', 'mov', 'm4v', 'm4a', '3gp'),
    'matroska-webm': ('mkv', 'webm'),
    'mpegts': ('ts', 'm2ts', 'mts'),
}

# A directory with no gconv modules, as scripts/oracle-ffprobe.sh sets: a
# static FFmpeg build that loads a newer system's gconv modules can crash on
# transport streams with broadcast text (docs/design/mpegts.md). Both tools
# get it, so that they run in the same environment.
NO_GCONV = '/nonexistent/gconv'
UNMEASURED = 3

# The sections of the set's key names (`stream.codec_name`,
# `stream_tags.rotate`) and the prefixes of the same keys in the flattened
# listings of `run.py keys` (`streams.codec_name`, `streams.tags.rotate`).
SECTIONS = {'stream': 'streams', 'format': 'format', 'packet': 'packets',
            'chapter': 'chapters', 'program': 'programs',
            'stream_tags': 'streams.tags', 'format_tags': 'format.tags'}


def entries():
    return json.loads(SET.read_text())['entries']


def commands(tool):
    """(entry, index, command) for every command of `tool` in the set."""
    return [(e, n, c) for e in entries() if e['tool'] == tool
            for n, c in enumerate(e['commands'])]


def containers(entry, command):
    """The families a command runs on: its own `input_container` (a
    command that forces a demuxer), else its entry's."""
    own = command.get('input_container')
    return [own] if own else entry['containers']


def family(path):
    """The family of a file, by its extension, or None."""
    ext = pathlib.Path(path).suffix.lower().lstrip('.')
    return next((f for f, exts in FAMILIES.items() if ext in exts), None)


def key_patterns():
    """The keys the set prints, as prefixes of the flattened names: a name
    ending in `.` covers every key under it (`stream.*`), any other is one
    key."""
    out = set()
    for e in entries():
        for key in e['keys']:
            section, _, name = key.partition('.')
            prefix = SECTIONS[section]
            out.add(prefix + '.' if name == '*' else f'{prefix}.{name}')
    return frozenset(out)


def in_set(name, patterns):
    return any(name.startswith(p) if p.endswith('.') else name == p for p in patterns)


def environment(log=None):
    """The environment of every run of both tools: strict-safe mode, so a
    command `coreav` does not run natively is refused and never handed to
    an FFmpeg program; no gconv modules; and, with `log`, the fallback log
    of the run."""
    env = {**os.environ, 'COREAV_STRICT_SAFE': '1', 'GCONV_PATH': NO_GCONV}
    env.pop('COREAV_FALLBACK', None)
    if log is not None:
        env['COREAV_FALLBACK_LOG'] = str(log)
    return env


def fallback_verdict(log):
    """The fallback log of a run: its lines, the refusals of strict-safe
    mode among them (counted for diagnosis), and whether the run is void.
    A run whose log holds any line is void, a refusal included
    (docs/measurements/compat-0.1.0/README.md, "The commit")."""
    path = pathlib.Path(log)
    lines = [line for line in path.read_text(errors='replace').splitlines()
             if line.strip()] if path.exists() else []
    refused = 0
    for line in lines:
        try:
            refused += json.loads(line).get('not_run') == 'strict_safe'
        except (ValueError, AttributeError):
            pass
    return {'lines': len(lines), 'refused': refused, 'void': bool(lines)}


# The stream types an `input_without` letter names, as ffprobe's
# `codec_type` spells them.
KINDS = {'v': 'video', 'a': 'audio', 's': 'subtitle', 'd': 'data', 't': 'attachment'}


def selected(spec, streams):
    """The indexes of the streams of input 0 a stream specifier of `-map`
    selects (`0`, `0:1`, `0:a`, `0:a:3`, `0:m:language:eng`), from
    ffprobe's stream listing."""
    parts = spec.split(':')[1:]
    if not parts:
        return {s['index'] for s in streams}
    if parts[0].isdigit():
        return {s['index'] for s in streams if s['index'] == int(parts[0])}
    if parts[0] == 'm' and len(parts) == 3:
        return {s['index'] for s in streams if s.get('tags', {}).get(parts[1]) == parts[2]}
    of_kind = [s['index'] for s in streams if s.get('codec_type') == KINDS.get(parts[0])]
    if len(parts) > 1:
        n = int(parts[1])
        return {of_kind[n]} if n < len(of_kind) else set()
    return set(of_kind)


def can_take(entry, command, fam, streams):
    """Whether a file of family `fam` with these streams (ffprobe's
    listing) can take an `ffmpeg` command: the command runs on the family,
    the file has none of the stream types its `input_without` names, every
    `-map` that adds streams selects one, and what the maps leave is not
    empty (docs/design/compat-cli.md, "The 0.1.0 bars": R3's rule)."""
    if fam not in containers(entry, command) or not streams:
        return False
    kinds = {s.get('codec_type') for s in streams}
    if any(KINDS[k] in kinds for k in command.get('input_without', [])):
        return False
    args = command['args']
    maps = [args[i + 1] for i, a in enumerate(args[:-1]) if a == '-map']
    chosen = set()
    for m in maps:
        if not m.startswith('-'):
            picked = selected(m, streams)
            if not picked:
                return False
            chosen |= picked
    for m in maps:
        if m.startswith('-'):
            chosen -= selected(m[1:], streams)
    return not maps or bool(chosen)


def output_name(job):
    """The output of an `ffmpeg` command, as the fixture test names it."""
    return {'extract-audio': 'out.m4a', 'split': 'out%03d.mp4'}.get(job, 'out.mp4')


def concat_list(directory, source):
    """A concat list in `directory` that names `source` twice by its name
    alone, beside a link to it, as the fixture test writes it."""
    directory = pathlib.Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    copy = directory / pathlib.Path(source).name
    if not copy.exists():
        try:
            os.link(source, copy)
        except OSError:
            shutil.copyfile(source, copy)
    listing = directory / 'list.txt'
    listing.write_text(f"file '{copy.name}'\n" * 2)
    return listing


def fill(args, input_path, output_path):
    return [str(input_path) if a == 'INPUT' else str(output_path) if a == 'OUTPUT' else a for a in args]
