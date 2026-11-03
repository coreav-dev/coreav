import argparse
import copy
import gzip
import hashlib
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import vp9_pixel_round as run
import vp9_pixel_build as build
import vp9_pixel_profiles as profiles

CANDIDATE = '1' * 40


def fixture(root):
    registration = run.plan(SCRIPTS.parent)
    for index, row in enumerate(registration['inputs']):
        data = f'synthetic input {index}'.encode()
        row['sha256'] = hashlib.sha256(data).hexdigest()
        path = root / 'files' / (row['sha256'] + '.webm')
        path.parent.mkdir(exist_ok=True)
        path.write_bytes(data)
    (root / run.PLAN_PATH).parent.mkdir(parents=True)
    run.write(root / run.PLAN_PATH, registration)
    recipe = run.read(SCRIPTS.parent / run.RECIPE_PATH)
    recipe['reviewed_plan_sha256'] = run.digest(root / run.PLAN_PATH)
    run.write(root / run.RECIPE_PATH, recipe)
    (root / 'scripts').mkdir()
    for name in run.CAPTURE_SCRIPTS:
        shutil.copy2(SCRIPTS / name, root / 'scripts' / name)
    (root / 'bin').mkdir()
    for name in registration['arms'] + ['ffprobe']:
        (root / 'bin' / name).write_text('synthetic binary ' + name)
    receipt = dict(repository='coreav-dev/coreav', number=340,
                   merge_commit_sha=CANDIDATE, merged_at='2026-11-03T00:00:00Z')
    native = {}
    for arm in registration['profiles']['native_arms']:
        features = ['coreav-decode-oracle/native-vp9']
        if arm == 'merged-safe-dist':
            features.append(registration['native_safe_feature'])
        native[arm] = dict(commit=registration['baseline_commit'] if arm == 'baseline-dist' else CANDIDATE,
                           profile='dist', features=features, adapter_sha256='a' * 64)
    builds = dict(candidate=CANDIDATE, baseline=registration['baseline_commit'], merge_receipt=receipt,
                  plan_sha256=run.digest(root / run.PLAN_PATH), recipe_sha256=run.digest(root / run.RECIPE_PATH),
                  reviewed_plan_commit=recipe['reviewed_plan_commit'], native=native,
                  profile_overrides=recipe['native_profile_overrides'], source_deleted_before_profiling=True,
                  oracle_pin_sha256=run.digest(root / 'scripts/oracle-ffmpeg.env'),
                  symbol_source=dict(commit=recipe['ffmpeg_source_commit'],
                                     archive_sha256=recipe['ffmpeg_source_archive_sha256'], debug_sections=False),
                  scripts={n: run.digest(root / 'scripts' / n) for n in run.CAPTURE_SCRIPTS},
                  binaries={n: run.digest(root / 'bin' / n) for n in registration['arms'] + ['ffprobe']})
    run.write(root / 'builds.json', builds)
    run.write(root / 'files/files.json', [dict(sha256=r['sha256'], ext='webm') for r in registration['inputs']])
    return registration, receipt, builds


def hashes(width, height, loops=1):
    lines = [f'#dimensions 0: {width}x{height}']
    lines += [f'0, {i}, {i}, 1, 1, {i % 120:032x}' for i in range(120 * loops)]
    return ('\n'.join(lines) + '\n').encode()


def native(frames=None):
    return json.dumps(dict(decoder='coreav vp9, scalar profile 0', codec='vp9', fed=120,
                           cutoff=None, frames=frames if frames is not None else [None] * 120)).encode()


