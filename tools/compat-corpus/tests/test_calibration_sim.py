import ast
import hashlib
import importlib.util
import json
import math
import pathlib
import random
import statistics
import unittest

try:
    import numpy as np
except ImportError:
    np = None


@unittest.skipIf(np is None, 'NumPy is needed only for the media-free design simulation')
class CalibrationSimulation(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        path = pathlib.Path(__file__).resolve().parents[1] / 'calibration_sim.py'
        spec = importlib.util.spec_from_file_location('calibration_sim', path)
        cls.sim = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.sim)

    def test_balanced_position_penalty_does_not_become_logical_bias(self):
        x = self.sim.synthetic(np.random.default_rng(4), 'nearly_noiseless_position')
        ratios = np.exp(x)
        self.assertGreater(float(np.median(ratios)), 1.0007)
        ci, diagnostic = self.sim.interval(x, self.sim.resampling_indices())
        np.testing.assert_array_equal(ci, np.ones((2, 2)))
        np.testing.assert_array_equal(diagnostic, np.ones((2, 2)))

    def test_vectorized_interval_matches_literal_cluster_resampling(self):
        x = np.random.default_rng(18).normal(0, 0.02, size=(2, 3, 8, 2, 16))
        indices = self.sim.resampling_indices()
        rng = random.Random(0)
        reference = [[], []]
        for draw in range(999):
            blocks = [rng.randrange(4) for _ in range(4)]
            self.assertEqual(indices[1][draw].tolist(), blocks)
            values = [[], []]
            for family in range(3):
                files = [rng.randrange(8) for _ in range(8)]
                self.assertEqual(indices[0][draw, family].tolist(), files)
                for file in files:
                    for command in range(2):
                        for block in blocks:
                            for pair in (4 * block, 4 * block + 2):
                                for arm in range(2):
                                    values[arm].append((float(x[arm, family, file, command, pair]) +
                                                        float(x[arm, family, file, command, pair + 1])) / 2)
            for arm in range(2):
                reference[arm].append(math.exp(statistics.median(values[arm])))
        reference = [sorted(values) for values in reference]
        gate, diagnostic = self.sim.interval(x, indices)
        np.testing.assert_allclose(gate, [[r[2], r[996]] for r in reference], rtol=1e-14)
        np.testing.assert_allclose(diagnostic, [[r[24], r[974]] for r in reference], rtol=1e-14)

    def test_constant_named_bias_shifts_both_interval_endpoints(self):
        x = self.sim.synthetic(np.random.default_rng(2), 'shared_shocks')
        indices = self.sim.resampling_indices()
        ci, _ = self.sim.interval(x, indices)
        shifted, _ = self.sim.interval(x + math.log(1.02), indices)
        np.testing.assert_allclose(shifted, ci * 1.02, rtol=1e-14)

    def test_schedule_preserves_complete_crossover_units(self):
        position = self.sim.schedule(np.random.default_rng(4))
        np.testing.assert_array_equal(position[..., ::2], -position[..., 1::2])
        self.assertEqual(position.shape, (3, 8, 2, 16))


class CalibrationEvidence(unittest.TestCase):
    def test_committed_validation_is_complete_and_binds_the_simulator(self):
        root = pathlib.Path(__file__).resolve().parents[3]
        source = (root / 'tools/compat-corpus/calibration_sim.py').read_bytes()
        digest = hashlib.sha256(source).hexdigest()
        scenarios = next(node.value for node in ast.parse(source).body
                         if isinstance(node, ast.Assign) and any(
                             isinstance(target, ast.Name) and target.id == 'SCENARIOS'
                             for target in node.targets))
        expected_scenarios = {ast.literal_eval(key) for key in scenarios.keys}
        for name, seed, bootstrap_seed in [('calibration-simulation.json', 35612, 0),
                                           ('calibration-simulation-sensitivity.json', 35613, 1)]:
            with self.subTest(name=name):
                data = json.loads((root / 'docs/measurements/compat-ffprobe' / name).read_text())
                self.assertEqual(data['script_sha256'], digest)
                self.assertEqual(data['seed'], seed)
                self.assertEqual(data['bootstrap_seed'], bootstrap_seed)
                self.assertEqual(data['trials_per_scenario'], 2000)
                self.assertEqual({r['scenario'] for r in data['results']}, expected_scenarios)
                self.assertTrue(data['all_null_upper99_at_most_005'])
                for row in data['results']:
                    null = row['bias_results'][0]
                    self.assertEqual(null['multiplicative_bias'], 0)
                    count = null['rejections']['either']
                    self.assertEqual(null['rates']['either'], count / 2000)
                    z, trials = 2.3263478740408408, 2000
                    p = count / trials
                    denominator = 1 + z ** 2 / trials
                    center = (p + z ** 2 / (2 * trials)) / denominator
                    radius = z * math.sqrt(p * (1 - p) / trials + z ** 2 / (4 * trials ** 2)) / denominator
                    expected = [max(0, center - radius), min(1, center + radius)]
                    for actual, bound in zip(null['either_wilson99'], expected):
                        self.assertAlmostEqual(actual, bound, places=14)
                    self.assertLessEqual(null['either_wilson99'][1], 0.05)


if __name__ == '__main__':
    unittest.main()
