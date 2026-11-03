"""Media-free tests of the registered PGO evidence boundary."""
import ast
from contextlib import redirect_stderr
import io
import json
import os
import subprocess
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import h264_parity_bars as bars
import h264_pgo_round as round_tool
import h264_pgo_build as build_tool
import h264_pgo_diagnostics as diagnostics


class PgoRound(unittest.TestCase):
    def setUp(self):
        self.plan = json.loads(round_tool.PLAN.read_text())

    def test_interval_bar_is_inclusive_but_significance_is_strict(self):
        one = {'median': 0.97, 'ci95': [0.94, 0.999]}
        eight = {c: {'ci95': [0.98, 1.01]} for c in bars.CLASSES}
        self.assertTrue(round_tool.verdict(one, eight, self.plan))
        eight['1080p']['ci95'][1] = 1.010001
        self.assertFalse(round_tool.verdict(one, eight, self.plan))
        eight['1080p']['ci95'][1] = 1.01
        one['ci95'][1] = 1
        self.assertFalse(round_tool.verdict(one, eight, self.plan))
        one['ci95'][1] = 0.999
        one['median'] = 0.970001
        self.assertFalse(round_tool.verdict(one, eight, self.plan))

    def test_paired_round_bootstrap_preserves_round_pairing(self):
        streams = {c: c for c in bars.CLASSES}
        times = {(s, a): {i: (i + 1) * (0.95 if a == 'pgo' else 1) for i in range(3)}
                 for s in streams for a in ('base', 'pgo')}
        value = bars.paired_round_summary(times, streams, 'pgo', 'base', 3, 42, 100)
        for cell in value['groups'].values():
            self.assertAlmostEqual(cell['median'], 0.95)
            for bound in cell['ci95']:
                self.assertAlmostEqual(bound, 0.95)
        del times['4k', 'pgo'][2]
        with self.assertRaises(bars.Refused):
            bars.paired_round_summary(times, streams, 'pgo', 'base', 3, 42, 100)

    def test_duplicate_source_and_registry_owner_omissions_are_refused(self):
        registry = {c: {'commitment': 'a' * 64, 'hashes': ['a' * 64], 'identities': ['old'], 'groups': ['old-group']}
                    for c in round_tool.COVERAGE}
        candidate = {'identity': 'new', 'groups': ['new-group']}
        self.assertEqual(round_tool.allocate([candidate], registry, self.plan), [candidate])
        self.assertEqual(round_tool.allocate([candidate | {'groups': ['old-group']}], registry, self.plan), [])
        self.assertEqual(round_tool.allocate([candidate | {'sha256': 'a' * 64}], registry, self.plan), [])
        registry['other-draws']['hashes'].append('b' * 12)
        self.assertEqual(round_tool.allocate([candidate | {'sha256': 'b' * 12 + 'c' * 52}], registry, self.plan), [])
        with self.assertRaises(ValueError):
            round_tool.exclusions(sorted(round_tool.COVERAGE))
        with self.assertRaises(ValueError):
            round_tool.exclusion_registry(sorted(round_tool.COVERAGE))
        with self.assertRaises(ValueError):
            round_tool.allocate([candidate, candidate], registry, self.plan)
        del registry['encoder-sets']
        with self.assertRaises(ValueError):
            round_tool.allocate([candidate], registry, self.plan)

    def test_source_normalization_cannot_bypass_an_exclusion(self):
        registry = {c: {'commitment': 'a' * 64, 'hashes': ['a' * 64],
                       'identities': ['https://example.test/a'], 'groups': ['old']}
                    for c in round_tool.COVERAGE}
        candidate = {'identity': 'https://EXAMPLE.test/a#fragment', 'groups': ['new']}
        self.assertEqual(round_tool.allocate([candidate], registry, self.plan), [])

    def test_standing_reserved_hashes_cannot_be_omitted_from_the_registry(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            receipt = {'commit': 'a' * 40, 'manifest_sha256': 'b' * 64}
            evidence = {'schema': 1, 'allocation': self.plan['allocation'], **receipt,
                        'frozen_sha256': 'c' * 64, 'marked_rows': 1, 'frozen_rows': 1,
                        'hashes': ['d' * 64, 'e' * 64]}
            registry = {c: {'commitment': 'a' * 64, 'hashes': ['d' * 64], 'identities': ['old'], 'groups': ['old']}
                        for c in round_tool.COVERAGE}
            round_tool.write(root / 'reserved-inputs.json', evidence)
            round_tool.write(root / 'exclusions.json', registry)
            with self.assertRaisesRegex(ValueError, 'omits standing'):
                round_tool.reserved_evidence(root, self.plan, receipt)
            registry['other-draws']['hashes'].append('e' * 64)
            round_tool.write(root / 'exclusions.json', registry)
            self.assertEqual(round_tool.reserved_evidence(root, self.plan, receipt), evidence)
            for change in ({'marked_rows': 0}, {'frozen_rows': 0}, {'manifest_sha256': 'f' * 64}, {'hashes': []}):
                round_tool.write(root / 'reserved-inputs.json', evidence | change)
                with self.subTest(change=change), self.assertRaises(ValueError):
                    round_tool.reserved_evidence(root, self.plan, receipt)

    def test_malformed_identity_or_provider_returns_incomplete_not_a_speed_miss(self):
        for bad in (None, 1, [], {}):
            with self.subTest(identity=bad), self.assertRaises(ValueError):
                round_tool.source_identity(bad)
            with self.subTest(provider=bad), self.assertRaises(ValueError):
                round_tool.host({'machine_type': bad})
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            for malformed in (lambda *_: round_tool.source_identity(1),
                              lambda *_: round_tool.host({'machine_type': 1})):
                with mock.patch.object(sys, 'argv', ['pgo', 'audit', str(root)]), \
                     mock.patch.object(round_tool, 'audit', side_effect=malformed), redirect_stderr(io.StringIO()):
                    self.assertEqual(round_tool.main(), 2)
                self.assertEqual(json.loads((root / 'failure.json').read_text()),
                                 {'status': 'incomplete', 'speed_verdict': None})
                self.assertFalse((root / 'summary.json').exists())

    def test_schedule_has_every_cell_once_and_is_deterministic(self):
        rows = [{'label': f'h{i:03}'} for i in range(64)]
        cells = round_tool.schedule(rows, self.plan)
        self.assertEqual(cells, round_tool.schedule(rows, self.plan))
        keys = {(r['label'], r['arm'], r['threads'], r['round']) for r in cells}
        self.assertEqual(len(keys), 64 * 3 * 2 * self.plan['rounds'])
        self.assertEqual(len(cells), len(keys))

    def test_process_failures_and_raw_overflow_cannot_be_fast_samples(self):
        with self.assertRaises(subprocess.CalledProcessError):
            round_tool.execute([sys.executable, '-c', 'raise SystemExit(1)'])
        with self.assertRaises(subprocess.CalledProcessError):
            round_tool.execute([sys.executable, '-c', 'print("unexpected", file=__import__("sys").stderr)'])
        with self.assertRaises(ValueError):
            round_tool.execute([sys.executable, '-c', 'print("x" * 1000)'], maximum=10)
        result = round_tool.execute([sys.executable, '-c', 'print("abc", end="")'], maximum=3)
        self.assertEqual(result['bytes'], 3)
        self.assertGreater(result['wall'], 0)

    def test_registry_indexes_hashes_sources_and_groups_from_actual_files(self):
        with tempfile.TemporaryDirectory() as name:
            p = Path(name) / 'inventory.json'
            p.write_text(json.dumps({'sha256': 'b' * 64, 'url': 'https://example.test/new', 'groups': ['family']}))
            owners = {owner: [str(p)] for owner in round_tool.COVERAGE}
            result = round_tool.exclusion_registry(owners)
            self.assertIn('b' * 64, result['training']['hashes'])
            self.assertIn('https://example.test/new', result['encoder-sets']['identities'])
            self.assertIn('family', result['other-draws']['groups'])
            previous = result['training']['commitment']
            p.write_text(p.read_text() + '\n')
            self.assertNotEqual(previous, round_tool.exclusion_registry(owners)['training']['commitment'])

    def test_training_roster_is_the_profile_only_half(self):
        rows = build_tool.training_roster(ROOT)
        self.assertEqual(len(rows), 16)
        self.assertEqual(sum(r['class'] == '1080p' for r in rows.values()), 4)
        import hashlib
        commitment = hashlib.sha256(json.dumps(sorted(r['es_sha256'] for r in rows.values())).encode()).hexdigest()
        self.assertEqual(commitment, 'f0a71e9f33523c11da1e31a0e6cc1cd69b3216788e103475fb11c2eb778908eb')

    def test_clean_compile_instruments_only_the_explicit_target(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            def fake_run(command, **kwargs):
                self.assertEqual(command[-2:], ['--target', build_tool.TARGET])
                self.assertIn('--remap-path-prefix=', kwargs['env']['CARGO_ENCODED_RUSTFLAGS'])
                self.assertIn('build-only-', kwargs['env']['LLVM_PROFILE_FILE'])
                binary = root / 'new/target' / build_tool.TARGET / 'dist/examples/decode'
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b'compiled')
            with mock.patch.object(build_tool.subprocess, 'run', side_effect=fake_run):
                value = build_tool.compile_binary(root / 'source', root / 'new', ['-Cprofile-generate=/profiles'])
            self.assertEqual(value['sha256'], round_tool.digest(root / 'new/decode'))

    def test_pgo_subprocess_probes_and_builds_have_finite_timeouts(self):
        for name in ('h264_pgo_build.py', 'h264_pgo_round.py', 'h264_pgo_diagnostics.py'):
            for call in ast.walk(ast.parse((ROOT / 'scripts' / name).read_text())):
                if not (isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute) and
                        isinstance(call.func.value, ast.Name) and call.func.value.id == 'subprocess' and
                        call.func.attr in ('run', 'check_output')):
                    continue
                limit = next((k.value for k in call.keywords if k.arg == 'timeout'), None)
                with self.subTest(file=name, line=call.lineno):
                    self.assertIsInstance(limit, ast.Constant, 'add a finite timeout to this subprocess call')
                    self.assertIs(type(limit.value), int)
                    self.assertGreater(limit.value, 0)

    def cargo_stub(self, command, **kwargs):
        target = Path(kwargs['env']['CARGO_TARGET_DIR'])
        if '--target' in command:
            target /= command[command.index('--target') + 1]
        binary = target / 'dist' / ('coreav' if '--bin' in command else 'examples/decode')
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b'compiled fixture')
        self.assertFalse(any(arg == '--config' or arg.startswith('--config=') for arg in command))

    def test_profile_environment_overrides_are_refused_before_compilation(self):
        overrides = {'CARGO_PROFILE_DIST_OPT_LEVEL': '0', 'CARGO_PROFILE_DIST_LTO': 'off',
                     'CARGO_PROFILE_DIST_CODEGEN_UNITS': '256', 'CARGO_PROFILE_DIST_DEBUG': '2',
                     'CARGO_PROFILE_RELEASE_PANIC': 'unwind', 'CARGO_PROFILE_DIST_BUILD_OVERRIDE_OPT_LEVEL': '0',
                     'CARGO_PROFILE_DIST_FUTURE_SETTING': '', 'CARGO_BUILD_RUSTFLAGS': '-Copt-level=0',
                     'CARGO_BUILD_INCREMENTAL': 'true', 'CARGO_INCREMENTAL': '1',
                     'CARGO_BUILD_RUSTC': '/outside/rustc', 'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS': '-Copt-level=0',
                     'RUSTFLAGS': '-Copt-level=0', 'CARGO_ENCODED_RUSTFLAGS': '', 'RUSTC_BOOTSTRAP': '1'}
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            for key, value in overrides.items():
                for artifact, historical in (('decode', False), ('cli', False), ('decode', True)):
                    out = root / (key + artifact + str(historical))
                    with self.subTest(key=key, artifact=artifact, historical=historical), \
                         mock.patch.dict(os.environ, {key: value}), \
                         mock.patch.object(build_tool.subprocess, 'run', side_effect=self.cargo_stub) as child:
                        with self.assertRaisesRegex(ValueError, 'override'):
                            build_tool.compile_binary(root / 'source', out, [], historical=historical, artifact=artifact)
                        child.assert_not_called()
                        self.assertFalse(out.exists())
            with mock.patch.dict(os.environ, {'CARGO_PROFILE_DIST_OPT_LEVEL': '0'}), \
                 mock.patch.object(build_tool.subprocess, 'check_output') as compiler:
                with self.assertRaisesRegex(ValueError, 'CARGO_PROFILE_DIST_OPT_LEVEL'):
                    build_tool.toolchain(root / 'source')
                compiler.assert_not_called()

    def test_cargo_configuration_is_refused_at_the_compilation_boundary(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            source, home = root / 'source', root / 'cargo-home'
            source.mkdir()
            for index, directory in enumerate((source / '.cargo', root / '.cargo', home)):
                directory.mkdir(exist_ok=True)
                for filename in ('config', 'config.toml'):
                    config = directory / filename
                    out = root / f'new-{index}-{filename}'
                    config.write_text('[profile.dist]\nopt-level = 0\n')
                    with self.subTest(config=str(config)), mock.patch.dict(os.environ, {'CARGO_HOME': str(home)}), \
                         mock.patch.object(build_tool.subprocess, 'run', side_effect=self.cargo_stub) as child:
                        with self.assertRaisesRegex(ValueError, 'override'):
                            build_tool.compile_binary(source, out, [])
                        child.assert_not_called()
                        self.assertFalse(out.exists())
                    config.unlink()
            for index, cargo_home in enumerate(('', 'relative-home')):
                with self.subTest(cargo_home=cargo_home), mock.patch.dict(os.environ, {'CARGO_HOME': cargo_home}), \
                     mock.patch.object(build_tool.subprocess, 'run', side_effect=self.cargo_stub) as child:
                    with self.assertRaisesRegex(ValueError, 'absolute CARGO_HOME'):
                        build_tool.compile_binary(source, root / f'home-{index}', [])
                    child.assert_not_called()
            default_home = root / 'user'
            (default_home / '.cargo').mkdir(parents=True)
            (default_home / '.cargo/config.toml').write_text('[profile.dist]\nopt-level = 0\n')
            with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(Path, 'home', return_value=default_home), \
                 mock.patch.object(build_tool.subprocess, 'run', side_effect=self.cargo_stub) as child:
                with self.assertRaisesRegex(ValueError, 'configuration overrides'):
                    build_tool.compile_binary(source, root / 'default-home', [])
                child.assert_not_called()

    def test_command_line_config_is_not_forwarded_to_cargo(self):
        for extra in (['--config', 'profile.dist.opt-level=0'], ['--config=profile.dist.opt-level=0'],
                      ['--', '--config', '/outside/config.toml']):
            with self.subTest(extra=extra), mock.patch.object(build_tool, 'toolchain') as compiler, \
                 mock.patch.object(sys, 'argv', ['build', 'collect', '--source', '/source', '--out', '/out', *extra]), \
                 redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as refused:
                    build_tool.main()
                self.assertEqual(refused.exception.code, 2)
                compiler.assert_not_called()

    def test_cli_companion_is_explicit_and_does_not_change_the_example_target(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            def fake_run(command, **kwargs):
                self.assertIn('coreav-cli', command)
                self.assertEqual(command[command.index('--features') + 1], 'native-h264')
                self.assertNotIn('--example', command)
                self.assertEqual(command[-2:], ['--target', build_tool.TARGET])
                binary = root / 'new/target' / build_tool.TARGET / 'dist/coreav'
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b'compiled CLI')
            with mock.patch.object(build_tool.subprocess, 'run', side_effect=fake_run):
                value = build_tool.compile_binary(root / 'source', root / 'new', [], artifact='cli')
            self.assertEqual(value['sha256'], round_tool.digest(root / 'new/coreav'))

    def test_audit_refuses_missing_duplicate_aborted_and_changed_evidence(self):
        plan = self.plan | {'count_per_class': 1, 'rounds': 2, 'bootstrap_replicates': 20}
        registry = {c: {'commitment': 'f' * 64, 'hashes': ['f' * 64], 'identities': ['old'], 'groups': ['old']}
                    for c in round_tool.COVERAGE}
        rows = [{'label': f'h{i:03}', 'class': c, 'width': w, 'height': h, 'groups': [f'group{i}'],
                 'identity': f'candidate{i}', 'source_sha256': str(i) * 64, 'sha256': str(i + 4) * 64}
                for i, (c, w, h) in enumerate((('sd', 640, 480), ('720p', 1280, 720),
                                               ('1080p', 1920, 1080), ('4k', 3840, 2160)))]
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            round_tool.write(root / 'roster.json', rows)
            round_tool.write(root / 'exclusions.json', registry)
            round_tool.write(root / 'reservation-receipt.json', {'allocation': plan['allocation'], 'commit': 'a' * 40,
                              'manifest_sha256': 'b' * 64, 'count_per_class': 1, 'population': 'neutral-population',
                              'checker': {'path': 'scripts/check.py', 'sha256': 'c' * 64}})
            round_tool.write(root / 'reserved-inputs.json', {'schema': 1, 'allocation': plan['allocation'], 'commit': 'a' * 40,
                             'manifest_sha256': 'b' * 64, 'frozen_sha256': 'e' * 64,
                             'marked_rows': 1, 'frozen_rows': 1, 'hashes': ['f' * 64]})
            provider = {'machine_type': 'c4-standard-16', 'provisioning': 'STANDARD',
                        'advanced_machine_features': {'threadsPerCore': 1, 'turboMode': 'ALL_CORE_MAX'}}
            round_tool.write(root / 'instance.json', provider)
            approved = build_tool.training_roster(ROOT)
            selected = [next(n for n, r in approved.items() if r['class'] == cls) for cls in bars.CLASSES]
            refs = [{'label': f'r{i:03}', 'class': approved[n]['class'], 'sha256': approved[n]['es_sha256']}
                    for i, n in enumerate(selected)]
            for row in rows + refs:
                p = root / 'media' / (row['label'] + '.h264')
                p.parent.mkdir(exist_ok=True)
                p.write_bytes(b'bound-media-placeholder-' + row['label'].encode())
                row['sha256'] = round_tool.digest(p)
            for name, row in zip(selected, refs):
                approved[name] = approved[name] | {'es_sha256': row['sha256']}
            round_tool.write(root / 'roster.json', rows)
            round_tool.write(root / 'references.json', refs)
            candidates = round_tool.allocate([{'identity': r['identity'], 'groups': r['groups']} for r in rows], registry, plan)
            rows = [next(r for r in rows if r['identity'] == c['identity']) for c in candidates]
            round_tool.write(root / 'roster.json', rows)
            round_tool.write(root / 'allocation.json', candidates)
            (root / 'snapshots').mkdir()
            round_tool.write(root / 'snapshots/s0000.json', {'neutral': 'metadata-only fixture'})
            round_tool.write(root / 'population.json', {'allocation': plan['allocation'], 'population': 'neutral-population',
                             'snapshots': {'s0000.json': round_tool.digest(root / 'snapshots/s0000.json')}, 'candidates': candidates})
            decisions = [{'identity': r['identity'], 'status': 'selected', 'source_sha256': r['source_sha256'],
                          'capture_status': 'ok', 'metadata': {'codec_name': 'h264', 'pix_fmt': 'yuv420p',
                          'field_order': 'progressive', 'profile': 'High', 'width': r['width'], 'height': r['height'], 'frames': 1}}
                         for r in rows]
            round_tool.write(root / 'collection.json', decisions)
            for name in ('base', 'pgo', 'independent', 'profile.profdata', 'ffmpeg', 'ffprobe', 'aa', 'null',
                         'cli-base', 'cli-pgo', 'cli-independent', *diagnostics.FILES):
                (root / name).parent.mkdir(exist_ok=True)
                (root / name).write_bytes(b'bound-artifact-placeholder')
            sha = round_tool.digest(root / 'pgo')
            base_flags = ['--remap-path-prefix=/source=/coreav-source']
            pgo_flags = [*base_flags, '-Cprofile-use=/profile.profdata', '-Cllvm-args=-pgo-warn-missing-function']
            cli_command = ['cargo', '+1.99.1', 'build', '--locked', '--profile', 'dist', '-p', 'coreav-cli',
                           '--bin', 'coreav', '--features', 'native-h264', '--target', build_tool.TARGET]
            build = {'schema': 1, 'source': {'commit': 'c' * 40, 'lock_sha256': round_tool.digest(ROOT / 'Cargo.lock')},
                     'recipe_sha256': round_tool.digest(ROOT / 'scripts/h264_pgo_build.py'), 'independent_match': True,
                     'pgo': {'sha256': sha, 'flags': pgo_flags}, 'independent': {'sha256': sha},
                     'base': {'sha256': sha, 'flags': base_flags},
                     'profile_sha256': round_tool.digest(root / 'profile.profdata'),
                     'tools': {'rustc': 'release: 1.99.1', 'profdata': '23.1.1'},
                     'training': [{'input': n, 'threads': t, 'input_sha256': r['es_sha256'], 'bytes': 1}
                                  for n, r in approved.items() for t in (1, 8)],
                     'cli': {'features': ['native-h264'], 'independent_match': True,
                             'base': {'sha256': sha, 'command': cli_command, 'flags': base_flags},
                             'pgo': {'sha256': sha, 'command': cli_command, 'flags': pgo_flags},
                             'independent': {'sha256': sha, 'command': cli_command, 'flags': pgo_flags}}}
            round_tool.write(root / 'build.json', build)
            contract = {'schema': 1, 'plan': round_tool.digest(round_tool.PLAN), 'revision': 'c' * 40,
                        'files': {n: round_tool.digest(root / n) for n in round_tool.allocation_files(['snapshots/s0000.json'])}}
            round_tool.write(root / 'allocation-contract.json', contract)
            allocation_sha256 = round_tool.digest(root / 'allocation-contract.json')
            binding = {'plan': round_tool.digest(round_tool.PLAN), 'revision': 'c' * 40, 'allocation_sha256': allocation_sha256,
                       'tools': {f'scripts/{n}': round_tool.digest(ROOT / 'scripts' / n) for n in
                                 ('h264_pgo_round.py', 'h264_pgo_build.py', 'h264_parity_bars.py', 'h264_pgo_diagnostics.py')},
                       'files': {n: round_tool.digest(root / n) for n in round_tool.evidence_inventory(rows, refs)}}
            round_tool.write(root / 'binding.json', binding)
            events = [{'kind': 'host', 'phase': 'before', 'instance': provider, 'affinity': list(range(8)),
                       'topology': [str(c) for c in range(8)], 'cpu': '8581C'}]
            for t in (1, 8):
                for comparison in ('aa', 'null'):
                    events.extend({'kind': 'calibration-call', 'comparison': comparison, 'threads': t,
                                   'class': c, 'arm': a, 'round': n, 'wall': 1.0}
                                  for c in bars.CLASSES for n in range(2) for a in ('base', comparison))
                    events.append({'kind': 'calibration', 'comparison': comparison, 'threads': t, 'ratio': 1.0})
            references = {str(t): {c: 1.0 for c in bars.CLASSES} for t in (1, 8)}
            events.append({'kind': 'health', 'phase': 'before', 'references': references})
            for row in rows:
                events.append({'kind': 'metadata', 'label': row['label'], 'value': {
                    'width': row['width'], 'height': row['height'], 'codec_name': 'h264', 'pix_fmt': 'yuv420p',
                    'field_order': 'progressive', 'profile': 'High'}})
                pixels = row['width'] * row['height'] * 3 // 2
                for t in (1, 8):
                    for a in round_tool.ARMS:
                        events.append({'kind': 'qualification', 'label': row['label'], 'threads': t,
                                       'arm': a, 'bytes': pixels, 'sha256': 'd' * 64, 'frames': 1})
            events.extend({'kind': 'warmup', 'label': row['label'], 'threads': t, 'arm': a}
                          for row in rows for t in (1, 8) for a in round_tool.ARMS)
            events.extend({'kind': 'timed', **cell, 'wall': 0.95 if cell['arm'] == 'pgo' else 1.0}
                          for cell in round_tool.schedule(rows, plan))
            events.extend([{'kind': 'diagnostics', 'status': 'complete'},
                           {'kind': 'health', 'phase': 'after', 'references': references},
                           events[0] | {'phase': 'after'}, {'kind': 'complete'}])
            def save(values):
                (root / 'events.jsonl').write_text(''.join(json.dumps(v) + '\n' for v in values))
            def audited():
                with mock.patch.object(build_tool, 'training_roster', return_value=approved), \
                     mock.patch.object(diagnostics, 'verify'), \
                     mock.patch.object(diagnostics, 'audit', return_value={'status': 'complete', 'gates_h264': False,
                                                                         'commands': {'probe': {'1': {'guard': False}}}}):
                    return round_tool.audit(root, plan, allocation_sha256)
            save(events)
            checked = audited()
            self.assertTrue(checked['pass'])
            self.assertFalse(checked['diagnostics']['gates_h264'])
            self.assertFalse(checked['diagnostics']['commands']['probe']['1']['guard'])
            for change in ({'files': {}, 'tools': {}}, {'files': list(binding['files'])}, {'tools': list(binding['tools'])}):
                round_tool.write(root / 'binding.json', binding | change)
                with self.subTest(change=change), self.assertRaises(ValueError):
                    audited()

            round_tool.write(root / 'binding.json', binding)
            old_profile = (root / 'profile.profdata').read_bytes()
            (root / 'profile.profdata').write_bytes(b'replacement')
            with self.assertRaises(ValueError):
                audited()
            (root / 'profile.profdata').write_bytes(old_profile)
            old_population = (root / 'population.json').read_bytes()
            (root / 'population.json').write_bytes(old_population + b' ')
            with self.assertRaises(ValueError):
                audited()
            (root / 'population.json').write_bytes(old_population)
            bad = [d | {'status': 'excluded', 'reason': 'unsupported'} for d in decisions]
            with self.assertRaises(ValueError):
                round_tool.selection(rows, candidates, bad, registry, plan)
            timed = next(i for i, r in enumerate(events) if r['kind'] == 'timed')
            for broken in (events[:timed] + events[timed + 1:],
                           [r for r in events if not (r['kind'] == 'qualification' and r['label'] == rows[0]['label'])],
                           events[:timed] + [events[timed]] + events[timed:],
                           events[:-1] + [{'kind': 'abort'}],
                           [r for r in events if r['kind'] != 'health'][:-1] +
                           [r for r in events if r['kind'] == 'health'] + [{'kind': 'complete'}],
                           [r for r in events if r['kind'] != 'metadata'][:-1] +
                           [r for r in events if r['kind'] == 'metadata'] + [{'kind': 'complete'}],
                           [r | {'ratio': 0.999} if r['kind'] == 'calibration' else r for r in events]):
                save(broken)
                with self.assertRaises(ValueError):
                    audited()
            save(events)
            (root / 'roster.json').write_text('[]')
            with self.assertRaises(ValueError):
                audited()

    def test_run_checks_the_actual_merged_reservation_before_host_or_media(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            round_tool.write(root / 'reservation-receipt.json', {'allocation': self.plan['allocation'],
                             'commit': 'a' * 40, 'manifest_sha256': 'b' * 64, 'count_per_class': 16, 'population': 'neutral',
                             'checker': {'path': 'scripts/check.py', 'sha256': 'c' * 64}})
            with mock.patch.object(round_tool, 'merged', return_value='a' * 40), \
                 mock.patch.object(round_tool, 'reservation', side_effect=ValueError('not merged')) as reserved, \
                 mock.patch.object(round_tool, 'execute') as child:
                with self.assertRaises(ValueError):
                    round_tool.run(root, self.plan, 'a' * 64)
                child.assert_not_called()
                reserved.assert_called_once()

    def test_reservation_requires_a_confined_tracked_manifest(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            for manifest in ('/outside.json', '../outside.json'):
                round_tool.write(root / 'reservation-checkout.json', {'checkout': name, 'manifest': manifest})
                with self.assertRaises(ValueError):
                    round_tool.reservation(root, self.plan)
            round_tool.write(root / 'reservation-checkout.json', {'checkout': name, 'manifest': 'untracked.json'})
            with mock.patch.object(round_tool, 'merged', return_value='a' * 40), \
                 mock.patch.object(round_tool.subprocess, 'run'), \
                 mock.patch.object(round_tool.subprocess, 'check_output', return_value=''):
                with self.assertRaises(ValueError):
                    round_tool.reservation(root, self.plan)

    def test_neutral_build_evidence_has_complete_equal_work(self):
        data = json.loads((ROOT / 'docs/design/evidence/h264-pgo-build.json').read_text())
        self.assertFalse(data['timed'])
        self.assertEqual(data['profile_calls'], 32)
        self.assertEqual(data['raw_profiles'], 32)
        self.assertFalse(data['build_script_functions'])
        self.assertEqual(data['pgo_sha256'], data['separate_host_rebuild_sha256'])
        self.assertEqual(data['pgo_sha256'], data['separate_target_rebuild_sha256'])
        self.assertEqual(data['historical_rebuilds'], [build_tool.HISTORICAL_BINARY] * 2)
        self.assertEqual(len(data['inputs']), 16)
        for row in data['inputs']:
            self.assertRegex(row['label'], '^t[0-9]{3}$')
            self.assertEqual(row['output_bytes'], row['output_frames'] * (row['width'] * row['height'] +
                             2 * ((row['width'] + 1) // 2) * ((row['height'] + 1) // 2)))
            self.assertEqual({(c['arm'], c['threads']) for c in row['calls']},
                             {(a, t) for a in ('base', 'pgo', 'ffmpeg') for t in (1, 8)})
            self.assertTrue(all(c['exit_code'] == 0 for c in row['calls']))


if __name__ == '__main__':
    unittest.main()
