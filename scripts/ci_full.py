#!/usr/bin/env python3
"""Report main full-suite failures and verify exact-head manual evidence."""
import argparse
import collections
import datetime
import json
import os
from pathlib import Path
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

REPOSITORY = 'coreav-dev/coreav'
BOT = 'github-actions[bot]'
AREAS = {
    'ci': ('CI infrastructure', 'area:ci'),
    'h264': ('H.264 decoding', 'area:h264'),
    'transcode': ('Transcode compatibility', 'area:transcode'),
    'vp9': ('VP9 decoding', 'area:ci'),
    'hevc': ('HEVC decoding', 'area:ci'),
}
FULL_WORKFLOWS = {
    'ci-full': {
        'file': 'ci-full.yml',
        'final_head': 'always',
        'areas': {
            'ci': ('full-fixtures',),
            'h264': ('h264-full', 'conformance-arm64'),
            'transcode': tuple(f'transcode-compat-shard ({i})' for i in (1, 2, 3)) + ('transcode-compat',),
        },
    },
    'vp9-corpus': {
        'file': 'vp9-corpus.yml',
        'final_head': 'always',
        'areas': {'vp9': ('contract',) + tuple(f'corpus ({i})' for i in range(8)) + ('corpus-summary',)},
    },
    'vp9-conformance': {
        'file': 'vp9-conformance.yml',
        'final_head': 'always',
        'areas': {'vp9': tuple(f'{job} ({host})' for job in ('vectors', 'interpolation-fuzz')
                              for host in ('ubuntu-24.04', 'macos-15')) + ('adapter-contract',)},
    },
    'hevc-conformance': {
        'file': 'hevc-conformance.yml',
        'final_head': 'area',
        'areas': {'hevc': ('hevc-vectors', 'profile-census (0)', 'profile-census (1)',
                           'profile-census (2)', 'profile-census (3)')},
    },
}
PROOF_STEPS = {
    ('ci-full', 'full-fixtures'): ('Publish the full fixture sets',),
    ('ci-full', 'h264-full'): ('Decode every in-scope stream', 'Decode every in-scope stream in safe mode',
                              'Lint and test safe mode'),
    ('ci-full', 'conformance-arm64'): ('Decode every in-scope stream',),
    ('ci-full', 'transcode-compat-shard'): ('Prove composed output identity and semantic mutations',
                                         'Run the registered composed compatibility check'),
    ('ci-full', 'transcode-compat'): ('Judge the registered compatibility check over every shard',
                                    'Check the registered version and pinned oracle'),
    ('vp9-corpus', 'contract'): ('Check the comparison contract before corpus acquisition',
                                'Preflight the original-file oracle on a pinned public vector',
                                'Compare the routine original-file smoke subset'),
    ('vp9-corpus', 'corpus'): ('Compare exact clocks and pixels without credentials',),
    ('vp9-corpus', 'corpus-summary'): ('Require every registered shard and comparison',),
    ('vp9-conformance', 'vectors'): ('Read vector headers and compare decoded frames',
                                     'Compare native boundary frames and keyframe-only selection',
                                     'Compare scalar-only vector frames without changing comparators'),
    ('vp9-conformance', 'interpolation-fuzz'): ('Compare automatic and scalar interpolation for C3',),
    ('vp9-conformance', 'adapter-contract'): ('Check the scratch adapter without building timing artifacts',),
    ('hevc-conformance', 'hevc-vectors'): (
        'Verify registered synthetic references before native classification',
        'Acquire every registered official archive',
        'Bind archive and extracted-data hashes',
        'Check complete headers and every selected intra reference',
        'Check complete scalar conformance output',
        'Check complete conformance output and random access through the decode boundary'),
    ('hevc-conformance', 'profile-census'): (
        'Inspect all fixed files with isolated metadata subprocesses',),
}


class Failure(Exception):
    pass


def require(condition, reason):
    if not condition:
        raise Failure(reason)


def positive(value):
    return type(value) is int and value > 0


def object_unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate_api_field')
        result[key] = value
    return result


