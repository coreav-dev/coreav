#!/usr/bin/env python3
"""Observe required reports missing from a queued head's check-run index."""
import argparse
import datetime
import email.utils
import hashlib
import http.client
import json
import os
import pathlib
import random
import re
import signal
import time
import urllib.error
import urllib.parse
import urllib.request


REPO = 'coreav-dev/coreav'
TRACKING_ISSUE = 43
WORKFLOW_OF = {'ci-report': 'ci.yml', 'deny': 'deny.yml',
               'python-report': 'python.yml', 'npm-report': 'npm.yml'}
APP = 15368
SCHEMA = 1
GAP = 1200
MIN_AGE = 1200
RULES_TTL = 3600
CALL_CAP = 120
WALL = 60
NOTICE = '<!-- coreav-queue-detector:v1:activation -->'
QUEUE_QUERY = '''query($cursor:String) {
  repository(owner:"coreav-dev",name:"coreav") {
    mergeQueue(branch:"main") {
      entries(first:100,after:$cursor) {
        totalCount pageInfo { hasNextPage endCursor }
        nodes { id state headCommit { oid } pullRequest { number headRefOid } }
      }
    }
  }
}'''


class Unknown(Exception):
    pass


class ConfigError(Unknown):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def stamp(value):
    parsed = email.utils.parsedate_to_datetime(value)
    if parsed.tzinfo is None:
        raise ValueError('HTTP Date has no timezone')
    return int(parsed.timestamp())


def positive(value):
    if type(value) is not int or value <= 0:
        raise Unknown('Invalid identifier')
    return value


def sha(value):
    if not isinstance(value, str) or not re.fullmatch('[0-9a-f]{40}', value):
        raise Unknown('Invalid commit')
    return value


