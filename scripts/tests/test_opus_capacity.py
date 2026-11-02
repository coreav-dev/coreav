import copy
import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('opus_capacity', ROOT / 'bench/audio-decode/capacity.py')
capacity = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = capacity
SPEC.loader.exec_module(capacity)


def evidence():
    limits = {'per_call_seconds': 600, 'job_seconds': 7200, 'concurrency': 1,
              'memory_bytes': 4 << 30, 'tmpfs_bytes': 5 << 30}
    inputs = []
    rows = []
    frames = capacity.MIN_FULL_FRAMES
    for mode in ('celt', 'hybrid', 'silk'):
        for cls in ('mp4', 'matroska'):
            digest = hashlib.sha256(f'{mode}/{cls}'.encode()).hexdigest()
            inputs.append({'sha256': digest, 'class': cls, 'channels': 2,
                           'packets': {mode + '/20': 45000}, 'mode': mode})
            presented = frames - 312
            results = {
                'raw_identity': {'passed': True, 'packets': 45000, 'frames': frames, 'channels': 2},
                'cli_identity': {'passed': True, 'frames': presented, 'channels': 2,
                                 'sample_rate': 48000, 'sample_width': 2, 'output_bytes': presented * 4 + 44},
                'oracle': {'schema': 'opus-oracle/1', 'status': 'ok', 'passed': True, 'error': None,
                           'detail': {'samples': {'ours': presented, 'ref': presented, 'default': presented},
                                      'length_ok': True, 'default_length_ok': True,
                                      'compare_passed': True, 'quality': 100.0}},
                'head_lib': {'samples': presented, 'bytes': presented * 4},
                'base_lib': {'samples': presented, 'bytes': presented * 4},
                'lav_lib': {'samples': presented, 'bytes': presented * 4, 'errors': 0},
                'head_dec': {'samples': frames}, 'base_dec': {'samples': frames},
                'lav_dec': {'samples': frames - 312, 'packets': 45000, 'errors': 0},
                'ffmpeg_cli': {'frames': presented, 'channels': 2, 'sample_rate': 48000,
                               'sample_width': 2, 'output_bytes': presented * 4 + 44},
            }
            rows.append({'sha256': digest, 'class': cls, 'mode': mode,
                         'operations': {name: {'status': 'complete', 'result': value, 'error': None}
                                        for name, value in results.items()}})
    inputs.sort(key=lambda row: (row['sha256'], row['class']))
    source_names = (*capacity.SCRIPT_INPUTS, *(f'bench/audio-decode/{name}.py'
                      for name in ('capacity', 'run', 'receipts', 'oracle', 'agreement')))
    binding = {'schema': 'opus-capacity-binding/1', 'scope': 'spent', 'inputs': inputs,
               'tools': {name: 'a' * 64 for name in capacity.TOOLS},
               'judging': {name: 'b' * 64 for name in source_names},
               'runtime': {'python': '3.12', 'numpy': '2.5.4'}, 'operations': list(capacity.OPERATIONS),
               'minimum_full_frames': frames, 'limits': limits}
    receipt = {'schema': 'opus-capacity/1', 'purpose': 'full', 'binding': copy.deepcopy(binding),
               'host': {'cpu': 'INTEL(R) XEON(R) PLATINUM 8581C CPU @ 2.30GHz', 'smt': False,
                        'outputs_tmpfs': True, 'memory_as_bytes': limits['memory_bytes'],
                        'tmpfs_budget_bytes': limits['tmpfs_bytes']},
               'inputs': rows, 'complete': True, 'passed': True}
    return receipt, binding