class Api:
    def __init__(self, token):
        self.token = token
        self.requests = 0
        self.deadline = time.monotonic() + 180

    def request(self, method, path, data=None):
        self.requests += 1
        require(self.requests <= 200 and time.monotonic() < self.deadline, 'api_budget_exhausted')
        require(path.startswith('/repos/' + REPOSITORY + '/'), 'unexpected_api_repository')
        request = urllib.request.Request('https://api.github.com' + path, method=method,
                                        headers={'Authorization': 'Bearer ' + self.token,
                                                 'Accept': 'application/vnd.github+json',
                                                 'X-GitHub-Api-Version': '2022-11-28'},
                                        data=None if data is None else json.dumps(data).encode())
        try:
            with urllib.request.urlopen(request, timeout=min(20, max(1, self.deadline - time.monotonic()))) as response:
                raw = response.read((2 << 20) + 1)
                require(len(raw) <= 2 << 20, 'api_response_too_large')
            return json.loads(raw, object_pairs_hook=object_unique) if raw else None
        except urllib.error.HTTPError as error:
            status = error.code
            error.close()
            raise Failure(f'api_http_{status}' if type(status) is int and 100 <= status <= 599
                          else 'api_http_error') from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise Failure('api_transport_error') from None
        except (UnicodeError, ValueError):
            raise Failure('invalid_api_json') from None

    def pages(self, path, key=None):
        result = []
        for page in range(1, 101):
            query = ('&' if '?' in path else '?') + f'per_page=100&page={page}'
            data = self.request('GET', path + query)
            items = data.get(key) if key and isinstance(data, dict) else data if key is None else None
            require(isinstance(items, list), 'invalid_api_page')
            result.extend(items)
            if len(items) < 100:
                if key:
                    require(type(data.get('total_count')) is int and len(result) == data['total_count'],
                            'incomplete_api_pages')
                return result
        raise Failure('api_page_limit')


def attempt_evidence(api, name, run_id, attempt):
    require(name in FULL_WORKFLOWS and positive(run_id) and positive(attempt), 'invalid_run_identity')
    base = f'/repos/{REPOSITORY}/actions/runs/{run_id}/attempts/{attempt}'
    run = api.request('GET', base)
    require(isinstance(run, dict) and run.get('id') == run_id and run.get('run_attempt') == attempt,
            'wrong_run_attempt')
    require(run.get('path') == '.github/workflows/' + FULL_WORKFLOWS[name]['file']
            and run.get('name') == name, 'wrong_workflow')
    require(run.get('repository', {}).get('full_name') == REPOSITORY
            and run.get('head_repository', {}).get('full_name') == REPOSITORY, 'wrong_run_repository')
    require(isinstance(run.get('head_sha'), str) and re.fullmatch('[0-9a-f]{40}', run['head_sha']),
            'invalid_run_head')
    jobs = api.pages(base + '/jobs', 'jobs')
    require(all(isinstance(job, dict) and job.get('run_id') == run_id
                and job.get('run_attempt') == attempt and job.get('head_sha') == run['head_sha']
                and isinstance(job.get('name'), str)
                for job in jobs), 'wrong_job_attempt')
    return run, jobs


def area_failures(name, jobs, conclusion):
    require(name in FULL_WORKFLOWS, 'unknown_full_workflow')
    by_name = collections.defaultdict(list)
    for job in jobs:
        by_name[job['name']].append(job)
    failures = {}
    incomplete = []
    for area, names in FULL_WORKFLOWS[name]['areas'].items():
        bad = []
        for job_name in names:
            matches = by_name[job_name]
            if (len(matches) != 1 or matches[0].get('status') != 'completed'
                    or matches[0].get('conclusion') not in ('success', 'failure', 'cancelled', 'timed_out')):
                incomplete.append(job_name)
                continue
            if matches[0]['conclusion'] != 'success':
                bad.append(job_name)
                continue
            steps = matches[0].get('steps')
            if not isinstance(steps, list):
                bad.append(job_name)
                continue
            for proof in PROOF_STEPS[name, job_name.split(' (', 1)[0]]:
                selected = [step for step in steps if isinstance(step, dict) and step.get('name') == proof]
                if (len(selected) != 1 or selected[0].get('status') != 'completed'
                        or selected[0].get('conclusion') != 'success' or not selected[0].get('started_at')
                        or not selected[0].get('completed_at')):
                    bad.append(job_name)
                    break
        if bad:
            failures[area] = bad
    if incomplete:
        failures.setdefault('ci', []).extend(incomplete)
    if not failures and conclusion != 'success':
        failures['ci'] = ['workflow-result']
    return failures


