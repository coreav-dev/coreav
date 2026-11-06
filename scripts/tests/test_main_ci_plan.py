"""Evaluate the workflow conditions for queue-tested main trees and fallback events."""
import itertools
import json
import math
import pathlib
import re
import shlex
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


def evaluate(expression, context):
    def value(match):
        item = context
        for key in match.group().split('.'):
            item = item.get(key, '') if isinstance(item, dict) else ''
        return repr(item)

    expression = expression.strip().removeprefix('${{').removesuffix('}}').strip()
    expression = re.sub(r'\b(?:github|steps|needs|strategy|inputs)\.[\w.*-]+', value, expression)
    expression = re.sub(r'!(?!=)', ' not ', expression.replace('&&', ' and ').replace('||', ' or '))
    return eval(expression, {'__builtins__': {}},
                {'always': lambda: True, 'contains': lambda values, entry: entry in values,
                 'startsWith': lambda value, prefix: value.startswith(prefix),
                 'true': True, 'false': False})


class MainPlan(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflows = {}
        for name in ('ci', 'ci-full', 'deny', 'python', 'npm'):
            text = subprocess.check_output(['ruby', '-rjson', '-rpsych', '-e',
                                            'puts JSON.generate(Psych.load_file(ARGV[0]))',
                                            str(ROOT / '.github/workflows' / (name + '.yml'))], text=True)
            cls.workflows[name] = json.loads(text)

    def concurrency(self, setting, workflow, run_id, event='push', ref='refs/heads/main', matrix_index=0):
        context = {'github': {'event_name': event, 'ref': ref, 'run_id': run_id, 'workflow': workflow},
                   'strategy': {'job-index': matrix_index}}
        pieces = re.split(r'(\$\{\{.*?\}\})', setting['group'])
        key = ''.join(str(evaluate(piece, context)) if piece.startswith('${{') else piece for piece in pieces)
        return key, bool(evaluate(setting['cancel-in-progress'], context))

    def test_aac_source_build_runs_the_whole_transcode_target(self):
        steps = self.workflows['ci']['jobs']['isobmff-decode']['steps']
        step, = [s for s in steps if s.get('name') == 'coreav transcode on the fixtures, with both encoders']
        commands = [shlex.split(line) for line in step['run'].replace('\\\n', '').splitlines()
                    if line.lstrip().startswith('cargo test')]
        aac, = [c for c in commands if '--features' in c and 'aac' in c[c.index('--features') + 1].split(',')]
        self.assertEqual(aac, [
            'cargo', 'test', '--locked', '--release', '-p', 'coreav-cli',
            '--features', 'ffmpeg-fallback,dav1d,libvpx,libopus,aac', '--test', 'transcode',
            '--target-dir', 'target/fallback',
        ], 'Run the complete transcode target with AAC enabled; remove test-name filters')

    def test_macos_runs_the_declared_sar_media_case_with_qualified_tools(self):
        steps = self.workflows['ci']['jobs']['check-macos']['steps']
        selected = [step for step in steps if step.get('name') == 'Declared sample aspect ratios on macOS']
        self.assertEqual(len(selected), 1,
                         'Add the declared-SAR media step to check-macos; workspace features run zero transcode tests')
        step = selected[0]
        self.assertEqual(step.get('if'), "env.COREAV_FFMPEG != '' && env.COREAV_FFPROBE != ''",
                         'Run the SAR media case when the registered oracle pair was provisioned')
        self.assertEqual(step['env'], {
            'COREAV_REQUIRE_FFMPEG': '1',
            'COREAV_X264': '${{ runner.temp }}/coreav-test-oracle/x264',
        })
        self.assertEqual([line.strip() for line in step['run'].splitlines() if line.strip()], [
            'scripts/install-x264.sh "$RUNNER_TEMP/coreav-test-oracle"',
            'cargo test --locked -p coreav-cli --features native-h264 --test transcode '
            'unscaled_encoding_preserves_declared_sar_without_changing_scale '
            '-- --exact --nocapture --color never 2>&1 | tee "$RUNNER_TEMP/declared-sar-macos.log"',
            "grep -F 'test unscaled_encoding_preserves_declared_sar_without_changing_scale ... ok' "
            '"$RUNNER_TEMP/declared-sar-macos.log"',
        ], 'Install pinned x264 and require the actual SAR test success line; zero tests must fail')
        provision, = [s for s in steps if s.get('name') == 'Provision the registered macOS oracle or select oracle-free tests']
        self.assertLess(steps.index(provision), steps.index(step))

    def test_macos_sar_success_witness_rejects_zero_tests(self):
        steps = self.workflows['ci']['jobs']['check-macos']['steps']
        step, = [s for s in steps if s.get('name') == 'Declared sample aspect ratios on macOS']
        witness = step['run'].splitlines()[-1]
        with tempfile.TemporaryDirectory() as directory:
            log = pathlib.Path(directory) / 'declared-sar-macos.log'
            for output, succeeds in [
                ('running 0 tests\ntest result: ok. 0 passed; 0 failed\n', False),
                ('test unscaled_encoding_preserves_declared_sar_without_changing_scale ... FAILED\n', False),
                ('test unscaled_encoding_preserves_declared_sar_without_changing_scale ... ok\n', True),
            ]:
                log.write_text(output)
                done = subprocess.run(['bash', '-c', witness], capture_output=True, text=True,
                                      env={'RUNNER_TEMP': directory, 'PATH': '/usr/bin:/bin'})
                self.assertEqual(done.returncode == 0, succeeds,
                                 'The macOS SAR witness must reject zero or failed tests')

    def test_macos_crop_runs_real_media_with_qualified_tools(self):
        steps = self.workflows['ci']['jobs']['check-macos']['steps']
        selected = [s for s in steps if s.get('name') == 'Literal crop frames on macOS']
        self.assertEqual(len(selected), 1, 'Run the literal-crop media case in check-macos')
        step = selected[0]
        self.assertEqual(step.get('if'), "env.COREAV_FFMPEG != '' && env.COREAV_FFPROBE != ''")
        self.assertEqual(step['env'], {
            'COREAV_REQUIRE_FFMPEG': '1',
            'COREAV_X264': '${{ runner.temp }}/coreav-test-oracle/x264',
        })
        self.assertEqual(step['run'].splitlines(), [
            'cargo test --locked -p coreav-cli --features native-h264 --test transcode '
            'ffmpeg_libx264_crop_matches_the_oracle -- --exact --nocapture --color never '
            '2>&1 | tee "$RUNNER_TEMP/literal-crop-macos.log"',
            "grep -F 'test ffmpeg_libx264_crop_matches_the_oracle ... ok' "
            '"$RUNNER_TEMP/literal-crop-macos.log"',
        ], 'Run the exact crop test and require its success witness, not a zero-test pass')
        install, = [s for s in steps if s.get('name') == 'Declared sample aspect ratios on macOS']
        self.assertIn('scripts/install-x264.sh', install['run'])
        self.assertLess(steps.index(install), steps.index(step))
        with tempfile.TemporaryDirectory() as directory:
            log = pathlib.Path(directory) / 'literal-crop-macos.log'
            for output, succeeds in [
                ('running 0 tests\ntest result: ok. 0 passed; 0 failed\n', False),
                ('test ffmpeg_libx264_crop_matches_the_oracle ... FAILED\n', False),
                ('test ffmpeg_libx264_crop_matches_the_oracle ... ok\n', True),
            ]:
                log.write_text(output)
                result = subprocess.run(['bash', '-c', step['run'].splitlines()[-1]],
                                        capture_output=True, text=True,
                                        env={'RUNNER_TEMP': directory, 'PATH': '/usr/bin:/bin'})
                self.assertEqual(result.returncode == 0, succeeds)

    def test_aac_qualification_report_survives_failure(self):
        workflow = json.loads(subprocess.check_output(
            ['ruby', '-rjson', '-rpsych', '-e', 'puts JSON.generate(Psych.load_file(ARGV[0]))',
             str(ROOT / '.github/workflows/audio-conformance.yml')], text=True))
        steps = workflow['jobs']['aac']['steps']
        upload, = [step for step in steps if step.get('name') == 'Keep the results']
        self.assertEqual(upload['if'], 'always()')
        self.assertIn('${{ runner.temp }}/aac-pcm.json', upload['with']['path'].splitlines(),
                      'Keep aac-pcm.json in the AAC artifact even when qualification fails')

    def test_aac_pns_controls_build_diagnostics_and_install_pinned_numpy(self):
        workflow = json.loads(subprocess.check_output(
            ['ruby', '-rjson', '-rpsych', '-e',
             'd = Psych.load_file(ARGV[0]); d["on"] ||= d.delete(true); puts JSON.generate(d)',
             str(ROOT / '.github/workflows/audio-conformance.yml')], text=True, timeout=10))
        for event in ('push', 'pull_request'):
            for path in ('scripts/aac_pns.py', 'scripts/tests/test_aac_pns.py',
                         'scripts/aac-pns-requirements.txt', 'docs/design/aac-pns-eligibility.md'):
                self.assertIn(path, workflow['on'][event]['paths'])
        steps = workflow['jobs']['aac']['steps']
        build, = [step for step in steps if step.get('name') == 'Build the command with the feature aac']
        self.assertIn('cargo build --locked --release -p coreav-aac --features diagnostics '
                      '--example pns_diagnostic', build['run'])
        install, = [step for step in steps if step.get('name') == 'Install the pinned PNS transform dependency']
        self.assertIn('python3 -m venv "$RUNNER_TEMP/aac-pns-python"', install['run'])
        self.assertIn('"$RUNNER_TEMP/aac-pns-python/bin/pip" install --quiet --only-binary=:all: '
                      '--require-hashes -r scripts/aac-pns-requirements.txt', install['run'])
        self.assertIn('echo "$RUNNER_TEMP/aac-pns-python/bin" >> "$GITHUB_PATH"', install['run'])
        controls, = [step for step in steps if step.get('name') == 'Check independent PNS transforms and native diagnostics']
        self.assertIn('python3 -I -m unittest discover -s scripts/tests -p test_aac_pns.py', controls['run'])
        self.assertIn('cargo test --locked --release -p coreav-aac --features diagnostics '
                      '--lib --example pns_diagnostic', controls['run'])
        self.assertEqual(controls['env']['OPENBLAS_NUM_THREADS'], '1')
        qualify, = [step for step in steps if step.get('name') == 'Qualify AAC decode-tolerance presentations']
        self.assertIn('--pns-diagnostic target/release/examples/pns_diagnostic', qualify['run'])
        self.assertLess(steps.index(build), steps.index(controls))
        self.assertLess(steps.index(install), steps.index(controls))
        self.assertLess(steps.index(controls), steps.index(qualify))

    def test_deny_discovery_uses_the_hash_pinned_numpy_interpreter(self):
        steps = self.workflows['deny']['jobs']['deny-checks']['steps']
        install, = [step for step in steps if 'python3 -m venv' in step.get('run', '')]
        self.assertIn('python3 -m venv "$RUNNER_TEMP/parser"', install['run'])
        self.assertIn('"$RUNNER_TEMP/parser/bin/pip" install --quiet --only-binary=:all: '
                      '--require-hashes -r scripts/aac-pns-requirements.txt', install['run'])
        self.assertIn('echo "$RUNNER_TEMP/parser/bin" >> "$GITHUB_PATH"', install['run'])
        discovery, = [step for step in steps if step.get('name') == 'Check the measurement analyzers']
        self.assertIn("python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'", discovery['run'])
        self.assertEqual(install['if'], discovery['if'])
        self.assertEqual(discovery['env']['OPENBLAS_NUM_THREADS'], '1')
        self.assertLess(steps.index(install), steps.index(discovery))

    def test_docs_push_never_drops_an_earlier_source_pushs_main_obligations(self):
        for old_reused, new_reused in ((True, True), (False, False), (True, False), (False, True)):
            old = self.plan('push', old_reused)
            new = self.plan('push', new_reused, code=False, h264=False)
            for name, workflow in self.workflows.items():
                with self.subTest(old_reused=old_reused, new_reused=new_reused, workflow=name):
                    retained = set(old[name])
                    if 'concurrency' in workflow:
                        old_key, _ = self.concurrency(workflow['concurrency'], name, '1')
                        new_key, cancel = self.concurrency(workflow['concurrency'], name, '2')
                        if cancel and old_key == new_key:
                            retained.clear()
                    for job in new[name]:
                        setting = workflow['jobs'][job].get('concurrency')
                        if setting:
                            old_key, _ = self.concurrency(setting, name, '1')
                            new_key, cancel = self.concurrency(setting, name, '2')
                            if cancel and old_key == new_key:
                                retained.discard(job)
                    covered = retained | set(new[name])
                    self.assertTrue(set(old[name]) <= covered,
                                    f'needed work dropped: {set(old[name]) - covered}')

    def test_main_work_survives_every_three_push_gate_ordering(self):
        """Reserve one of four runner slots for gates; children run or all wait."""
        events = [(kind, run) for run in range(3) for kind in ('push', 'gate')]
        orders = [order for order in itertools.permutations(events)
                  if all(order.index(('push', run)) < order.index(('gate', run)) for run in range(3))]
        self.assertEqual(len(orders), 90)
        failures = []
        checked = 0
        for evidence in itertools.product((False, True), repeat=3):
            plans = [self.plan('push', evidence[run], code=run != 1, h264=run != 1)
                     for run in range(3)]
            for name in ('ci', 'ci-full', 'python', 'npm'):
                workflow = self.workflows[name]
                self.assertNotIn('concurrency', workflow)
                gate = 'full-fixtures' if name == 'ci-full' else name + '-changes'
                selected = [[(job, index) for job, count in plan[name].items()
                             if not job.endswith('-report') for index in range(count)] for plan in plans]
                expected = {(run, job, index) for run in range(3) for job, index in selected[run]}
                for order in orders:
                    for blocked in (False, True):
                        checked += 1
                        running, pending, completed = {}, {}, set()
                        for kind, run in order:
                            if kind == 'push':
                                arriving = [(gate, 0)]
                            else:
                                key, _ = self.concurrency(workflow['jobs'][gate]['concurrency'], name, str(run))
                                token = pending.pop(key, None)
                                if token != (run, gate, 0):
                                    continue
                                completed.add(token)
                                arriving = [(job, index) for job, index in selected[run] if job != gate]
                            for job, index in arriving:
                                setting = workflow['jobs'][job]['concurrency']
                                key, cancel = self.concurrency(setting, name, str(run), matrix_index=index)
                                pending.pop(key, None)
                                if cancel:
                                    running.pop(key, None)
                                pending[key] = (run, job, index)
                                if job != gate and not blocked and key not in running and len(running) < 3:
                                    running[key] = pending.pop(key)
                        retained = completed | set(running.values()) | set(pending.values())
                        if not expected <= retained:
                            failures.append((name, evidence, order, blocked, expected - retained))
        self.assertEqual(checked, 5760)
        self.assertEqual(len(failures), 0, f'{len(failures)} lost-work orderings; first: {failures[:1]}')

    def plan(self, event, reused, code=True, h264=True, oracle_installer=False):
        plans = {}
        for name, workflow in self.workflows.items():
            if name == 'ci-full' and (event not in ('push', 'schedule', 'workflow_dispatch')
                                      or (event == 'push' and not code)):
                plans[name] = {}
                continue
            github = {'event_name': event, 'ref': 'refs/heads/main', 'run_id': '17', 'workflow': name,
                      'repository': 'owner/repo', 'event': {'pull_request': {'draft': False, 'labels': [],
                      'head': {'repo': {'full_name': 'owner/repo'}}}}}
            steps = {key: {'outputs': {'run': 'true' if selected else 'false'}}
                     for key, selected in (('filter', code), ('platforms', code),
                                            ('h264-conformance', h264), ('bench-guard', code),
                                            ('oracle-installer', oracle_installer))}
            steps['result'] = {'outputs': {'reuse': 'true' if reused else 'false'}}
            steps['fuzz-plan'] = {'outputs': {'plan': ''}}
            context = {'github': github, 'steps': steps, 'needs': {}}
            jobs = workflow['jobs']
            selected = {}
            for job, fields in jobs.items():
                needs = fields.get('needs', [])
                needs = [needs] if isinstance(needs, str) else needs
                condition = fields.get('if', 'true')
                dependencies = all(context['needs'][dependency]['result'] == 'success' for dependency in needs)
                run = (dependencies or 'always()' in condition) and evaluate(condition, context)
                result = {'result': 'success' if run else 'skipped', 'outputs': {}}
                if run:
                    for key, expression in fields.get('outputs', {}).items():
                        value = evaluate(expression, context)
                        result['outputs'][key] = str(value).lower() if isinstance(value, bool) else value
                    matrix = fields.get('strategy', {}).get('matrix', {})
                    count = math.prod(len(values) for key, values in matrix.items() if key not in ('include', 'exclude'))
                    selected[job] = count
                context['needs'][job] = result
            plans[name] = selected
        return plans

    def test_reused_main_keeps_only_main_maintenance_and_required_reports(self):
        plan = self.plan('push', True)
        self.assertEqual(set(plan['ci']), {'ci-changes', 'ci-report',
                                          'main-cache', 'bench-guard'})
        self.assertEqual(plan['ci-full'], {
            'full-fixtures': 1, 'h264-full': 1, 'conformance-arm64': 1,
            'transcode-compat-shard': 3, 'transcode-compat': 1, 'ci-full-report': 1,
        })
        self.assertEqual(set(plan['python']), {'python-changes', 'python-report'})
        self.assertEqual(set(plan['npm']), {'npm-changes', 'npm-report'})
        self.assertEqual(set(plan['deny']), {'deny', 'deny-checks'})
        self.assertEqual(sum(sum(jobs.values()) for jobs in plan.values()), 18)

    def test_installer_only_proof_is_reused_without_starting_the_h264_group(self):
        for event, reused in (('pull_request', False), ('merge_group', False),
                              ('push', False), ('push', True)):
            with self.subTest(event=event, reused=reused):
                ci = self.plan(event, reused, h264=False, oracle_installer=True)['ci']
                self.assertEqual('modes-identity' in ci, not reused)
                for job in ('conformance', 'conformance-arm64', 'training-identity',
                            'kernels-arm64', 'vector-width'):
                    self.assertNotIn(job, ci)
                if reused:
                    self.assertEqual(set(ci), {'ci-changes', 'ci-report', 'bench-guard', 'main-cache'})

    def test_unproven_main_and_merge_groups_do_not_skip_the_suite(self):
        main = self.plan('push', False)
        group = self.plan('merge_group', False)
        for plan in (main, group):
            self.assertIn('check', plan['ci'])
            self.assertIn('isobmff-writers', plan['ci'])
            self.assertIn('isobmff-command-compat', plan['ci'])
            self.assertNotIn('transcode-compat-shard', plan['ci'])
            self.assertIn('svt-av1', plan['ci'])
            self.assertIn('linux-x86_64', plan['python'])
            self.assertIn('package', plan['npm'])
            self.assertNotIn('main-cache', plan['ci'])
        self.assertEqual(sum(sum(jobs.values()) for jobs in main.values()), 39)
        self.assertEqual(group['ci-full'], {})
        self.assertNotIn('conformance-arm64', group['ci'])

    def test_documentation_only_main_keeps_the_eight_lightweight_jobs(self):
        plan = self.plan('push', True, code=False, h264=False)
        self.assertEqual(sum(sum(jobs.values()) for jobs in plan.values()), 8)

    def test_representative_non_h264_main_retains_independent_full_work(self):
        counts = [sum(sum(jobs.values()) for jobs in self.plan('push', reused, h264=False).values())
                  for reused in (False, True)]
        self.assertEqual(counts, [34, 18])

    def test_concurrency_never_cancels_or_joins_merge_groups(self):
        for name, workflow in self.workflows.items():
            settings = [('workflow', workflow['concurrency'])] if 'concurrency' in workflow else [
                (job, fields['concurrency']) for job, fields in workflow['jobs'].items()
                if 'concurrency' in fields]
            for job, setting in settings:
                with self.subTest(workflow=name, job=job):
                    unfiltered = name == 'deny'
                    keys = []
                    for event, ref, run_id in (('merge_group', 'refs/heads/gh-readonly-queue/main/group', '1'),
                                               ('merge_group', 'refs/heads/gh-readonly-queue/main/group', '2'),
                                               ('pull_request', 'refs/pull/1/merge', '3'),
                                               ('push', 'refs/heads/main', '4')):
                        key, cancel = self.concurrency(setting, name, run_id, event, ref)
                        keys.append(key)
                        self.assertEqual(cancel, event == 'pull_request' or (event == 'push' and unfiltered))
                    self.assertEqual(len(set(keys)), len(keys))
                    for event in ('push', 'pull_request'):
                        pair = [self.concurrency(setting, name, run_id, event)[0] for run_id in ('5', '6')]
                        self.assertEqual(pair[0] == pair[1], event == 'pull_request' or unfiltered)
                    if 'strategy' in workflow['jobs'].get(job, {}):
                        pair = [self.concurrency(setting, name, '5', matrix_index=index)[0] for index in (0, 1)]
                        self.assertNotEqual(*pair)
