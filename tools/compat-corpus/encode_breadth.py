#!/usr/bin/env python3
"""V2 and V3 of the 0.1.0 codec coverage bars (docs/design/compat-cli.md,
"The 0.1.0 codec coverage bars"; docs/measurements/compat-0.1.0/
codec-coverage-plan.md, "V2, encode breadth" and "V3, the native share").

    encode_breadth.py run --coreav PATH --harness PATH --ffmpeg PATH --ffprobe PATH
                          --x264 PATH --fixtures DIR --work DIR --records FILE
                          --summary FILE [--set v3|v2] [--mediainfo PATH] [--jobs N]
        Runs the commands of commands-0.1.0/coverage.json (the 630 of V3, or
        with --set v2 the 488 of class "media") one by one. The pinned ffmpeg
        tries the assignments of the fixtures to the command's input
        placeholders in the plan's order; the first it runs to success is the
        command's input. Its "Stream mapping" gives the job of each output
        stream (copy, libx264, libopus, aac, other encoders). COREAV runs the
        command on that input, outside strict-safe mode, with no fallback
        program, a fallback log, and COREAV_X264 and COREAV_FFMPEG naming two
        wrappers that report each start of the program. The output goes to the
        oracles of its jobs: a command that only copies to HARNESS (the
        coreav-cli "ffmpeg" test executable, test
        the_0_1_0_commands_on_listed_pairs, which reports the job CoreAV chose
        and its writer's summary), a byte comparison with the binary, and
        copy_check.py through r3.py; libx264 to B1 to B4 of
        transcode_check.py; libopus and aac to O1 to O4 and A1 to A4 on the
        helpers of opus_encode_check.py; a stream copied beside an encode to
        B3's packet rule. FILE gets one line per command; SUMMARY is
        "summarize"'s result with the SHA-256 of the programs and fixtures.
        Exits 2 when the run is void: a fallback log line whose "ran" is not
        null, or a command with no result.

    encode_breadth.py summarize --records FILE... [--set v3|v2] [--summary FILE]
        V2 (the share against 40%, 196 of 488) and V3 (the share of 630,
        beside the 79 commands classify.py labels native): the counts per
        container and class, the misses by code, the options CoreAV refused,
        and the per-encoder groups, from the records of a run.

The runs use the five fixtures only and read no corpus file. FFmpeg, ffprobe
and x264 are black boxes. Python 3 standard library only.
"""
import argparse
import collections
import concurrent.futures
import hashlib
import itertools
import json
import os
import pathlib
import re
import shlex
import shutil
import signal
import subprocess
import sys
import types

HERE = pathlib.Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent.parent / 'scripts')]
import classify  # noqa: E402
import opus_encode_check as opus  # noqa: E402
import r3  # noqa: E402
import release_round  # noqa: E402
import transcode_check  # noqa: E402
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT  # noqa: E402

SET = HERE / 'commands-0.1.0' / 'coverage.json'
FIXTURES = ('basic.mp4', 'mkv_h264_aac.mkv', 'webm_vp9_opus.webm', 'ts_h264_aac.ts', 'audio_aac.m4a')
RATE_44100 = frozenset({'basic.mp4'})
PLACEHOLDER = re.compile(r'INPUT[23]?')
REFERENCE_SECONDS = 120
COREAV_SECONDS = 300
FILE_BLOCKS = 2 * 1024 * 1024
NO_FALLBACK = '/nonexistent/coreav-fallback'
STDOUT = 'stdout'
BAR_PERCENT = 40
MISSES = ('no_input', 'refused', 'rejected', 'timeout', 'failed', 'ffmpeg_job', 'no_oracle', 'oracle',
          'streams_differ')
GROUPS = ('x264', 'libopus', 'aac', 'other encoders', 'copy only')
COPY_JOBS = ('remux', 'clip', 'split', 'join', 'extract-audio')
REGISTERED = ('copy', 'libx264', 'libopus', 'aac')
UNJUDGED = ('decode-only', 'subtitles', 'other')
TIMING = ('-ss', '-t', '-to', '-sseof', '-itsoffset')
MAPPED = re.compile(r"^\s*Stream #(\d+):(\d+)(?:\[[^]]*\])?(?: \([^)]*\))? -> #0:(\d+) \((.*)\)\s*$", re.M)
ENCODER = re.compile(r'-> (\S+)(?: \(([^)]*)\))?\Z')
NUMBER = re.compile(r'%0?\d*d')

# The bars of the audio encoders (docs/design/transcode.md, "Opus bars" and
# "AAC bars") as the plan applies them to one output: the bit rate ratio's
# bounds for one input, the SNR margin, and whether A4 or O4 judges a 44.1 kHz
# source.
AUDIO = {
    'libopus': {'letter': 'O', 'codec': 'opus', 'rate': (0.85, 1.15), 'margin': 1.0, 'all_rates': False},
    'aac': {'letter': 'A', 'codec': 'aac', 'rate': (0.95, 1.05), 'margin': 0.5, 'all_rates': True},
}

