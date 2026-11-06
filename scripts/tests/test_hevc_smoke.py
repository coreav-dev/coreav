import argparse
import copy
import hashlib
import importlib.util
import json
import pathlib
import tempfile
import types
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('hevc_smoke', ROOT / 'scripts/hevc_smoke.py')
smoke = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(smoke)


class Smoke(unittest.TestCase):
    def execute(self, mutation=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / 'stream.hevc'
            source.write_bytes(b'synthetic-placeholder')
            recipes = smoke.fixture.recipes()
            records = [dict(test='SYNTHETIC_' + recipe['id']) for recipe in recipes]
            census = dict(complete=True, streams=copy.deepcopy(records))
            if mutation in ('missing-recipe', 'extra-recipe', 'duplicate-recipe'):
                if mutation == 'missing-recipe':
                    census['streams'].pop()
                elif mutation == 'extra-recipe':
                    census['streams'].append(dict(test='extra'))
                else:
                    census['streams'][1] = census['streams'][0]
            reference = dict(bytes=6, md5=hashlib.md5(b'\0' * 6).hexdigest())
            streams = [dict(test=record['test'], source=source,
                            record=dict(sha256=smoke.harness.digest(source)),
                            targets=[dict(candidate=dict(picture_index=index,
                                         reference=dict(reference, output_index=index))) for index in (0, 1)])
                       for record in records]
            if mutation == 'missing-picture':
                streams[0]['targets'].pop()
            elif mutation == 'extra-picture':
                streams[0]['targets'].append(dict(candidate=dict(picture_index=2)))
            elif mutation == 'duplicate-picture':
                streams[0]['targets'][1] = streams[0]['targets'][0]
            elif mutation == 'hash':
                streams[0]['record']['sha256'] = '0' * 64
            args = argparse.Namespace(data=root, out=root / 'out', parser=root / 'headers',
                                      native=root / 'intra', oracle_bin=root / 'oracle')
            commands = []
            paths = []
            def child(command, directory, label, **kwargs):
                commands.append((command, kwargs))
                directory.mkdir(parents=True, exist_ok=True)
                stdout = directory / (label + '.stdout')
                stdout.write_text('{}\n')
                command[3].write_bytes(b'raw-placeholder')
                if mutation == 'pixels':
                    stdout.write_text(json.dumps(dict(coded_width=2, coded_height=2, width=2,
                        height=2, bit_depth=8, conformance_window=[0] * 4)))
                    command[3].write_bytes(b'\x01\x00' * 6)
                paths.append(directory)
                failed = mutation == 'native' or mutation == 'late-native' and len(commands) == 3
                return (1 if failed else 0), stdout, directory / 'stderr'
            qualified = types.SimpleNamespace(ffmpeg=root / 'qualified-ffmpeg', ffprobe=root / 'qualified-ffprobe')
            validate_metadata = smoke.intra.validate_metadata
            compare_picture = smoke.intra.compare_picture
            with mock.patch.object(smoke.sys, 'platform', 'linux'), \
                 mock.patch.object(smoke.harness.test_oracle, 'require', return_value=qualified) as owner, \
                 mock.patch.object(smoke.synthetic, 'inspect', return_value=census) as inspect, \
                 mock.patch.object(smoke.intra, 'stream_targets', side_effect=streams), \
                 mock.patch.object(smoke.intra, 'oracle_frames', return_value=[reference, reference]) as frames, \
                 mock.patch.object(smoke.harness, 'child', side_effect=child), \
                 mock.patch.object(smoke.intra, 'validate_metadata') as metadata, \
                 mock.patch.object(smoke.intra, 'compare_picture', return_value={'bytes': 1}) as compare:
                if mutation == 'inspection':
                    inspect.side_effect = ValueError('PRIVATE_MARKER')
                if mutation == 'qualification':
                    owner.side_effect = smoke.harness.test_oracle.QualificationError('PRIVATE_MARKER')
                if mutation == 'metadata':
                    metadata.side_effect = validate_metadata
                if mutation == 'pixels':
                    compare.side_effect = compare_picture
                result = smoke.run(args)
            text = (args.out / 'result.json').read_text()
            self.assertEqual(json.loads(text), result)
            self.assertNotIn('PRIVATE_MARKER', text)
            self.assertTrue(all(not path.exists() for path in paths))
            owner.assert_called_once_with(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
            self.selection = (qualified, inspect.call_args_list, frames.call_args_list)
            return result, commands

    def test_exact_eight_complete_identities_and_bounded_commands(self):
        result, commands = self.execute()
        self.assertTrue(result['complete'])
        self.assertEqual(len(result['results']), 8)
        self.assertEqual(len(commands), 8)
        for command, kwargs in commands:
            self.assertEqual(command[-2:], [str(256 << 20), str(1 << 28)])
            self.assertEqual(kwargs, dict(file_limit=256 << 20))

    def test_inspection_and_oracle_frames_take_only_the_owner_returned_pair(self):
        result, _ = self.execute()
        self.assertTrue(result['complete'])
        qualified, inspections, frames = self.selection
        self.assertEqual([call.args[3] for call in inspections], [qualified])
        self.assertEqual(len(frames), 4)
        self.assertTrue(all(call.args[0] == qualified.ffmpeg for call in frames))
        result, commands = self.execute('qualification')
        self.assertEqual((result['stage'], result['reason'], commands), ('inspection', smoke.REASONS['inspection'], []))
        self.assertEqual(self.selection[1], [])

    def test_missing_extra_and_duplicate_membership_never_launch_native(self):
        for mutation in ('missing-recipe', 'extra-recipe', 'duplicate-recipe',
                         'missing-picture', 'extra-picture', 'duplicate-picture'):
            with self.subTest(mutation=mutation):
                result, commands = self.execute(mutation)
                self.assertFalse(result['complete'])
                self.assertEqual(commands, [])

    def test_failures_keep_closed_partial_report_and_clean_raw_outputs(self):
        for mutation, stage in (('inspection', 'inspection'), ('hash', 'identity'),
                                ('native', 'native'), ('metadata', 'metadata'), ('pixels', 'comparison')):
            with self.subTest(mutation=mutation):
                result, _ = self.execute(mutation)
                self.assertFalse(result['complete'])
                self.assertEqual(result['stage'], stage)
                self.assertEqual(result['reason'], smoke.REASONS[stage])
        result, _ = self.execute('late-native')
        self.assertFalse(result['complete'])
        self.assertEqual(len(result['results']), 2)

    def test_non_linux_refuses_before_any_child_boundary(self):
        with tempfile.TemporaryDirectory() as temporary:
            args = argparse.Namespace(out=pathlib.Path(temporary))
            with mock.patch.object(smoke.sys, 'platform', 'darwin'), \
                 mock.patch.object(smoke.synthetic, 'inspect') as inspect, \
                 mock.patch.object(smoke.harness, 'child') as child:
                result = smoke.run(args)
            self.assertFalse(result['complete'])
            self.assertEqual(result['reason'], 'linux_required')
            inspect.assert_not_called()
            child.assert_not_called()


if __name__ == '__main__':
    unittest.main()
