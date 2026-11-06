"""Exercise source planning and artifact custody without media or network access."""
import copy
import ast
import fnmatch
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import compat_sources as sources
import registered_candidate


def synthetic_commit(git, tree, parent):
    ancestry = f'parent {parent}\n' if parent else ''
    raw = (f'tree {tree}\n{ancestry}author Fixture <fixture@example.invalid> 0 +0000\n'
           'committer Fixture <fixture@example.invalid> 0 +0000\n\nSynthetic source fixture\n')
    return git('hash-object', '-t', 'commit', '-w', '--stdin', input=raw)


class SourceRepo:
    def __init__(self, root):
        self.root = root
        self.git('init', '-q')
        self.git('symbolic-ref', 'HEAD', 'refs/heads/main')
        for path in sources.FROZEN:
            target = path + '/mod.rs' if path == 'crates/coreav-isobmff/tests/common' else path
            self.write(target, path + '\n')
        for path in sources.JUDGING:
            self.write(path, path + '\n')
        for name in ('compat_sources.py', 'registered_candidate.py'):
            self.write('scripts/' + name, (ROOT / 'scripts' / name).read_text())
        self.write(sources.README, '')
        self.write('.gitignore', '/target/\n')
        self.candidate = self.save()
        self.write('deadline', 'floor\n')
        self.floor = self.save()
        self.registration = f'Release candidate: `{self.candidate}`.\nTooling floor: `{self.floor}`.\n'
        self.write(sources.README, self.registration)
        registered = self.save()
        self.write(sources.JUDGING_REGISTRATION, sources.register(root, registered))
        self.tooling = self.save()
        self.git('update-ref', 'refs/remotes/origin/main', self.tooling)

    def git(self, *args, input=None):
        return subprocess.check_output(['git', *args], cwd=self.root, input=input, text=True,
                                       stderr=subprocess.PIPE).strip()

    def write(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def save(self, parent=None):
        self.git('add', '.')
        tree = self.git('write-tree')
        if parent is None:
            done = subprocess.run(['git', 'rev-parse', '--verify', 'HEAD'], cwd=self.root,
                                  capture_output=True, text=True, check=False)
            parent = done.stdout.strip() if done.returncode == 0 else ''
        commit = synthetic_commit(self.git, tree, parent)
        self.git('update-ref', 'refs/heads/main', commit)
        return commit


class Sources(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / 'tooling'
        self.root.mkdir()
        self.repo = SourceRepo(self.root)
        self.run = {'id': '123', 'attempt': '1', 'event': 'workflow_dispatch',
                    'ref': 'refs/heads/main', 'set': 'rehearsal'}

    def plan(self, commit=None, tooling=None, **run):
        context = {**self.run, **run}
        return sources.plan(self.root, context['event'], context['ref'],
                            self.repo.candidate if commit is None else commit,
                            self.repo.tooling if tooling is None else tooling)

    def bundle(self):
        candidate = Path(self.tmp.name) / 'candidate'
        self.repo.git('worktree', 'add', '--detach', str(candidate), self.repo.candidate)
        dist = Path(self.tmp.name) / 'dist'
        dist.mkdir()
        (dist / 'coreav').write_bytes(b'candidate executable')
        (dist / 'r3-harness').write_bytes(b'observer executable')
        manifest = sources.record(self.root, candidate, dist, self.repo.candidate,
                                  self.repo.tooling, self.run, 'rustc fixture')
        return candidate, dist, manifest

    def verify(self, dist, **run):
        return sources.verify(self.root, dist, self.repo.candidate, self.repo.tooling, {**self.run, **run})

    def test_dispatch_uses_registered_rust_and_main_tooling(self):
        plan = self.plan()
        self.assertEqual(plan['commit'], self.repo.candidate)
        self.assertEqual(plan['tooling'], self.repo.tooling)
        self.assertEqual(plan['floor'], self.repo.floor)
        self.assertIs(plan['on_main'], True)

    def test_wrong_or_mutable_sources_never_emit_a_plan(self):
        for commit in ('', 'b' * 40, self.repo.candidate[:12], 'main', 'A' * 40):
            with self.subTest(commit=commit), self.assertRaises(ValueError):
                self.plan(commit=commit)
        for tooling in ('main', self.repo.tooling[:12], self.repo.candidate, 'b' * 40):
            with self.subTest(tooling=tooling), self.assertRaises(ValueError):
                self.plan(tooling=tooling)
        for event, ref in (('workflow_dispatch', 'refs/heads/other'), ('push', 'refs/heads/main')):
            with self.subTest(event=event, ref=ref), self.assertRaises(ValueError):
                self.plan(event=event, ref=ref)

    def test_floor_and_candidate_ancestry_fail_closed(self):
        self.repo.tooling = self.repo.save(parent=self.repo.candidate)
        self.repo.git('update-ref', 'refs/remotes/origin/main', self.repo.tooling)
        with self.assertRaisesRegex(ValueError, 'floor'):
            self.plan()
        self.repo.tooling = self.repo.save(parent=self.repo.floor)
        self.repo.write(sources.README, self.repo.registration.replace(self.repo.candidate, 'b' * 40))
        self.repo.tooling = self.repo.save()
        with self.assertRaises(ValueError):
            self.plan(commit='b' * 40)

    def test_non_main_tooling_is_only_a_spent_pr_rehearsal(self):
        self.repo.write('pr-tool', 'changed tooling\n')
        self.repo.tooling = self.repo.save()
        with self.assertRaisesRegex(ValueError, 'main'):
            self.plan()
        self.assertFalse(self.plan(event='pull_request', ref='refs/pull/1/merge')['on_main'])
        self.run.update(event='pull_request', ref='refs/pull/1/merge', set='draw6')
        with self.assertRaisesRegex(ValueError, 'spent'):
            self.bundle()

    def test_every_frozen_path_refuses_drift_or_absence(self):
        for path in sources.FROZEN:
            for missing in (False, True):
                with self.subTest(path=path, missing=missing):
                    self.repo.git('reset', '--hard', self.repo.tooling)
                    target = path + '/mod.rs' if path == 'crates/coreav-isobmff/tests/common' else path
                    if missing:
                        (self.root / target).unlink()
                    else:
                        self.repo.write(target, 'drift\n')
                    changed = self.repo.save()
                    if path in sources.OBSERVER:
                        planned = self.plan(tooling=changed, event='pull_request')
                        self.assertEqual(planned['frozen'][path], {
                            'owner': 'r3-harness',
                            'object': self.repo.git('rev-parse', f'{self.repo.candidate}:{path}'),
                        })
                    else:
                        with self.assertRaises(ValueError):
                            self.plan(tooling=changed, event='pull_request')

    def test_floor_registration_is_exact_unique_and_documented(self):
        for text in ('', self.repo.registration * 2, self.repo.registration.replace(self.repo.floor, 'main'),
                     self.repo.registration.replace(self.repo.floor, self.repo.floor.upper())):
            with self.subTest(text=text), self.assertRaises(ValueError):
                registered_candidate.tooling_floor(text)
        lines = (ROOT / sources.README).read_text().splitlines()
        example, = [line.strip() for line in lines if line.startswith('    Tooling floor:')]
        self.assertEqual(registered_candidate.tooling_floor(example.replace('<SHA>', self.repo.floor)), self.repo.floor)

    def test_recorded_sources_hashes_and_roles_survive_a_job_retry(self):
        candidate, dist, manifest = self.bundle()
        verified = self.verify(dist, attempt='2')
        self.assertEqual(verified, manifest)
        self.assertEqual(set(p.name for p in dist.iterdir()), {'coreav', 'r3-harness', 'sha256.txt', 'sources.json'})
        for name, role in (('coreav', 'candidate'), ('r3-harness', 'observer')):
            self.assertEqual(manifest['artifacts'][name]['role'], role)
            self.assertEqual(manifest['artifacts'][name]['built_from'], self.repo.candidate)
            self.assertEqual(manifest['artifacts'][name]['sha256'], sources.digest(dist / name))
        self.repo.git('-C', str(candidate), 'checkout', '--detach', self.repo.tooling)
        with self.assertRaises(ValueError):
            sources.record(self.root, candidate, dist, self.repo.candidate, self.repo.tooling, self.run, 'rustc')

    def test_stale_or_altered_manifest_is_refused(self):
        _, dist, original = self.bundle()
        mutations = [('schema',), ('candidate', 'commit'), ('candidate', 'registration'), ('tooling', 'commit'),
                     ('tooling', 'floor'), ('tooling', 'on_main'), ('run', 'id'), ('run', 'event'),
                     ('run', 'ref'), ('run', 'set'), ('frozen',), ('artifacts',), ('judging',),
                     ('judging', 'registration_sha256'), ('judging', 'files', sources.JUDGING[0])]
        mutations += [('artifacts', name, field) for name in sources.ARTIFACTS
                      for field in ('role', 'built_from', 'sha256')]
        mutations += [('frozen', path, field) for path in sources.FROZEN for field in ('owner', 'object')]
        mutations += [('judging', 'observer', 'commit'), ('judging', 'observer', 'sources')]
        for path in mutations:
            with self.subTest(path=path):
                changed = copy.deepcopy(original)
                value = changed
                for key in path[:-1]:
                    value = value[key]
                value[path[-1]] = {} if path[-1] in ('frozen', 'artifacts') else 'altered'
                (dist / 'sources.json').write_text(json.dumps(changed))
                with self.assertRaises(ValueError):
                    self.verify(dist)

    def test_changed_swapped_missing_extra_and_symlinked_artifacts_are_refused(self):
        _, dist, _ = self.bundle()
        original = {path.name: path.read_bytes() for path in dist.iterdir()}
        for mutation in ('bytes', 'swap', 'missing', 'extra', 'symlink', 'duplicate-checksum', 'missing-checksum'):
            with self.subTest(mutation=mutation):
                for path in dist.iterdir():
                    path.unlink()
                for name, data in original.items():
                    (dist / name).write_bytes(data)
                if mutation == 'bytes':
                    (dist / 'coreav').write_bytes(b'changed')
                elif mutation == 'swap':
                    (dist / 'coreav').write_bytes(original['r3-harness'])
                    (dist / 'r3-harness').write_bytes(original['coreav'])
                elif mutation == 'missing':
                    (dist / 'r3-harness').unlink()
                elif mutation == 'extra':
                    (dist / 'extra').touch()
                elif mutation == 'symlink':
                    (dist / 'coreav').unlink()
                    (dist / 'coreav').symlink_to(dist / 'r3-harness')
                elif mutation == 'duplicate-checksum':
                    (dist / 'sha256.txt').write_bytes(original['sha256.txt'] * 2)
                else:
                    (dist / 'sha256.txt').write_text('')
                with self.assertRaises(ValueError):
                    self.verify(dist)

    def test_modified_sources_cannot_claim_clean_commit_provenance(self):
        _, dist, _ = self.bundle()
        self.repo.write('deadline', 'dirty\n')
        with self.assertRaises(ValueError):
            self.plan()
        with self.assertRaises(ValueError):
            self.verify(dist)

    def test_the_harness_driver_and_its_path_modules_are_frozen(self):
        driver = ROOT / 'crates/coreav-cli/tests/ffmpeg.rs'
        self.assertIn(driver.relative_to(ROOT).as_posix(), sources.FROZEN)
        modules = re.findall(r'#\[path\s*=\s*"([^"]+)"\]', driver.read_text())
        self.assertTrue(modules)
        for module in modules:
            path = (driver.parent / module).resolve()
            self.assertTrue(any(path == ROOT / frozen or path.is_relative_to(ROOT / frozen)
                                for frozen in sources.FROZEN), module)

    def test_observer_sources_are_registered_at_the_candidate_not_the_tooling_copy(self):
        for path in sources.OBSERVER:
            target = path + '/mod.rs' if path == 'crates/coreav-isobmff/tests/common' else path
            self.repo.write(target, 'development observer\n')
        self.repo.tooling = self.repo.save()
        self.repo.git('update-ref', 'refs/remotes/origin/main', self.repo.tooling)
        planned = self.plan()
        _, dist, manifest = self.bundle()
        for path in sources.OBSERVER:
            expected = self.repo.git('rev-parse', f'{self.repo.candidate}:{path}')
            self.assertNotEqual(expected, self.repo.git('rev-parse', f'{self.repo.tooling}:{path}'))
            self.assertEqual(planned['frozen'][path], {'owner': 'r3-harness', 'object': expected})
            self.assertEqual(manifest['judging']['observer']['sources'][path], expected)
        shallow = Path(self.tmp.name) / 'shallow'
        subprocess.run(['git', 'clone', '--quiet', '--depth', '1', self.root.as_uri(), str(shallow)],
                       check=True, capture_output=True)
        absent = subprocess.run(['git', 'cat-file', '-e', self.repo.candidate], cwd=shallow,
                                capture_output=True, check=False)
        self.assertNotEqual(absent.returncode, 0)
        self.assertEqual(sources.verify(shallow, dist, self.repo.candidate, self.repo.tooling, self.run), manifest)
        for path in sources.OBSERVER:
            for oid in ('a' * 40, self.repo.git('rev-parse', f'{self.repo.tooling}:{path}')):
                with self.subTest(path=path, object=oid):
                    altered = copy.deepcopy(manifest)
                    altered['frozen'][path]['object'] = oid
                    (dist / 'sources.json').write_text(json.dumps(altered))
                    with self.assertRaisesRegex(ValueError, re.escape(path)):
                        sources.verify(shallow, dist, self.repo.candidate, self.repo.tooling, self.run)

    def test_candidate_build_configuration_must_be_clean_but_ignored_outputs_are_allowed(self):
        candidate, dist, _ = self.bundle()
        (candidate / 'target').mkdir()
        (candidate / 'target/cache').write_text('ignored build output\n')
        sources.record(self.root, candidate, dist, self.repo.candidate,
                       self.repo.tooling, self.run, 'rustc fixture')
        (candidate / '.cargo').mkdir()
        (candidate / '.cargo/config.toml').write_text('[build]\njobs = 1\n')
        with self.assertRaisesRegex(ValueError, 'untracked'):
            sources.record(self.root, candidate, dist, self.repo.candidate,
                           self.repo.tooling, self.run, 'rustc fixture')

    def test_the_registered_candidate_cannot_omit_an_observer_source(self):
        for path in sources.OBSERVER:
            with self.subTest(path=path):
                self.repo.git('reset', '--hard', self.repo.candidate)
                self.repo.git('rm', '-r', path)
                missing = self.repo.save()
                with self.assertRaisesRegex(ValueError, re.escape(path)):
                    sources.observer_sources(self.root, missing)

    def test_version_one_bundles_and_ambiguous_source_entries_are_rejected(self):
        _, dist, manifest = self.bundle()
        old = copy.deepcopy(manifest)
        old['schema'] = 'compat-0.1.0-sources/1'
        old['frozen'] = {path: value['object'] for path, value in old['frozen'].items()}
        (dist / 'sources.json').write_text(json.dumps(old))
        with self.assertRaisesRegex(ValueError, 'schema'):
            self.verify(dist)
        path = sources.OBSERVER[0]
        for entry in ('a' * 40, {'owner': 'tooling', 'object': manifest['frozen'][path]['object']},
                      {**manifest['frozen'][path], 'extra': True}):
            with self.subTest(entry=entry):
                altered = copy.deepcopy(manifest)
                altered['frozen'][path] = entry
                (dist / 'sources.json').write_text(json.dumps(altered))
                with self.assertRaisesRegex(ValueError, re.escape(path)):
                    self.verify(dist)

    def test_observer_registration_rejects_wrong_candidate_objects_and_rosters(self):
        base = self.repo.tooling
        original = json.loads((self.root / sources.JUDGING_REGISTRATION).read_text())
        path = sources.OBSERVER[0]
        for mutation in ('candidate', 'object', 'missing', 'extra', 'format'):
            with self.subTest(mutation=mutation):
                self.repo.git('reset', '--hard', base)
                changed = copy.deepcopy(original)
                observer = changed['observer']
                if mutation == 'candidate':
                    observer['commit'] = 'b' * 40
                elif mutation == 'object':
                    observer['sources'][path] = 'a' * 40
                elif mutation == 'missing':
                    del observer['sources'][path]
                elif mutation == 'extra':
                    observer['sources']['unknown.rs'] = 'a' * 40
                else:
                    observer['sources'][path] = 'A' * 40
                self.repo.write(sources.JUDGING_REGISTRATION, json.dumps(changed))
                self.repo.tooling = self.repo.save()
                self.repo.git('update-ref', 'refs/remotes/origin/main', self.repo.tooling)
                with self.assertRaises(ValueError):
                    self.plan()
                with self.assertRaises(ValueError):
                    sources.check(self.root)

    def test_each_judging_file_drift_and_deletion_names_the_file(self):
        base = self.repo.tooling
        for path in sources.JUDGING:
            with self.subTest(path=path):
                self.repo.git('reset', '--hard', base)
                self.repo.write(path, (self.root / path).read_text() + '\n')
                changed = self.repo.save()
                self.repo.git('update-ref', 'refs/remotes/origin/main', changed)
                with self.assertRaisesRegex(ValueError, re.escape(path)):
                    self.plan(tooling=changed)
        self.repo.git('reset', '--hard', base)
        (self.root / 'tools/compat-corpus/r3.py').unlink()
        with self.assertRaisesRegex(ValueError, 'tools/compat-corpus/r3.py'):
            sources.judging(self.root, self.repo.save())

    def test_judging_hashes_use_committed_bytes_and_reregistration_is_explicit(self):
        before = sources.judging(self.root, self.repo.tooling)
        registration = sources.register(self.root, self.repo.tooling)
        self.assertEqual(registration, sources.register(self.root, self.repo.tooling))
        self.repo.write('tools/compat-corpus/r3.py', 'changed judge\n')
        self.assertEqual(sources.judging(self.root, self.repo.tooling), before)
        changed = self.repo.save()
        with self.assertRaisesRegex(ValueError, 'tools/compat-corpus/r3.py'):
            self.plan(tooling=changed, event='pull_request')
        self.repo.write(sources.JUDGING_REGISTRATION, sources.register(self.root, changed))
        self.repo.tooling = self.repo.save()
        revised = self.plan(event='pull_request')
        self.assertNotEqual(revised['judging'], before)
        self.assertEqual(revised['judging']['registration_sha256'],
                         sources.digest(self.root / sources.JUDGING_REGISTRATION))

    def test_judging_registration_refuses_malformed_or_incomplete_rosters(self):
        base = self.repo.tooling
        original = json.loads(sources.register(self.root, base))
        invalid = ['', '[]', '{', json.dumps({**original, 'schema': 'old'}),
                   json.dumps({**original, 'extra': {}})]
        invalid.append(json.dumps({'schema': 'compat-0.1.0-judging/1', 'files': original['files']}))
        for extra in (sources.README, sources.JUDGING_REGISTRATION, sources.FROZEN[0], 'unknown.py'):
            invalid.append(json.dumps({**original, 'files': {**original['files'], extra: 'a' * 64}}))
        for hash_value in ('A' * 64, 'a' * 63, None):
            invalid.append(json.dumps({**original, 'files': {**original['files'], sources.JUDGING[0]: hash_value}}))
        invalid.append(json.dumps({**original, 'files': dict(list(original['files'].items())[1:])}))
        for text in invalid:
            with self.subTest(text=text[:70]):
                self.repo.git('reset', '--hard', base)
                self.repo.write(sources.JUDGING_REGISTRATION, text)
                with self.assertRaises(ValueError):
                    sources.judging(self.root, self.repo.save())
        self.repo.git('reset', '--hard', base)
        (self.root / sources.JUDGING_REGISTRATION).unlink()
        with self.assertRaisesRegex(ValueError, re.escape(sources.JUDGING_REGISTRATION)):
            sources.judging(self.root, self.repo.save())


class JudgingRoster(unittest.TestCase):
    def test_frozen_and_observer_rosters_keep_their_exact_scope(self):
        self.assertEqual(sources.FROZEN, (
            'scripts/oracle-ffmpeg.env', 'scripts/install-oracle-ffmpeg.sh',
            'scripts/install-oracle-mediainfo.sh', 'scripts/oracle-audio-args.txt',
            'tools/compat-corpus/commands-0.1.0', 'tools/compat-corpus/round_files.py',
            'crates/coreav-cli/tests/ffmpeg.rs', 'crates/coreav-isobmff/tests/common',
        ))
        self.assertEqual(sources.OBSERVER, ('crates/coreav-cli/tests/ffmpeg.rs',
                                          'crates/coreav-isobmff/tests/common'))
        self.assertLessEqual(set(sources.OBSERVER), set(sources.FROZEN))

    def test_deny_checks_the_real_registration_and_rejects_stale_committed_bytes(self):
        from test_registered_candidate import load_workflow
        workflow = load_workflow('deny.yml')
        step = next(step for step in workflow['jobs']['deny-checks']['steps']
                    if 'scripts/compat_sources.py check' in step.get('run', ''))
        self.assertIn('python3 -I scripts/compat_sources.py check', step['run'].splitlines())
        self.assertEqual(step['if'], "steps.result.outputs.reuse != 'true'")
        command = [sys.executable, '-I', str(ROOT / 'scripts/compat_sources.py'), 'check']
        git_env = {key: value for key, value in os.environ.items()
                   if not key.startswith(('GIT_AUTHOR_', 'GIT_COMMITTER_'))}
        git_env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_COUNT='1',
                       GIT_CONFIG_KEY_0='user.useConfigOnly', GIT_CONFIG_VALUE_0='true')
        valid = subprocess.run(command, cwd=ROOT, env=git_env, capture_output=True, text=True, check=False)
        self.assertEqual(valid.returncode, 0, valid.stderr)
        self.assertIn('36 files verified', valid.stdout)
        for changed_file in (sources.JUDGING_REGISTRATION, 'scripts/oracle_audio.py'):
            with self.subTest(path=changed_file), tempfile.TemporaryDirectory() as tmp:
                clone = Path(tmp) / 'source'
                subprocess.run(['git', 'clone', '--quiet', '--shared', '--no-checkout', str(ROOT), str(clone)],
                               env=git_env, check=True, capture_output=True)
                def git(*args, input=None):
                    return subprocess.check_output(['git', *args], cwd=clone, env=git_env,
                                                   input=input, text=True).strip()
                for key in ('user.name', 'user.email'):
                    identity = subprocess.run(['git', 'config', '--get', key], cwd=clone, env=git_env,
                                              capture_output=True, text=True, check=False)
                    self.assertNotEqual(identity.returncode, 0)
                git('read-tree', 'HEAD')
                text = subprocess.check_output(['git', 'show', f'HEAD:{changed_file}'], cwd=clone, text=True)
                if changed_file == sources.JUDGING_REGISTRATION:
                    registration = json.loads(text)
                    registration['files']['scripts/oracle_audio.py'] = 'a' * 64
                    text = json.dumps(registration)
                else:
                    text += '\n'
                oid = git('hash-object', '-w', '--stdin', input=text)
                self.assertNotEqual(oid, git('rev-parse', f'HEAD:{changed_file}'))
                git('update-index', '--cacheinfo', f'100644,{oid},{changed_file}')
                commit = synthetic_commit(git, git('write-tree'), git('rev-parse', 'HEAD'))
                git('update-ref', '--no-deref', 'HEAD', commit)
                failed = subprocess.run(command, cwd=clone, env=git_env, capture_output=True, text=True, check=False)
                self.assertEqual(failed.returncode, 1, failed.stdout + failed.stderr)
                self.assertIn('scripts/oracle_audio.py differs from its registered judging hash', failed.stderr)

    def assert_imports_frozen(self, root, roster):
        owned = set(roster) | set(sources.FROZEN)
        search = ('scripts', 'tools/compat-corpus', 'tools/oracle', 'tools/stage3-report')
        for path in sorted(owned):
            if not path.endswith('.py'):
                continue
            for node in ast.walk(ast.parse((root / path).read_text())):
                names = [alias.name for alias in node.names] if isinstance(node, ast.Import) else \
                    [node.module] if isinstance(node, ast.ImportFrom) and node.module else []
                for name in names:
                    for directory in search:
                        imported = f'{directory}/{name.split(".")[0]}.py'
                        if (root / imported).is_file():
                            self.assertIn(imported, owned, f'{path} imports {imported}; add it to JUDGING, then register')
                if isinstance(node, ast.Constant) and isinstance(node.value, str) and \
                        re.fullmatch(r'(?:docs/)?known-divergences(?:-[a-z-]+)?\.json|ffmpeg9\.json', node.value):
                    data = node.value if node.value.startswith('docs/') else \
                        'tools/compat-corpus/ffmpeg9.json' if node.value == 'ffmpeg9.json' else 'docs/' + node.value
                    self.assertIn(data, owned, f'{path} reads {data}; add it to JUDGING, then register')

    def test_judging_imports_data_and_dynamic_entry_points_are_frozen(self):
        self.assert_imports_frozen(ROOT, sources.JUDGING)
        for path in ('tools/compat-corpus/copy_check.py', 'scripts/oracle_kd.py'):
            self.assertIn(path, sources.JUDGING)
        with self.assertRaises(AssertionError):
            self.assert_imports_frozen(ROOT, set(sources.JUDGING) - {'scripts/clip_check.py'})
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'scripts').mkdir()
            (root / 'scripts/entry.py').write_text('import new_helper\n')
            (root / 'scripts/new_helper.py').write_text('')
            with self.assertRaisesRegex(AssertionError, 'scripts/new_helper.py'):
                self.assert_imports_frozen(root, {'scripts/entry.py'})

    def test_registration_structure_and_sparse_checkout_cover_the_roster(self):
        from test_registered_candidate import load_workflow
        workflow = load_workflow('compat-0.1.0.yml')
        patterns = workflow['jobs']['compat-plan']['steps'][0]['with']['sparse-checkout'].splitlines()
        for path in (*sources.JUDGING, sources.JUDGING_REGISTRATION):
            self.assertTrue(any(fnmatch.fnmatch(path, pattern.lstrip('/') + ('*' if pattern.endswith('/') else ''))
                                for pattern in patterns), path)
        self.assertEqual(len(sources.JUDGING), len(set(sources.JUDGING)))
        self.assertFalse(set(sources.JUDGING) & set(sources.FROZEN))
        self.assertNotIn(sources.README, sources.JUDGING)
        self.assertNotIn(sources.JUDGING_REGISTRATION, sources.JUDGING)
        self.assertFalse(any('/tests/' in path for path in sources.JUDGING))
        registration = json.loads((ROOT / sources.JUDGING_REGISTRATION).read_text())
        self.assertEqual(registration['schema'], sources.JUDGING_SCHEMA)
        self.assertEqual(set(registration['files']), set(sources.JUDGING))


if __name__ == '__main__':
    unittest.main()
