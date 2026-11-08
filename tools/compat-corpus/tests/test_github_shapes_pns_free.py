"""Independent controls for fixture registration and PNS generation admission."""
import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import github_shapes as run
import github_shapes_fixtures as fixtures
import github_shapes_pns_gate as gate


class Recipes(unittest.TestCase):
    def test_original_recipe_retains_its_registered_bytes(self):
        encoded = (json.dumps(fixtures.recipe(fixtures.ORIGINAL), indent=2) + '\n').encode()
        self.assertEqual(hashlib.sha256(encoded).hexdigest(),
                         'a7cb02bfd0775ff560b5cb4ad52b69e9c3ca6261ef1dd2f075ef7254c461b8c8')

    def test_only_pns_encoder_switch_differs(self):
        original = fixtures.recipe(fixtures.ORIGINAL)
        changed = fixtures.recipe(fixtures.PNS_FREE)
        position = changed[0].index('-aac_pns')
        self.assertEqual(changed[0][position - 2:position + 2], ['-c:a', 'aac', '-aac_pns', '0'])
        del changed[0][position:position + 2]
        self.assertEqual(changed, original)

    def test_unknown_registration_creates_nothing(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / 'fixtures'
            execute = mock.Mock()
            with self.assertRaisesRegex(ValueError, 'fixture_registration'):
                fixtures.generate(target, '/unused', {}, execute, 'unknown')
            self.assertFalse(target.exists())
            execute.assert_not_called()


class FakeScan:
    def __init__(self, fault=None):
        self.fault = fault
        self.calls = []

    def __call__(self, argv, cwd, env, seconds, tag, stdin=None):
        argv = list(map(str, argv))
        self.calls.append(argv)
        output = b''
        code = 0
        if tag == 'carriage':
            content = Path(argv[argv.index('-i') + 1]).read_bytes()
            Path(argv[-1]).write_bytes(b'wrong' if self.fault == 'carriage_payload' else content)
            code = int(self.fault == 'carriage_failed')
        elif tag.endswith('probe'):
            packet = Path(argv[-1]).read_bytes()
            doc = {'streams': [{'index': 0, 'codec_type': 'audio', 'codec_name': 'aac', 'profile': 'LC',
                'sample_rate': '48000', 'channels': 1, 'nb_read_packets': '1',
                'extradata_hash': 'SHA256:' + 'a' * 64}], 'packets': [{'stream_index': 0,
                    'size': str(len(packet)), 'data_hash': 'SHA256:' + hashlib.sha256(packet).hexdigest()}]}
            if self.fault == 'packet_count':
                doc['streams'][0]['nb_read_packets'] = '2'
            if self.fault == 'extra_stream':
                doc['streams'].append(copy.deepcopy(doc['streams'][0]))
            if self.fault == 'carriage_configuration' and tag == 'carriage-probe':
                doc['streams'][0]['extradata_hash'] = 'SHA256:' + 'b' * 64
            output = json.dumps(doc).encode()
        else:
            metadata = {'schema': 'coreav.aac-pns-diagnostic/1', 'packets': 1, 'rate': 48000,
                'channels': 1, 'frames': [{'offset': 0, 'size': 4, 'channels': [{'noise': []}]}]}
            if self.fault == 'pns_present':
                metadata['frames'][0]['channels'][0]['noise'] = [{}]
            if self.fault == 'scan_incomplete':
                metadata['packets'] = 2
            if self.fault == 'missing_channel':
                metadata['frames'][0]['channels'] = []
            if self.fault == 'invalid_noise':
                metadata['frames'][0]['channels'][0]['noise'] = None
            if self.fault == 'diagnostic_packet':
                metadata['frames'][0]['offset'] = 1
            if self.fault == 'diagnostic_count':
                metadata['packets'] = 2
                metadata['frames'].append(copy.deepcopy(metadata['frames'][0]))
            if self.fault == 'diagnostic_rate':
                metadata['rate'] = 44100
            (Path(argv[-1]) / 'metadata.json').write_text(json.dumps(metadata))
            code = int(self.fault == 'scan_failed')
        (cwd / (tag + '.stdout')).write_bytes(output)
        (cwd / (tag + '.stderr')).write_bytes(b'')
        return {'args': argv, 'code': code, 'timed_out': False, 'stdin': stdin,
                'stdout': tag + '.stdout', 'stderr': tag + '.stderr'}


class Controls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.programs = {key: '/' + key[1:].lower() for key in ('$FFMPEG', '$FFPROBE', '$AAC_DIAGNOSTIC')}


class GateControls(Controls):
    def test_both_containers_have_complete_packet_bound_receipts(self):
        for name in ('source.mp4', 'source.mkv'):
            source = self.root / name
            source.write_bytes(b'data')
            execute = FakeScan()
            result = gate.scan_fixture(source, self.root / ('scan-' + name), self.programs, {}, execute)
            self.assertEqual((result['packets'], result['pns_bands']), (1, 0))
            self.assertEqual(result['fixture_sha256'], gate.sha(source))
            self.assertNotIn(str(self.root), json.dumps(result))
            if name.endswith('mkv'):
                self.assertIn(['-c:a', 'copy'], [call[i:i + 2] for call in execute.calls
                    for i in range(len(call) - 1)])

    def test_all_generation_faults_stop_before_any_shape(self):
        faults = {'pns_present': 'pns_present', 'scan_incomplete': 'scan_incomplete',
            'missing_channel': 'scan_incomplete', 'invalid_noise': 'scan_incomplete',
            'scan_failed': 'scan_failed', 'packet_count': 'packet_count',
            'extra_stream': 'configuration', 'carriage_failed': 'carriage',
            'carriage_payload': 'packet_binding', 'carriage_configuration': 'configuration',
            'diagnostic_packet': 'packet_binding', 'diagnostic_count': 'packet_count',
            'diagnostic_rate': 'configuration'}
        binary = self.root / 'binary'
        binary.write_bytes(b'fake executable')
        identity = {'coreav': {'sha256': run.digest(binary)}}
        build = self.root / 'build.json'
        run.write(build, {'commit': 'a' * 40, 'command': run.BUILD, 'coreav_sha256': run.digest(binary),
                          'lock_sha256': run.digest(run.ROOT / 'Cargo.lock')})

        def generate(folder, ffmpeg, env, execute, registration):
            folder.mkdir()
            for name in ('source.mp4', 'source.mkv'):
                (folder / name).write_bytes(b'data')

        for fault, reason in faults.items():
            with self.subTest(fault=fault):
                opts = types.SimpleNamespace(commit='a' * 40, registration=fixtures.PNS_FREE,
                    build_record=build, bundle=self.root / fault, **{name: str(binary)
                        for name in ('coreav', 'ffmpeg', 'ffprobe', 'x264', 'pns_diagnostic')})
                with mock.patch.object(run, 'merged_source'), \
                     mock.patch.object(run.platform, 'system', return_value='Linux'), \
                     mock.patch.object(run.platform, 'machine', return_value='x86_64'), \
                     mock.patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}), \
                     mock.patch.object(run, 'verify_programs', return_value=identity), \
                     mock.patch.object(fixtures, 'generate', side_effect=generate), \
                     mock.patch.object(run, 'execute', side_effect=FakeScan(fault)), \
                     mock.patch.object(run, 'acquire_one') as acquire, \
                     self.assertRaisesRegex(run.EvidenceError, 'fixture_generation:' + reason):
                    run.run(opts)
                acquire.assert_not_called()
                self.assertFalse((opts.bundle / 'calls').exists())


