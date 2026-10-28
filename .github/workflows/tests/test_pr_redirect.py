""".github/workflows/pr-redirect.yml: who is redirected, and what the issue quotes.

Runs the workflow's own script, read out of the YAML, under Node with a
stand-in for the GitHub API (`github`, `context`, `core`, as
actions/github-script passes them), and checks what it would do. Needs
`node` on PATH; skipped without it, except in CI, where that is a failure.
"""
import json
import os
import pathlib
import shutil
import subprocess
import unittest

WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / 'pr-redirect.yml'
OLD = '2020-01-01T00:00:00Z'

# The stand-in: answers the calls the script makes from the scenario on
# stdin, and prints what the script did.
HARNESS = r"""
const input = JSON.parse(require('fs').readFileSync(0, 'utf8'));
const sc = input.scenario;
const did = { closed: [], issues: [], comments: [], labels: [], info: [], warnings: [], failed: null };
const fail = (status) => Object.assign(new Error(`HTTP ${status}`), { status });
const ok = (data) => ({ data });
const github = {
  rest: {
    repos: {
      async getCollaboratorPermissionLevel({ username }) {
        const r = (sc.roles || {})[username];
        if (r === undefined) throw fail(404);
        if (typeof r === 'number') throw fail(r);
        if (typeof r === 'object') return ok(r);
        const permission = { admin: 'admin', maintain: 'write', write: 'write', triage: 'read', read: 'read' }[r];
        return ok({ role_name: r, permission });
      },
    },
    issues: {
      async getLabel() { if (!sc.labelExists) throw fail(404); return ok({}); },
      async createLabel(p) { did.labels.push(p.name); return ok({}); },
      async listForRepo() { return ok(sc.issues || []); },
      async create(p) { did.issues.push({ title: p.title, body: p.body }); return ok({ number: 1000 + did.issues.length }); },
      async listComments({ issue_number }) { return ok((sc.comments || {})[issue_number] || []); },
      async createComment(p) { did.comments.push(p.issue_number); return ok({}); },
    },
    pulls: {
      async list() { return ok(sc.prs); },
      async update(p) { if (p.state === 'closed') did.closed.push(p.pull_number); return ok({}); },
    },
    reactions: {
      async listForIssue({ issue_number }) {
        const r = (sc.reactions || {})[issue_number];
        if (typeof r === 'number') throw fail(r);
        return ok(r || []);
      },
    },
  },
  async paginate(fn, params) { return (await fn(params)).data; },
};
const core = {
  info: (m) => did.info.push(m),
  warning: (m) => did.warnings.push(m),
  setFailed: (m) => { did.failed = m; },
};
const context = { repo: { owner: 'o', repo: 'r' } };
if (input.dryRun === null) delete process.env.DRY_RUN;
else process.env.DRY_RUN = input.dryRun;
const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor;
new AsyncFunction('github', 'context', 'core', input.script)(github, context, core)
  .then(() => process.stdout.write(JSON.stringify(did)))
  .catch((e) => { process.stderr.write(String(e && e.stack || e)); process.exit(1); });
"""


def script():
    """The `script: |` block of the workflow's github-script step."""
    lines = WORKFLOW.read_text(encoding='utf-8').split('\n')
    start = next(i for i, line in enumerate(lines) if line.strip() == 'script: |')
    indent = len(lines[start]) - len(lines[start].lstrip()) + 2
    body = []
    for line in lines[start + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) < indent:
            break
        body.append(line[indent:])
    return '\n'.join(body)


def pr(number, login='outsider', association='CONTRIBUTOR', created=OLD, body='', kind='User'):
    return {'number': number, 'title': f'Fix {number}', 'body': body, 'created_at': created,
            'author_association': association, 'user': {'login': login, 'type': kind}}


NODE = shutil.which('node')


