#!/usr/bin/env python3
"""What the compatibility command does with one invocation
(docs/design/compat-cli.md, "The command-coverage set").

`label(tool, args)` returns one of

- `native`: every option in it is in step 1's vocabulary and the job is one
  of step 1's. `coreav ffprobe`/`coreav ffmpeg` must
  run it and its output must pass the oracles.
- `out-of-scope`: a real invocation of a job step 1 does not do (an encode,
  a filter, a device, several inputs, a container CoreAV does not read or
  write, an option not implemented). The command must refuse it with a
  clear error, never run it differently.
- `unusable`: not a job at all — the extraction cut the line short, or a
  value in it was never expanded. Counted and listed, outside both bars.

with the reason: the first option or token that decided it.

`normalize(tool, args)` replaces the file names with placeholders, which is
the form the set is committed in: what a command-coverage set measures is
which options are combined, and every run substitutes a corpus file for
each input anyway.

The vocabulary below is step 1's, written down before anything was
measured. Widening it is a change to this file, so the labels of the
committed set move with it and the difference shows in the pull request
that makes it; `tests/` recompute every label and check this file against
the vocabulary table of the design note. Python 3.11 standard library
only.
"""
import re

# ---------------------------------------------------------------- ffprobe

# Options that take a value. `-of`/`-print_format` take `WRITER[=OPTIONS]`.
FFPROBE_VALUE = {
    '-i', '-v', '-loglevel', '-of', '-print_format', '-output_format',
    '-show_entries', '-select_streams', '-read_intervals', '-f',
}
FFPROBE_FLAG = {
    '-show_format', '-show_streams', '-show_chapters', '-show_programs',
    '-show_packets', '-show_error', '-count_packets',
    '-hide_banner', '-pretty', '-sexagesimal', '-nostdin',
}
# The output writers step 1 prints. `xml` and `ini` are not among them.
FFPROBE_WRITERS = {'json', 'default', 'compact', 'csv', 'flat'}
# Demuxer names step 1 reads, as FFmpeg spells them.
FFPROBE_FORMATS = {'mov,mp4,m4a,3gp,3g2,mj2', 'matroska,webm', 'mpegts'}
# `-show_entries SECTION=FIELD,...`: the sections step 1 prints. Anything
# per-frame needs a decoder, which is why `-show_frames` is refused too.
FFPROBE_SECTIONS = {'format', 'format_tags', 'stream', 'stream_tags',
                    'stream_side_data', 'packet', 'packet_tags',
                    'chapter', 'chapter_tags', 'program', 'program_tags'}
# Fields FFmpeg only fills after decoding every frame (`-count_frames`).
DECODED_FIELDS = {'nb_read_frames'}

# --------------------------------------------------------------- ffmpeg

# Every option that names a codec. Step 1 copies; anything else encodes.
FFMPEG_CODEC = ('-c', '-codec', '-vcodec', '-acodec', '-scodec')
FFMPEG_VALUE = {
    '-i', '-ss', '-sseof', '-t', '-to', '-map', '-map_metadata', '-map_chapters',
    '-movflags', '-f', '-loglevel', '-v', '-segment_time', '-segment_format',
    '-segment_list', '-hls_time', '-hls_list_size', '-hls_segment_filename',
    '-hls_playlist_type', '-safe', '-fflags', '-avoid_negative_ts',
    '-reset_timestamps', '-max_muxing_queue_size', '-threads',
}
FFMPEG_FLAG = {
    '-y', '-n', '-an', '-vn', '-sn', '-dn', '-hide_banner', '-nostdin',
    '-copyts', '-start_at_zero', '-ignore_unknown', '-shortest',
}
# `-f` before the first `-i` names the demuxer of the input; after it, the
# muxer of the output. CoreAV reads more containers than it writes, so the
# two sets differ: writing Matroska or MPEG-TS is not step 1's.
# `-f mp4` and the rest are the aliases FFmpeg matches against the
# comma-separated demuxer name. MPEG-TS is not here: `coreav probe` reads
# transport streams, but no writer of step 1 does.
FFMPEG_DEMUXERS = {'concat', 'mov,mp4,m4a,3gp,3g2,mj2', 'mp4', 'mov', 'm4a', '3gp',
                   'matroska,webm', 'matroska', 'webm'}
