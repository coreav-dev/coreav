#!/usr/bin/env python3
r"""Decides whether the jobs behind a path filter have to run for this event
(docs/design/ci-checks.md).

A workflow whose trigger carries `paths` or `paths-ignore` does not start at
all when the filter excludes the event, so no check of it is reported and a
branch ruleset cannot require one. Moving the filter from the trigger to the
jobs keeps the saving and lets one job always run and report. This is the
filter those jobs are gated on.

    PATHS='crates/**' changed_paths.py
    PATHS_IGNORE='docs/design/**' changed_paths.py

Exactly one of the environment variables PATHS and PATHS_IGNORE holds the
filter, one pattern per line (`#` comments and blank lines are dropped), in
GitHub's filter-pattern syntax (`*` matches
anything but a slash, `**` anything at all; `!`, `+`, `?`, `\` and
character classes are refused, because they do not mean in that syntax what
a glob reader expects, so that no pattern is read differently here than by
GitHub). The semantics are GitHub's:

- with PATHS, run when any changed file matches any pattern;
- with PATHS_IGNORE, run unless every changed file matches some pattern.

The changed files come from git, which needs the two commits present
(`actions/checkout` with `fetch-depth: 2`):

- a pull request: the merge commit against its first parent, the base;
- a push: the event's `before` commit against the event's commit, read from
  the BEFORE environment variable;
- a merge group: the group's base commit, read from the GROUP_BASE
  environment variable, against the group's commit. The base is the tip of
  the branch or the head of the group queued ahead, often more than one
  parent away, so it is fetched from `origin` first;
- anything else (a manual or scheduled run), or a commit git cannot reach,
  or a diff that fails: everything runs, which is the safe direction.

Writes `run=true` or `run=false` to $GITHUB_OUTPUT and the reason to stdout.
Exit status 0 unless the filter itself is unusable. Python 3.11 standard
library only.
"""
import os
import pathlib
import re
import subprocess
import sys

ZERO = re.compile(r'\A0{40,}\Z')
# GitHub's filter patterns are not ordinary globs: `?` and `+` are
# quantifiers over the preceding character, not wildcards, `\` escapes, and
# `[]` is a character class. Only `*` and `**` are translated here; the rest
# are refused, so that no pattern can mean one thing to GitHub and another
# here. None is in use.
UNSUPPORTED = ('!', '+', '?', '[', ']', '\\')


def to_regex(pattern):
    """GitHub's filter-pattern syntax as a full-path regular expression."""
    for char in UNSUPPORTED:
        if char in pattern:
            raise SystemExit(f'changed-paths: unsupported pattern {pattern!r} ({char!r})')
    out, i = [], 0
    while i < len(pattern):
        char = pattern[i]
        if char == '*':
            if pattern[i + 1:i + 2] == '*':
                out.append('.*')
                i += 2
            else:
                out.append('[^/]*')
                i += 1
        else:
            out.append(re.escape(char))
            i += 1
    return re.compile(r'\A' + ''.join(out) + r'\Z')


def patterns_of(text):
    """The patterns of one filter: one per line, `#` comments and blank
    lines dropped. A pattern never starts with `#`, and the lists are
    easier to keep next to the trigger's own comments with them."""
    lines = (line.strip() for line in text.splitlines())
    return [line for line in lines if line and not line.startswith('#')]


def decide(changed, paths, paths_ignore):
    """(run, reason) for the changed files and one of the two filters.

    `changed` is None when the changed files are unknown, which runs
    everything.
    """
    if (not paths) == (not paths_ignore):
        raise SystemExit('changed-paths: give exactly one of paths and paths-ignore')
    if changed is None:
        return True, 'the changed files are unknown'
    if not changed:
        return True, 'the event changes no file'
    if paths:
        matchers = [to_regex(p) for p in paths]
        hit = [name for name in changed if any(m.match(name) for m in matchers)]
        if hit:
            return True, f'{len(hit)} of {len(changed)} changed files match, first {hit[0]}'
        return False, f'none of the {len(changed)} changed files matches'
    matchers = [to_regex(p) for p in paths_ignore]
    kept = [name for name in changed if not any(m.match(name) for m in matchers)]
    if kept:
        return True, f'{len(kept)} of {len(changed)} changed files are outside the filter, first {kept[0]}'
    return False, f'all {len(changed)} changed files are inside the filter'


def git(*args):
    return subprocess.run(('git',) + args, capture_output=True, text=True, check=True).stdout


def changed_files(event, before, group_base):
    """The event's changed files, or None when they cannot be determined."""
    if event == 'pull_request':
        base, head = 'HEAD^1', 'HEAD'
    elif event == 'push':
        if not before or ZERO.match(before):
            return None
        base, head = before, 'HEAD'
    elif event == 'merge_group':
        if not group_base:
            return None
        base, head = group_base, 'HEAD'
    else:
        return None
    try:
        if event == 'merge_group':
            git('fetch', '--quiet', '--no-tags', '--depth=1', 'origin', base)
        git('cat-file', '-e', base + '^{commit}')
        out = git('-c', 'core.quotePath=false', 'diff', '--name-only', '--no-renames', '-z', base, head)
    except (subprocess.CalledProcessError, OSError):
        return None
    return [name for name in out.split('\0') if name]


def main():
    event = os.environ.get('GITHUB_EVENT_NAME', '')
    changed = changed_files(event, os.environ.get('BEFORE', ''), os.environ.get('GROUP_BASE', ''))
    run, reason = decide(
        changed,
        patterns_of(os.environ.get('PATHS', '')),
        patterns_of(os.environ.get('PATHS_IGNORE', '')),
    )
    print(f'{event}: {"run" if run else "skip"}, {reason}')
    if changed is not None:
        for name in changed:
            print(f'  {name}')
    output = os.environ.get('GITHUB_OUTPUT')
    if output:
        with pathlib.Path(output).open('a', encoding='utf-8') as handle:
            handle.write(f'run={"true" if run else "false"}\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