@unittest.skipUnless(NODE or os.environ.get('CI'), 'node is not on PATH')
class Redirect(unittest.TestCase):
    def run_script(self, prs, dry_run='false', **scenario):
        scenario['prs'] = prs
        payload = json.dumps({'script': script(), 'scenario': scenario, 'dryRun': dry_run})
        done = subprocess.run([NODE or 'node', '-e', HARNESS], input=payload, capture_output=True,
                              text=True, check=False, timeout=60)
        self.assertEqual(done.returncode, 0, done.stderr)
        return json.loads(done.stdout)

    def test_an_outside_pull_request_is_redirected(self):
        did = self.run_script([pr(7)])
        self.assertEqual(did['closed'], [7])
        self.assertEqual(len(did['issues']), 1)
        self.assertEqual(did['comments'], [7])
        self.assertIsNone(did['failed'])

    def test_a_dry_run_changes_nothing(self):
        # DRY_RUN unset, or anything but `false`, is a dry run too.
        for dry_run in ('true', None, '', 'False'):
            did = self.run_script([pr(7)], dry_run=dry_run)
            self.assertEqual((did['closed'], did['issues'], did['comments'], did['labels']),
                             ([], [], [], []), dry_run)
            self.assertIn('#7: would redirect', did['info'])

    def test_the_workflow_is_live_only_when_asked(self):
        # GitHub evaluates the expression; this pins its form, which is a
        # dry run for every event but the two explicit live cases.
        text = WORKFLOW.read_text(encoding='utf-8')
        self.assertIn("DRY_RUN: ${{ !((github.event_name == 'schedule' && vars.PR_REDIRECT_LIVE == 'true') || "
                      "(github.event_name == 'workflow_dispatch' && inputs.dry-run == false)) }}", text)

    def test_a_rerun_opens_no_second_issue_or_comment(self):
        bot = {'login': 'github-actions[bot]'}
        marker = '<!-- coreav-pr-redirect #7 -->'
        issues = [{'number': 55, 'body': marker, 'user': bot}]
        did = self.run_script([pr(7)], issues=issues, comments={'7': [{'body': marker, 'user': bot}]})
        self.assertEqual((did['issues'], did['comments'], did['closed']), ([], [], [7]))
        # The issue exists but the marker comment is someone else's: the
        # comment is made.
        did = self.run_script([pr(7)], issues=issues,
                              comments={'7': [{'body': marker, 'user': {'login': 'someone'}}]})
        self.assertEqual((did['issues'], did['comments'], did['closed']), ([], [7], [7]))

    def test_a_marker_from_anyone_but_the_bot_is_ignored(self):
        marker = '<!-- coreav-pr-redirect #7 -->'
        did = self.run_script([pr(7)], issues=[{'number': 55, 'body': marker, 'user': {'login': 'someone'}}])
        self.assertEqual(len(did['issues']), 1)
        self.assertEqual(did['closed'], [7])

    def test_anyone_with_write_access_is_trusted(self):
        # An organization member whose membership is private shows as a
        # contributor; the repository's permission says otherwise.
        for role in ('write', 'maintain', 'admin'):
            did = self.run_script([pr(7, login='member')], dry_run='true', roles={'member': role})
            self.assertFalse([m for m in did['info'] if m.startswith('#7')], role)
        # A custom role is judged by the permission it is based on.
        did = self.run_script([pr(7, login='custom')], dry_run='true',
                              roles={'custom': {'role_name': 'releaser', 'permission': 'write'}})
        self.assertFalse([m for m in did['info'] if m.startswith('#7')])
        for role in ('read', 'triage', {'role_name': 'auditor', 'permission': 'read'}):
            did = self.run_script([pr(7, login='reader')], roles={'reader': role})
            self.assertEqual(did['closed'], [7], role)

    def test_a_trusted_association_needs_no_lookup(self):
        did = self.run_script([pr(7, association='MEMBER')], roles={'outsider': 500})
        self.assertEqual(did['closed'], [])
        self.assertIsNone(did['failed'])

    def test_a_failed_lookup_of_the_author_leaves_it_open(self):
        did = self.run_script([pr(7), pr(8, login='other')], roles={'outsider': 502})
        self.assertNotIn(7, did['closed'])
        self.assertEqual(did['closed'], [8])
        self.assertIn('#7', did['failed'])

    def test_a_maintainers_eyes_keep_it_open(self):
        eyes = [{'content': 'eyes', 'user': {'login': 'boss'}}]
        did = self.run_script([pr(7)], roles={'boss': 'maintain'}, reactions={'7': eyes})
        self.assertEqual(did['closed'], [])
        self.assertIn('#7: kept open by a maintainer', did['info'])
        # Eyes from someone with write access only do not.
        did = self.run_script([pr(7)], roles={'dev': 'write'},
                              reactions={'7': [{'content': 'eyes', 'user': {'login': 'dev'}}]})
        self.assertEqual(did['closed'], [7])

    def test_a_failed_lookup_of_a_reaction_leaves_it_open(self):
        eyes = [{'content': 'eyes', 'user': {'login': 'boss'}}]
        for scenario in ({'roles': {'boss': 503}, 'reactions': {'7': eyes}}, {'reactions': {'7': 500}}):
            did = self.run_script([pr(7)], **scenario)
            self.assertEqual(did['closed'], [], scenario)
            self.assertIn('#7', did['failed'])

    def test_young_and_bot_pull_requests_wait(self):
        young = pr(7, created='2999-01-01T00:00:00Z')
        did = self.run_script([young, pr(8, login='dependabot[bot]', kind='Bot')])
        self.assertEqual(did['closed'], [])
        self.assertIn('#7: younger than three hours, next run', did['info'])