# An FFmpeg start is the AAC encoder's only when its whole invocation is:
# raw samples on standard input, one audio stream encoded with -c:a aac, and
# no option beyond these.
RAW_SAMPLES = frozenset({'u8', 's8', 's16le', 's16be', 'u16le', 'u16be', 's24le', 's24be', 'u24le', 'u24be',
                         's32le', 's32be', 'u32le', 'u32be', 'f32le', 'f32be', 'f64le', 'f64be'})
START_FLAGS = frozenset({'-nostdin', '-hide_banner', '-nostats', '-y', '-n'})
START_VALUES = frozenset({'-loglevel', '-v', '-f', '-ar', '-ac', '-i', '-map', '-c:a', '-codec:a', '-acodec',
                          '-b:a'})
WRAPPER = '''#!/bin/sh
{python} -I -B -S -c 'import json, sys; print(json.dumps({{"program": sys.argv[1], "args": sys.argv[2:]}}))' \
{name} "$@" > {fifo}
exec {program} "$@"
'''

Job = collections.namedtuple('Job', 'output file stream kind')
Ran = collections.namedtuple('Ran', 'code timed_out stderr')
Prepared = collections.namedtuple('Prepared', 'cwd args paths')


def commands(selected):
    """(index in the file, entry) for each command of the set: all 630, or
    with "v2" the 488 of class media."""
    document = json.loads(SET.read_text())['commands']
    return [(i, e) for i, e in enumerate(document) if selected == 'v3' or e['class'] == 'media']


def placeholders(args):
    """The input placeholders of a command, in the order they first stand."""
    return list(dict.fromkeys(a for a in args if PLACEHOLDER.fullmatch(a)))


def assignments(args):
    """The fixtures each placeholder takes, in the order they are tried: the
    placeholder that stands first changes slowest."""
    return list(itertools.product(FIXTURES, repeat=len(placeholders(args))))


def concat_lists(args):
    """The placeholders that stand as the input of -f concat."""
    lists, demuxer = set(), None
    for option, value in zip(args, args[1:]):
        if option == '-f':
            demuxer = value
        elif option == '-i':
            if demuxer == 'concat':
                lists.add(value)
            demuxer = None
    return lists


def stage(args, assignment, fixtures, directory):
    """Copies of the fixtures in directory, a folder for each placeholder; for
    the input of -f concat a list that names the copy twice by its name
    alone, in the same folder: {placeholder: path}. A command may write over
    its input, so every run gets copies of its own."""
    paths, lists = {}, concat_lists(args)
    for placeholder, name in zip(placeholders(args), assignment):
        folder = directory / placeholder
        folder.mkdir(parents=True)
        copy = folder / name
        shutil.copyfile(fixtures / name, copy)
        paths[placeholder] = str(release_round.concat_list(folder, copy) if placeholder in lists else copy)
    return paths


def fill(args, paths, output):
    return [paths.get(a, output if a == 'OUTPUT' else a) for a in args]


def info(args):
    """The command with the value of its own -v or -loglevel replaced by
    info, which prints the stream mapping and changes no output."""
    return ['info' if before in ('-v', '-loglevel') else a for before, a in zip([None] + args, args)]


def timing(args):
    """The timing options of the command as two lists of option and value:
    those before its first -i and those after its last value of -i."""
    inputs = [n for n, a in enumerate(args) if a == '-i']
    if not inputs:
        return [], []

    def pairs(part):
        return [x for n, a in enumerate(part[:-1]) if a in TIMING for x in part[n:n + 2]]

    return pairs(args[:inputs[0]]), pairs(args[inputs[-1] + 2:])


def jobs(stderr):
    """The jobs of the output streams in ffmpeg's Stream mapping: each output
    stream with the input file and stream it comes from, and copy or the
    name of its encoder."""
    found = sorted((int(o), int(f), int(s), text) for f, s, o, text in MAPPED.findall(stderr))
    return [Job(o, f, s, 'copy' if text == 'copy' else encoder(text)) for o, f, s, text in found]


def encoder(text):
    """The encoder in the mapping of "h264 (native) -> h264 (libx264)": the
    name in the last brackets, or the codec's when they say native."""
    match = ENCODER.search(text)
    if not match:
        return text
    return match.group(2) if match.group(2) not in (None, 'native') else match.group(1)