def evidence(directory, registration, receipt, builds):
    directory.mkdir()
    run.write(directory / 'complete.json', dict(complete=True))
    run.write(directory / 'header.json', dict(plan=registration, builds=builds, merge_receipt=receipt))
    rows = []

    def emit(kind, data=b'', errors=b'', **fields):
        prefix = directory / f'call-{len(rows):04}'
        pathlib.Path(str(prefix) + '.stdout').write_bytes(data)
        pathlib.Path(str(prefix) + '.stderr').write_bytes(errors)
        run.write(str(prefix) + '.command.json', ['synthetic', kind])
        rows.append(dict(kind=kind, rc=0, unexpected_stderr=False, **fields))

    def decode(kind, index, arm, round_number=None):
        is_native = arm in registration['profiles']['native_arms']
        loops = 1 if is_native else registration['ffmpeg_loops'][registration['inputs'][index]['class']]
        data = native() if is_native else f'frame={120 * loops}\n'.encode()
        fields = dict(input=index, arm=arm, loops=loops, frames=120 * loops, round=round_number)
        if kind == 'timed':
            user = {'baseline-dist': 4, 'merged-safe-dist': 2, 'merged-auto-dist': 1,
                    'ffmpeg-pinned': 0.5, 'ffmpeg-symbols': 0.55}[arm]
            fields.update(user=user * loops, wall=user * 2 * loops, system=0)
        emit(kind, data, **fields)

    emit('counter-preflight', errors=b'  1,000 cycles:u\n')
    for i, row in enumerate(registration['inputs']):
        emit('prepare', input=i)
        packets = json.dumps(dict(packets=[dict(size='5', data_hash='SHA256:' + 'b' * 64)] * 120)).encode()
        emit('packet-binding', packets, input=i, original=True)
        emit('packet-binding', packets, input=i, original=False)
        emit('gold', hashes(row['width'], row['height']), input=i)
        frames = [dict(md5=f'{n:032x}', width=row['width'], height=row['height'], pts=n) for n in range(120)]
        for arm in registration['profiles']['native_arms']:
            emit('pixel-preflight', native(frames), input=i, arm=arm)
        for arm in ('ffmpeg-pinned', 'ffmpeg-symbols'):
            for loops in (1, 2):
                emit('repeat-preflight', hashes(row['width'], row['height'], loops), input=i, arm=arm, loops=loops)
            decode('work-preflight', i, arm)
    for i in range(6):
        for arm in registration['arms']:
            decode('warmup', i, arm)
        for round_number in range(5):
            for arm in run.order(registration, i, round_number):
                decode('timed', i, arm, round_number)
    for i in range(6):
        for arm in registration['profiles']['native_arms'] + ['ffmpeg-symbols']:
            decode('profile', i, arm)
            symbol = 'ff_vp9_put_8tap' if arm == 'ffmpeg-symbols' else 'coreav_kernels_vp9::inter::predict_inter'
            graph = f'decode 12/12 1.0: 100 cycles:u:\n  1234 {symbol} (binary)\n\n'.encode()
            emit('profile-script', graph, input=i, arm=arm)
            if arm != 'ffmpeg-symbols':
                emit('demangle', graph, input=i, arm=arm)
            flat = b'# Total Lost Samples: 0\n# Event count (approx.): 100\n'
            emit('profile-report', flat, input=i, arm=arm)
            stem = directory / f'profile-{i}-{arm}'
            with gzip.open(str(stem) + '.gz', 'wb') as stream:
                stream.write(graph)
            pathlib.Path(str(stem) + '.flat').write_bytes(flat)
            run.write(str(stem) + '.quality.json', dict(record_diagnostics='', script_diagnostics='', lost=0))
    run.write(directory / 'calls.json', rows)
    return rows


