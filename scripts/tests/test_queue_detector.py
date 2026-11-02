"""Shadow the queue detector with scripted API evidence and server clocks."""
import copy
import datetime
import email.utils
import importlib.util
import http.client
import io
import json
import os
import pathlib
import tempfile
import unittest
import urllib.error
import urllib.parse
from unittest.mock import patch


ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('queue_detector', ROOT / 'scripts/queue_detector.py')
detector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(detector)
NOW = int(datetime.datetime(2026, 11, 2, tzinfo=datetime.timezone.utc).timestamp())
HEAD = 'a' * 40
PR_HEAD = 'b' * 40
BASE = '/repos/' + detector.REPO


def entry(identifier='entry1', head=HEAD):
    return {'id': identifier, 'state': 'AWAITING_CHECKS', 'headCommit': {'oid': head},
            'pullRequest': {'number': 12, 'headRefOid': PR_HEAD}}


def run(index):
    workflow = list(detector.WORKFLOW_OF.values())[index]
    return {'id': index + 1, 'workflow_id': 10 + index, 'run_number': 1, 'run_attempt': 1, 'check_suite_id': 300 + index,
            'path': '.github/workflows/' + workflow, 'event': 'merge_group', 'head_sha': HEAD,
            'head_repository': {'full_name': detector.REPO}, 'head_branch': 'gh-readonly-queue/main/test',
            'status': 'completed', 'conclusion': 'success', 'created_at': '2026-11-02T00:00:00Z'}


def check(index):
    return {'id': 100 + index, 'name': list(detector.WORKFLOW_OF)[index], 'head_sha': HEAD,
            'app': {'id': detector.APP}, 'status': 'completed', 'conclusion': 'success',
            'completed_at': '2026-11-01T23:00:00Z'}


class Response(io.BytesIO):
    def __init__(self, data, headers, status=200):
        super().__init__(json.dumps(data).encode())
        self.headers = headers
        self.status = status


class FakeApi(detector.Api):
    def __init__(self):
        self.now, self.elapsed = NOW, 0
        self.nodes = [entry()]
        self.runs = [run(i) for i in range(4)]
        self.index = {context: [check(i)] for i, context in enumerate(detector.WORKFLOW_OF)}
        self.checks = {100 + i: check(i) for i in range(4)}
        self.jobs = {i + 1: [{'id': 200 + i, 'name': context, 'status': 'completed',
                             'conclusion': 'success',
                             'check_run_url': 'https://api.github.com' + BASE + f'/check-runs/{100 + i}'}]
                     for i, context in enumerate(detector.WORKFLOW_OF)}
        self.rules = [{'type': 'required_status_checks', 'parameters': {'required_status_checks':
                       [{'context': name, 'integration_id': detector.APP} for name in detector.WORKFLOW_OF]}}]
        self.suite_checks, self.comments, self.posted = [], [], []
        self.paths, self.hook = [], None
        self.page_size, self.total_delta, self.foreign_link = 100, 0, False
        self.headers = {}
        self.queue_page_size = 100
        super().__init__('test-token-private-marker', opener=self.open, clock=lambda: self.elapsed,
                         pause=self.pause)

    def pause(self, seconds):
        self.elapsed += seconds

    def open(self, request, timeout):
        parsed = urllib.parse.urlsplit(request.full_url)
        path, query = parsed.path, urllib.parse.parse_qs(parsed.query)
        self.paths.append((request.method, path, query))
        if self.hook:
            self.hook(request)
        headers = {'Date': email.utils.formatdate(self.now, usegmt=True),
                   'X-RateLimit-Remaining': '10000', **self.headers}
        if path == '/graphql':
            cursor = json.loads(request.data)['variables']['cursor']
            page = int(cursor or 0)
            start = page * self.queue_page_size
            nodes = self.nodes[start:start + self.queue_page_size]
            following = start + len(nodes) < len(self.nodes)
            return Response({'data': {'repository': {'mergeQueue': {'entries': {
                'totalCount': len(self.nodes), 'nodes': nodes,
                'pageInfo': {'hasNextPage': following, 'endCursor': str(page + 1) if following else None}}}}}}, headers)
        if path.endswith('/rules/branches/main'):
            rows, key = self.rules, None
        elif '/actions/workflows/' in path:
            return Response({'workflow_runs': [self.runs[1]]}, headers)
        elif path.endswith('/actions/runs'):
            rows, key = self.runs, 'workflow_runs'
        elif '/attempts/' in path:
            rows, key = self.jobs[int(path.split('/runs/')[1].split('/')[0])], 'jobs'
        elif '/check-suites/' in path:
            rows, key = self.suite_checks, 'check_runs'
        elif '/commits/' in path:
            rows, key = self.index[query['check_name'][0]], 'check_runs'
        elif '/check-runs/' in path:
            return Response(self.checks[int(path.rsplit('/', 1)[1])], headers)
        elif path.endswith('/issues/43/comments'):
            if request.method == 'POST':
                body = json.loads(request.data)['body']
                self.posted.append(body)
                comment = {'id': len(self.comments) + 1000, 'body': body,
                           'user': {'login': 'github-actions[bot]'}}
                self.comments.append(comment)
                return Response(comment, headers, status=201)
            rows, key = self.comments, None
        else:
            raise AssertionError(path)
        page = int(query.get('page', ['1'])[0])
        start = (page - 1) * self.page_size
        values = rows[start:start + self.page_size]
        if start + len(values) < len(rows):
            parameters = {name: values[0] for name, values in query.items()}
            parameters['page'] = str(page + 1)
            host = 'other.invalid' if self.foreign_link else 'api.github.com'
            headers['Link'] = f'<https://{host}{path}?{urllib.parse.urlencode(parameters)}>; rel="next"'
        body = values if key is None else {key: values, 'total_count': len(rows) + self.total_delta}
        return Response(body, headers)


