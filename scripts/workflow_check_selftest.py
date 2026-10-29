#!/usr/bin/env python3
"""Tests of the pin check in scripts/workflow_check.py: the step forms
review found passing or failing wrongly, on fixtures and on a copy of this
repository's workflows."""
import importlib.util
import pathlib
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

    def test_a_yaml_workflow_is_read(self):
        self.write('.github/workflows/probe.yaml', '# workflow-check: not required\non: push\njobs:\n  probe:\n'
                   '    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@v4\n')
        self.assertTrue(any(p.startswith('.github/workflows/probe.yaml: line 7') for p in check.check(self.root)))

    def test_local_actions_are_read_wherever_they_are(self):
        composite = "runs:\n  using: composite\n  steps:\n    - uses: actions/checkout@v4\n"
        for ref, file in (('./tools/probe', 'tools/probe/action.yml'), ('./.github/actions/probe', '.github/actions/probe/action.yaml')):
            self.write(file, composite)
            self.call_local(ref)
        found = check.check(self.root)
        for file in ('tools/probe/action.yml', '.github/actions/probe/action.yaml'):
            self.assertTrue(any(p.startswith(f'{file}: line 4') for p in found), (file, found))

    def test_a_local_ref_to_nothing_is_reported(self):
        self.call_local('./tools/none')
        self.assertTrue(any('names no action or workflow file' in p for p in check.check(self.root)))


if __name__ == '__main__':
    unittest.main()
