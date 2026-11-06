#!/usr/bin/env python3
"""Reuse successful checks only for the same tested tree and event boundary."""
import argparse
import datetime
import json
import os
import pathlib
import re
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request

SHA = re.compile('[0-9a-f]{40}')


def eligible_pull(pull, repo):
    return (pull['state'] == 'open' and pull['draft'] is False
            and pull['base']['ref'] == 'main'
            and pull['base']['repo']['full_name'] == repo
            and pull['head']['repo']['full_name'] == repo
            and pull['user']['login'] != 'dependabot[bot]'
            and SHA.fullmatch(pull['head']['sha']) is not None)


def receipt_name(tree, base, attempt):
    if not SHA.fullmatch(tree) or not SHA.fullmatch(base) or type(attempt) is not int or attempt < 1:
        raise ValueError('invalid_receipt_identity')
    return f'tested-tree-v1-{tree}-{base}-{attempt}'


def pr_receipt(env, event, tree, commit, parents):
    if env.get('GITHUB_EVENT_NAME') != 'pull_request':
        return None
    try:
        pull = event['pull_request']
        number = event['number']
        attempt = int(env['GITHUB_RUN_ATTEMPT'])
        if (type(number) is not int or number < 1
                or env.get('GITHUB_REF') != f'refs/pull/{number}/merge'
                or not eligible_pull(pull, env['GITHUB_REPOSITORY'])
                or 'dependabot[bot]' in (env.get('GITHUB_ACTOR'), env.get('GITHUB_TRIGGERING_ACTOR'))
                or commit != env.get('GITHUB_SHA') or not SHA.fullmatch(commit)
                or len(parents) != 2 or parents != [pull['base']['sha'], pull['head']['sha']]):
            return None
        return receipt_name(tree, parents[0], attempt)
    except (KeyError, TypeError, ValueError):
        return None


def run_index(request, path):
    data = request(path)
    runs = data['workflow_runs']
    if (not isinstance(runs, list) or type(data['total_count']) is not int
            or data['total_count'] != len(runs) or len(runs) > 20):
        raise ValueError('incomplete_run_index')
    for run in runs:
        if (type(run['id']) is not int or run['id'] < 1
                or type(run['run_attempt']) is not int or run['run_attempt'] < 1):
            raise ValueError('invalid_run_identity')
        datetime.datetime.fromisoformat(run['created_at'].replace('Z', '+00:00'))
    return runs


def run_order(run):
    return datetime.datetime.fromisoformat(run['created_at'].replace('Z', '+00:00')), run['id']


def find_pr_run(request, env, event, tree, parents, now, workflow):
    ref = re.fullmatch(r'refs/heads/gh-readonly-queue/main/pr-([1-9][0-9]*)-[0-9a-f]{40}',
                       env.get('GITHUB_REF', ''))
    if ref is None or not parents or parents[0] != event['merge_group']['base_sha']:
        return None
    group = event['merge_group']
    if group['base_ref'] != 'refs/heads/main' or group['head_sha'] != env['GITHUB_SHA']:
        return None
    repo = env['GITHUB_REPOSITORY']
    root = f'/repos/{repo}'
    current = request(f'{root}/actions/runs/{env["GITHUB_RUN_ID"]}')
    if (current['event'] != 'merge_group' or str(current['id']) != env['GITHUB_RUN_ID']
            or current['head_sha'] != env['GITHUB_SHA']
            or current['head_commit']['id'] != env['GITHUB_SHA']
            or current['head_commit']['tree_id'] != tree
            or current['head_branch'] != env['GITHUB_REF'].removeprefix('refs/heads/')
            or current['path'] != f'.github/workflows/{workflow}'
            or current['head_repository']['full_name'] != repo):
        return None
    created = datetime.datetime.fromisoformat(current['created_at'].replace('Z', '+00:00'))
    pull_path = f'{root}/pulls/{ref[1]}'
    pull = request(pull_path)
    if not eligible_pull(pull, repo):
        return None
    head = pull['head']['sha']
    query = urllib.parse.urlencode({'event': 'pull_request', 'head_sha': head, 'per_page': 20})
    path = f'{root}/actions/workflows/{workflow}/runs?{query}'
    runs = run_index(request, path)
    if not runs:
        return None
    latest = max(runs, key=run_order)
    stamp = datetime.datetime.fromisoformat(latest['created_at'].replace('Z', '+00:00'))
    if (latest['event'] != 'pull_request' or latest['head_sha'] != head
            or latest['head_branch'] != pull['head']['ref']
            or latest['path'] != f'.github/workflows/{workflow}'
            or latest['repository']['full_name'] != repo
            or latest['head_repository']['full_name'] != repo
            or latest['status'] != 'completed' or latest['conclusion'] != 'success'
            or not now - datetime.timedelta(days=1) <= stamp <= created <= now):
        return None
    name = receipt_name(tree, parents[0], latest['run_attempt'])
    query = urllib.parse.urlencode({'name': name, 'per_page': 100})
    data = request(f'{root}/actions/runs/{latest["id"]}/artifacts?{query}')
    artifacts = data['artifacts']
    if (not isinstance(artifacts, list) or type(data['total_count']) is not int
            or data['total_count'] != len(artifacts) or len(artifacts) != 1):
        return None
    artifact, = artifacts
    if (artifact['name'] != name or artifact['expired'] is not False
            or type(artifact['id']) is not int or artifact['id'] < 1
            or artifact['workflow_run']['id'] != latest['id']
            or artifact['workflow_run']['head_sha'] != head
            or artifact['workflow_run']['head_branch'] != pull['head']['ref']
            or artifact['workflow_run']['head_repository_id'] != pull['head']['repo']['id']
            or artifact['workflow_run']['repository_id'] != pull['base']['repo']['id']):
        return None
    fresh = run_index(request, path)
    if not fresh or max(fresh, key=run_order) != latest:
        return None
    fresh_pull = request(pull_path)
    if not eligible_pull(fresh_pull, repo) or fresh_pull['head']['sha'] != head:
        return None
    return latest['id']


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


