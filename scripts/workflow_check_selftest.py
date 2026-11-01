#!/usr/bin/env python3
"""Tests of scripts/workflow_check.py: action pins, required triggers and
concurrency, on fixtures and on a copy of this repository's workflows."""
import importlib.util
import pathlib
import re
import shutil
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
SPEC = importlib.util.spec_from_file_location('workflow_check', HERE / 'workflow_check.py')
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)

SHA = '0123456789abcdef0123456789abcdef01234567'


def problems(text):
    """The pin problems of one step file's text, as the full check reports them."""
    with tempfile.TemporaryDirectory() as d:
        path = pathlib.Path(d) / 'w.yml'
        path.write_text(text)
        return check.pin_problems(pathlib.Path(d), path, set())


def steps(*lines):
    return 'jobs:\n  a:\n    runs-on: ubuntu-24.04\n    steps:\n' + ''.join(f'      {line}\n' for line in lines)


class Steps(unittest.TestCase):
    def test_movable_refs_are_reported(self):
        for step in (['- uses: actions/checkout@v4'], ['- uses : actions/checkout@v4'],
                     ['- "uses" : actions/checkout@v4'], ["- 'uses': actions/checkout@v4"],
                     [f'- uses: actions/checkout@{SHA.upper()}'], [f'- uses: evil/repo@{SHA},moving'],
                     [f'- uses: evil/repo@{SHA}}}'], [f"- uses: evil/repo@{SHA}'"],
                     [f'- uses: "evil/repo@{SHA},moving"'], [f'- uses: "evil/repo@{SHA}\\""'],
                     ['- name: x', '  uses: actions/checkout@v4 # v4'], [f'- uses: docker://alpine:3'],
                     [f'- uses: actions/checkout@{SHA}', '    @v4']):
            with self.subTest(step=step):
                self.assertEqual(len(problems(steps(*step))), 1, problems(steps(*step)))

    def test_forms_the_reader_does_not_decode_are_reported(self):
        for step in (['- "\\u0075ses": actions/checkout@v4'], ['- {uses: actions/checkout@v4}'],
                     [f'- {{uses: "evil/repo@{SHA},moving"}}'], ['- &a uses: actions/checkout@v4'],
                     ['- !!str uses: actions/checkout@v4'], ['- ? uses', '  : actions/checkout@v4'],
                     ['- <<: *step'], ['- [uses: actions/checkout@v4]']):
            with self.subTest(step=step):
                self.assertTrue(problems(steps(*step)), step)
        self.assertTrue(problems('jobs:\n  a:\n    steps: *common\n'))

    def test_pinned_and_local_steps_pass(self):
        for step in ([f'- uses: actions/checkout@{SHA} # v4.2.2'], [f'- uses: "actions/checkout@{SHA}"'],
                     [f"- 'uses': 'actions/checkout@{SHA}'"], [f'- "uses" : actions/checkout@{SHA}'],
                     ['- uses: docker://alpine@sha256:' + 'a' * 64], [f'- {{ name: x, uses: actions/checkout@{SHA} }}'],
                     ['- name: x', f'  uses: actions/checkout@{SHA}', '  with:', '    fetch-depth: 0']):
            with self.subTest(step=step):
                self.assertEqual(problems(steps(*step)), [])
        self.assertEqual(problems(f'jobs:\n  a:\n    uses: o/r/.github/workflows/x.yml@{SHA}\n'), [])
        self.assertEqual(problems('permissions: {}\non:\n  push:\n    branches: [main, "release/*"]\n'), [])

    def test_text_in_comments_and_scripts_is_not_a_step(self):
        for text in (steps('# Example: {uses: actions/checkout@v4}', f'- uses: actions/checkout@{SHA} # {{uses: x@v1}}'),
                     steps('- run: \'echo "{uses: actions/checkout@v4}"\''),
                     steps('- run: |', '    uses: actions/checkout@v4', '    - {uses: x@v1}', f'- uses: actions/checkout@{SHA}'),
                     steps('- run: >-', '    uses: actions/checkout@v4'),
                     steps('- name: x', '  run: |', '    uses: actions/checkout@v4'),
                     steps('- |', '  uses: actions/checkout@v4'),
                     steps('- run: "echo', '    uses: actions/checkout@v4"', f'- uses: actions/checkout@{SHA}')):
            with self.subTest(text=text):
                self.assertEqual(problems(text), [])

    def test_anchors_aliases_and_merges_are_expanded(self):
        for text in ('jobs:\n  a:\n    steps: &steps [{uses: actions/checkout@v4}]\n',
                     steps('- &step {"uses":"actions/checkout@v4"}'),
                     'x: &s\n  - uses: actions/checkout@v4\njobs:\n  a:\n    steps: *s\n',
                     steps('- <<: {uses: actions/checkout@v4}', '  name: x')):
            with self.subTest(text=text):
                self.assertTrue(any('actions/checkout@v4' in p for p in problems(text)), problems(text))

    def test_quoted_scripts_over_several_lines_end_where_yaml_ends_them(self):
        # The closing quote after an escaped backslash ends the script, so the next step is read.
        self.assertEqual(len(problems(steps('- run: "echo', '    b\\\\"', '- uses: actions/checkout@v4'))), 1)
        # A single quote inside a double-quoted script does not end it.
        self.assertEqual(problems(steps('- run: "echo it\'s', '    uses: actions/checkout@v4"')), [])

    def test_files_that_are_not_one_readable_document_are_reported(self):
        self.assertTrue(problems(steps(f'- uses: actions/checkout@{SHA}') + '---\n' + steps('- uses: actions/checkout@v4')))
        self.assertTrue(problems('jobs: [\n'))
        self.assertTrue(problems(steps('- uses: [actions/checkout@v4]')))

    def test_keys_are_compared_as_written(self):
        # YAML 1.1 reads the job ids on and yes as one boolean key; GitHub keeps both jobs.
        text = ('jobs:\n  on:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@v4\n'
                '  yes:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: echo harmless\n')
        self.assertEqual(len(problems(text)), 1, problems(text))
        for text in (steps('- uses: actions/checkout@v4', f'  uses: actions/checkout@{SHA}'),
                     'r: &r actions/checkout@v4\n' + steps('- uses: *r'),
                     'k: &k uses\n' + steps('- *k : actions/checkout@v4'),
                     's: &s {uses: actions/checkout@v4}\n' + steps('- <<: *s', '  name: x')):
            with self.subTest(text=text):
                self.assertTrue(any('actions/checkout@v4' in p for p in problems(text)), problems(text))
        self.assertTrue(any('names no anchor' in p for p in problems(steps('- uses: *missing'))))

    def test_a_block_scalar_ends_where_its_indentation_does(self):
        self.assertEqual(len(problems(steps('- run: |', '    echo', '- uses: actions/checkout@v4'))), 1)


