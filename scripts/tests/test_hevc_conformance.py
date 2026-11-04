import copy
import importlib.util
import json
import pathlib
import os
import stat
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import zipfile

PATH = pathlib.Path(__file__).resolve().parents[1] / 'hevc_conformance.py'
SPEC = importlib.util.spec_from_file_location('hevc_conformance', PATH)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class Conformance(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)

    def archive(self, members):
        path = self.root / 'sample.zip'
        with zipfile.ZipFile(path, 'w') as archive:
            for name, value in members:
                archive.writestr(name, value)
        return path

    def inventory(self, members):
        return runner.archive_inventory(self.archive(members), self.root / 'data')

    def test_official_registration_is_bijective_and_keeps_the_unequal_depth_refusal(self):
        data = runner.registration()
        self.assertEqual(data['archive_count'], 147)
        self.assertEqual(data['compressed_bytes'], 129093824)
        case = next(c for c in data['archives'] if c['test'] == 'WP_B_MAIN10')
        self.assertEqual(case['archive'], 'WP_MAIN10_B_Toshiba_3.zip')
        case = next(c for c in data['archives'] if c['test'] == 'TSUNEQBD_A_MAIN10')
        self.assertEqual(case['expected_refusal'], 'unequal component depths')
        counts = {c['test']: c['published_output_count']['value'] for c in data['archives']
                  if 'published_output_count' in c}
        self.assertEqual(counts['BUMPING_A'], 49)

    def test_registration_cannot_drop_or_duplicate_a_candidate(self):
        data = runner.registration()
        for variant in [dict(data, archives=data['archives'][:-1]),
                        dict(data, archives=[data['archives'][0]] + data['archives'][:-1])]:
            path = self.root / 'registration.json'
            runner.write_json(path, variant)
            with self.assertRaises(ValueError):
                runner.registration(path)

    def test_acquisition_identity_allows_pins_but_not_changed_membership_or_scope(self):
        data = runner.registration()
        pinned = copy.deepcopy(data)
        pinned['archives'][0].update(archive_sha256='f' * 64, extracted=[], references=[])
        pinned['status'] = 'acquired'
        self.assertEqual(runner.registration_identity(data), runner.registration_identity(pinned))
        pinned['archives'][0]['test'] = 'different'
        self.assertNotEqual(runner.registration_identity(data), runner.registration_identity(pinned))

    def test_published_bitstream_and_output_hashes_have_distinct_roles(self):
        bitstream, yuv = b'\x00\x00\x01\x40\x01\x80', b'123456'
        md5 = runner.hashlib.md5
        members = [('a.bit', bitstream), ('a.yuv', yuv),
                   ('a.md5', f'{md5(bitstream).hexdigest()}  a.bit\n'),
                   ('a_yuv.md5', f'{md5(yuv).hexdigest()}  a.yuv\n'),
                   ('a.txt', 'contains one picture'), ('trace.txt', b'ignored'), ('decoder.cpp', b'ignored')]
        result = self.inventory(members)
        self.assertEqual([r['layout'] for r in result['references']],
                         ['bitstream_file', 'whole_output_yuv_file'])
        self.assertEqual(result['references'][1]['archived_bytes'], 6)
        self.assertEqual(len(result['descriptions']), 1)
        self.assertEqual([m['role'] for m in result['members'][-2:]], ['not_read', 'not_read'])
        self.assertEqual(len(list((self.root / 'data').iterdir())), 1)

    def test_published_yuv_without_a_hash_file_is_explicitly_derived(self):
        result = self.inventory([('a.bin', b'abc'), ('a.yuv', b'123456')])
        self.assertTrue(result['references'][0]['derived_from_published_yuv'])

    def test_relative_hash_targets_are_resolved_without_creating_duplicate_references(self):
        value = runner.hashlib.md5(b'123456').hexdigest()
        result = self.inventory([('folder/a.bit', b'abc'), ('folder/a.yuv', b'123456'),
                                 ('folder/a_yuv.md5', value + '  a.yuv\n')])
        self.assertEqual(len(result['references']), 1)
        self.assertEqual(result['references'][0]['resolved_member'], 'folder/a.yuv')

    def test_wrong_published_md5_is_not_replaced_with_an_oracle_hash(self):
        with self.assertRaisesRegex(ValueError, 'published hash differs'):
            self.inventory([('a.bit', b'abc'), ('a.md5', '0' * 32 + '  a.bit\n')])

    def test_unknown_hash_layout_never_becomes_a_frame_count(self):
        for text in ['0' * 32 + '\n' + '1' * 32, 'frame 0 ' + '0' * 32, '',
                     '# Generated 4/16/2013 1:04:07 PM\n', '0' * 32 + '  ../outside.yuv']:
            with self.subTest(text=text), self.assertRaises(ValueError):
                runner.md5_records(text, 'a.md5')
        self.assertEqual(runner.md5_records('MD5 (a.bin) = ' + 'f' * 32, 'a.md5')[0]['layout'],
                         'bitstream_file')

    def test_official_bare_digest_syntaxes_record_one_digest_not_one_picture(self):
        for source, raw in [
                ('AMVP_A_MTK_4/AMVP_A_MTK_4.md5', b'5dd1b1099391ad659225fbf94965a6c7'),
                ('MERGE_A_TI_3/MERGE_A_TI_3.md5', b'3a15886a0158ef1aaf557869b2e9c522\r\n'),
                ('DBLK_B_SONY_3.bit.yuv.md5', b'cf5de25a94a33e2d4237b8d1c773db06\r\n'),
                ('SAODBLK_A_MainConcept_4_md5.txt', b'b9c98937bcb6be445348fdc89c208f1e')]:
            with self.subTest(source=source):
                stem = source[:-8] if source.endswith('_md5.txt') else source[:-4]
                result = self.inventory([(stem + '.bit', b'abc'), (source, raw), ('description.txt', 'test data')])
                reference, = result['references']
                self.assertEqual(reference['layout'], 'whole_output_yuv_file')
                self.assertEqual(reference['syntax'], 'bare_digest')
                self.assertEqual(reference['digest_count'], 1)
                self.assertEqual(reference['md5'], raw.decode().strip())
                self.assertIsNone(reference['target'])
                self.assertIsNone(reference['archived_bytes'])
                member = next(m for m in result['members'] if m['name'] == source)
                self.assertEqual(member['role'], 'reference_hash')
                self.assertEqual(member['digest_count'], 1)
                self.assertEqual(member['sha256'], runner.hashlib.sha256(raw).hexdigest())
                self.assertEqual(runner.published_counts({}, result), [])

    def test_unbound_bare_digest_has_no_output_role_even_if_a_binary_could_match_it(self):
        for members in [[('a.bit', b'abc'), ('unrelated.md5', 'f' * 32), ('a.txt', 'test data')],
                        [('a.bit', b'abc'), ('a.md5', 'f' * 32)],
                        [('a.bit', b'abc'), ('b.bit', b'def'), ('a.md5', 'f' * 32), ('a.txt', 'test data')]]:
            with self.subTest(members=members):
                reference, = self.inventory(members)['references']
                self.assertEqual(reference['layout'], 'unresolved')

    def test_bare_named_bitstream_digest_remains_an_integrity_check(self):
        value = runner.hashlib.md5(b'abc').hexdigest()
        result = self.inventory([('a.bit', b'abc'), ('a.bit.md5', value)])
        reference, = result['references']
        self.assertEqual(reference['layout'], 'bitstream_file')
        self.assertEqual(reference['resolved_member'], 'a.bit')
        with self.assertRaisesRegex(ValueError, 'published hash differs'):
            self.inventory([('a.bit', b'abd'), ('a.bit.md5', value)])

    def test_bare_output_hash_can_bind_archived_yuv_without_inventing_a_target_label(self):
        value = runner.hashlib.md5(b'123456').hexdigest()
        result = self.inventory([('a.bit', b'abc'), ('a.md5', value), ('a.yuv', b'123456')])
        reference, = result['references']
        self.assertIsNone(reference['target'])
        self.assertEqual(reference['resolved_member'], 'a.yuv')
        self.assertEqual(reference['archived_bytes'], 6)

    def test_official_md5summer_comments_do_not_hide_hashes_or_mismatches(self):
        prefix = ('# MD5 checksums generated by MD5summer (http://www.md5summer.org)\r\n'
                  '# Generated 4/16/2013 1:04:07 PM\r\n\r\n')
        raw = (prefix + runner.hashlib.md5(b'abc').hexdigest() + ' *CIP_A_Panasonic_3.bit\r\n').encode()
        result = self.inventory([('CIP_A_Panasonic_3.bit', b'abc'), ('CIP_A_Panasonic_3.md5', raw)])
        self.assertEqual(len(result['references']), 1)
        self.assertEqual(result['references'][0]['layout'], 'bitstream_file')
        self.assertEqual(result['members'][1]['sha256'], runner.hashlib.sha256(raw).hexdigest())
        with self.assertRaisesRegex(ValueError, 'published hash differs'):
            self.inventory([('CIP_A_Panasonic_3.bit', b'abd'), ('CIP_A_Panasonic_3.md5', raw)])
        with self.assertRaises(ValueError):
            runner.md5_records(prefix + 'unknown non-comment metadata\n', 'a.md5')

    def test_sao_h_producer_absolute_target_is_metadata_not_an_extraction_path(self):
        raw = b'7e23c16898b14831b25d7857b69a9a9a */cygdrive/d/test.yuv\n'
        result = self.inventory([('SAO_H_Parabola_1.bit', b'abc'), ('SAO_H_Parabola_1.md5', raw)])
        reference, = result['references']
        self.assertEqual(reference['target'], '/cygdrive/d/test.yuv')
        self.assertEqual(reference['layout'], 'whole_output_yuv_file')
        self.assertIsNone(reference['resolved_member'])
        self.assertEqual([p.name for p in (self.root / 'data').iterdir()], ['000.bit'])
        self.assertIsNone(runner.referenced_member(reference, [dict(name='/cygdrive/d/test.yuv')]))
        with self.assertRaisesRegex(ValueError, 'unsafe archive member path'):
            self.inventory([('/cygdrive/d/test.bit', b'abc')])

    def test_appledouble_sidecars_are_hashed_as_metadata_not_text_or_references(self):
        raw = b'\x00\x05\x16\x07\x00\x02\x00\x00Mac OS X        \x00\xab'
        result = self.inventory([('a.bit', b'abc'), ('a_yuv.md5', 'f' * 32 + '  a.yuv\n'),
                                 ('__MACOSX/a/._a.txt', raw), ('__MACOSX/a/._a_yuv.md5', raw),
                                 ('__MACOSX/a/._a.bit', b'def')])
        self.assertEqual(len(result['references']), 1)
        self.assertEqual(result['descriptions'], [])
        self.assertEqual([m['role'] for m in result['members'][-3:]],
                         ['archive_metadata', 'archive_metadata', 'bitstream'])
        for member in result['members'][2:4]:
            self.assertEqual(member['sha256'], runner.hashlib.sha256(raw).hexdigest())

    def test_description_encoding_is_lossless_and_raw_bytes_keep_their_digest(self):
        for raw, encoding, text in [
                (b'The \x93bumping\x94 process.', 'cp1252', 'The \u201cbumping\u201d process.'),
                (b'plus \xb1 one', 'cp1252', 'plus \u00b1 one'),
                (b'byte \x81', 'iso-8859-1', 'byte \x81'),
                (b'\xef\xbb\xbfcontains one picture', 'utf-8-sig', 'contains one picture'),
                ('contains one picture'.encode('utf-16'), 'utf-16', 'contains one picture'),
                ('contains one picture'.encode('utf-32'), 'utf-32', 'contains one picture'),
                (b'charset=ISO-8859-1\nbyte \x93', 'iso-8859-1', 'charset=ISO-8859-1\nbyte \x93')]:
            with self.subTest(encoding=encoding, raw=raw):
                result = self.inventory([('a.bit', b'abc'), ('a.txt', raw)])
                description, = result['descriptions']
                self.assertEqual(description['encoding'], encoding)
                self.assertEqual(description['text'], text)
                self.assertNotIn('\ufffd', description['text'])
                self.assertEqual(result['members'][1]['sha256'], runner.hashlib.sha256(raw).hexdigest())
        for raw in [b'encoding=UTF-8\ninvalid \x93', b'encoding=unknown\ntext', b'binary\x00text',
                    b'\xff\xfex']:
            with self.subTest(raw=raw), self.assertRaises((ValueError, UnicodeError)):
                runner.metadata_text(raw)

    def test_zip_traversal_duplicate_names_links_and_bombs_are_rejected(self):
        for name in ['../a.bit', '/a.bit', 'C:/a.bit', 'a\\b.bit']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.inventory([(name, b'abc')])
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            self.inventory([('a.bit', b'abc'), ('A.bit', b'abc')])
        info = zipfile.ZipInfo('link.bit')
        info.create_system = 3
        info.external_attr = (stat.S_IFLNK | 0o777) << 16
        with self.assertRaisesRegex(ValueError, 'linked'):
            self.inventory([(info, b'abc')])
        with mock.patch.object(runner, 'EXPANDED_CAP', 1), self.assertRaisesRegex(ValueError, 'expansion'):
            self.inventory([('a.bit', b'abc')])

    def test_annex_b_independently_checks_all_nal_headers_and_trailing_zeroes(self):
        nals = runner.annex_b(b'\0\0\0\1\x40\1\x80\0\0\1\x42\x09\x80\0\0')
        self.assertEqual(nals, [dict(index=0, offset=4, size=3, nal_type=32, layer_id=0, temporal_id=0),
                               dict(index=1, offset=10, size=3, nal_type=33, layer_id=1, temporal_id=0)])
        for data in [b'', b'x\0\0\1\x40\1\x80', b'\0\0\1\x40\0\x80']:
            with self.subTest(data=data), self.assertRaises(ValueError):
                runner.annex_b(data)

    def test_native_complete_byte_counts_picture_boundaries_and_outputs_are_checked(self):
        data = b'\0\0\1\x26\1\x80'
        nal = dict(kind='nal', **runner.annex_b(data)[0], first_slice_segment_in_pic_flag=True,
                   picture_index=0, coded_width=2, coded_height=2, width=2, height=2, bit_depth=8,
                   slice_type=2, sao=[False, False], deblocking_disabled=True,
                   dependent_slice_segment_flag=False, profile_idc=1, tiles=False, wpp=False)
        output = dict(kind='output', picture_index=0, poc=0, width=2, height=2, bit_depth=8)
        summary = dict(kind='summary', complete=True, bytes=len(data), nals=1, pictures=1, output_pictures=1)
        rows = [nal, output, summary]
        path = self.root / 'native.jsonl'

        def capture(values):
            path.write_text(''.join(json.dumps(row) + '\n' for row in values))
            return runner.native_rows(path, data)

        self.assertEqual(capture(rows), (summary, [dict(nal, all_intra=True, filters_disabled=True,
                                                      dependent_segments=False)], [output]))
        for broken in [[nal, output], [nal, summary], [dict(nal, offset=4), output, summary],
                       [nal, dict(output, picture_index=1), summary],
                       [nal, output, dict(summary, pictures=2)],
                       [nal, output, dict(summary, complete=False)]]:
            with self.subTest(broken=broken), self.assertRaises(ValueError):
                capture(broken)

    def fixture(self):
        picture = dict(offset=3, width=2, height=2, coded_width=2, coded_height=2, bit_depth=8, picture_index=0)
        return (dict(published_output_count=dict(value=1, source='published')),
                dict(descriptions=[], references=[dict(layout='whole_output_yuv_file', archived_bytes=6, target='a.yuv')]),
                dict(pictures=1, output_pictures=1), [picture], [picture],
                dict(packets=[dict(pos='0', size='10')],
                     streams=[dict(codec_name='hevc', coded_width=2, coded_height=2)]),
                [dict(width=2, height=2, pix_fmt='yuv420p')])

    def test_independent_boundaries_formats_counts_and_published_yuv_sizes_are_required(self):
        fixture = self.fixture()
        self.assertEqual(runner.compare(*fixture)['output_pictures'], 1)
        for index, value in [(0, dict(published_output_count=dict(value=2))),
                             (1, dict(descriptions=[], references=[])),
                             (5, dict(packets=[dict(pos='5', size='10')], streams=fixture[5]['streams'])),
                             (6, [dict(width=3, height=2, pix_fmt='yuv420p')]),
                             (6, [dict(width=2, height=2, pix_fmt='yuv420p10le')])]:
            broken = list(copy.deepcopy(fixture))
            broken[index] = value
            with self.subTest(index=index, value=value), self.assertRaises(ValueError):
                runner.compare(*broken)
        broken = list(copy.deepcopy(fixture))
        broken[0] = {}
        broken[1]['references'][0]['archived_bytes'] = None
        with self.assertRaisesRegex(ValueError, 'published output count'):
            runner.compare(*broken)

    def test_bare_digest_count_and_coded_description_do_not_supply_output_counts(self):
        fixture = list(self.fixture())
        fixture[0] = {}
        fixture[1] = self.inventory([('a.bit', b'abc'), ('a.md5', 'f' * 32),
                                     ('a.txt', 'contains one picture')])
        self.assertEqual(runner.published_counts({}, fixture[1])[0]['scope'], 'coded')
        with self.assertRaisesRegex(ValueError, 'published output count'):
            runner.compare(*fixture)

    def confirmation(self, frames, md5='f' * 32, progress=None, code=0):
        out, err = self.root / 'oracle.stdout', self.root / 'oracle.stderr'
        out.write_text('MD5=' + md5 + '\n')
        err.write_text(progress if progress is not None else
                       f'frame={len(frames)}\ndup_frames=0\ndrop_frames=0\nprogress=end\n')
        with mock.patch.object(runner, 'child', return_value=(code, out, err)) as child:
            result = runner.confirm_reference(self.root / 'ffmpeg', self.root / 'a.bit', self.root,
                                              frames, [dict(layout='whole_output_yuv_file', md5='f' * 32)])
        command = child.call_args[0][0]
        self.assertIn('-noautorotate', command)
        self.assertIn('-noautoscale', command)
        self.assertEqual(command[command.index('-progress') + 1], 'pipe:2')
        self.assertEqual(command[command.index('-c:v') + 1], 'hevc')
        return result

    def test_hash_validated_count_requires_complete_exact_published_md5_confirmation(self):
        fixture = list(self.fixture())
        fixture[0] = {}
        fixture[1] = self.inventory([('a.bit', b'abc'), ('a.md5', 'f' * 32), ('a.txt', 'test data')])
        confirmation = self.confirmation(fixture[6])
        result = runner.compare(*fixture, reference_confirmation=confirmation)
        self.assertEqual(result['published_counts'], [])
        self.assertEqual(result['reference_count']['method'], 'hash_validated_oracle_count')
        self.assertFalse(result['reference_count']['published'])
        self.assertEqual(result['reference_count']['source'], 'a.md5')
        self.assertEqual(result['reference_count']['value'], 1)
        self.assertEqual(result['reference_count']['md5'], 'f' * 32)
        self.assertEqual(result['reference_count']['oracle_build'], runner.oracle_pin.load()['BUILD'])
        for broken in [None, dict(confirmation, complete=False), dict(confirmation, md5='0' * 32),
                       dict(confirmation, output_pictures=2)]:
            with self.subTest(confirmation=broken), self.assertRaises(ValueError):
                runner.compare(*fixture, reference_confirmation=broken)
        fixture[2] = dict(pictures=1, output_pictures=2)
        with self.assertRaisesRegex(ValueError, 'output count'):
            runner.compare(*fixture, reference_confirmation=confirmation)

    def test_reference_count_priority_keeps_published_bytes_and_prose_ahead_of_oracle(self):
        fixture = list(self.fixture())
        fixture[0] = dict(published_output_count=dict(value=1, source='H.265.1 6.6.3.1'))
        self.assertEqual(runner.compare(*fixture)['reference_count']['method'], 'archived_yuv_length')
        fixture[1]['references'][0]['archived_bytes'] = None
        self.assertEqual(runner.compare(*fixture)['reference_count']['method'], 'h265_1_prose_count')
        fixture[0]['published_output_count']['source'] = 'published description output count'
        self.assertEqual(runner.compare(*fixture)['reference_count']['method'], 'published_output_count')

    def test_hash_confirmation_rejects_wrong_hash_errors_missing_end_and_partial_frames(self):
        frames = self.fixture()[6]
        for options in [dict(md5='0' * 32), dict(code=1),
                        dict(progress='frame=1\ndup_frames=0\ndrop_frames=0\nprogress=continue\n'),
                        dict(progress='frame=2\ndup_frames=0\ndrop_frames=0\nprogress=end\n'),
                        dict(progress='frame=1\ndup_frames=1\ndrop_frames=0\nprogress=end\n'),
                        dict(progress='frame=1\ndup_frames=0\ndrop_frames=0\nprogress=end\nError decoding\n'),
                        dict(progress='frame=1\nprogress=end\n'), dict(progress='')]:
            with self.subTest(options=options), self.assertRaises(ValueError):
                self.confirmation(frames, **options)
        with mock.patch.object(runner, 'child') as child:
            for references in [[], [dict(layout='bitstream_file', md5='f' * 32)],
                               [dict(layout='whole_output_yuv_file', md5='not a digest')]]:
                with self.subTest(references=references), self.assertRaises(ValueError):
                    runner.confirm_reference('ffmpeg', 'a.bit', self.root, frames, references)
            child.assert_not_called()

    def test_run_confirms_the_published_hash_before_comparing_or_crediting_a_count(self):
        directory = self.root / 'data' / 'sample'
        directory.mkdir(parents=True)
        (directory / '000.bit').write_bytes(b'abc')
        inventory = dict(archive_sha256='f' * 64, descriptions=[], references=[],
                         members=[dict(name='a.bit', path='000.bit', role='bitstream', bytes=3, sha256='f' * 64)])
        case = dict(archive='a.zip', test='sample', archive_sha256='f' * 64,
                    extracted=runner.extracted_data(inventory))
        data = dict(archives=[case])
        acquisition = dict(complete=True, registration_identity_sha256=runner.registration_identity(data),
                           archives=[dict(archive='a.zip', archive_sha256='f' * 64, inventory_sha256='f' * 64)])
        fixture = self.fixture()
        args = SimpleNamespace(registration=self.root / 'registration.json', data=self.root / 'data',
                               oracle_bin=self.root, parser=self.root / 'parser', out=self.root / 'out')
        with mock.patch.object(runner, 'registration', return_value=data), \
                mock.patch.object(runner, 'read_json', side_effect=[acquisition, inventory]), \
                mock.patch.object(runner, 'digest', return_value='f' * 64), \
                mock.patch.object(runner.oracle_pin, 'verify_bin', return_value=[]), \
                mock.patch.object(runner, 'child', return_value=(0, self.root / 'native', None)), \
                mock.patch.object(runner, 'native_rows', return_value=fixture[2:5]), \
                mock.patch.object(runner, 'probe', side_effect=[fixture[5], dict(frames=fixture[6])]), \
                mock.patch.object(runner, 'confirm_reference', side_effect=ValueError('published output MD5 differs')), \
                mock.patch.object(runner, 'compare') as compare:
            with self.assertRaisesRegex(ValueError, 'G1 blocked'):
                runner.run(args)
            compare.assert_not_called()
        result = runner.read_json(args.out / 'census.json')
        self.assertFalse(result['complete'])
        self.assertEqual(result['archives'][0]['status'], 'blocked')
        self.assertEqual(result['reference_count_methods'], dict(blocked=1))
        self.assertIsNone(result['reference_count_sources'][0]['reference_count'])

    def test_count_source_audit_preserves_blocked_streams_and_never_calls_oracle_counts_published(self):
        records = [dict(test='a', status='accounted', streams=[dict(name='a.bit', sha256='a' * 64,
                        status='headers_checked', reference_count=dict(method='hash_validated_oracle_count',
                                                                     published=False, value=1))]),
                   dict(test='b', status='blocked', reason='published output MD5 differs',
                        streams=[dict(name='b.bit', sha256='b' * 64, status='not_run')]),
                   dict(test='c', status='accounted', streams=[dict(name='c.bit', sha256='c' * 64,
                        status='refused', reason='unequal component depths')])]
        result = runner.census_result(records, 'd' * 64, 'e' * 64, {})
        self.assertEqual(result['reference_count_methods'],
                         dict(hash_validated_oracle_count=1, blocked=1, expected_refusal=1))
        self.assertEqual(len(result['reference_count_sources']), 3)
        self.assertFalse(result['reference_count_sources'][0]['reference_count']['published'])
        self.assertEqual(result['reference_count_sources'][1]['reason'], 'published output MD5 differs')
        self.assertFalse(result['complete'])

    def test_duplicate_json_keys_and_boolean_counts_are_rejected(self):
        with self.assertRaisesRegex(ValueError, 'duplicate JSON key'):
            json.loads('{"nals": 1, "nals": 2}', object_pairs_hook=runner.unique_object)
        self.assertFalse(runner.integer(True))

    def test_fixed_roster_retains_all_54_files(self):
        files = runner.roster()
        self.assertEqual(len(files), 54)
        self.assertEqual(sum(f['size'] for f in files), 23305573398)

    def test_intra_coverage_is_picture_wide_and_requires_published_picture_bytes(self):
        pictures = [dict(all_intra=True, filters_disabled=True, bit_depth=8, tiles=True, wpp=False,
                         dependent_segments=True),
                    dict(all_intra=False, filters_disabled=True, bit_depth=8, dependent_segments=False)]
        outputs = [dict(picture_index=0, poc=0), dict(picture_index=1, poc=1)]
        references = [dict(md5='0' * 32), dict(md5='1' * 32)]
        coverage = runner.coverage(pictures, outputs, references)
        self.assertEqual(len(coverage), 1)
        self.assertEqual(coverage[0]['gate'], 'G2')
        self.assertTrue(coverage[0]['dependent_segments'])
        self.assertEqual(runner.coverage(pictures, outputs, []), [])
        pictures[0]['filters_disabled'] = False
        self.assertEqual(runner.coverage(pictures, outputs, references)[0]['gate'], 'G3')

    def test_reference_picture_hashes_come_from_published_yuv_and_reject_missing_tails(self):
        archive = self.archive([('a.yuv', b'123456abcdef')])
        inventory = dict(members=[dict(name='a.yuv', role='reference_yuv',
                                      sha256=runner.hashlib.sha256(b'123456abcdef').hexdigest())])
        frames = [dict(width=2, height=2, pix_fmt='yuv420p')] * 2
        result = runner.reference_frames(archive, inventory, frames)
        self.assertEqual([r['md5'] for r in result],
                         [runner.hashlib.md5(b'123456').hexdigest(), runner.hashlib.md5(b'abcdef').hexdigest()])
        for broken in [frames[:-1], frames + frames[:1]]:
            with self.subTest(count=len(broken)), self.assertRaises(ValueError):
                runner.reference_frames(archive, inventory, broken)

    def test_a_partial_census_and_missing_intra_depths_cannot_pass(self):
        record = dict(test='sample', status='accounted', streams=[dict(sha256='f' * 64, name='a.bit')])
        result = runner.census_result([record], 'a' * 64, 'b' * 64, {})
        self.assertFalse(result['complete'])
        self.assertEqual(len(result['missing_intra_sets']), 4)
        result = runner.census_result([record, dict(record, test='duplicate')], 'a' * 64, 'b' * 64, {})
        self.assertEqual(len(result['duplicate_bitstreams']['f' * 64]), 2)

    def test_download_rejects_nonofficial_urls_before_network_access(self):
        with mock.patch.object(runner.urllib.request, 'urlopen') as open_url:
            for url in ['https://example.com/test.zip', 'http://www.itu.int/test.zip']:
                with self.assertRaises(ValueError):
                    runner.download(url, self.root / 'download', 10)
            open_url.assert_not_called()

    def test_media_subprocesses_do_not_receive_credentials_or_home_configuration(self):
        with mock.patch.dict(os.environ, {'R2_SECRET_ACCESS_KEY': 'test-canary', 'CAPY_API_KEY': 'test-canary'}):
            code, output, _ = runner.child([sys.executable, '-c',
                                           'import os,json; print(json.dumps(sorted(os.environ)))'],
                                          self.root / 'child', 'environment')
        self.assertEqual(code, 0)
        keys = json.loads(output.read_text())
        self.assertNotIn('R2_SECRET_ACCESS_KEY', keys)
        self.assertNotIn('CAPY_API_KEY', keys)
        self.assertNotIn('HOME', keys)

    def test_output_order_uses_frame_packet_positions_when_the_oracle_exposes_them(self):
        fixture = list(self.fixture())
        fixture[6][0]['pkt_pos'] = '0'
        self.assertTrue(runner.compare(*fixture)['output_order_checked_by_packet_positions'])
        fixture[6][0]['pkt_pos'] = '5'
        with self.assertRaisesRegex(ValueError, 'output order'):
            runner.compare(*fixture)

    def test_online_census_releases_each_owned_media_file_and_keeps_receipts(self):
        files = [dict(sha256=character * 64, ext='mp4', size=3, partition='development', sets=[])
                 for character in ('a', 'b')]
        manifests = self.root / 'manifests'
        for corpus in ('v1', 'v1-ts-audio', 'v2-heldout'):
            runner.write_json(manifests / 'corpus' / corpus / 'manifest.json',
                              dict(files=[dict(f, key='data/' + f['sha256']) for f in files]))
        dest = self.root / 'downloads'
        dest.mkdir()
        keep = dest / 'keep.txt'
        keep.write_text('existing file')
        pool = mock.Mock()

        def fetch_one(_pool, entry, directory):
            self.assertEqual(list(directory.iterdir()), [])
            path = directory / (entry['sha256'] + '.mp4')
            path.write_bytes(b'abc')
            return path

        def inspect(item, path, out, oracle_bin):
            self.assertEqual(path.read_bytes(), b'abc')
            return dict(item, status='inspected')

        round_files = SimpleNamespace(pool=lambda: pool, fetch_one=fetch_one)
        limits = SimpleNamespace(Limits=lambda **kwargs: kwargs)
        args = SimpleNamespace(dest=dest, out=self.root / 'results', oracle_bin=self.root,
                               manifests=manifests)
        with mock.patch.dict(sys.modules, {'round_files': round_files, 'r2pool': limits}), \
                mock.patch.object(runner, 'roster', return_value=files), \
                mock.patch.object(runner.oracle_pin, 'verify_bin', return_value=[]), \
                mock.patch.object(runner, 'corpus_record', side_effect=inspect):
            runner.fetch_corpus(args, inspect=True)
        self.assertEqual(list(dest.iterdir()), [keep])
        receipt = runner.read_json(args.out / 'corpus-census.json')
        self.assertEqual([f['status'] for f in receipt['files']], ['inspected', 'inspected'])
        self.assertFalse(receipt['complete'])
        self.assertTrue(all('key' not in f for f in receipt['files']))
        pool.close.assert_called_once()


if __name__ == '__main__':
    unittest.main()
