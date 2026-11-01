#!/usr/bin/env python3
"""Tests of the pin check in scripts/workflow_check.py: the step forms
review found passing or failing wrongly, on fixtures and on a copy of this
repository's workflows."""
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
                      ['- run: sudo bash --norc -x scripts/install-oracle-ffmpeg.sh /opt/bin'],
                      ['- run: |', '    set -e', '', '    ./scripts/install-oracle-ffmpeg.sh "$RUNNER_TEMP/bin"']):
            with self.subTest(steps=steps):
                self.write('.github/workflows/probe.yml', workflow(*steps))
                self.assertEqual(check.check(self.root), [missing])

    def test_a_local_ref_to_nothing_is_reported(self):
        self.call_local('./tools/none')
        self.assertTrue(any('names no action or workflow file' in p for p in check.check(self.root)))


if __name__ == '__main__':
    unittest.main()
