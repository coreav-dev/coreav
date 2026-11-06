import json
import math
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class SearchReference(unittest.TestCase):
    def test_x264_receipts_preserve_the_native_input_bytes(self):
        record = json.loads((ROOT / 'docs/design/h264-encoder-search-reference.json').read_text())
        calibration = json.loads((ROOT / 'docs/design/h264-encoder-rate-weight-reference.json').read_text())
        calls = record['calls'] + record['sweep']['calls'] + calibration['sanity_b']
        for clip in calibration['curves'].values():
            calls += clip['curves']['x264']['points']
        for call in calls:
            if call['arm'] != 'x264':
                continue
            with self.subTest(clip=call['clip'], qp=call['qp']):
                receipt = call['input_passthrough']
                self.assertEqual(receipt['output_sha256'], call['binding']['input_sha256'],
                                 'prove byte-preserving raw input before comparing quality')
                self.assertEqual(receipt['input_sha256'], receipt['output_sha256'])
                for command in (call['command'], receipt['command']):
                    before_input = command[:command.index('-i')]
                    for flag, value in (('-colorspace', 'bt709'), ('-color_range', 'tv'),
                                        ('-color_primaries', 'bt709'), ('-color_trc', 'bt709'),
                                        ('-chroma_sample_location', 'left')):
                        self.assertIn(flag, before_input,
                                      'put colour metadata before -i to prevent implicit matrix conversion')
                        self.assertEqual(before_input[before_input.index(flag) + 1], value)

    def test_calibration_quality_is_recomputed_from_complete_frame_errors(self):
        record = json.loads((ROOT / 'docs/design/h264-encoder-rate-weight-reference.json').read_text())
        for clip, result in record['curves'].items():
            self.assertEqual(sorted(result['curves']), ['before', 'q16', 'q4', 'q8', 'x264'])
            for arm, curve in result['curves'].items():
                self.assertEqual([c['qp'] for c in curve['points']], [22, 28, 34, 40])
                for call in curve['points']:
                    with self.subTest(clip=clip, arm=arm, qp=call['qp']):
                        self.assertEqual(len(call['frame_plane_sse']), 60)
                        self.assertEqual(sum(map(sum, call['frame_plane_sse'])), call['sse'])
                        samples = call['width'] * call['height'] * 3 // 2 * 60
                        self.assertAlmostEqual(call['psnr'], 10 * math.log10(255 * 255 * samples / call['sse']), places=9)

    def test_three_qp_comparison_uses_the_same_complete_source(self):
        record = json.loads((ROOT / 'docs/design/h264-encoder-search-reference.json').read_text())
        calls = record['sweep']['calls']
        self.assertEqual(sorted((c['arm'], c['qp']) for c in calls),
                         sorted((arm, qp) for arm in ('main', 'bounded', 'x264') for qp in (22, 28, 34)))
        self.assertEqual({c['binding']['input_sha256'] for c in calls},
                         {record['calls'][0]['binding']['input_sha256']})
        for call in calls:
            self.assertEqual((call['clip'], call['frames'], call['width'], call['height']),
                             ('a', 60, 640, 480))
            self.assertEqual(call['psnr_frames'], 60)
            self.assertEqual(call['ssim_frames'], 60)
            self.assertAlmostEqual(call['psnr'], call['psnr_from_squared_error'], places=6)


if __name__ == '__main__':
    unittest.main()