class Evidence(unittest.TestCase):
    def setUp(self):
        self.api, self.state = FakeApi(), detector.empty_state()
        self.api.index['ci-report'] = []

    def tick(self, seconds=0):
        self.api.now = NOW + seconds
        self.api.calls, self.api.started = 0, self.api.elapsed
        tick = detector.observe(self.api, self.state)
        self.state['episodes'], due = detector.step(self.state['episodes'], tick)
        return tick, due

    def due(self):
        self.tick()
        self.tick(600)
        return self.tick(1200)[1]

    def test_three_clean_server_clock_samples_before_one_comment(self):
        self.assertFalse(self.tick()[1])
        self.assertFalse(self.tick(600)[1])
        self.assertFalse(self.tick(1199)[1])
        due = self.tick(1200)[1]
        self.assertEqual(len(due), 1)
        self.api.allow_comments = True
        detector.report(self.api, self.state, due, True)
        self.assertEqual(len(self.api.posted), 1)
        body = self.api.posted[0]
        for fragment in (PR_HEAD, HEAD, 'ci-report', 'KST', 'First seen', '-3600 seconds'):
            self.assertIn(fragment, body)
        self.assertNotIn('test-token-private-marker', body)
        self.assertFalse(self.tick(1800)[1])

    def test_gap_boundary_unknown_and_clock_regression_restart(self):
        self.tick()
        self.tick(1200)
        self.assertEqual(next(iter(self.state['episodes'].values()))['clean'], 2)
        self.tick(2401)
        record = next(iter(self.state['episodes'].values()))
        self.assertEqual((record['clean'], record['eligible_since'], record['first_seen']), (1, NOW + 2401, NOW))
        detector.reset(self.state['episodes'])
        self.tick(2500)
        self.assertEqual(next(iter(self.state['episodes'].values()))['clean'], 1)
        self.tick(2499)
        self.assertEqual(next(iter(self.state['episodes'].values()))['first_seen'], NOW + 2499)

    def test_present_stale_no_gate_and_unknown_are_distinct(self):
        for outcome in ('PRESENT', 'INDEX_STALE', 'NO_GATE_JOB', 'UNKNOWN'):
            with self.subTest(outcome=outcome):
                api = FakeApi()
                if outcome == 'INDEX_STALE':
                    api.index['ci-report'][0]['conclusion'] = 'failure'
                if outcome in ('NO_GATE_JOB', 'UNKNOWN'):
                    api.index['ci-report'] = []
                    api.jobs[1] = []
                if outcome == 'UNKNOWN':
                    api.suite_checks = [check(0)]
                tick = detector.observe(api, detector.empty_state())
                self.assertEqual(tick['results']['entry1', 'ci-report']['kind'], outcome)

    def test_index_different_id_success_suppresses_and_wrong_app_does_not(self):
        self.api.index['ci-report'] = [dict(check(0), id=999)]
        self.assertFalse(self.tick(1200)[1])
        self.assertFalse(self.state['episodes'])
        self.api.index['ci-report'] = [dict(check(0), app={'id': 999})]
        self.assertEqual(self.tick(1800)[0]['results']['entry1', 'ci-report']['kind'], 'MISSING_REQUIRED')
        self.api.index['ci-report'][0].pop('app')
        self.assertEqual(self.tick(2400)[0]['results']['entry1', 'ci-report']['kind'], 'UNKNOWN')

    def test_pagination_reaches_later_index_and_job_rows_and_queue_entries(self):
        self.api.page_size = 1
        self.api.queue_page_size = 1
        self.api.nodes.append(entry('entry2'))
        self.api.index['ci-report'] = [dict(check(0), id=900, name='different'), check(0)]
        tick, _ = self.tick()
        self.assertEqual(len(tick['entries']), 2)
        self.assertEqual(tick['results']['entry1', 'ci-report']['kind'], 'PRESENT')
        self.api.index['ci-report'] = []
        self.api.jobs[1].insert(0, dict(self.api.jobs[1][0], id=900, name='other'))
        self.assertEqual(self.tick(600)[0]['results']['entry1', 'ci-report']['kind'], 'MISSING_REQUIRED')

    def test_incomplete_or_foreign_pagination_is_unknown(self):
        self.api.total_delta = 1
        self.assertEqual(self.tick()[0]['results']['entry1', 'ci-report']['kind'], 'UNKNOWN')
        self.api.total_delta, self.api.page_size, self.api.foreign_link = 0, 1, True
        with self.assertRaises(detector.Unknown):
            detector.latest_runs(self.api, HEAD)

    def test_latest_attempt_and_run_number_override_broken_creation_timestamps(self):
        newer = dict(self.api.runs[0], id=10, run_number=2, run_attempt=2,
                     created_at='2026-11-01T22:00:00Z')
        self.api.runs.append(newer)
        self.api.jobs[10] = self.api.jobs[1]
        tick, _ = self.tick()
        self.assertEqual(tick['runs']['entry1']['ci.yml']['id'], 10)
        self.assertTrue(any('/runs/10/attempts/2/jobs' in path for _, path, _ in self.api.paths))
        newer['status'] = 'in_progress'
        self.assertFalse(self.tick(600)[0]['runs'])
        self.assertFalse(self.state['episodes'])

    def test_missing_or_foreign_or_failed_workflow_is_not_eligible(self):
        for change in ({'head_repository': {'full_name': 'someone/else'}}, {'conclusion': 'failure'},
                       {'event': 'pull_request'}, {'head_sha': 'c' * 40}):
            with self.subTest(change=change):
                api = FakeApi()
                api.runs[0].update(change)
                self.assertIsNone(detector.latest_runs(api, HEAD))
        self.api.runs.pop()
        self.assertFalse(self.tick()[0]['runs'])

    def test_recreated_workflows_and_ambiguous_counters_are_unknown(self):
        for variant in ({'workflow_id': 99}, {}):
            with self.subTest(variant=variant):
                api = FakeApi()
                api.runs.append(dict(api.runs[0], id=99, **variant))
                with self.assertRaises(detector.Unknown):
                    detector.latest_runs(api, HEAD)

    def test_episodes_change_with_entry_group_pr_head_check_or_attempt(self):
        for change in ('entry', 'head', 'pr_head', 'check', 'attempt'):
            with self.subTest(change=change):
                self.setUp()
                self.tick()
                self.tick(600)
                if change == 'entry':
                    self.api.nodes[0]['id'] = 'new-entry'
                elif change == 'head':
                    self.api.nodes[0]['headCommit']['oid'] = 'c' * 40
                elif change == 'pr_head':
                    self.api.nodes[0]['pullRequest']['headRefOid'] = 'c' * 40
                elif change == 'check':
                    self.api.jobs[1][0]['check_run_url'] = 'https://api.github.com' + BASE + '/check-runs/999'
                    self.api.checks[999] = dict(check(0), id=999)
                else:
                    self.api.runs[1]['run_attempt'] = 2
                self.assertFalse(self.tick(1200)[1])
                for record in self.state['episodes'].values():
                    self.assertEqual(record['clean'], 1)

    def test_unknown_context_resets_but_keeps_first_seen(self):
        self.tick()
        self.tick(600)
        original = copy.deepcopy(self.api.checks[100])
        self.api.checks[100]['status'] = 'in_progress'
        self.assertFalse(self.tick(1200)[1])
        record = next(iter(self.state['episodes'].values()))
        self.assertIsNone(record['eligible_since'])
        self.assertEqual(record['first_seen'], NOW)
        self.api.checks[100] = original
        self.tick(1800)
        self.assertEqual(next(iter(self.state['episodes'].values()))['clean'], 1)

    def test_stale_index_resets_age_without_losing_the_audit_first_seen(self):
        self.tick()
        self.tick(600)
        self.api.index['ci-report'] = [dict(check(0), status='in_progress')]
        self.assertFalse(self.tick(1200)[1])
        record = next(iter(self.state['episodes'].values()))
        self.assertEqual(record['first_seen'], NOW)
        self.assertIsNone(record['eligible_since'])
        self.api.index['ci-report'] = []
        self.assertFalse(self.tick(1800)[1])
        self.assertEqual(next(iter(self.state['episodes'].values()))['clean'], 1)

    def test_recheck_refuses_every_changed_predicate(self):
        for change in ('gone', 'state', 'head', 'pr_head', 'attempt', 'other_attempt', 'run_status',
                       'index_success', 'index_pending', 'direct', 'rules'):
            with self.subTest(change=change):
                self.setUp()
                due = self.due()
                if change == 'gone':
                    self.api.nodes = []
                elif change == 'state':
                    self.api.nodes[0]['state'] = 'MERGEABLE'
                elif change == 'head':
                    self.api.nodes[0]['headCommit']['oid'] = 'c' * 40
                elif change == 'pr_head':
                    self.api.nodes[0]['pullRequest']['headRefOid'] = 'c' * 40
                elif change in ('attempt', 'other_attempt'):
                    self.api.runs[int(change == 'other_attempt')]['run_attempt'] = 2
                elif change == 'run_status':
                    self.api.runs[0]['status'] = 'queued'
                elif change.startswith('index_'):
                    self.api.index['ci-report'] = [check(0)]
                    if change == 'index_pending':
                        self.api.index['ci-report'][0]['status'] = 'in_progress'
                elif change == 'rules':
                    self.api.rules[0]['parameters']['required_status_checks'].pop()
                else:
                    self.api.checks[100]['conclusion'] = 'failure'
                self.api.allow_comments = True
                try:
                    detector.report(self.api, self.state, due, True)
                except detector.Unknown:
                    pass
                self.assertFalse(self.api.posted)

    def test_dedupe_survives_cache_loss_and_old_comments_on_later_pages(self):
        due = self.due()
        self.api.allow_comments = True
        detector.report(self.api, self.state, due, True)
        self.assertEqual(len(self.api.posted), 1)
        self.state = detector.empty_state()
        self.api.page_size = 1
        self.api.comments.insert(0, {'id': 2, 'body': 'other', 'user': {'login': 'someone'}})
        self.api.now = NOW + 4 * 86400
        self.tick(4 * 86400)
        self.tick(4 * 86400 + 600)
        due = self.tick(4 * 86400 + 1200)[1]
        detector.report(self.api, self.state, due, True)
        self.assertEqual(len(self.api.posted), 1)
        self.assertFalse(any('since' in query for _, path, query in self.api.paths if path.endswith('/comments')))

    def test_untrusted_marker_does_not_suppress_and_two_contexts_share_one_comment(self):
        self.api.index['deny'] = []
        due = self.due()
        self.assertEqual(len(due), 2)
        self.api.comments = [{'id': 1, 'user': {'login': 'someone'},
                              'body': '<!-- coreav-queue-detector:v1:' + due[0] + ' -->'}]
        self.api.allow_comments = True
        detector.report(self.api, self.state, due, True)
        self.assertEqual(len(self.api.posted), 1)
        self.assertIn('`deny`', self.api.posted[0])

    def test_ambiguous_post_is_not_retried_and_reconciles_from_comments(self):
        due = self.due()
        self.api.allow_comments = True
        original = self.api.open

        def lost_response(request, timeout):
            response = original(request, timeout)
            if request.method == 'POST' and request.full_url.endswith('/comments'):
                raise urllib.error.URLError('response lost')
            return response

        self.api.opener = lost_response
        with self.assertRaises(detector.Unknown):
            detector.report(self.api, self.state, due, True)
        self.assertEqual(len(self.api.posted), 1)
        self.api.opener = original
        detector.report(self.api, self.state, due, True)
        self.assertEqual(len(self.api.posted), 1)

    def test_shadow_records_the_same_alert_without_posting(self):
        due = self.due()
        messages = detector.report(self.api, self.state, due, False)
        self.assertIn('Shadow: would publish', messages[0])
        self.assertFalse(self.api.posted)

    def test_shadow_episode_is_rechecked_once_but_can_still_publish_live(self):
        due = self.due()
        self.assertEqual(len(detector.report(self.api, self.state, due, False)), 1)
        self.assertEqual(len(self.state['shadow']['would_alert']), 1)
        self.state['shadow']['would_alert'] = [{'date': NOW, 'keys': []}] * 20
        due = self.tick(1800)[1]
        calls = self.api.calls
        self.assertEqual(detector.report(self.api, self.state, due, False), [])
        self.assertEqual(self.api.calls, calls)
        self.assertIsNone(self.state['episodes'][due[0]]['alerted_at'])
        self.api.allow_comments = True
        detector.report(self.api, self.state, due, True)
        self.assertEqual(len(self.api.posted), 1)

    def test_a_later_group_failure_preserves_the_completed_shadow_summary(self):
        self.api.nodes.append(entry('entry2'))
        due = self.due()
        summaries = []

        def fail_later(request):
            if self.state['shadow']['would_alert'] and request.full_url.endswith('/graphql'):
                raise urllib.error.URLError('late read failure')

        self.api.hook = fail_later
        with self.assertRaises(detector.Unknown):
            detector.report(self.api, self.state, due, False, summaries=summaries)
        self.assertEqual(len(summaries), 1)
        self.assertIn('Shadow: would publish', summaries[0])
        self.assertEqual(len(self.state['shadow']['would_alert']), 1)
        self.assertFalse(self.api.posted)

    def test_rules_cache_refreshes_hourly_and_before_a_post(self):
        self.tick()
        self.tick(600)
        self.assertEqual(sum(path.endswith('/rules/branches/main') for _, path, _ in self.api.paths), 1)
        self.tick(3600)
        self.assertEqual(sum(path.endswith('/rules/branches/main') for _, path, _ in self.api.paths), 2)
        self.api.rules[0]['parameters']['required_status_checks'][0]['integration_id'] = None
        with self.assertRaises(detector.ConfigError):
            detector.required(self.api, self.state, NOW + 3600, refresh=True)

    def test_probe_reads_every_endpoint_even_with_empty_queue(self):
        self.api.nodes = []
        text = detector.probe(self.api)
        for fragment in ('queue', 'rules', 'runs', 'attempt jobs', 'direct check', 'index', 'issue comments'):
            self.assertIn(fragment, text)
        self.assertTrue(any('/attempts/1/jobs' in path for _, path, _ in self.api.paths))
        self.assertFalse(self.api.posted)