def groups(found):
    """The per-encoder groups of V2's result that a command is in."""
    kinds = {j.kind for j in found}
    names = {'libx264': 'x264', 'libopus': 'libopus', 'aac': 'aac'}
    out = [names[k] for k in kinds if k in names]
    if kinds - {'copy'} - set(names):
        out.append('other encoders')
    if found and kinds == {'copy'}:
        out.append('copy only')
    return sorted(out, key=GROUPS.index)


def aac_encoder_start(args):
    """Whether the arguments of an ffmpeg start are an AAC encoder's, the
    whole invocation and not the presence of its tokens: one input, raw
    samples on standard input (-f a raw sample format, -i pipe:0 or -i -),
    one output, -c:a aac, maps that take audio, and no other option than
    START_FLAGS and START_VALUES hold."""
    inputs, outputs, maps, codec, raw = [], [], [], None, None
    rest = iter(args)
    for a in rest:
        if a in START_FLAGS:
            continue
        if a not in START_VALUES:
            if a.startswith('-') and a != '-':
                return False
            outputs.append(a)
            continue
        value = next(rest, None)
        if value is None:
            return False
        if a == '-f':
            raw = value
        elif a == '-i':
            inputs.append((raw, value))
            raw = None
        elif a == '-map':
            maps.append(value)
        elif a in ('-c:a', '-codec:a', '-acodec'):
            codec = value
    return (len(inputs) == 1 and inputs[0][0] in RAW_SAMPLES and inputs[0][1] in ('pipe:0', '-')
            and len(outputs) == 1 and codec == 'aac' and all(re.fullmatch(r'0:a(:\d+)?', m) for m in maps))


def encoder_start(program, args):
    """Whether a start of the program log is an encoder's: any x264 start,
    or an ffmpeg start that aac_encoder_start takes."""
    return program == 'x264' or (program == 'ffmpeg' and aac_encoder_start(args))


def execute(argv, cwd, env, seconds, pipe=False):
    """Runs a program in cwd for at most seconds of wall time, killing its
    process group at the deadline, and with no file larger than FILE_BLOCKS
    blocks of the shell's ulimit -f (1 GiB or 2 GiB). With pipe its standard output is the file STDOUT of
    cwd."""
    out = open(cwd / STDOUT, 'wb') if pipe else subprocess.DEVNULL
    try:
        limited = ['/bin/sh', '-c', 'ulimit -f "$0" && exec "$@"', str(FILE_BLOCKS)] + [str(a) for a in argv]
        with subprocess.Popen(limited, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=out,
                              stderr=subprocess.PIPE, start_new_session=True) as child:
            try:
                stderr, timed_out = child.communicate(timeout=seconds)[1], False
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                stderr, timed_out = child.communicate()[1], True
    finally:
        if pipe:
            out.close()
    return Ran(child.returncode, timed_out, stderr.decode(errors='replace'))


def nothing_to_find(name):
    """An output name with no file to look for: standard output, which a
    run captures, or a device."""
    return name == '-' or name.startswith('/dev/')


def produced(name, directory):
    """The files a run left for an output name: the captured standard
    output, those that a number pattern matches, or the file of that name."""
    if name == '-':
        return [directory / STDOUT]
    if not name or name.startswith('/dev/'):
        return []
    match = NUMBER.search(name)
    if not match:
        return [directory / name] if (directory / name).exists() else []
    head, tail = name[:match.start()], name[match.end():]
    return sorted(p for p in directory.iterdir() if p.name.startswith(head) and p.name.endswith(tail)
                  and p.name[len(head):len(p.name) - len(tail)].isdigit())


def succeeded(ran, name, directory):
    """Whether a run exited 0 within its time and wrote its output."""
    if ran.code != 0 or ran.timed_out:
        return False
    if nothing_to_find(name):
        return True
    return bool(produced(name, directory))


def unreachable(name):
    """An output the pinned ffmpeg cannot be given: a network address, or a
    device other than the null device."""
    return '://' in name or (name.startswith('/dev/') and name != '/dev/null')


class ProgramLog:
    """The two wrappers that COREAV_X264 and COREAV_FFMPEG name, and the
    starts they report. The encoder runs with no regular-file writes
    (RLIMIT_FSIZE 0, docs/design/decode.md), so a wrapper cannot append to a
    file: it writes one JSON line to a pipe this object holds open, and
    drain reads them."""

    def __init__(self, directory, x264, ffmpeg):
        directory.mkdir(parents=True)
        self.fifo = directory / 'programs.fifo'
        os.mkfifo(self.fifo)
        self.fd = os.open(self.fifo, os.O_RDWR | os.O_NONBLOCK)
        self.wrappers = {}
        for name, program in (('x264', x264), ('ffmpeg', ffmpeg)):
            path = self.wrappers[name] = directory / name
            path.write_text(WRAPPER.format(python=shlex.quote(sys.executable), name=name,
                                           fifo=shlex.quote(str(self.fifo)), program=shlex.quote(str(program))))
            path.chmod(0o755)

    def drain(self):
        """The starts reported so far, one object each."""
        data = b''
        while True:
            try:
                chunk = os.read(self.fd, 1 << 16)
            except BlockingIOError:
                break
            data += chunk
            if not chunk:
                break
        starts = []
        for line in data.decode(errors='replace').splitlines():
            try:
                starts.append(json.loads(line))
            except ValueError:
                starts.append({'program': None, 'args': [line]})
        return starts

    def close(self):
        os.close(self.fd)


