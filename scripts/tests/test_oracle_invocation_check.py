import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import oracle_invocation_check as check


class Provisioning(unittest.TestCase):
    def test_ci_installs_the_pinned_structural_parser_before_script_discovery(self):
        workflow = (check.ROOT / '.github/workflows/deny.yml').read_text()
        install = workflow.index('name: Install structural invocation discovery')
        tests = workflow.index('name: Check the measurement analyzers')
        self.assertLess(install, tests)
        block = workflow[install:tests]
        self.assertRegex(block, r'@ast-grep/cli@[0-9]+\.[0-9]+\.[0-9]+(?:\s|$)')
        self.assertIn('--save-exact', block)
        self.assertIn('"$RUNNER_TEMP/oracle-discovery/node_modules/.bin" >> "$GITHUB_PATH"', block)


class HistoricalRecords(unittest.TestCase):
    def fixture(self, root):
        source = root / 'docs/measurements/past/run.py'
        source.parent.mkdir(parents=True)
        source.write_text('historical source\n')
        record = {'schema': 1, 'boundary': 'docs/measurements',
                  'files': {source.relative_to(root).as_posix(): hashlib.sha256(source.read_bytes()).hexdigest()}}
        return source, record

    def test_exact_record_is_the_only_historical_exception(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, record = self.fixture(root)
            self.assertEqual(check.historical_records(root, record), {source.relative_to(root).as_posix()})

    def test_edited_historical_source_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, record = self.fixture(root)
            source.write_text('new oracle invocation\n')
            with self.assertRaisesRegex(ValueError, 'edited historical file'):
                check.historical_records(root, record)

    def test_added_historical_source_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, record = self.fixture(root)
            source.with_name('bypass.py').write_text('new source\n')
            with self.assertRaisesRegex(ValueError, 'added historical file'):
                check.historical_records(root, record)

    def test_missing_or_renamed_historical_source_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, record = self.fixture(root)
            source.rename(source.with_name('new-name.py'))
            with self.assertRaisesRegex(ValueError, 'missing historical file'):
                check.historical_records(root, record)

    def test_boundary_cannot_be_broadened(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, record = self.fixture(root)
            record['boundary'] = 'scripts'
            with self.assertRaisesRegex(ValueError, 'restore the recorded historical boundary'):
                check.historical_records(root, record)

    def test_a_symlink_cannot_substitute_for_recorded_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, record = self.fixture(root)
            copy = root / 'copy'
            copy.write_bytes(source.read_bytes())
            source.unlink()
            source.symlink_to(copy)
            with self.assertRaisesRegex(ValueError, 'symbolic link'):
                check.historical_records(root, record)

    def test_the_boundary_and_its_parents_cannot_be_symbolic_links(self):
        for name in ('docs', 'docs/measurements'):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                _, record = self.fixture(root)
                boundary = root / name
                copy = root / 'retained'
                boundary.rename(copy)
                boundary.symlink_to(copy, target_is_directory=True)
                with self.assertRaisesRegex(ValueError, 'historical boundary'):
                    check.historical_records(root, record)


class Discovery(unittest.TestCase):
    def whole_inventory(self, sources, prepare=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            historical = 'docs/measurements/past/source.txt'
            files = {
                'scripts/compat_sources.py': 'FROZEN=()\nJUDGING=()\nJUDGING_REGISTRATION="registration.json"\n',
                'tools/compat-corpus/b4_round10.py': 'TOOL_FILES=()\n',
                'scripts/vp9_pixel_round.py': 'CAPTURE_SCRIPTS=()\n',
                'bench/audio-decode/capacity.py': 'SCRIPT_INPUTS=()\n',
                'scripts/h264_pgo_diagnostics.py': 'TOOLS=()\n',
                historical: 'historical bytes\n',
                check.HISTORICAL_RECORD: json.dumps(dict(schema=1, boundary='docs/measurements',
                    files={historical: hashlib.sha256(b'historical bytes\n').hexdigest()})),
                **sources,
            }
            for path, source in files.items():
                target = root / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(source)
            subprocess.run(['git', 'init', '--quiet', str(root)], check=True, capture_output=True)
            return check.inventory(prepare(root) if prepare else root)

    def test_inventory_normalizes_a_symlinked_root(self):
        def prepare(root):
            link = root / 'linked-root'
            link.symlink_to(root, target_is_directory=True)
            return link
        sites = self.whole_inventory({'kept.sh': 'ffmpeg -version\n'}, prepare)
        self.assertTrue(any(site['path'] == 'kept.sh' for site in sites))

    def test_missing_index_files_and_gitlinks_do_not_crash_discovery(self):
        def prepare(root):
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            for name in ('deleted.txt', 'deleted.rs', 'deleted.js'):
                (root / name).unlink()
            (root / 'module').mkdir()
            subprocess.run(['git', '-C', str(root), 'update-index', '--add', '--cacheinfo',
                            '160000,' + '1' * 40 + ',module'], check=True)
            return root
        sites = self.whole_inventory({'kept.sh': 'ffmpeg -version\n', 'deleted.txt': 'data',
                                      'deleted.rs': 'fn main() {}', 'deleted.js': 'let x = 1;'}, prepare)
        self.assertTrue(any(site['path'] == 'kept.sh' for site in sites))

    def test_extensionless_shebang_scripts_are_discovered(self):
        sites = self.whole_inventory({
            'shell-runner': '#!/bin/sh\nffmpeg -version\n',
            'python-runner': '#!/usr/bin/env python3\nimport subprocess\nsubprocess.run(["ffprobe", "-version"])\n',
        })
        self.assertTrue(any(site['path'] == 'shell-runner' and site['kind'] == 'shell' for site in sites))
        self.assertTrue(any(site['path'] == 'python-runner' and site['program'] == "'ffprobe'"
                            and site['role'] == 'oracle' for site in sites))

    def test_extensionless_python_relays_have_explicit_source_dependencies(self):
        sites = self.whole_inventory({'runner': '#!/usr/bin/env python3\nimport subprocess\n'
            'def launch(command):\n    subprocess.run(command)\nlaunch(command)\n'})
        delegated = [site for site in sites if site['path'] == 'runner' and site['kind'] == 'delegated']
        self.assertEqual(len(delegated), 1)
        self.assertEqual(delegated[0]['requires'], ['runner'])
        self.assertEqual(delegated[0]['relay'], [['runner', 'launch']])

    def test_shebang_like_yaml_comments_cannot_hide_workflow_commands(self):
        sites = self.whole_inventory({'flow.yml': '#!/bin/sh\nsteps:\n  - run: ffmpeg -version\n'})
        self.assertTrue(any(site['path'] == 'flow.yml' and site['program'] == 'ffmpeg -version'
                            for site in sites))

    def test_shell_commands_are_not_confused_with_copy_operands(self):
        sites = check.shell_sites('ffmpeg -i in out\nmv ffmpeg dest\necho ffmpeg\n', 'run.sh')
        self.assertEqual(len(sites), 1)
        self.assertEqual(sites[0]['program'], 'ffmpeg -i in out')

    def test_shell_command_substitutions_are_discovered(self):
        sites = check.shell_sites('ENCODERS="$(ffmpeg -encoders)"', 'run.sh')
        self.assertEqual(len(sites), 1)
        self.assertEqual(sites[0]['program'], 'ffmpeg -encoders')

    def test_an_oracle_hidden_behind_a_shell_interpreter_is_discovered(self):
        sites = check.PythonSites('import subprocess\nsubprocess.run(["bash", "-c", "ffmpeg -version"])\n',
                                  'consumer.py').collect()
        self.assertTrue(sites)
        self.assertEqual(sites[0]['role'], 'oracle')

    def test_namespaced_rust_constructor_is_discovered(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'consumer.rs').write_text('fn f() { std::process::Command::new("ffmpeg"); }')
            sites = check.structural_sites(root, ['consumer.rs'], 'Rust', '$CALL($PROGRAM)', 'PROGRAM', None,
                                           r'(^|::)Command::new$')
            self.assertEqual(len(sites), 1)
            self.assertEqual(sites[0]['program'], '"ffmpeg"')

    def test_owner_import_does_not_prove_executable_identity(self):
        sites = check.PythonSites('import test_oracle, subprocess\n'
                                  'test_oracle.require(args.ffmpeg)\n'
                                  'subprocess.run([args.ffmpeg, "-version"])\n', 'consumer.py').collect()
        self.assertTrue(sites)
        self.assertTrue(all(site['status'] == 'unproven' for site in sites))

    def test_an_assigned_process_alias_is_discovered(self):
        sites = check.PythonSites('import subprocess\nlaunch = subprocess.run\nlaunch(command)\n',
                                  'consumer.py').collect()
        self.assertTrue(sites)
        self.assertEqual(sites[0]['role'], 'opaque')

    def test_a_default_process_callable_is_discovered(self):
        sites = check.PythonSites('import subprocess\ndef capture(command, launch=subprocess.run):\n    launch(command)\n',
                                  'consumer.py').collect()
        self.assertTrue(sites)
        self.assertEqual(sites[0]['role'], 'opaque')

    def test_renamed_literal_executable_is_not_assumed_non_oracle(self):
        sites = check.PythonSites('import subprocess\nsubprocess.run(["renamed-program", "-version"])\n',
                                  'consumer.py').collect()
        self.assertTrue(sites)
        self.assertEqual(sites[0]['role'], 'opaque')

    def test_keyword_argv_and_actual_executable_override_are_discovered(self):
        sites = check.PythonSites('import subprocess\nsubprocess.run(args=command, executable=override)\n',
                                  'consumer.py').collect()
        self.assertEqual({site['kind'] for site in sites}, {'process', 'executable-override'})

    def test_a_delegated_caller_without_a_process_constructor_is_discovered(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'helper.py').write_text('import subprocess\ndef capture(command):\n    subprocess.run(command)\n')
            (root / 'caller.py').write_text('from helper import capture as launch\nlaunch(command)\n')
            sites = check.delegated_python_sites(root, ['helper.py', 'caller.py'])
            self.assertTrue(any(site['path'] == 'caller.py' and site['program'] == 'launch' for site in sites))

    def test_workflow_environment_fields_are_not_commands(self):
        source = 'steps:\n  - run: |\n      ffprobe -version\n    env:\n      COREAV_FFMPEG: /bin/ffmpeg\n'
        sites = check.workflow_sites(source, 'workflow.yml')
        self.assertEqual(len(sites), 1)
        self.assertEqual(sites[0]['program'], 'ffprobe -version')
        self.assertEqual(sites[0]['line'], 3)


if __name__ == '__main__':
    unittest.main()