@unittest.skipUnless(NODE or os.environ.get('CI'), 'node is not on PATH')
class Quoted(unittest.TestCase):
    def quote(self, body):
        did = Redirect.run_script(self, [pr(7, body=body)])
        issue = did['issues'][0]['body']
        quoted = issue.split('Its description, without code:\n\n', 1)[1]
        return '\n'.join(line[2:] if line.startswith('> ') else line[1:] for line in quoted.rstrip('\n').split('\n'))

    def assertOmitted(self, body, secret, kept=()):
        got = self.quote(body)
        self.assertNotIn(secret, got)
        self.assertIn('[code omitted]', got)
        for text in kept:
            self.assertIn(text, got)

    def test_fenced_blocks(self):
        self.assertOmitted('Before\n```rust\nfn secret() {}\n```\nAfter', 'secret', ['Before', 'After'])
        self.assertOmitted('Before\n~~~\nsecret\n~~~\nAfter', 'secret', ['After'])

    def test_a_fence_under_a_list_item(self):
        self.assertOmitted('- step\n\n   ```\n   secret()\n   ```\n- next', 'secret', ['step', 'next'])

    def test_a_longer_fence_holding_a_shorter_one(self):
        self.assertOmitted('````\n```\nsecret_a\n```\nsecret_b\n````\nAfter', 'secret', ['After'])

    def test_an_unclosed_fence_runs_to_the_end(self):
        self.assertOmitted('Before\n```\nsecret\nmore', 'secret', ['Before'])

    def test_an_indented_block_at_the_start(self):
        self.assertOmitted('    fn a() {\n        secret();\n    }\n\nAfter', 'secret', ['After'])
        self.assertOmitted('\n\n    secret_first()\nAfter', 'secret', ['After'])

    def test_an_indented_block_after_a_heading_or_a_rule(self):
        self.assertOmitted('## Patch\n    secret()\nAfter', 'secret', ['Patch', 'After'])
        self.assertOmitted('Text\n\n---\n    secret()\n', 'secret', ['Text'])

    def test_a_fence_in_a_blockquote(self):
        self.assertOmitted('> quoted\n> ```\n> secret()\n> ```\nAfter', 'secret', ['quoted', 'After'])
        self.assertOmitted('> quoted\n>\n>     secret()\n\nAfter', 'secret', ['quoted', 'After'])

    def test_a_fence_on_a_list_marker_line(self):
        self.assertOmitted('- ```rust\n  secret()\n  ```\n- after', 'secret', ['after'])
        self.assertOmitted('1. ```\n   secret()\n   ```\n2. after', 'secret', ['after'])

    def test_multi_line_and_multi_backtick_inline_spans(self):
        self.assertOmitted('Call `secret(\n  x)` here', 'secret', ['Call', 'here'])
        self.assertOmitted('Call ``a ` secret_long_enough_to_be_removed_here()`` now', 'secret', ['Call', 'now'])
        got = self.quote('Run ``coreav probe`` and `a`\n\n`b` too')
        self.assertIn('``coreav probe``', got)
        self.assertIn('`a`', got)
        self.assertIn('`b` too', got)

    def test_indented_blocks(self):
        body = 'Before\n\n    let secret = 1;\n\n    secret_two();\nAfter\n\n\tsecret_tab\n'
        self.assertOmitted(body, 'secret', ['Before', 'After'])

    def test_an_indented_continuation_line_is_prose(self):
        got = self.quote('A paragraph\n    that goes on')
        self.assertIn('that goes on', got)

    def test_html_code(self):
        self.assertOmitted('Use <pre class="x">\nsecret\n</pre> then', 'secret', ['Use', 'then'])
        self.assertOmitted('Call <code>secret()</code> here', 'secret', ['Call', 'here'])
        self.assertOmitted('<details><summary>Patch</summary><PRE>secret</PRE></details>', 'secret', ['Patch'])
        self.assertOmitted('Open <pre>\nsecret to the end', 'secret', ['Open'])

    def test_long_inline_code_goes_and_short_stays(self):
        got = self.quote('Run `coreav probe` or `' + 'x' * 50 + '`')
        self.assertIn('`coreav probe`', got)
        self.assertNotIn('x' * 50, got)

    def test_mentions_references_and_comments_are_defused(self):
        got = self.quote('cc @someone, see #12 <!-- hidden -->')
        self.assertNotIn('@someone', got)
        self.assertNotIn('#12', got)
        self.assertNotIn('<!--', got)


if __name__ == '__main__':
    unittest.main()