class Api:
    def __init__(self, token, opener=None, clock=time.monotonic, pause=time.sleep):
        self.token = token
        self.opener = opener or urllib.request.build_opener(NoRedirect()).open
        self.clock = clock
        self.pause = pause
        self.started = clock()
        self.calls = 0
        self.date = None
        self.links = ''
        self.allow_comments = False

    def request(self, method, path, body=None):
        if method not in ('GET', 'POST'):
            raise ConfigError('Unexpected API method')
        if path != '/graphql' and not path.startswith(f'/repos/{REPO}/'):
            raise ConfigError('Unexpected API path')
        if path == '/graphql' and (body or {}).get('query') != QUEUE_QUERY:
            raise ConfigError('Unexpected GraphQL operation')
        if method == 'POST' and path != '/graphql':
            if not self.allow_comments or path != f'/repos/{REPO}/issues/{TRACKING_ISSUE}/comments':
                raise ConfigError('Comment publishing is disabled')
        for attempt in range(3):
            remaining = WALL - (self.clock() - self.started)
            if self.calls >= CALL_CAP or remaining <= 0:
                raise Unknown('Tick budget exhausted')
            self.calls += 1
            req = urllib.request.Request(
                'https://api.github.com' + path, method=method,
                data=None if body is None else json.dumps(body).encode(),
                headers={'Authorization': 'Bearer ' + self.token,
                         'Accept': 'application/vnd.github+json',
                         'Content-Type': 'application/json', 'X-GitHub-Api-Version': '2022-11-28'})
            try:
                with self.opener(req, timeout=min(10, remaining)) as response:
                    if method == 'POST' and path != '/graphql' and response.status != 201:
                        raise Unknown('Comment creation was not acknowledged')
                    self.date = stamp(response.headers['Date'])
                    self.links = response.headers.get('Link', '')
                    quota = response.headers.get('X-RateLimit-Remaining')
                    if quota is not None and int(quota) < 150:
                        raise Unknown('Rate reserve reached')
                    chunks, size = [], 0
                    while True:
                        if self.clock() - self.started >= WALL:
                            raise Unknown('Tick budget exhausted')
                        chunk = response.read1(16 * 1024)
                        if not chunk:
                            break
                        chunks.append(chunk)
                        size += len(chunk)
                        if size > 2 * 1024 * 1024:
                            raise Unknown('API response exceeds limit')
                    data = b''.join(chunks)
                    result = json.loads(data)
                    if method == 'POST' and path != '/graphql':
                        positive(result['id'])
                    if self.clock() - self.started > WALL:
                        raise Unknown('Tick budget exhausted')
                    return result
            except urllib.error.HTTPError as error:
                if error.code == 401:
                    raise ConfigError('API authentication refused') from None
                if error.code in (403, 429):
                    headers = error.headers or {}
                    if error.code == 429 or headers.get('Retry-After') or headers.get('X-RateLimit-Remaining') == '0':
                        raise Unknown('API rate limited') from None
                    if error.fp is not None:
                        try:
                            detail = error.read(4096).decode('utf-8', errors='replace').lower()
                        except (OSError, http.client.HTTPException):
                            raise Unknown('Incomplete API refusal') from None
                        if 'secondary rate limit' in detail:
                            raise Unknown('API rate limited') from None
                    raise ConfigError('API permission refused') from None
                retryable = error.code >= 500
            except (urllib.error.URLError, TimeoutError, OSError, http.client.HTTPException):
                retryable = True
            except (ValueError, TypeError, KeyError, AttributeError):
                raise Unknown('Invalid API response or HTTP Date') from None
            if not retryable or attempt == 2 or (method == 'POST' and path != '/graphql'):
                raise Unknown('API request failed') from None
            delay = random.uniform(0.2, 0.8) * (attempt + 1)
            if self.clock() - self.started + delay >= WALL:
                raise Unknown('Tick budget exhausted')
            self.pause(delay)
        raise Unknown('API request failed')

    def get(self, path, **params):
        suffix = '?' + urllib.parse.urlencode(params) if params else ''
        return self.request('GET', path + suffix)

    def pages(self, path, key=None, **params):
        rows, total, seen, urls = [], None, set(), set()
        url = path + '?' + urllib.parse.urlencode(dict(params, per_page=100))
        while url:
            if url in urls:
                raise Unknown('Pagination repeated a page')
            urls.add(url)
            data = self.request('GET', url)
            page = data if key is None else data[key]
            if not isinstance(page, list):
                raise Unknown('Invalid API page')
            if key is not None:
                count = data['total_count']
                if type(count) is not int or count < 0 or (total is not None and total != count):
                    raise Unknown('Pagination count changed')
                total = count
            for row in page:
                if not isinstance(row, dict):
                    raise Unknown('Invalid API row')
                if 'id' in row:
                    if row['id'] in seen:
                        raise Unknown('Pagination repeated a row')
                    seen.add(row['id'])
            rows.extend(page)
            links = re.findall(r'<([^>]+)>;\s*rel="([^"]+)"', self.links)
            if self.links and not links:
                raise Unknown('Invalid pagination link')
            following = [link for link, relation in links if relation == 'next']
            if len(following) > 1:
                raise Unknown('Ambiguous pagination link')
            url = None
            if following:
                parsed = urllib.parse.urlsplit(following[0])
                if (parsed.scheme != 'https' or parsed.netloc != 'api.github.com'
                        or parsed.path != path or parsed.fragment):
                    raise Unknown('Unexpected pagination destination')
                url = parsed.path + '?' + parsed.query
        if total is not None and len(rows) != total:
            raise Unknown('Incomplete pagination')
        return rows

    def graphql(self, cursor=None):
        body = self.request('POST', '/graphql', {'query': QUEUE_QUERY, 'variables': {'cursor': cursor}})
        if body.get('errors') or not body.get('data'):
            raise Unknown('GraphQL evidence unavailable')
        return body['data']

    def post(self, body):
        return self.request('POST', f'/repos/{REPO}/issues/{TRACKING_ISSUE}/comments', {'body': body})