class Workflow(unittest.TestCase):
    def test_required_check_runs_the_real_gate_and_propagates_its_failure(self):
        workflow = (ROOT / '.github/workflows/deny.yml').read_text()
        reader = "require 'yaml'; require 'json'; puts JSON.generate(YAML.safe_load(STDIN.read)['jobs']['deny-checks']['steps'])"
        steps = json.loads(subprocess.run(['ruby', '-e', reader], input=workflow, text=True,
                                         capture_output=True, check=True).stdout)
        entries = [step for step in steps if step.get('name') == 'Check Opus capacity before a new registration']
        self.assertEqual(len(entries), 1, 'Restore the Opus capacity step in deny-checks')
        step = entries[0]
        self.assertEqual(step.get('if'), "steps.result.outputs.reuse != 'true'")
        gate = ['-I', 'bench/audio-decode/capacity.py', 'check-registrations']
        with tempfile.TemporaryDirectory() as directory:
            directory = pathlib.Path(directory)
            wrapper = directory / 'python3'
            calls = directory / 'calls'
            wrapper.write_text(f'#!{sys.executable}\nimport json,os,sys\n'
                               'with open(os.environ["CALLS"], "a") as out: out.write(json.dumps(sys.argv[1:]) + "\\n")\n'
                               f'sys.exit(7 if sys.argv[1:] == {gate!r} else 0)\n')
            wrapper.chmod(0o755)
            done = subprocess.run(['bash', '-c', step['run']], cwd=ROOT, capture_output=True,
                                  env=dict(os.environ, PATH=str(directory) + os.pathsep + os.environ['PATH'],
                                           CALLS=str(calls)))
            self.assertEqual(done.returncode, 7, 'The required step must fail when capacity refuses registration')
            self.assertEqual([json.loads(line) for line in calls.read_text().splitlines()], [
                ['-m', 'pip', 'install', '--quiet', '--require-hashes', '-r', 'bench/audio-decode/requirements.txt'],
                ['-I', '-m', 'unittest', 'discover', '-s', 'bench/audio-decode/tests', '-p', 'test_*.py'], gate])