def fallback_lines(path):
    lines = []
    for line in (path.read_text(errors='replace').splitlines() if path.exists() else []):
        if line.strip():
            try:
                lines.append(json.loads(line))
            except ValueError:
                lines.append({'raw': line})
    return lines


def environment():
    """The environment of every program: no gconv modules, and none of the
    runner's CoreAV or credential variables."""
    env = {k: v for k, v in os.environ.items() if not k.startswith(('COREAV_', 'R2_', 'AWS_', 'CORPUS_R2'))}
    env['GCONV_PATH'] = release_round.NO_GCONV
    return env


def prepare(opts, entry, base, tag, assignment):
    """A run's directories, base/tag/in with the inputs and base/tag/out, the
    working directory, and the command filled in."""
    cwd = base / tag / 'out'
    cwd.mkdir(parents=True)
    paths = stage(entry['args'], assignment, opts.fixtures, base / tag / 'in')
    return Prepared(cwd, fill(entry['args'], paths, entry['output']), paths)


def choose(opts, entry, base):
    """The first assignment on which the pinned ffmpeg runs the command to
    success and writes its output: (assignment, its directories), else
    None."""
    if unreachable(entry['output']) or 'OUTPUT' not in entry['args']:
        return None
    for n, assignment in enumerate(assignments(entry['args'])):
        run = prepare(opts, entry, base, f'ffmpeg{n}', assignment)
        ran = execute([opts.ffmpeg] + run.args, run.cwd, opts.env, REFERENCE_SECONDS, entry['output'] == '-')
        if succeeded(ran, entry['output'], run.cwd):
            return assignment, run
        shutil.rmtree(base / f'ffmpeg{n}')
    return None


def scrub(opts, text):
    for root in {str(opts.work), str(opts.work.resolve())}:
        text = text.replace(root, '$WORK')
    return text


def last_line(opts, text):
    lines = [line for line in scrub(opts, text).splitlines() if line.strip()]
    return lines[-1][:200] if lines else ''


def rejected_kind(stderr):
    return next((kind for kind in ('truncated_structure', 'invalid_structure')
                 if kind.replace('_', ' ') in stderr), None)


def failure(opts, entry, c):
    """Why a run of coreav is a miss before any oracle sees it: the first of
    timeout, refused, rejected and failed that applies, else None."""
    if c.ran.timed_out:
        return 'timeout', {}
    if c.ran.code == 4:
        return 'refused', {'reason': next((l['reason'] for l in c.lines if l.get('reason')), 'none')}
    kind = rejected_kind(c.ran.stderr) if c.ran.code == 1 else None
    if kind:
        return 'rejected', {'kind': kind}
    if not succeeded(c.ran, entry['output'], c.run.cwd):
        return 'failed', {'status': c.ran.code, 'stderr': last_line(opts, c.ran.stderr)}
    return None


def coreav_run(opts, entry, base, assignment):
    """coreav ffmpeg on the command: no fallback program, a fallback log, and
    the wrappers; its exit, its directories, the fallback log's lines and
    the starts of the encoders."""
    run = prepare(opts, entry, base, 'coreav', assignment)
    log = base / 'fallback.log'
    program = ProgramLog(base / 'programs', opts.x264, opts.ffmpeg)
    env = {**opts.env, 'COREAV_FALLBACK': NO_FALLBACK, 'COREAV_FALLBACK_LOG': str(log),
           'COREAV_X264': str(program.wrappers['x264']), 'COREAV_FFMPEG': str(program.wrappers['ffmpeg'])}
    try:
        ran = execute([opts.coreav, 'ffmpeg'] + run.args, run.cwd, env, COREAV_SECONDS, entry['output'] == '-')
        starts = program.drain()
    finally:
        program.close()
    for start in starts:
        start['encoder'] = encoder_start(start.get('program'), start.get('args', []))
        start['args'] = [scrub(opts, a) for a in start.get('args', [])]
    return types.SimpleNamespace(ran=ran, run=run, lines=fallback_lines(log), starts=starts)


