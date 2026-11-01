#!/usr/bin/env python3
"""Reuse a recent successful merge-group workflow for the same Git tree on main."""
import argparse
import datetime
import json
import os
import pathlib
import re
import subprocess
import urllib.error
import urllib.parse
import urllib.request


def find_run(request, repo, workflow, tree, now):
    since = (now - datetime.timedelta(days=1)).isoformat()
    query = urllib.parse.urlencode({'event': 'merge_group', 'per_page': 20, 'created': '>=' + since})
    runs = request(f'/repos/{repo}/actions/workflows/{workflow}/runs?{query}')['workflow_runs']
    for run in runs[:20]:
        if (run.get('event') != 'merge_group'
                or not (run.get('head_branch') or '').startswith('gh-readonly-queue/main/')
                or run.get('path') != f'.github/workflows/{workflow}'
                or (run.get('head_repository') or {}).get('full_name') != repo):
            continue
        created = datetime.datetime.fromisoformat(run['created_at'].replace('Z', '+00:00'))
        if not now - datetime.timedelta(days=1) <= created <= now:
            continue
        sha = run['head_sha']
        if not re.fullmatch('[0-9a-f]{40}', sha):
            return None
        commit = run.get('head_commit') or {}
        if commit.get('id') != sha or not re.fullmatch('[0-9a-f]{40}', commit.get('tree_id', '')):
            return None
        if commit['tree_id'] == tree:
            return int(run['id']) if run.get('status') == 'completed' and run.get('conclusion') == 'success' else None
    return None


def result(env, request, tree, now, workflow):
    if env.get('GITHUB_EVENT_NAME') != 'push' or env.get('GITHUB_REF') != 'refs/heads/main':
        return None, 'Not a push to main; run the normal checks.'
    try:
        run = find_run(request, env['GITHUB_REPOSITORY'], workflow, tree, now)
    except (OSError, ValueError, KeyError, TypeError):
        return None, 'Merge-group evidence unavailable; run the normal checks.'
    if run is None:
        return None, 'No recent successful merge-group workflow has this tree; run the normal checks.'
    url = f"https://github.com/{env['GITHUB_REPOSITORY']}/actions/runs/{run}"
    return run, f'Reusing successful merge_group run {url} for tree {tree}; skip repeated checks.'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workflow', choices=('ci.yml', 'deny.yml', 'python.yml', 'npm.yml'), required=True)
    args = parser.parse_args()
    env = os.environ

    def request(path):
        req = urllib.request.Request(env.get('GITHUB_API_URL', 'https://api.github.com') + path,
                                     headers={'Authorization': 'Bearer ' + env['GH_TOKEN'],
                                              'Accept': 'application/vnd.github+json'})
        with urllib.request.urlopen(req, timeout=10) as response:
            return json.load(response)

    tree = subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], text=True).strip()
    run, message = result(env, request, tree, datetime.datetime.now(datetime.timezone.utc), args.workflow)
    print(message)
    with pathlib.Path(env['GITHUB_OUTPUT']).open('a', encoding='utf-8') as output:
        output.write(f'reuse={"true" if run is not None else "false"}\n')
    if env.get('GITHUB_STEP_SUMMARY'):
        with pathlib.Path(env['GITHUB_STEP_SUMMARY']).open('a', encoding='utf-8') as summary:
            summary.write(message + '\n')


if __name__ == '__main__':
    main()