class Qualification(unittest.TestCase):
    def test_complete_full_length_six_cell_receipt_qualifies(self):
        receipt, binding = evidence()
        proof = capacity.validate_capacity(receipt, binding)
        self.assertEqual(proof.binding_sha256, capacity.canonical(binding))

    def test_smoke_partial_and_unchanged_pass_flag_do_not_qualify(self):
        for mutate in (lambda value: value.update(purpose='smoke'),
                       lambda value: value.update(complete=False),
                       lambda value: value['inputs'].pop(),
                       lambda value: value['inputs'].append(value['inputs'][0])):
            receipt, binding = evidence()
            mutate(receipt)
            self.assertTrue(receipt['passed'])
            self.assertFalse(capacity.qualify(receipt, binding))

    def test_each_operation_must_be_present_and_terminal_without_error(self):
        for name in capacity.OPERATIONS:
            for status in ('pending', 'running', 'interrupted', 'failed'):
                receipt, binding = evidence()
                receipt['inputs'][0]['operations'][name]['status'] = status
                self.assertFalse(capacity.qualify(receipt, binding), (name, status))
            receipt, binding = evidence()
            del receipt['inputs'][0]['operations'][name]
            self.assertFalse(capacity.qualify(receipt, binding), name)

    def test_every_input_must_complete_the_full_length_not_just_its_header(self):
        for role, field, value in (('raw_identity', 'frames', capacity.MIN_FULL_FRAMES - 1),
                                   ('head_dec', 'samples', 1), ('lav_dec', 'packets', 1),
                                   ('head_lib', 'bytes', 16), ('ffmpeg_cli', 'frames', 1),
                                   ('cli_identity', 'output_bytes', 44)):
            receipt, binding = evidence()
            receipt['inputs'][0]['operations'][role]['result'][field] = value
            self.assertFalse(capacity.qualify(receipt, binding), (role, field))

    def test_normal_oracle_disagreement_is_complete_work_but_errors_are_not(self):
        receipt, binding = evidence()
        receipt['inputs'][0]['operations']['oracle']['result']['passed'] = False
        receipt['inputs'][0]['operations']['oracle']['result']['detail']['compare_passed'] = False
        self.assertTrue(capacity.qualify(receipt, binding))
        receipt['inputs'][0]['operations']['oracle']['result']['status'] = 'error'
        self.assertFalse(capacity.qualify(receipt, binding))

    def test_every_result_has_the_same_closed_schema_as_runtime_checkpoints(self):
        for operation in capacity.OPERATIONS:
            receipt, binding = evidence()
            result = receipt['inputs'][0]['operations'][operation]['result']
            result['stderr'] = 'PRIVATE-MARKER'
            self.assertFalse(capacity.qualify(receipt, binding), operation)
        for value in (float('nan'), float('inf'), 'PRIVATE-MARKER'):
            receipt, binding = evidence()
            receipt['inputs'][0]['operations']['oracle']['result']['detail']['quality'] = value
            self.assertFalse(capacity.qualify(receipt, binding))
        receipt, binding = evidence()
        receipt['inputs'][0]['operations']['head_lib']['result']['ns'] = 1
        self.assertFalse(capacity.qualify(receipt, binding))

    def test_binding_requires_every_runtime_source_not_only_the_reader(self):
        for name in capacity.JUDGING_INPUTS:
            receipt, binding = evidence()
            del binding['judging'][name]
            receipt['binding'] = copy.deepcopy(binding)
            self.assertFalse(capacity.qualify(receipt, binding), name)

    def test_truncated_oracle_work_is_not_a_capacity_pass(self):
        receipt, binding = evidence()
        receipt['inputs'][0]['operations']['oracle']['result']['detail']['samples']['ref'] = 1
        self.assertFalse(capacity.qualify(receipt, binding))

    def test_byte_and_runtime_identity_changes_invalidate_reuse(self):
        for group in ('tools', 'judging', 'runtime'):
            receipt, binding = evidence()
            first = next(iter(binding[group]))
            binding[group][first] = 'c' * 64 if group != 'runtime' else '3.13'
            self.assertFalse(capacity.qualify(receipt, binding), group)
        receipt, binding = evidence()
        binding['inputs'][0]['sha256'] = 'c' * 64
        self.assertFalse(capacity.qualify(receipt, binding))

    def test_wrong_host_or_resource_envelope_is_refused(self):
        for field, value in (('cpu', 'another processor'), ('smt', True), ('outputs_tmpfs', False),
                             ('memory_as_bytes', 1), ('tmpfs_budget_bytes', 1)):
            receipt, binding = evidence()
            receipt['host'][field] = value
            self.assertFalse(capacity.qualify(receipt, binding), field)

    def test_missing_mode_class_and_identity_mismatches_fail(self):
        for role in ('raw_identity', 'cli_identity'):
            receipt, binding = evidence()
            receipt['inputs'][0]['operations'][role]['result']['passed'] = False
            self.assertFalse(capacity.qualify(receipt, binding))
        receipt, binding = evidence()
        missing = binding['inputs'].pop()
        receipt['binding'] = copy.deepcopy(binding)
        receipt['inputs'] = [row for row in receipt['inputs'] if row['sha256'] != missing['sha256']]
        self.assertFalse(capacity.qualify(receipt, binding))