def queue(api):
    entries, cursor, cursors, identities, total, now = [], None, set(), set(), None, None
    while True:
        data = api.graphql(cursor)['repository']['mergeQueue']
        if data is None:
            raise ConfigError('Merge queue is unavailable')
        now = api.date if now is None else now
        connection = data['entries']
        count = connection['totalCount']
        if type(count) is not int or count < 0 or (total is not None and count != total):
            raise Unknown('Queue changed during pagination')
        total = count
        for node in connection['nodes']:
            entry_id = node['id']
            if (not isinstance(entry_id, str) or not re.fullmatch(r'[A-Za-z0-9_=-]{1,256}', entry_id)
                    or entry_id in identities):
                raise Unknown('Invalid queue entry identity')
            identities.add(entry_id)
            entry = {'entry': entry_id, 'state': node['state'],
                     'pr': positive(node['pullRequest']['number']),
                     'pr_head': sha(node['pullRequest']['headRefOid']),
                     'head': sha((node.get('headCommit') or {}).get('oid'))}
            entries.append(entry)
        page = connection['pageInfo']
        if page['hasNextPage'] is False:
            break
        cursor = page['endCursor']
        if page['hasNextPage'] is not True or not cursor or cursor in cursors:
            raise Unknown('Invalid queue pagination')
        cursors.add(cursor)
    if len(entries) != total:
        raise Unknown('Incomplete queue pagination')
    return entries, now


def required(api, state, now, refresh=False):
    cached = state.get('rules', {})
    age = now - cached.get('fetched_at', 0)
    if not refresh and 0 <= age < RULES_TTL and cached.get('contexts') == WORKFLOW_OF:
        return cached
    rules = api.pages(f'/repos/{REPO}/rules/branches/main')
    contexts = {}
    for rule in rules:
        if rule['type'] != 'required_status_checks':
            continue
        for check in rule['parameters']['required_status_checks']:
            context = check['context']
            if context not in WORKFLOW_OF or check.get('integration_id') != APP:
                raise ConfigError('Unmapped required context or app')
            contexts[context] = WORKFLOW_OF[context]
    if contexts != WORKFLOW_OF:
        raise ConfigError('Required-context configuration changed')
    state['rules'] = {'fetched_at': now, 'contexts': contexts}
    return state['rules']


def latest_runs(api, head):
    rows = api.pages(f'/repos/{REPO}/actions/runs', 'workflow_runs', head_sha=head, event='merge_group')
    if len(rows) >= 1000:
        raise Unknown('Actions search may have reached its result limit')
    latest, identities, numbers = {}, {}, set()
    for run in rows:
        workflow = run.get('path', '').removeprefix('.github/workflows/')
        if workflow not in WORKFLOW_OF.values():
            continue
        positive(run['id'])
        positive(run['run_number'])
        positive(run['run_attempt'])
        workflow_id = positive(run['workflow_id'])
        if workflow in identities and identities[workflow] != workflow_id:
            raise Unknown('Workflow identity changed')
        identities[workflow] = workflow_id
        number = workflow_id, run['run_number']
        if number in numbers:
            raise Unknown('Ambiguous workflow run ordering')
        numbers.add(number)
        previous = latest.get(workflow)
        if previous is None or (run['run_number'], run['id']) > (previous['run_number'], previous['id']):
            latest[workflow] = run
    if set(latest) != set(WORKFLOW_OF.values()):
        return None
    for run in latest.values():
        if (run.get('head_sha') != head or run.get('event') != 'merge_group'
                or (run.get('head_repository') or {}).get('full_name') != REPO
                or not run.get('head_branch', '').startswith('gh-readonly-queue/main/')
                or run.get('status') != 'completed' or run.get('conclusion') != 'success'):
            return None
    return latest


def matches(check, head, context):
    return (check.get('name') == context and check.get('head_sha') == head
            and (check.get('app') or {}).get('id') == APP)


def successful(check):
    return check.get('status') == 'completed' and check.get('conclusion') == 'success'


def index_rows(api, head, context):
    rows = api.pages(f'/repos/{REPO}/commits/{head}/check-runs', 'check_runs',
                     check_name=context, filter='latest', app_id=APP)
    for row in rows:
        positive(row['id'])
        sha(row['head_sha'])
        positive(row['app']['id'])
        if not isinstance(row['name'], str) or not isinstance(row['status'], str):
            raise Unknown('Incomplete check-run index row')
    return [row for row in rows if matches(row, head, context)]


