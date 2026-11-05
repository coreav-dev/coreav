import argparse
import copy
import hashlib
import json
import pathlib
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_intra as runner


class Packing(unittest.TestCase):
    def picture(self, depth=8):
        base = 0 if depth == 8 else 512
        planes = [[base + i for i in range(24)],
                  [base + 50 + i for i in range(6)],
                  [base + 80 + i for i in range(6)]]
        raw = b''.join(struct.pack('<H', value) for plane in planes for value in plane)
        metadata = dict(coded_width=6, coded_height=4, width=2, height=2, bit_depth=depth,
                        planes=[dict(width=6, height=4), dict(width=3, height=2), dict(width=3, height=2)],
                        conformance_window=[1, 1, 0, 1], output_bytes=len(raw))
        pack = (lambda values: bytes(values)) if depth == 8 else (
            lambda values: b''.join(struct.pack('<H', value) for value in values))
        visible = pack([base + 2, base + 3, base + 8, base + 9, base + 51, base + 81])
        reference = dict(bytes=len(visible), md5=hashlib.md5(visible).hexdigest(), output_index=0,
                         coded_component_md5=[hashlib.md5(pack(values)).hexdigest() for values in planes])
        return raw, metadata, dict(reference=reference), dict(bytes=len(visible), md5=reference['md5'])

    def test_both_depths_crop_in_chroma_units_and_pack_at_reference_width(self):
        for depth in (8, 10):
            raw, metadata, candidate, oracle = self.picture(depth)
            result = runner.compare_picture(raw, metadata, candidate, oracle)
            self.assertEqual(result['bytes'], 6 if depth == 8 else 12)
            self.assertEqual(result['md5'], candidate['reference']['md5'])
            self.assertEqual(result['coded_component_md5'], candidate['reference']['coded_component_md5'])
            self.assertNotEqual(result['coded_component_md5'], result['visible_component_md5'])
            if depth == 10:
                self.assertTrue(all(p['above_255'] and p['nonzero_low_two_bits'] for p in result['sample_proof']))

    def test_samples_components_crops_lengths_and_hashes_cannot_be_mutated(self):
        for depth in (8, 10):
            raw, metadata, candidate, oracle = self.picture(depth)
            changed = bytearray(raw)
            changed[4] ^= 1
            swapped = raw[:48] + raw[60:] + raw[48:60]
            for value in (bytes(changed), swapped, raw[:-1], raw + b'\0\0'):
                with self.subTest(depth=depth, mutation=len(value)), self.assertRaises(ValueError):
                    runner.compare_picture(value, metadata, candidate, oracle)
            for key, value in [('conformance_window', [0, 2, 0, 1]),
                               ('planes', [dict(width=6, height=4)]), ('output_bytes', len(raw) - 2)]:
                with self.subTest(depth=depth, field=key), self.assertRaises(ValueError):
                    runner.compare_picture(raw, dict(metadata, **{key: value}), candidate, oracle)
            bad = copy.deepcopy(candidate)
            bad['reference']['coded_component_md5'][1] = '0' * 32
            with self.assertRaisesRegex(ValueError, 'coded_component'):
                runner.compare_picture(raw, metadata, bad, oracle)
            with self.assertRaisesRegex(ValueError, 'reference_mismatch'):
                runner.compare_picture(raw, metadata, candidate, dict(oracle, md5='0' * 32))

    def test_high_bytes_and_out_of_depth_samples_are_not_discarded(self):
        for depth, invalid in ((8, 256), (10, 1024)):
            raw, metadata, candidate, oracle = self.picture(depth)
            with self.assertRaisesRegex(ValueError, 'sample_range'):
                runner.compare_picture(struct.pack('<H', invalid) + raw[2:], metadata, candidate, oracle)

    def test_row_streaming_and_nonoutput_references_do_not_invent_output_frames(self):
        for depth in (8, 10):
            raw, metadata, candidate, oracle = self.picture(depth)
            with tempfile.TemporaryDirectory() as directory:
                path = pathlib.Path(directory) / 'coded.u16le'
                path.write_bytes(raw)
                self.assertEqual(runner.compare_picture(path, metadata, candidate, oracle),
                                 runner.compare_picture(raw, metadata, candidate, oracle))
            reference = dict(source_scope='coded-picture reference (SEI), not output',
                             coded_component_md5=candidate['reference']['coded_component_md5'])
            runner.compare_picture(raw, metadata, dict(reference=reference), None)
            for ref, output in [(dict(reference, output_index=0), None), (reference, oracle)]:
                with self.assertRaisesRegex(ValueError, 'mapping_invented'):
                    runner.compare_picture(raw, metadata, dict(reference=ref), output)
            with self.assertRaisesRegex(ValueError, 'coded_component_mismatch'):
                runner.compare_picture(bytes([raw[0] ^ 1]) + raw[1:], metadata, dict(reference=reference), None)


