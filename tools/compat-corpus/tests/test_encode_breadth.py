"""encode_breadth.py, V2 and V3 of the 0.1.0 codec coverage bars: the order
in which inputs are tried, the commands' jobs, the encoder-start rule, each
miss code and its order, the oracles' thresholds and the result. The programs
are stubs the tests write; no media is made or read.

    python3 -I -m unittest discover -s tools/compat-corpus/tests -p 'test_encode_breadth.py'
"""
import contextlib
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import encode_breadth as eb  # noqa: E402
import r3  # noqa: E402
import release_round  # noqa: E402

MAPPING = 'Stream mapping:\n  Stream #0:0 -> #0:0 (copy)\n  Stream #0:1 -> #0:1 (copy)\n'
LISTING = [['video', 'h264'], ['audio', 'aac']]
AAC_START = ['-nostdin', '-hide_banner', '-loglevel', 'error', '-f', 'f32le', '-ar', '44100', '-ac', '1', '-i',
             'pipe:0', '-map', '0:a:0', '-c:a', 'aac', '-f', 'matroska', 'pipe:1']
DECODE_START = ['-nostdin', '-v', 'error', '-i', '/tmp/in.mp4', '-map', '0:v:0', '-f', 'rawvideo', 'pipe:1']
SAMPLE_MAPPING = '''Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'in.mp4':
Stream mapping:
  Stream #0:1[0x101] -> #0:0 (aac (native) -> aac (native))
  Stream #0:0 (libdav1d) -> #0:1 (av1 (libdav1d) -> h264 (libx264))
  Stream #0:2 -> #0:2 (aac (native) -> opus (libopus))
  Stream #0:3 -> #0:3 (h264 (native) -> mjpeg (native))
  Stream #0:4 -> #0:4 (copy)
  Stream #1:0 -> #0:5 (h264 (native) -> wrapped_avframe (native))
Press [q] to stop, [?] for help
'''

FFMPEG = '''#!{py}
import json, os, pathlib, sys
cfg = json.load(open(os.environ['FAKE_CONFIG']))
args = sys.argv[1:]
with open(cfg['argv_log'], 'a') as f:
    f.write(json.dumps(args) + chr(10))
name = pathlib.Path(args[args.index('-i') + 1]).name if '-i' in args else ''
if cfg['accept'] is not None and name not in cfg['accept']:
    sys.stderr.write('Invalid data found when processing input' + chr(10))
    sys.exit(1)
sys.stderr.write(cfg['mapping'])
out = args[-1]
if out != '-' and not out.startswith('/dev/'):
    pathlib.Path(out.replace('%03d', '001')).write_text(json.dumps(cfg['listing']))
'''
FFPROBE = '''#!{py}
import json, pathlib, sys
try:
    listing = json.loads(pathlib.Path(sys.argv[-1]).read_text())
except (OSError, ValueError):
    sys.exit(1)
print(json.dumps({'streams': [{'index': i, 'codec_type': t, 'codec_name': c} for i, (t, c) in enumerate(listing)]}))
'''
COREAV = '''#!{py}
import json, os, pathlib, resource, signal, subprocess, sys, time
cfg = json.load(open(os.environ['FAKE_CONFIG']))['coreav']
args = sys.argv[2:]
time.sleep(cfg.get('sleep', 0))
if cfg.get('writes', True) and cfg['exit'] == 0:
    pathlib.Path(args[-1]).write_text(json.dumps(cfg['listing']))
if cfg.get('fallback'):
    with open(os.environ['COREAV_FALLBACK_LOG'], 'a') as f:
        f.write(json.dumps(cfg['fallback']) + chr(10))
if cfg.get('starts'):
    signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
    resource.setrlimit(resource.RLIMIT_FSIZE, (0, 0))
for program, extra in cfg.get('starts', []):
    subprocess.run([os.environ['COREAV_' + program.upper()]] + extra)
sys.stderr.write(cfg.get('stderr', ''))
if cfg.get('signal'):
    os.kill(os.getpid(), cfg['signal'])
sys.exit(cfg['exit'])
'''
X264 = '''#!{py}
import json, os
open(json.load(open(os.environ['FAKE_CONFIG']))['x264_marker'], 'a').close()
'''
HARNESS = '''#!{py}
import json, os, pathlib
cfg = json.load(open(os.environ['FAKE_CONFIG']))['harness']
records = []
for pair in json.load(open(os.environ['COREAV_R3_PAIRS'])):
    work = pathlib.Path(pair['work'])
    if cfg['status'] == 'written':
        pathlib.Path(pair['args'][-1]).write_text(cfg['bytes'])
    records.append({**pair, 'status': cfg['status'], 'detail': cfg['detail'], 'chosen': cfg['chosen'],
                    'files': len(list(work.iterdir()))})
json.dump(records, open(os.environ['COREAV_R3_RECORDS'], 'w'))
'''
CHECK = '''#!{py}
import collections, json, os, sys
cfg = json.load(open(os.environ['FAKE_CONFIG']))['check']
args = dict(zip(sys.argv[1::2], sys.argv[2::2]))
records = json.load(open(args['--records']))
failures = [{'id': r['id'], 'args': r['args'], 'source': r['source'], 'selection': None, 'problems': ['p']}
            for r in records if cfg == 'fail']
if cfg == 'crash':
    sys.exit(1)
json.dump({'counts': {r['id']: {'written': 1} for r in records}, 'failures': failures}, open(args['--report'], 'w'))
'''


def entry(args, output='OUTPUT.mp4', cls='media', container='MP4'):
    return {'args': args, 'class': cls, 'container': container, 'muxer': None, 'output': output}


class Placeholders(unittest.TestCase):
    def test_the_first_placeholder_changes_slowest(self):
        got = eb.assignments(['-i', 'INPUT', '-i', 'INPUT2'])
        self.assertEqual(len(got), 25)
        self.assertEqual(got[:6], [('basic.mp4', 'basic.mp4'), ('basic.mp4', 'mkv_h264_aac.mkv'),
                                   ('basic.mp4', 'webm_vp9_opus.webm'), ('basic.mp4', 'ts_h264_aac.ts'),
                                   ('basic.mp4', 'audio_aac.m4a'), ('mkv_h264_aac.mkv', 'basic.mp4')])
        self.assertEqual(got[-1], ('audio_aac.m4a', 'audio_aac.m4a'))

    def test_one_placeholder_follows_the_notes_order_and_none_is_one_empty_assignment(self):
        self.assertEqual([a for (a,) in eb.assignments(['-i', 'INPUT'])], list(eb.FIXTURES))
        self.assertEqual(eb.assignments(['-codecs']), [()])
        self.assertEqual(len(eb.assignments(['-i', 'INPUT', '-i', 'INPUT2', '-i', 'INPUT3'])), 125)

    def test_the_placeholder_that_stands_first_is_first_whatever_its_name(self):
        args = ['-i', 'INPUT2', '-i', 'INPUT']
        self.assertEqual(eb.placeholders(args), ['INPUT2', 'INPUT'])
        self.assertEqual(eb.placeholders(['-i', 'INPUT', 'FILE', '-map', 'ALL', 'INPUT', 'OUTPUT']), ['INPUT'])

    def test_every_placeholder_of_the_set_is_one_the_runner_fills(self):
        found = {a for _, e in eb.commands('v3') for a in e['args'] if a.startswith('INPUT')}
        self.assertEqual(found, {'INPUT', 'INPUT2', 'INPUT3'})
        self.assertEqual((len(eb.commands('v3')), len(eb.commands('v2'))), (630, 488))


