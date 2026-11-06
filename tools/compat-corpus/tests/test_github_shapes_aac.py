"""Independent generated controls, never the judged fixture generator."""
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import github_shapes as run
import github_shapes_aac as aac
import github_shapes_inputs as inputs
import github_shapes_set as roster
import aac_pcm_eligibility as qualification
import test_oracle


SOURCE = {'streams': [{'codec_type': 'audio', 'codec_name': 'aac', 'sample_rate': '48000', 'channels': 1}]}


def command(form='f32le', options=(), pipe=False):
    name = '-' if pipe else 'control.' + form
    opts = ('-f', form, *options)
    return inputs.Command('ffmpeg', ('-i', 'in0/control.m4a', *opts, name),
                          (('in0/control.m4a', 'control.m4a'),), (name,), (opts,))


def wave(pcm, info=b''):
    body = b'WAVEfmt ' + struct.pack('<IHHIIHH', 16, 1, 1, 48000, 96000, 2, 16)
    body += info + b'data' + struct.pack('<I', len(pcm)) + pcm
    return b'RIFF' + struct.pack('<I', len(body)) + body


class ScopeAndAccounting(unittest.TestCase):
    def test_pure_roster_scope_without_executing_any_representative(self):
        found = []
        for row in roster.load():
            try:
                value = inputs.instantiate(row['shape'])
            except inputs.Uninstantiable:
                continue
            if aac.output_format(value, SOURCE):
                found.append(row['rank'])
        self.assertEqual(found, [5, 13, 33, 43, 53, 57, 67, 72, 77, 89, 98, 139, 168, 174])

    def test_unsupported_operations_and_changed_rate_or_channels_do_not_qualify(self):
        for options in (('-ar', '44100'), ('-ac', '2'), ('-af', 'volume=0.5'),
                        ('-ss', '0.1'), ('-c:a', 'aac'), ('-f', 'wav')):
            self.assertIsNone(aac.output_format(command(options=options), SOURCE))
        for form in ('mp4', 'mp3', 'f64le'):
            self.assertIsNone(aac.output_format(command(form), SOURCE))
        self.assertIsNone(aac.output_format(command('wav', ('-c:a', 'pcm_f32le')), SOURCE))
        value = command()
        self.assertIsNone(aac.output_format(inputs.Command(value.tool, value.args, value.sources,
            value.outputs * 2, value.output_options * 2), SOURCE))
        self.assertIsNone(aac.output_format(inputs.Command(value.tool, value.args, value.sources,
            value.outputs, value.output_options, 'in0/control.m4a'), SOURCE))
        self.assertEqual(aac.output_format(command(options=('-ar', '48000', '-ac', '1')), SOURCE), 'f32le')

    def test_compatibility_never_increases_either_exact_share(self):
        rows = [{'rank': i, 'repository_count': 1271 if i == 200 else 1,
                 'shape': {'tool': 'ffmpeg'}} for i in range(1, 201)]
        outcomes = [{'rank': i, 'outcome': 'counted' if i == 1 else 'different'} for i in range(1, 201)]
        before = run.summarize(rows, outcomes)
        outcomes[1]['outcome'] = aac.COMPATIBLE
        outcomes[-1]['outcome'] = aac.COMPATIBLE
        outcomes[2]['outcome'] = run.COMPATIBLE
        after = run.summarize(rows, outcomes)
        for key in ('counted', 'counted_pairs', 'top_200_pair_share', 'whole_sample_presence_lower_bound'):
            self.assertEqual(before[key], after[key])
        self.assertEqual((after[aac.COMPATIBLE], after[aac.COMPATIBLE + '_pairs']), (2, 1272))
        self.assertEqual(after['tools']['ffmpeg']['shapes'][aac.COMPATIBLE], 2)
        self.assertEqual(sum(after['outcomes'].values()), 200)
        self.assertEqual(sum(after['outcome_pairs'].values()), 1470)