def checked(api, check_id, head, context):
    check = api.get(f'/repos/{REPO}/check-runs/{positive(check_id)}')
    if check.get('id') != check_id or not matches(check, head, context) or not successful(check):
        raise Unknown('Direct check no longer proves success')
    return check


def direct(api, run, context, head):
    jobs = api.pages(f'/repos/{REPO}/actions/runs/{run["id"]}/attempts/{run["run_attempt"]}/jobs', 'jobs')
    gates = [job for job in jobs if job.get('name') == context]
    if not gates:
        suite = positive(run['check_suite_id'])
        checks = api.pages(f'/repos/{REPO}/check-suites/{suite}/check-runs', 'check_runs', filter='all')
        if any(matches(check, head, context) for check in checks):
            raise Unknown('Gate job and suite disagree')
        return None
    if len(gates) != 1 or not successful(gates[0]):
        raise Unknown('Gate job is ambiguous or not successful')
    job = gates[0]
    url = urllib.parse.urlsplit(job['check_run_url'])
    reference = re.search(r'/repos/' + re.escape(REPO) + r'/check-runs/([0-9]+)$', url.path)
    if not reference or url.query or url.fragment:
        raise Unknown('Invalid direct check reference')
    check_id = positive(int(reference.group(1)))
    check = checked(api, check_id, head, context)
    return {'check_id': check_id, 'job_id': positive(job['id']),
            'completed_at': check.get('completed_at'), 'run_created_at': run.get('created_at')}


def observe(api, state):
    entries, now = queue(api)
    required(api, state, now)
    tick = {'now': now, 'entries': entries, 'results': {}, 'runs': {}}
    inspected = 0
    for entry in entries:
        if entry['state'] != 'AWAITING_CHECKS':
            continue
        identifier = entry['entry']
        inspected += 1
        if inspected > 5:
            for context in WORKFLOW_OF:
                tick['results'][identifier, context] = {'kind': 'UNKNOWN'}
            continue
        try:
            runs = latest_runs(api, entry['head'])
        except ConfigError:
            raise
        except (Unknown, KeyError, TypeError, ValueError, AttributeError):
            for context in WORKFLOW_OF:
                tick['results'][identifier, context] = {'kind': 'UNKNOWN'}
            continue
        if runs is None:
            for context in WORKFLOW_OF:
                tick['results'][identifier, context] = {'kind': 'INELIGIBLE'}
            continue
        tick['runs'][identifier] = runs
        for context, workflow in WORKFLOW_OF.items():
            try:
                rows = index_rows(api, entry['head'], context)
                if rows:
                    result = {'kind': 'PRESENT' if any(successful(row) for row in rows) else 'INDEX_STALE'}
                else:
                    proof = direct(api, runs[workflow], context, entry['head'])
                    result = {'kind': 'MISSING_REQUIRED', 'proof': proof} if proof else {'kind': 'NO_GATE_JOB'}
                tick['results'][identifier, context] = result
            except ConfigError:
                raise
            except (Unknown, KeyError, TypeError, ValueError, AttributeError):
                tick['results'][identifier, context] = {'kind': 'UNKNOWN'}
    return tick


def episode_key(entry, context, check_id):
    values = [REPO, entry['entry'], entry['head'], context, check_id]
    return hashlib.sha256(json.dumps(values, separators=(',', ':')).encode()).hexdigest()


def reset(episodes):
    for record in episodes.values():
        record.update(eligible_since=None, clean=0)