class Merge(unittest.TestCase):
    def test_only_the_actual_merged_commit_is_accepted(self):
        registration = run.plan(SCRIPTS.parent)
        value = dict(merged=True, state='closed', merge_commit_sha=CANDIDATE, merged_at='date', number=340,
                     base=dict(ref='main', repo=dict(full_name='coreav-dev/coreav')))
        cases = [dict(), dict(merged=False), dict(state='open'), dict(merge_commit_sha='2' * 40),
                 dict(number=341), dict(merged_at=None), dict(base=dict(ref='other'))]
        for change in cases:
            with self.subTest(change=change), mock.patch.object(run.urllib.request, 'urlopen',
                    return_value=io.BytesIO(json.dumps(value | change).encode())) as network:
                if change:
                    with self.assertRaises(ValueError):
                        run.merge_receipt(registration, CANDIDATE)
                else:
                    self.assertEqual(run.merge_receipt(registration, CANDIDATE)['merge_commit_sha'], CANDIDATE)
                self.assertEqual(network.call_args.kwargs['timeout'], 30)
        with mock.patch.object(run.urllib.request, 'urlopen') as network:
            for candidate in ('main', '1' * 39, CANDIDATE + '\n'):
                with self.assertRaises(ValueError):
                    run.merge_receipt(registration, candidate)
            network.assert_not_called()

    def test_duplicate_keys_and_oversized_receipts_fail_closed(self):
        for payload in (b'{"merged":false,"merged":true}', b' ' * ((2 << 20) + 1)):
            with mock.patch.object(run.urllib.request, 'urlopen', return_value=io.BytesIO(payload)):
                with self.assertRaises(ValueError):
                    run.merge_receipt(run.plan(SCRIPTS.parent), CANDIDATE)

    def test_unmerged_build_and_acquisition_cannot_launch_work(self):
        args = argparse.Namespace(candidate=CANDIDATE, out='unused')
        with mock.patch.object(build.sys, 'platform', 'linux'), mock.patch.object(build.platform, 'machine',
                return_value='x86_64'), mock.patch.object(run, 'merge_receipt', side_effect=ValueError), \
                mock.patch.object(build.subprocess, 'run') as child:
            with self.assertRaises(ValueError):
                build.build(args)
            child.assert_not_called()
        with mock.patch.object(sys, 'argv', ['driver', 'fetch', '--candidate', CANDIDATE, '--media', 'unused']), \
                mock.patch.object(run, 'merge_receipt', side_effect=ValueError), \
                mock.patch.object(build.ceiling, 'fetch') as fetch, mock.patch('sys.stderr', new_callable=io.StringIO):
            self.assertEqual(run.main(), 1)
            fetch.assert_not_called()

    def test_acquisition_passes_only_the_six_registered_originals_to_the_bounded_owner(self):
        with mock.patch.object(sys, 'argv', ['driver', 'fetch', '--candidate', CANDIDATE, '--media', 'unused']), \
                mock.patch.object(run, 'merge_receipt'), mock.patch.object(build.ceiling, 'fetch') as fetch:
            self.assertEqual(run.main(), 0)
            self.assertEqual(fetch.call_args.kwargs['wanted'], run.plan(SCRIPTS.parent)['inputs'])

    def test_full_roster_acquisition_shares_finite_caps_and_restores_the_pool_after_failure(self):
        wanted = run.plan(SCRIPTS.parent)['inputs']
        entries = [dict(row, set='real', ext='webm', size=1, key=f'synthetic-{i}') for i, row in enumerate(wanted)]
        original = object()
        owner = argparse.Namespace(pool=original, manifest=mock.Mock(return_value=entries),
                                   fetch=mock.Mock(side_effect=ValueError('synthetic failure')))
        limits = argparse.Namespace(requests=4, bytes=12)
        pool = argparse.Namespace(Limits=mock.Mock(return_value=limits))
        with tempfile.TemporaryDirectory() as temporary, mock.patch.dict(sys.modules,
                {'round_files': owner, 'r2pool': pool}), mock.patch.object(build.ceiling.sys, 'platform', 'linux'):
            directory = pathlib.Path(temporary) / 'media'
            with self.assertRaises(ValueError):
                build.ceiling.fetch(argparse.Namespace(media=directory), wanted=wanted)
            self.assertIs(owner.pool, original)
            self.assertEqual(pool.Limits.call_args.kwargs, dict(requests=5000, bytes_=896 << 20))
            self.assertEqual(run.read(directory / 'acquisition.json')['requests_reserved'], 4)
            self.assertEqual([r['sha256'] for r in owner.fetch.call_args.args[0]], [r['sha256'] for r in wanted])