def covers(name, scheduled, covering, jobs):
    try:
        require(name in FULL_WORKFLOWS, 'unknown_full_workflow')
        for run in (scheduled, covering):
            require(positive(run['id']) and positive(run['run_attempt'])
                    and positive(run['workflow_id']) and run['name'] == name
                    and run['path'] == '.github/workflows/' + FULL_WORKFLOWS[name]['file']
                    and run['head_branch'] == 'main'
                    and run['repository']['full_name'] == REPOSITORY
                    and run['head_repository']['full_name'] == REPOSITORY
                    and isinstance(run['head_sha'], str)
                    and re.fullmatch('[0-9a-f]{40}', run['head_sha']), 'wrong_coverage_identity')
        require(scheduled['event'] == 'schedule' and covering['event'] == 'push'
                and scheduled['workflow_id'] == covering['workflow_id']
                and scheduled['head_sha'] == covering['head_sha']
                and covering['status'] == 'completed' and covering['conclusion'] == 'success'
                and dispatch_order(covering)[0] <= dispatch_order(scheduled)[0], 'uncovered_schedule')
        require(isinstance(jobs, list) and all(isinstance(job, dict)
                and job['run_id'] == covering['id'] and job['run_attempt'] == covering['run_attempt']
                and job['head_sha'] == covering['head_sha'] for job in jobs), 'wrong_coverage_jobs')
        return not area_failures(name, jobs, 'success')
    except (Failure, KeyError, TypeError, ValueError):
        return False


def covered_schedule(api, name, run, jobs):
    if run.get('event') != 'schedule' or run.get('conclusion') != 'success':
        return False
    gates = [job for job in jobs if job.get('name') == f'{name}-coverage']
    if len(gates) != 1 or gates[0].get('status') != 'completed' or gates[0].get('conclusion') != 'success':
        return False
    gate, = gates
    others = [job for job in jobs if job is not gate]
    if not others or any(job.get('status') != 'completed' or job.get('conclusion') != 'skipped'
                         for job in others):
        return False
    claims = []
    steps = gate.get('steps')
    if not isinstance(steps, list) or not all(isinstance(step, dict) for step in steps):
        return False
    for step in steps:
        if not isinstance(step.get('name'), str):
            return False
        match = re.fullmatch(r'Covered by run ([1-9][0-9]{0,18}) attempt ([1-9][0-9]{0,3})',
                            step.get('name', ''))
        if step.get('name', '').startswith('Covered by run '):
            if match is None:
                return False
            claims.append((match, step))
    if len(claims) != 1:
        return False
    match, step = claims[0]
    if step.get('status') != 'completed' or step.get('conclusion') != 'success':
        return False
    try:
        covering, full_jobs = attempt_evidence(api, name, int(match[1]), int(match[2]))
    except Failure as error:
        if str(error).startswith('api_'):
            raise
        return False
    return covers(name, run, covering, full_jobs)


def issue_marker(name, area):
    require(name in FULL_WORKFLOWS and area in AREAS, 'invalid_issue_class')
    return f'<!-- ci-full-failure:{name}:{area} -->'


def run_marker(run):
    require(positive(run.get('id')) and positive(run.get('run_attempt')), 'invalid_run_identity')
    return f'<!-- ci-full-run:{run["id"]}:{run["run_attempt"]} -->'


def bot_owned(item):
    return isinstance(item, dict) and item.get('user', {}).get('login') == BOT


def matching_issues(api, marker):
    items = api.pages(f'/repos/{REPOSITORY}/issues?state=open&creator={urllib.parse.quote(BOT)}')
    found = [item for item in items if bot_owned(item) and 'pull_request' not in item
             and isinstance(item.get('body'), str) and marker in item['body']]
    require(all(positive(item.get('number')) for item in found), 'invalid_issue_number')
    return sorted(found, key=lambda item: item['number'])


