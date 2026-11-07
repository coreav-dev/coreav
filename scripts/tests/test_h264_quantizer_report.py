import copy
import math
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import h264_quantizer_report as report


def fixture():
    cells = {}
    for qp, q in zip(report.QPS, (45, 40, 35, 30)):
        rates = math.exp(q / 10)
        cells[qp] = {'identity': {
            'old': {'bytes': rates, 'psnr': {'y': q, 'u': q + 2, 'v': q + 4}, 'ffmpeg_decode_matches': True},
            'new': {'bytes': rates * 0.9, 'psnr': {'y': q, 'u': q + 2, 'v': q + 4}, 'ffmpeg_decode_matches': True},
        }}
    return {'label': 'synthetic', 'cells': cells}


class QuantizerReportTests(unittest.TestCase):
    def test_all_planes_and_weighted_curve_have_the_known_rate_change(self):
        row = report.summarize(fixture(), 'old', 'new')
        self.assertEqual(set(row['bd_rate']), set(report.PLANES))
        for change in row['bd_rate'].values():
            self.assertAlmostEqual(change, -10., places=10)
        scores = {'y': 30., 'u': 40., 'v': 50., 'weighted': -999}
        self.assertEqual(report.quality(scores, 'weighted'), 33.75)

    def test_chroma_regression_is_not_hidden_by_luma(self):
        data = fixture()
        for cell in data['cells'].values():
            cell['identity']['new']['psnr']['y'] += 2
            cell['identity']['new']['psnr']['u'] -= 2
        row = report.summarize(data, 'old', 'new')['bd_rate']
        self.assertLess(row['y'], 0)
        self.assertGreater(row['u'], 0)

    def test_no_extrapolation_and_reciprocal_comparison(self):
        a = [(30 + i * 3, math.exp((30 + i * 3) / 10)) for i in range(4)]
        b = [(32 + i * 3, 0.8 * math.exp((32 + i * 3) / 10)) for i in range(4)]
        self.assertAlmostEqual(report.bd_rate(a, b), -20., places=10)
        self.assertAlmostEqual(report.bd_rate(b, a), 25., places=10)
        with self.assertRaises(ValueError):
            report.bd_rate(a, [(q + 100, r) for q, r in b])

    def test_preserves_and_flags_nonmonotone_plane_observations(self):
        data = fixture()
        data['cells']['34']['identity']['old']['bytes'] = 1000
        data['cells']['34']['identity']['new']['bytes'] = 900
        result = report.summarize(data, 'old', 'new')
        self.assertEqual(result['nonmonotone_rate_planes'], list(report.PLANES))
        for change in result['bd_rate'].values():
            self.assertAlmostEqual(change, -10., places=10)

    def test_rejects_missing_qps_oracle_mismatch_and_invalid_curves(self):
        data = fixture()
        cases = []
        missing = copy.deepcopy(data)
        del missing['cells']['22']
        cases.append(missing)
        for field, value in [('bytes', -1), ('ffmpeg_decode_matches', False)]:
            changed = copy.deepcopy(data)
            changed['cells']['22']['identity']['new'][field] = value
            cases.append(changed)
        changed = copy.deepcopy(data)
        changed['cells']['22']['identity']['new']['psnr']['u'] = math.inf
        cases.append(changed)
        changed = copy.deepcopy(data)
        changed['cells']['22']['identity']['new']['psnr']['y'] = 40
        cases.append(changed)
        for case in cases:
            with self.assertRaises(ValueError):
                report.summarize(case, 'old', 'new')


if __name__ == '__main__':
    unittest.main()