def harness(cx):
    """The record the coreav-cli test executable makes of the command run
    through the function coreav ffmpeg runs, in strict-safe mode and in a
    directory of its own: (record, the directory's Prepared)."""
    run = prepare(cx.opts, cx.entry, cx.base, 'harness', cx.assignment)
    pairs, records = cx.base / 'pairs.json', cx.base / 'records.json'
    pairs.write_text(json.dumps([{'id': 'c', 'command': 0, 'args': run.args, 'work': str(run.cwd)}]))
    env = {**cx.opts.env, 'COREAV_R3_PAIRS': str(pairs), 'COREAV_R3_RECORDS': str(records)}
    ran = execute([cx.opts.harness, '--ignored', '--exact', r3.TEST, '--test-threads', '1'], run.cwd, env,
                  COREAV_SECONDS)
    if ran.code != 0 or not records.exists():
        raise RuntimeError('the harness timed out' if ran.timed_out else 'the harness crashed')
    return json.loads(records.read_text())[0], run


def copy_only(cx):
    """The oracle of a command that only copies: the job the harness says
    CoreAV chose, the same bytes from the binary, and copy_check.py's
    oracle of that job."""
    name = cx.entry['output']
    if nothing_to_find(name) or not cx.run.paths:
        return 'no_oracle', {'job': 'output not a file' if nothing_to_find(name) else 'no input'}
    record, run = harness(cx)
    chosen = record['chosen']
    if record['status'] == 'written' and chosen not in COPY_JOBS:
        return 'no_oracle', {'job': chosen or 'unknown'}
    if record['status'] != 'written' or not record['files'] or r3.written(run.cwd) != r3.written(cx.run.cwd):
        return 'oracle', {'check': 'binary_differs'}
    names = dict(zip(placeholders(cx.entry['args']), cx.assignment))
    primary = 'INPUT' if 'INPUT' in run.paths else next(iter(run.paths))
    record.update(job=chosen, args=fill(cx.entry['args'], {k: v for k, v in run.paths.items() if k != 'INPUT'}, 'OUTPUT'),
                  source=str(cx.opts.fixtures / names[primary]), input=run.paths[primary], output=str(run.cwd / name))
    for segment in record['detail'].get('segments', []):
        segment['output']['path'] = str(run.cwd / segment['output']['path'])
    scratch = cx.base / 'check'
    scratch.mkdir()
    outcome, why = r3.check_copies(cx.opts.check, [record], scratch, cx.opts.env)[('c', 0)]
    if why == 'copy_check crashed':
        raise RuntimeError(why)
    if outcome == 'set aside':
        return 'no_oracle', {'job': chosen}
    return ('oracle', {'check': 'copy_check', 'why': why}) if outcome == 'failed' else None


def source_of(cx, file):
    """The fixture (name and path) that the command's file-th input is, or
    (None, None) for an input that is not one."""
    inputs = [b for a, b in zip(cx.entry['args'], cx.entry['args'][1:]) if a == '-i']
    name = dict(zip(placeholders(cx.entry['args']), cx.assignment)).get(inputs[file] if file < len(inputs) else None)
    return name, (cx.opts.fixtures / name if name else None)


def source(cx, file):
    """The transcode_check source of the command's file-th input, with the
    packets of every stream kind of coreav remux read."""
    if file not in cx.sources:
        cx.sources[file] = transcode_check.Source(cx.tc, str(source_of(cx, file)[1]))
        cx.sources[file].kinds = ('audio', 'video')
    return cx.sources[file]


def copied(cx, job, out):
    """B3's rule for a stream copied beside an encode: its packets equal the
    source stream's in count and data, and its timestamps those of the same
    stream of coreav remux of the source."""
    src = source(cx, job.file)
    mine = transcode_check.packets(cx.tc.ffprobe, out, str(job.output), data_hash=True)
    theirs = transcode_check.packets(cx.tc.ffprobe, src.path, str(job.stream), data_hash=True)
    if mine[0] is None or theirs[0] is None:
        return False
    remux = src.remux(cx.scratch)
    stream = remux.get(job.stream) if isinstance(remux, dict) else None
    return transcode_check.compare_audio(theirs, mine, stream if stream and stream[0] is not None else None)['ok']