class ReplayRegistration(Controls):
    def prepare(self, registration):
        bundle = self.root / 'bundle'
        folder = bundle / 'fixtures'
        folder.mkdir(parents=True)
        for name in ('source.mp4', 'source.mkv'):
            (folder / name).write_bytes(b'data')
        run.write(folder / 'commands.json', fixtures.recipe(registration))
        self.rows = [{'rank': i, 'repository_count': 1271 if i == 200 else 1,
                      'shape': {'tool': 'ffmpeg'}} for i in range(1, 201)]
        run.write(bundle / 'records.json', [{'rank': i} for i in range(1, 201)])
        binary = self.root / 'fake-binary'
        binary.write_bytes(b'fake')
        self.identity = {'coreav': {'sha256': run.digest(binary)}}
        manifest = {'registration': registration, 'commit': 'a' * 40, 'features': run.FEATURES,
            'build_command': run.BUILD, 'method': {}, 'programs': self.identity,
            'build': {'commit': 'a' * 40, 'command': run.BUILD, 'coreav_sha256': run.digest(binary),
                      'lock_sha256': run.digest(run.ROOT / 'Cargo.lock')}}
        if registration == fixtures.PNS_FREE:
            manifest['pns_gate'] = {name: gate.scan_fixture(folder / name, bundle / 'pns-gate' / name,
                self.programs, {}, FakeScan()) for name in ('source.mp4', 'source.mkv')}
        manifest['inventory'] = run.inventory(bundle)
        run.write(bundle / 'manifest.json', manifest)
        return types.SimpleNamespace(bundle=bundle, registration=registration,
            scratch=self.root / 'scratch', summary=self.root / 'summary.json',
            **{name: str(binary) for name in ('coreav', 'ffmpeg', 'ffprobe', 'x264', 'pns_diagnostic')}), manifest

    def replay(self, opts):
        with mock.patch.object(run, 'method', return_value={}), \
             mock.patch.object(run, 'verify_programs', return_value=self.identity), \
             mock.patch.object(run.roster, 'load', return_value=self.rows), \
             mock.patch.object(run, 'judge', return_value='different'), \
             mock.patch.object(run, 'execute', side_effect=FakeScan()):
            return run.replay(opts)

    def test_replay_repeats_scan_and_names_the_registration(self):
        opts, _ = self.prepare(fixtures.PNS_FREE)
        self.assertEqual(self.replay(opts), 0)
        self.assertEqual(json.loads(opts.summary.read_text())['registration'], fixtures.PNS_FREE)
        self.assertTrue((opts.scratch / 'pns-gate/source.mkv/diagnostic/metadata.json').is_file())

    def test_recipe_substitution_is_refused_even_with_rebound_inventory(self):
        opts, manifest = self.prepare(fixtures.PNS_FREE)
        run.write(opts.bundle / 'fixtures/commands.json', fixtures.recipe(fixtures.ORIGINAL))
        manifest['inventory'] = run.inventory(opts.bundle)
        run.write(opts.bundle / 'manifest.json', manifest)
        with self.assertRaisesRegex(run.EvidenceError, 'fixture_recipe'):
            self.replay(opts)

    def test_missing_and_substituted_registration_are_refused(self):
        opts, manifest = self.prepare(fixtures.ORIGINAL)
        for value in (None, fixtures.PNS_FREE, 'unknown'):
            manifest['registration'] = value
            run.write(opts.bundle / 'manifest.json', manifest)
            with self.assertRaisesRegex(run.EvidenceError, 'registration'):
                self.replay(opts)

    def test_missing_gate_and_nonzero_retained_scan_are_refused(self):
        opts, manifest = self.prepare(fixtures.PNS_FREE)
        stored = copy.deepcopy(manifest['pns_gate'])
        del manifest['pns_gate']
        run.write(opts.bundle / 'manifest.json', manifest)
        with self.assertRaisesRegex(run.EvidenceError, 'fixture_generation:scan_incomplete'):
            self.replay(opts)
        opts.scratch = self.root / 'scratch-again'
        manifest['pns_gate'] = stored
        path = opts.bundle / 'pns-gate/source.mp4/diagnostic/metadata.json'
        metadata = json.loads(path.read_text())
        metadata['frames'][0]['channels'][0]['noise'] = [{}]
        run.write(path, metadata)
        manifest['inventory'] = run.inventory(opts.bundle)
        run.write(opts.bundle / 'manifest.json', manifest)
        with self.assertRaisesRegex(run.EvidenceError, 'fixture_generation:scan_incomplete'):
            self.replay(opts)

    def test_original_cannot_carry_a_pns_free_gate(self):
        opts, manifest = self.prepare(fixtures.ORIGINAL)
        manifest['pns_gate'] = {}
        run.write(opts.bundle / 'manifest.json', manifest)
        with self.assertRaisesRegex(run.EvidenceError, 'registration'):
            self.replay(opts)


    def test_changed_retained_carriage_is_refused_even_after_inventory_update(self):
        opts, manifest = self.prepare(fixtures.PNS_FREE)
        (opts.bundle / 'pns-gate/source.mkv/carriage.mp4').write_bytes(b'changed')
        manifest['inventory'] = run.inventory(opts.bundle)
        run.write(opts.bundle / 'manifest.json', manifest)
        with self.assertRaisesRegex(run.EvidenceError, 'fixture_generation:packet_binding'):
            self.replay(opts)

    def test_valid_but_substituted_metadata_cannot_replace_an_independent_scan(self):
        opts, manifest = self.prepare(fixtures.PNS_FREE)
        path = opts.bundle / 'pns-gate/source.mp4/diagnostic/metadata.json'
        metadata = json.loads(path.read_text())
        metadata['rate'] = 44100
        run.write(path, metadata)
        manifest['pns_gate']['source.mp4']['metadata_sha256'] = gate.sha(path)
        run.write(path.parents[1] / 'gate.json', manifest['pns_gate']['source.mp4'])
        manifest['inventory'] = run.inventory(opts.bundle)
        run.write(opts.bundle / 'manifest.json', manifest)
        with self.assertRaisesRegex(run.EvidenceError, 'fixture_generation:packet_binding'):
            self.replay(opts)


