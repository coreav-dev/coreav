from pathlib import Path
import re
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import oracle_invocation_check as discovery
import oracle_invocation_classify as classifier


class Classification(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / 'scripts').mkdir()
        self.path = 'scripts/consumer.py'
        self.file = self.root / self.path

    def tearDown(self):
        self.directory.cleanup()

    def sites(self, source):
        self.file.write_text(source)
        sites = discovery.PythonSites(source, self.path).collect()
        for site in sites:
            site['status'] = 'unclassified'
        return sites

    def record(self, sites, verdict, fold=None):
        return dict(schema=1, source_snapshot_sha256=classifier.source_snapshot(self.root),
                    sources={self.path: classifier.digest(self.file)}, dependencies={}, frozen={},
                    rules=[dict(id='fixture-call-pattern', verdict=verdict, path=self.path,
                                lines=[site['line']], kinds=[site['kind']],
                                pattern=re.escape(site['program']), fold_to=fold or {},
                                evidence='generated fixture with a concrete call-pattern proof')
                           for site in sites])

    def test_import_alone_does_not_qualify_a_literal(self):
        sites = self.sites('import test_oracle, subprocess\nsubprocess.run(["ffmpeg", "-version"])\n')
        report = classifier.classify(self.root, sites, self.record(sites, 'confirmed-bypass'))
        self.assertEqual(report['counts']['confirmed-bypass']['locations'], 1)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_owner_return_value_qualifies(self):
        sites = self.sites('import test_oracle, subprocess\ndef main():\n'
                           '    pair = test_oracle.require()\n'
                           '    subprocess.run([str(pair.ffmpeg), "-version"])\n')
        report = classifier.classify(self.root, sites, self.record(sites, 'qualified-via-owner'))
        self.assertEqual(report['counts']['qualified-via-owner']['locations'], 1)

    def test_a_same_named_module_is_not_the_owner(self):
        sites = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\nsubprocess.run([pair.ffmpeg])\n')
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'scripts/test_oracle.py').write_text('def require(): return None\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['source_snapshot_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_a_parameter_cannot_shadow_the_owner(self):
        sites = self.sites('import test_oracle, subprocess\ndef f():\n'
                           '    pair = test_oracle.require()\n    subprocess.run([pair.ffmpeg])\n')
        record = self.record(sites, 'qualified-via-owner')
        changed = self.sites(self.file.read_text().replace('def f():', 'def f(test_oracle):'))
        self.assertEqual(classifier.classify(self.root, changed, record)['counts']['qualified-via-owner']['rows'], 0)

    def test_branch_only_qualification_remains_unknown(self):
        sites = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\nsubprocess.run([pair.ffmpeg])\n')
        record = self.record(sites, 'qualified-via-owner')
        changed = self.sites('import test_oracle, subprocess\nif ready:\n    pair = test_oracle.require()\nsubprocess.run([pair.ffmpeg])\n')
        report = classifier.classify(self.root, changed, record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertEqual(report['counts']['confirmed-bypass']['rows'], 0)
        self.assertGreater(report['counts']['unknown']['rows'], 0)

    def test_ignored_directory_guard_is_not_an_owner_return(self):
        sites = self.sites('import test_oracle, subprocess\ntest_oracle.verify_bin(program.parent)\nsubprocess.run([program])\n')
        report = classifier.classify(self.root, sites)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertGreater(report['counts']['unknown']['rows'], 0)

    def test_docstrings_are_not_executed_programs(self):
        sites = self.sites('"""Example ffmpeg\nnot an executed program\n"""\n')
        self.assertEqual(classifier.classify(self.root, sites)['counts']['excluded']['locations'], 1)

    def test_an_opaque_executable_stays_failed(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        self.assertEqual(classifier.classify(self.root, sites)['counts']['unknown']['locations'], 1)

    def test_argv_alias_rows_fold_into_one_construction(self):
        sites = self.sites('import subprocess\ncommand = ["ffmpeg", "-version"]\nsubprocess.run(command)\n')
        report = classifier.classify(self.root, sites, self.record(sites, 'confirmed-bypass', {'3': 2}))
        self.assertEqual(report['counts']['confirmed-bypass']['locations'], 1)
        self.assertTrue(report['reconciliation']['exact'])

    def test_distinct_argv_constructions_are_not_one_origin(self):
        sites = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\n'
                           'subprocess.run([pair.ffmpeg, "-version"])\nsubprocess.run([pair.ffmpeg, "-encoders"])\n')
        report = classifier.classify(self.root, sites, self.record(sites, 'qualified-via-owner'))
        self.assertEqual(report['counts']['qualified-via-owner']['locations'], 2)

    def test_execution_override_is_not_qualified_by_argv(self):
        sites = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\nsubprocess.run([pair.ffmpeg])\n')
        record = self.record(sites, 'qualified-via-owner')
        changed = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\n'
                             'subprocess.run(\n    [pair.ffmpeg],\n    executable=override)\n')
        self.assertEqual(classifier.classify(self.root, changed, record)['counts']['qualified-via-owner']['rows'], 0)

    def test_qualified_argv_mutation_invalidates_the_proof(self):
        sites = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\n'
                           'command = [pair.ffmpeg]\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        changed = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\n'
                             'command = [pair.ffmpeg]\ncommand[0] = override\nsubprocess.run(command)\n')
        process = [item for item in classifier.classify(self.root, changed, record)['locations'] if item['line'] == 5]
        self.assertTrue(process)
        self.assertTrue(all(item['verdict'] == 'unknown' for item in process))

    def test_rust_native_identity_is_not_a_substring_hint(self):
        self.path = 'scripts/consumer.rs'
        self.file = self.root / self.path
        self.file.write_text('fn f() {\nCommand::new(env!("CARGO_BIN_EXE_coreav"));\nCommand::new(other);\n}\n')
        sites = [dict(path=self.path, line=2, kind='Rust', program='env!("CARGO_BIN_EXE_coreav")', status='unclassified'),
                 dict(path=self.path, line=3, kind='Rust', program='PathBuf::from(env!("CARGO_BIN_EXE_coreav")).with_file_name("ffmpeg")', status='unclassified')]
        report = classifier.classify(self.root, sites, self.record(sites[:1], 'excluded'))
        self.assertEqual(report['counts']['excluded']['locations'], 1)
        self.assertEqual(report['counts']['unknown']['locations'], 1)

    def test_rust_environment_binding_has_a_positive_origin(self):
        self.path = 'scripts/consumer.rs'
        self.file = self.root / self.path
        self.file.write_text('fn f() {\nlet ffmpeg = std::env::var_os("COREAV_FFMPEG");\nCommand::new(&ffmpeg);\n}\n')
        sites = [dict(path=self.path, line=3, kind='Rust', program='&ffmpeg', status='unclassified')]
        report = classifier.classify(self.root, sites, self.record(sites, 'confirmed-bypass'))
        self.assertEqual(report['counts']['confirmed-bypass']['locations'], 1)

    def test_shell_arrays_need_a_concrete_program_assignment(self):
        self.path = 'scripts/fixture.sh'
        self.file = self.root / self.path
        self.file.write_text('FF=(ffmpeg -v error)\n"$' + '{FF[@]}" -version\n')
        sites = [dict(path=self.path, line=2, kind='shell', program='"$' + '{FF[@]}" -version', status='unclassified')]
        record = self.record(sites, 'confirmed-bypass')
        self.assertEqual(classifier.classify(self.root, sites, record)['counts']['confirmed-bypass']['locations'], 1)
        self.file.write_text('"$' + '{FF[@]}" -version\n')
        self.assertEqual(classifier.classify(self.root, sites, record)['counts']['confirmed-bypass']['locations'], 0)

    def test_dependency_edit_invalidates_unchanged_callers(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        dependency = self.root / 'scripts/helper.py'
        dependency.write_text('def command(): pass\n')
        record = self.record(sites, 'qualified-via-owner')
        record['dependencies']['scripts/helper.py'] = classifier.digest(dependency)
        dependency.write_text('def command(): return "ffmpeg"\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['source_snapshot_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_new_sources_do_not_inherit_a_disposition(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'new.py').write_text('import subprocess\nsubprocess.run(["renamed-program"])\n')
        self.assertFalse(classifier.classify(self.root, sites, record)['source_snapshot_valid'])

    def test_a_renamed_executable_cannot_escape_by_losing_its_extension(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'renamed-reference').write_text('#!/bin/sh\nexec ffmpeg "$@"\n')
        self.assertFalse(classifier.classify(self.root, sites, record)['source_snapshot_valid'])

    def test_unknown_row_wins_over_a_qualified_row_at_the_same_location(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        sites.append(dict(sites[0], program='other'))
        report = classifier.classify(self.root, sites, record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertEqual(report['counts']['unknown']['rows'], 2)
        self.assertTrue(report['reconciliation']['exact'])


if __name__ == '__main__':
    unittest.main()