FFMPEG_MUXERS = {'mp4', 'ipod', 'hls', 'segment'}
# Options whose value is a file name, so that `normalize` can replace it.
FFMPEG_FILE_VALUE = {'-i', '-hls_segment_filename', '-segment_list'}

# A value that is not a file this reads: a device, a pipe, a stream
# protocol, a filter graph. Files and HTTP(S) URLs only.
NOT_A_FILE = ('pipe:', '-', 'rtmp://', 'rtsp://', 'rtp://', 'udp://', 'srt://',
              'tcp://', 'ftp://', 'sftp://', 'smb://', 'data:', 'concat:',
              'async:', 'cache:', 'crypto:', 'subfile:', 'hw:', '/dev/',
              'video=', 'audio=', 'title=', 'desktop', 'default', 'lavfi',
              'movie=', 'anullsrc', 'testsrc', 'color=')
# A number pattern in a name: an image sequence on the way in, and segment
# numbering on the way out.
NUMBERED = re.compile(r'%\d*d')
# A value the shell never expanded, so the command cannot be run as it
# stands: `$x`, `${x}`, `%x%`, `<x>`.
UNEXPANDED = re.compile(r'\$\w|\$\{|%\w+%|<[^>]*>')

# The containers CoreAV reads, and the ones it writes (`coreav remux`, `clip`, `split`, `extract-audio` and `hls` write MP4, M4A
# and HLS). A name with no extension is a placeholder in an answer, not a
# statement about the container: the measurement substitutes a corpus file
# for it, so it does not decide the label.
READ_EXT = {'mp4', 'm4v', 'm4a', 'm4b', 'mov', 'qt', 'mkv', 'mka', 'mks',
            'webm', 'ts', 'm2ts', 'mts', '3gp', '3g2'}
WRITE_EXT = {'mp4', 'm4a', 'm4v', 'm3u8'}
# The input of `-f concat` is a list of files, not media.
LIST_EXT = {'txt', 'list', 'ffcat', 'concat'}


def extension(name):
    """The lower-case extension of a file name or URL, or '' when it has
    none this can read."""
    name = name.split('?', 1)[0].split('#', 1)[0].rstrip('/')
    tail = name.rsplit('/', 1)[-1].rsplit('\\', 1)[-1]
    if '.' not in tail[1:]:
        return ''
    ext = tail.rsplit('.', 1)[1].lower()
    return ext if ext.isalnum() else ''


# FFmpeg's time syntax: `[-][HH:]MM:SS[.m...]`, with minutes and seconds
# below 60, or `[-]S+[.m...][s|ms|us]`.
CLOCK = re.compile(r'\A-?(?:(\d+):)?([0-5]?\d):([0-5]?\d)(\.\d+)?\Z')
SECONDS = re.compile(r'\A-?\d+(\.\d+)?(s|ms|us)?\Z')
TIME_OPTIONS = ('-ss', '-sseof', '-t', '-to', '-segment_time', '-hls_time')


def is_time(value):
    return bool(CLOCK.match(value) or SECONDS.match(value))


def is_option(token):
    return token.startswith('-') and token != '-' and not token[1:2].isdigit()


def codec_of(token):
    """The codec option's base when `token` is one (`-c:v` -> `-c`)."""
    base = token.split(':', 1)[0]
    return base if base in FFMPEG_CODEC else None


def value_options(tool):
    return FFPROBE_VALUE if tool == 'ffprobe' else FFMPEG_VALUE | set(FFMPEG_CODEC)


def walk(args, options_with_values):
    """[(option, value)] for every option, and the bare tokens, in order."""
    options, bare, i = [], [], 0
    while i < len(args):
        token = args[i]
        if is_option(token):
            base = token.split(':', 1)[0]
            takes = base in options_with_values or token in options_with_values
            if '=' in token and not takes:
                options.append((token.split('=', 1)[0], token.split('=', 1)[1]))
            elif takes and i + 1 < len(args):
                options.append((token, args[i + 1]))
                i += 1
            else:
                options.append((token, None))
        else:
            bare.append(token)
        i += 1
    return options, bare


def files(tool, args):
    """(inputs, outputs) of an invocation."""
    options, bare = walk(list(args), value_options(tool))
    inputs = [v for o, v in options if o == '-i' and v]
    if tool == 'ffprobe':
        return inputs + bare, []
    return inputs, [token for token in bare if token not in inputs]


