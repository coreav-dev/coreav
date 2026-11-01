import importlib.util
import json
import pathlib
import re
import subprocess
import unittest
from unittest.mock import patch


ROOT = pathlib.Path(__file__).resolve().parents[4]
SPEC = importlib.util.spec_from_file_location(
    'changed_paths', ROOT / '.github/actions/changed-paths/changed_paths.py')
cp = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cp)
WORKFLOW_SPEC = importlib.util.spec_from_file_location(
    'workflow_check', ROOT / 'scripts/workflow_check.py')
workflow_check = importlib.util.module_from_spec(WORKFLOW_SPEC)
WORKFLOW_SPEC.loader.exec_module(workflow_check)


class PackageFilters(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.filters = {}
        for workflow in ('ci', 'npm', 'python'):
            script = ('job = Psych.load_file(ARGV[0])["jobs"][ARGV[1]]; '
                      'puts JSON.generate(job["steps"].find { |s| s["id"] == "filter" }["with"])')
            text = subprocess.check_output(
                ['ruby', '-rjson', '-rpsych', '-e', script,
                 str(ROOT / '.github/workflows' / (workflow + '.yml')), workflow + '-changes'],
                text=True)
            settings = json.loads(text)
            cls.filters[workflow] = (cp.patterns_of(settings.get('paths', '')),
                                     cp.patterns_of(settings.get('paths-ignore', '')))

    def selected(self, changed):
        return {name: cp.decide(changed, *settings)[0] for name, settings in self.filters.items()}

    def test_wasm_package_changes_do_not_run_native_suites(self):
        changed = ['packages/wasm/src/workers.js', 'packages/wasm/src/r2.js',
                   'packages/wasm/package.json', 'packages/wasm/scripts/build.sh',
                   'packages/wasm/test/streams.test.mjs', 'docs/design/wasm.md']
        self.assertEqual(self.selected(changed), dict.fromkeys(self.filters, False))

    def test_wasm_triggers_cover_module_fixture_inputs(self):
        helpers = re.findall(r'\$SCRIPTS/([A-Za-z0-9_.-]+)',
                             (ROOT / 'scripts/gen-fixtures.sh').read_text())
        inputs = {f'scripts/{name}' for name in helpers}
        inputs.update(str(path.relative_to(ROOT)) for path in
                      (ROOT / 'docs/schema/policy-examples').rglob('*.json'))
        self.assertTrue(helpers)
        filters = workflow_check.trigger_filters(
            (ROOT / '.github/workflows/wasm.yml').read_text())
        self.assertIsInstance(filters, list)
        events = {event for event, _, _ in filters}
        self.assertTrue({'push', 'pull_request'} <= events)
        for event, key, patterns in filters:
            self.assertIsNotNone(patterns)
            for path in sorted(inputs):
                with self.subTest(event=event, path=path):
                    selected, _ = cp.decide([path], patterns if key == 'paths' else [],
                                            patterns if key == 'paths-ignore' else [])
                    self.assertTrue(selected, f'add {path} to the WASM {event} trigger')

    def test_cli_package_and_shared_build_inputs_keep_native_validation(self):
        for path in ('packages/cli/package.json', 'packages/cli/scripts/codecs.sh',
                     'packages/cli/test/cli.test.mjs', 'crates/coreav-cli/src/lib.rs',
                     'crates/coreav-py/src/lib.rs', 'Cargo.lock',
                     'docs/schema/policy-examples/web-upload.json', 'scripts/oracle-ffmpeg.env',
                     '.github/actions/changed-paths/changed_paths.py'):
            with self.subTest(path=path):
                self.assertEqual(self.selected([path]), dict.fromkeys(self.filters, True))

    def test_wasm_crate_and_other_rust_inputs_still_run_rust_ci(self):
        for path in ('crates/coreav-wasm/src/bindings.rs', 'crates/coreav-isobmff/src/source.rs',
                     'Cargo.toml', 'rust-toolchain.toml'):
            with self.subTest(path=path):
                self.assertTrue(self.selected([path])['ci'])

    def test_mixed_changes_and_renames_keep_native_validation(self):
        for paths in (['packages/wasm/src/node.js', 'packages/cli/lib/main.js'],
                      ['packages/cli/lib/main.js', 'packages/wasm/src/node.js'],
                      ['packages/wasm/src/node.js', 'Cargo.lock']):
            with self.subTest(paths=paths):
                self.assertEqual(self.selected(paths), dict.fromkeys(self.filters, True))

    def test_unknown_or_empty_diffs_run_native_suites(self):
        for changed in (None, []):
            with self.subTest(changed=changed):
                self.assertEqual(self.selected(changed), dict.fromkeys(self.filters, True))

    def test_each_workflow_change_keeps_its_own_validation(self):
        for name in self.filters:
            with self.subTest(workflow=name):
                self.assertTrue(self.selected(['.github/workflows/' + name + '.yml'])[name])

    def test_pr_push_and_merge_group_apply_the_same_package_filters(self):
        for event in ('pull_request', 'push', 'merge_group'):
            for paths, expected in ((['packages/wasm/src/node.js'], False),
                                    (['packages/cli/lib/main.js', 'packages/wasm/src/node.js'], True)):
                with self.subTest(event=event, paths=paths):
                    def git(*args):
                        return '\0'.join(paths) + '\0' if 'diff' in args else ''

                    with patch.object(cp, 'git', side_effect=git) as calls:
                        changed = cp.changed_files(event, 'before', 'group-base')
                    self.assertEqual(self.selected(changed), dict.fromkeys(self.filters, expected))
                    base = {'pull_request': 'HEAD^1', 'push': 'before', 'merge_group': 'group-base'}[event]
                    calls.assert_any_call('-c', 'core.quotePath=false', 'diff', '--name-only',
                                          '--no-renames', '-z', base, 'HEAD')
                    if event == 'merge_group':
                        calls.assert_any_call('fetch', '--quiet', '--no-tags', '--depth=1', 'origin', 'group-base')

    def test_unreadable_diffs_keep_native_validation_for_each_event(self):
        for event in ('pull_request', 'push', 'merge_group'):
            with self.subTest(event=event), patch.object(cp, 'git', side_effect=OSError('unreachable')):
                changed = cp.changed_files(event, 'before', 'group-base')
                self.assertEqual(self.selected(changed), dict.fromkeys(self.filters, True))


if __name__ == '__main__':
    unittest.main()