def video_checks(cx, job, out, ref, listing):
    """B1 to B4 of the stream libx264 encoded, with the pinned ffmpeg's output
    of the command as B4's baseline; B3 also judges every other stream the
    output copies."""
    tc = cx.tc
    kind = [t for t, _ in listing]
    copies = [j for j in cx.jobs if j.kind == 'copy' and j.output < len(kind)]
    same = [j for j in copies if kind[j.output] == 'audio' and j.file == job.file]
    extra = sum(1 for j in cx.jobs if j.output < len(kind) and kind[j.output] == 'audio') - len(same)
    layout = {'video': job.output, 'audio': [j.output for j in same], 'videos': kind.count('video'),
              'streams': len(kind)}
    src = source(cx, job.file)
    judged = transcode_check.judge_output(tc, src, out, layout, job.stream, [j.stream for j in same], cx.scratch,
                                          expected_audio=[j.stream for j in same], extra_audio=extra)
    yield 'B1', judged['B1']['ok']
    yield 'B2', judged['B2']['ok']
    yield 'B3', judged.get('B3', {'ok': True})['ok'] and all(copied(cx, j, out) for j in copies if j not in same)
    width, height = judged['video']['width'], judged['video']['height']
    base_width, base_height = transcode_check.probe_size(tc, ref, job.output)
    if not (width and height and base_width and base_height):
        yield 'B4', False
        return
    q1 = transcode_check.psnr(tc.ffmpeg, out, job.output, src.path, job.stream, width, height)
    baseline = transcode_check.psnr(tc.ffmpeg, ref, job.output, src.path, job.stream, base_width, base_height)
    q2 = transcode_check.psnr(tc.ffmpeg, out, job.output, ref, job.output, width, height)
    delta = 0.0 if q1['average'] == baseline['average'] else (q1['average'] or 0) - (baseline['average'] or 0)
    yield 'B4', (q1['ok'] and baseline['ok'] and q2['ok'] and delta >= transcode_check.Q1_FLOOR
                 and q2['average'] >= transcode_check.Q2_FLOOR)


def audio_checks(cx, job, out, ref):
    """O1 to O4 for a stream libopus encoded and A1 to A4 for one aac
    encoded (docs/design/transcode.md), one output against the pinned
    ffmpeg's output of the same command; for libopus first that the stream
    is not a packet copy of a source stream."""
    bars, tc = AUDIO[job.kind], cx.tc
    letter, at = bars['letter'], str(job.output)
    name, path = source_of(cx, job.file)
    if job.kind == 'libopus':
        yield 'not_copy', transcode_check.audio_origin(tc, source(cx, job.file), out, job.output) is None
    decode = opus.run([tc.ffmpeg, '-nostdin', '-v', 'error', *AUDIO_DECODE_INPUT, '-i', out, '-map', '0:a',
                       '-f', 'null', '-'])
    yield letter + '1', decode.returncode == 0 and not decode.stderr
    ours, theirs = opus.probe_audio(tc.ffprobe, out, at), opus.probe_audio(tc.ffprobe, ref, at)
    timeline = (ours.get('codec_name') == bars['codec'] and ours.get('start_pts') == 0
                and ours.get('duration_ts') is not None and theirs.get('duration_ts') is not None)
    if job.kind == 'libopus':
        timeline = (timeline and ours.get('initial_padding') == theirs.get('initial_padding')
                    and abs(ours['duration_ts'] - theirs['duration_ts']) <= 48)
    else:
        timeline = (timeline and ours.get('profile') == 'LC' and ours.get('sample_rate') == theirs.get('sample_rate')
                    and ours['duration_ts'] == theirs['duration_ts'])
    yield letter + '2', timeline
    try:
        ratio = int(ours.get('bit_rate', 0)) / max(1, int(theirs.get('bit_rate', 1)))
    except ValueError:
        ratio = 0
    yield letter + '3', bars['rate'][0] <= ratio <= bars['rate'][1]
    if bars['all_rates'] or name not in RATE_44100:
        before, after = timing(cx.entry['args'])
        reference = opus.pcm(tc.ffmpeg, str(path), str(job.stream), before, after)
        mine = opus.quality(reference, opus.pcm(tc.ffmpeg, out, at))
        baseline = opus.quality(reference, opus.pcm(tc.ffmpeg, ref, at))
        yield letter + '4', mine is not None and baseline is not None and mine >= baseline - bars['margin']


def checks(cx, out, ref, listing):
    """The checks of a command that encodes, as (name, passed) in the order
    they are made; the caller stops at the first failure."""
    encoded = {k: [j for j in cx.jobs if j.kind == k] for k in ('libx264', 'libopus', 'aac')}
    if encoded['libx264']:
        yield 'x264_start', any(s['program'] == 'x264' for s in cx.starts)
    if encoded['aac']:
        yield 'aac_start', any(s['program'] == 'ffmpeg' and s['encoder'] for s in cx.starts)
    if encoded['libx264']:
        yield from video_checks(cx, encoded['libx264'][0], out, ref, listing)
    else:
        yield 'B3', all(copied(cx, j, out) for j in cx.jobs if j.kind == 'copy')
    for kind in ('libopus', 'aac'):
        for job in encoded[kind]:
            yield from audio_checks(cx, job, out, ref)