class WorkflowPrivacy(unittest.TestCase):
    def test_paired_run_stops_on_pns_free_failure_and_keeps_results_private(self):
        workflow = (run.ROOT / '.github/workflows/github-shapes.yml').read_text()
        measure = workflow.split('  measure:\n', 1)[1].split('  aac-controls:\n', 1)[0]
        self.assertNotIn('upload-artifact', measure)
        self.assertIn("if: github.event_name == 'workflow_dispatch'", measure)
        self.assertLess(measure.index('Require private evidence retention'), measure.index('Acquire and replay'))
        self.assertIn("if: always() && steps.acquire.outcome != 'skipped'", measure)
        self.assertIn('test_github_shapes_pns_free.py', workflow.split('  aac-controls:', 1)[1])
        step = measure.split('      - name: Acquire and replay all registered shapes\n', 1)[1]
        block = step.split('        run: |\n', 1)[1].split('      - name:', 1)[0]
        script = '\n'.join(line[10:] for line in block.splitlines())
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / 'bin'
            binary.mkdir()
            fake = binary / 'python3'
            fake.write_text('#!' + sys.executable + '\n' +
                'import os,sys\n'
                'if "--resolve" in sys.argv:\n'
                ' print("/oracle/ffmpeg\\n/oracle/ffprobe"); sys.exit(0)\n'
                'registration=sys.argv[sys.argv.index("--registration")+1]\n'
                'with open(os.environ["CALLS"],"a") as out: out.write(registration+"\\n")\n'
                'sys.exit(2 if os.environ.get("FAIL_REG") == registration else 0)\n')
            fake.chmod(0o755)
            for failure in (fixtures.PNS_FREE, ''):
                runner = root / ('failure' if failure else 'success')
                (runner / 'shape-evidence').mkdir(parents=True)
                calls = runner / 'calls'
                result = subprocess.run(['/bin/bash', '-c', script], cwd=root, capture_output=True,
                    timeout=10, env={'PATH': str(binary) + ':/bin:/usr/bin', 'RUNNER_TEMP': str(runner),
                        'COMMIT': 'a' * 40, 'CALLS': str(calls), 'FAIL_REG': failure})
                expected = [fixtures.PNS_FREE] if failure else [fixtures.PNS_FREE, fixtures.ORIGINAL]
                self.assertEqual(calls.read_text().splitlines(), expected, result.stderr)
                self.assertEqual(result.returncode, 2 if failure else 0, result.stderr)
                self.assertEqual((runner / 'shape-evidence/completion.json').exists(), not failure)
        for key in ('R2_ENDPOINT', 'R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_BUCKET'):
            self.assertNotIn(key, run.environment())