class Census(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.data = b'\0\0\1\x26\1\x80'
        self.source = self.root / 'original.hevc'
        self.source.write_bytes(self.data)
        self.sha = runner.harness.digest(self.source)
        self.nal = dict(kind='nal', **runner.harness.annex_b(self.data)[0],
                        first_slice_segment_in_pic_flag=True, dependent_slice_segment_flag=False,
                        picture_index=0, poc=0, coded_width=2, coded_height=2, width=2, height=2,
                        bit_depth=8, profile_idc=1, ctb_log2=4, tiles=False, wpp=False, slice_type=2,
                        sao=[False, False], deblocking_disabled=True, conformance_window=[0] * 4)
        self.output = dict(kind='output', picture_index=0, poc=0, width=2, height=2, bit_depth=8)
        self.summary = dict(kind='summary', complete=True, bytes=len(self.data), nals=1,
                            pictures=1, output_pictures=1)
        self.native = self.root / 'native.stdout'
        self.capture([self.nal, self.output, self.summary])
        self.picture = dict(self.nal, all_intra=True, filters_disabled=True, dependent_segments=False)
        self.reference = dict(output_index=0, bytes=6, md5='a' * 32,
                              source='published.yuv', source_sha256='b' * 64,
                              source_scope='published_yuv_picture')
        self.candidate = runner.harness.coverage([self.picture], [self.output], [self.reference])[0]
        self.record = dict(sha256=self.sha, reference_count=dict(value=1),
                           reference_frames=[self.reference], intra_candidates=[self.candidate],
                           output_order_checked_by_packet_positions=True)
        runner.harness.write_json(self.root / 'frames.stdout',
                                  dict(frames=[dict(width=2, height=2, pix_fmt='yuv420p')]))

    def capture(self, rows):
        self.native.write_text(''.join(json.dumps(row) + '\n' for row in rows))

    def targets(self, record=None, test='one'):
        return runner.stream_targets(self.source, record or self.record, self.native, self.root,
                                     test, 'published_conformance')

    def test_original_coded_and_output_indices_are_bound_without_deduplicating_aliases(self):
        one, two = self.targets(test='one'), self.targets(test='two')
        targets = [item['targets'][0]['candidate'] for item in (one, two)]
        self.assertEqual([row['picture_index'] for row in targets], [0, 0])
        self.assertEqual([row['reference']['output_index'] for row in targets], [0, 0])
        self.assertEqual([row['test'] for row in targets], ['one', 'two'])
        self.assertEqual(targets[0]['stream_sha256'], targets[1]['stream_sha256'])

    def test_missing_new_eligible_picture_or_reference_or_census_entry_fails(self):
        for field, value in [('reference_frames', []), ('intra_candidates', []),
                             ('reference_count', dict(value=2)),
                             ('output_order_checked_by_packet_positions', False)]:
            changed = copy.deepcopy(self.record)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.targets(changed)
        changed = copy.deepcopy(self.record)
        changed['intra_candidates'][0]['picture_index'] = 1
        with self.assertRaises(ValueError):
            self.targets(changed)
        self.source.write_bytes(self.data + b'\0')
        with self.assertRaisesRegex(ValueError, 'source_identity'):
            self.targets()

    def test_filtered_picture_cannot_retain_a_g2_candidate(self):
        self.capture([dict(self.nal, sao=[True, False]), self.output, self.summary])
        with self.assertRaisesRegex(ValueError, 'unexpected_g2'):
            self.targets()

    def test_eligible_coded_picture_without_output_cannot_disappear(self):
        self.capture([self.nal, dict(self.summary, output_pictures=0)])
        record = dict(self.record, reference_count=dict(value=0), reference_frames=[], intra_candidates=[])
        with self.assertRaisesRegex(ValueError, 'eligible_coded_output_absent'):
            self.targets(record)

    def test_g3_adds_original_filtered_candidates_without_moving_them_into_g2(self):
        picture = dict(self.nal, sao=[True, False])
        self.capture([picture, self.output, self.summary])
        candidate = dict(self.candidate, gate='G3')
        record = dict(self.record, intra_candidates=[candidate])
        self.assertIsNone(self.targets(record))
        result = runner.stream_targets(self.source, record, self.native, self.root,
                                       'one', 'published_conformance', True)
        self.assertEqual(len(result['targets']), 1)
        self.assertEqual(result['targets'][0]['candidate']['gate'], 'G3')
        self.assertEqual(result['targets'][0]['candidate']['reference'], self.reference)

    def test_only_the_two_authorized_nonoutput_pictures_receive_original_sei_references(self):
        pictures = [dict(self.picture, all_intra=i in (39, 73), filters_disabled=False) for i in range(74)]
        rows = []
        for i in (39, 73):
            rows.extend([dict(self.nal, picture_index=i),
                         dict(kind='nal', nal_type=40, picture_hash=True, reserved_hashes=0,
                              picture_hash_reference=dict(algorithm='md5', components=[[i] * 16] * 3))])
        self.capture(rows)
        runner.harness.write_json(self.root / 'frames.stdout', dict(frames=[]))
        record = dict(self.record, reference_count=dict(value=0), reference_frames=[], intra_candidates=[])
        with mock.patch.object(runner.harness, 'native_rows', return_value=(self.summary, pictures, [])):
            for test in ('other', 'OPFLAG_B'):
                if test == 'other':
                    with self.assertRaisesRegex(ValueError, 'output_absent'):
                        runner.stream_targets(self.source, record, self.native, self.root, test, 'published_conformance', True)
                else:
                    result = runner.stream_targets(self.source, record, self.native, self.root, test, 'published_conformance', True)
                    self.assertEqual([r['candidate']['picture_index'] for r in result['targets']], [39, 73])
                    for target in result['targets']:
                        self.assertNotIn('output_index', target['candidate']['reference'])
                    for mutation in ('missing', 'reserved', 'short'):
                        changed = copy.deepcopy(rows)
                        if mutation == 'missing':
                            changed.pop()
                        elif mutation == 'reserved':
                            changed[-1].update(picture_hash=False, reserved_hashes=1)
                        else:
                            changed[-1]['picture_hash_reference']['components'][0].pop()
                        self.capture(changed)
                        with self.assertRaises(ValueError):
                            runner.stream_targets(self.source, record, self.native, self.root, test, 'published_conformance', True)
                        self.capture(rows)

    def metadata(self):
        return dict(kind='intra_picture', complete=True, picture_index=0,
                    coded_width=2, coded_height=2, width=2, height=2, bit_depth=8, profile_idc=1, ctb_log2=4,
                    packing='planar_y_cb_cr_u16le', input_bytes=len(self.data), nals=1, pictures=1,
                    input_byte_limit=runner.INPUT_CAP, caller_input_capacity_bytes=len(self.data),
                    max_nal_bytes=runner.NAL_CAP,
                    max_memory_bytes=1024, max_bins=10, conformance_window=[0] * 4,
                    stats=dict({key: 0 for key in runner.COUNTERS}, ctus=1, coding_units=1,
                               memory_bytes=1024, bins=10, luma_modes=[0] * 35,
                               chroma_modes=[0] * 35, dct_blocks=[0] * 4,
                               deblock_edges=[0] * 3, deblock_changed_samples=[0] * 3,
                               sao_blocks=[0] * 3, sao_changed_samples=[0] * 3))

    def test_native_completion_budgets_indices_and_suffix_coverage_are_required(self):
        stream = self.targets()
        target = stream['targets'][0]
        metadata = self.metadata()
        runner.validate_metadata(metadata, target, stream, self.source, 1024, 10)
        for key, value in [('complete', False), ('picture_index', 1), ('input_bytes', 0),
                           ('pictures', 0), ('nals', 0), ('max_bins', 0), ('max_memory_bytes', 0),
                           ('packing', 'yuv420p'), ('conformance_window', [1, 0, 0, 0]), ('ctb_log2', 5)]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                runner.validate_metadata(dict(metadata, **{key: value}), target, stream, self.source, 1024, 10)
        for key, value in [('memory_bytes', 1025), ('bins', 11), ('picture_hashes', 1),
                           ('reserved_hashes', 1), ('bins', True), ('dct_blocks', [0] * 3),
                           ('memory_bytes', 11), ('bins', 0), ('ctus', 2), ('coding_units', 0),
                           ('nonzero_transform_units', 1), ('transform_skips', 1),
                           ('transquant_bypass_units', 2), ('pcm_units', 2), ('nonzero_qp_deltas', 1),
                           ('scaling_list_blocks', 1), ('dst_blocks', 1), ('dct_blocks', [1, 0, 0, 0])]:
            changed = copy.deepcopy(metadata)
            changed['stats'][key] = value
            with self.subTest(counter=key), self.assertRaises(ValueError):
                runner.validate_metadata(changed, target, stream, self.source, 1024, 10)

    def test_ctu_completion_uses_ceiling_division_in_both_dimensions(self):
        stream = self.targets()
        target = stream['targets'][0]
        target['picture'].update(coded_width=17, coded_height=17, width=17, height=17)
        metadata = self.metadata()
        metadata.update(coded_width=17, coded_height=17, width=17, height=17)
        metadata['stats'].update(ctus=4, coding_units=4, bins=4, memory_bytes=902)
        runner.validate_metadata(metadata, target, stream, self.source, 1024, 10)
        for field, value in [('ctus', 1), ('ctus', 3), ('ctus', 5), ('coding_units', 3),
                             ('bins', 3), ('memory_bytes', 901)]:
            changed = copy.deepcopy(metadata)
            changed['stats'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                runner.validate_metadata(changed, target, stream, self.source, 1024, 10)

    def test_equal_sized_crop_with_a_shifted_origin_does_not_match_activated_header(self):
        stream = self.targets()
        target = stream['targets'][0]
        target['picture'].update(coded_width=6, coded_height=4, width=2, height=2,
                                 conformance_window=[1, 1, 0, 1])
        metadata = self.metadata()
        metadata.update(coded_width=6, coded_height=4, width=2, height=2,
                        conformance_window=[1, 1, 0, 1])
        runner.validate_metadata(metadata, target, stream, self.source, 1024, 10)
        for crop in ([0, 2, 0, 1], [1, 1, 1, 0]):
            with self.subTest(crop=crop), self.assertRaisesRegex(ValueError, 'native_crop'):
                runner.validate_metadata(dict(metadata, conformance_window=crop), target,
                                         stream, self.source, 1024, 10)

    def test_selected_suffix_counts_do_not_count_another_picture_or_reserved_as_defined(self):
        rows = [self.nal, dict(kind='nal', nal_type=40, picture_hash=True, reserved_hashes=0),
                dict(self.nal, picture_index=1),
                dict(kind='nal', nal_type=40, picture_hash=False, reserved_hashes=1)]
        self.assertEqual(runner.selected_hashes(rows, 0), (1, 0))
        self.assertEqual(runner.selected_hashes(rows, 1), (0, 1))

    def test_complete_candidate_results_still_report_absent_serial_effects_as_gaps(self):
        candidate = self.targets()['targets'][0]['candidate']
        result = dict(candidate=candidate, metadata=self.metadata(), complete=True)
        report = runner.aggregate([candidate], [result])
        self.assertTrue(report['complete'])
        self.assertFalse(report['serial_effective_coverage_complete'])
        self.assertEqual(report['coverage_gaps'], ['tiles', 'wpp', 'dependent_segments'])
        for variants in ([], [dict(result, complete=False)], [result, result]):
            with self.assertRaises(ValueError):
                runner.aggregate([candidate], variants)
        with self.assertRaises(ValueError):
            runner.aggregate([], [])

    def test_serial_effects_require_signalled_features_and_exclude_the_initial_tile(self):
        unflagged = self.targets()['targets'][0]['candidate']
        unflagged_metadata = self.metadata()
        unflagged_metadata['stats'].update(tile_restarts=5, wpp_restarts=5, dependent_segments=5)
        flagged = dict(unflagged, test='serial', tiles=True, wpp=True, dependent_segments=True)
        flagged_metadata = self.metadata()
        flagged_metadata['stats'].update(tile_restarts=1, wpp_restarts=0, dependent_segments=0)
        results = [dict(candidate=unflagged, metadata=unflagged_metadata, complete=True),
                   dict(candidate=flagged, metadata=flagged_metadata, complete=True)]
        report = runner.aggregate([unflagged, flagged], results)
        self.assertEqual(report['effective_counters']['tile_restarts'], 6)
        self.assertEqual(report['serial_coverage'], {
            'tiles': dict(candidates=1, effective=0), 'wpp': dict(candidates=1, effective=0),
            'dependent_segments': dict(candidates=1, effective=0)})
        self.assertFalse(report['serial_effective_coverage_complete'])
        flagged_metadata['stats'].update(tile_restarts=2, wpp_restarts=1, dependent_segments=1)
        report = runner.aggregate([unflagged, flagged], results)
        self.assertTrue(report['serial_effective_coverage_complete'])
        self.assertTrue(all(item == dict(candidates=1, effective=1)
                            for item in report['serial_coverage'].values()))

    def test_full_census_denominator_retains_archive_aliases_and_all_four_recipes(self):
        args = argparse.Namespace(registration=self.root / 'registration.json', data=self.root / 'data',
                                  out=self.root / 'out', synthetic_data=self.root / 'synthetic')
        cases = [dict(test=name, archive=name + '.zip') for name in ('alias_a', 'alias_b')]
        archives, expected, descriptors = [], [], {}
        for case in cases:
            member = dict(name='same.bit', path='000.bit', role='bitstream', sha256=self.sha)
            runner.harness.write_json(args.data / case['test'] / 'inventory.json', dict(members=[member]))
            record = dict(name=member['name'], sha256=self.sha, status='headers_checked')
            archives.append(dict(case, status='accounted', streams=[record]))
            candidate = dict(self.candidate, test=case['test'], stream_sha256=self.sha,
                             source_kind='published_conformance')
            expected.append(candidate)
            descriptors[case['test']] = dict(targets=[dict(candidate=candidate)])
        synthetic = []
        for recipe in runner.fixture.recipes():
            name = 'SYNTHETIC_' + recipe['id']
            synthetic.append(dict(test=name, status='headers_checked'))
            candidate = dict(self.candidate, bit_depth=10, test=name, stream_sha256='c' * 64,
                             source_kind='synthetic')
            expected.append(candidate)
            descriptors[name] = dict(targets=[dict(candidate=candidate)])
        census = dict(complete=True, archives=archives, intra_candidates=expected,
                      synthetic_census=dict(complete=True, streams=synthetic))

        def targets(source, record, native, directory, name, source_kind, filters=False):
            self.assertFalse(filters)
            return descriptors[name]

        with mock.patch.object(runner.harness, 'registration', return_value=dict(archives=cases)), \
                mock.patch.object(runner, 'stream_targets', side_effect=targets):
            result = runner.candidates(args, census)
            self.assertEqual([row['targets'][0]['candidate'] for row in result], expected)
            for mutation in ('archive', 'stream', 'recipe', 'candidate', 'complete'):
                changed = copy.deepcopy(census)
                if mutation == 'archive':
                    changed['archives'].pop()
                elif mutation == 'stream':
                    changed['archives'][0]['streams'] = []
                elif mutation == 'recipe':
                    changed['synthetic_census']['streams'].pop()
                elif mutation == 'candidate':
                    changed['intra_candidates'].pop()
                else:
                    changed['complete'] = False
                with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                    runner.candidates(args, changed)


class Oracle(unittest.TestCase):
    def frame(self):
        return dict(width=2, height=2, pix_fmt='yuv420p')

    def reference(self):
        return dict(output_index=0, bytes=6, md5='a' * 32)

    def capture(self, digest='a' * 32, size=6):
        return ('#format: frame checksums\n#version: 2\n#hash: MD5\n'
                '#media_type 0: video\n#codec_id 0: rawvideo\n'
                f'0, 0, 0, 1, {size}, {digest}\n')

    def test_complete_frame_count_size_reference_and_packing_are_required(self):
        expected = [dict(bytes=6, md5='a' * 32)]
        self.assertEqual(runner.harness.frame_hashes(self.capture(), [self.frame()], [self.reference()]), expected)
        for text in ('', self.capture('b' * 32), self.capture(size=12),
                     self.capture() + '0, 1, 1, 1, 6, ' + 'a' * 32 + '\n',
                     self.capture().replace('rawvideo', 'hevc'),
                     self.capture().replace('0, 0, 0, 1,', '1, 0, 0, 1,')):
            with self.subTest(text=text), self.assertRaises(ValueError):
                runner.harness.frame_hashes(text, [self.frame()], [self.reference()])
        with self.assertRaises(ValueError):
            runner.harness.frame_hashes(self.capture(), [dict(self.frame(), pix_fmt='yuv420p10le')], [self.reference()])
        with self.assertRaises(ValueError):
            runner.harness.frame_hashes(self.capture(), [self.frame()], [dict(self.reference(), output_index=1)])

    def test_unknown_stderr_is_informational_only_when_complete_reference_output_agrees(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            stdout, stderr = root / 'stdout', root / 'stderr'
            stdout.write_text(self.capture())
            stderr.write_text('New decoder informational text\nframe=1\ndup_frames=0\ndrop_frames=0\nprogress=end\n')
            stream = dict(test='case', frames=[self.frame()], record=dict(reference_frames=[self.reference()]))
            with mock.patch.object(runner.harness, 'child', return_value=(0, stdout, stderr)) as child:
                self.assertEqual(runner.oracle_frames(root / 'ffmpeg', root / 'original.hevc', root, stream),
                                 [dict(bytes=6, md5='a' * 32)])
                command = child.call_args.args[0]
                self.assertNotIn('-frames:v', command)
                self.assertNotIn('-vf', command)
                self.assertIn('-noautorotate', command)
                self.assertIn('-noautoscale', command)
                self.assertEqual(command[command.index('-pix_fmt') + 1], '+yuv420p')
                self.assertEqual(command[command.index('-i') + 1], root / 'original.hevc')
            for code, text in [(1, stderr.read_text()),
                               (0, 'frame=1\ndup_frames=0\ndrop_frames=0\nprogress=continue\n'),
                               (0, 'frame=2\ndup_frames=0\ndrop_frames=0\nprogress=end\n')]:
                stderr.write_text(text)
                with mock.patch.object(runner.harness, 'child', return_value=(code, stdout, stderr)), \
                        self.assertRaises(ValueError):
                    runner.oracle_frames(root / 'ffmpeg', root / 'original.hevc', root, stream)


class Controls(unittest.TestCase):
    def test_entropy_truncation_counts_rbsp_header_bytes_and_boundary_epbs(self):
        cases = [(b'\x80' * 12 + b'\x42\x7f\x80', 12, 13),
                 (b'\0\0\3\1\x80\x42\x7f', 4, 6),
                 (b'\x80\0\0\3\1\x7f', 3, 5),
                 (b'\x80\0\x7f', 1, 2)]
        for payload, header, retained in cases:
            data = b'\0\0\1\x26\1' + payload
            picture = dict(offset=3, size=len(payload) + 2, rbsp_header_bytes=header)
            with self.subTest(payload=payload):
                cut = runner.entropy_truncation_offset(data, picture)
                self.assertEqual(cut, 5 + retained)
                self.assertEqual(data[:cut], b'\0\0\1\x26\1' + payload[:retained])
                unescaped = runner.fixture.rbsp(payload)
                self.assertEqual(runner.fixture.rbsp(data[5:cut]), unescaped[:header + 1])
        for payload, header in [(b'\x80', 1), (b'\x80\x42', 1),
                                (b'\0\0\3\4\x80\x42\x7f', 4),
                                (b'\x80\x42\x7f', 99)]:
            data = b'\0\0\1\x26\1' + payload
            with self.subTest(payload=payload), self.assertRaises(ValueError):
                runner.entropy_truncation_offset(data, dict(offset=3, size=len(payload) + 2,
                                                           rbsp_header_bytes=header))

    def test_absence_truncation_budget_and_clobber_controls_need_nonzero_exit_and_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / 'original.hevc'
            original = b'\0\0\1\x26\1' + b'\x80' * 12 + b'\x42\x7f\x80'
            source.write_bytes(original)
            args = argparse.Namespace(native=root / 'native', max_memory_bytes=1024, max_bins=100)
            stream = dict(source=source, summary=dict(pictures=2),
                          targets=[dict(candidate=dict(picture_index=0),
                                        picture=dict(offset=3, size=len(original) - 3, rbsp_header_bytes=12))])

            def reject(command, directory, label):
                self.assertEqual(command[0], args.native)
                self.assertNotEqual(command[3], source)
                if label == 'truncated_entropy':
                    self.assertEqual(pathlib.Path(command[1]).read_bytes(), original[:18])
                stdout, stderr = directory / (label + '.stdout'), directory / (label + '.stderr')
                stdout.write_text('')
                category = 'limit' if 'budget' in label else 'invalid_data'
                runner.harness.write_json(stderr, dict(kind='refusal', category=category, reason='test_static_label'))
                return 1, stdout, stderr

            directory = root / 'controls'
            with mock.patch.object(runner.harness, 'child', side_effect=reject) as child:
                result = runner.negative_controls(args, stream, directory)
                self.assertEqual(len(result), 5)
                self.assertEqual(child.call_count, 5)
            self.assertEqual(source.read_bytes(), original)
            self.assertFalse((directory / 'truncated.hevc').exists())

            def partial(command, directory, label):
                result = reject(command, directory, label)
                pathlib.Path(command[3]).write_bytes(b'partial')
                return result

            for fault in ('partial', 'success', 'metadata'):
                def broken(command, directory, label):
                    result = partial(command, directory, label) if fault == 'partial' else reject(command, directory, label)
                    if fault == 'success':
                        return 0, result[1], result[2]
                    if fault == 'metadata':
                        result[1].write_text('{"complete":true}\n')
                    return result
                with self.subTest(fault=fault), mock.patch.object(runner.harness, 'child', side_effect=broken), \
                        self.assertRaises(ValueError):
                    runner.negative_controls(args, stream, root / fault)

    def test_cli_help_is_media_free_and_positional_native_contract_is_stable(self):
        path = pathlib.Path(runner.__file__)
        result = subprocess.run([sys.executable, '-I', str(path), '--help'],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--max-memory-bytes', result.stdout)
        self.assertIn('--max-bins', result.stdout)
        self.assertEqual(runner.native_command('native', 'original', 7, 'new.u16le', 1024, 100),
                         ['native', 'original', '7', 'new.u16le', '1024', '100'])


if __name__ == '__main__':
    unittest.main()
