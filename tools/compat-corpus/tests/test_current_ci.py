import collections
import hashlib
import json
import os
import pathlib
import shutil
import struct
import sys
import tempfile
import types
import unittest
from unittest import mock
import wave
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = ROOT / 'tools' / 'compat-corpus'
sys.path.insert(0, str(HERE))
import current_ci
import decoded_output


class CurrentPolicy(unittest.TestCase):
    def run_policy(self, code, stdout=b'', stderr=b'', result=('verified', 'exact'), event=None, modify=False):
        row = {'tool': 'ffmpeg', 'label': 'out-of-scope',
               'args': ['-i', 'INPUT', '-f', 's16le', '-acodec', 'pcm_s16le', 'OUTPUT']}
        legacy = types.SimpleNamespace(load_commands=lambda: [row], ENV={}, HELP_QUERIES=[])
        records, failures = [], []
        def invoke(argv, env, cwd):
            if modify:
                pathlib.Path(argv[argv.index('-i') + 1]).write_bytes(b'changed')
            if event is not None:
                pathlib.Path(env['COREAV_FALLBACK_LOG']).write_text(json.dumps(event) + '\n')
            if code == 0:
                (cwd / 'output.mp4').write_bytes(b'not enough to prove an output')
            return code, stdout, stderr
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(decoded_output, 'invoke', side_effect=invoke), \
                mock.patch.object(current_ci, 'verify', return_value=result) as verify:
            fixture = pathlib.Path(tmp) / 'fixture.wav'
            fixture.write_bytes(b'unchanged input')
            current_ci.refusals(legacy, types.SimpleNamespace(coreav='stand-in'), [fixture],
                                collections.Counter(), failures, records, 'ffmpeg', 'ffprobe')
            self.assertEqual(fixture.read_bytes(), b'unchanged input')
        return records[0], failures, verify.call_count

    def test_clean_refusal_and_verified_success_pass_on_either_side_of_a_feature_merge(self):
        for code, err, status in ((1, b'rejected', 'refused'), (2, b'usage', 'refused'),
                                  (4, b'unsupported', 'refused'), (0, b'', 'verified')):
            record, failures, calls = self.run_policy(code, stderr=err)
            self.assertEqual(record['status'], status)
            self.assertEqual(failures, [])
            self.assertEqual(calls, int(code == 0))

    def test_a_wrong_output_success_fails(self):
        record, failures, _ = self.run_policy(0, result=('failed', 'decoded samples'))
        self.assertEqual(record['status'], 'failed')
        self.assertEqual(len(failures), 1)

    def test_an_unverified_success_never_counts(self):
        record, failures, _ = self.run_policy(0, result=('unverified', 'no_oracle: PNG/BMP colour conversion'))
        self.assertEqual(record['status'], 'unverified')
        self.assertEqual(len(failures), 1)

    def test_crashes_and_timeouts_do_not_become_refusals(self):
        for code in (-11, -6, 3, 101, 134, 139, None):
            record, failures, calls = self.run_policy(code, stderr=b'a diagnostic')
            self.assertEqual(record['status'], 'failed')
            self.assertEqual(len(failures), 1)
            self.assertEqual(calls, 0)

    def test_empty_error_is_not_a_clean_refusal(self):
        self.assertEqual(self.run_policy(4)[0]['status'], 'failed')

    def test_input_mutation_fails_and_cannot_modify_the_fixture(self):
        record, failures, calls = self.run_policy(4, stderr=b'unsupported', modify=True)
        self.assertEqual(record['reason'], 'modified input')
        self.assertEqual(len(failures), 1)
        self.assertEqual(calls, 0)

    def test_refusal_logs_are_allowed_but_fallback_and_success_logs_are_not(self):
        refusal = {'ran': None, 'reason': 'unsupported'}
        self.assertEqual(self.run_policy(4, stderr=b'unsupported', event=refusal)[0]['status'], 'refused')
        self.assertEqual(self.run_policy(4, stderr=b'unsupported', event={'ran': 'ffmpeg'})[0]['status'], 'failed')
        self.assertEqual(self.run_policy(0, event=refusal)[0]['status'], 'failed')

    def test_a_baseline_cannot_hide_a_current_policy_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = pathlib.Path(tmp) / 'report.json'
            def commands(opts):
                counts, failures = legacy.ffmpeg_refusals(opts, [], collections.Counter(), [])
                report.write_text(json.dumps({'B1 ffmpeg': dict(counts), 'failures': failures}))
                return 0
            legacy = types.SimpleNamespace(cmd_commands=commands)
            def refusals(*args):
                args[5].append({'status': 'unverified'})
                return collections.Counter(fail=1), []
            with mock.patch.object(sys, 'argv', ['current_ci.py', 'commands', '--coreav', 'candidate',
                                                '--report', str(report), '--baseline', 'baseline.json']), \
                    mock.patch.object(current_ci, 'reporter', return_value=legacy), \
                    mock.patch.object(current_ci, 'refusals', side_effect=refusals), \
                    mock.patch.object(current_ci.shutil, 'which', side_effect=lambda n: '/oracle/' + n), \
                    mock.patch.object(current_ci.oracle_pin, 'verify_bin', return_value=[]):
                self.assertEqual(current_ci.main(), 1)

    def test_the_frozen_judging_files_keep_their_registered_hashes(self):
        registration = json.loads((ROOT / 'docs/measurements/compat-0.1.0/judging.json').read_text())
        self.assertIn('tools/compat-corpus/run.py', registration['files'])
        for name in ('tools/compat-corpus/run.py', '.github/workflows/compat-0.1.0.yml'):
            with self.subTest(path=name):
                self.assertEqual(hashlib.sha256((ROOT / name).read_bytes()).hexdigest(), registration['files'][name])

    def test_current_ci_is_not_used_by_the_frozen_workflow(self):
        workflow = (ROOT / '.github/workflows/compat-0.1.0.yml').read_text()
        self.assertNotIn('current_ci.py', workflow)
        self.assertNotIn('decoded_output.py', workflow)

    def test_every_current_fixture_refusal_call_uses_the_current_adapter(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertEqual(workflow.count('tools/compat-corpus/current_ci.py commands'), 3)
        self.assertNotIn('tools/compat-corpus/run.py commands --coreav', workflow)
        self.assertEqual(workflow.count('tools/compat-corpus/run.py commands --set 0.1.0'), 3)


class DecodedSamples(unittest.TestCase):
    def test_frame_probes_use_the_shared_audio_input_options(self):
        with mock.patch.object(decoded_output, 'invoke', return_value=(0, b'{"streams":[{}]}', b'')) as invoke:
            self.assertIsNotNone(decoded_output.probe('media', 'ffprobe', {}))
        args = invoke.call_args.args[0]
        self.assertEqual(args[args.index('-i') - len(decoded_output.AUDIO_DECODE_INPUT):args.index('-i')],
                         list(decoded_output.AUDIO_DECODE_INPUT))

    def test_a_selected_encode_does_not_replace_the_copy_stream_oracle(self):
        stream = {'index': 1, 'codec_type': 'audio', 'codec_name': 'flac', 'sample_fmt': 's16',
                  'sample_rate': '48000', 'channels': 1}
        listing = {'streams': [{'index': 0, 'codec_type': 'video', 'codec_name': 'h264'}, stream], 'frames': []}
        def decode(args, env, output):
            self.assertIn('0:1', args)
            output.write_bytes(struct.pack('<i', 256))
            return 0, b'', b''
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(decoded_output, 'probe', return_value=listing), \
                mock.patch.object(decoded_output, 'invoke', side_effect=decode):
            self.assertEqual(decoded_output.compare('a', 'b', 'ffmpeg', 'ffprobe', {}, pathlib.Path(tmp), stream=1)[0],
                             'verified')
            self.assertEqual(decoded_output.compare('a', 'b', 'ffmpeg', 'ffprobe', {}, pathlib.Path(tmp))[0],
                             'unverified')

    def test_timed_out_programs_are_killed_and_reaped(self):
        popen, children = decoded_output.subprocess.Popen, []
        def launch(*args, **kwargs):
            child = popen(*args, **kwargs)
            children.append(child)
            return child
        with mock.patch.object(decoded_output, 'SECONDS', 0.1), \
                mock.patch.object(decoded_output.subprocess, 'Popen', side_effect=launch):
            result = decoded_output.invoke([sys.executable, '-c', 'import time; time.sleep(60)'], os.environ)
        self.assertEqual(result[0], None)
        self.assertIsNotNone(children[0].returncode)

    def compare_audio(self, left, right):
        stream = {'codec_type': 'audio', 'codec_name': 'flac', 'sample_fmt': 's32',
                  'sample_rate': '48000', 'channels': 1, 'bits_per_raw_sample': '24'}
        listing = {'streams': [stream], 'frames': []}
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            def decode(argv, env, output):
                output.write_bytes(left if argv[argv.index('-i') + 1] == 'candidate' else right)
                return 0, b'', b''
            with mock.patch.object(decoded_output, 'probe', return_value=listing), \
                    mock.patch.object(decoded_output, 'invoke', side_effect=decode):
                return decoded_output.compare('candidate', 'reference', 'ffmpeg', 'ffprobe', {}, directory)

    def test_the_last_significant_sample_bit_is_not_lost(self):
        self.assertEqual(self.compare_audio(struct.pack('<i', 256), struct.pack('<i', 512))[0], 'failed')
        self.assertEqual(self.compare_audio(struct.pack('<i', 256), struct.pack('<i', 256))[0], 'verified')

    def test_candidate_format_mismatches_fail_instead_of_becoming_unverified(self):
        expected = {'streams': [{'codec_type': 'audio', 'codec_name': 'flac', 'sample_fmt': 's16',
                                  'sample_rate': '48000', 'channels': 1}], 'frames': []}
        actual = {'streams': [{**expected['streams'][0], 'sample_rate': '16000'}], 'frames': []}
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(decoded_output, 'probe', side_effect=[expected, actual]):
            result = decoded_output.compare('candidate', 'reference', 'ffmpeg', 'ffprobe', {}, pathlib.Path(tmp))
        self.assertEqual(result, ('failed', 'stream fields or timing'))

    def test_raw_description_follows_the_selected_stream_not_output_length(self):
        source = {'streams': [{'index': 0, 'codec_type': 'audio', 'sample_rate': '44100', 'channels': 1},
                              {'index': 1, 'codec_type': 'audio', 'sample_rate': '48000', 'channels': 2}]}
        args = ['-i', 'INPUT', '-f', 's16le', '-map', '0:0', 'OUTPUT']
        self.assertEqual(decoded_output.raw_inputs(args, source),
                         ['-f', 's16le', '-sample_rate', '44100', '-ch_layout', 'mono'])
        self.assertEqual(decoded_output.raw_inputs(['-i', 'INPUT', '-f', 's16le', 'OUTPUT'], source),
                         ['-f', 's16le', '-sample_rate', '48000', '-ch_layout', 'stereo'])


class CopyMapping(unittest.TestCase):
    def setUp(self):
        self.video = {'index': 4, 'codec_type': 'video', 'codec_name': 'h264'}
        self.audio = {'index': 8, 'codec_type': 'audio', 'codec_name': 'aac'}
        self.sources = [{'streams': [self.video, self.audio]}] * 2
        self.reference = {'streams': [{**self.video, 'index': 0}, {**self.audio, 'index': 1}]}
        self.lines = ['  Stream #0:4 -> #0:0 (copy)', '  Stream #1:8 -> #0:1 (copy)']

    def mapping(self, lines):
        return current_ci.copy_mapping(('Stream mapping:\n' + '\n'.join(lines) + '\nOutput #0, mp4\n').encode(),
                                        self.sources, self.reference)

    def test_every_output_binds_its_actual_source_index_in_mapping_order(self):
        self.assertEqual(self.mapping(self.lines), ((0, 0, 4), (1, 1, 8)))
        self.assertEqual(self.mapping(self.lines[::-1]), ((0, 0, 4), (1, 1, 8)))
        self.reference['streams'][1] = {**self.video, 'index': 1}
        self.assertEqual(self.mapping([self.lines[0], '  Stream #0:4 -> #0:1 (copy)']),
                         ((0, 0, 4), (1, 0, 4)))

    def test_mp4_muxer_diagnostics_do_not_replace_or_end_mapping_validation(self):
        warning = '[mp4 @ 0x55d61386ad80] track 1: codec frame size is not set'
        self.assertEqual(self.mapping([warning, *self.lines, warning]), ((0, 0, 4), (1, 1, 8)))
        for lines in ([self.lines[0], warning], [self.lines[0], warning, '  unparsed mapping'],
                      [*self.lines, warning, self.lines[0]],
                      [self.lines[0], warning, self.lines[1].replace('(copy)', '(aac -> aac)')],
                      [*self.lines, '[unrecognized diagnostic]']):
            with self.subTest(lines=lines):
                self.assertEqual(self.mapping(lines), ())

    def test_copied_audio_compares_its_complete_rational_frame_timeline(self):
        for codec in ('aac', 'opus'):
            stream = {'index': 0, 'codec_type': 'audio', 'codec_name': codec, 'sample_fmt': 'fltp',
                      'sample_rate': '48000', 'channels': 1, 'time_base': '1/1000', 'start_pts': 0}
            frames = [{'stream_index': 0, 'best_effort_timestamp': pts, 'duration': 20, 'nb_samples': 960}
                      for pts in (0, 20, 40)]
            reference = {'streams': [stream], 'frames': frames}
            mutations = [frames[:2] + [{**frames[2], 'best_effort_timestamp': 1040}],
                         [frames[0], {**frames[1], 'duration': 19}, frames[2]], frames[:-1], [],
                         [frames[0], {**frames[1], 'best_effort_timestamp': 'invalid'}, frames[2]],
                         [frames[0], {**frames[1], 'nb_samples': 959}, frames[2]],
                         [frames[0], {**frames[1], 'best_effort_timestamp': None}, frames[2]],
                         [frames[0], {**frames[1], 'nb_samples': 0}, frames[2]]]
            scaled = [{**f, 'best_effort_timestamp': f['best_effort_timestamp'] * 48,
                       'duration': f['duration'] * 48} for f in frames]
            cases = [(reference, 'verified'),
                     ({'streams': [{**stream, 'time_base': '1/48000'}], 'frames': scaled}, 'verified')]
            cases += [({'streams': [stream], 'frames': changed}, 'failed') for changed in mutations]
            for actual, status in cases:
                with self.subTest(codec=codec, actual=actual), tempfile.TemporaryDirectory() as tmp:
                    def decode(argv, env, output):
                        output.write_bytes(struct.pack('<d', 0.25) * 2880)
                        return 0, b'', b''
                    with mock.patch.object(decoded_output, 'probe', side_effect=[reference, actual]), \
                            mock.patch.object(decoded_output, 'invoke', side_effect=decode):
                        result = decoded_output.compare('candidate', 'reference', 'ffmpeg', 'ffprobe', {},
                                                        pathlib.Path(tmp), copied_streams=(0,))
                    self.assertEqual(result[0], status, result)

    def test_copied_audio_with_unknown_durations_on_both_sides_is_not_verified(self):
        for codec in ('aac', 'opus'):
            stream = {'index': 0, 'codec_type': 'audio', 'codec_name': codec, 'sample_fmt': 'fltp',
                      'sample_rate': '48000', 'channels': 1, 'time_base': '1/1000', 'start_pts': 0}
            for duration in ({}, {'duration': 'N/A'}, {'duration': None}):
                frame = {'stream_index': 0, 'best_effort_timestamp': 0, 'nb_samples': 960, **duration}
                listing = {'streams': [stream], 'frames': [frame]}
                with self.subTest(codec=codec, duration=duration), tempfile.TemporaryDirectory() as tmp:
                    def decode(argv, env, output):
                        output.write_bytes(struct.pack('<d', 0.25) * 960)
                        return 0, b'', b''
                    with mock.patch.object(decoded_output, 'probe', return_value=listing), \
                            mock.patch.object(decoded_output, 'invoke', side_effect=decode):
                        result = decoded_output.compare('candidate', 'reference', 'ffmpeg', 'ffprobe', {},
                                                        pathlib.Path(tmp), copied_streams=(0,))
                    self.assertEqual(result, ('failed', 'invalid audio timing'))

    def test_incomplete_or_noncopy_mapping_never_qualifies(self):
        changes = [[], self.lines[:1], self.lines + self.lines[:1],
                   self.lines + ['  unparsed mapping'],
                   [self.lines[0], self.lines[1].replace('(copy)', '(aac (native) -> aac (native))')],
                   [self.lines[0], '  Stream #1:8 -> output:audio (copy)'],
                   [self.lines[0], 'unparsed mapping'],
                   [self.lines[0], self.lines[1].replace('#1:8', '#2:8')],
                   [self.lines[0], self.lines[1].replace('#1:8', '#1:1')],
                   [self.lines[0], self.lines[1].replace('#0:1', '#1:1')],
                   [self.lines[0], self.lines[1].replace('#0:1', '#0:2')],
                   [self.lines[0], self.lines[1].replace('#1:8', '#1:4')]]
        for lines in changes:
            with self.subTest(lines=lines):
                self.assertEqual(self.mapping(lines), ())

    def test_probe_stream_indices_cannot_be_duplicated_or_relabelled(self):
        for indices in ((0, 0), (1, 2), (1, 0)):
            self.reference['streams'] = [{**s, 'index': i} for s, i in zip((self.video, self.audio), indices)]
            self.assertEqual(self.mapping(self.lines), ())

    def test_missing_and_duplicate_mapping_sections_never_qualify(self):
        for data in (b'', b'Stream mapping:\nStream mapping:\n', b'\xff'):
            self.assertEqual(current_ci.copy_mapping(data, self.sources, self.reference), ())

    def test_the_default_comparator_does_not_admit_copied_codecs(self):
        for codec in ('h264', 'aac', 'vp9', 'opus'):
            with mock.patch.object(decoded_output, 'probe', return_value={'streams': [{'codec_name': codec}]}):
                self.assertEqual(decoded_output.compare('a', 'b', 'ffmpeg', 'ffprobe', {}, pathlib.Path('.')),
                                 ('unverified', 'unregistered codec ' + codec))

    def test_verification_stages_distinct_bindings_in_input_occurrence_order(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = pathlib.Path(tmp)
            candidate, reference, scratch = (work / n for n in ('candidate', 'reference', 'scratch'))
            for directory in (candidate, reference, scratch):
                directory.mkdir()
            first, second = work / 'a.mp4', work / 'b.mp4'
            first.write_bytes(b'first')
            second.write_bytes(b'second')
            (candidate / 'output.mp4').write_bytes(b'output')
            args = ['-i', 'INPUT2', '-i', 'INPUT', '-c', 'copy', 'OUTPUT']
            def reference_command(argv, env, cwd):
                inputs = [pathlib.Path(argv[i + 1]) for i, a in enumerate(argv[:-1]) if a == '-i']
                self.assertEqual([p.read_bytes() for p in inputs], [b'second', b'first'])
                self.assertTrue(all(not p.is_relative_to(reference) for p in inputs))
                self.assertEqual(argv[argv.index('-v') + 1], 'info')
                (cwd / 'output.mp4').write_bytes(b'output')
                return 0, b'', ('Stream mapping:\n' + '\n'.join(self.lines) + '\nOutput #0, mp4\n').encode()
            with mock.patch.object(decoded_output, 'probe', side_effect=[*self.sources, self.reference]), \
                    mock.patch.object(decoded_output, 'invoke', side_effect=reference_command), \
                    mock.patch.object(decoded_output, 'compare', return_value=('verified', 'exact')) as compare:
                self.assertEqual(current_ci.verify(args, {'INPUT': first, 'INPUT2': second}, candidate, reference,
                                                    {}, 'ffmpeg', 'ffprobe', scratch)[0], 'verified')
            self.assertEqual(compare.call_args.kwargs, {'copied_streams': (0, 1)})


@unittest.skipUnless(os.environ.get('COREAV_CURRENT_CI_ORACLE') == '1', 'pinned Linux oracle runs in fixture CI')
class PinnedOracle(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ffmpeg, cls.ffprobe = shutil.which('ffmpeg'), shutil.which('ffprobe')
        if not cls.ffmpeg or not cls.ffprobe:
            raise AssertionError('fixture CI must install the pinned oracle')
        problems = list(current_ci.oracle_pin.verify_bin(pathlib.Path(cls.ffmpeg).parent))
        if problems:
            raise AssertionError(problems)
        cls.env = {'PATH': os.environ.get('PATH', ''), 'GCONV_PATH': '/nonexistent/gconv'}

    def wav(self, path, samples, junk=False):
        with wave.open(str(path), 'wb') as writer:
            writer.setparams((1, 2, 48000, 0, 'NONE', 'not compressed'))
            writer.writeframes(struct.pack('<' + 'h' * len(samples), *samples))
        if junk:
            data = path.read_bytes()
            extra = b'JUNK' + struct.pack('<I', 4) + b'test'
            path.write_bytes(data[:4] + struct.pack('<I', len(data) - 8 + len(extra)) + data[8:12] + extra + data[12:])

    def png(self, path, pixel, level):
        def chunk(kind, data):
            return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
        header = struct.pack('>IIBBBBB', 1, 1, 8, 2, 0, 0, 0)
        path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', header)
                         + chunk(b'IDAT', zlib.compress(b'\0' + pixel, level)) + chunk(b'IEND', b''))

    def copy_sources(self, work, video, audio):
        paths = []
        for index, colours in enumerate((('red', 'green'), ('blue', 'white'))):
            path = work / f'source-{index}.mkv'
            argv = [self.ffmpeg, '-nostdin', '-v', 'error']
            for expression in (f'color=c={colours[0]}:s=16x16:r=5:d=0.6',
                               f'color=c={colours[1]}:s=16x16:r=5:d=0.6',
                               f'sine=frequency={440 + index * 400}:sample_rate=48000:duration=0.6',
                               f'sine=frequency={660 + index * 400}:sample_rate=48000:duration=0.6'):
                argv += [*decoded_output.AUDIO_DECODE_INPUT, '-f', 'lavfi', '-i', expression]
            argv += ['-map', '0:v', '-map', '2:a', '-map', '1:v', '-map', '3:a',
                     '-c:v', video, '-pix_fmt', 'yuv420p', '-c:a', audio]
            argv += ['-preset', 'ultrafast', '-bf', '0'] if video == 'libx264' else [
                '-deadline', 'realtime', '-cpu-used', '8']
            code, _, err = decoded_output.invoke([*argv, str(path)], self.env)
            self.assertEqual(code, 0, err)
            paths.append(path)
        decoded = {}
        for file, path in enumerate(paths):
            for stream in range(4):
                output = ['-c:a', 'pcm_f64le', '-f', 'f64le'] if stream % 2 else ['-f', 'rawvideo']
                code, data, err = decoded_output.invoke(
                    [self.ffmpeg, '-nostdin', '-v', 'error', *decoded_output.AUDIO_DECODE_INPUT,
                     '-i', str(path), '-map', f'0:{stream}', *output, '-'], self.env)
                self.assertEqual((code, err), (0, b''))
                self.assertTrue(data)
                decoded[file, stream] = data
        for stream in (0, 1):
            self.assertNotEqual(decoded[0, stream], decoded[1, stream])
            for file in (0, 1):
                self.assertNotEqual(decoded[file, stream], decoded[file, stream + 2])
        return {'INPUT': paths[0], 'INPUT2': paths[1], 'INPUT3': paths[0]}

    def copy_case(self, work, args, bindings, candidate_args=None):
        with tempfile.TemporaryDirectory(dir=work) as tmp:
            root = pathlib.Path(tmp)
            candidate, reference, scratch = (root / n for n in ('candidate', 'reference', 'scratch'))
            for directory in (candidate, reference, scratch):
                directory.mkdir()
            argv = [str(candidate / 'output.mp4') if a == 'OUTPUT' else str(bindings[a]) if a in bindings else a
                    for a in (candidate_args or args)]
            code, _, err = decoded_output.invoke([self.ffmpeg, '-nostdin', '-v', 'error', *argv], self.env)
            self.assertEqual(code, 0, err)
            return current_ci.verify(args, bindings, candidate, reference, self.env, self.ffmpeg, self.ffprobe, scratch)

    def test_mapped_mp4_commands_and_explicit_matroska_copies_are_verified(self):
        rows = [json.loads(line) for line in (HERE / 'commands-v1/commands.jsonl').read_text().splitlines()]
        commands = [rows[i - 1]['args'] for i in (321, 392, 597, 710, 748, 804, 1052, 1120)]
        matroska = ['-i', 'INPUT', '-map', '0:0', '-map', '0:1', '-c', 'copy', '-f', 'matroska', 'OUTPUT']
        for video, audio in (('libx264', 'aac'), ('libvpx-vp9', 'libopus')):
            with self.subTest(video=video), tempfile.TemporaryDirectory() as tmp:
                work = pathlib.Path(tmp)
                bindings = self.copy_sources(work, video, audio)
                for args in [*commands, matroska]:
                    with self.subTest(args=args):
                        result = self.copy_case(work, args, bindings)
                        self.assertEqual(result[0], 'verified', result)
                changed_container = matroska[:-1] + ['-metadata', 'title=different container bytes', 'OUTPUT']
                self.assertEqual(self.copy_case(work, matroska, bindings, changed_container)[0], 'verified')
                for maps in (['0:1', '0:0'], ['0:0'], ['0:0', '0:1', '0:2']):
                    wrong = ['-i', 'INPUT'] + [a for spec in maps for a in ('-map', spec)] + [
                        '-c', 'copy', '-f', 'matroska', 'OUTPUT']
                    with self.subTest(maps=maps):
                        self.assertEqual(self.copy_case(work, matroska, bindings, wrong)[0], 'failed')

    def test_wrong_input_and_same_kind_stream_copies_fail_for_each_codec(self):
        for video, audio in (('libx264', 'aac'), ('libvpx-vp9', 'libopus')):
            with self.subTest(video=video), tempfile.TemporaryDirectory() as tmp:
                work = pathlib.Path(tmp)
                bindings = self.copy_sources(work, video, audio)
                for form in ('mp4', 'matroska'):
                    for stream in (0, 1):
                        args = ['-i', 'INPUT', '-i', 'INPUT2', '-map', f'1:{stream}',
                                '-c', 'copy', '-f', form, 'OUTPUT']
                        result = self.copy_case(work, args, bindings)
                        self.assertEqual(result[0], 'verified', result)
                        for wrong in (f'0:{stream}', f'1:{stream + 2}'):
                            candidate = [wrong if a == f'1:{stream}' else a for a in args]
                            with self.subTest(form=form, stream=stream, wrong=wrong):
                                result = self.copy_case(work, args, bindings, candidate)
                                self.assertEqual(result[0], 'failed', result)
                        shifted = args[:-1] + ['-output_ts_offset', '1', 'OUTPUT']
                        self.assertEqual(self.copy_case(work, args, bindings, shifted)[0], 'failed')

    def test_byte_different_audio_passes_but_one_wrong_sample_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = pathlib.Path(tmp)
            left, right = work / 'left.wav', work / 'right.wav'
            self.wav(left, [1, 2, 3], junk=True)
            self.wav(right, [1, 2, 3])
            self.assertNotEqual(left.read_bytes(), right.read_bytes())
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work)[0], 'verified')
            self.wav(left, [1, 2, 4], junk=True)
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work),
                             ('failed', 'decoded samples'))

    def test_copied_audio_with_only_an_interior_timestamp_change_fails(self):
        for video, audio in (('libx264', 'aac'), ('libvpx-vp9', 'libopus')):
            with self.subTest(audio=audio), tempfile.TemporaryDirectory() as tmp:
                work = pathlib.Path(tmp)
                bindings = self.copy_sources(work, video, audio)
                candidate, reference, scratch = (work / n for n in ('candidate', 'reference', 'scratch'))
                for directory in (candidate, reference, scratch):
                    directory.mkdir()
                args = ['-i', 'INPUT', '-map', '0:1', '-c', 'copy', '-f', 'matroska', 'OUTPUT']
                shift = 'setts=pts=PTS+if(eq(N\\,4)\\,1\\,0):dts=DTS+if(eq(N\\,4)\\,1\\,0)'
                changed = args[:-1] + ['-bsf:a', shift, 'OUTPUT']
                argv = [str(candidate / 'output.mp4') if a == 'OUTPUT' else
                        str(bindings[a]) if a in bindings else a for a in changed]
                code, _, err = decoded_output.invoke([self.ffmpeg, '-nostdin', '-v', 'error', *argv], self.env)
                self.assertEqual(code, 0, err)
                self.assertEqual(current_ci.verify(args, bindings, candidate, reference, self.env,
                                                    self.ffmpeg, self.ffprobe, scratch),
                                 ('failed', 'audio frame timing or count'))
                listings, samples = [], []
                for directory in (candidate, reference):
                    path = directory / 'output.mp4'
                    listing = decoded_output.probe(path, self.ffprobe, self.env)
                    self.assertIsNotNone(listing)
                    self.assertNotIn('duration_ts', listing['streams'][0])
                    listings.append(listing)
                    code, data, err = decoded_output.invoke(
                        [self.ffmpeg, '-nostdin', '-v', 'error', *decoded_output.AUDIO_DECODE_INPUT,
                         '-i', str(path), '-map', '0:0', '-c:a', 'pcm_f64le', '-f', 'f64le', '-'], self.env)
                    self.assertEqual((code, err), (0, b''))
                    self.assertTrue(data)
                    samples.append(data)
                self.assertTrue(decoded_output.metadata(listings[0]['streams'][0], listings[1]['streams'][0]))
                self.assertNotEqual([f['best_effort_timestamp'] for f in listings[0]['frames']],
                                    [f['best_effort_timestamp'] for f in listings[1]['frames']])
                self.assertEqual(samples[0], samples[1])

    def test_raw_audio_success_is_verified_but_one_wrong_sample_fails(self):
        for channels in (1, 2):
            with self.subTest(channels=channels), tempfile.TemporaryDirectory() as tmp:
                work = pathlib.Path(tmp)
                candidate, reference, scratch = (work / name for name in ('candidate', 'reference', 'scratch'))
                for directory in (candidate, reference, scratch):
                    directory.mkdir()
                source = work / 'input.wav'
                samples = [1, 2, 3, 4, 5, 6, 7, 8]
                pcm = struct.pack('<8h', *samples)
                with wave.open(str(source), 'wb') as writer:
                    writer.setparams((channels, 2, 48000, 0, 'NONE', 'not compressed'))
                    writer.writeframes(pcm)
                output = candidate / 'output.mp4'
                output.write_bytes(pcm)
                args = ['-i', 'INPUT', '-f', 's16le', '-acodec', 'pcm_s16le', 'OUTPUT']
                self.assertEqual(current_ci.verify(args, {'INPUT': source}, candidate, reference, self.env,
                                                    self.ffmpeg, self.ffprobe, scratch)[0], 'verified')
                for path in reference.iterdir():
                    path.unlink()
                output.write_bytes(struct.pack('<8h', *samples[:-1], 9))
                self.assertEqual(current_ci.verify(args, {'INPUT': source}, candidate, reference, self.env,
                                                    self.ffmpeg, self.ffprobe, scratch),
                                 ('failed', 'decoded samples'))

    def test_byte_different_images_pass_but_one_wrong_pixel_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = pathlib.Path(tmp)
            left, right = work / 'left.png', work / 'right.png'
            self.png(left, b'\x01\x02\x03', 0)
            self.png(right, b'\x01\x02\x03', 9)
            self.assertNotEqual(left.read_bytes(), right.read_bytes())
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work)[0], 'verified')
            self.png(left, b'\x01\x02\x04', 0)
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work),
                             ('failed', 'decoded pixels'))


if __name__ == '__main__':
    unittest.main()