# Somebody's home directory in a path, which names a person. The file
# names themselves are replaced; this catches a path that survives as the
# value of an option the classifier does not know.
HOME = re.compile(r'(?i)(/home/|/Users/|[A-Z]:\\Users\\)[^/\\\s]+')


def scrub(token):
    """A token with any home directory in it made anonymous."""
    return HOME.sub(lambda m: m.group(1) + 'USER', token)


def normalize(tool, args):
    """The argument list with every file name replaced by a placeholder.

    What a command-coverage set measures is which options are combined;
    the names in an answer are examples (`input.mp4`, `$f`, a path on
    someone's disk) and every run substitutes a corpus file for each input
    anyway. Keeping the shape alone also keeps the committed set clear of
    whatever a file name in a post happened to hold.

    Only the positions that are file names are replaced: the value of
    `-i` and of the options that name a file, and the last bare token,
    which is FFmpeg's output. A bare token anywhere else belongs to an
    option this does not know, and keeps its own text so that the record
    stays the invocation it was.
    """
    args = list(args)
    takes_file = FFMPEG_FILE_VALUE if tool == 'ffmpeg' else {'-i'}
    inputs = [v for o, v in walk(args, value_options(tool))[0] if o == '-i' and v]
    names, out, i = {}, [], 0
    for n, name in enumerate(dict.fromkeys(inputs)):
        names[name] = 'INPUT' if n == 0 else f'INPUT{n + 1}'
    bare = [n for n, token in enumerate(args)
            if not is_option(token) and not _is_value(args, n, tool)]
    # FFmpeg's output and ffprobe's input are the last bare argument. A
    # bare token before it belongs to an option this does not know, and
    # keeps its own text (scrubbed of a home directory) rather than being
    # called a file.
    last = bare[-1] if bare else None
    while i < len(args):
        token = args[i]
        base = token.split(':', 1)[0]
        if (base in takes_file or token in takes_file) and i + 1 < len(args) \
                and not is_option(args[i + 1]):
            out.extend([token, names.get(args[i + 1], 'FILE')])
            i += 2
            continue
        if is_option(token):
            takes = base in value_options(tool) or token in value_options(tool)
            if takes and i + 1 < len(args):
                out.extend([token, args[i + 1]])
                i += 2
                continue
            out.append(token)
            i += 1
            continue
        if i == last and token not in names:
            out.append('OUTPUT' if tool == 'ffmpeg' else 'INPUT')
        else:
            out.append(names.get(token, scrub(token)))
        i += 1
    return [scrub(token) for token in out]


def _is_value(args, index, tool):
    """The token at `index` is the value of the option before it."""
    if index == 0:
        return False
    previous = args[index - 1]
    if not is_option(previous):
        return False
    base = previous.split(':', 1)[0]
    return (base in value_options(tool) or previous in value_options(tool)) \
        and '=' not in previous


def truncated(args, options_with_values):
    """The extraction cut the line short: it ends in an option that needs
    a value. Such a record is not a job and is outside both bars."""
    last = args[-1] if args else None
    return not args or (last in options_with_values
                        or last.split(':', 1)[0] in options_with_values)


def unexpanded_value(options):
    """An option value the shell never expanded. A file name may be a
    placeholder — the run substitutes one — but a time, a map or a writer
    option cannot be."""
    for option, value in options:
        if option == '-i' or value is None:
            continue
        if UNEXPANDED.search(value):
            return f'{option} {value}'
    return None


def unknown_entry(value):
    """`-show_entries SECTION=FIELD,...:SECTION=...`: the first section or
    field that is not step 1's, or None."""
    for part in (value or '').split(':'):
        if not part:
            continue
        section, _, fields = part.partition('=')
        if section.strip() not in FFPROBE_SECTIONS:
            return section.strip()
        for field in fields.split(','):
            if field.strip() in DECODED_FIELDS:
                return field.strip()
    return None


