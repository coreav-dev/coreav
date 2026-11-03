"""Supplemental inputs preserve legacy choices, bytes and registered identities."""
import ast
import hashlib
import itertools
import json
from pathlib import Path
import re
import sys
import tempfile
import types
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT / 'scripts'), str(ROOT / 'tools/compat-corpus')]
import encode_breadth as v2
import v2_fixture_inputs as inputs
import v2_fixture_check as check
import avi_v2_check as avi


class Registration(unittest.TestCase):
    def test_every_inventory_case_has_one_bound_registration(self):
        plan = (ROOT / 'docs/design/v2-fixtures.md').read_text()
        listed = [int(n) for n in re.findall(r'^\| (\d+) \|', plan, re.M)]
        indices = [r['index'] for r in inputs.DOCUMENT['cases']]
        self.assertEqual(len(indices), 171)
        self.assertEqual(indices, sorted(set(listed)))
        self.assertEqual(len(listed), len(set(listed)))
        self.assertEqual(inputs.registration(), inputs.DOCUMENT)
        tuples = {int(n): re.findall(r'`([^`]+)`', value) for n, value in
                  re.findall(r'^\| (\d+) \| ([^|]+) \|', plan, re.M)}
        self.assertEqual(tuples, {r['index']: [name for a in r['assignments'] for name in a]
                                  for r in inputs.DOCUMENT['cases']})

    def test_the_generator_and_registration_have_exactly_the_same_files(self):
        tree = ast.parse(inputs.GENERATOR.read_text())
        recipes = next(n.value for n in tree.body if isinstance(n, ast.Assign)
                       and any(isinstance(t, ast.Name) and t.id == 'RECIPES' for t in n.targets))
        self.assertEqual(set(inputs.FILES), {n.value for n in recipes.keys})

    def test_whole_legacy_cartesian_order_is_unchanged(self):
        for count in (1, 2, 3):
            args = list(itertools.chain.from_iterable(('-i', n) for n in ('INPUT', 'INPUT2', 'INPUT3')[:count]))
            self.assertEqual(v2.assignments(args), list(itertools.product(inputs.LEGACY, repeat=count)))
        self.assertEqual(v2.assignments(['-i', 'INPUT', '-i', 'INPUT']), [(n,) for n in inputs.LEGACY])

    def choose(self, index, accepted):
        entry = json.loads(inputs.COMMANDS.read_text())['commands'][index]
        seen = []
        with tempfile.TemporaryDirectory() as temp:
            def prepare(opts, entry, base, tag, assignment):
                seen.append(assignment)
                return types.SimpleNamespace(cwd=Path(temp), args=[], paths={})

            def execute(*args):
                return v2.Ran(0 if seen[-1] == accepted else 1, False, '')

            with mock.patch.object(v2, 'prepare', side_effect=prepare), \
                    mock.patch.object(v2, 'execute', side_effect=execute), \
                    mock.patch.object(v2, 'succeeded', side_effect=lambda ran, *args: ran.code == 0), \
                    mock.patch.object(v2.shutil, 'rmtree'):
                result = v2.choose(types.SimpleNamespace(ffmpeg='ffmpeg', env={}), entry, Path(temp))
        return result, seen, entry

    def test_a_legacy_success_never_tries_a_supplement(self):
        accepted = (inputs.LEGACY[0], inputs.LEGACY[1])
        result, seen, entry = self.choose(16, accepted)
        self.assertEqual(result[0], accepted)
        self.assertEqual(seen, v2.assignments(entry['args'])[:2])

    def test_supplements_follow_the_entire_legacy_product(self):
        row = next(r for r in inputs.DOCUMENT['cases'] if r['index'] == 16)
        accepted = tuple(row['assignments'][0])
        result, seen, entry = self.choose(16, accepted)
        self.assertEqual(result[0], accepted)
        self.assertEqual(seen, v2.assignments(entry['args']) + [accepted])

    def test_a_different_command_or_output_does_not_acquire_a_tuple(self):
        entry = dict(json.loads(inputs.COMMANDS.read_text())['commands'][63])
        entry['output'] = 'another.m4a'
        self.assertEqual(inputs.assignments(entry), ())
        entry['args'] = ['-i', 'INPUT', '-new-option', 'OUTPUT']
        self.assertEqual(inputs.assignments(entry), ())

    def test_each_dependency_and_sparse_file_is_declared_and_hashed(self):
        self.assertTrue(inputs.SPARSE <= set(inputs.FILES))
        for name, dependencies in inputs.DEPENDENCIES.items():
            self.assertIn(name, inputs.FILES)
            self.assertTrue(set(dependencies) <= set(inputs.FILES))
            self.assertTrue(all(Path(n).name == n for n in dependencies))
        self.assertEqual(set(v2.INPUT_FILES), set(inputs.LEGACY) | set(inputs.FILES))

    def test_missing_supplemental_bytes_stop_before_oracle_discovery(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in v2.INPUT_FILES[1:]:
                (root / name).touch()
            (root / v2.INPUT_FILES[0]).touch()
            (root / inputs.FILES[0]).unlink()
            with mock.patch.object(v2, 'oracle_programs') as oracle:
                self.assertEqual(v2.cmd_run(types.SimpleNamespace(fixtures=root)), v2.release_round.UNMEASURED)
            oracle.assert_not_called()


class Staging(unittest.TestCase):
    def test_zero_only_raw_staging_writes_no_payload_blocks(self):
        with tempfile.TemporaryDirectory() as temp:
            source, destination = Path(temp) / 'zeros', Path(temp) / 'copy'
            with source.open('wb') as stream:
                stream.truncate(8 << 20)
            inputs.sparse_copy(source, destination)
            self.assertEqual(destination.stat().st_size, 8 << 20)
            self.assertLess(destination.stat().st_blocks * 512, 1 << 20)
            self.assertEqual(destination.read_bytes(), bytes(8 << 20))

    def test_local_playlist_dependencies_keep_their_bytes_and_relative_names(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fixtures, staged = root / 'fixtures', root / 'staged'
            fixtures.mkdir()
            staged.mkdir()
            expected = set()
            for name, dependencies in inputs.DEPENDENCIES.items():
                expected.update((name, *dependencies))
                for item in (name, *dependencies):
                    (fixtures / item).write_bytes(item.encode())
                inputs.stage(name, fixtures, staged)
            self.assertEqual(sorted(p.name for p in staged.iterdir()), sorted(expected))
            for item in expected:
                self.assertEqual((fixtures / item).read_bytes(), (staged / item).read_bytes())

    def test_sparse_staging_preserves_holes_and_real_data(self):
        with tempfile.TemporaryDirectory() as temp:
            source, destination = Path(temp) / 'source', Path(temp) / 'copy'
            with source.open('wb') as stream:
                stream.truncate(8 << 20)
                stream.seek(1 << 20)
                stream.write(b'not a hole')
                stream.seek((7 << 20) - 1)
                stream.write(b'x')
            inputs.sparse_copy(source, destination)
            self.assertEqual(source.stat().st_size, destination.stat().st_size)
            self.assertEqual(hashlib.sha256(source.read_bytes()).digest(), hashlib.sha256(destination.read_bytes()).digest())
            if source.stat().st_blocks * 512 < source.stat().st_size // 2:
                self.assertLess(destination.stat().st_blocks * 512, destination.stat().st_size // 2)
            else:
                self.assertLessEqual(destination.stat().st_blocks, source.stat().st_blocks)


class ReferenceDiagnostics(unittest.TestCase):
    def test_avi_hashes_the_complete_input_without_reading_it_into_memory(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'sparse'
            size = 8 << 20
            with path.open('wb') as stream:
                stream.truncate(size)
                stream.seek(size - 4)
                stream.write(b'end!')
            expected = hashlib.sha256()
            block = bytes(1 << 20)
            for _ in range(7):
                expected.update(block)
            expected.update(block[:-4] + b'end!')
            with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('fixture hashes must stream')):
                self.assertEqual(avi.digest(path), expected.hexdigest())

    def test_reference_command_is_unchanged_and_failed_attempts_keep_the_reason(self):
        row = next(r for r in inputs.DOCUMENT['cases'] if r['index'] == 17)
        entry = json.loads(inputs.COMMANDS.read_text())['commands'][17]
        for success in (True, False):
            with self.subTest(success=success), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                fixtures = root / 'fixtures'
                fixtures.mkdir()
                name = row['assignments'][0][0]
                for item in (name, *inputs.DEPENDENCIES[name]):
                    (fixtures / item).write_bytes(b'synthetic')
                opts = types.SimpleNamespace(fixtures=fixtures, work=root / 'work',
                                             ffmpeg=Path('/oracle/ffmpeg'), env={})
                captured = []

                def execute(argv, cwd, env, seconds, pipe):
                    captured.append((argv, seconds, pipe))
                    if success:
                        (cwd / entry['output']).write_bytes(b'reference output')
                    return v2.Ran(0 if success else 8, False,
                                  str(opts.work) + '\n' + '\n'.join(f'line {i}' for i in range(50)))

                with mock.patch.object(v2, 'execute', side_effect=execute):
                    result = check.verify(opts, row)
                self.assertEqual(result['ok'], success)
                self.assertEqual(captured[0][1:], (v2.REFERENCE_SECONDS, False))
                argv = captured[0][0]
                self.assertEqual(argv[0], opts.ffmpeg)
                original = [a if a != 'INPUT' else argv[entry['args'].index('INPUT') + 1]
                            for a in entry['args']]
                original[-1] = entry['output']
                self.assertEqual(argv[1:], original)
                attempt = result['attempts'][0]
                self.assertEqual(attempt['exit'], 0 if success else 8)
                self.assertEqual(attempt['output_written'], success)
                self.assertEqual(attempt['stderr'].splitlines(), [f'line {i}' for i in range(10, 50)])


if __name__ == '__main__':
    unittest.main()
