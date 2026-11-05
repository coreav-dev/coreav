import copy
import hashlib
import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_decode as runner


class CompleteDecode(unittest.TestCase):
    def fixture(self, depth=8):
        sample_bytes = 1 if depth == 8 else 2
        stats = dict.fromkeys(runner.intra.COUNTERS, 0)
        stats.update({key: [0] * length for key, length in runner.intra.ARRAY_COUNTERS.items()})
        stats.update(ctus=1, coding_units=1, pcm_units=1, bins=3, tile_restarts=1, memory_bytes=700000)
        inter = dict.fromkeys(runner.INTER_COUNTERS, 0)
        inter.update({key: [0] * length for key, length in runner.INTER_ARRAYS.items()})
        inter['boundary_changed_samples'] = [[0] * 3 for _ in range(3)]
        geometry = dict(coded_width=16, coded_height=16, width=16, height=16, bit_depth=depth,
                        profile_idc=1 if depth == 8 else 2, ctb_log2=4, conformance_window=[0] * 4)
        digest = hashlib.md5(bytes(384 * sample_bytes)).hexdigest()
        picture = dict(type='picture', picture_index=0, poc=0, timestamp=None, **geometry,
                       packing='yuv420p' if depth == 8 else 'yuv420p10le',
                       output_bytes=384 * sample_bytes, output_md5=digest, stats=stats, inter=inter)
        picture['coded_components'] = [dict(width=width, height=height, bytes=width * height * sample_bytes,
            md5=hashlib.md5(bytes(width * height * sample_bytes)).hexdigest()) for width, height in [(16, 16), (8, 8), (8, 8)]]
        headers = dict(nals=4, pictures=1, output_flag_pictures=1, output_pictures=1,
                       independent_slices=1, dependent_slices=0, picture_hashes=0, reserved_hashes=0)
        budgets = dict(max_memory_bytes=1 << 20, max_stream_bins=100, max_picture_bins=50)
        summary = dict(type='summary', complete=True, headers=copy.deepcopy(headers), decoded_pictures=1,
                       skipped_pictures=0, stats=copy.deepcopy(stats), inter=copy.deepcopy(inter), memory_bytes=700000,
                       input_bytes=123, input_capacity=123, input_byte_limit=runner.intra.INPUT_CAP,
                       max_nal_bytes=runner.intra.NAL_CAP, **budgets, output_pictures=1,
                       output_bytes=384 * sample_bytes, output_md5=digest)
        return dict(pictures=[picture], summary=summary, expected=[geometry],
                    outputs=[dict(picture_index=0, poc=0)], references=[dict(output_index=0, bytes=384 * sample_bytes, md5=digest)],
                    header_summary=headers, source_size=123, published_md5=digest, budgets=budgets)

    def test_complete_packing_and_counts_at_both_depths(self):
        for depth in (8, 10):
            runner.validate(**self.fixture(depth))

    def test_output_order_pixels_packing_and_geometry_cannot_be_relaxed(self):
        for key, value in [('picture_index', False), ('picture_index', 1), ('poc', 1),
                           ('timestamp', 0), ('coded_width', 32), ('width', 8),
                           ('bit_depth', 10), ('packing', 'yuv420p10le'), ('output_bytes', 383),
                           ('output_md5', '0' * 32), ('conformance_window', [1, 0, 0, 0])]:
            with self.subTest(key=key, value=value):
                fixture = self.fixture()
                fixture['pictures'][0][key] = value
                with self.assertRaises(ValueError):
                    runner.validate(**fixture)
        for component in range(3):
            for key, value in [('width', 99), ('height', 99), ('bytes', 1), ('md5', 'not-a-digest')]:
                fixture = self.fixture(10)
                fixture['pictures'][0]['coded_components'][component][key] = value
                with self.assertRaises(ValueError):
                    runner.validate(**fixture)

    def test_every_completion_and_limit_field_is_binding(self):
        for key, value in [('input_bytes', 122), ('input_capacity', 0), ('input_byte_limit', 1),
                           ('max_nal_bytes', 1), ('max_memory_bytes', 900000), ('max_stream_bins', 99),
                           ('max_picture_bins', 49), ('decoded_pictures', 0), ('skipped_pictures', 1),
                           ('memory_bytes', 1), ('memory_bytes', 1 << 21), ('output_pictures', True),
                           ('output_pictures', 0), ('output_bytes', 383), ('output_md5', 'f' * 32)]:
            with self.subTest(key=key, value=value):
                fixture = self.fixture()
                fixture['summary'][key] = value
                with self.assertRaises(ValueError):
                    runner.validate(**fixture)
        for key in runner.HEADERS:
            fixture = self.fixture()
            fixture['summary']['headers'][key] += 1
            with self.assertRaisesRegex(ValueError, 'native_header_completion'):
                runner.validate(**fixture)

    def test_missing_extra_or_reordered_outputs_fail(self):
        for key in ('pictures', 'outputs', 'references'):
            for change in ('missing', 'extra'):
                fixture = self.fixture()
                if change == 'missing':
                    fixture[key].clear()
                else:
                    fixture[key].append(copy.deepcopy(fixture[key][0]))
                with self.assertRaises(ValueError):
                    runner.validate(**fixture)
        fixture = self.fixture()
        fixture['references'][0]['output_index'] = 1
        with self.assertRaisesRegex(ValueError, 'native_reference_picture'):
            runner.validate(**fixture)

    def test_inter_and_strength_counters_have_actual_consumers(self):
        for owner, key, value in [('inter', 'weighted_units', 1), ('inter', 'amvp_lists', 1),
                                  ('inter', 'prediction_units', 1), ('inter', 'boundary_strengths', [0, 1, 0]),
                                  ('inter', 'boundary_changed_samples', [[1, 0, 0], [0, 0, 0], [0, 0, 0]]),
                                  ('inter', 'boundary_changed_samples', [[0, 0, 0], [0, 1, 0], [0, 0, 0]]),
                                  ('stats', 'bins', 0), ('stats', 'pcm_units', 2), ('stats', 'memory_bytes', 0)]:
            fixture = self.fixture()
            fixture['pictures'][0][owner][key] = value
            with self.assertRaises(ValueError):
                runner.validate(**fixture)
        fixture = self.fixture()
        fixture['summary']['stats']['coding_units'] = 2
        with self.assertRaisesRegex(ValueError, 'native_aggregate_counters'):
            runner.validate(**fixture)

    def test_nonoutput_hashes_cannot_disappear_from_the_denominator(self):
        fixture = self.fixture()
        fixture['expected'].append(copy.deepcopy(fixture['expected'][0]))
        for headers in (fixture['header_summary'], fixture['summary']['headers']):
            headers.update(nals=6, pictures=2, independent_slices=2, picture_hashes=1)
        fixture['summary']['decoded_pictures'] = 2
        fixture['summary']['stats'].update(ctus=2, coding_units=2, pcm_units=2, bins=6, picture_hashes=1)
        runner.validate(**fixture)
        fixture['summary']['stats']['picture_hashes'] = 0
        with self.assertRaisesRegex(ValueError, 'native_complete_coded_hashes'):
            runner.validate(**fixture)

    def test_records_require_one_successful_terminal_summary(self):
        fixture = self.fixture()
        picture, summary = fixture['pictures'][0], fixture['summary']
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'native.stdout'
            for rows in [[], [picture], [dict(summary, complete=False)], [summary, picture],
                         [picture, summary, summary], [dict(type='unknown'), summary]]:
                path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
                with self.assertRaises(ValueError):
                    runner.records(path)
            path.write_text(json.dumps(picture) + '\n' + json.dumps(summary) + '\n')
            self.assertEqual(runner.records(path), ([picture], summary))
            path.write_text('{"type":"summary","type":"summary","complete":true}\n')
            with self.assertRaisesRegex(ValueError, 'duplicate JSON key'):
                runner.records(path)


if __name__ == '__main__':
    unittest.main()
