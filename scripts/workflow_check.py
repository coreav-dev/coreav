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
- a step in a workflow (.yml or .yaml), in a local action
  (.github/actions/*/action.yml or .yaml) or in a local action or workflow
  a step calls (`./...`) uses an action by a tag or branch instead of a
  full lowercase commit sha, so the code it runs can change under the same
  name; or such a file is written in a form the pin reader does not
  decode (see `action_refs`).

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
# A step's ref: a full lowercase commit sha, or a Docker image by digest.
PINNED = re.compile(r'\A[^@\s]+@[0-9a-f]{40}\Z|\Adocker://[^@\s]+@sha256:[0-9a-f]{64}\Z')
# Where a plain mapping key ends (its colon), or a comment starts first.
PLAIN_KEY = re.compile(r':(?=[ \t]|$)|[ \t]#')
SEQUENCE_ITEM = re.compile(r'-(?=[ \t]|$)[ \t]*')
BLOCK_SCALAR = re.compile(r'(?:[&!]\S*[ \t]+)*[|>][1-9+-]{0,2}(?:[ \t]+#.*)?\Z')
WORKFLOW_SUFFIXES = ('.yml', '.yaml')
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
    return {workflow.name for workflow in workflow_files(root)
            if UNLISTED in (line.strip() for line in workflow.read_text(encoding='utf-8').splitlines())}


def workflow_files(root):
    """Every workflow GitHub runs: .yml and .yaml files alike."""
    return sorted(path for path in (root / '.github' / 'workflows').iterdir()
                  if path.is_file() and path.suffix in WORKFLOW_SUFFIXES)


def quoted_end(text):
    """The index just past the quoted scalar that opens `text`, or -1 when
    it does not close on this line."""
    quote, i = text[0], 1
    while i < len(text):
        if quote == '"' and text[i] == '\\':
            i += 2
        elif text[i] == quote and quote == "'" and text[i + 1:i + 2] == "'":
            i += 2
        elif text[i] == quote:
            return i + 1
        else:
            i += 1
    return -1


def mapping_key(text):
    """(key, value) when `text` opens a block mapping entry, else (None, text).
    The key is False when it is written in a form this reader does not
    decode: an escape, an anchor, an alias, a tag or a merge key."""
    if text[:1] in ('"', "'"):
        end = quoted_end(text)
        rest = text[end:].lstrip(' \t') if end > 0 else ''
        if not rest.startswith(':') or rest[1:2] not in ('', ' ', '\t'):
            return None, text
        raw = text[1:end - 1]
        if text[0] == "'":
            return raw.replace("''", "'"), rest[1:]
        return (False if '\\' in raw else raw), rest[1:]
    found = PLAIN_KEY.search(text)
    if text[:1] in ('{', '[') or not found or found.group() != ':':
        return None, text
    key = text[:found.start()].rstrip()
    if not key or key[0] in '&*!' or key == '<<':
        return False, text[found.end():]
    return key, text[found.end():]


def flow_mapping(text):
    """[(key, value)] of a one-line flow mapping (`{ a: 1, b: 2 }`), or None when it
    nests, spans lines, or holds an entry this reader does not decode."""
    if not text.endswith('}'):
        return None
    body, entries, entry, i = text[1:-1], [], '', 0
    while i < len(body):
        if body[i] in ('"', "'"):
            end = quoted_end(body[i:])
            if end < 0:
                return None
            entry, i = entry + body[i:i + end], i + end
        elif body[i] in '{}[]':
            return None
        elif body[i] == ',':
            entries, entry, i = entries + [entry], '', i + 1
        else:
            entry, i = entry + body[i], i + 1
    out = []
    for entry in entries + [entry]:
        if not entry.strip():
            continue
        key, value = mapping_key(entry.strip())
        if not key:
            return None
        out.append((key, value.strip()))
    return out


def scalar(text):
    """The block-context YAML scalar at the start of `text`, or None when it
    is quoted in a form this reader does not decode (an escape, or no
    closing quote)."""
    quote = text[:1]
    if quote in ('"', "'"):
        end = text.find(quote, 1)
        if end < 0 or (quote == '"' and '\\' in text[1:end]) or (quote == "'" and text[end + 1:end + 2] == "'"):
            return None
        return text[1:end]
    return re.split(r'\s#', text, maxsplit=1)[0].strip()


def action_refs(lines):
    """([(line number, ref)], problems) for the `uses` keys of a workflow or an
    action file. The file is read as YAML block mappings and sequences:
    comments, block scalars (`run: |`) and quoted scalars that span lines
    are skipped, so text in a script is not a step; a form this reader does
    not decode (a flow mapping, an alias, an explicit `?` key, an escaped or
    tagged key, a `uses` value that continues on the next line) is a problem,
    never a silent pass. A ref is None when its value cannot be read."""
    refs, problems = [], []
    deeper = None       # lines indented more than this belong to a block scalar
    closing = False     # inside a quoted scalar that spans lines
    pending = None      # (line number, column) of a uses whose plain value may continue
    for n, line in enumerate(lines, 1):
        stripped = line.lstrip(' ')
        indent = len(line) - len(stripped)
        if closing:
            closing = not re.search(r'(?<!\\)"|(?<!\')\'(?!\')', line)
            continue
        if deeper is not None:
            if not stripped.strip() or indent > deeper:
                continue
            deeper = None
        if not stripped.strip() or stripped.startswith('#') or stripped.rstrip() in ('---', '...'):
            continue
        if pending:
            if indent > pending[1]:
                problems.append(f'line {pending[0]}: the `uses` value continues on the next line: '
                                'write it on one line')
            pending = None
        column, text, dash = indent, stripped.rstrip(), None
        while (item := SEQUENCE_ITEM.match(text)):
            dash, column, text = column, column + item.end(), text[item.end():]
        if not text:
            continue
        if re.match(r'[?:](?:[ \t]|$)', text):
            problems.append(f'line {n}: an explicit `?` key or a bare `:` value: write the entry as `key: value`')
            continue
        key, value = mapping_key(text)
        if key is False:
            problems.append(f'line {n}: a key written with an escape, an anchor, an alias, a tag or `<<`: '
                            'write it plainly')
            continue
        value = value.lstrip(' \t') if key is not None else text
        if key == 'uses':
            refs.append((n, scalar(value)))
            if value[:1] not in ('"', "'"):
                pending = (n, column)
        flow = re.split(r'\s#', value, maxsplit=1)[0].rstrip()
        if flow in ('{}', '[]'):
            continue
        entries = flow_mapping(flow) if flow.startswith('{') else []
        refs += [(n, scalar(v)) for k, v in entries or [] if k == 'uses']
        if entries is None or (flow.startswith('[') and (
                '{' in flow or re.search(r':(?=[\s,\]])', flow) or not flow.endswith(']'))):
            problems.append(f'line {n}: a flow collection that nests, spans lines or holds a form this reader '
                            'does not decode: write it in block form, or as a one-line `{ key: value }`')
        elif value.startswith('*'):
            problems.append(f'line {n}: an alias: write the content out')
        elif value[:1] in ('"', "'") and quoted_end(value) < 0:
            closing = True
        elif BLOCK_SCALAR.match(value):
            deeper = column if key is not None else dash
    return refs, problems


def pin_problems(root, path, seen):
    """What is wrong with the refs of one workflow or action file, and of the
    local actions and workflows it calls (`./...`), each read once."""
    if path in seen:
        return []
    seen.add(path)
    name = path.relative_to(root)
    refs, problems = action_refs(path.read_text(encoding='utf-8').splitlines())
    out = [f'{name}: {problem}' for problem in problems]
    for n, ref in refs:
        if ref is not None and ref.startswith('./'):
            target = (root / ref).resolve()
            files = [target] if target.suffix in WORKFLOW_SUFFIXES else [target / 'action.yml', target / 'action.yaml']
            found = [f for f in files if f.is_file()]
            if not target.is_relative_to(root.resolve()) or not found:
                out.append(f'{name}: line {n}: {ref} names no action or workflow file in this repository')
            for f in found:
                out += pin_problems(root, root / f.relative_to(root.resolve()), seen)
        elif ref is None or not PINNED.match(ref):
            out.append(f'{name}: line {n}: `{ref}` is not pinned: use the full lowercase commit sha with the '
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
        patterns, bad = filter_patterns(lines)
        problems.extend(f'{path.name}: {problem}' for problem in bad)
        for key, pattern in patterns:
            try:
                changed_paths.to_regex(pattern)
            except SystemExit as error:
                problems.append(f'{path.name}: {key}: {error}')

    for action in sorted((root / '.github' / 'actions').glob('*/action.y*ml')):
        problems += pin_problems(root, action, seen)

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