class Transport(unittest.TestCase):
    def test_partial_read_status_cannot_supply_index_evidence(self):
        api = FakeApi()
        api.opener = lambda *args, **kwargs: Response({'total_count': 0, 'check_runs': []}, {}, status=206)
        with self.assertRaises(detector.Unknown):
            detector.index_rows(api, HEAD, 'ci-report')

    def test_content_length_premature_eof_cannot_become_an_empty_index(self):
        payload = b'{"total_count":0,"check_runs":[]}'

        class Socket:
            def makefile(self, *args, **kwargs):
                header = (f'HTTP/1.1 200 OK\r\nDate: {email.utils.formatdate(NOW, usegmt=True)}\r\n'
                          f'Content-Length: {len(payload) + 1}\r\n\r\n').encode()
                return io.BytesIO(header + payload)

        def opened(*args, **kwargs):
            response = http.client.HTTPResponse(Socket())
            response.begin()
            return response

        api = detector.Api('fake', opener=opened, pause=lambda seconds: None)
        with self.assertRaises(detector.Unknown):
            detector.index_rows(api, HEAD, 'ci-report')
        self.assertEqual(api.calls, 3)

    def test_partial_http_responses_retry_then_become_unknown(self):
        api, calls = FakeApi(), []

        class Partial(Response):
            def read1(self, count):
                raise http.client.IncompleteRead(b'private-transfer-marker', 1)

        def partial(request, timeout):
            calls.append(request.full_url)
            return Partial({}, {'Date': email.utils.formatdate(NOW, usegmt=True)})

        api.opener = partial
        with self.assertRaises(detector.Unknown):
            detector.queue(api)
        self.assertEqual(len(calls), 3)

    def test_secondary_limit_without_retry_headers_is_unknown_not_configuration(self):
        api = FakeApi()

        def limited(request, timeout):
            raise urllib.error.HTTPError(request.full_url, 403, 'Forbidden', {},
                io.BytesIO(b'{"message":"You have exceeded a secondary rate limit."}'))

        api.opener = limited
        try:
            detector.queue(api)
        except detector.Unknown as error:
            self.assertNotIsInstance(error, detector.ConfigError)
        else:
            self.fail('Secondary limit was not refused')
        self.assertEqual(api.calls, 1)

    def test_budgets_rate_date_and_graphql_errors_fail_closed(self):
        for variant in ('calls', 'wall', 'quota', 'date', 'graphql'):
            with self.subTest(variant=variant):
                api = FakeApi()
                if variant == 'calls':
                    api.calls = detector.CALL_CAP
                elif variant == 'wall':
                    api.elapsed = detector.WALL
                elif variant == 'quota':
                    api.headers['X-RateLimit-Remaining'] = '149'
                elif variant == 'date':
                    api.headers['Date'] = 'not a date'
                else:
                    api.opener = lambda *args, **kwargs: Response({'errors': [{'message': 'no'}]},
                        {'Date': email.utils.formatdate(NOW, usegmt=True)})
                with self.assertRaises(detector.Unknown):
                    detector.queue(api)
                self.assertFalse(api.posted)

    def test_timeouts_and_server_errors_retry_twice_but_not_permissions_or_limits(self):
        for code, retries, failure in ((500, 3, detector.Unknown), (403, 1, detector.ConfigError),
                                       (429, 1, detector.Unknown), (401, 1, detector.ConfigError),
                                       (None, 3, detector.Unknown)):
            with self.subTest(code=code):
                api, calls = FakeApi(), []

                def broken(request, timeout):
                    calls.append(timeout)
                    if code is None:
                        raise TimeoutError()
                    headers = {'Retry-After': '1'} if code == 429 else {}
                    raise urllib.error.HTTPError(request.full_url, code, 'error', headers, None)

                api.opener = broken
                with self.assertRaises(failure):
                    detector.queue(api)
                self.assertEqual(len(calls), retries)
                self.assertTrue(all(0 < timeout <= 10 for timeout in calls))
                self.assertLess(api.elapsed, detector.WALL)

    def test_redirect_handler_never_forwards_credentials(self):
        handler = detector.NoRedirect()
        self.assertIsNone(handler.redirect_request(None, None, 302, '', {}, 'https://other.invalid'))
        api = FakeApi()
        with self.assertRaises(detector.ConfigError):
            api.request('POST', BASE + '/actions/runs/1/rerun', {})
        with self.assertRaises(detector.ConfigError):
            api.request('PATCH', BASE + '/rulesets/1', {})
        with self.assertRaises(detector.ConfigError):
            api.request('POST', '/graphql', {'query': 'mutation { forbidden }'})
        self.assertFalse(api.paths)

    def test_only_created_comment_responses_acknowledge_a_post(self):
        api = FakeApi()
        api.allow_comments = True
        original = api.open

        def unacknowledged(request, timeout):
            response = original(request, timeout)
            response.status = 200
            return response

        api.opener = unacknowledged
        with self.assertRaisesRegex(detector.Unknown, 'acknowledged'):
            api.post('test')
        self.assertEqual(len(api.posted), 1)

    def test_response_stream_cannot_extend_the_tick_by_dripping_bytes(self):
        api = FakeApi()

        class Dripping(Response):
            def read1(self, count):
                api.elapsed += 1
                return b' '

        api.opener = lambda *args, **kwargs: Dripping({}, {'Date': email.utils.formatdate(NOW, usegmt=True)})
        with self.assertRaisesRegex(detector.Unknown, 'budget'):
            detector.queue(api)
        self.assertEqual(api.elapsed, detector.WALL)

    def test_call_cap_is_global_including_retries(self):
        api = FakeApi()
        api.calls = detector.CALL_CAP - 1
        calls = []

        def unavailable(request, timeout):
            calls.append(request.full_url)
            raise TimeoutError()

        api.opener = unavailable
        with self.assertRaises(detector.Unknown):
            detector.queue(api)
        self.assertEqual(len(calls), 1)
        self.assertEqual(api.calls, detector.CALL_CAP)