def label_ffprobe(args):
    if truncated(args, FFPROBE_VALUE):
        return 'unusable', 'the line was cut short'
    options, bare = walk(args, FFPROBE_VALUE)
    stuck = unexpanded_value(options)
    if stuck:
        return 'unusable', f'an unexpanded value: {stuck}'
    # The options first: an option this does not know takes its value for a
    # file name, so the file counts would blame the wrong thing.
    for option, value in options:
        base = option.split(':', 1)[0]
        if base in ('-of', '-print_format', '-output_format'):
            writer, _, settings = (value or '').partition('=')
            if writer.strip() not in FFPROBE_WRITERS:
                return 'out-of-scope', f'{option} {writer.strip()}'
            # Each setting is one `key=value`; FFmpeg refuses anything
            # else, and so does this.
            for setting in settings.split(':'):
                if setting and setting.count('=') != 1:
                    return 'out-of-scope', f'{option} {value}'
            continue
        if base == '-f':
            if (value or '') not in FFPROBE_FORMATS:
                return 'out-of-scope', f'-f {value}'
            continue
        if base == '-show_entries':
            unknown = unknown_entry(value)
            if unknown is not None:
                return 'out-of-scope', f'-show_entries {unknown}'
            continue
        if base in FFPROBE_VALUE or base in FFPROBE_FLAG:
            continue
        return 'out-of-scope', option
    inputs = [v for o, v in options if o == '-i' and v] + bare
    if not inputs:
        return 'out-of-scope', 'no input file'
    if len(inputs) > 1:
        return 'out-of-scope', 'more than one input'
    for name in inputs:
        if name.startswith(NOT_A_FILE) or NUMBERED.search(name):
            return 'out-of-scope', f'input {name}'
        ext = extension(name)
        if ext and ext not in READ_EXT:
            return 'out-of-scope', f'input container .{ext}'
    return 'native', ''


def label_ffmpeg(args):
    if truncated(args, FFMPEG_VALUE | set(FFMPEG_CODEC)):
        return 'unusable', 'the line was cut short'
    options, bare = walk(args, FFMPEG_VALUE | set(FFMPEG_CODEC))
    stuck = unexpanded_value(options)
    if stuck:
        return 'unusable', f'an unexpanded value: {stuck}'
    copying, numbered_output, seen_input = False, False, False
    # The options first, as above.
    for option, value in options:
        codec = codec_of(option)
        if codec:
            if (value or '').strip() != 'copy':
                return 'out-of-scope', f'{option} {value}'
            copying = True
            continue
        base = option.split(':', 1)[0]
        if base == '-i':
            seen_input = True
            continue
        if base == '-f':
            allowed = FFMPEG_MUXERS if seen_input else FFMPEG_DEMUXERS
            if (value or '') not in allowed:
                return 'out-of-scope', f'-f {value}'
            numbered_output = numbered_output or value == 'segment'
            continue
        if base == '-hls_segment_filename':
            numbered_output = True
            continue
        if base in TIME_OPTIONS and value is not None and not is_time(value):
            return 'out-of-scope', f'{option} {value}'
        if base in FFMPEG_VALUE or base in FFMPEG_FLAG or option in FFMPEG_FLAG:
            continue
        return 'out-of-scope', option
    if not copying:
        # No codec option at all means FFmpeg picks an encoder from the
        # output's format: a transcode, whatever the input was.
        return 'out-of-scope', 'no -c copy'
    inputs = [v for o, v in options if o == '-i' and v]
    outputs = [token for token in bare if token not in inputs]
    if not inputs:
        return 'out-of-scope', 'no input file'
    if not outputs:
        return 'out-of-scope', 'no output file'
    if len(inputs) > 1:
        return 'out-of-scope', 'more than one input'
    if len(outputs) > 1:
        return 'out-of-scope', 'more than one output'
    for name in inputs + outputs:
        if name.startswith(NOT_A_FILE):
            return 'out-of-scope', f'file {name}'
    if NUMBERED.search(inputs[0]):
        return 'out-of-scope', f'input {inputs[0]}'
    # A number pattern in the output is segment numbering, which only the
    # segment muxer writes; anywhere else it is an image sequence.
    if NUMBERED.search(outputs[0]) and not numbered_output:
        return 'out-of-scope', f'output {outputs[0]}'
    concat = any(o == '-f' and v == 'concat' for o, v in options)
    ext = extension(inputs[0])
    allowed = LIST_EXT if concat else READ_EXT
    if ext and ext not in allowed:
        return 'out-of-scope', f'input container .{ext}'
    ext = extension(outputs[0])
    if ext and ext not in WRITE_EXT:
        return 'out-of-scope', f'output container .{ext}'
    return 'native', ''


def label(tool, args):
    if tool == 'ffprobe':
        return label_ffprobe(list(args))
    if tool == 'ffmpeg':
        return label_ffmpeg(list(args))
    raise ValueError(f'classify: unknown tool {tool!r}')