class Binding(unittest.TestCase):
    def test_inputs_tools_and_judging_are_bound_to_bytes_not_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            _, fixture = evidence()
            for name in fixture['judging']:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'judging')
            media = root / 'PRIVATE-MARKER-media'
            media.mkdir()
            source = media / 'PRIVATE-MARKER.mp4'
            source.write_bytes(b'input bytes')
            inputs = {'scope': 'spent', 'inputs': [{'file': source.name, 'sha256': capacity.digest(source),
                'class': 'mp4', 'channels': 2, 'packets': {'celt/20': 45000}}]}
            tools = {}
            for role in capacity.TOOLS:
                path = root / ('PRIVATE-MARKER-' + role)
                path.write_bytes(b'executable')
                tools[role] = str(path)
            with patch.object(capacity.importlib.metadata, 'version', return_value='2.5.4'):
                first = capacity.make_binding(root, inputs, media, tools, fixture['limits'])
                pathlib.Path(tools['lav']).write_bytes(b'changed executable')
                second = capacity.make_binding(root, inputs, media, tools, fixture['limits'])
                self.assertNotEqual(first['tools'], second['tools'])
                (root / 'scripts/oracle_kd.py').write_bytes(b'changed judging')
                third = capacity.make_binding(root, inputs, media, tools, fixture['limits'])
                self.assertNotEqual(second['judging'], third['judging'])
                source.write_bytes(b'changed media')
                with self.assertRaises(capacity.Invalid):
                    capacity.make_binding(root, inputs, media, tools, fixture['limits'])
            self.assertNotIn('PRIVATE-MARKER', json.dumps(first))

    def test_duplicate_keys_nonfinite_json_and_invalid_limits_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'input.json'
            for data in ('{"a":1,"a":2}', '{"a":NaN}', '{'):
                path.write_text(data)
                with self.assertRaises(capacity.Invalid):
                    capacity.load(path)
        for concurrency in (2, True, 0):
            _, binding = evidence()
            binding['limits']['concurrency'] = concurrency
            with self.assertRaises(capacity.Invalid):
                capacity.normalized_limits(binding['limits'])

    def test_atomic_failure_retains_the_previous_checkpoint(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'checkpoint.json'
            capacity.atomic(path, {'old': True})
            with patch.object(pathlib.Path, 'replace', side_effect=OSError('PRIVATE-MARKER')):
                with self.assertRaises(OSError):
                    capacity.atomic(path, {'new': True})
            self.assertEqual(capacity.load(path), {'old': True})
            self.assertEqual(list(path.parent.iterdir()), [path])


class Registration(unittest.TestCase):
    def prepare(self, root):
        receipt, binding = evidence()
        for name in binding['judging']:
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('source')
        binding['judging'] = capacity.judging(root)
        receipt['binding'] = copy.deepcopy(binding)
        directory = root / 'evidence'
        directory.mkdir()
        proof, expected = directory / 'capacity.json', directory / 'binding.json'
        capacity.atomic(proof, receipt)
        capacity.atomic(expected, binding)
        line = (f'Capacity preflight: `evidence/capacity.json` sha256:`{capacity.digest(proof)}`; '
                f'binding: `evidence/binding.json` sha256:`{capacity.digest(expected)}`.')
        return receipt, binding, line

    def test_historical_registrations_are_unchanged_but_new_ones_need_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            document = root / 'docs/design/audio-decode.md'
            document.parent.mkdir(parents=True)
            document.write_text('### Opus speed, round 3\n\nExisting registration.\n')
            capacity.check_registrations(root)
            document.write_text(document.read_text() + '\n### Opus speed, round 4\n')
            with self.assertRaisesRegex(capacity.Invalid, 'needs_capacity_preflight'):
                capacity.check_registrations(root)

    def test_only_complete_current_full_receipt_allows_a_future_fixture_registration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            receipt, binding, line = self.prepare(root)
            doc = root / 'docs/design/audio-decode.md'
            doc.parent.mkdir(parents=True, exist_ok=True)
            doc.write_text('### Opus speed, round 4\n\n' + line + '\n')
            capacity.check_registrations(root)
            (root / 'scripts/oracle_kd.py').write_text('changed')
            with self.assertRaisesRegex(capacity.Invalid, 'changed_judging_code'):
                capacity.check_registrations(root)

    def test_smoke_or_partial_receipt_cannot_be_registered_even_with_updated_hashes(self):
        for field, value in (('purpose', 'smoke'), ('complete', False)):
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                receipt, binding, line = self.prepare(root)
                receipt[field] = value
                proof = root / 'evidence/capacity.json'
                before = capacity.digest(proof)
                capacity.atomic(proof, receipt)
                line = line.replace(before, capacity.digest(proof))
                doc = root / 'docs/design/audio-decode.md'
                doc.parent.mkdir(parents=True, exist_ok=True)
                doc.write_text('### Opus speed, round 4\n\n' + line + '\n')
                with self.assertRaises(capacity.Invalid):
                    capacity.check_registrations(root)


if __name__ == '__main__':
    unittest.main()