@unittest.skipUnless(os.environ.get('COREAV_SHAPES_AAC_DIAGNOSTIC') and os.environ.get('COREAV_SHAPES_ORACLE'),
                     'pinned PNS controls run in dedicated Linux CI')
class PinnedPinkNoise(unittest.TestCase):
    def test_default_has_pns_and_disabled_has_none_in_both_containers(self):
        programs = {'$FFMPEG': str(Path(os.environ['COREAV_SHAPES_ORACLE'], 'ffmpeg')),
                    '$FFPROBE': str(Path(os.environ['COREAV_SHAPES_ORACLE'], 'ffprobe')),
                    '$AAC_DIAGNOSTIC': os.environ['COREAV_SHAPES_AAC_DIAGNOSTIC']}
        import test_oracle
        test_oracle.require(programs['$FFMPEG'], programs['$FFPROBE'])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for disabled in (False, True):
                folder = root / ('off' if disabled else 'default')
                folder.mkdir()
                args = [programs['$FFMPEG'], '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                    'anoisesrc=c=pink:a=0.25:r=48000:d=2:seed=1', '-c:a', 'aac',
                    *(['-aac_pns', '0'] if disabled else []), 'source.mp4']
                self.assertEqual(run.execute(args, folder, run.environment(), 120, 'encode')['code'], 0)
                self.assertEqual(run.execute([programs['$FFMPEG'], '-nostdin', '-v', 'error',
                    '-i', 'source.mp4', '-c', 'copy', 'source.mkv'], folder, run.environment(),
                    120, 'copy')['code'], 0)
                for name in ('source.mp4', 'source.mkv'):
                    target = folder / ('scan-' + name)
                    if disabled:
                        receipt = gate.scan_fixture(folder / name, target, programs, run.environment(), run.execute)
                        self.assertGreater(receipt['packets'], 0)
                        self.assertEqual(receipt['pns_bands'], 0)
                        repeated = gate.scan_fixture(folder / name, folder / ('repeat-' + name),
                            programs, run.environment(), run.execute)
                        self.assertEqual(repeated, receipt)
                    else:
                        with self.assertRaisesRegex(gate.FixtureError, 'pns_present'):
                            gate.scan_fixture(folder / name, target, programs, run.environment(), run.execute)


if __name__ == '__main__':
    unittest.main()
