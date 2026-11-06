"""Keep the AV1 experiment out of default and shipped configurations."""

import os
import copy
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('av1_reference', ROOT / 'scripts/av1_process_reference.py')
reference = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(reference)


class ReferenceReplay(unittest.TestCase):
    def fixture(self):
        data = {'inputs': [{'label': 'sample', 'frames': 100, 'geometry': [320, 180]}], 'calls': []}
        for round_ in range(3):
            for arm, elapsed in zip(reference.ARMS, (12, 11, 10)):
                data['calls'].append({'input': 'sample', 'round': round_, 'arm': arm,
                    'seconds': elapsed + round_ * 0.1, 'metric_frames': 100, 'psnr_db': 40,
                    'probe': {'frames': 100, 'width': 320, 'height': 180,
                              'pixel_format': 'yuv420p', 'cadence': '25/1', 'payload_bytes': 10000},
                    'decode': {'frames': 100},
                    'encoder_observation': {'sessions': 1, 'version': 'v4.2.0', 'gop_frames': 126, 'parallelism_level': 1},
                    'receipt': {'video': {'backend': 'native', 'codec': 'h264', 'frames': 100,
                        'encoding': {'segments': 1, 'keyint': 125, 'isolation': 'sandboxed' if arm == 'sandbox' else 'in-process-measurement'}}}})
        return data

    def test_medians_spread_and_signed_decomposition(self):
        result = reference.summarize(self.fixture())
        row = result['inputs'][0]
        self.assertEqual(row['median_seconds']['sandbox'], 12.1)
        self.assertEqual(row['range_seconds']['sandbox'], [12, 12.2])
        self.assertAlmostEqual(row['boundary_share'], 0.5)
        self.assertAlmostEqual(result['aggregate']['boundary_seconds'], 1)
        data = self.fixture()
        for call in data['calls']:
            if call['arm'] == 'in_process':
                call['seconds'] += 3
        self.assertAlmostEqual(reference.summarize(data)['inputs'][0]['boundary_share'], -1)

    def test_incomplete_duplicate_wrong_work_and_equivalence_failures_refuse(self):
        mutations = [lambda data: data['calls'].pop(),
                     lambda data: data['calls'].__setitem__(0, copy.deepcopy(data['calls'][1])),
                     lambda data: data['calls'][0].__setitem__('input', 'foreign'),
                     lambda data: data['calls'][0].__setitem__('seconds', float('nan')),
                     lambda data: data['calls'][0]['probe'].__setitem__('frames', 99),
                     lambda data: data['calls'][0].__setitem__('metric_frames', 99),
                     lambda data: data['calls'][0]['probe'].__setitem__('width', 640),
                     lambda data: data['calls'][0]['receipt']['video'].__setitem__('backend', 'ffmpeg'),
                     lambda data: data['calls'][0]['receipt']['video']['encoding'].__setitem__('segments', 2),
                     lambda data: data['calls'][0]['receipt']['video']['encoding'].__setitem__('isolation', 'in-process-measurement'),
                     lambda data: data['calls'][0].__setitem__('decode', None),
                     lambda data: data['calls'][2]['encoder_observation'].__setitem__('sessions', 2),
                     lambda data: data['calls'][2]['encoder_observation'].__setitem__('version', 'v4.2.1'),
                     lambda data: data['calls'][2]['encoder_observation'].__setitem__('gop_frames', 125),
                     lambda data: data['calls'][2]['encoder_observation'].__setitem__('parallelism_level', 2),
                     lambda data: data['calls'][1]['probe'].__setitem__('payload_bytes', 10300),
                     lambda data: data['calls'][1].__setitem__('psnr_db', 40.21),
                     lambda data: data['calls'][1]['probe'].__setitem__('cadence', '24/1')]
        for mutation in mutations:
            data = self.fixture()
            mutation(data)
            with self.assertRaises(ValueError):
                reference.summarize(data)

    def test_infinite_quality_requires_both_infinite(self):
        data = self.fixture()
        for row in data['calls']:
            row['psnr_db'] = None
        reference.summarize(data)
        data['calls'][1]['psnr_db'] = 40
        with self.assertRaises(ValueError):
            reference.summarize(data)


class BuildIsolation(unittest.TestCase):
    def test_default_and_release_feature_closures_exclude_measurement(self):
        packages = {}
        for manifest in (ROOT / 'crates').glob('*/Cargo.toml'):
            data = tomllib.loads(manifest.read_text())
            packages[data['package']['name']] = data.get('features', {})

        def visit(package, feature, seen):
            key = (package, feature)
            if key in seen:
                return
            seen.add(key)
            self.assertNotIn(feature, ('measurement', 'av1-measurement'))
            for child in packages[package].get(feature, []):
                if child.startswith('dep:'):
                    continue
                if '/' in child:
                    dependency, selected = child.split('/', 1)
                    dependency = dependency.rstrip('?')
                    if dependency in packages:
                        visit(dependency, selected, seen)
                else:
                    visit(package, child, seen)

        for feature in ('default', 'release-default', 'release-full'):
            visit('coreav-cli', feature, set())
        self.assertEqual(packages['coreav-cli']['av1-measurement'], [
            'svt-av1-native', 'native-h264', 'coreav-transcode/av1-measurement'])

    @unittest.skipUnless(shutil.which('rustc'), 'Rust is needed for the executable profile guard')
    def test_profile_guard_refuses_release_dist_and_unknown_layouts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / 'guard'
            subprocess.run(['rustc', str(ROOT / 'crates/coreav-backend-svt-av1/build.rs'),
                            '-o', str(executable)], check=True, capture_output=True, timeout=60)
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith('CARGO_FEATURE_')}
            environment['CARGO_FEATURE_MEASUREMENT'] = '1'
            for profile in ('release', 'dist', 'debug', 'edge', 'unknown'):
                environment['OUT_DIR'] = str(root / profile / 'build/backend-hash/out')
                result = subprocess.run([str(executable)], env=environment,
                                        capture_output=True, text=True, timeout=10)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('release and dist are forbidden', result.stdout)
            environment['OUT_DIR'] = str(root / 'av1-measurement/build/backend-hash/out')
            subprocess.run([str(executable)], env=environment, check=True,
                           capture_output=True, timeout=10)
            del environment['CARGO_FEATURE_MEASUREMENT']
            environment['OUT_DIR'] = str(root / 'release/build/backend-hash/out')
            subprocess.run([str(executable)], env=environment, check=True,
                           capture_output=True, timeout=10)


if __name__ == '__main__':
    unittest.main()