def encoding(cx):
    """The oracle of a command with a stream that libx264, libopus or aac
    encodes: its one output file, against the pinned ffmpeg's."""
    name = cx.entry['output']
    outs, refs = produced(name, cx.run.cwd), produced(name, cx.reference.cwd)
    if len(outs) != 1 or len(refs) != 1:
        return 'no_oracle', {'job': 'output not one file'}
    if len([j for j in cx.jobs if j.kind == 'libx264']) > 1:
        return 'no_oracle', {'job': 'libx264'}
    out, ref = str(outs[0]), str(refs[0])
    for check, passed in checks(cx, out, ref, transcode_check.written(cx.tc, out)[0]):
        if not passed:
            return 'oracle', {'check': check}
    return None


def oracle(cx):
    """The first miss that the oracles of the command's jobs find, else
    None."""
    kinds = [j.kind for j in cx.jobs]
    if cx.entry['class'] in UNJUDGED:
        return 'no_oracle', {'job': cx.entry['class']}
    if not kinds:
        return 'no_oracle', {'job': 'none'}
    if any(k not in REGISTERED for k in kinds):
        return 'no_oracle', {'job': next(k for k in kinds if k not in REGISTERED)}
    if any(source_of(cx, j.file)[0] is None for j in cx.jobs):
        return 'no_oracle', {'job': 'input not a fixture'}
    return copy_only(cx) if set(kinds) == {'copy'} else encoding(cx)


def listings(cx):
    """The stream listings, by kind and codec in order, of both outputs when
    they differ; for outputs of several files, file by file."""
    name = cx.entry['output']
    mine = [transcode_check.written(cx.tc, str(p))[0] for p in produced(name, cx.run.cwd)]
    pinned = [transcode_check.written(cx.tc, str(p))[0] for p in produced(name, cx.reference.cwd)]
    return None if mine == pinned else {'coreav': mine, 'ffmpeg': pinned}


def header(index, entry):
    return {'index': index, 'class': entry['class'], 'container': entry['container'], 'args': entry['args'],
            'output': entry['output']}


def judge(opts, index, entry, base):
    """One command's record: its input, the jobs of its streams, the run of
    coreav, and the first miss that applies or none."""
    record = header(index, entry)
    chosen = choose(opts, entry, base)
    if chosen is None:
        return {**record, 'counted': False, 'miss': 'no_input', 'detail': {}}
    assignment, reference = chosen
    mapping = prepare(opts, entry, base, 'info', assignment)
    found = jobs(execute([opts.ffmpeg] + info(mapping.args), mapping.cwd, opts.env, REFERENCE_SECONDS,
                         entry['output'] == '-').stderr)
    c = coreav_run(opts, entry, base, assignment)
    record.update(inputs=list(assignment), jobs=[j.kind for j in found], groups=groups(found), exit=c.ran.code,
                  starts=c.starts, void=any(l.get('ran') is not None for l in c.lines))
    miss = failure(opts, entry, c)
    if miss is None and any(not s['encoder'] for s in c.starts):
        miss = 'ffmpeg_job', {}
    if miss is None:
        cx = types.SimpleNamespace(opts=opts, entry=entry, base=base, assignment=assignment, jobs=found, run=c.run,
                                   reference=reference, starts=c.starts, sources={}, scratch=base / 'scratch',
                                   tc=opts.tc)
        cx.scratch.mkdir()
        miss = oracle(cx)
        difference = listings(cx) if miss is None else None
        miss = ('streams_differ', difference) if difference else miss
    code, detail = miss or (None, {})
    return {**record, 'counted': code is None, 'miss': code, 'detail': detail}


def evaluate(opts, index, entry):
    """judge, with a failure of the tools themselves recorded as a fault,
    which leaves the command with no result and voids the run."""
    base = opts.work / f'{index:03d}'
    shutil.rmtree(base, ignore_errors=True)
    base.mkdir(parents=True)
    try:
        return judge(opts, index, entry, base)
    except Exception as error:  # noqa: BLE001
        return {**header(index, entry), 'counted': False, 'miss': None,
                'fault': scrub(opts, f'{type(error).__name__}: {error}')[:300]}
    finally:
        shutil.rmtree(base, ignore_errors=True)


def tally(rows):
    return {'commands': len(rows), 'counted': sum(1 for r in rows if r['counted'])}


def misses(rows):
    found = collections.Counter(r['miss'] for r in rows if r.get('miss'))
    return {code: found[code] for code in MISSES if found[code]}


def share(rows, total):
    counted = sum(1 for r in rows if r['counted'])
    return {'counted': counted, 'of': total, 'share': round(counted / total, 4)}


