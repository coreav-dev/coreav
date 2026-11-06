import json
import os
from pathlib import Path
import sys
import tempfile
import traceback
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ffv1_check as ffv1
import h264_deblock_conformance as deblock
import h264_g1 as measure
import h264_g1_conformance as g1
import h264_p_conformance as p
import svt_av1_check as svt
import test_oracle
import transcode_decoder_host as host
import v2_fixture_check as v2


PAIR = test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified-probe/ffprobe'))


class Halt(RuntimeError):
    pass


class Selection(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()

    def test_ffv1_executes_owner_pair(self):
        with patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(ffv1, 'run') as run:
            self.assertEqual(ffv1.verify_oracles(self.root / 'ffmpeg', self.root / 'ffprobe'),
                             (PAIR.ffmpeg, PAIR.ffprobe))
        qualify.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ffprobe')
        self.assertEqual([call.args[0][0] for call in run.call_args_list], [PAIR.ffmpeg, PAIR.ffprobe])

    def test_ffv1_pin_failure_precedes_execution(self):
        with patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('bad pin')), \
                patch.object(ffv1, 'run') as run:
            with self.assertRaisesRegex(ffv1.GateError, 'bad pin'):
                ffv1.verify_oracles(self.root / 'ffmpeg', self.root / 'ffprobe')
        run.assert_not_called()

    def test_svt_executes_owner_path(self):
        with patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(svt, 'run', side_effect=Halt) as run:
            with self.assertRaises(Halt):
                svt.check(self.root / 'coreav', self.root, self.root / 'work')
        qualify.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ffprobe')
        self.assertEqual(run.call_args.args[0][0], PAIR.ffmpeg)

    def v2_args(self):
        return ['v2_fixture_check.py', '--ffmpeg', str(self.root / 'ffmpeg'),
                '--fixtures', str(self.root), '--work', str(self.root / 'work'),
                '--report', str(self.root / 'report.json')]

    def test_v2_hands_owner_path_to_verification(self):
        with patch.object(sys, 'argv', self.v2_args()), \
                patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(v2, 'verify', side_effect=Halt) as verify:
            with self.assertRaises(Halt):
                v2.main()
        qualify.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ffprobe')
        self.assertEqual(verify.call_args.args[0].ffmpeg, PAIR.ffmpeg)

    def test_v2_pin_failure_precedes_output(self):
        with patch.object(sys, 'argv', self.v2_args()), \
                patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('bad pin')), \
                patch.object(v2, 'verify') as verify:
            with self.assertRaisesRegex(ValueError, 'reference fixture verification requires the pinned oracle: bad pin'):
                v2.main()
        verify.assert_not_called()
        self.assertFalse((self.root / 'work').exists())
        self.assertFalse((self.root / 'report.json').exists())

    def conformance_args(self, module):
        args = ['--driver', str(self.root / 'driver'), '--ffmpeg', str(self.root / 'ffmpeg'),
                '--jm', str(self.root / 'jm'), '--output', str(self.root / 'work')]
        if module is p:
            args += ['--decoder', str(self.root / 'decoder'), '--forced-fixtures', str(self.root / 'fixtures')]
        return args

    def test_g1_hands_owner_pair_to_compare(self):
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(sys, 'argv', ['g1', *self.conformance_args(g1)]), \
                patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(g1, 'digest', return_value='hash'), \
                patch.object(g1, 'compare', side_effect=Halt) as compare:
            with self.assertRaises(Halt):
                g1.main()
        qualify.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ffprobe')
        self.assertEqual(compare.call_args.args[1], PAIR)

    def test_g1_pin_failure_precedes_output(self):
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(sys, 'argv', ['g1', *self.conformance_args(g1)]), \
                patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('bad pin')), \
                patch.object(g1, 'compare') as compare:
            with self.assertRaisesRegex(ValueError, 'bad pin'):
                g1.main()
        compare.assert_not_called()
        self.assertFalse((self.root / 'work').exists())

    def test_deblock_hands_owner_pair_to_compare(self):
        def digest(path):
            return deblock.BASELINE['input_sha256']

        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(g1, 'digest', side_effect=digest), \
                patch.object(g1, 'compare', side_effect=Halt) as compare:
            with self.assertRaises(Halt):
                deblock.main(self.conformance_args(deblock))
        qualify.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ffprobe')
        self.assertEqual(compare.call_args.args[1], PAIR)

    def test_deblock_pin_failure_precedes_generated_output(self):
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('bad pin')), \
                patch.object(g1, 'compare') as compare:
            with self.assertRaisesRegex(ValueError, 'bad pin'):
                deblock.main(self.conformance_args(deblock))
        compare.assert_not_called()
        self.assertEqual(list((self.root / 'work').glob('*.yuv')), [])
        self.assertFalse(json.loads((self.root / 'work/report.json').read_text())['complete'])

    def test_p_verification_returns_owner_pair(self):
        for name in ('driver', 'ffmpeg', 'ffprobe', 'jm', 'decoder'):
            path = self.root / name
            path.write_bytes(b'synthetic executable')
            path.chmod(0o755)
        pin = dict(line.split('=', 1) for line in (p.ROOT / 'scripts/jm-decoder.env').read_text().splitlines() if line)
        (self.root / 'jm-build.txt').write_text(
            f'JM_VERSION={pin["JM_VERSION"]}\nJM_SHA256={pin["JM_SHA256"]}\n'
            "BUILD_COMMAND=make -C JM/ldecod -j2 CC='cc -fcommon'\n"
            'hash  /synthetic/ldecod\n')
        with patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(g1, 'digest', return_value='hash') as digest:
            tools, report = p.verify_tools(self.root / 'driver', self.root / 'ffmpeg',
                                           self.root / 'jm', self.root / 'decoder')
        qualify.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ffprobe')
        self.assertIs(tools, PAIR)
        self.assertEqual(set(report['executables']), {'driver', 'ffmpeg', 'ffprobe', 'jm', 'decoder'})
        self.assertIn(PAIR.ffmpeg, [call.args[0] for call in digest.call_args_list])
        self.assertIn(PAIR.ffprobe, [call.args[0] for call in digest.call_args_list])

    def test_p_pin_failure_precedes_generated_output(self):
        for name in ('driver', 'ffmpeg', 'ffprobe', 'jm', 'decoder'):
            path = self.root / name
            path.write_bytes(b'synthetic executable')
            path.chmod(0o755)
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('bad pin')), \
                patch.object(p, 'compare') as compare, patch.object(p, 'forced_inventory') as inventory:
            with self.assertRaisesRegex(ValueError, 'bad pin'):
                p.main(self.conformance_args(p))
        compare.assert_not_called()
        inventory.assert_not_called()
        self.assertEqual(list((self.root / 'work').glob('*.yuv')), [])
        self.assertFalse(json.loads((self.root / 'work/report.json').read_text())['complete'])

    def test_p_hands_owner_pair_to_compare(self):
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(test_oracle, 'require', return_value=PAIR), \
                patch.object(p, 'verify_tools', return_value=(PAIR, {})), \
                patch.object(p, 'forced_inventory', return_value={}), \
                patch.object(p, 'compare', side_effect=Halt) as compare:
            with self.assertRaises(Halt):
                p.main(self.conformance_args(p))
        self.assertEqual(compare.call_args.args[1], PAIR)

    def test_host_smoke_executes_and_hands_on_owner_pair(self):
        args = SimpleNamespace(tools=str(self.root), tmp=str(self.root / 'work'))
        process = host.subprocess.CompletedProcess([], 0, b'', b'')
        with patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(host.Executor, 'call', return_value=(1, process)) as execute, \
                patch.object(host, 'digest', return_value='hash'), \
                patch.object(host, 'preflight', side_effect=Halt) as preflight:
            with self.assertRaises(Halt):
                host.smoke(args)
        qualify.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ffprobe')
        self.assertEqual(execute.call_args.args[0][0], PAIR.ffmpeg)
        self.assertEqual(preflight.call_args.args[3]['ffmpeg'], PAIR.ffmpeg)
        self.assertEqual(preflight.call_args.args[3]['ffprobe'], PAIR.ffprobe)

    def test_host_pin_failure_precedes_output_and_hides_details(self):
        args = SimpleNamespace(tools=str(self.root), tmp=str(self.root / 'work'))
        with patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('private marker')), \
                patch.object(host.Executor, 'call') as execute:
            with self.assertRaisesRegex(ValueError, '^oracle pin verification failed$') as error:
                host.smoke(args)
        self.assertNotIn('private marker', ''.join(traceback.format_exception(error.exception)))
        execute.assert_not_called()
        self.assertFalse((self.root / 'work').exists())

    def measure_args(self):
        tools = self.root / 'tools.json'
        tools.write_text(json.dumps({name: str(self.root / name) for name in ('ffmpeg', 'ffprobe', 'jm')}))
        return SimpleNamespace(authorize='a' * 40, candidate='a' * 40, identity='identity.json',
                               driver=self.root / 'driver', decoder=self.root / 'decoder', tools=tools,
                               inputs=self.root, output=self.root / 'work', freeze='freeze', sha256='hash',
                               metric_fixture=self.root, resume_fault=None)

    def measure_manifest(self):
        (self.root / 'clip.yuv').write_bytes(b'x')
        (self.root / 'reference.yuv').write_bytes(b'x' * 360)
        (self.root / 'reconstruction.yuv').write_bytes(b'x' * 360)
        return {'tools': {name: {'sha256': 'hash'} for name in ('ffmpeg', 'ffprobe', 'jm')},
                'clips': [{'label': 'clip', 'bytes': 1, 'sha256': 'hash', 'width': 2, 'height': 2}],
                'metric_fixture': {'reference_sha256': 'hash', 'reconstruction_sha256': 'hash',
                                   'width': 2, 'height': 2, 'png_sha256': []}}

    def test_measure_hands_owner_pair_to_compare(self):
        args, manifest = self.measure_args(), self.measure_manifest()
        identity = json.dumps({'driver_sha256': 'a' * 64, 'decoder_sha256': 'a' * 64})
        def digest(path):
            return 'a' * 64 if Path(path).name in ('driver', 'decoder') else 'hash'
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(measure, 'git', return_value=identity), patch.object(measure, 'digest', side_effect=digest), \
                patch.object(test_oracle, 'require', return_value=PAIR) as qualify, \
                patch.object(measure, 'score'), patch.object(measure, 'resume_cell', return_value=None), \
                patch.object(measure, 'compare', side_effect=Halt) as compare:
            with self.assertRaises(Halt):
                measure.measure(args, manifest)
        qualify.assert_called_once_with(str(self.root / 'ffmpeg'), str(self.root / 'ffprobe'))
        self.assertEqual(compare.call_args.args[1], PAIR)

    def test_measure_pin_failure_precedes_output(self):
        args, manifest = self.measure_args(), self.measure_manifest()
        identity = json.dumps({'driver_sha256': 'a' * 64, 'decoder_sha256': 'a' * 64})
        def digest(path):
            return 'a' * 64 if Path(path).name in ('driver', 'decoder') else 'hash'
        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(sys, 'platform', 'linux'), \
                patch.object(measure, 'git', return_value=identity), patch.object(measure, 'digest', side_effect=digest), \
                patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('bad pin')), \
                patch.object(measure, 'compare') as compare:
            with self.assertRaisesRegex(ValueError, 'bad pin'):
                measure.measure(args, manifest)
        compare.assert_not_called()
        self.assertFalse(args.output.exists())


if __name__ == '__main__':
    unittest.main()