def ensure_comment(api, issue, marker, body):
    number = issue['number']
    comments = api.pages(f'/repos/{REPOSITORY}/issues/{number}/comments')
    found = [comment for comment in comments if bot_owned(comment)
             and isinstance(comment.get('body'), str) and marker in comment['body']]
    if marker not in issue['body'] and not found:
        try:
            api.request('POST', f'/repos/{REPOSITORY}/issues/{number}/comments', {'body': body})
        except Failure:
            comments = api.pages(f'/repos/{REPOSITORY}/issues/{number}/comments')
            require(any(bot_owned(comment) and marker in (comment.get('body') or '') for comment in comments),
                    'comment_write_unconfirmed')
            return
        comments = api.pages(f'/repos/{REPOSITORY}/issues/{number}/comments')
        found = [comment for comment in comments if bot_owned(comment)
                 and isinstance(comment.get('body'), str) and marker in comment['body']]
    require(all(positive(comment.get('id')) for comment in found), 'invalid_comment_identity')
    keep = None if marker in issue['body'] else min((comment['id'] for comment in found), default=None)
    for comment in found:
        if comment['id'] != keep:
            api.request('DELETE', f'/repos/{REPOSITORY}/issues/comments/{comment["id"]}')


def failure_body(name, area, bad, run):
    title, _ = AREAS[area]
    require(isinstance(run.get('head_sha'), str) and re.fullmatch('[0-9a-f]{40}', run['head_sha']),
            'invalid_run_head')
    return '\n'.join([issue_marker(name, area), run_marker(run),
                      f'Owning area: {title}.', '',
                      'The owning team must fix or revert the regression within one day.', '',
                      f'Workflow: `{name}`. Incomplete or unsuccessful checks: ' + ', '.join(f'`{job}`' for job in bad) + '.',
                      f'Head: `{run["head_sha"]}`. Attempt: {run["run_attempt"]}.',
                      f'https://github.com/{REPOSITORY}/actions/runs/{run["id"]}/attempts/{run["run_attempt"]}', '',
                      'Inspect the workflow evidence before choosing a fix. Later success does not automatically close this issue.'])


def report(event, api):
    candidate = event.get('workflow_run', {})
    if (event.get('action') != 'completed' or event.get('repository', {}).get('full_name') != REPOSITORY
            or candidate.get('name') not in FULL_WORKFLOWS or candidate.get('event') not in ('push', 'schedule')
            or candidate.get('head_branch') != 'main'
            or candidate.get('head_repository', {}).get('full_name') != REPOSITORY):
        return []
    name = candidate['name']
    run, jobs = attempt_evidence(api, name, candidate.get('id'), candidate.get('run_attempt'))
    require(run.get('status') == 'completed' and run.get('event') == candidate['event']
            and run.get('head_branch') == 'main' and run.get('head_sha') == candidate.get('head_sha'),
            'wrong_completed_event')
    if covered_schedule(api, name, run, jobs):
        return []
    results = []
    for area, bad in area_failures(name, jobs, run.get('conclusion')).items():
        marker = issue_marker(name, area)
        body = failure_body(name, area, bad, run)
        matches = matching_issues(api, marker)
        if not matches:
            title, label = AREAS[area]
            try:
                api.request('POST', f'/repos/{REPOSITORY}/issues', {
                    'title': f'Full CI failed for {title}', 'body': body,
                    'labels': list(dict.fromkeys(('bug', 'area:ci', label))),
                })
            except Failure:
                matches = matching_issues(api, marker)
                require(matches, 'issue_write_unconfirmed')
            matches = matching_issues(api, marker)
            require(matches, 'issue_write_unconfirmed')
        keeper = matches[0]
        ensure_comment(api, keeper, run_marker(run), body)
        for duplicate in matches[1:]:
            sources = [duplicate['body']] + [comment.get('body', '') for comment in
                       api.pages(f'/repos/{REPOSITORY}/issues/{duplicate["number"]}/comments') if bot_owned(comment)]
            for source in sources:
                for found in re.finditer(r'<!-- ci-full-run:([1-9][0-9]*):([1-9][0-9]*) -->', source):
                    copied = {'id': int(found[1]), 'run_attempt': int(found[2]), 'head_sha': run['head_sha']}
                    link = f'https://github.com/{REPOSITORY}/actions/runs/{copied["id"]}/attempts/{copied["run_attempt"]}'
                    ensure_comment(api, keeper, run_marker(copied), run_marker(copied) + '\n' + link)
            api.request('PATCH', f'/repos/{REPOSITORY}/issues/{duplicate["number"]}', {'state': 'closed'})
        results.append(keeper['number'])
    return results


