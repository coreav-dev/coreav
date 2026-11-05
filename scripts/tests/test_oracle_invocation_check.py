import hashlib
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import oracle_invocation_check as check


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


class Discovery(unittest.TestCase):
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