def step(episodes, tick):
    now = tick['now']
    if any(record['last_clean'] > now for record in episodes.values()):
        episodes = {}
    updated, due = {}, []
    for entry in tick['entries']:
        for context, workflow in WORKFLOW_OF.items():
            result = tick['results'].get((entry['entry'], context), {})
            if result.get('kind') in ('UNKNOWN', 'INDEX_STALE'):
                for key, old in episodes.items():
                    if (old['entry'] == entry['entry'] and old['head'] == entry['head']
                            and old['pr_head'] == entry['pr_head'] and old['context'] == context):
                        updated[key] = dict(old, eligible_since=None, clean=0)
                continue
            if result.get('kind') != 'MISSING_REQUIRED':
                continue
            proof = result['proof']
            key = episode_key(entry, context, proof['check_id'])
            old = episodes.get(key)
            if old is not None and old['pr_head'] != entry['pr_head']:
                old = None
            run = tick['runs'][entry['entry']][workflow]
            roster = {name: [item['id'], item['run_attempt']]
                      for name, item in tick['runs'][entry['entry']].items()}
            record = dict(entry, **proof, context=context, run_id=run['id'], attempt=run['run_attempt'], runs=roster)
            record.update(first_seen=old['first_seen'] if old else now,
                          alerted_at=old['alerted_at'] if old else None)
            if (old is None or old['eligible_since'] is None or now - old['last_clean'] > GAP
                    or old.get('runs') != roster):
                record.update(eligible_since=now, clean=1)
            else:
                record.update(eligible_since=old['eligible_since'],
                              clean=old['clean'] + (now > old['last_clean']))
            record['last_clean'] = now
            updated[key] = record
            if (record['clean'] >= 3 and now - record['eligible_since'] >= MIN_AGE
                    and record['alerted_at'] is None):
                due.append(key)
    return updated, due


def recheck(api, state, due):
    entries, now = queue(api)
    required(api, state, now, refresh=True)
    current = {entry['entry']: entry for entry in entries}
    valid, runs_by_head = [], {}
    for key in due:
        record = state['episodes'][key]
        entry = current.get(record['entry'])
        if (entry is None or entry['state'] != 'AWAITING_CHECKS' or entry['head'] != record['head']
                or entry['pr_head'] != record['pr_head'] or entry['pr'] != record['pr']):
            continue
        if record['head'] not in runs_by_head:
            runs_by_head[record['head']] = latest_runs(api, record['head'])
        runs = runs_by_head[record['head']]
        if runs is None:
            continue
        if {name: [item['id'], item['run_attempt']] for name, item in runs.items()} != record['runs']:
            continue
        run = runs[WORKFLOW_OF[record['context']]]
        if run['id'] != record['run_id'] or run['run_attempt'] != record['attempt']:
            continue
        if index_rows(api, record['head'], record['context']):
            continue
        checked(api, record['check_id'], record['head'], record['context'])
        valid.append(key)
    return valid


def published(api):
    comments = api.pages(f'/repos/{REPO}/issues/{TRACKING_ISSUE}/comments')
    markers = set()
    for comment in comments:
        if (comment.get('user') or {}).get('login') != 'github-actions[bot]':
            continue
        body = comment.get('body') or ''
        markers.update(re.findall(r'<!-- coreav-queue-detector:v1:([a-z0-9]+) -->', body))
    return markers


def kst(seconds):
    zone = datetime.timezone(datetime.timedelta(hours=9))
    return datetime.datetime.fromtimestamp(seconds, zone).isoformat()


def source_time(value):
    try:
        return datetime.datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()
    except (AttributeError, TypeError, ValueError):
        return None


def comment_body(records, now):
    first = records[0]
    lines = [f"Queue required-check observation for coreav-dev/coreav#{first['pr']}.", '',
             f"PR head: `{first['pr_head']}`. Group SHA: `{first['head']}`.",
             f"Observed at {kst(now)} (KST).", '',
             'Required report present by direct record but absent from the check-run index for a queued head; '
             'the cause of the omission is not established; the detector took no action.', '']
    for record in records:
        end, start = source_time(record['completed_at']), source_time(record['run_created_at'])
        delta = 'unavailable' if end is None or start is None else f'{end - start:.0f} seconds'
        lines.extend([
            f"- `{record['context']}`: run {record['run_id']}, attempt {record['attempt']}, "
            f"job {record['job_id']}, direct check {record['check_id']}, entry `{record['entry']}`.",
            f"  First seen {kst(record['first_seen'])}; eligible since {kst(record['eligible_since'])}; "
            f"{record['clean']} clean observations.",
            f"  Index: `GET /repos/{REPO}/commits/{record['head']}/check-runs?"
            f"check_name={record['context']}&filter=latest&app_id={APP}` returned zero matching rows.",
            f'  Recorded check completion minus run creation: {delta}.',
            '<!-- coreav-queue-detector:v1:' + episode_key(record, record['context'], record['check_id']) + ' -->'])
    return '\n'.join(lines) + '\n'


