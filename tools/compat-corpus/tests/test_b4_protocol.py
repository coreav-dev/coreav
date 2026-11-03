"""Media-free checks of the frozen B4 schedule and inference."""
import collections
import copy
import math
import pathlib
import random
import statistics
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import b4_protocol as protocol

try:
    import numpy as np
except ImportError:
    np = None


def literal_indices():
    rng = random.Random(0)
    draws = []
    for _ in range(999):
        blocks = []
        for _ in range(4):
            blocks.append(rng.randrange(4))
        families = []
        for _ in range(3):
            files = []
            for _ in range(8):
                files.append(rng.randrange(8))
            families.append(tuple(files))
        draws.append((tuple(blocks), tuple(families)))
    return draws


def log_data(value):
    return {arm: {family: [[[value(a, f, file, command, pair) for pair in range(16)]
                           for command in range(2)] for file in range(8)]
                  for f, family in enumerate(protocol.FAMILIES)}
            for a, arm in enumerate(protocol.ARMS)}


def literal_sample(data, blocks, files):
    sample = []
    for f, family in enumerate(protocol.FAMILIES):
        for block in blocks:
            for file in files[f]:
                for command in range(2):
                    for start in (4 * block, 4 * block + 2):
                        pair = data[family][file][command]
                        sample.append((pair[start] + pair[start + 1]) / 2)
    return sample


