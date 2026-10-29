#!/usr/bin/env python3
"""Keeps the required status checks and the workflows in step
(docs/design/ci-checks.md). Fails, naming the workflow, when:

- two jobs across .github/workflows would report the same check name (a
  branch ruleset matches a required check by name, so a repeated name
  cannot be required);
- a check name listed as required in docs/design/ci-checks.md is no
  job's check name, or a report job exists
  whose name is not listed there
  (a workflow that carries the line `# workflow-check: not required`
  may not be listed, and its report job need not be);
- a required job does not always run: a workflow its trigger's `paths` or
  `paths-ignore` skips reports nothing at all, and a required check that
  never reports leaves every pull request pending;
- a report job does not carry `if: always()`, or does not wait for every
  other job of its workflow, so that a failure below it could pass
  unnoticed;
- a filter pattern passed to .github/actions/changed-paths is one GitHub
  and the action would read differently;
- a step in a workflow or in a local action (.github/actions/*/action.yml)
  uses an action by a tag or branch instead of a full commit sha, so the
  code it runs can change under the same name.

    workflow_check.py [--root DIR]

Exit status 0 when the workflows and the note agree, 1 otherwise. Reads the
files as text: the workflows are written in the narrow shape this parser
accepts, and anything else is an error rather than a silent pass. Python
3.11 standard library only.
"""
import argparse
import importlib.util
import pathlib
import re
import sys

JOB = re.compile(r'\A {2}([A-Za-z0-9_-]+):\s*\Z')
FIELD = re.compile(r'\A {4}([a-z-]+):\s*(.*?)\s*\Z')
TOP = re.compile(r'\A\S')
FILTER = re.compile(r'\A( +)(paths|paths-ignore):(.*)\Z')
USES = re.compile(r'\A\s+uses: \./\.github/actions/changed-paths/?\s*\Z')
PULL_REQUEST = re.compile(r'\A {2}pull_request:\s*\Z')
TRIGGER = re.compile(r'\A {2}\S')
ROW = re.compile(r'\A\| `([^`]+)` \| `([^`]+)` \|')
SECTION = '### Required status checks (exact names)'
LABELS = 'github.event.pull_request.labels'
# A block step's value runs to the end of the line or a comment; a flow
# mapping's (`{uses: ...}`) ends at its comma or brace.
BLOCK_USES = re.compile(r'''\A\s*(?:-\s+)?(['"]?)uses\1:\s*(.*?)\s*(?:\s#.*)?\Z''')
FLOW_USES = re.compile(r'''[{,]\s*(['"]?)uses\1:\s*([^,}]*)''')
PINNED = re.compile(r'\A[^@\s]+@[0-9a-f]{40}\Z|\Adocker://[^@\s]+@sha256:[0-9a-f]{64}\Z')
# A workflow whose checks the table does not require, such as one this
# repository runs for itself alone.
UNLISTED = '# workflow-check: not required'