class OutputBinding(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for arm in ('reference', 'candidate'):
            folder = self.root / arm
            folder.mkdir()
            (folder / 'stdout').write_bytes(b'')
            (folder / 'command.stderr').write_text('Stream #0:0 -> #0:0 (aac (native) -> pcm_f32le (native))\n')
        self.capture = {'f32': (struct.pack('<ff', 0.0, 0.1), struct.pack('<ff', 0.0, 0.10000001)),
                        's16': (b'\0\0\x01\0', b'\0\0\x02\0')}
        self.metadata = {'schema': 'coreav.aac-pns-diagnostic/1', 'packets': 1, 'channels': 1,
                         'unsafe': [], 'frames': [{'channels': [{'noise': []}]}]}

    def scan(self, args):
        (Path(args[-1]) / 'metadata.json').write_text(json.dumps(self.metadata))
        return types.SimpleNamespace(returncode=0)

    def qualify(self, coreav, oracle, source, work, metadata, pns, capture):
        self.assertIs(oracle, self.oracle)
        capture.update(self.capture)
        return {'status': 'pass', 'channels': 1, 'rate': 48000}

    def check(self, value=None, verdict=('failed', 'decoded samples')):
        value = value or command()
        codec = 'pcm_f32le' if '-f' in value.args and value.args[value.args.index('-f') + 1] == 'f32le' else 'pcm_s16le'
        self.oracle = types.SimpleNamespace(ffmpeg=Path('/qualified/ffmpeg'), ffprobe=Path('/qualified/ffprobe'))
        with mock.patch.object(qualification, 'run', side_effect=self.scan), \
             mock.patch.object(qualification.test_oracle, 'require', return_value=self.oracle) as owner, \
             mock.patch.object(qualification, 'check_output', side_effect=self.qualify), \
             mock.patch.object(qualification, 'pns_check', return_value='qualified PNS'):
            result = aac.compare(value, self.root, SOURCE, {(0, 0): codec}, verdict,
                {'$AAC_DIAGNOSTIC': 'diagnostic', '$COREAV': 'coreav', '$FFMPEG': '/oracle/ffmpeg',
                 '$FFPROBE': '/oracle/ffprobe'}, self.root)
        for call in owner.call_args_list:
            self.assertEqual(call, mock.call('/oracle/ffmpeg', '/oracle/ffprobe'))
        return result

    def outputs(self, value):
        form = value.output_options[0][1]
        fmt = 'f32' if form == 'f32le' else 's16'
        name = 'stdout' if value.outputs[0] == '-' else value.outputs[0]
        codec = 'pcm_f32le' if fmt == 'f32' else 'pcm_s16le'
        for arm, data in zip(('candidate', 'reference'), self.capture[fmt]):
            (self.root / arm / name).write_bytes(wave(data) if form == 'wav' else data)
            (self.root / arm / 'command.stderr').write_text(f'Stream #0:0 -> #0:0 (aac (native) -> {codec} (native))\n')
        return name

    def test_each_output_is_bound_to_its_own_qualified_decoder_bytes(self):
        for form in ('f32le', 's16le', 'wav'):
            for pipe in (False, True):
                value = command(form, pipe=pipe)
                name = self.outputs(value)
                self.assertEqual(self.check(value), aac.COMPATIBLE)
                for arm in ('candidate', 'reference'):
                    path = self.root / arm / name
                    original = path.read_bytes()
                    for mutated in (original[:-1], original + b'\0', original[:-1] + b'\x7f'):
                        path.write_bytes(mutated)
                        self.assertEqual(self.check(value), 'different')
                    path.write_bytes(original)
                if pipe:
                    for arm in ('candidate', 'reference'):
                        (self.root / arm / 'stdout').write_bytes(b'')

    def test_wav_metadata_and_file_stdout_are_not_ignored(self):
        value = command('wav')
        name = self.outputs(value)
        path = self.root / 'candidate' / name
        path.write_bytes(wave(self.capture['s16'][0], b'LIST\x04\0\0\0INFO'))
        self.assertEqual(self.check(value), 'different')
        self.outputs(value)
        (self.root / 'candidate/stdout').write_bytes(b'unexpected')
        self.assertEqual(self.check(value), 'different')

    def test_exact_no_pns_stays_exact_but_equal_pns_never_does(self):
        value = command()
        self.outputs(value)
        exact = ('verified', 'exact decoded output')
        self.assertEqual(self.check(value, exact), 'counted')
        self.metadata['frames'][0]['channels'][0]['noise'] = [{'coded': 'noise'}]
        self.assertEqual(self.check(value, exact), aac.COMPATIBLE)
        self.metadata['unsafe'] = ['tns_pns']
        self.assertEqual(self.check(value, exact), 'different')

    def test_failed_or_unavailable_qualification_cannot_create_exact_or_no_oracle(self):
        self.outputs(command())
        for error in (ValueError('energy'), OSError('missing'), subprocess.TimeoutExpired('diagnostic', 1)):
            with mock.patch.object(qualification, 'check_output', side_effect=error), \
                 mock.patch.object(qualification, 'run', side_effect=self.scan):
                self.assertEqual(aac.compare(command(), self.root, SOURCE, {(0, 0): 'pcm_f32le'},
                    ('failed', 'decoded samples'), {'$AAC_DIAGNOSTIC': 'd', '$FFMPEG': '/o/ffmpeg', '$COREAV': 'c'},
                    self.root), 'different')
        for reason in ('stream fields or timing', 'decoded pixels', 'invalid audio timing'):
            self.assertEqual(self.check(verdict=('failed', reason)), 'different')

    def test_wrong_decoder_and_extra_mapping_do_not_qualify(self):
        self.outputs(command())
        (self.root / 'reference/command.stderr').write_text('Stream #0:0 -> #0:0 (mp3 (native) -> pcm_f32le (native))\n')
        self.assertEqual(self.check(), 'different')
        self.assertFalse(aac.decodes_aac(self.root / 'reference'))

    def test_compatibility_cannot_waive_output_fields_or_frame_timing(self):
        value = command()
        self.outputs(value)
        programs = {'$FFMPEG': 'ffmpeg', '$FFPROBE': 'ffprobe'}
        for failure in ('invalid audio timing', 'stream fields or timing', 'decode or incomplete samples'):
            with mock.patch.object(run.decoded_output, 'probe', return_value=SOURCE), \
                 mock.patch.object(run.decoded_output, 'compare', side_effect=[('failed', 'decoded samples'),
                                                                             ('failed', failure)]), \
                 mock.patch.object(aac, 'compare', return_value=aac.COMPATIBLE):
                self.assertEqual(run.compare_outputs(value, self.root, programs, {}, self.root), 'different')

    def test_no_pns_scan_guard_also_covers_encoded_outputs_and_fails_closed(self):
        programs = {'$AAC_DIAGNOSTIC': 'diagnostic'}
        with mock.patch.object(qualification, 'run', side_effect=self.scan):
            self.assertTrue(aac.exact_without_pns(command(), self.root, SOURCE, programs, self.root))
            self.metadata['frames'][0]['channels'][0]['noise'] = [{}]
            self.assertFalse(aac.exact_without_pns(command(), self.root, SOURCE, programs, self.root))
            self.metadata['frames'] = []
            self.assertFalse(aac.exact_without_pns(command(), self.root, SOURCE, programs, self.root))
        for mapping in ('aac (native) -> aac (native)', 'aac (native) -> mp3 (libmp3lame)',
                        'aac (native) -> opus (libopus)'):
            (self.root / 'reference/command.stderr').write_text(f'Stream #0:0 -> #0:0 ({mapping})\n')
            self.assertTrue(aac.decodes_aac(self.root / 'reference'))
        (self.root / 'reference/command.stderr').write_text('Stream #0:0 -> #0:0 (copy)\n')
        self.assertFalse(aac.decodes_aac(self.root / 'reference'))

    def test_aac_scan_does_not_bypass_a_sibling_copied_stream_packet_gate(self):
        value = command()
        self.outputs(value)
        (self.root / 'reference/command.stderr').write_text(
            'Stream #0:0 -> #0:0 (copy)\nStream #0:1 -> #0:1 (aac (native) -> pcm_f32le (native))\n')
        programs = {'$FFMPEG': 'ffmpeg', '$FFPROBE': 'ffprobe'}
        with mock.patch.object(run.decoded_output, 'probe', return_value=SOURCE), \
             mock.patch.object(run.decoded_output, 'compare', return_value=('verified', 'exact decoded output')), \
             mock.patch.object(aac, 'exact_without_pns', return_value=True), \
             mock.patch.object(aac, 'compare') as compatible, \
             mock.patch.object(run, 'packet_rows', side_effect=[[(0, 'changed')], [(0, 'original')]]), \
             mock.patch.object(run.decoded_output, 'avc1_carriage', return_value=False):
            self.assertEqual(run.compare_outputs(value, self.root, programs, {}, self.root), 'different')
            compatible.assert_not_called()

    def test_encoded_pns_cannot_receive_exact_credit(self):
        value = command()
        self.outputs(value)
        (self.root / 'reference/command.stderr').write_text('Stream #0:0 -> #0:0 (aac (native) -> opus (libopus))\n')
        with mock.patch.object(run.decoded_output, 'probe', return_value=SOURCE), \
             mock.patch.object(run.decoded_output, 'compare', return_value=('verified', 'exact decoded output')), \
             mock.patch.object(aac, 'exact_without_pns', return_value=False), \
             mock.patch.object(aac, 'compare') as compatible:
            self.assertEqual(run.compare_outputs(value, self.root, {'$FFMPEG': 'f', '$FFPROBE': 'p'}, {}, self.root), 'different')
            compatible.assert_not_called()


@unittest.skipUnless(os.environ.get('COREAV_SHAPES_COREAV'), 'AAC command control binaries not configured')
class PinnedAacControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.oracle = test_oracle.require(ffmpeg=str(Path(os.environ['COREAV_SHAPES_ORACLE']) / 'ffmpeg'))
        cls.coreav = Path(os.environ['COREAV_SHAPES_COREAV']).resolve()
        cls.diagnostic = Path(os.environ['COREAV_SHAPES_AAC_DIAGNOSTIC']).resolve()
        cls.tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.root = Path(cls.tmp.name)
        cls.tone = cls.root / 'tone.m4a'
        cls.call([cls.oracle.ffmpeg, '-v', 'error', '-f', 'lavfi', '-i',
            'sine=frequency=673:sample_rate=44100:duration=0.0928798185941043', '-c:a', 'aac', '-aac_pns', '0', cls.tone])
        packets = cls.root / 'packets'
        packets.mkdir()
        cls.call([cls.diagnostic, '--fixtures', packets])
        for source in packets.glob('*.aac'):
            cls.call([cls.oracle.ffmpeg, '-v', 'error', '-f', 'aac', '-i', source,
                      '-c:a', 'copy', cls.root / (source.stem + '.m4a')])

    @staticmethod
    def call(args):
        result = subprocess.run(list(map(str, args)), capture_output=True, timeout=120)
        if result.returncode:
            raise AssertionError(result.stderr.decode(errors='replace')[-1000:])
        return result

    def capture(self, source, form, pipe=False):
        temporary = tempfile.TemporaryDirectory(dir=self.root)
        self.addCleanup(temporary.cleanup)
        base = Path(temporary.name)
        value = command(form, pipe=pipe)
        for arm in ('reference', 'candidate'):
            folder = base / arm
            (folder / 'in0').mkdir(parents=True)
            shutil.copyfile(source, folder / 'in0/control.m4a')
            argv = run.expected_invocation(value, arm == 'candidate')
            programs = {'$COREAV': str(self.coreav), '$FFMPEG': str(self.oracle.ffmpeg),
                        '$FFPROBE': str(self.oracle.ffprobe), '$AAC_DIAGNOSTIC': str(self.diagnostic)}
            env = run.environment()
            env.update(COREAV_FALLBACK='/nonexistent/no-fallback', COREAV_STRICT_SAFE='1')
            receipt = run.execute([programs.get(arg, arg) for arg in argv], folder, env, 120, 'command')
            if receipt['code']:
                raise AssertionError((folder / 'command.stderr').read_text()[-1000:])
            (folder / 'stdout').write_bytes((folder / 'command.stdout').read_bytes())
        return value, base, programs

    def test_real_no_pns_and_pns_raw_and_wav_file_and_stdout(self):
        for source in (self.tone, self.root / 'pns-independent.m4a', self.root / 'pns-independent-stereo.m4a'):
            for form in ('f32le', 's16le', 'wav'):
                for pipe in (False, True):
                    with self.subTest(source=source.name, form=form, pipe=pipe):
                        value, base, programs = self.capture(source, form, pipe)
                        name = 'stdout' if pipe else value.outputs[0]
                        path = base / 'candidate' / name
                        expected = (base / 'reference' / name).read_bytes()
                        actual = path.read_bytes()
                        if form == 'wav':
                            channels = 2 if 'stereo' in source.name else 1
                            actual = qualification.wav(actual, 's16', channels, 44100)
                            expected = qualification.wav(expected, 's16', channels, 44100)
                        outcome = 'counted' if source == self.tone and actual == expected else aac.COMPATIBLE
                        self.assertEqual(run.compare_outputs(value, base, programs, run.environment(), base), outcome)
                        path.write_bytes(path.read_bytes()[:-1])
                        self.assertEqual(run.compare_outputs(value, base, programs, run.environment(), base), 'different')

    def test_generated_excluded_pns_tools_refuse_and_cannot_qualify(self):
        for name in ('pns-tns', 'pns-ms', 'pns-illegal-window'):
            source = self.root / (name + '.m4a')
            result = subprocess.run(list(map(str, qualification.native(self.coreav, source,
                ['-f', 's16le', '-']))), capture_output=True, timeout=120)
            self.assertEqual((result.returncode, result.stdout), (4, b''))
            with tempfile.TemporaryDirectory(dir=self.root) as temporary:
                row = qualification.check(self.coreav, self.oracle, source,
                    Path(temporary), self.diagnostic, {})
                self.assertEqual(row['status'], 'ineligible')

    def test_capture_exports_both_complete_qualified_arm_payloads(self):
        source = self.root / 'pns-independent.m4a'
        with tempfile.TemporaryDirectory(dir=self.root) as temporary:
            capture = {}
            row = qualification.check(self.coreav, self.oracle, source,
                                      Path(temporary), self.diagnostic, capture)
            self.assertEqual(row['status'], 'pass')
            self.assertIn('pns', row)
            for fmt, codec in (('f32', 'pcm_f32le'), ('s16', 'pcm_s16le')):
                actual = self.call(qualification.native(self.coreav, source, ['-c:a', codec, '-f', fmt + 'le', '-']))
                reference = self.call([self.oracle.ffmpeg, '-v', 'error', *qualification.AUDIO_DECODE_INPUT,
                    '-i', source, '-c:a', codec, '-f', fmt + 'le', '-'])
                self.assertEqual(capture[fmt], (actual.stdout, reference.stdout))


if __name__ == '__main__':
    unittest.main()