class Capture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.registration, self.receipt, self.builds = fixture(self.root)
        for patch in (mock.patch.object(run, 'ROOT', self.root),
                      mock.patch.object(run, 'merge_receipt', return_value=self.receipt),
                      mock.patch.object(run, 'host', return_value=dict(receipt='synthetic'))):
            patch.start()
            self.addCleanup(patch.stop)
        self.args = argparse.Namespace(root=self.root, out=self.root / 'out', candidate=CANDIDATE, max_seconds=60)

    def construct(self):
        capture = run.Capture(self.args)
        self.addCleanup(capture.lock.close)
        return capture

    def test_valid_binding_is_exclusive_even_after_the_lock_is_released(self):
        capture = self.construct()
        capture.lock.close()
        self.args.out = self.root / 'other'
        with self.assertRaises(FileExistsError):
            self.construct()
        self.assertEqual(run.read(self.args.out / 'complete.json'), dict(complete=False))

    def test_unstaged_tooling_refusal_retains_an_incomplete_marker(self):
        self.args.root = self.root / 'different'
        with self.assertRaises(ValueError):
            self.construct()
        self.assertEqual(run.read(self.args.out / 'complete.json'), dict(complete=False))

    def test_changed_inputs_binary_scripts_plan_or_recipe_are_rejected(self):
        paths = [self.root / 'bin/merged-auto-dist', self.root / 'scripts/vp9_pixel_profiles.py',
                 next((self.root / 'files').glob('*.webm')), self.root / run.PLAN_PATH, self.root / run.RECIPE_PATH]
        for index, path in enumerate(paths):
            old = path.read_bytes()
            path.write_bytes(old + b' ')
            self.args.out = self.root / f'bad-{index}'
            with self.subTest(path=path.name), self.assertRaises(ValueError):
                self.construct()
            self.assertEqual(run.read(self.args.out / 'complete.json'), dict(complete=False))
            path.write_bytes(old)

    def test_source_features_and_symbol_source_must_match_the_build_recipe(self):
        for index, mutate in enumerate((
                lambda b: b['native']['merged-auto-dist'].update(commit='2' * 40),
                lambda b: b['native']['merged-safe-dist'].update(features=[]),
                lambda b: b['native']['merged-auto-dist'].update(adapter_sha256='b' * 64),
                lambda b: b['symbol_source'].update(debug_sections=True),
                lambda b: b.update(merge_receipt={}),
                lambda b: b.update(scripts={}),
                lambda b: b.update(source_deleted_before_profiling=False))):
            changed = copy.deepcopy(self.builds)
            mutate(changed)
            run.write(self.root / 'builds.json', changed)
            self.args.out = self.root / f'bad-{index}'
            with self.subTest(index=index), self.assertRaises(ValueError):
                self.construct()

    def test_timeout_kills_the_process_group_and_retains_an_incomplete_capture(self):
        capture = self.construct()
        capture.deadline = time.monotonic() + 0.05
        with mock.patch.object(run, 'bounds'), self.assertRaises(subprocess.TimeoutExpired):
            capture.child([sys.executable, '-I', '-c', 'import time; time.sleep(10)'], dict(kind='probe'))
        self.assertTrue(run.read(self.args.out / 'calls.json')[0]['interrupted'])
        self.assertEqual(run.read(self.args.out / 'complete.json'), dict(complete=False))

    def test_foreign_source_locations_are_never_retained(self):
        capture = self.construct()
        program = 'print("  /synthetic directory/foreign.c:12 (inlined)")'
        with mock.patch.object(run, 'bounds'), self.assertRaises(ValueError):
            capture.child([sys.executable, '-I', '-c', program], dict(kind='profile-script'), source_blind=True)
        self.assertTrue((self.args.out / 'source-location-fault.json').exists())
        self.assertFalse(list(self.args.out.glob('*.stdout')))

    def test_failed_preparation_removes_partial_clips(self):
        capture = self.construct()

        def fail():
            (self.root / 'clips').mkdir()
            (self.root / 'clips/partial').write_text('synthetic')
            raise ValueError('preflight failed')

        with mock.patch.object(capture, 'prepare', side_effect=fail), self.assertRaises(ValueError):
            capture.run()
        self.assertFalse((self.root / 'clips').exists())
        self.assertEqual(run.read(self.args.out / 'complete.json'), dict(complete=False))