def report(api, state, due, live):
    groups, summaries = {}, []
    for key in due:
        record = state['episodes'][key]
        groups.setdefault((record['entry'], record['head']), []).append(key)
    for keys in groups.values():
        valid = recheck(api, state, keys)
        for key in set(keys) - set(valid):
            state['episodes'][key].update(eligible_since=None, clean=0)
        if not valid:
            continue
        markers = published(api)
        unpublished = []
        for key in valid:
            if key in markers:
                state['episodes'][key]['alerted_at'] = api.date
            else:
                unpublished.append(key)
        if not unpublished:
            continue
        records = [state['episodes'][key] for key in unpublished]
        body = comment_body(records, api.date)
        summaries.append(('Published alert' if live else 'Shadow: would publish') + '\n\n' + body)
        if live:
            api.post(body)
            for key in unpublished:
                state['episodes'][key]['alerted_at'] = api.date
    return summaries


def empty_state():
    return {'schema_version': SCHEMA, 'repo': REPO, 'rules': {}, 'episodes': {},
            'shadow': {'ticks': 0, 'unknown_ticks': 0, 'would_alert': []}}


def load_state(path):
    try:
        if path.stat().st_size > 1024 * 1024:
            return empty_state()
        state = json.loads(path.read_text())
        if type(state['schema_version']) is not int or state['schema_version'] != SCHEMA or state['repo'] != REPO:
            return empty_state()
        if not isinstance(state['rules'], dict) or not isinstance(state['episodes'], dict):
            return empty_state()
        rules = state['rules']
        if rules and (type(rules.get('fetched_at')) is not int or rules['fetched_at'] < 0
                      or rules.get('contexts') != WORKFLOW_OF):
            return empty_state()
        for key, record in state['episodes'].items():
            if episode_key(record, record['context'], positive(record['check_id'])) != key:
                return empty_state()
            sha(record['head'])
            sha(record['pr_head'])
            for name in ('first_seen', 'last_clean', 'clean'):
                if type(record[name]) is not int or record[name] < 0:
                    return empty_state()
            for name in ('eligible_since', 'alerted_at'):
                if record[name] is not None and (type(record[name]) is not int or record[name] < 0):
                    return empty_state()
            positive(record['run_id'])
            positive(record['attempt'])
            if record['context'] not in WORKFLOW_OF or not isinstance(record['runs'], dict):
                return empty_state()
            if record['eligible_since'] is not None and not record['first_seen'] <= record['eligible_since'] <= record['last_clean']:
                return empty_state()
        shadow = state['shadow']
        if (type(shadow['ticks']) is not int or shadow['ticks'] < 0
                or type(shadow['unknown_ticks']) is not int or shadow['unknown_ticks'] < 0
                or not isinstance(shadow['would_alert'], list) or len(shadow['would_alert']) > 20
                or ('last_tick' in shadow and type(shadow['last_tick']) is not int)):
            return empty_state()
        return state
    except (OSError, ValueError, KeyError, TypeError, AttributeError, Unknown):
        return empty_state()


def save_state(path, state):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(state, sort_keys=True, allow_nan=False) + '\n')
    temporary.replace(path)


def probe(api):
    entries, now = queue(api)
    state = empty_state()
    required(api, state, now)
    rows = api.get(f'/repos/{REPO}/actions/workflows/deny.yml/runs', event='merge_group',
                   status='success', per_page=1)['workflow_runs']
    if not rows:
        raise ConfigError('No completed merge-group run for the permission probe')
    run = rows[0]
    head = sha(run['head_sha'])
    api.pages(f'/repos/{REPO}/actions/runs', 'workflow_runs', head_sha=head, event='merge_group')
    proof = direct(api, run, 'deny', head)
    if proof is None:
        raise ConfigError('No direct gate check for the permission probe')
    index_rows(api, head, 'deny')
    api.pages(f'/repos/{REPO}/check-suites/{positive(run["check_suite_id"])}/check-runs',
              'check_runs', filter='all')
    published(api)
    return (f'Permission probe read queue ({len(entries)} entries), rules, runs, attempt jobs, '
            'direct check, suite checks, index and issue comments.')