class CalibrationSchedule(unittest.TestCase):
    def test_public_constants(self):
        self.assertEqual(protocol.METHOD, 'b4-counterbalanced-v2')
        self.assertEqual(protocol.FAMILIES, ('isobmff', 'matroska', 'mpegts'))
        self.assertEqual(protocol.ARMS, ('aa', 'null'))
        self.assertEqual(protocol.CALIBRATION_COMMANDS,
                         [['-show_format', '-show_streams'], ['-show_packets']])

    def test_every_descriptor_and_visit_order_matches_the_registration(self):
        schedule = protocol.calibration_schedule()
        expected_visits = []
        for block in range(4):
            for ordinal in range(24):
                rank, family_index = divmod(ordinal, 3)
                arms = ['aa', 'null']
                commands = [0, 1]
                if (block + ordinal) % 2:
                    arms.reverse()
                    commands.reverse()
                for arm in arms:
                    for command in commands:
                        expected_visits.append((block, rank, protocol.FAMILIES[family_index], arm, command))
        visits = []
        key = lambda row: (row['block'], row['rank'], row['class'], row['arm'], row['command'])
        import itertools
        for visit, rows in itertools.groupby(schedule, key):
            rows = list(rows)
            visits.append(visit)
            block, rank, family, arm, command = visit
            warm = 4 if block == 0 else 2
            self.assertEqual([row['timed'] for row in rows], [False] * warm + [True] * 4)
            self.assertEqual([row['pair'] for row in rows],
                             list(range(warm)) + list(range(4 * block, 4 * block + 4)))
            self.assertEqual([row['order'] for row in rows[:warm]], ['AB', 'BA'] * (warm // 2))
            for row in rows:
                self.assertEqual(set(row), {'phase', 'arm', 'file', 'class', 'rank',
                                            'command', 'block', 'pair', 'timed', 'order'})
                self.assertEqual(row['phase'], 'calibration')
                self.assertEqual(row['file'], f'calibration:{family}:{rank}')
                self.assertIn(row['order'], ('AB', 'BA'))
        self.assertEqual(visits, expected_visits)
        self.assertEqual(schedule, protocol.calibration_schedule())

    def test_counts_and_crossovers_share_exact_seeded_bits_between_arms(self):
        rng = random.Random(10)
        with mock.patch.object(protocol.random, 'Random', return_value=rng) as constructor:
            with mock.patch.object(rng, 'getrandbits', wraps=rng.getrandbits) as bits:
                schedule = protocol.calibration_schedule()
        constructor.assert_called_once_with(10)
        self.assertEqual(bits.call_args_list, [mock.call(1)] * 384)
        self.assertEqual(len(schedule) * 2, 4992)
        self.assertEqual(sum(row['timed'] for row in schedule) * 2, 3072)
        cases = collections.defaultdict(list)
        for row in schedule:
            cases[row['rank'], row['class'], row['command'], row['arm']].append(row)
        self.assertEqual(len(cases), 96)
        rng = random.Random(10)
        first_bits = []
        for rank in range(8):
            for family in protocol.FAMILIES:
                for command in range(2):
                    expected = []
                    for _ in range(8):
                        bit = rng.getrandbits(1)
                        first_bits.append(bit)
                        expected += ['BA', 'AB'] if bit else ['AB', 'BA']
                    for arm in protocol.ARMS:
                        rows = cases[rank, family, command, arm]
                        self.assertEqual(len(rows), 26)
                        timed = [row for row in rows if row['timed']]
                        self.assertEqual([row['pair'] for row in timed], list(range(16)))
                        self.assertEqual([row['order'] for row in timed], expected)
        self.assertEqual(first_bits[:24],
                         [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 0, 0, 1, 1, 1])


class ProductSchedule(unittest.TestCase):
    def test_command_major_selection_order_parity_and_eligibility(self):
        selection = [{'class': 'matroska', 'file': 'm', 'rank': 3},
                     {'class': 'mpegts', 'file': 't', 'rank': 0},
                     {'class': 'isobmff', 'file': 'i', 'rank': 2},
                     {'class': 'mpegts', 'file': 'u', 'rank': 1}]
        commands = [['-show_format'], ['-show_packets'], ['-f', 'mpegts', '-show_streams']]
        before = copy.deepcopy((selection, commands))
        with mock.patch.object(protocol.speed, 'eligible', wraps=protocol.speed.eligible) as eligible:
            schedule = protocol.product_schedule(selection, commands)
        self.assertEqual(eligible.call_args_list,
                         [mock.call(args, entry['class']) for args in commands for entry in selection])
        expected = [(0, entry) for entry in selection] + [(1, entry) for entry in selection]
        expected += [(2, selection[1]), (2, selection[3])]
        self.assertEqual(len(schedule), 90)
        self.assertEqual(sum(row['timed'] for row in schedule), 50)
        for index, (command, entry) in enumerate(expected):
            rows = schedule[index * 9:(index + 1) * 9]
            base = {'phase': 'product', 'arm': 'product', 'block': None,
                    'class': entry['class'], 'file': entry['file'],
                    'rank': entry['rank'], 'command': command}
            expected_rows = [dict(base, timed=False, pair=pair, order=order)
                             for pair, order in enumerate(['AB', 'BA', 'AB', 'BA'])]
            orders = ['AB', 'BA', 'AB', 'BA', 'AB']
            if (command + entry['rank']) % 2:
                orders = ['BA', 'AB', 'BA', 'AB', 'BA']
            expected_rows += [dict(base, timed=True, pair=pair, order=order)
                              for pair, order in enumerate(orders)]
            self.assertEqual(rows, expected_rows)
        self.assertEqual((selection, commands), before)

    def test_empty_and_unregistered_forced_formats(self):
        self.assertEqual(protocol.product_schedule([], [['-show_format']]), [])
        self.assertEqual(protocol.product_schedule([{'class': 'mpegts', 'file': 't', 'rank': 0}], []), [])
        for args in (['-f', 'matroska'], ['-f']):
            with self.subTest(args=args), self.assertRaises(ValueError):
                protocol.product_schedule([{'class': 'matroska', 'file': 'm', 'rank': 0}], [args])


class CalibrationInference(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.data = log_data(lambda arm, family, file, command, pair:
                            (family * 83 + file * 19 + command * 11 + (pair // 4) * 31
                             + (pair % 4) ** 2) * (1 if arm == 0 else -0.7) / 1000)
        cls.actual = protocol.calibration_statistics(cls.data)

    def test_all_seeded_indices_equal_the_literal_reference(self):
        actual = list(protocol.resampling_indices())
        self.assertEqual(actual, literal_indices())
        self.assertEqual(len(actual), 999)
        self.assertEqual(actual[0], ((3, 3, 0, 2),
                         ((7, 6, 4, 7, 5, 3, 2, 4), (2, 1, 4, 2, 4, 1, 1, 5), (7, 1, 5, 6, 5, 3, 7, 7))))
        self.assertEqual(actual[-1], ((3, 2, 1, 3),
                         ((7, 2, 7, 7, 5, 1, 0, 3), (2, 2, 7, 1, 7, 2, 0, 2), (0, 0, 6, 5, 2, 2, 1, 0))))
        self.assertEqual(actual, list(protocol.resampling_indices()))

    def test_point_and_both_quantiles_match_the_literal_cluster_reference(self):
        for arm in protocol.ARMS:
            point = math.exp(statistics.median(literal_sample(self.data[arm], range(4), [range(8)] * 3)))
            draws = sorted(math.exp(statistics.median(literal_sample(self.data[arm], blocks, files)))
                           for blocks, files in literal_indices())
            actual = self.actual[arm]
            self.assertEqual(actual['estimate'], point)
            for key, probabilities in [('ci95_diagnostic', (0.025, 0.975)),
                                       ('ci995_gate', (0.0025, 0.9975))]:
                indices = [math.ceil(p * 999) - 1 for p in probabilities]
                self.assertEqual(actual[key], [draws[index] for index in indices])
            self.assertNotEqual(actual['ci95_diagnostic'], actual['ci995_gate'])
            self.assertEqual(actual['point_in_band'], 0.98 <= point <= 1.02)
            self.assertEqual(actual['gate_contains_one'], draws[2] <= 1 <= draws[996])

    def test_repeated_blocks_and_files_repeat_the_entire_cluster_for_both_arms(self):
        draw = ((3, 3, 0, 3), ((7, 7, 7, 7, 0, 0, 0, 0),
                               (1, 1, 1, 1, 1, 1, 1, 1), (6, 6, 6, 6, 2, 2, 2, 2)))
        median = statistics.median
        samples = []
        def record(values):
            samples.append(collections.Counter(values))
            self.assertEqual(len(values), 384)
            return median(values)
        with mock.patch.object(protocol, 'resampling_indices', return_value=iter([draw] * 999)):
            with mock.patch.object(protocol.statistics, 'median', side_effect=record):
                protocol.calibration_statistics(self.data)
        for index, sample in enumerate(samples[2:]):
            arm = protocol.ARMS[index % 2]
            self.assertEqual(sample, collections.Counter(literal_sample(self.data[arm], *draw)))

    @unittest.skipIf(np is None, 'NumPy is an optional simulation cross-check')
    def test_vectorized_simulation_uses_identical_indices_and_statistics(self):
        import calibration_sim
        files, blocks = calibration_sim.resampling_indices()
        for draw, (expected_blocks, expected_files) in enumerate(protocol.resampling_indices()):
            self.assertEqual(blocks[draw].tolist(), list(expected_blocks))
            self.assertEqual(files[draw].tolist(), [list(family) for family in expected_files])
        ratios = np.array([[self.data[arm][family] for family in protocol.FAMILIES] for arm in protocol.ARMS])
        gate, diagnostic = calibration_sim.interval(ratios, (files, blocks))
        points = np.exp(np.median(ratios.reshape(2, -1, 2).mean(axis=-1), axis=-1))
        for index, arm in enumerate(protocol.ARMS):
            self.assertAlmostEqual(self.actual[arm]['estimate'], float(points[index]), places=14)
            np.testing.assert_allclose(self.actual[arm]['ci995_gate'], gate[index], rtol=1e-14)
            np.testing.assert_allclose(self.actual[arm]['ci95_diagnostic'], diagnostic[index], rtol=1e-14)

    def test_reciprocal_position_penalty_cancels_but_constant_logical_bias_does_not(self):
        orders = {(row['arm'], row['class'], row['rank'], row['command'], row['pair']): row['order']
                  for row in protocol.calibration_schedule() if row['timed']}
        def positional(arm, family, file, command, pair):
            order = orders[protocol.ARMS[arm], protocol.FAMILIES[family], file, command, pair]
            return math.log(1.04) * (1 if order == 'AB' else -1)
        null = log_data(positional)
        raw = [math.exp(value) for families in null.values() for files in families.values()
               for commands in files for pairs in commands for value in pairs]
        self.assertGreater(statistics.median(raw), 1)
        for bias in (1.0, 1.005, 1.021, 0.979):
            shifted = log_data(lambda a, f, i, c, p: positional(a, f, i, c, p) + math.log(bias))
            for result in protocol.calibration_statistics(shifted).values():
                self.assertAlmostEqual(result['estimate'], bias)
                self.assertEqual(result['point_in_band'], 0.98 <= bias <= 1.02)
                self.assertEqual(result['gate_contains_one'], bias == 1)
                for interval in ('ci95_diagnostic', 'ci995_gate'):
                    for endpoint in result[interval]:
                        self.assertAlmostEqual(endpoint, bias)

    def test_incomplete_or_malformed_shapes_are_rejected_before_resampling(self):
        bad = [None, [], {}, {'aa': {}}, dict(self.data, extra={})]
        for depth in range(4):
            for replacement in (None, {}, (), [], [0] * 17):
                data = copy.deepcopy(self.data)
                if depth == 0:
                    data['aa'] = replacement
                elif depth == 1:
                    data['aa']['matroska'] = replacement
                elif depth == 2:
                    data['aa']['matroska'][3] = replacement
                else:
                    data['aa']['matroska'][3][1] = replacement
                bad.append(data)
        data = copy.deepcopy(self.data)
        data['null']['extra'] = data['null']['mpegts']
        bad.append(data)
        data = copy.deepcopy(self.data)
        del data['null']['mpegts']
        bad.append(data)
        with mock.patch.object(protocol, 'resampling_indices') as indices:
            for index, data in enumerate(bad):
                with self.subTest(index=index), self.assertRaises(ValueError):
                    protocol.calibration_statistics(data)
        indices.assert_not_called()

    def test_nonfinite_and_nonnumeric_observations_are_rejected(self):
        for value in (float('nan'), float('inf'), -float('inf'), None, '0.0', True, [], {}, 10 ** 1000):
            data = copy.deepcopy(self.data)
            data['null']['mpegts'][7][1][15] = value
            with self.subTest(type=type(value).__name__), self.assertRaises(ValueError):
                protocol.calibration_statistics(data)


if __name__ == '__main__':
    unittest.main()
