from pathlib import Path
import copy
import json
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

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

    def record(self, sites, verdict, fold=None, requires=()):
        bindings = classifier.Bindings(self.root)
        context = bindings.context(self.path, requires)
        proof = dict(context, sites=[dict(site=list(classifier.site_key(site)), verdict=verdict,
                                         **{'class': {'qualified-via-owner': 'reviewed-reference',
                                                      'confirmed-bypass': 'reviewed-bypass',
                                                      'excluded': 'reviewed-exclusion'}[verdict]},
                                         fold_to=(fold or {}).get(str(site['line']), site['line']),
                                         evidence='generated fixture with an exact-site proof')
                                    for site in sites])
        for site, entry in zip(sites, proof['sites']):
            if site.get('program_span'):
                entry['program_span'] = dict(site['program_span'])
        return dict(schema=2, files={self.path: proof},
                    inventory=classifier.file_inventory(self.root), frozen={},
                    dependencies={path: bindings.hash(path) for path in context['requires']})

    def test_import_alone_does_not_qualify_a_literal(self):
        sites = self.sites('import test_oracle, subprocess\nsubprocess.run(["ffmpeg", "-version"])\n')
        report = classifier.classify(self.root, sites, self.record(sites, 'confirmed-bypass'))
        self.assertEqual(report['counts']['confirmed-bypass']['locations'], 1)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_missing_index_files_and_gitlinks_remain_unknown(self):
        (self.root / 'deleted.rs').write_text('fn main() {}')
        subprocess.run(['git', 'init', '--quiet', str(self.root)], check=True)
        subprocess.run(['git', '-C', str(self.root), 'add', '.'], check=True)
        (self.root / 'deleted.rs').unlink()
        (self.root / 'module').mkdir()
        subprocess.run(['git', '-C', str(self.root), 'update-index', '--add', '--cacheinfo',
                        '160000,' + '1' * 40 + ',module'], check=True)
        report = classifier.classify(self.root, [])
        self.assertEqual({site['path'] for site in report['locations']}, {'deleted.rs', 'module'})
        self.assertEqual(report['counts']['unknown']['rows'], 2)

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
        self.assertFalse(report['evidence_valid'])
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
        record = self.record(sites, 'qualified-via-owner', requires=['scripts/helper.py'])
        dependency.write_text('def command(): return "ffmpeg"\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_new_sources_do_not_inherit_a_disposition(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'new.py').write_text('import subprocess\nsubprocess.run(["renamed-program"])\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertGreater(report['counts']['unknown']['rows'], 0)

    def test_a_renamed_executable_cannot_escape_by_losing_its_extension(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'renamed-reference').write_text('#!/bin/sh\nexec ffmpeg "$@"\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertGreater(report['counts']['unknown']['rows'], 0)

    def test_unknown_row_wins_over_a_qualified_row_at_the_same_location(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        sites.append(dict(sites[0], program='other'))
        report = classifier.classify(self.root, sites, record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertEqual(report['counts']['unknown']['rows'], 2)
        self.assertTrue(report['reconciliation']['exact'])

    def test_an_unrelated_existing_document_edit_keeps_reviewed_verdicts(self):
        document = self.root / 'README.md'
        document.write_text('before\n')
        sites = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\nsubprocess.run([pair.ffmpeg])\n')
        record = self.record(sites, 'qualified-via-owner')
        document.write_text('after\n')
        report = classifier.classify(self.root, sites, record)
        self.assertTrue(report['evidence_valid'], report['problems'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 1)

    def test_a_source_edit_invalidates_only_the_bound_file(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        first = self.path
        other = 'scripts/other.py'
        (self.root / other).write_text('import subprocess\nsubprocess.run(program)\n')
        record = self.record(sites, 'qualified-via-owner')
        self.path, self.file = other, self.root / other
        second = self.sites(self.file.read_text())
        proof = self.record(second, 'qualified-via-owner')
        record['files'].update(proof['files'])
        record['dependencies'].update(proof['dependencies'])
        (self.root / first).write_text('import subprocess\nsubprocess.run(override)\n')
        changed = discovery.PythonSites((self.root / first).read_text(), first).collect()
        report = classifier.classify(self.root, [*changed, *second], record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 1)
        self.assertTrue(all(item['verdict'] == 'unknown' for item in report['locations'] if item['path'] == first))

    def test_new_standard_library_shadow_without_calls_invalidates_its_importers(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'subprocess.py').write_text('run = None\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertGreater(report['counts']['unknown']['rows'], 0)

    def test_new_namespace_package_without_calls_invalidates_its_importers(self):
        sites = self.sites('import test_oracle, subprocess\npair = test_oracle.require()\nsubprocess.run([pair.ffmpeg])\n')
        record = self.record(sites, 'qualified-via-owner')
        package = self.root / 'elsewhere/test_oracle'
        package.mkdir(parents=True)
        (package / '__init__.py').write_text('require = None\n')
        report = classifier.classify(self.root, sites, record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_transitive_import_edit_invalidates_its_callers(self):
        helper = self.root / 'scripts/helper.py'
        leaf = self.root / 'scripts/leaf.py'
        helper.write_text('import leaf\n')
        leaf.write_text('value = "before"\n')
        sites = self.sites('import helper, subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        leaf.write_text('value = "after"\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_a_new_empty_source_remains_an_unplaced_closure_site(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'unrelated.py').write_text('')
        report = classifier.classify(self.root, sites, record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 1)
        self.assertFalse(report['evidence_valid'])
        self.assertTrue(any(item['path'] == 'unrelated.py' and item['verdict'] == 'unknown'
                            for item in report['locations']))

    def test_executable_data_cannot_keep_its_inert_inventory_class(self):
        data = self.root / 'notes.txt'
        data.write_text('not executable\n')
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        data.chmod(0o700)
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertTrue(any(item['path'] == 'notes.txt' and item['verdict'] == 'unknown'
                            for item in report['locations']))

    def test_outward_symlinks_fail_without_reading_the_target(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / 'payload'
            target.write_text('external content never enters evidence\n')
            (self.root / 'subprocess.py').symlink_to(target)
            report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertNotIn('external content', json.dumps(report))

    def test_duplicate_and_regex_entries_are_rejected(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        duplicate = copy.deepcopy(record)
        duplicate['files'][self.path]['sites'].append(copy.deepcopy(duplicate['files'][self.path]['sites'][0]))
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            classifier.classify(self.root, sites, duplicate)
        record['files'][self.path]['sites'][0]['pattern'] = '.*'
        with self.assertRaisesRegex(ValueError, 'regex'):
            classifier.classify(self.root, sites, record)

    def test_outward_parent_symlinks_are_not_followed_for_tracked_inputs(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        subprocess.run(['git', 'init', '--quiet', str(self.root)], check=True, capture_output=True)
        subprocess.run(['git', '-C', str(self.root), 'add', self.path], check=True, capture_output=True)
        with tempfile.TemporaryDirectory() as outside:
            (Path(outside) / 'consumer.py').write_text('external content\n')
            (self.root / 'scripts').rename(self.root / 'retained')
            (self.root / 'scripts').symlink_to(outside, target_is_directory=True)
            real_open = Path.open

            def checked_open(path, *args, **kwargs):
                self.assertTrue(path.resolve().is_relative_to(self.root), 'read left the checkout')
                return real_open(path, *args, **kwargs)

            with patch.object(Path, 'open', checked_open):
                report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertNotIn('external content', json.dumps(report))

    def test_an_entry_for_a_disappeared_site_is_not_valid_evidence(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        report = classifier.classify(self.root, [], record)
        self.assertFalse(report['evidence_valid'])
        self.assertIn('dead-site-evidence', {item['reason'] for item in report['problems']})

    def test_evidence_class_must_agree_with_the_verdict(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        record['files'][self.path]['sites'][0]['class'] = 'reviewed-bypass'
        with self.assertRaisesRegex(ValueError, 'evidence class'):
            classifier.classify(self.root, sites, record)

    def test_literal_reference_cannot_be_promoted_by_a_reviewed_owner_class(self):
        sites = self.sites('import subprocess\nsubprocess.run(["ffmpeg", "-version"])\n')
        report = classifier.classify(self.root, sites, self.record(sites, 'qualified-via-owner'))
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertGreater(report['counts']['unknown']['rows'], 0)

    def test_each_repeated_site_requires_its_own_occurrence(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        report = classifier.classify(self.root, [*sites, dict(sites[0])], record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertEqual(report['counts']['unknown']['rows'], 2)
        self.assertTrue(report['reconciliation']['exact'])

    def test_fold_targets_cannot_hide_a_more_severe_verdict(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\nsubprocess.run(other)\n')
        record = self.record(sites, 'qualified-via-owner')
        entries = record['files'][self.path]['sites']
        entries[0].update(verdict='confirmed-bypass', **{'class': 'reviewed-bypass'}, fold_to=3)
        with self.assertRaisesRegex(ValueError, 'equally severe'):
            classifier.classify(self.root, sites, record)

    def test_frozen_record_must_match_the_registered_roster(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        (self.root / 'scripts/compat_sources.py').write_text('')
        record = self.record(sites, 'qualified-via-owner')
        with patch.object(discovery, 'frozen_files', return_value={'scripts/registered.py'}):
            report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_refresh_only_proposes_unreviewed_sites_without_mutating_the_record(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        original = copy.deepcopy(record)
        changed = self.sites('import subprocess\nsubprocess.run(override)\n')
        proposed = classifier.refresh(self.root, changed, record, [self.path])
        self.assertEqual(record, original)
        self.assertEqual(proposed['kind'], 'unreviewed-evidence-proposal')
        self.assertTrue(proposed['files'][self.path]['requires_review'])
        self.assertEqual(classifier.classify(self.root, changed, record)['counts']['qualified-via-owner']['rows'], 0)

    def test_an_omitted_delegated_dependency_cannot_keep_its_verdict(self):
        sites = self.sites('import subprocess\nsubprocess.run(command)\n')
        helper = self.root / 'scripts/helper.py'
        helper.write_text('value = 1\n')
        record = self.record(sites, 'qualified-via-owner')
        sites[0]['requires'] = ['scripts/helper.py']
        report = classifier.classify(self.root, sites, record)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertEqual(report['counts']['unknown']['rows'], 1)

    def test_rust_program_spans_bind_the_actual_source_bytes(self):
        self.path = 'scripts/consumer.rs'
        self.file = self.root / self.path
        self.file.write_text('fn f() { Command::new(program); }\n')
        start = self.file.read_bytes().index(b'program')
        sites = [dict(path=self.path, line=1, kind='Rust', program='program', status='unclassified',
                      program_span=dict(start=start, end=start + 7))]
        record = self.record(sites, 'qualified-via-owner')
        self.assertEqual(classifier.classify(self.root, sites, record)['counts']['qualified-via-owner']['rows'], 1)
        sites[0]['program_span']['start'] += 1
        forged = self.record(sites, 'qualified-via-owner')
        report = classifier.classify(self.root, sites, forged)
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)
        self.assertEqual(report['counts']['unknown']['rows'], 1)

    def test_new_rust_sibling_modules_invalidate_their_callers(self):
        self.path = 'scripts/consumer.rs'
        self.file = self.root / self.path
        self.file.write_text('mod helper;\nfn f() { Command::new(program); }\n')
        sites = [dict(path=self.path, line=2, kind='Rust', program='program', status='unclassified')]
        record = self.record(sites, 'qualified-via-owner')
        (self.root / 'scripts/helper.rs').write_text('pub fn value() {}\n')
        report = classifier.classify(self.root, sites, record)
        self.assertFalse(report['evidence_valid'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 0)

    def test_unrelated_rust_module_edits_do_not_invalidate_sibling_imports(self):
        self.path = 'scripts/consumer.rs'
        self.file = self.root / self.path
        self.file.write_text('mod helper;\nfn f() { Command::new(program); }\n')
        unrelated = self.root / 'elsewhere/helper.rs'
        unrelated.parent.mkdir()
        unrelated.write_text('pub const VALUE: u8 = 1;\n')
        sites = [dict(path=self.path, line=2, kind='Rust', program='program', status='unclassified')]
        record = self.record(sites, 'qualified-via-owner')
        unrelated.write_text('pub const VALUE: u8 = 2;\n')
        report = classifier.classify(self.root, sites, record)
        self.assertTrue(report['evidence_valid'], report['problems'])
        self.assertEqual(report['counts']['qualified-via-owner']['rows'], 1)

    def test_refresh_does_not_label_stale_dependency_evidence_as_carried(self):
        helper = self.root / 'scripts/helper.py'
        helper.write_text('value = 1\n')
        sites = self.sites('import helper, subprocess\nsubprocess.run(command)\n')
        record = self.record(sites, 'qualified-via-owner')
        helper.write_text('value = 2\n')
        proposal = classifier.refresh(self.root, sites, record, [self.path])
        self.assertEqual(proposal['files'][self.path]['carried_sites'], [])
        self.assertTrue(proposal['files'][self.path]['changed_sites'])


if __name__ == '__main__':
    unittest.main()