def deadline_alarm(signum, frame):
    raise Unknown('Tick deadline reached')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state', type=pathlib.Path, required=True)
    parser.add_argument('--mode', choices=('shadow', 'live', 'probe', 'notice'), default='shadow')
    args = parser.parse_args()
    state = load_state(args.state)
    api = Api(os.environ.get('GH_TOKEN', ''))
    api.allow_comments = args.mode in ('live', 'notice')
    summaries, exit_code = [], 0
    previous_alarm = signal.signal(signal.SIGALRM, deadline_alarm)
    signal.setitimer(signal.ITIMER_REAL, WALL)
    try:
        if not api.token:
            raise ConfigError('API token is missing')
        if (os.environ.get('GITHUB_REPOSITORY') != REPO
                or os.environ.get('GITHUB_REF') != 'refs/heads/main'):
            raise ConfigError('Only trusted main may run the detector')
        if args.mode == 'live' and os.environ.get('GITHUB_EVENT_NAME') != 'schedule':
            raise ConfigError('Only scheduled runs may publish alerts')
        if args.mode in ('probe', 'notice') and os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch':
            raise ConfigError('Permission probes require manual dispatch')
        if args.mode == 'live' and 'activation' not in published(api):
            raise ConfigError('Live activation notice is missing; keep alerts disabled')
        if args.mode in ('probe', 'notice'):
            summaries.append(probe(api))
            if args.mode == 'notice':
                if 'activation' not in published(api):
                    api.post('Queue detector permission check completed. Alerts remain controlled by '
                             'the live-mode setting. This workflow never reruns jobs or changes the queue.\n\n' + NOTICE)
                    summaries.append('Activation notice published; issue-comment write permission verified.')
                else:
                    summaries.append('Activation notice already exists; no duplicate posted.')
        else:
            tick = observe(api, state)
            state['episodes'], due = step(state['episodes'], tick)
            unknown = sum(result['kind'] == 'UNKNOWN' for result in tick['results'].values())
            state['shadow']['ticks'] += 1
            state['shadow']['unknown_ticks'] += bool(unknown)
            summaries.append(f"At {kst(tick['now'])} (server Date, KST), observed "
                             f"{len(tick['entries'])} queue entries; {unknown} unknown contexts.")
            previous_tick = state['shadow'].get('last_tick')
            if previous_tick is not None:
                summaries.append(f"Server-clock gap since previous tick: {tick['now'] - previous_tick} seconds.")
            state['shadow']['last_tick'] = tick['now']
            for entry in tick['entries']:
                outcomes = ', '.join(context + '=' + tick['results'].get((entry['entry'], context),
                                     {'kind': entry['state']})['kind'] for context in WORKFLOW_OF)
                summaries.append(f"coreav-dev/coreav#{entry['pr']} PR head `{entry['pr_head']}`, "
                                 f"group `{entry['head']}`: {outcomes}.")
            save_state(args.state, state)
            summaries.extend(report(api, state, due, args.mode == 'live'))
            if args.mode == 'shadow' and due:
                state['shadow']['would_alert'] = (state['shadow']['would_alert'] +
                                                 [{'date': tick['now'], 'keys': due}])[-20:]
    except ConfigError as error:
        reset(state['episodes'])
        summaries.append('Configuration unavailable: ' + str(error))
        exit_code = 1
    except (Unknown, OSError, ValueError, KeyError, TypeError, AttributeError):
        reset(state['episodes'])
        summaries.append('UNKNOWN: evidence incomplete; observation streaks reset and no further comment attempted.')
        exit_code = int(args.mode in ('probe', 'notice'))
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous_alarm)
        save_state(args.state, state)
    summaries.append(f'API requests this run: {api.calls}.')
    text = '\n\n'.join(summaries) + '\n'
    print(text)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        pathlib.Path(os.environ['GITHUB_STEP_SUMMARY']).write_text(text)
    return exit_code


if __name__ == '__main__':
    raise SystemExit(main())
