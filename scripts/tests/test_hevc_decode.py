import copy
import hashlib
import json
import pathlib
import sys
import tempfile
import types
import unittest
from unittest import mock

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
                       skipped_picture_hashes=0, skipped_reserved_hashes=0,
                       input_bytes=123, input_capacity=123, input_byte_limit=runner.intra.INPUT_CAP,
                       max_nal_bytes=runner.intra.NAL_CAP, **budgets, output_pictures=1,
                       output_bytes=384 * sample_bytes, output_md5=digest)
        return dict(pictures=[picture], summary=summary, expected=[geometry],
                    outputs=[dict(picture_index=0, poc=0)], references=[dict(output_index=0, bytes=384 * sample_bytes, md5=digest)],
                    header_summary=headers, source_size=123, published_md5=digest, budgets=budgets,
                    dispositions=dict(pictures=[], skipped_picture_hashes=0, skipped_reserved_hashes=0))

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


class PacketDecode(unittest.TestCase):
    def test_only_explicitly_suppressed_rasl_hashes_are_not_sample_checks(self):
        pictures = [dict(picture_index=0, poc=0, nal_type=19, reconstruction_required=True),
                    dict(picture_index=1, poc=-1, nal_type=9, no_rasl_output=True, reconstruction_required=False),
                    dict(picture_index=2, poc=1, nal_type=1, pic_output_flag=False, reconstruction_required=True)]
        rows = [dict(kind='nal', nal_type=40, picture_index=i, picture_hash=True, reserved_hashes=0) for i in range(3)]
        rows.append(dict(kind='nal', nal_type=40, picture_index=1, picture_hash=False, reserved_hashes=1))
        accounting = runner.hash_dispositions(rows, pictures)
        self.assertEqual([p['picture_index'] for p in accounting['pictures']], [1])
        self.assertEqual(accounting['skipped_picture_hashes'], 1)
        self.assertEqual(accounting['skipped_reserved_hashes'], 1)
        for key, value in [('no_rasl_output', False), ('nal_type', 1)]:
            invalid = copy.deepcopy(pictures)
            invalid[1][key] = value
            with self.assertRaises(ValueError):
                runner.hash_dispositions(rows, invalid)
        fixture = CompleteDecode().fixture()
        fixture['expected'] *= 3
        fixture['dispositions'] = accounting
        fixture['summary'].update(decoded_pictures=2, skipped_pictures=1, skipped_picture_hashes=1, skipped_reserved_hashes=1)
        fixture['summary']['stats']['picture_hashes'] = 2
        fixture['header_summary'].update(pictures=3, picture_hashes=3, reserved_hashes=1)
        fixture['summary']['headers'] = copy.deepcopy(fixture['header_summary'])
        runner.validate(**fixture)
        for key in ('skipped_picture_hashes', 'skipped_reserved_hashes'):
            bad = copy.deepcopy(fixture)
            bad['summary'][key] = 0
            with self.assertRaises(ValueError):
                runner.validate(**bad)
        bad = copy.deepcopy(fixture)
        bad['summary']['stats']['picture_hashes'] = 1
        with self.assertRaises(ValueError):
            runner.validate(**bad)

    def fixture(self):
        fixture = CompleteDecode().fixture()
        pictures = [copy.deepcopy(fixture['pictures'][0]) for _ in range(2)]
        for index, picture in enumerate(pictures):
            picture.update(picture_index=index, timestamp=[10, 30][index])
        summary = copy.deepcopy(fixture['summary'])
        summary.update(output_pictures=2, decoded_pictures=2, output_bytes=768)
        summary['headers'].update(output_pictures=2, pictures=2)
        pts = [10, 30] + list(range(40, 1230, 10))
        pts[-1] = 15
        oracle = [dict(pts=str(time), pos=str(index * 10), size='4', data_hash='SHA256:' + '0' * 64)
                  for index, time in enumerate(pts)]
        stream = dict(type='stream', container='mp4', length_size=4, stream_index=2,
                      max_packet_bytes=64 << 20, max_packets=1_000_000,
                      time_base=dict(num='1', den=1000), packet_count=121, sent_packets=120,
                      clock='demux_presentation_ticks_nearest_away', cutoff=15)
        packets = [dict(type='packet', index=index, sent=index < 120, pts=time, media_pts=time + 5,
                        presented=True, offset=index * 10, size=4) for index, time in enumerate(pts)]
        payloads = [dict(type='payload', index=index, bytes=4, stored_sha256='0' * 64) for index in range(120)]
        return [stream, *packets, *payloads, *pictures, summary], oracle, dict(index=2, time_base='1/1000')

    def parse(self, rows, oracle, stream):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'records.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            return runner.packet_records(path, oracle, stream, 120)

    def test_unsent_metadata_cutoff_follows_the_preserved_packet_clock(self):
        rows, oracle, stream = self.fixture()
        _, summary, pictures, selected = self.parse(rows, oracle, stream)
        self.assertEqual(summary['output_pictures'], 2)
        self.assertEqual([p['timestamp'] for p in pictures], [10, 30])
        self.assertEqual([p['timestamp'] for p in selected], [10])

    def test_packet_clock_payload_identity_and_completion_are_binding(self):
        mutations = [(0, 'cutoff', 16), (0, 'time_base', dict(num='1', den=500)),
                     (1, 'pts', 11), (1, 'offset', 1), (1, 'index', False),
                     (122, 'stored_sha256', '1' * 64), (122, 'bytes', 0),
                     (-3, 'timestamp', None), (-1, 'complete', False), (-1, 'output_pictures', 1)]
        for index, key, value in mutations:
            with self.subTest(index=index, key=key):
                rows, oracle, stream = self.fixture()
                rows[index][key] = value
                with self.assertRaises(ValueError):
                    self.parse(rows, oracle, stream)
        rows, oracle, stream = self.fixture()
        del rows[122]
        with self.assertRaises(ValueError):
            self.parse(rows, oracle, stream)

    def test_integer_pts_packing_and_frame_samples_are_compared_in_order(self):
        fixture = CompleteDecode().fixture(10)
        picture = dict(fixture['pictures'][0], timestamp=-2)
        reference = dict(pts=-2, width=16, height=16, bit_depth=10, bytes=768,
                         md5=picture['output_md5'])
        self.assertEqual(runner.compare_packet_pictures([picture], [reference], None), 1)
        for key, value in [('timestamp', -1), ('width', 8), ('height', 32), ('bit_depth', 8),
                           ('output_bytes', 384), ('output_md5', '1' * 32)]:
            with self.subTest(key=key):
                changed = dict(picture, **{key: value})
                with self.assertRaises(ValueError):
                    runner.compare_packet_pictures([changed], [reference], None)
        with self.assertRaises(ValueError):
            runner.compare_packet_pictures([picture], [reference], -3)

    def test_oracle_demux_time_base_and_integer_pts_are_independently_checked(self):
        digest = hashlib.md5(bytes(384)).hexdigest()
        text = ('#hash: MD5\n#codec_id 0: rawvideo\n#media_type 0: video\n#tb 0: 1/1000\n'
                f'0, -2, -2, 1, 384, {digest}\n')
        frames = [dict(pts=-2, width=16, height=16, pix_fmt='yuv420p')]
        rows = runner.presentation_hashes(text, frames, '1/1000')
        self.assertEqual(rows[0]['pts'], -2)
        with self.assertRaises(ValueError):
            runner.presentation_hashes(text, frames, '1/500')
        with self.assertRaises(ValueError):
            runner.presentation_hashes(text, [dict(frames[0], pts=-1)], '1/1000')

    def test_prefix_geometry_stops_near_the_original_prefix_not_the_whole_file(self):
        packets = [dict(pts=str(i * 40), dts=str(i * 40 - 80)) for i in range(184)]
        packets.append(dict(pts='10800000', dts='10800000'))
        self.assertEqual(runner.geometry_end(packets, '1/1000'), '%8.320000')
        self.assertEqual(runner.geometry_end([dict(pts='1', dts='0')], '1/3'), '%1.333334')
        digest = hashlib.md5(bytes(384)).hexdigest()
        text = ('#hash: MD5\n#codec_id 0: rawvideo\n#media_type 0: video\n#tb 0: 1/1000\n'
                f'0, 10, 10, 1, 384, {digest}\n0, 30, 30, 1, 384, {digest}\n')
        frames = [dict(pts=10, width=16, height=16, pix_fmt='yuv420p')]
        self.assertEqual(len(runner.presentation_timestamps(text, '1/1000')), 2)
        self.assertEqual(len(runner.presentation_hashes(text, frames, '1/1000', 20)), 1)
        with self.assertRaises(ValueError):
            runner.presentation_hashes(text, [], '1/1000', 20)

    def test_all_roster_identities_and_each_b5_eof_arm_are_required(self):
        files = [dict(item, status='inspected', profile='Main', bit_depth=8,
                      decode=dict(complete=True, results=[dict(packet_limit=120, complete=True)]
                          + ([dict(packet_limit=0, complete=True)] if 'b5' in item['sets'] else [])))
                 for item in runner.harness.roster()]
        summary = runner.harness.corpus_result(files)
        self.assertTrue(summary['complete'])
        self.assertEqual(summary['packet_decode']['prefix_matches'], 54)
        self.assertEqual(summary['packet_decode']['eof_matches'], 8)
        changed = copy.deepcopy(files)
        next(row for row in changed if 'b5' in row['sets'])['decode']['results'].pop()
        self.assertFalse(runner.harness.corpus_result(changed)['complete'])
        self.assertFalse(runner.harness.corpus_result(files[:-1])['complete'])

    def test_container_follow_ups_preserve_all_identities_without_pixel_credit(self):
        files = [dict(item, status='inspected', profile='Main', bit_depth=8,
                      decode=dict(complete=True, results=[dict(packet_limit=120, complete=True)]
                          + ([dict(packet_limit=0, complete=True)] if 'b5' in item['sets'] else [])))
                 for item in runner.harness.roster()]
        files[0].update(status='follow_up', decode=dict(complete=False,
            follow_up='packet_container_truncated', results=[dict(packet_limit=120, complete=False)]))
        result = runner.harness.corpus_result(files)
        self.assertTrue(result['passed'])
        self.assertFalse(result['complete'])
        self.assertEqual(len(result['files']), 54)
        self.assertEqual(result['packet_decode']['completed'], 53)
        self.assertEqual(result['packet_decode']['prefix_matches'], 53)
        self.assertEqual(result['packet_decode']['follow_ups'], {'packet_container_truncated': 1})
        for reason in ('intra_substream_end', 'packet_picture_mismatch', 'packet_presentation_clock', 'unknown'):
            changed = copy.deepcopy(files)
            changed[0]['decode']['follow_up'] = reason
            with self.subTest(reason=reason):
                self.assertFalse(runner.harness.corpus_result(changed)['passed'])
        changed = copy.deepcopy(files)
        next(row for row in changed[1:] if 'b5' in row['sets'])['decode']['results'].pop()
        self.assertFalse(runner.harness.corpus_result(changed)['passed'])
        self.assertFalse(runner.harness.corpus_result(files[:-1])['passed'])

    def test_fixed_corpus_shards_cover_each_original_identity_once(self):
        roster = runner.harness.roster()
        groups = [runner.harness.corpus_shard(roster, i, 4) for i in range(4)]
        self.assertEqual(sorted(len(group) for group in groups), [13, 13, 14, 14])
        hashes = [item['sha256'] for group in groups for item in group]
        self.assertEqual(len(set(hashes)), 54)
        self.assertEqual(set(hashes), {item['sha256'] for item in roster})
        self.assertEqual(groups, [runner.harness.corpus_shard(list(reversed(roster)), i, 4) for i in range(4)])
        self.assertEqual(runner.harness.corpus_shard(roster, 0, 1), roster)
        for index, count in ((-1, 4), (4, 4), (0, 0), (0, 5), (True, 4)):
            with self.subTest(index=index, count=count), self.assertRaises(ValueError):
                runner.harness.corpus_shard(roster, index, count)

    def test_shard_completion_requires_its_exact_members_and_every_eof_arm(self):
        for index in range(4):
            expected = runner.harness.corpus_shard(runner.harness.roster(), index, 4)
            records = [dict(item, status='inspected', decode=dict(complete=True,
                results=[dict(packet_limit=120, complete=True)]
                + ([dict(packet_limit=0, complete=True)] if 'b5' in item['sets'] else []))) for item in expected]
            for record in records:
                if record['partition'] == 'development':
                    record['intra'] = dict(complete=True, stage='complete')
            scope = dict(kind='fixed_roster_shard', index=index, count=4, roster_files=54, files=len(expected))
            summary = runner.harness.corpus_result(records, expected, scope)
            self.assertTrue(summary['passed'])
            self.assertTrue(summary['complete'])
            self.assertEqual(summary['scope'], scope)
            self.assertEqual(summary['packet_decode']['expected'], len(expected))
            self.assertFalse(runner.harness.corpus_result(records)['passed'])
            for changed in (records[:-1], records + records[:1], list(reversed(records))):
                self.assertFalse(runner.harness.corpus_result(changed, expected, scope)['passed'])
            changed = copy.deepcopy(records)
            changed[0]['sha256'] = '0' * 64
            self.assertFalse(runner.harness.corpus_result(changed, expected, scope)['passed'])
            changed = copy.deepcopy(records)
            changed[0]['sets'] = []
            self.assertFalse(runner.harness.corpus_result(changed, expected, scope)['passed'])
            for position, record in enumerate(records):
                if 'b5' in record['sets']:
                    changed = copy.deepcopy(records)
                    changed[position]['decode']['results'].pop()
                    self.assertFalse(runner.harness.corpus_result(changed, expected, scope)['passed'])


    def test_oracle_runs_only_the_owner_returned_pair(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            args = types.SimpleNamespace(max_memory_bytes=1 << 20, max_picture_bins=1, max_stream_bins=2,
                                         oracle_bin=root / 'bin', out=root / 'out')
            error = runner.harness.test_oracle.QualificationError('changed')
            with mock.patch.object(runner.sys, 'platform', 'linux'), \
                 mock.patch.object(runner.harness.test_oracle, 'require', side_effect=error) as owner:
                with self.assertRaisesRegex(ValueError, '^pinned oracle binaries are required: changed$'):
                    runner.run(args)
            owner.assert_called_once_with(root / 'bin' / 'ffmpeg', root / 'bin' / 'ffprobe')
            self.assertFalse(args.out.exists())
            source, native, parser = root / 'source.mp4', root / 'native', root / 'parser'
            for path in (source, native, parser):
                path.write_bytes(path.name.encode())
            oracle = types.SimpleNamespace(ffmpeg=root / 'qualified-ffmpeg', ffprobe=root / 'qualified-ffprobe')
            for path in (oracle.ffmpeg, oracle.ffprobe):
                path.write_bytes(path.name.encode())
            item = dict(sha256=runner.harness.digest(source), size=source.stat().st_size)
            with mock.patch.object(runner.harness, 'child', return_value=(1, None, None)) as child:
                with self.assertRaisesRegex(ValueError, '^packet_configuration$'):
                    runner.corpus_decode(item, source, root / 'decode', oracle, native=native, parser=parser,
                                         max_memory_bytes=1, max_stream_bins=2, max_picture_bins=1)
            self.assertEqual(child.call_args.args[0][0], oracle.ffmpeg)
            report = json.loads((root / 'decode' / 'decode.json').read_text())
            self.assertEqual(report['oracle'], {name: runner.harness.digest(getattr(oracle, name))
                                                for name in ('ffmpeg', 'ffprobe')})

if __name__ == '__main__':
    unittest.main()