class PersistenceAndWorkflow(unittest.TestCase):
    def test_a_cancelled_recheck_does_not_enter_shadow_alert_history(self):
        api, state = FakeApi(), detector.empty_state()
        api.index['ci-report'] = []
        for seconds in (0, 600):
            api.now = NOW + seconds
            tick = detector.observe(api, state)
            state['episodes'], _ = detector.step(state['episodes'], tick)
        graphs = []

        def leave_queue(request):
            if request.full_url.endswith('/graphql'):
                graphs.append(request.full_url)
                if len(graphs) == 2:
                    api.nodes = []

        api.hook = leave_queue
        api.now = NOW + 1200
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'state.json'
            detector.save_state(path, state)
            env = {'GH_TOKEN': 'test', 'GITHUB_REPOSITORY': detector.REPO,
                   'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'schedule'}
            with patch.dict(os.environ, env, clear=True), patch.object(detector, 'Api', return_value=api), \
                    patch('sys.argv', ['queue_detector', '--state', str(path)]), \
                    patch('sys.stdout', new_callable=io.StringIO):
                self.assertEqual(detector.main(), 0)
            saved = detector.load_state(path)
            self.assertEqual(saved['shadow']['would_alert'], [])
            self.assertFalse(api.posted)

    def test_premature_eof_resets_an_existing_observation_streak(self):
        api, state = FakeApi(), detector.empty_state()
        api.index['ci-report'] = []
        for seconds in (0, 600):
            api.now = NOW + seconds
            tick = detector.observe(api, state)
            state['episodes'], _ = detector.step(state['episodes'], tick)
        original = api.open

        def short_response(request, timeout):
            response = original(request, timeout)
            response.headers['Content-Length'] = str(len(response.getvalue()) + 1)
            return response

        api.opener = short_response
        api.now = NOW + 1200
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'state.json'
            detector.save_state(path, state)
            env = {'GH_TOKEN': 'test', 'GITHUB_REPOSITORY': detector.REPO,
                   'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'schedule'}
            with patch.dict(os.environ, env, clear=True), patch.object(detector, 'Api', return_value=api), \
                    patch('sys.argv', ['queue_detector', '--state', str(path)]), \
                    patch('sys.stdout', new_callable=io.StringIO):
                self.assertEqual(detector.main(), 0)
            record = next(iter(detector.load_state(path)['episodes'].values()))
            self.assertIsNone(record['eligible_since'])
            self.assertEqual(record['clean'], 0)
            self.assertFalse(api.posted)

    def test_null_api_fields_cannot_preserve_age_through_a_crashed_tick(self):
        api, state = FakeApi(), detector.empty_state()
        api.index['ci-report'] = []
        for seconds in (0, 600):
            api.now = NOW + seconds
            tick = detector.observe(api, state)
            state['episodes'], _ = detector.step(state['episodes'], tick)
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'state.json'
            detector.save_state(path, state)
            api.now = NOW + 1200
            api.runs[0]['path'] = None
            env = {'GH_TOKEN': 'test', 'GITHUB_REPOSITORY': detector.REPO,
                   'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'schedule'}
            with patch.dict(os.environ, env, clear=True), patch.object(detector, 'Api', return_value=api), \
                    patch('sys.argv', ['queue_detector', '--state', str(path)]), \
                    patch('sys.stdout', new_callable=io.StringIO):
                self.assertEqual(detector.main(), 0)
            record = next(iter(detector.load_state(path)['episodes'].values()))
            self.assertIsNone(record['eligible_since'])
            self.assertEqual(record['clean'], 0)
            self.assertEqual(record['first_seen'], NOW)
            self.assertFalse(api.posted)

    def test_corrupt_missing_wrong_version_or_wrong_repo_state_is_discarded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'state.json'
            self.assertEqual(detector.load_state(path), detector.empty_state())
            for value in ('{', json.dumps(dict(detector.empty_state(), schema_version=2)),
                          json.dumps(dict(detector.empty_state(), repo='other/repo')),
                          json.dumps(dict(detector.empty_state(), schema_version=True)),
                          json.dumps(dict(detector.empty_state(), rules={'fetched_at': 'broken'})), '[]'):
                path.write_text(value)
                self.assertEqual(detector.load_state(path), detector.empty_state())
            detector.save_state(path, detector.empty_state())
            self.assertEqual(detector.load_state(path), detector.empty_state())
            self.assertFalse(path.with_suffix('.tmp').exists())

    def test_main_saves_on_unknown_and_never_publishes_from_untrusted_events(self):
        for mode, event, ref in (('shadow', 'schedule', 'refs/heads/main'),
                                 ('live', 'pull_request', 'refs/heads/main'),
                                 ('notice', 'schedule', 'refs/heads/main'),
                                 ('probe', 'workflow_dispatch', 'refs/heads/topic')):
            with self.subTest(mode=mode, event=event, ref=ref), tempfile.TemporaryDirectory() as directory:
                path = pathlib.Path(directory) / 'state.json'
                api = FakeApi()
                api.headers['Date'] = 'bad'
                env = {'GH_TOKEN': 'test', 'GITHUB_REPOSITORY': detector.REPO,
                       'GITHUB_REF': ref, 'GITHUB_EVENT_NAME': event}
                with patch.dict(os.environ, env, clear=True), patch.object(detector, 'Api', return_value=api), \
                        patch('sys.argv', ['queue_detector', '--state', str(path), '--mode', mode]), \
                        patch('sys.stdout', new_callable=io.StringIO):
                    result = detector.main()
                self.assertEqual(result, int(mode != 'shadow'))
                self.assertFalse(api.posted)
                self.assertTrue(path.exists())

    def test_notice_posts_once_and_the_second_dispatch_proves_dedupe(self):
        api = FakeApi()
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'state.json'
            for _ in range(2):
                env = {'GH_TOKEN': 'test', 'GITHUB_REPOSITORY': detector.REPO,
                       'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'workflow_dispatch'}
                with patch.dict(os.environ, env, clear=True), patch.object(detector, 'Api', return_value=api), \
                        patch('sys.argv', ['queue_detector', '--state', str(path), '--mode', 'notice']), \
                        patch('sys.stdout', new_callable=io.StringIO):
                    self.assertEqual(detector.main(), 0)
            self.assertEqual(len(api.posted), 1)
            self.assertIn(detector.NOTICE, api.posted[0])

    def test_a_live_flag_without_the_permission_notice_cannot_publish(self):
        api = FakeApi()
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'state.json'
            env = {'GH_TOKEN': 'test', 'GITHUB_REPOSITORY': detector.REPO,
                   'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'schedule'}
            with patch.dict(os.environ, env, clear=True), patch.object(detector, 'Api', return_value=api), \
                    patch('sys.argv', ['queue_detector', '--state', str(path), '--mode', 'live']), \
                    patch('sys.stdout', new_callable=io.StringIO):
                self.assertEqual(detector.main(), 1)
            self.assertFalse(api.posted)

    def test_workflow_and_mapping_keep_the_monitor_separate_and_trusted(self):
        text = (ROOT / '.github/workflows/queue-detector.yml').read_text()
        self.assertIn('# workflow-check: not required', text)
        self.assertIn("cron: '7,17,27,37,47,57 * * * *'", text)
        self.assertIn('ref: main', text)
        self.assertIn('persist-credentials: false', text)
        self.assertNotIn('pull_request:', text)
        self.assertNotIn('pull_request_target:', text)
        self.assertNotIn('workflow_run:', text)
        self.assertIn("vars.QUEUE_DETECTOR_LIVE == 'true'", text)
        self.assertIn('cancel-in-progress: false', text)
        self.assertIn("if: always() && hashFiles('.queue-detector/state.json') != ''", text)
        spec = importlib.util.spec_from_file_location('workflow_check', ROOT / 'scripts/workflow_check.py')
        checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(checker)
        self.assertEqual(detector.WORKFLOW_OF, dict(checker.required_names(ROOT / 'docs/design/ci-checks.md')))


if __name__ == '__main__':
    unittest.main()