def load_changed_paths(root):
    spec = importlib.util.spec_from_file_location(
        'changed_paths', root / '.github' / 'actions' / 'changed-paths' / 'changed_paths.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def jobs_of(lines):
    """{job id: {'name': str | None, 'needs': [str], 'always': bool}}.

    `needs:` is read in its inline forms, `needs: a` and `needs: [a, b]`; a
    block sequence reads as no dependency at all, which the caller sees as
    a report job that waits for nothing.
    """
    jobs, current, reading_if = {}, None, False
    inside = False
    for line in lines:
        if line.rstrip() == 'jobs:':
            inside = True
            continue
        if not inside:
            continue
        if TOP.match(line):
            break
        match = JOB.match(line)
        if match:
            current = {'name': None, 'needs': [], 'always': False, 'if': ''}
            reading_if = False
            jobs[match.group(1)] = current
            continue
        if current is None:
            continue
        field = FIELD.match(line)
        if not field:
            if reading_if and line.startswith(' ' * 6):
                current['if'] += ' ' + line.strip()
            continue
        reading_if = False
        key, value = field.group(1), field.group(2)
        if key == 'name':
            current['name'] = value
        elif key == 'needs':
            current['needs'] = [n.strip() for n in value.strip('[]').split(',') if n.strip()]
        elif key == 'if':
            current['always'] = value == 'always()'
            current['if'] = value
            reading_if = value in ('>-', '>', '|', '|-')
    return jobs


def trigger(lines):
    """(a `paths` or `paths-ignore` key is in the `on:` block, the block
    has a `pull_request:` trigger that an ordinary push to the branch
    reaches)."""
    filtered, pull_request, inside, seen_pull_request = False, False, False, False
    for line in lines:
        if line.rstrip() == 'on:':
            inside = True
            continue
        if not inside:
            continue
        if TOP.match(line):
            break
        # Any form: a block sequence, a flow sequence on the same line, or
        # a block scalar. None of them may gate a required workflow.
        if re.match(r'\A +(paths|paths-ignore):', line):
            filtered = True
        if PULL_REQUEST.match(line):
            pull_request = True
            seen_pull_request = True
            continue
        if seen_pull_request and re.match(r'\A {4}types:', line):
            pull_request = 'synchronize' in line
        elif seen_pull_request and TRIGGER.match(line):
            seen_pull_request = False
    return filtered, pull_request


def filter_patterns(lines):
    """Every pattern passed to the changed-paths action, as (key, pattern),
    and a problem for each step that passes no filter or writes it in a
    form this cannot read. The patterns have to be checkable, so only the
    block scalar `paths: |` is accepted under the action's `with:`."""
    out, problems = [], []
    for i, line in enumerate(lines):
        if not USES.match(line):
            continue
        step = len(line) - len(line.lstrip())
        found = False
        for j in range(i + 1, len(lines)):
            item = lines[j]
            if not item.strip():
                break
            indent = len(item) - len(item.lstrip())
            if indent < step or item.lstrip().startswith('- '):
                break
            match = FILTER.match(item)
            if not match:
                continue
            found = True
            if match.group(3).strip() != '|':
                problems.append(f'line {j + 1}: the changed-paths filter is not a `|` block')
                continue
            for pattern in lines[j + 1:]:
                # A blank line is part of a block scalar, not its end.
                if not pattern.strip():
                    continue
                if len(pattern) - len(pattern.lstrip()) <= len(match.group(1)):
                    break
                text = pattern.strip()
                if not text.startswith('#'):
                    out.append((match.group(2), text))
        if not found:
            problems.append(f'line {i + 1}: the changed-paths step passes no filter')
    return out, problems


def label_gated(jobs):
    """The jobs whose condition reads the pull request's labels, and the
    jobs that wait for one of those."""
    gated = {job for job, fields in jobs.items() if LABELS in fields['if']}
    while True:
        more = {job for job, fields in jobs.items()
                if job not in gated and gated.intersection(fields['needs'])}
        if not more:
            return gated
        gated |= more


def required_names(note):
    """The check names of the note's required-check table."""
    lines = note.read_text(encoding='utf-8').splitlines()
    if SECTION not in lines:
        raise SystemExit(f'workflow-check: {note} has no "{SECTION}" section')
    names = []
    for line in lines[lines.index(SECTION):]:
        if line.startswith('#') and names:
            break
        match = ROW.match(line)
        if match and match.group(1) != 'Check':
            names.append((match.group(1), match.group(2)))
    return names


def unlisted_workflows(root):
    """The file names of the workflows that carry the line UNLISTED: their
    checks are not required in the table."""
    return {workflow.name for workflow in (root / '.github' / 'workflows').glob('*.yml')
            if UNLISTED in (line.strip() for line in workflow.read_text(encoding='utf-8').splitlines())}


def unpinned(lines):
    """Steps that use an action by a name whose code can move."""
    out = []
    for n, line in enumerate(lines, 1):
        block = BLOCK_USES.match(line)
        found = [block] if block and not block.group(2).startswith('{') else FLOW_USES.finditer(line)
        for match in found:
            ref = match.group(2).strip().strip('\'"')
            if ref.startswith('./') or PINNED.match(ref):
                continue
            out.append(f'line {n}: `{ref}` is not pinned: use the full commit sha with the '
                       'version as a comment (`owner/repo@<40-hex sha> # vX.Y.Z`), as every other step does')
    return out


def check(root):
    problems = []
    unlisted = unlisted_workflows(root)
    changed_paths = load_changed_paths(root)
    workflows = sorted((root / '.github' / 'workflows').glob('*.yml'))
    jobs = {}        # workflow file name -> job id -> fields
    names = {}       # check name -> [(workflow, job id)]
    for path in workflows:
        lines = path.read_text(encoding='utf-8').splitlines()
        jobs[path.name] = found = jobs_of(lines)
        if not found:
            problems.append(f'{path.name}: no job found; the parser expects two-space job ids')
        for job, fields in found.items():
            names.setdefault(fields['name'] or job, []).append((path.name, job))
        problems.extend(f'{path.name}: {problem}' for problem in unpinned(lines))
        patterns, bad = filter_patterns(lines)
        problems.extend(f'{path.name}: {problem}' for problem in bad)
        for key, pattern in patterns:
            try:
                changed_paths.to_regex(pattern)
            except SystemExit as error:
                problems.append(f'{path.name}: {key}: {error}')

    for action in sorted((root / '.github' / 'actions').glob('*/action.yml')):
        lines = action.read_text(encoding='utf-8').splitlines()
        problems.extend(f'{action.relative_to(root)}: {problem}' for problem in unpinned(lines))

    for name, where in sorted(names.items()):
        if len(where) > 1:
            places = ', '.join(f'{w}:{j}' for w, j in where)
            problems.append(f'check name `{name}` is reported by more than one job: {places}')

    note = root / 'docs' / 'design' / 'ci-checks.md'
    listed = required_names(note)
    for name, workflow in listed:
        if workflow in unlisted:
            problems.append(f'{note.name} requires `{name}` of {workflow}, which is marked not required')
            continue
        where = names.get(name)
        if not where:
            problems.append(f'{note.name} requires `{name}`, which no job reports')
            continue
        if where[0][0] != workflow:
            problems.append(f'{note.name} puts `{name}` in {workflow}, but it is in {where[0][0]}')
            continue
        filtered, pull_request = trigger(
            (root / '.github' / 'workflows' / workflow).read_text().splitlines())
        if filtered:
            problems.append(f'{workflow}: required `{name}`, but the trigger carries a path filter, '
                            'so the check is not reported on every pull request')
        if not pull_request:
            problems.append(f'{workflow}: required `{name}`, but the workflow has no '
                            '`pull_request:` trigger, so the check is never reported on one')
        job = where[0][1]
        fields = jobs[workflow][job]
        if not job.endswith('-report'):
            continue
        if not fields['always']:
            problems.append(f'{workflow}: the report job `{job}` needs `if: always()`')
        # A job whose condition reads the pull request's labels may not be
        # covered: the label set belongs to the pull request, not to the
        # commit, so the same commit would report differently before and
        # after a label changed (docs/design/ci-checks.md).
        labelled = label_gated(jobs[workflow])
        for other in sorted(labelled & set(fields['needs'])):
            problems.append(f'{workflow}: the report job `{job}` waits for `{other}`, '
                            'whose condition reads the pull request labels')
        missing = sorted(set(jobs[workflow]) - {job} - labelled - set(fields['needs']))
        if missing:
            problems.append(f'{workflow}: the report job `{job}` does not wait for '
                            + ', '.join(missing))

    required = {name for name, _ in listed}
    for workflow, found in jobs.items():
        if workflow in unlisted:
            continue
        for job in found:
            if job.endswith('-report') and job not in required:
                problems.append(f'{workflow}: the report job `{job}` is not in {note.name}')
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    problems = check(args.root)
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        print(f'{len(problems)} problem(s); see docs/design/ci-checks.md', file=sys.stderr)
        return 1
    print('workflow-check: the required checks and the workflows agree')
    return 0


if __name__ == '__main__':
    sys.exit(main())
