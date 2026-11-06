import collections
import json
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
RECORD = ROOT / 'docs/design/h264-encoder-efficiency-reference.json'


class EfficiencyReference(unittest.TestCase):
    def test_final_curves_use_complete_identical_reference_samples(self):
        data = json.loads(RECORD.read_text())
        for clip in ('a', 'c'):
            calls = [c for c in data['calls'] if c['clip'] == clip]
            self.assertEqual(len({c['binding']['input_sha256'] for c in calls}), 1)
            for arm in ('main', 'bounded', 'final'):
                self.assertEqual(sorted(c['qp'] for c in calls if c['arm'] == arm), [22, 28, 34])
            for call in calls:
                self.assertEqual((call['frames'], call['psnr_frames'], call['ssim_frames']), (60, 60, 60))
                self.assertAlmostEqual(call['psnr'], call['psnr_from_squared_error'], places=6)
                self.assertEqual(len(call['decoded_sha256']), 64)
                if call['arm'] != 'x264':
                    self.assertTrue(call['oracle_exact'])

    def test_x264_receipts_bind_colour_metadata_before_input(self):
        data = json.loads(RECORD.read_text())
        calls = [c for c in data['calls'] if c['arm'] == 'x264']
        self.assertEqual({(c['clip'], c['qp']) for c in calls},
                         {(clip, qp) for clip in ('a', 'c') for qp in (22, 28, 34)} | {('c', 40), ('b', 28)})
        for call in calls:
            receipt = call['input_passthrough']
            self.assertEqual(receipt['input_sha256'], receipt['output_sha256'])
            self.assertEqual(receipt['output_sha256'], call['binding']['input_sha256'])
            for command in (call['command'], receipt['command']):
                before_input = command[:command.index('-i')]
                for flag, value in (('-colorspace', 'bt709'), ('-color_range', 'tv'),
                                    ('-color_primaries', 'bt709'), ('-color_trc', 'bt709'),
                                    ('-chroma_sample_location', 'left')):
                    self.assertIn(flag, before_input, 'declare raw input metadata before -i')
                    self.assertEqual(before_input[before_input.index(flag) + 1], value)

    def test_rate_intervals_do_not_extrapolate_or_hide_the_extra_anchor(self):
        data = json.loads(RECORD.read_text())
        for row in data['summary']:
            calls = [c for c in data['calls'] if c['clip'] == row['clip']]
            ours = [c['psnr'] for c in calls if c['arm'] == row['arm'] and c['qp'] in (22, 28, 34)]
            for key, arm, qps in [('bd_vs_old_main', 'main', (22, 28, 34)),
                                   ('bd_vs_x264', 'x264', (22, 28, 34)),
                                   ('bd_vs_x264_extended', 'x264', (22, 28, 34, 40))]:
                if key not in row:
                    continue
                anchor = [c['psnr'] for c in calls if c['arm'] == arm and c['qp'] in qps]
                low, high = max(min(ours), min(anchor)), min(max(ours), max(anchor))
                result = row[key]
                if low >= high:
                    self.assertIsNone(result['percent'])
                    self.assertEqual(result['reason'], 'no common PSNR interval')
                else:
                    self.assertAlmostEqual(result['low_db'], low)
                    self.assertAlmostEqual(result['high_db'], high)
                    self.assertIsInstance(result['percent'], float)

    def test_paired_timings_are_complete_and_preserve_each_measured_stream(self):
        data = json.loads(RECORD.read_text())
        for clip, report in data['paired_timing'].items():
            calls = {c['arm']: c for c in data['calls'] if c['clip'] == clip and c['qp'] == 28}
            counts = collections.Counter((o['repeat'], o['arm']) for o in report['observations'])
            self.assertEqual(counts, collections.Counter({(r, arm): 1 for r in range(5) for arm in ('bounded', 'final')}))
            for observation in report['observations']:
                self.assertEqual(observation['stream_sha256'], calls[observation['arm']]['stream_sha256'])
                self.assertAlmostEqual(observation['fps'], 60 / observation['wall'])


if __name__ == '__main__':
    unittest.main()