class Repository(unittest.TestCase):
    """The complete check on a copy of this repository."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        shutil.copytree(ROOT / '.github', self.root / '.github', ignore=shutil.ignore_patterns('__pycache__'))
        (self.root / 'docs' / 'design').mkdir(parents=True)
        shutil.copy(ROOT / 'docs' / 'design' / 'ci-checks.md', self.root / 'docs' / 'design')
        self.labels = self.root / '.github' / 'workflows' / 'labels.yml'

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, path, text):
        path = self.root / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def call_local(self, ref):
        text = self.labels.read_text()
        self.labels.write_text(text + f'      - uses: {ref}\n')

    def test_the_repository_passes(self):
        self.assertEqual(check.check(self.root), [])

    def test_a_comment_is_not_a_step(self):
        self.labels.write_text(self.labels.read_text() + '# Example: {uses: actions/checkout@v4}\n')
        self.assertEqual(check.check(self.root), [])

    def test_a_spaced_key_in_a_real_workflow(self):
        text = self.labels.read_text()
        self.assertIn('- uses: actions/checkout@', text)
        self.labels.write_text(text.replace('- uses: actions/checkout@' + text.split('- uses: actions/checkout@')[1].split()[0],
                                            '- uses : actions/checkout@v4', 1))
        self.assertTrue(any('actions/checkout@v4' in p for p in check.check(self.root)))

    def test_boolean_job_ids_hide_no_job(self):
        self.labels.write_text(self.labels.read_text() + '  on:\n    runs-on: ubuntu-24.04\n    steps:\n'
                               '      - uses: actions/checkout@v4\n  yes:\n    runs-on: ubuntu-24.04\n    steps:\n'
                               '      - run: echo harmless\n')
        self.assertTrue(any(p.startswith('.github/workflows/labels.yml: line ') and 'actions/checkout@v4' in p
                            for p in check.check(self.root)), check.check(self.root))

    def test_a_yaml_workflow_is_read(self):
        self.write('.github/workflows/probe.yaml', '# workflow-check: not required\non: push\njobs:\n  probe:\n'
                   '    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@v4\n')
        self.assertTrue(any(p.startswith('.github/workflows/probe.yaml: line 7') for p in check.check(self.root)))

    def test_local_actions_are_read_wherever_they_are(self):
        composite = "runs:\n  using: composite\n  steps:\n    - uses: actions/checkout@v4\n"
        for ref, file in (('./tools/probe', 'tools/probe/action.yml'), ('./.github/actions/probe', '.github/actions/probe/action.yaml')):
            self.write(file, composite)
        self.write('tools/anchored/action.yml', 'runs:\n  using: composite\n  steps: &s [{uses: actions/checkout@v4}]\n')
        self.call_local('./tools/anchored')
        for ref in ('./tools/probe', './.github/actions/probe'):
            self.call_local(ref)
        found = check.check(self.root)
        for file in ('tools/probe/action.yml', '.github/actions/probe/action.yaml', 'tools/anchored/action.yml'):
            self.assertTrue(any(p.startswith(f'{file}: line ') and 'actions/checkout@v4' in p for p in found), (file, found))

    def ci(self, old, new):
        ci = self.root / '.github' / 'workflows' / 'ci.yml'
        text = ci.read_text()
        self.assertIn(old, text)
        ci.write_text(text.replace(old, new, 1))
        return check.check(self.root)

    def test_a_job_no_report_waits_for_is_reported(self):
        found = self.ci(', conformance-arm64, bench-guard]\n', ', conformance-arm64]\n')
        self.assertIn('ci.yml: no report job waits for bench-guard', found)

    def test_a_job_without_its_concurrency_group_is_reported(self):
        group = ('    concurrency:\n      group: ${{ github.workflow }}-msrv-'
                 "${{ github.event_name == 'pull_request' && github.ref || github.run_id }}\n"
                 "      cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n")
        self.assertTrue(any(p.startswith('ci.yml: give `msrv` exactly') for p in self.ci(group, '')))

    def test_a_group_another_job_has_is_reported(self):
        found = self.ci('${{ github.workflow }}-msrv-', '${{ github.workflow }}-check-')
        self.assertTrue(any(p.startswith('ci.yml: give `msrv` exactly') for p in found), found)

    def test_a_workflow_without_any_job_group_is_reported(self):
        ci = self.root / '.github' / 'workflows' / 'ci.yml'
        text = ci.read_text()
        bare = re.sub(r'    concurrency:\n      group: .*\n      cancel-in-progress: .*\n', '', text)
        self.assertNotIn('concurrency:', bare)
        ci.write_text(bare)
        found = check.check(self.root)
        self.assertTrue(any(p.startswith('ci.yml: give `check` exactly') for p in found), found)

    def test_matrix_legs_sharing_a_group_are_reported(self):
        found = self.ci('-b9-shard-${{ strategy.job-index }}-', '-b9-shard-')
        self.assertTrue(any(p.startswith('ci.yml: give `b9-shard` exactly') for p in found), found)

    def test_a_report_job_with_a_group_is_reported(self):
        found = self.ci('  ci-report:\n', '  ci-report:\n    concurrency:\n      group: x\n')
        self.assertTrue(any(p.startswith('ci.yml: remove `ci-report`') for p in found), found)

    def test_a_workflow_group_beside_the_job_groups_is_reported(self):
        found = self.ci('\njobs:\n', '\nconcurrency:\n  group: ci\n\njobs:\n')
        self.assertTrue(any('remove the workflow\'s own `concurrency:`' in p for p in found), found)

    def edit(self, workflow, old, new):
        """The problems of the repository with one edit of a workflow, which
        is then undone."""
        path = self.root / '.github' / 'workflows' / workflow
        text = path.read_text()
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1))
        try:
            return check.check(self.root)
        finally:
            path.write_text(text)

    def test_a_required_workflow_without_a_merge_group_trigger_is_reported(self):
        missing = ('deny.yml: required `deny`, but the workflow has no `merge_group:` trigger '
                   '(`types: [checks_requested]`), so the merge queue waits for the check until it times out '
                   '(docs/design/ci-checks.md, "The merge queue")')
        self.assertEqual(self.edit('deny.yml', '  merge_group:\n    types: [checks_requested]\n', ''), [missing])
        self.assertEqual(self.edit('deny.yml', 'types: [checks_requested]', 'types: [destroyed]'),
                         ['deny.yml: `merge_group:` types must include `checks_requested`: '
                          'use `types: [checks_requested]`'])
        self.assertEqual(self.edit('deny.yml', '  merge_group:\n    types: [checks_requested]\n',
                                   '  merge_group:\n'), [])

    def test_the_activity_type_is_matched_as_a_whole_item(self):
        invalid = 'deny.yml: `merge_group:` types must include `checks_requested`'
        for types in ('[not_checks_requested]', '[checks_requested_later]', '[destroyed] # checks_requested',
                      '["destroyed, checks_requested"]'):
            with self.subTest(types=types):
                found = self.edit('deny.yml', 'types: [checks_requested]', f'types: {types}')
                self.assertEqual(len(found), 1, found)
                self.assertTrue(found[0].startswith(invalid), found)
        for types in ('[destroyed, checks_requested]', "['checks_requested']", '[checks_requested] # queue',
                      'checks_requested', '\n      - checks_requested'):
            with self.subTest(types=types):
                self.assertEqual(self.edit('deny.yml', 'types: [checks_requested]', f'types: {types}'), [])

    def test_a_merge_group_branch_filter_is_reported(self):
        filtered = ('deny.yml: required `deny`, but the `merge_group:` trigger carries a branch filter, so a '
                    'group the filter leaves out never reports the check: remove it '
                    '(docs/design/ci-checks.md, "The merge queue")')
        for key in ('branches: [release]', 'branches: [main]', 'branches-ignore: [main]'):
            with self.subTest(key=key):
                self.assertEqual(self.edit('deny.yml', '    types: [checks_requested]\n',
                                           f'    types: [checks_requested]\n    {key}\n'), [filtered])
        # A branch filter on another trigger is not the queue's concern.
        self.assertEqual(self.edit('deny.yml', 'push:\n    branches: [main]', 'push:\n    branches: [release]'), [])

    def test_a_required_workflow_without_its_workflow_group_is_reported(self):
        path = self.root / '.github' / 'workflows' / 'npm.yml'
        text = path.read_text()
        bare = re.sub(r'\nconcurrency:\n(?:  .*\n)+', '\n', text)
        self.assertNotIn('\nconcurrency:', bare)
        path.write_text(bare)
        found = check.check(self.root)
        self.assertEqual(len(found), 1, found)
        self.assertTrue(found[0].startswith('npm.yml: required `npm-report`, so give the workflow exactly '
                                            '`concurrency:'), found)

    def test_yaml_trigger_keys_cannot_hide_types_or_branch_filters(self):
        for key in ('"types"', "'types'", 'types ', '"typ\\u0065s"'):
            with self.subTest(key=key):
                found = self.edit('deny.yml', 'types: [checks_requested]', f'{key}: [destroyed]')
                self.assertEqual(found, ['deny.yml: `merge_group:` types must include `checks_requested`: '
                                         'use `types: [checks_requested]`'])
        for key in ('"branches"', "'branches'", 'branches ', '"branch\\u0065s"',
                    '"branches-ignore"', 'branches-ignore '):
            with self.subTest(key=key):
                found = self.edit('deny.yml', '    types: [checks_requested]\n',
                                  f'    types: [checks_requested]\n    {key}: [release]\n')
                self.assertTrue(any('branch filter' in p for p in found), found)

    def test_yaml_trigger_mappings_are_read_regardless_of_layout(self):
        for event in ('"merge_group"', 'merge_group ', '"merge_\\u0067roup"'):
            for options in ('{types: [destroyed]}', '{types: [checks_requested], "branches" : [release]}'):
                with self.subTest(event=event, options=options):
                    found = self.edit('deny.yml', '  merge_group:\n    types: [checks_requested]',
                                      f'  {event}: {options}')
                    self.assertTrue(found, found)
        self.assertEqual(self.edit('deny.yml', 'on:\n', '"on" :\n'), [])
        self.assertEqual(self.edit('deny.yml', '  merge_group:\n    types: [checks_requested]',
                                   '  "merge_group" : {"types" : [checks_requested]}'), [])

    def test_unreadable_or_ambiguous_trigger_options_fail_closed(self):
        for replacement in ('    types: [checks_requested]\n    "types": [destroyed]',
                            '    <<: {branches: [release]}\n    types: [checks_requested]',
                            '    types: &types [checks_requested]\n    branches: *types',
                            '    types: !!str checks_requested',
                            '    ? [types]\n    : [destroyed]'):
            with self.subTest(replacement=replacement):
                found = self.edit('deny.yml', '    types: [checks_requested]', replacement)
                self.assertTrue(any('not readable as YAML' in p for p in found), found)
        found = self.edit('deny.yml', 'on:\n', '<<: {on: {merge_group: {branches: [release]}}}\non:\n')
        self.assertTrue(any('expand YAML merge keys' in p for p in found), found)

    def test_a_workflow_group_that_cancels_or_joins_merge_groups_is_reported(self):
        group = "${{ github.workflow }}-${{ github.event_name == 'pull_request' && github.ref || github.run_id }}"
        for old, new in ((group, '${{ github.workflow }}-${{ github.ref }}'),
                         ("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", 'cancel-in-progress: true')):
            with self.subTest(new=new):
                found = self.edit('python.yml', old, new)
                self.assertEqual(len(found), 1, found)
                self.assertTrue(found[0].startswith('python.yml: required `python-report`, so give the workflow '
                                                    'exactly `concurrency:'), found)

    def test_a_pin_change_starts_every_workflow_that_installs_the_oracle(self):
        workflows = self.root / '.github' / 'workflows'
        audio = workflows / 'audio-conformance.yml'
        text = audio.read_text()
        self.assertEqual(text.count(' "scripts/install-oracle-ffmpeg.sh",'), 2)
        audio.write_text(text.replace(' "scripts/install-oracle-ffmpeg.sh",', ''))
        mediainfo = workflows / 'mediainfo-oracle.yml'
        text = mediainfo.read_text()
        self.assertIn("      - '*.md'\n", text)
        self.assertIn('      - scripts/oracle-ffmpeg.env\n', text)
        mediainfo.write_text(text.replace("      - '*.md'\n", "      - '*.md'\n      - 'scripts/*.env'\n")
                             .replace('      - scripts/oracle-ffmpeg.env\n', ''))

        def problem(workflow, pin, fix, key, event):
            return (f'{workflow}: runs the oracle FFmpeg installer, but a change of scripts/{pin} '
                    f'does not start it: {fix} the `{key}` of `{event}`')
        self.assertEqual(sorted(check.check(self.root)), sorted([
            problem('audio-conformance.yml', 'install-oracle-ffmpeg.sh', 'add it to', 'paths', 'push'),
            problem('audio-conformance.yml', 'install-oracle-ffmpeg.sh', 'add it to', 'paths', 'pull_request'),
            problem('mediainfo-oracle.yml', 'oracle-ffmpeg.env', 'take it out of', 'paths-ignore', 'push'),
            problem('mediainfo-oracle.yml', 'oracle-ffmpeg.env', 'add it to', 'paths', 'pull_request'),
        ]))

    def test_only_a_workflow_that_runs_the_installer_must_start_on_the_pin(self):
        def workflow(*steps):
            return ('# workflow-check: not required\non:\n  pull_request:\n'
                    '    paths: ["scripts/install-oracle-ffmpeg.sh"]\njobs:\n  probe:\n'
                    '    runs-on: ubuntu-24.04\n    steps:\n' + ''.join(f'      {step}\n' for step in steps))
        missing = ('probe.yml: runs the oracle FFmpeg installer, but a change of scripts/oracle-ffmpeg.env '
                   'does not start it: add it to the `paths` of `pull_request`')
        for steps in (['- run: bash -n scripts/install-oracle-ffmpeg.sh'],
                      ['- run: shellcheck scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash -en scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash -o noexec scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash -D scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash -e -- -x scripts/install-oracle-ffmpeg.sh'],
                      ['- run: "bash -n scripts/install-oracle-ffmpeg.sh"'],
                      ['- run: |', "    echo done # it's scripts/install-oracle-ffmpeg.sh /opt/bin"],
                      ["- run: bash -c 'set -e; bash -n scripts/install-oracle-ffmpeg.sh'"],
                      ['- run: echo "scripts/install-oracle-ffmpeg.sh; x"'],
                      ['- run: echo "oops scripts/install-oracle-ffmpeg.sh'],
                      ['- run: "echo scripts/install-oracle-ffmpeg.sh"'],
                      ['- run: bash -O scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash -s scripts/install-oracle-ffmpeg.sh'],
                      ['- run: sh -s scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash -n +o pipefail scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash --rcfile scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash --dump-strings scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash --dump-po-strings scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash --help scripts/install-oracle-ffmpeg.sh'],
                      ['- run: bash --version scripts/install-oracle-ffmpeg.sh'],
                      ['# scripts/install-oracle-ffmpeg.sh "$RUNNER_TEMP/bin"', '- run: echo none']):
            with self.subTest(steps=steps):
                self.write('.github/workflows/probe.yml', workflow(*steps))
                self.assertEqual(check.check(self.root), [])
        for steps in (['- run: scripts/install-oracle-ffmpeg.sh "$RUNNER_TEMP/bin"'],
                      ['- name: x', '  run: bash scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -e scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -eo pipefail scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -e -- scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: "bash -e scripts/install-oracle-ffmpeg.sh /opt/bin"'],
                      ["- run: 'bash -e scripts/install-oracle-ffmpeg.sh /opt/bin'"],
                      ['- run: "bash -c \'scripts/install-oracle-ffmpeg.sh /opt/bin\'"'],
                      ['- run: |', '    bash -e scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: >', '    bash -e scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -O extglob scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -c \'scripts/install-oracle-ffmpeg.sh /opt/bin\''],
                      ['- run: sh -c "./scripts/install-oracle-ffmpeg.sh"'],
                      ['- run: bash -c -x scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -cs \'scripts/install-oracle-ffmpeg.sh /opt/bin\''],
                      ["- run: bash -c 'set -e; scripts/install-oracle-ffmpeg.sh /opt/bin'"],
                      ["- run: bash -c 'set -e; bash -e scripts/install-oracle-ffmpeg.sh /opt/bin'"],
                      ['- run: cd /tmp && scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: |', "    scripts/install-oracle-ffmpeg.sh /opt/bin # don't skip it"],
                      ['- run: echo a#b; scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash +o noexec scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -n +n scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash -o noexec +o noexec scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash +O extglob scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: bash --rcfile /dev/null scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: sudo bash --norc -x scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: |', '    set -e', '', '    ./scripts/install-oracle-ffmpeg.sh "$RUNNER_TEMP/bin"']):
            with self.subTest(steps=steps):
                self.write('.github/workflows/probe.yml', workflow(*steps))
                self.assertEqual(check.check(self.root), [missing])

    def test_trigger_filters_are_read_from_the_yaml_nodes(self):
        text = ('on:\n  push:\n    paths:\n      # a note\n      - "scripts/install-oracle-ffmpeg.sh"\n'
                "      - 'scripts/oracle-ffmpeg.env'\n  pull_request:\n    paths-ignore: [ 'docs/**',\n"
                '      "x" ]\n    paths:\n      - *pins\njobs: {}\n')
        self.assertEqual(check.trigger_filters(text.replace('      - *pins\n', '      - y\n')), [
            ('push', 'paths', ['scripts/install-oracle-ffmpeg.sh', 'scripts/oracle-ffmpeg.env']),
            ('pull_request', 'paths', ['y']),
            ('pull_request', 'paths-ignore', ['docs/**', 'x'])])
        self.assertIsInstance(check.trigger_filters(text), str)
        self.assertEqual(check.trigger_filters('on:\n  push:\n    paths: scripts/x\njobs: {}\n'), [('push', 'paths', None)])

    def test_a_local_ref_to_nothing_is_reported(self):
        self.call_local('./tools/none')
        self.assertTrue(any('names no action or workflow file' in p for p in check.check(self.root)))


if __name__ == '__main__':
    unittest.main()