def summarize(records, selected):
    """V2 and V3 from the records of a run."""
    expected = {i for i, _ in commands(selected)}
    seen = [r['index'] for r in records]
    absent = sorted(expected - set(seen))
    faults = sorted(r['index'] for r in records if r.get('fault'))
    ran = sorted(r['index'] for r in records if r.get('void'))
    reasons = [text for text, found in (('a fallback log line shows a program that ran', ran),
                                         ('a command has no result', faults + absent),
                                         ('a command is recorded twice', len(seen) - len(set(seen)))) if found]
    media = [r for r in records if r['class'] == 'media']
    total = len(commands('v2'))
    v2 = {**share(media, total), 'bar_percent': BAR_PERCENT, 'needed': -(-total * BAR_PERCENT // 100)}
    v2['met'] = v2['counted'] >= v2['needed']
    v2['per_container'] = {c: tally([r for r in media if r['container'] == c])
                           for c in sorted({r['container'] for r in media})}
    v2['misses'] = misses(media)
    v2['refused_options'] = dict(collections.Counter(
        r['detail']['reason'] for r in media if r.get('miss') == 'refused').most_common())
    v2['per_encoder'] = {}
    for group in GROUPS:
        rows = [r for r in media if group in r.get('groups', [])]
        v2['per_encoder'][group] = {**tally(rows), 'misses': misses(rows)}
    summary = {'schema': 'coreav.encode-breadth/1', 'set': selected, 'records': len(records),
               'void': {'void': bool(reasons), 'reasons': reasons, 'fallback_ran': ran, 'faults': faults,
                        'missing': absent},
               'v2': v2}
    if selected == 'v3':
        native = [r for r in records if classify.label('ffmpeg', r['args'])[0] == 'native']
        summary['v3'] = {**share(records, len(expected)),
                         'per_class': {c: tally([r for r in records if r['class'] == c])
                                       for c in sorted({r['class'] for r in records})},
                         'misses': misses(records), 'native': tally(native)}
    return summary


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def brief(summary):
    shares = {key: {k: summary[key][k] for k in ('counted', 'of', 'share')} for key in ('v2', 'v3') if key in summary}
    print(json.dumps({'records': summary['records'], **shares, 'void': summary['void']}))
    return 2 if summary['void']['void'] else 0


def cmd_run(opts):
    missing = [name for name in FIXTURES if not (opts.fixtures / name).is_file()]
    if missing:
        print(f'UNMEASURED: no fixture {", ".join(missing)} in {opts.fixtures}')
        return release_round.UNMEASURED
    for path in (opts.work, opts.records.parent, opts.summary.parent):
        path.mkdir(parents=True, exist_ok=True)
    opts.env = environment()
    os.environ['GCONV_PATH'] = release_round.NO_GCONV
    transcode_check.ENV = opts.env
    opts.tc = types.SimpleNamespace(ffmpeg=opts.ffmpeg, ffprobe=opts.ffprobe, coreav=opts.coreav, decode_oracle=None)
    opts.check = types.SimpleNamespace(ffmpeg=opts.ffmpeg, ffprobe=opts.ffprobe, mediainfo=opts.mediainfo,
                                       log=str(opts.work / 'copy_check.log'))
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        records = sorted(pool.map(lambda c: evaluate(opts, *c), commands(opts.set)), key=lambda r: r['index'])
    opts.records.write_text(''.join(json.dumps(r) + '\n' for r in records))
    summary = summarize(records, opts.set)
    summary['programs'] = {name: sha256(getattr(opts, name)) for name in ('coreav', 'harness', 'ffmpeg', 'ffprobe', 'x264')}
    summary['fixtures'] = {name: sha256(opts.fixtures / name) for name in FIXTURES}
    opts.summary.write_text(json.dumps(summary, indent=1) + '\n')
    return brief(summary)


def cmd_summarize(opts):
    summary = summarize([json.loads(line) for path in opts.records for line in open(path) if line.strip()], opts.set)
    if opts.summary:
        opts.summary.write_text(json.dumps(summary, indent=1) + '\n')
    return brief(summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='mode', required=True)
    p = sub.add_parser('run')
    for name in ('coreav', 'harness', 'ffmpeg', 'ffprobe', 'x264'):
        p.add_argument('--' + name, required=True)
    p.add_argument('--mediainfo')
    p.add_argument('--fixtures', type=pathlib.Path, required=True)
    p.add_argument('--work', type=pathlib.Path, required=True)
    p.add_argument('--records', type=pathlib.Path, required=True)
    p.add_argument('--summary', type=pathlib.Path, required=True)
    p.add_argument('--set', choices=('v3', 'v2'), default='v3')
    p.add_argument('--jobs', type=int, default=4)
    p = sub.add_parser('summarize')
    p.add_argument('--records', nargs='+', required=True)
    p.add_argument('--set', choices=('v3', 'v2'), default='v3')
    p.add_argument('--summary', type=pathlib.Path)
    opts = parser.parse_args()
    return cmd_run(opts) if opts.mode == 'run' else cmd_summarize(opts)


if __name__ == '__main__':
    sys.exit(main())