class Replay(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.registration, self.receipt, self.builds = fixture(self.root)
        patch = mock.patch.object(run, 'ROOT', self.root)
        patch.start()
        self.addCleanup(patch.stop)
        self.directory = self.root / 'out'
        self.rows = evidence(self.directory, self.registration, self.receipt, self.builds)
        self.args = argparse.Namespace(out=self.directory, summary=self.root / 'summary.json')

    def test_complete_replay_recomputes_paired_gains_normalization_and_profile_totals(self):
        run.replay(self.args)
        value = run.read(self.args.summary)
        self.assertEqual(value['classes']['720p']['user']['total_gain'], 4)
        self.assertEqual(value['classes']['720p']['user']['scalar_gain'], 2)
        self.assertEqual(value['classes']['720p']['user']['simd_increment'], 2)
        self.assertEqual(value['classes']['720p']['user']['remaining_ffmpeg_ratio'], 2)
        self.assertFalse(value['classes']['720p']['prediction_matched'])
        self.assertTrue(value['classes']['1080p']['prediction_matched'])
        self.assertAlmostEqual(value['inputs'][0]['symbol_build_drift'], 1.1)
        self.assertEqual(value['inputs'][0]['paired_ranges']['user']['total_gain'], [4, 4])
        self.assertFalse(value['performance_vote'])
        self.assertEqual(value['inputs'][0]['profiles']['merged-auto-dist']['components']['interpolation']['estimated_user'], 1)
        self.assertEqual(value['inputs'][0]['profiles']['ffmpeg-symbols']['components']['interpolation']['estimated_user'], 0.5)
        first = self.args.summary.read_bytes()
        run.replay(self.args)
        self.assertEqual(self.args.summary.read_bytes(), first)

    def test_missing_duplicate_reordered_and_failed_work_cannot_be_complete(self):
        mutations = [lambda r: r.pop(next(i for i, x in enumerate(r) if x['kind'] == 'pixel-preflight')),
                     lambda r: r.append(copy.deepcopy(next(x for x in r if x['kind'] == 'profile'))),
                     lambda r: r.reverse(),
                     lambda r: next(x for x in r if x['kind'] == 'timed').update(frames=119),
                     lambda r: next(x for x in r if x['kind'] == 'timed').update(user=float('nan')),
                     lambda r: next(x for x in r if x['kind'] == 'profile').update(rc=1)]
        for mutate in mutations:
            rows = copy.deepcopy(self.rows)
            mutate(rows)
            run.write(self.directory / 'calls.json', rows)
            with self.subTest(mutation=mutations.index(mutate)), self.assertRaises(ValueError):
                run.replay(self.args)
            self.assertFalse(run.read(self.args.summary)['complete'])

    def test_raw_pixel_work_and_packet_corruption_are_rechecked(self):
        for kind in ('pixel-preflight', 'repeat-preflight', 'packet-binding', 'timed'):
            index = next(i for i, row in enumerate(self.rows) if row['kind'] == kind)
            path = self.directory / f'call-{index:04}.stdout'
            before = path.read_bytes()
            if kind == 'repeat-preflight':
                path.write_bytes(before.replace(b'00000000000000000000000000000001', b'f' * 32, 1))
            elif kind == 'packet-binding':
                path.write_bytes(b'{"packets": []}')
            else:
                value = json.loads(before)
                if kind == 'pixel-preflight':
                    value['frames'][0]['md5'] = 'f' * 32
                else:
                    value['fed'] = 119
                path.write_text(json.dumps(value))
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                run.replay(self.args)
            path.write_bytes(before)

    def test_missing_profiles_changed_quality_and_stale_source_binding_are_rejected(self):
        path = self.directory / 'profile-0-merged-auto-dist.quality.json'
        run.write(path, dict(lost=0))
        with self.assertRaises(ValueError):
            run.replay(self.args)
        run.write(path, dict(record_diagnostics='', script_diagnostics='', lost=0))
        graph = self.directory / 'profile-0-merged-auto-dist.gz'
        graph.unlink()
        with self.assertRaises(FileNotFoundError):
            run.replay(self.args)
        header = run.read(self.directory / 'header.json')
        header['builds']['candidate'] = '2' * 40
        run.write(self.directory / 'header.json', header)
        with self.assertRaises(ValueError):
            run.replay(self.args)

    def test_order_balances_every_arm_in_every_position(self):
        arms = self.registration['arms']
        for index in range(6):
            rounds = [run.order(self.registration, index, r) for r in range(5)]
            for position in range(5):
                self.assertEqual(sorted(row[position] for row in rounds), sorted(arms))

    def test_profile_period_sum_must_reconcile_with_the_independent_flat_counter(self):
        index = next(i for i, row in enumerate(self.rows)
                     if row['kind'] == 'profile-report' and row['input'] == 0 and row['arm'] == 'merged-auto-dist')
        for path in (self.directory / f'call-{index:04}.stdout',
                     self.directory / 'profile-0-merged-auto-dist.flat'):
            path.write_bytes(b'# Total Lost Samples: 0\n# Event count (approx.): 101\n')
        with self.assertRaises(ValueError):
            run.replay(self.args)

    def test_preflights_warmups_and_profiles_cannot_move_across_timing(self):
        for kind in ('packet-binding', 'counter-preflight', 'warmup', 'profile'):
            changed = copy.deepcopy(self.rows)
            position = next(i for i, row in enumerate(changed) if row['kind'] == kind)
            row = changed.pop(position)
            if kind == 'profile':
                changed.insert(next(i for i, item in enumerate(changed) if item['kind'] == 'timed'), row)
            else:
                changed.append(row)
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                run.validate_calls(changed, self.registration)


class WorkAndProfiles(unittest.TestCase):
    def test_host_checks_live_machine_metadata_smt_avx2_and_noncoerced_provider_facts(self):
        registration = run.plan(SCRIPTS.parent)
        expected = registration['host']
        receipt = dict(machine_type=expected['machine'], provisioning=expected['provisioning'],
                       cpu_platform=expected['cpu_platform'], advanced_machine_features=dict(
                       threadsPerCore=1, turboMode='ALL_CORE_MAX', performanceMonitoringUnit='STANDARD'))
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            original_read = pathlib.Path.read_text

            def text(path, *args, **kwargs):
                if str(path) == '/sys/devices/system/cpu/smt/active':
                    return '0'
                if str(path) == '/proc/cpuinfo':
                    return 'flags : avx2 sse2\n'
                return original_read(path, *args, **kwargs)

            for invalid in (None, 'spot', 'boolean-threads', 'wrong-live-machine'):
                changed = copy.deepcopy(receipt)
                if invalid == 'spot':
                    changed['provisioning'] = 'SPOT'
                if invalid == 'boolean-threads':
                    changed['advanced_machine_features']['threadsPerCore'] = True
                run.write(root / 'instance.json', changed)
                response = io.BytesIO(b'other-machine' if invalid == 'wrong-live-machine' else b'c4-standard-16')
                response.headers = {'Metadata-Flavor': 'Google'}
                with mock.patch.object(run.sys, 'platform', 'linux'), \
                        mock.patch.object(pathlib.Path, 'read_text', text), \
                        mock.patch.object(run.urllib.request, 'urlopen', return_value=response), \
                        mock.patch.object(run.os, 'sched_getaffinity', return_value={0}, create=True):
                    if invalid:
                        with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                            run.host(root, registration)
                    else:
                        self.assertEqual(run.host(root, registration)['receipt'], receipt)

    def test_build_environment_rejects_hidden_overrides_and_configurations(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            with mock.patch.dict(os.environ, {'HOME': temporary, 'PATH': '/synthetic'}, clear=True), \
                    mock.patch.object(build, 'ROOT', root):
                recipe = dict(native_profile_overrides={'CARGO_PROFILE_DIST_DEBUG': '2',
                                                       'CARGO_PROFILE_DIST_STRIP': 'none'})
                self.assertEqual(build.cargo_environment(recipe)['CARGO_PROFILE_DIST_DEBUG'], '2')
                for variable in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
                                 'CARGO_PROFILE_DIST_LTO', 'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS'):
                    with mock.patch.dict(os.environ, {variable: 'unrecorded'}), self.assertRaises(ValueError):
                        build.cargo_environment(recipe)
                (root / '.cargo').mkdir()
                (root / '.cargo/config').write_text('[build]\nrustflags=["-Ctarget-cpu=native"]\n')
                with self.assertRaises(ValueError):
                    build.cargo_environment(recipe)

    def test_an_unsupported_or_uncounted_counter_is_not_a_successful_preflight(self):
        run.counter_available(b'1,234 cycles:u\n')
        for data in (b'', b'<not supported> cycles:u\n', b'<not counted> cycles:u\n', b'0 cycles:u\n'):
            with self.assertRaises(ValueError):
                run.counter_available(data)

    def test_decoder_commands_keep_native_selection_affinity_and_single_thread_oracle(self):
        command = run.ffmpeg_command(pathlib.Path('/bundle'), 'ffmpeg-pinned', pathlib.Path('/input'), 80)
        for key, value in (('-c:v', 'vp9'), ('-threads', '1'), ('-stream_loop', '79'),
                           ('-frames:v', '9600'), ('-enc_time_base:v', 'demux')):
            self.assertEqual(command[command.index(key) + 1], value)
        self.assertEqual(command[:3], ['taskset', '-c', '0'])
        command = run.native_command(pathlib.Path('/bundle'), 'merged-auto-dist', pathlib.Path('/input'))
        self.assertEqual(command[command.index('--backend') + 1], 'native')
        self.assertNotIn('COREAV_PIXEL_HASHES=1', command)
        self.assertIn('COREAV_PIXEL_HASHES=1', run.native_command(pathlib.Path('/bundle'), 'merged-auto-dist',
                                                                 pathlib.Path('/input'), True))

    @unittest.skipUnless(os.environ.get('COREAV_PIXEL_BUILD_CONTRACT') == '1', 'hosted compile-only contract')
    def test_native_adapter_compiles_for_each_source_feature_and_profile(self):
        registration = run.plan(SCRIPTS.parent)
        recipe = run.read(SCRIPTS.parent / run.RECIPE_PATH)
        env = build.cargo_environment(recipe)
        with tempfile.TemporaryDirectory(prefix='pixel-adapter-contract-') as temporary:
            for arm in registration['profiles']['native_arms']:
                revision = registration['baseline_commit'] if arm == 'baseline-dist' else 'HEAD'
                build.native_snapshot(pathlib.Path(temporary), arm, revision, registration, env, check_only=True)

    def test_work_counts_decoder_identity_and_hash_mode_are_exact(self):
        run.work(native(), True, 120)
        for change in (dict(fed=119), dict(codec='h264'), dict(cutoff='limit'), dict(frames=[None] * 119),
                       dict(frames=[dict(md5='a' * 32)] * 120)):
            with self.subTest(change=list(change)), self.assertRaises(ValueError):
                run.work(json.dumps(json.loads(native()) | change).encode(), True, 120)
        run.work(b'frame=240\n', False, 240)
        with self.assertRaises(ValueError):
            run.work(b'frame=120\n', False, 240)

    def test_exclusive_owner_skips_generic_type_arguments_and_sums_periods_once(self):
        text = ('decode 1/1 1.0: 70 cycles:u:\n'
                '  1234 core::ptr::drop_in_place<coreav_kernels_vp9::inter::ReferencePlane> (binary)\n'
                '  2345 coreav_vp9[12345678]::picture::Planes<u8>::picture (binary)\n\n'
                'decode 1/1 2.0: 30 cycles:u:\n  9999 [unknown] ([unknown])\n')
        with tempfile.TemporaryDirectory() as temporary:
            path = pathlib.Path(temporary) / 'profile.gz'
            with gzip.open(path, 'wt') as stream:
                stream.write(text)
            value = profiles.analyse(path, True, 10)
        self.assertEqual(value['period'], 100)
        self.assertEqual(value['samples'], 2)
        self.assertEqual(value['components']['reference_packing']['estimated_user'], 7)
        self.assertEqual(value['components']['interpolation']['period'], 0)
        self.assertEqual(value['components']['unresolved']['period'], 30)
        self.assertEqual(value['unknown_leaf_share'], 0.3)
        self.assertEqual(value['leaf_only_share'], 0.3)
        self.assertEqual(sum(c['period'] for c in value['components'].values()), value['period'])

    def test_foreign_locations_empty_and_nonpositive_profiles_are_refused(self):
        for text in ('', 'decode 1/1 1.0: 0 cycles:u:\n  1234 ff_vp9_put_8tap (binary)\n',
                     'decode 1/1 1.0: 5 cycles:u:\n  /synthetic directory/test.c:10 (inlined)\n'):
            with tempfile.TemporaryDirectory() as temporary:
                path = pathlib.Path(temporary) / 'profile.gz'
                with gzip.open(path, 'wt') as stream:
                    stream.write(text)
                with self.subTest(text=text), self.assertRaises(ValueError):
                    profiles.analyse(path, False, 1)

    def test_lost_chunks_are_not_misreported_as_lost_sample_counts(self):
        record = b'[ perf record: Woken up 1510 times to write data ]\n'
        warning = b'Warning:\nProcessed 13657 events and lost 1 chunks!\n\nCheck IO/CPU overload!\n'
        self.assertEqual(profiles.lost(record + warning, warning, b'# Total Lost Samples: 30\n'), 30)
        self.assertEqual(profiles.lost(record, b'', b'# Total Lost Samples: 0\n'), 0)
        for flat in (b'', b'# Total Lost Samples: 0\n', b'# Total Lost Samples: 30\n# Total Lost Samples: 30\n'):
            with self.assertRaises(ValueError):
                profiles.lost(record, warning, flat)
        with self.assertRaises(ValueError):
            profiles.lost(b'unexpected diagnostic', b'', b'# Total Lost Samples: 0\n')

    def test_child_environment_excludes_credentials_and_build_overrides(self):
        with mock.patch.dict(os.environ, dict(R2_SECRET_ACCESS_KEY='marker', CAPY_API_KEY='marker',
                                             COREAV_PIXEL_HASHES='1', RUSTFLAGS='-Ctarget-cpu=native')):
            self.assertLessEqual(set(run.environment()), {'PATH', 'HOME', 'TMPDIR'})

    def test_native_profile_contract_rejects_optimizer_and_hierarchical_overrides(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = pathlib.Path(temporary)
            manifest = source / 'Cargo.toml'
            text = '[profile.dist]\ninherits="release"\nlto="fat"\ncodegen-units=1\n'
            manifest.write_text(text)
            build.profile_contract(source)
            manifest.write_text(text + 'opt-level=0\n')
            with self.assertRaises(ValueError):
                build.profile_contract(source)
            manifest.write_text(text)
            (source / '.cargo').mkdir()
            (source / '.cargo/config.toml').write_text('[build]\nrustflags=["-Ctarget-cpu=native"]\n')
            with self.assertRaises(ValueError):
                build.profile_contract(source)