def result(env, request, tree, now, workflow, event=None, parents=()):
    if env.get('GITHUB_EVENT_NAME') == 'merge_group':
        try:
            run = find_pr_run(request, env, event, tree, parents, now, workflow)
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            return None, 'PR evidence unavailable; run the normal checks.'
        if run is None:
            return None, 'No successful PR run proves this tested tree and base; run the normal checks.'
        url = f"https://github.com/{env['GITHUB_REPOSITORY']}/actions/runs/{run}"
        return run, f'Reusing successful pull_request run {url} for tested tree {tree} and base {parents[0]}.'
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
    deadline = time.monotonic() + 60
    calls = 0

    def request(path):
        nonlocal calls
        calls += 1
        if calls > 20 or time.monotonic() >= deadline:
            raise ValueError('api_budget_exhausted')
        req = urllib.request.Request(env.get('GITHUB_API_URL', 'https://api.github.com') + path,
                                     headers={'Authorization': 'Bearer ' + env['GH_TOKEN'],
                                              'Accept': 'application/vnd.github+json',
                                              'Cache-Control': 'no-cache'})
        with urllib.request.urlopen(req, timeout=min(10, max(1, deadline - time.monotonic()))) as response:
            raw = response.read((2 << 20) + 1)
        if len(raw) > 2 << 20:
            raise ValueError('api_response_too_large')
        return json.loads(raw)

    tree = subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], text=True).strip()
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    parents = subprocess.check_output(['git', 'rev-list', '--parents', '-n', '1', 'HEAD'], text=True).split()[1:]
    event = {}
    try:
        if env.get('GITHUB_EVENT_PATH'):
            with pathlib.Path(env['GITHUB_EVENT_PATH']).open('rb') as source:
                raw = source.read((2 << 20) + 1)
            if len(raw) <= 2 << 20:
                event = json.loads(raw)
    except (OSError, ValueError):
        pass
    run, message = result(env, request, tree, datetime.datetime.now(datetime.timezone.utc), args.workflow,
                          event, parents)
    print(message)
    with pathlib.Path(env['GITHUB_OUTPUT']).open('a', encoding='utf-8') as output:
        output.write(f'reuse={"true" if run is not None else "false"}\n')
        receipt = pr_receipt(env, event, tree, commit, parents)
        if receipt is not None:
            try:
                path = pathlib.Path(env['RUNNER_TEMP']) / 'tested-tree.txt'
                path.write_text(f'tree={tree}\nbase={parents[0]}\ncommit={commit}\n')
                output.write(f'record_name={receipt}\nrecord_path={path}\n')
            except (OSError, KeyError):
                print('Tested-tree receipt unavailable; merge groups will run the normal checks.')
    if env.get('GITHUB_STEP_SUMMARY'):
        with pathlib.Path(env['GITHUB_STEP_SUMMARY']).open('a', encoding='utf-8') as summary:
            summary.write(message + '\n')


if __name__ == '__main__':
    main()