def dispatch_order(run):
    value = run.get('created_at')
    require(isinstance(value, str) and re.fullmatch(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z', value),
            'invalid_dispatch_timestamp')
    try:
        parsed = datetime.datetime.fromisoformat(value.replace('Z', '+00:00'))
    except ValueError:
        raise Failure('invalid_dispatch_timestamp') from None
    return parsed, run['id']


def selected_suites(suites=None):
    names = ([name for name, spec in FULL_WORKFLOWS.items() if spec['final_head'] == 'always']
             if suites is None else suites)
    require(bool(names) and all(name in FULL_WORKFLOWS for name in names), 'invalid_suite_selection')
    return list(dict.fromkeys(names))


def verify(pr_number, api, suites=None):
    names = selected_suites(suites)
    require(positive(pr_number), 'invalid_pull_number')
    pull = api.request('GET', f'/repos/{REPOSITORY}/pulls/{pr_number}')
    require(isinstance(pull, dict) and pull.get('state') == 'open'
            and pull.get('head', {}).get('repo', {}).get('full_name') == REPOSITORY, 'invalid_pull_head')
    head = pull['head'].get('sha')
    require(isinstance(head, str) and re.fullmatch('[0-9a-f]{40}', head), 'invalid_pull_head')
    evidence = []
    for name in names:
        spec = FULL_WORKFLOWS[name]
        query = urllib.parse.urlencode({'event': 'workflow_dispatch', 'head_sha': head})
        runs = api.pages(f'/repos/{REPOSITORY}/actions/workflows/{spec["file"]}/runs?{query}', 'workflow_runs')
        require(all(isinstance(run, dict) and positive(run.get('id')) and positive(run.get('run_attempt'))
                    and isinstance(run.get('created_at'), str) for run in runs), 'invalid_dispatch_index')
        require(runs, 'missing_final_head_dispatch')
        latest = max(runs, key=dispatch_order)
        run, jobs = attempt_evidence(api, name, latest['id'], latest['run_attempt'])
        require(run.get('event') == 'workflow_dispatch' and run.get('head_sha') == head
                and run.get('head_branch') == pull['head'].get('ref'), 'dispatch_not_at_final_head')
        require(run.get('status') == 'completed' and run.get('conclusion') == 'success'
                and not area_failures(name, jobs, run.get('conclusion')), 'final_head_dispatch_not_green')
        evidence.append((name, run['id'], run['run_attempt']))
    for name, run_id, attempt in evidence:
        query = urllib.parse.urlencode({'event': 'workflow_dispatch', 'head_sha': head})
        runs = api.pages(f'/repos/{REPOSITORY}/actions/workflows/{FULL_WORKFLOWS[name]["file"]}/runs?{query}',
                         'workflow_runs')
        require(isinstance(runs, list) and runs and all(isinstance(run, dict)
                and positive(run.get('id')) and positive(run.get('run_attempt')) for run in runs),
                'dispatch_evidence_changed')
        latest = max(runs, key=dispatch_order)
        require(latest['id'] == run_id and latest['run_attempt'] == attempt
                and latest.get('status') == 'completed' and latest.get('conclusion') == 'success',
                'dispatch_evidence_changed')
    fresh = api.request('GET', f'/repos/{REPOSITORY}/pulls/{pr_number}')
    require(fresh.get('state') == 'open' and fresh.get('head', {}).get('sha') == head, 'pull_head_changed')
    return head, evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    reporting = commands.add_parser('report')
    reporting.add_argument('--event', type=Path, required=True)
    checking = commands.add_parser('verify')
    checking.add_argument('--pr', type=int, required=True)
    checking.add_argument('--suite', action='append')
    args = parser.parse_args()
    try:
        token = os.environ.get('GH_TOKEN')
        require(bool(token), 'missing_github_token')
        api = Api(token)
        if args.command == 'report':
            event = json.loads(args.event.read_text(), object_pairs_hook=object_unique)
            issues = report(event, api)
            print(f'ci-full: {len(issues)} failing areas reported')
        else:
            head, evidence = verify(args.pr, api, args.suite)
            print(f'ci-full: exact head {head}')
            for name, run, attempt in evidence:
                print(f'{name}: https://github.com/{REPOSITORY}/actions/runs/{run}/attempts/{attempt}')
        return 0
    except Failure as error:
        print('ci-full: ' + str(error), file=sys.stderr)
        return 1
    except Exception:
        print('ci-full: invalid_evidence', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
