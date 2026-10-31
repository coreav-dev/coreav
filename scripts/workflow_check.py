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
- a report job does not carry `if: always()`, or a job of a workflow
  with a required report job is waited for by none of them, so that its
  failure could pass unnoticed;
- a workflow cancels a superseded run job by job (it is in
  `JOB_GROUPS`, or a job carries a `concurrency:` block) and a job other
  than a report job has no group, or not exactly `GROUP` with its own job
  id (and `MATRIX` after it for a job with a `strategy:`, whose legs would
  otherwise share one group) and `cancel-in-progress: CANCEL`, or a report job has a group, or the
  workflow has a group of its own as well: a workflow group keeps the next
  run waiting for the old run's report jobs, a group two jobs share
  cancels one of them in every run, a gate job without one can start an
  old run's jobs after the new run's, which then cancel the new ones, and
  a report job in another job's group can cancel that job;
- a filter pattern passed to .github/actions/changed-paths is one GitHub
  and the action would read differently;
- a workflow with a step whose `run:` script calls the oracle FFmpeg's
  installer has a trigger `paths` or `paths-ignore` filter that a change of
  the pin (ORACLE_PIN) does not pass, so a new oracle would not start it;
- a step in a workflow (.yml or .yaml), in a local action
  (.github/actions/*/action.yml or .yaml) or in a local action or workflow
  a step calls (`./...`) uses an action by a tag or branch instead of a
  full lowercase commit sha, so the code it runs can change under the same
  name; or such a file is not one YAML document Ruby's standard library
  (Psych) can read. The pin check walks Psych's node tree, so keys are
  compared as written and anchors, aliases, merge keys, duplicate keys and
  every quoting form hide no step.

    workflow_check.py [--root DIR]

Exit status 0 when the workflows and the note agree, 1 otherwise. Reads the
files as text for the required-check table: the workflows are written in
the narrow shape this parser accepts, and anything else is an error rather
than a silent pass. Python 3.11 standard library, and `ruby` (on the
runners) for the pin check's YAML.
"""
import argparse
import importlib.util
import json
import pathlib
import re
import subprocess
import sys

JOB = re.compile(r'\A {2}([A-Za-z0-9_-]+):\s*\Z')
FIELD = re.compile(r'\A {4}([a-z-]+):\s*(.*?)\s*\Z')
CONCURRENCY = re.compile(r'\A {6}(group|cancel-in-progress):\s*(.*?)\s*\Z')
TOP = re.compile(r'\A\S')
FILTER = re.compile(r'\A( +)(paths|paths-ignore):(.*)\Z')
USES = re.compile(r'\A\s+uses: \./\.github/actions/changed-paths/?\s*\Z')
PULL_REQUEST = re.compile(r'\A {2}pull_request:\s*\Z')
TRIGGER = re.compile(r'\A {2}\S')
ROW = re.compile(r'\A\| `([^`]+)` \| `([^`]+)` \|')
SECTION = '### Required status checks (exact names)'
LABELS = 'github.event.pull_request.labels'
# A job's concurrency group in a workflow that cancels a superseded pull
# request run job by job (docs/design/ci-checks.md, "Concurrency"). JOB_ID is
# the job's own id: `github.job` is not available where the group is read.
GROUP = ('${{ github.workflow }}-JOB_ID-'
         "${{ github.event_name == 'pull_request' && github.ref || github.run_id }}")
CANCEL = "${{ github.event_name == 'pull_request' }}"
# After JOB_ID in the group of a job with a `strategy:`: each leg of its
# matrix in a group of its own.
MATRIX = '-${{ strategy.job-index }}'
# The workflows that cancel job by job, so that removing every group from
# one fails rather than leaving nothing to check.
JOB_GROUPS = {'ci.yml'}
# A step's ref: a full lowercase commit sha, or a Docker image by digest.
PINNED = re.compile(r'\A[^@\s]+@[0-9a-f]{40}\Z|\Adocker://[^@\s]+@sha256:[0-9a-f]{64}\Z')
WORKFLOW_SUFFIXES = ('.yml', '.yaml')
# A workflow whose checks the table does not require, such as one this
# repository runs for itself alone.
UNLISTED = '# workflow-check: not required'
# The oracle FFmpeg's pin and its installer: a workflow that runs the
# installer starts when either changes.
ORACLE_PIN = ('scripts/oracle-ffmpeg.env', 'scripts/install-oracle-ffmpeg.sh')
# A script line that calls the installer, as a command or through a shell
# with options (`bash -e`, `bash -eo pipefail`, `bash --norc`); `bash -n`,
# which only reads the script, is no call.
SHELL_OPTION = r'(?:-[A-Za-mo-z]*o\s+\w+|-[A-Za-mo-z]+|--[a-z-]+)\s+'
INSTALLS = re.compile(r'''\A\s*['"]?(?:sudo\s+)?(?:(?:ba)?sh\s+(?:''' + SHELL_OPTION + r''')*)?(?:\./)?'''
                      + re.escape(ORACLE_PIN[1]) + r'(?:\s|\Z)')


def load_changed_paths(root):
    spec = importlib.util.spec_from_file_location(
        'changed_paths', root / '.github' / 'actions' / 'changed-paths' / 'changed_paths.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def jobs_of(lines):
    """{job id: {'name': str | None, 'needs': [str], 'always': bool, 'if': str,
    'concurrency': {'group': str, 'cancel-in-progress': str} | None, 'matrix': bool}}.

    `needs:` is read in its inline forms, `needs: a` and `needs: [a, b]`; a
    block sequence reads as no dependency at all, which the caller sees as
    a report job that waits for nothing.
    """
    jobs, current, reading_if, reading_concurrency = {}, None, False, False
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
            current = {'name': None, 'needs': [], 'always': False, 'if': '', 'concurrency': None, 'matrix': False}
            reading_if = reading_concurrency = False
            jobs[match.group(1)] = current
            continue
        if current is None:
            continue
        field = FIELD.match(line)
        if not field:
            if reading_if and line.startswith(' ' * 6):
                current['if'] += ' ' + line.strip()
            setting = CONCURRENCY.match(line)
            if reading_concurrency and setting:
                current['concurrency'][setting.group(1)] = setting.group(2)
            continue
        reading_if = reading_concurrency = False
        key, value = field.group(1), field.group(2)
        if key == 'name':
            current['name'] = value
        elif key == 'needs':
            current['needs'] = [n.strip() for n in value.strip('[]').split(',') if n.strip()]
        elif key == 'if':
            current['always'] = value == 'always()'
            current['if'] = value
            reading_if = value in ('>-', '>', '|', '|-')
        elif key == 'concurrency':
            current['concurrency'] = {'group': value} if value else {}
            reading_concurrency = not value
        elif key == 'strategy':
            current['matrix'] = True
    return jobs


def concurrency_problems(workflow, lines, jobs):
    """What is wrong with the concurrency groups of a workflow that cancels
    a superseded run job by job: one in JOB_GROUPS, or whose jobs carry
    `concurrency:`."""
    if workflow not in JOB_GROUPS and all(fields['concurrency'] is None for fields in jobs.values()):
        return []
    out = []
    if any(re.match(r'\Aconcurrency:', line) for line in lines):
        out.append(f'{workflow}: its jobs carry concurrency groups, so remove the workflow\'s own '
                   '`concurrency:`: it keeps the next run waiting until the report jobs of this one end')
    for job, fields in sorted(jobs.items()):
        if job.endswith('-report'):
            if fields['concurrency'] is not None:
                out.append(f'{workflow}: remove `{job}`\'s `concurrency:`: a report job has to run in a '
                           'cancelled run too, and in another job\'s group it can cancel that job')
            continue
        group = GROUP.replace('JOB_ID', job + (MATRIX if fields['matrix'] else ''))
        if fields['concurrency'] != {'group': group, 'cancel-in-progress': CANCEL}:
            out.append(f'{workflow}: give `{job}` exactly `concurrency: {{group: {group}, '
                       f'cancel-in-progress: {CANCEL}}}`, as a block (docs/design/ci-checks.md, "Concurrency")')
    return out


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


def run_lines(lines):
    """Each line of every step's `run:` script, a block scalar's to where its
    indentation ends."""
    for i, line in enumerate(lines):
        match = re.match(r'\A(\s*(?:-\s+)?)run:\s*(.*?)\s*\Z', line)
        if not match:
            continue
        if not match.group(2).startswith(('|', '>')):
            yield match.group(2)
            continue
        for item in lines[i + 1:]:
            if item.strip() and len(item) - len(item.lstrip()) <= len(match.group(1)):
                break
            yield item


def trigger_filters(lines):
    """[(event, key, [pattern] or None)]: each `paths` and `paths-ignore`
    list of the `on:` block, None where it is not a flow sequence on its
    own line or a block sequence of plain items."""
    out, inside, event = [], False, None
    for i, line in enumerate(lines):
        if line.rstrip() == 'on:':
            inside = True
            continue
        if not inside:
            continue
        if TOP.match(line):
            break
        if TRIGGER.match(line):
            event = line.strip().rstrip(':')
            continue
        match = re.match(r'\A {4}(paths|paths-ignore):\s*(.*?)\s*\Z', line)
        if not match:
            continue
        value, patterns = match.group(2), []
        if value.startswith('[') and value.endswith(']'):
            patterns = [p.strip().strip('\'"') for p in value[1:-1].split(',') if p.strip()]
        elif not value:
            for item in lines[i + 1:]:
                if item.strip().startswith('#'):
                    continue
                entry = re.match(r'\A {4,}- (.+?)\s*\Z', item)
                if not entry:
                    break
                patterns.append(entry.group(1).strip('\'"'))
        out.append((event, match.group(1), patterns or None))
    return out


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
    return {workflow.name for workflow in workflow_files(root)
            if UNLISTED in (line.strip() for line in workflow.read_text(encoding='utf-8').splitlines())}


def workflow_files(root):
    """Every workflow GitHub runs: .yml and .yaml files alike."""
    return sorted(path for path in (root / '.github' / 'workflows').iterdir()
                  if path.is_file() and path.suffix in WORKFLOW_SUFFIXES)


# Ruby's standard YAML library (Psych) parses each file for the pin check,
# and the check walks the node tree it builds rather than loading it into
# objects: a key is compared as the string it is written as, so YAML 1.1's
# booleans (`on`, `yes`), merge keys and duplicate keys cannot hide a step.
# Every `uses` key is read wherever it appears, anchored subtrees included;
# an alias is resolved through the anchors defined before it.
PARSE = r'''
require "json"
require "psych"
begin
  stream = Psych.parse_stream(STDIN.read)
rescue Psych::SyntaxError => e
  abort e.message
end
documents = stream.children.size
abort "it holds #{documents} YAML documents; a workflow or action file holds one" if documents > 1
anchors, out = {}, []
resolve = lambda do |node|
  next node unless node.is_a?(Psych::Nodes::Alias)
  anchors[node.anchor] or out << { line: node.start_line + 1, error: "the alias *#{node.anchor} names no anchor" }
  anchors[node.anchor]
end
walk = lambda do |node|
  anchors[node.anchor] = node if !node.is_a?(Psych::Nodes::Alias) && node.respond_to?(:anchor) && node.anchor
  resolve.(node) if node.is_a?(Psych::Nodes::Alias)
  if node.is_a?(Psych::Nodes::Mapping)
    node.children.each_slice(2) do |key, value|
      walk.(key)
      name = key.is_a?(Psych::Nodes::Alias) ? anchors[key.anchor] : key
      if name.is_a?(Psych::Nodes::Scalar) && name.value == "uses"
        ref = value.is_a?(Psych::Nodes::Alias) ? anchors[value.anchor] : value
        out << if ref.is_a?(Psych::Nodes::Scalar)
                 { line: value.start_line + 1, ref: ref.value }
               else
                 { line: value.start_line + 1, error: "a uses value that is not a string" }
               end
      end
      walk.(value)
    end
  elsif node.respond_to?(:children) && node.children
    node.children.each { |child| walk.(child) }
  end
end
walk.(stream)
puts JSON.generate(out)
'''


def uses_entries(text):
    """[{'line': n, 'ref': str} or {'line': n, 'error': str}] for the `uses`
    keys of a file, or a str saying why it cannot be read."""
    try:
        run = subprocess.run(['ruby', '--disable-gems', '-e', PARSE], input=text, capture_output=True, text=True,
                             timeout=60)
    except FileNotFoundError:
        return 'ruby is not installed; the pin check parses YAML with its standard library (Psych)'
    if run.returncode != 0:
        return 'not readable as YAML: ' + (run.stderr.strip().splitlines() or ['ruby failed'])[-1]
    return json.loads(run.stdout)


def pin_problems(root, path, seen):
    """What is wrong with the refs of one workflow or action file, and of the
    local actions and workflows it calls (`./...`), each read once."""
    if path in seen:
        return []
    seen.add(path)
    name = path.relative_to(root)
    entries = uses_entries(path.read_text(encoding='utf-8'))
    if isinstance(entries, str):
        return [f'{name}: {entries}']
    out = []
    for entry in entries:
        where, ref = f'{name}: line {entry["line"]}: ', entry.get('ref')
        if ref is None:
            out.append(where + entry['error'])
        elif ref.startswith('./'):
            target = (root / ref).resolve()
            files = [target] if target.suffix in WORKFLOW_SUFFIXES else [target / 'action.yml', target / 'action.yaml']
            found = [f for f in files if f.is_file()]
            if not target.is_relative_to(root.resolve()) or not found:
                out.append(f'{where}{ref} names no action or workflow file in this repository')
            for f in found:
                out += pin_problems(root, root / f.relative_to(root.resolve()), seen)
        elif not PINNED.match(ref):
            out.append(f'{where}`{ref}` is not pinned: use the full lowercase commit sha with the '
                       'version as a comment (`owner/repo@<40-hex sha> # vX.Y.Z`), as every other step does')
    return out


def check(root):
    problems = []
    unlisted = unlisted_workflows(root)
    changed_paths = load_changed_paths(root)
    workflows = workflow_files(root)
    seen = set()
    jobs = {}        # workflow file name -> job id -> fields
    names = {}       # check name -> [(workflow, job id)]
    for path in workflows:
        lines = path.read_text(encoding='utf-8').splitlines()
        jobs[path.name] = found = jobs_of(lines)
        if not found:
            problems.append(f'{path.name}: no job found; the parser expects two-space job ids')
        for job, fields in found.items():
            names.setdefault(fields['name'] or job, []).append((path.name, job))
        problems += pin_problems(root, path, seen)
        problems += concurrency_problems(path.name, lines, found)
        patterns, bad = filter_patterns(lines)
        problems.extend(f'{path.name}: {problem}' for problem in bad)
        for key, pattern in patterns:
            try:
                changed_paths.to_regex(pattern)
            except SystemExit as error:
                problems.append(f'{path.name}: {key}: {error}')
        if any(INSTALLS.match(line) for line in run_lines(lines)):
            for event, key, listed in trigger_filters(lines):
                if listed is None:
                    problems.append(f'{path.name}: the `{key}` filter of `{event}` is not a list this check reads')
                    continue
                try:
                    regexes = [changed_paths.to_regex(pattern) for pattern in listed]
                except SystemExit as error:
                    problems.append(f'{path.name}: {key} of `{event}`: {error}')
                    continue
                for pin in ORACLE_PIN:
                    if any(r.match(pin) for r in regexes) == (key == 'paths-ignore'):
                        fix = 'take it out of' if key == 'paths-ignore' else 'add it to'
                        problems.append(f'{path.name}: runs the oracle FFmpeg installer, but a change of {pin} '
                                        f'does not start it: {fix} the `{key}` of `{event}`')

    for action in sorted((root / '.github' / 'actions').glob('*/action.y*ml')):
        problems += pin_problems(root, action, seen)

    for name, where in sorted(names.items()):
        if len(where) > 1:
            places = ', '.join(f'{w}:{j}' for w, j in where)
            problems.append(f'check name `{name}` is reported by more than one job: {places}')

    note = root / 'docs' / 'design' / 'ci-checks.md'
    listed = required_names(note)
    reports = {}     # workflow file name -> its required report jobs
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
        reports.setdefault(workflow, set()).add(job)

    for workflow, covering in sorted(reports.items()):
        found = jobs[workflow]
        covered = set().union(*(found[job]['needs'] for job in covering))
        missing = sorted(set(found) - covering - label_gated(found) - covered)
        if missing:
            problems.append(f'{workflow}: no report job waits for ' + ', '.join(missing))

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