class Staging(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.fixtures = self.dir / 'fixtures'
        self.fixtures.mkdir()
        for name in eb.FIXTURES:
            (self.fixtures / name).write_bytes(name.encode())

    def tearDown(self):
        self.tmp.cleanup()

    def test_a_concat_input_is_a_list_naming_the_fixture_twice_beside_it(self):
        for args in (['-f', 'concat', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'],
                     ['-safe', '0', '-f', 'concat', '-i', 'INPUT', '-c', 'copy', 'OUTPUT']):
            paths = eb.stage(args, ('mkv_h264_aac.mkv',), self.fixtures, self.dir / ''.join(args[:2]))
            listing = pathlib.Path(paths['INPUT'])
            self.assertEqual(listing.read_text(), "file 'mkv_h264_aac.mkv'\nfile 'mkv_h264_aac.mkv'\n")
            self.assertEqual((listing.parent / 'mkv_h264_aac.mkv').read_bytes(), b'mkv_h264_aac.mkv')

    def test_only_the_input_of_concat_is_a_list_and_inputs_are_copies(self):
        args = ['-f', 'concat', '-i', 'INPUT', '-i', 'INPUT2', 'OUTPUT']
        paths = eb.stage(args, ('basic.mp4', 'audio_aac.m4a'), self.fixtures, self.dir / 'in')
        self.assertTrue(paths['INPUT'].endswith('list.txt'))
        self.assertTrue(paths['INPUT2'].endswith('audio_aac.m4a'))
        pathlib.Path(paths['INPUT2']).write_bytes(b'changed')
        self.assertEqual((self.fixtures / 'audio_aac.m4a').read_bytes(), b'audio_aac.m4a')

    def test_the_other_upper_case_words_are_the_commands_own(self):
        paths = {'INPUT': '/in/a.mp4'}
        self.assertEqual(eb.fill(['-i', 'INPUT', '-vf', 'ALL', 'OUTPUT'], paths, 'OUTPUT%03d.jpg'),
                         ['-i', '/in/a.mp4', '-vf', 'ALL', 'OUTPUT%03d.jpg'])


class Mapping(unittest.TestCase):
    def test_the_log_level_of_the_command_is_replaced_by_info(self):
        self.assertEqual(eb.info(['-v', 'quiet', '-i', 'a', 'o']), ['-v', 'info', '-i', 'a', 'o'])
        self.assertEqual(eb.info(['-i', 'a', '-loglevel', 'error', 'o']), ['-i', 'a', '-loglevel', 'info', 'o'])
        self.assertEqual(eb.info(['-i', 'a', 'o']), ['-i', 'a', 'o'])

    def test_each_output_stream_gets_the_job_its_mapping_names(self):
        found = eb.jobs(SAMPLE_MAPPING)
        self.assertEqual([(j.output, j.file, j.stream, j.kind) for j in found],
                         [(0, 0, 1, 'aac'), (1, 0, 0, 'libx264'), (2, 0, 2, 'libopus'), (3, 0, 3, 'mjpeg'),
                          (4, 0, 4, 'copy'), (5, 1, 0, 'wrapped_avframe')])

    def test_a_copy_mapping_is_all_copy_and_the_groups_follow_the_encoders(self):
        jobs = lambda text: eb.jobs(text)  # noqa: E731
        self.assertEqual(eb.groups(jobs(MAPPING)), ['copy only'])
        self.assertEqual(eb.groups(jobs(SAMPLE_MAPPING)), ['x264', 'libopus', 'aac', 'other encoders'])
        self.assertEqual(eb.groups(jobs('  Stream #0:0 -> #0:0 (h264 (native) -> h264 (libx264))\n'
                                         '  Stream #0:1 -> #0:1 (copy)\n')), ['x264'])
        self.assertEqual(eb.groups([]), [])

    def test_the_timing_options_are_those_before_the_input_and_after_it(self):
        args = ['-ss', '1', '-itsoffset', '2', '-i', 'a', '-t', '3', '-c', 'copy', '-to', '4', 'o']
        self.assertEqual(eb.timing(args), (['-ss', '1', '-itsoffset', '2'], ['-t', '3', '-to', '4']))
        self.assertEqual(eb.timing(['-codecs']), ([], []))


class EncoderStart(unittest.TestCase):
    def test_the_aac_encode_of_coreav_is_an_encoder_start(self):
        self.assertTrue(eb.aac_encoder_start(AAC_START))
        self.assertTrue(eb.aac_encoder_start(AAC_START[:-3] + ['-b:a', '64000'] + AAC_START[-3:]))
        self.assertTrue(eb.aac_encoder_start(['-f', 's16le', '-i', '-', '-acodec', 'aac', 'out.m4a']))

    def test_a_decoding_start_is_not(self):
        self.assertFalse(eb.aac_encoder_start(DECODE_START))
        self.assertFalse(eb.aac_encoder_start(['-hide_banner', '-loglevel', 'error', '-h', 'encoder=aac']))

    def test_the_whole_invocation_is_judged_not_the_presence_of_its_tokens(self):
        bad = {
            'a file input': ['-f', 'f32le', '-i', 'in.raw', '-c:a', 'aac', 'o'],
            'no raw format': ['-i', 'pipe:0', '-c:a', 'aac', 'o'],
            'a container for the input': ['-f', 'matroska', '-i', 'pipe:0', '-c:a', 'aac', 'o'],
            'another encoder': ['-f', 'f32le', '-i', 'pipe:0', '-c:a', 'libopus', 'o'],
            'no encoder named': ['-f', 'f32le', '-i', 'pipe:0', 'o'],
            'a second input': ['-f', 'f32le', '-i', 'pipe:0', '-i', 'b.mp4', '-c:a', 'aac', 'o'],
            'a second output': ['-f', 'f32le', '-i', 'pipe:0', '-c:a', 'aac', 'o', 'p'],
            'no output': ['-f', 'f32le', '-i', 'pipe:0', '-c:a', 'aac'],
            'video encoded too': ['-f', 'f32le', '-i', 'pipe:0', '-c:a', 'aac', '-c:v', 'libx264', 'o'],
            'a map that takes video': ['-f', 'f32le', '-i', 'pipe:0', '-map', '0:v:0', '-c:a', 'aac', 'o'],
            'an option without its value': ['-f', 'f32le', '-i'],
        }
        for why, args in bad.items():
            self.assertFalse(eb.aac_encoder_start(args), why)

    def test_x264_always_starts_an_encoder_and_other_programs_do_not(self):
        self.assertTrue(eb.encoder_start('x264', ['--demuxer', 'raw']))
        self.assertTrue(eb.encoder_start('ffmpeg', AAC_START))
        self.assertFalse(eb.encoder_start('ffmpeg', DECODE_START))
        self.assertFalse(eb.encoder_start(None, AAC_START))


class Stubs(unittest.TestCase):
    """Stand-ins for the five programs, run as the runner runs the real ones."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        d = self.dir = pathlib.Path(self.tmp.name)
        for name, text in (('ffmpeg', FFMPEG), ('ffprobe', FFPROBE), ('coreav', COREAV), ('x264', X264),
                           ('harness', HARNESS)):
            (d / name).write_text(text.replace('{py}', sys.executable))
            (d / name).chmod(0o755)
        (d / 'copy_check.py').write_text(CHECK.replace('{py}', sys.executable))
        self.fixtures = d / 'fixtures'
        self.fixtures.mkdir()
        for name in eb.FIXTURES:
            (self.fixtures / name).write_bytes(b'x')
        self.config = {'argv_log': str(d / 'argv.log'), 'x264_marker': str(d / 'x264.ran'), 'accept': None,
                       'mapping': MAPPING, 'listing': LISTING,
                       'coreav': {'exit': 0, 'listing': LISTING},
                       'harness': {'status': 'written', 'chosen': 'remux', 'detail': {}, 'bytes': 'same'},
                       'check': 'pass'}
        patch = mock.patch.dict(os.environ, {'FAKE_CONFIG': str(d / 'config.json')})
        patch.start()
        self.addCleanup(patch.stop)
        here, r3.HERE = r3.HERE, d
        self.addCleanup(setattr, r3, 'HERE', here)
        self.opts = types.SimpleNamespace(
            coreav=str(d / 'coreav'), harness=str(d / 'harness'), ffmpeg=str(d / 'ffmpeg'),
            ffprobe=str(d / 'ffprobe'), x264=str(d / 'x264'), mediainfo=None, fixtures=self.fixtures,
            work=d / 'work', jobs=1, env=eb.environment())
        self.opts.work.mkdir()
        self.opts.tc = types.SimpleNamespace(ffmpeg=self.opts.ffmpeg, ffprobe=self.opts.ffprobe,
                                             coreav=self.opts.coreav, decode_oracle=None)
        self.opts.check = types.SimpleNamespace(ffmpeg=self.opts.ffmpeg, ffprobe=self.opts.ffprobe, mediainfo=None,
                                                log=str(d / 'check.log'))

    def tearDown(self):
        self.tmp.cleanup()

    def coreav(self, **settings):
        self.config['coreav'] = {'exit': 0, 'listing': LISTING, **settings}

    def evaluate(self, e, index=0):
        (self.dir / 'config.json').write_text(json.dumps(self.config))
        return eb.evaluate(self.opts, index, e)

    def attempts(self):
        log = self.dir / 'argv.log'
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    COPY = entry(['-i', 'INPUT', '-c', 'copy', 'OUTPUT'])


class Inputs(Stubs):
    def test_the_first_assignment_ffmpeg_runs_is_the_input(self):
        self.config['accept'] = ['mkv_h264_aac.mkv', 'audio_aac.m4a']
        with mock.patch.object(eb, 'oracle', return_value=None):
            record = self.evaluate(entry(['-i', 'INPUT', '-i', 'INPUT2', '-c', 'copy', 'OUTPUT']))
        self.assertEqual(record['inputs'], ['mkv_h264_aac.mkv', 'basic.mp4'])
        tried = [pathlib.Path(a[a.index('-i') + 1]).name for a in self.attempts()[:6]]
        self.assertEqual(tried, ['basic.mp4'] * 5 + ['mkv_h264_aac.mkv'])
        self.assertTrue(record['counted'])

    def test_no_assignment_is_no_input(self):
        self.config['accept'] = []
        record = self.evaluate(self.COPY)
        self.assertEqual((record['miss'], record['counted']), ('no_input', False))
        self.assertEqual(len(self.attempts()), 5)

    def test_an_address_or_a_device_or_a_command_without_output_is_no_input_and_never_run(self):
        for e in (entry(['-i', 'INPUT', '-f', 'flv', 'OUTPUT'], output='rtmp://OUTPUT'),
                  entry(['-i', 'INPUT', '-f', 'v4l2', 'OUTPUT'], output='/dev/OUTPUT'),
                  entry(['-codecs'], output='', cls='unknown')):
            self.assertEqual(self.evaluate(e)['miss'], 'no_input', e)
        self.assertEqual(self.attempts(), [])

    def test_a_command_without_an_input_placeholder_runs_once_as_it_is(self):
        with mock.patch.object(eb, 'oracle', return_value=None):
            record = self.evaluate(entry(['-f', 'lavfi', '-i', 'testsrc', '-t', '1', 'OUTPUT']))
        self.assertEqual((record['inputs'], len(self.attempts())), ([], 2))

    def test_a_pipe_is_captured_and_succeeds_on_exit_0_alone(self):
        e = entry(['-i', 'INPUT', '-f', 'null', 'OUTPUT'], output='-', cls='decode-only')
        record = self.evaluate(e)
        self.assertEqual((record['miss'], record['detail']), ('no_oracle', {'job': 'decode-only'}))

    def test_a_number_pattern_needs_one_file_that_matches(self):
        self.assertEqual(eb.produced('OUTPUT%03d.jpg', self.dir), [])
        (self.dir / 'OUTPUT001.jpg').write_text('x')
        (self.dir / 'OUTPUTx.jpg').write_text('x')
        self.assertEqual([p.name for p in eb.produced('OUTPUT%03d.jpg', self.dir)], ['OUTPUT001.jpg'])


class Misses(Stubs):
    def test_a_refusal_keeps_the_option_the_log_names_and_none_without_a_line(self):
        self.coreav(exit=4, fallback={'reason': '-vf', 'ran': None, 'not_run': 'fallback_unusable'})
        record = self.evaluate(self.COPY)
        self.assertEqual((record['miss'], record['detail'], record['void']), ('refused', {'reason': '-vf'}, False))
        self.coreav(exit=4)
        self.assertEqual(self.evaluate(self.COPY)['detail'], {'reason': 'none'})

    def test_a_truncated_or_invalid_structure_is_rejected_with_its_kind(self):
        for text, kind in (('ffmpeg: a.mp4: truncated structure at 12', 'truncated_structure'),
                           ('ffmpeg: a.mp4: invalid structure: box', 'invalid_structure')):
            self.coreav(exit=1, stderr=text)
            record = self.evaluate(self.COPY)
            self.assertEqual((record['miss'], record['detail']), ('rejected', {'kind': kind}))
        self.coreav(exit=1, stderr='ffmpeg: a.mp4: some other failure')
        self.assertEqual(self.evaluate(self.COPY)['miss'], 'failed')

    def test_a_run_the_runner_stopped_is_a_timeout_ahead_of_failed(self):
        self.coreav(exit=0, sleep=30)
        with mock.patch.object(eb, 'COREAV_SECONDS', 1):
            record = self.evaluate(self.COPY)
        self.assertEqual(record['miss'], 'timeout')

    def test_any_other_status_a_signal_or_no_output_is_failed(self):
        for settings in ({'exit': 3, 'stderr': 'ffmpeg: write failed\n'}, {'exit': 0, 'writes': False},
                         {'exit': 0, 'signal': 11}):
            self.coreav(**settings)
            record = self.evaluate(self.COPY)
            self.assertEqual(record['miss'], 'failed', settings)
        self.coreav(exit=3, stderr='first\nffmpeg: write failed in $WORK\n')
        self.assertEqual(self.evaluate(self.COPY)['detail'], {'status': 3, 'stderr': 'ffmpeg: write failed in $WORK'})

    def test_a_start_that_is_not_an_encoder_is_ffmpeg_job_ahead_of_no_oracle(self):
        self.config['mapping'] = 'Stream mapping:\n  Stream #0:0 -> #0:0 (h264 (native) -> mpeg4 (native))\n'
        self.coreav(exit=0, starts=[['x264', ['--demuxer', 'raw']], ['ffmpeg', DECODE_START]])
        record = self.evaluate(self.COPY)
        self.assertEqual(record['miss'], 'ffmpeg_job')
        self.assertEqual([s['encoder'] for s in record['starts']], [True, False])

    def test_an_aac_encoder_start_is_not_ffmpeg_job(self):
        self.config['mapping'] = 'Stream mapping:\n  Stream #0:0 -> #0:0 (h264 (native) -> mpeg4 (native))\n'
        self.coreav(exit=0, starts=[['ffmpeg', AAC_START]])
        record = self.evaluate(self.COPY)
        self.assertEqual((record['miss'], record['detail']), ('no_oracle', {'job': 'mpeg4'}))

    def test_a_job_with_no_oracle_is_no_oracle_and_names_it(self):
        self.config['mapping'] = ('Stream mapping:\n  Stream #0:0 -> #0:0 (copy)\n'
                                  '  Stream #0:1 -> #0:1 (aac (native) -> mp3 (libmp3lame))\n')
        record = self.evaluate(self.COPY)
        self.assertEqual((record['miss'], record['detail']), ('no_oracle', {'job': 'libmp3lame'}))
        self.assertEqual(record['groups'], ['other encoders'])
        for cls in ('decode-only', 'subtitles', 'other'):
            record = self.evaluate(entry(['-i', 'INPUT', 'OUTPUT'], cls=cls, container=None))
            self.assertEqual((record['miss'], record['detail']), ('no_oracle', {'job': cls}))
        self.config['mapping'] = 'nothing\n'
        self.assertEqual(self.evaluate(self.COPY)['detail'], {'job': 'none'})

    def test_a_failed_check_is_oracle_with_the_check_and_a_stream_listing_difference_comes_last(self):
        with mock.patch.object(eb, 'copy_only', return_value=('oracle', {'check': 'B2'})):
            record = self.evaluate(self.COPY)
        self.assertEqual((record['miss'], record['detail']), ('oracle', {'check': 'B2'}))
        self.config['coreav'] = {'exit': 0, 'listing': [['video', 'h264']]}
        with mock.patch.object(eb, 'copy_only', return_value=('oracle', {'check': 'B2'})):
            self.assertEqual(self.evaluate(self.COPY)['miss'], 'oracle')
        with mock.patch.object(eb, 'copy_only', return_value=None):
            record = self.evaluate(self.COPY)
        self.assertEqual(record['miss'], 'streams_differ')
        self.assertEqual(record['detail'], {'coreav': [[('video', 'h264')]], 'ffmpeg': [[('video', 'h264'), ('audio', 'aac')]]})
        self.config['coreav'] = {'exit': 0, 'listing': LISTING}
        with mock.patch.object(eb, 'copy_only', return_value=None):
            record = self.evaluate(self.COPY)
        self.assertEqual((record['miss'], record['counted']), (None, True))

    def test_a_line_whose_ran_is_not_null_voids_the_run(self):
        self.coreav(exit=4, fallback={'reason': '-vf', 'ran': '/usr/bin/ffmpeg'})
        record = self.evaluate(self.COPY)
        self.assertEqual((record['miss'], record['void']), ('refused', True))
        summary = eb.summarize([record], 'v2')
        self.assertEqual((summary['void']['void'], summary['void']['fallback_ran']), (True, [0]))

    def test_a_failure_of_the_tools_is_a_fault_and_leaves_no_result(self):
        with mock.patch.object(eb, 'copy_only', side_effect=RuntimeError('the harness crashed')):
            record = self.evaluate(self.COPY)
        self.assertEqual((record['fault'], record['miss'], record['counted']), ('RuntimeError: the harness crashed', None, False))
        self.assertIn('a command has no result', eb.summarize([record], 'v2')['void']['reasons'])
        self.assertFalse(list(self.opts.work.iterdir()))


class Programs(Stubs):
    def test_the_wrapper_reports_each_start_and_runs_the_program_under_no_file_writes(self):
        self.coreav(exit=0, starts=[['x264', ['--demuxer', 'raw', '--output', '-']], ['ffmpeg', AAC_START]])
        self.config['mapping'] = 'Stream mapping:\n  Stream #0:0 -> #0:0 (h264 (native) -> mpeg4 (native))\n'
        with mock.patch.object(eb, 'oracle', return_value=None):
            record = self.evaluate(self.COPY)
        self.assertEqual([(s['program'], s['args'], s['encoder']) for s in record['starts']],
                         [('x264', ['--demuxer', 'raw', '--output', '-'], True), ('ffmpeg', AAC_START, True)])
        self.assertTrue((self.dir / 'x264.ran').exists())
        self.assertEqual(record['miss'], None)

    def test_the_run_has_no_fallback_program_and_a_log_and_the_pinned_runs_none(self):
        self.coreav(exit=4, fallback={'reason': 'x', 'ran': None})
        seen = {}
        original = eb.execute

        def spy(argv, cwd, env, seconds, pipe=False):
            seen.setdefault(str(argv[1]) if len(argv) > 1 else 'other', env)
            return original(argv, cwd, env, seconds, pipe)

        with mock.patch.object(eb, 'execute', spy):
            self.evaluate(self.COPY)
        env = seen['ffmpeg']
        self.assertEqual(env['COREAV_FALLBACK'], '/nonexistent/coreav-fallback')
        self.assertNotIn('COREAV_STRICT_SAFE', env)
        self.assertTrue(env['COREAV_X264'].endswith('programs/x264') and env['COREAV_FFMPEG'].endswith('programs/ffmpeg'))
        self.assertEqual(env['GCONV_PATH'], release_round.NO_GCONV)
        for key, value in seen.items():
            if key != 'ffmpeg':
                self.assertNotIn('COREAV_FALLBACK', value)
                self.assertEqual(value['GCONV_PATH'], release_round.NO_GCONV)


class Limits(unittest.TestCase):
    def test_a_program_cannot_write_a_file_past_the_limit_and_the_wall_time_ends_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            cwd = pathlib.Path(tmp)
            writer = 'import sys; f = open("big", "wb"); [f.write(bytes(1024)) or f.flush() for _ in range(4096)]'
            with mock.patch.object(eb, 'FILE_BLOCKS', 64):
                ran = eb.execute([sys.executable, '-c', writer], cwd, dict(os.environ), 30)
            self.assertNotEqual(ran.code, 0)
            self.assertFalse(ran.timed_out)
            self.assertLessEqual((cwd / 'big').stat().st_size, 64 * 1024)
            ran = eb.execute([sys.executable, '-c', 'import time; time.sleep(30)'], cwd, dict(os.environ), 1)
            self.assertTrue(ran.timed_out)


class CopyOracle(Stubs):
    def run_copy(self, **settings):
        self.config['coreav'] = {'exit': 0, 'listing': LISTING}
        self.config['harness'].update(settings.pop('harness', {}))
        self.config.update(settings)
        return self.evaluate(self.COPY)

    def test_the_harness_the_binary_and_the_check_agree(self):
        self.config['harness']['bytes'] = json.dumps(LISTING)
        record = self.run_copy()
        self.assertEqual((record['miss'], record['counted']), (None, True))

    def test_a_binary_that_writes_other_bytes_or_a_harness_that_does_not_write_is_binary_differs(self):
        self.assertEqual(self.run_copy()['detail'], {'check': 'binary_differs'})
        record = self.run_copy(harness={'status': 'refused', 'chosen': None, 'detail': 'x'})
        self.assertEqual((record['miss'], record['detail']), ('oracle', {'check': 'binary_differs'}))

    def test_a_job_copy_check_has_no_oracle_for_is_no_oracle(self):
        record = self.run_copy(harness={'chosen': 'transcode', 'bytes': json.dumps(LISTING)})
        self.assertEqual((record['miss'], record['detail']), ('no_oracle', {'job': 'transcode'}))
        record = self.run_copy(harness={'chosen': None, 'bytes': json.dumps(LISTING)})
        self.assertEqual(record['detail'], {'job': 'unknown'})

    def test_a_failure_of_copy_check_is_oracle_and_a_crash_is_a_fault(self):
        harness = {'bytes': json.dumps(LISTING)}
        record = self.run_copy(harness=harness, check='fail')
        self.assertEqual((record['miss'], record['detail']['check']), ('oracle', 'copy_check'))
        record = self.run_copy(harness=harness, check='crash')
        self.assertEqual(record['fault'], 'RuntimeError: copy_check crashed')

    def test_the_job_is_the_one_the_summary_schema_names_and_never_the_command_text(self):
        source = (HERE.parent.parent / 'crates' / 'coreav-cli' / 'tests' / 'ffmpeg.rs').read_text()
        for schema, job in (('remux', 'remux'), ('clip', 'clip'), ('split', 'split'), ('join', 'join'),
                            ('extract-audio', 'extract-audio'), ('transcode', 'transcode')):
            self.assertIn(f'"coreav.{schema}/1" => Some("{job}")', source)
        self.assertEqual(set(eb.COPY_JOBS), {'remux', 'clip', 'split', 'join', 'extract-audio'})


def fake_source(**attrs):
    return types.SimpleNamespace(path='/fixtures/basic.mp4', kinds=('audio',), **attrs)


class AudioChecks(unittest.TestCase):
    def cx(self, args=None, fixture='mkv_h264_aac.mkv'):
        return types.SimpleNamespace(
            tc=types.SimpleNamespace(ffmpeg='ffmpeg', ffprobe='ffprobe'), sources={},
            entry=entry(args or ['-i', 'INPUT', '-ss', '1', '-c:a', 'libopus', 'OUTPUT']), assignment=(fixture,),
            opts=types.SimpleNamespace(fixtures=pathlib.Path('/fixtures')))

    def run_checks(self, kind, ours, theirs, snr=(30.0, 30.0), decode=(0, b''), fixture='mkv_h264_aac.mkv',
                   copy=False, args=None):
        job = eb.Job(1, 0, 1, kind)
        probes = {'/o': ours, '/r': theirs}
        reference = object()
        quality = {'/o': snr[0], '/r': snr[1]}
        with mock.patch.object(eb.opus, 'run', return_value=types.SimpleNamespace(returncode=decode[0], stderr=decode[1])), \
                mock.patch.object(eb.opus, 'probe_audio', lambda ffprobe, path, select='a': probes[path]), \
                mock.patch.object(eb.opus, 'pcm', lambda ffmpeg, path, select='a', before=(), after=():
                                  reference if path.startswith('/fixtures') else path), \
                mock.patch.object(eb.opus, 'quality', lambda ref, got: quality[got]), \
                mock.patch.object(eb.transcode_check, 'audio_origin', lambda *a: 1 if copy else None), \
                mock.patch.object(eb, 'source', lambda cx, file: fake_source()):
            return list(eb.audio_checks(self.cx(args, fixture), job, '/o', '/r'))

    OPUS = {'codec_name': 'opus', 'start_pts': 0, 'initial_padding': 312, 'duration_ts': 96000, 'bit_rate': '64000'}
    AAC = {'codec_name': 'aac', 'profile': 'LC', 'sample_rate': '48000', 'start_pts': 0, 'duration_ts': 96000,
           'bit_rate': '128000'}

    def failed(self, results):
        return next((name for name, ok in results if not ok), None)

    def test_opus_passes_o1_to_o4_for_a_48_khz_source(self):
        results = self.run_checks('libopus', self.OPUS, self.OPUS)
        self.assertEqual([name for name, _ in results], ['not_copy', 'O1', 'O2', 'O3', 'O4'])
        self.assertIsNone(self.failed(results))

    def test_opus_is_not_a_packet_copy_and_decodes_clean(self):
        self.assertEqual(self.failed(self.run_checks('libopus', self.OPUS, self.OPUS, copy=True)), 'not_copy')
        self.assertEqual(self.failed(self.run_checks('libopus', self.OPUS, self.OPUS, decode=(0, b'warn'))), 'O1')
        self.assertEqual(self.failed(self.run_checks('libopus', self.OPUS, self.OPUS, decode=(1, b''))), 'O1')

    def test_the_opus_timeline_takes_the_pinned_ffmpegs_padding_and_48_samples(self):
        near = {**self.OPUS, 'duration_ts': 96048}
        far = {**self.OPUS, 'duration_ts': 96049}
        self.assertIsNone(self.failed(self.run_checks('libopus', near, self.OPUS)))
        self.assertEqual(self.failed(self.run_checks('libopus', far, self.OPUS)), 'O2')
        self.assertEqual(self.failed(self.run_checks('libopus', {**self.OPUS, 'initial_padding': 0}, self.OPUS)), 'O2')
        self.assertEqual(self.failed(self.run_checks('libopus', {**self.OPUS, 'start_pts': 312}, self.OPUS)), 'O2')
        self.assertEqual(self.failed(self.run_checks('libopus', {**self.OPUS, 'codec_name': 'aac'}, self.OPUS)), 'O2')

    def test_the_opus_rate_ratio_is_0_85_to_1_15_for_one_input(self):
        theirs = {**self.OPUS, 'bit_rate': '100000'}
        for rate, expected in (('85000', None), ('115000', None), ('84999', 'O3'), ('115001', 'O3'), ('N/A', 'O3')):
            self.assertEqual(self.failed(self.run_checks('libopus', {**self.OPUS, 'bit_rate': rate}, theirs)), expected, rate)

    def test_o4_gives_a_dB_and_judges_only_the_48_khz_fixtures(self):
        self.assertIsNone(self.failed(self.run_checks('libopus', self.OPUS, self.OPUS, snr=(29.0, 30.0))))
        self.assertEqual(self.failed(self.run_checks('libopus', self.OPUS, self.OPUS, snr=(28.9, 30.0))), 'O4')
        self.assertEqual(self.failed(self.run_checks('libopus', self.OPUS, self.OPUS, snr=(None, 30.0))), 'O4')
        results = self.run_checks('libopus', self.OPUS, self.OPUS, snr=(1.0, 30.0), fixture='basic.mp4')
        self.assertEqual([name for name, _ in results], ['not_copy', 'O1', 'O2', 'O3'])

    def test_aac_a2_wants_lc_the_same_rate_and_exactly_the_same_duration(self):
        self.assertEqual([n for n, _ in self.run_checks('aac', self.AAC, self.AAC)], ['A1', 'A2', 'A3', 'A4'])
        for change in ({'profile': 'HE-AAC'}, {'sample_rate': '44100'}, {'duration_ts': 96001}, {'start_pts': 1},
                       {'codec_name': 'opus'}):
            self.assertEqual(self.failed(self.run_checks('aac', {**self.AAC, **change}, self.AAC)), 'A2', change)

    def test_aac_a3_is_0_95_to_1_05_and_a4_half_a_dB_for_every_source(self):
        theirs = {**self.AAC, 'bit_rate': '100000'}
        for rate, expected in (('95000', None), ('105000', None), ('94999', 'A3'), ('105001', 'A3')):
            self.assertEqual(self.failed(self.run_checks('aac', {**self.AAC, 'bit_rate': rate}, theirs)), expected, rate)
        self.assertIsNone(self.failed(self.run_checks('aac', self.AAC, self.AAC, snr=(29.5, 30.0), fixture='basic.mp4')))
        self.assertEqual(self.failed(self.run_checks('aac', self.AAC, self.AAC, snr=(29.4, 30.0), fixture='basic.mp4')), 'A4')
        self.assertEqual(self.failed(self.run_checks('aac', self.AAC, self.AAC, snr=(30.0, None))), 'A4')

    def test_the_reference_is_the_source_stream_with_the_commands_timing_options(self):
        calls = []
        with mock.patch.object(eb.opus, 'pcm', lambda *a: calls.append(a) or [0.0]), \
                mock.patch.object(eb.opus, 'quality', lambda *a: 1.0), \
                mock.patch.object(eb.opus, 'run', return_value=types.SimpleNamespace(returncode=0, stderr=b'')), \
                mock.patch.object(eb.opus, 'probe_audio', lambda *a: self.AAC):
            list(eb.audio_checks(self.cx(['-ss', '1', '-i', 'INPUT', '-t', '2', '-c:a', 'aac', 'OUTPUT']),
                                 eb.Job(1, 0, 3, 'aac'), '/o', '/r'))
        self.assertEqual(calls[0], ('ffmpeg', '/fixtures/mkv_h264_aac.mkv', '3', ['-ss', '1'], ['-t', '2']))
        self.assertEqual(calls[1:], [('ffmpeg', '/o', '1'), ('ffmpeg', '/r', '1')])


class VideoChecks(unittest.TestCase):
    def setUp(self):
        self.cx = types.SimpleNamespace(
            tc=types.SimpleNamespace(ffmpeg='ffmpeg', ffprobe='ffprobe'), sources={}, scratch=pathlib.Path('/tmp'),
            entry=entry(['-i', 'INPUT', '-c:v', 'libx264', 'OUTPUT']), assignment=('basic.mp4',),
            jobs=[eb.Job(0, 0, 0, 'libx264'), eb.Job(1, 0, 1, 'copy')],
            opts=types.SimpleNamespace(fixtures=pathlib.Path('/fixtures')))
        self.judged = {'B1': {'ok': True}, 'B2': {'ok': True}, 'B3': {'ok': True},
                       'video': {'width': 160, 'height': 120}}
        self.seen = {}

    def run_checks(self, q1=40.0, base=40.0, q2=35.0, judged=None, copies=True, listing=(('video', 'h264'), ('audio', 'aac'))):
        def judge(tc, src, out, layout, video, audio, tmp, expected_audio=None, extra_audio=0):
            self.seen.update(layout=layout, audio=audio, expected=expected_audio, extra=extra_audio)
            return judged or self.judged

        psnr = iter([{'ok': True, 'average': q1}, {'ok': True, 'average': base}, {'ok': True, 'average': q2}])
        with mock.patch.object(eb, 'source', lambda cx, file: fake_source()), \
                mock.patch.object(eb, 'copied', lambda *a: copies), \
                mock.patch.object(eb.transcode_check, 'judge_output', judge), \
                mock.patch.object(eb.transcode_check, 'probe_size', lambda *a: (160, 120)), \
                mock.patch.object(eb.transcode_check, 'psnr', lambda *a: next(psnr)):
            return list(eb.video_checks(self.cx, self.cx.jobs[0], '/o', '/r', list(listing)))

    def test_b1_to_b4_pass_and_the_copied_audio_is_b3s(self):
        results = self.run_checks()
        self.assertEqual(results, [('B1', True), ('B2', True), ('B3', True), ('B4', True)])
        self.assertEqual(self.seen, {'layout': {'video': 0, 'audio': [1], 'videos': 1, 'streams': 2}, 'audio': [1],
                                     'expected': [1], 'extra': 0})

    def test_audio_that_is_encoded_beside_counts_for_b1_only(self):
        self.cx.jobs[1] = eb.Job(1, 0, 1, 'libopus')
        self.run_checks()
        self.assertEqual((self.seen['audio'], self.seen['extra']), ([], 1))

    def test_q1_is_minus_half_a_dB_against_the_pinned_ffmpegs_and_q2_30_dB(self):
        self.assertEqual(dict(self.run_checks(q1=39.5))['B4'], True)
        self.assertEqual(dict(self.run_checks(q1=39.49))['B4'], False)
        self.assertEqual(dict(self.run_checks(q2=30.0))['B4'], True)
        self.assertEqual(dict(self.run_checks(q2=29.99))['B4'], False)
        self.assertEqual(dict(self.run_checks(q1=float('inf'), base=float('inf')))['B4'], True)

    def test_a_failed_b_check_and_a_copy_that_differs_fail_their_check(self):
        self.judged['B2'] = {'ok': False}
        self.assertEqual(dict(self.run_checks())['B2'], False)
        self.judged['B2'] = {'ok': True}
        self.judged['B3'] = {'ok': False}
        self.assertEqual(dict(self.run_checks())['B3'], False)
        self.judged.pop('B3')
        self.cx.jobs.append(eb.Job(2, 0, 2, 'copy'))
        self.assertEqual(dict(self.run_checks(copies=False, listing=(('video', 'h264'), ('audio', 'aac'), ('subtitle', 'mov_text'))))['B3'], False)


class Order(unittest.TestCase):
    def test_the_start_comes_first_then_the_video_then_audio_and_a_stream_beside_an_encode_is_b3(self):
        cx = types.SimpleNamespace(jobs=[eb.Job(0, 0, 0, 'libx264'), eb.Job(1, 0, 1, 'libopus'),
                                         eb.Job(2, 0, 2, 'aac')],
                                   starts=[{'program': 'x264', 'encoder': True},
                                           {'program': 'ffmpeg', 'encoder': True}])
        with mock.patch.object(eb, 'video_checks', lambda *a: iter([('B1', True)])), \
                mock.patch.object(eb, 'audio_checks', lambda cx_, job, *a: iter([(job.kind, True)])):
            self.assertEqual([n for n, _ in eb.checks(cx, '/o', '/r', [])],
                             ['x264_start', 'aac_start', 'B1', 'libopus', 'aac'])
        cx.starts = []
        with mock.patch.object(eb, 'video_checks', lambda *a: iter([])), \
                mock.patch.object(eb, 'audio_checks', lambda *a: iter([])):
            self.assertEqual([(n, ok) for n, ok in eb.checks(cx, '/o', '/r', [])],
                             [('x264_start', False), ('aac_start', False)])

    def test_a_copy_beside_audio_encodes_is_judged_by_b3_without_a_video_encode(self):
        cx = types.SimpleNamespace(jobs=[eb.Job(0, 0, 0, 'copy'), eb.Job(1, 0, 1, 'aac')],
                                   starts=[{'program': 'ffmpeg', 'encoder': True}])
        copies = []
        with mock.patch.object(eb, 'copied', lambda cx_, job, out: copies.append(job.output) or True), \
                mock.patch.object(eb, 'audio_checks', lambda *a: iter([('A1', True)])):
            self.assertEqual([n for n, _ in eb.checks(cx, '/o', '/r', [])], ['aac_start', 'B3', 'A1'])
        self.assertEqual(copies, [0])

    def test_a_copy_beside_an_encode_is_read_with_the_packets_of_every_kind(self):
        packets = {('/o', '0'): ('1/90000', [{'pts': 0, 'dts': 0, 'duration': 1, 'data_hash': 'a'}]),
                   ('/fixtures/basic.mp4', '0'): ('1/90000', [{'pts': 0, 'dts': 0, 'duration': 1, 'data_hash': 'a'}])}
        remux = {0: ('1/90000', [{'pts': 0, 'dts': 0, 'duration': 1}])}
        source = types.SimpleNamespace(path='/fixtures/basic.mp4', remux=lambda tmp: remux)
        cx = types.SimpleNamespace(tc=types.SimpleNamespace(ffprobe='ffprobe'), scratch=pathlib.Path('/tmp'))
        job = eb.Job(0, 0, 0, 'copy')
        with mock.patch.object(eb, 'source', lambda cx_, file: source), \
                mock.patch.object(eb.transcode_check, 'packets',
                                  lambda ffprobe, path, select, data_hash=False: packets[(path, select)]):
            self.assertTrue(eb.copied(cx, job, '/o'))
            packets[('/o', '0')][1][0]['data_hash'] = 'b'
            self.assertFalse(eb.copied(cx, job, '/o'))
            packets[('/o', '0')][1][0]['data_hash'] = 'a'
            remux[0][1][0]['dts'] = 1
            self.assertFalse(eb.copied(cx, job, '/o'))

    def test_the_source_reads_the_packets_of_video_and_audio(self):
        cx = types.SimpleNamespace(tc=object(), sources={}, entry=entry(['-i', 'INPUT', 'OUTPUT']),
                                   assignment=('basic.mp4',), opts=types.SimpleNamespace(fixtures=pathlib.Path('/f')))
        with mock.patch.object(eb.transcode_check, 'Source', lambda tc, path: types.SimpleNamespace(path=path)):
            self.assertEqual(eb.source(cx, 0).kinds, ('audio', 'video'))
            self.assertEqual(eb.source(cx, 0).path, '/f/basic.mp4')
        self.assertEqual(eb.source_of(cx, 1), (None, None))


def record(index, cls='media', container='MP4', miss=None, detail=None, groups=(), args=None, **more):
    return {'index': index, 'class': cls, 'container': container, 'args': args or ['-i', 'INPUT', 'OUTPUT'],
            'counted': miss is None, 'miss': miss, 'detail': detail or {}, 'groups': list(groups), **more}


def whole_run(counted_media=0, counted_other=0):
    """One record for each command of the set, the first counted_media of
    class media and the first counted_other of the rest counted."""
    rows, media, other = [], 0, 0
    for index, e in eb.commands('v3'):
        is_media = e['class'] == 'media'
        counted = (media < counted_media) if is_media else (other < counted_other)
        media, other = media + is_media, other + (not is_media)
        rows.append(record(index, e['class'], e['container'], None if counted else 'no_input', args=e['args']))
    return rows


class Result(unittest.TestCase):
    def test_v2_needs_196_of_488(self):
        for counted, met in ((195, False), (196, True)):
            v2 = eb.summarize(whole_run(counted), 'v3')['v2']
            self.assertEqual((v2['counted'], v2['of'], v2['needed'], v2['met']), (counted, 488, 196, met))
        self.assertEqual(v2['share'], 0.4016)

    def test_v3_is_the_share_of_630_per_class_beside_the_79_native(self):
        summary = eb.summarize(whole_run(100, 7), 'v3')
        v3 = summary['v3']
        self.assertEqual((v3['counted'], v3['of']), (107, 630))
        self.assertEqual(v3['per_class']['media'], {'commands': 488, 'counted': 100})
        self.assertEqual(set(v3['per_class']), {'media', 'unknown', 'decode-only', 'subtitles', 'other'})
        self.assertEqual(v3['native']['commands'], 79)
        self.assertEqual(v3['misses'], {'no_input': 523})
        self.assertNotIn('v3', eb.summarize([r for r in whole_run(100) if r['class'] == 'media'], 'v2'))

    def test_the_count_per_container_and_the_misses_by_code_in_the_plans_order(self):
        rows = [record(0, container='MP4'), record(1, container='MP4', miss='oracle', detail={'check': 'B1'}),
                record(2, container='Matroska/WebM', miss='refused', detail={'reason': 'x'}),
                record(3, container='Matroska/WebM', miss='no_input'), record(4, miss='streams_differ')]
        v2 = eb.summarize(rows, 'v2')['v2']
        self.assertEqual(v2['per_container'], {'MP4': {'commands': 3, 'counted': 1},
                                               'Matroska/WebM': {'commands': 2, 'counted': 0}})
        self.assertEqual(list(v2['misses']), ['no_input', 'refused', 'oracle', 'streams_differ'])

    def test_a_refused_command_counts_once_under_the_option_cored_named(self):
        rows = ([record(i, miss='refused', detail={'reason': '-vf scale'}) for i in range(3)]
                + [record(i, miss='refused', detail={'reason': 'none'}) for i in range(3, 8)]
                + [record(8, miss='refused', detail={'reason': '-c:s'}), record(9, miss='failed')])
        self.assertEqual(list(eb.summarize(rows, 'v2')['v2']['refused_options'].items()),
                         [('none', 5), ('-vf scale', 3), ('-c:s', 1)])

    def test_a_command_is_in_the_group_of_each_encoder_it_uses(self):
        rows = [record(0, groups=['x264', 'aac']), record(1, groups=['x264'], miss='oracle'),
                record(2, groups=['copy only']), record(3, groups=['other encoders'], miss='no_oracle'),
                record(4, groups=['libopus', 'x264'], miss='timeout'), record(5, groups=[], miss='no_input')]
        per = eb.summarize(rows, 'v2')['v2']['per_encoder']
        self.assertEqual(per['x264'], {'commands': 3, 'counted': 1, 'misses': {'timeout': 1, 'oracle': 1}})
        self.assertEqual(per['aac'], {'commands': 1, 'counted': 1, 'misses': {}})
        self.assertEqual(per['libopus'], {'commands': 1, 'counted': 0, 'misses': {'timeout': 1}})
        self.assertEqual(per['other encoders'], {'commands': 1, 'counted': 0, 'misses': {'no_oracle': 1}})
        self.assertEqual(per['copy only'], {'commands': 1, 'counted': 1, 'misses': {}})
        self.assertEqual(list(per), list(eb.GROUPS))

    def test_a_run_is_void_with_a_logged_program_a_fault_a_missing_or_a_doubled_command(self):
        rows = whole_run(0)
        self.assertFalse(eb.summarize(rows, 'v3')['void']['void'])
        self.assertEqual(eb.summarize(rows[1:], 'v3')['void']['missing'], [0])
        self.assertTrue(eb.summarize(rows[1:], 'v3')['void']['void'])
        self.assertTrue(eb.summarize(rows + rows[:1], 'v3')['void']['void'])
        fault = [{**rows[0], 'fault': 'x', 'miss': None, 'counted': False}] + rows[1:]
        self.assertEqual(eb.summarize(fault, 'v3')['void']['faults'], [0])
        ran = [{**rows[0], 'void': True}] + rows[1:]
        void = eb.summarize(ran, 'v3')['void']
        self.assertEqual((void['void'], void['fallback_ran']), (True, [0]))
        self.assertEqual(eb.summarize([{**r, 'void': False} for r in rows], 'v3')['void']['fallback_ran'], [])


class Cli(Stubs):
    def test_run_writes_a_record_a_command_and_a_summary_with_the_programs_hashes(self):
        small = [(0, entry(['-i', 'INPUT', '-c', 'copy', 'OUTPUT'])), (1, entry(['-i', 'INPUT', 'OUTPUT'], cls='other',
                                                                              container=None))]
        (self.dir / 'config.json').write_text(json.dumps(self.config))
        argv = ['encode_breadth.py', 'run'] + sum(([f'--{k}', str(getattr(self.opts, k))] for k in
                                                   ('coreav', 'harness', 'ffmpeg', 'ffprobe', 'x264', 'fixtures')), []) + [
            '--work', str(self.dir / 'w'), '--records', str(self.dir / 'out' / 'r.jsonl'),
            '--summary', str(self.dir / 'out' / 's.json'), '--jobs', '2']
        with mock.patch.object(sys, 'argv', argv), mock.patch.object(eb, 'commands', lambda selected: small), \
                mock.patch.object(eb, 'oracle', return_value=None), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(eb.main(), 0)
        rows = [json.loads(line) for line in (self.dir / 'out' / 'r.jsonl').read_text().splitlines()]
        self.assertEqual([r['index'] for r in rows], [0, 1])
        summary = json.loads((self.dir / 'out' / 's.json').read_text())
        self.assertEqual(set(summary['programs']), {'coreav', 'harness', 'ffmpeg', 'ffprobe', 'x264'})
        self.assertEqual(set(summary['fixtures']), set(eb.FIXTURES))
        self.assertEqual(summary['schema'], 'coreav.encode-breadth/1')

    def test_a_missing_fixture_is_unmeasured_and_summarize_exits_as_the_run_would(self):
        (self.fixtures / 'basic.mp4').unlink()
        argv = ['encode_breadth.py', 'run'] + sum(([f'--{k}', str(getattr(self.opts, k))] for k in
                                                   ('coreav', 'harness', 'ffmpeg', 'ffprobe', 'x264', 'fixtures')), []) + [
            '--work', str(self.dir / 'w'), '--records', str(self.dir / 'r'), '--summary', str(self.dir / 's')]
        with mock.patch.object(sys, 'argv', argv), contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(eb.main(), release_round.UNMEASURED)
        self.assertIn('basic.mp4', out.getvalue())
        records = self.dir / 'records.jsonl'
        records.write_text(''.join(json.dumps(r) + '\n' for r in whole_run(5)))
        done = subprocess.run([sys.executable, '-I', str(HERE / 'encode_breadth.py'), 'summarize', '--records',
                               str(records), '--summary', str(self.dir / 'sum.json')], capture_output=True, text=True,
                              timeout=60)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(json.loads((self.dir / 'sum.json').read_text())['v3']['counted'], 5)


if __name__ == '__main__':
    unittest.main()
