import importlib.util
from pathlib import Path
import unittest


ROOT = Path(__file__).parents[2]
SPEC = importlib.util.spec_from_file_location('crf', ROOT / 'scripts/h264_crf_conformance.py')
CRF = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CRF)


class CrfTests(unittest.TestCase):
    def test_known_activity_bands_include_crop_and_refresh_offset(self):
        for width, height in ((16, 16), (18, 34)):
            raw = CRF.pictures(width, height, 5)
            self.assertEqual(len(raw), width * height * 3 // 2 * 5)
            self.assertEqual(CRF.expected_qps(raw, width, height, 5, 23, 3), [21, 23, 24, 24, 25])
            self.assertEqual(CRF.expected_qps(raw, width, height, 5, 10, 3), [10, 10, 11, 11, 12])
            self.assertEqual(CRF.expected_qps(raw, width, height, 5, 51, 3), [49, 51, 51, 51, 51])
            with self.assertRaises(ValueError):
                CRF.expected_qps(raw[:-1], width, height, 5, 23, 3)

    def test_picture_inspection_keeps_fixed_qp_and_validates_every_crf_qp(self):
        inspect = CRF.p.inspect
        with self.assertRaisesRegex(ValueError, 'picture QP count'):
            inspect(b'', 16, 16, 23, 2, picture_qps=[23])
        with self.assertRaisesRegex(ValueError, 'picture QP outside'):
            inspect(b'', 16, 16, 23, 2, picture_qps=[23, 52])

    def test_workflow_runs_real_crf_conformance_and_retains_its_report(self):
        workflow = (ROOT / '.github/workflows/h264-encoder.yml').read_text()
        self.assertEqual(workflow.count('- scripts/h264_crf_conformance.py'), 2)
        self.assertEqual(workflow.count('- scripts/tests/test_h264_crf.py'), 2)
        for value in ('python3 -I scripts/h264_crf_conformance.py',
                      "-p 'test_h264_crf.py'", '${{ runner.temp }}/crf-conformance'):
            self.assertIn(value, workflow)
        identity = (ROOT / 'scripts/h264_g1_simd_identity.py').read_text()
        for value in ("'crf:10'", "'crf:23'", "'crf:51'"):
            self.assertIn(value, identity)


if __name__ == '__main__':
    unittest.main()
