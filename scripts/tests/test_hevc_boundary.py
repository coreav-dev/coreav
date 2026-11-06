import copy
import json
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_boundary as boundary

COLOR = dict(primaries=None, transfer=None, matrix=None, full_range=None, chroma_location=None)
MD5 = '0' * 32


class BoundaryReceipts(unittest.TestCase):
    def receipt(self, *, coded_clock=True, container=None, declared=None):
        declared = dict(COLOR, **(declared or {}))
        inherited = dict(COLOR, **(container or {}))
        pictures = [dict(picture_index=index, timestamp=1000 + index, width=16, height=8, bit_depth=8,
                         packing='yuv420p', output_bytes=192, output_md5=format(index, '032x'),
                         color_declarations=declared) for index in range(2)]
        summary = dict(output_pictures=2, output_bytes=384, output_md5='f' * 32)
        color = {key: declared[key] if declared[key] is not None else inherited[key] for key in COLOR}
        frames = [dict(type='picture', timestamp=row['picture_index'] if coded_clock else row['timestamp'],
                       color=color, **{key: row[key] for key in boundary.FRAME_FIELDS}) for row in pictures]
        if coded_clock:
            c5 = dict(complete=True, starts=1, keyframe_only_matches=1, matched_seek_outputs=2,
                      results=[dict(start=0, unit_type=19, matched_outputs=2, following_packets=1,
                                    keyframe_available=True)])
        else:
            c5 = dict(complete=True, candidates=2, matched=1, unsafe_refusals=1, duplicate_pts_checked=True,
                      dishonest_sync_checked=False, unavailable=None,
                      results=[dict(packet=0, status='matched', frame_md5=MD5, image_md5=MD5, pts=1000),
                               dict(packet=1, status='unsafe_start_refused', category='invalid_data')])
        result = dict(type='summary', complete=True, boundary=True, backend='native', input_bytes=99,
                      sent_packets=2, inherited_color=inherited, c5=c5, output_pictures=2,
                      output_bytes=384, output_md5='f' * 32)
        return frames + [result], pictures, summary

    def compare(self, rows, pictures, summary, **options):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'boundary.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            return boundary.compare(path, pictures, summary, **options)

    def test_coded_clock_receipt_requires_every_frame_and_random_access_totals(self):
        rows, pictures, summary = self.receipt()
        result = self.compare(rows, pictures, summary, coded_clock=True)
        self.assertEqual(result['outputs'], 2)
        self.assertEqual(result['c5'], dict(starts=1, keyframe_only_matches=1, matched_seek_outputs=2))
        for mutate in (lambda r: r[0].update(output_md5='1' * 32),
                       lambda r: r[1].update(timestamp=7),
                       lambda r: r.pop(0),
                       lambda r: r[-1].update(backend='ffmpeg'),
                       lambda r: r[-1].update(output_md5='e' * 32),
                       lambda r: r[-1].pop('c5'),
                       lambda r: r[-1]['c5'].update(complete=False),
                       lambda r: r[-1]['c5'].update(starts=0, results=[]),
                       lambda r: r[-1]['c5'].update(keyframe_only_matches=0),
                       lambda r: r[-1]['c5'].update(matched_seek_outputs=3),
                       lambda r: r[-1]['c5']['results'][0].update(unit_type=1),
                       lambda r: r[-1]['c5']['results'][0].update(following_packets=5)):
            changed = copy.deepcopy(rows)
            mutate(changed)
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                self.compare(changed, pictures, summary, coded_clock=True)

    def test_stream_colour_fills_only_absent_declarations(self):
        rows, pictures, summary = self.receipt(container=dict(primaries=1, matrix=6),
                                               declared=dict(primaries=9, full_range=True))
        self.compare(rows, pictures, summary, coded_clock=True)
        self.assertEqual(rows[0]['color'], dict(COLOR, primaries=9, matrix=6, full_range=True))
        for color in (dict(rows[0]['color'], primaries=1), dict(rows[0]['color'], matrix=None),
                      dict(rows[0]['color'], chroma_location=6)):
            changed = copy.deepcopy(rows)
            changed[0]['color'] = color
            with self.subTest(color=color), self.assertRaises(ValueError):
                self.compare(changed, pictures, summary, coded_clock=True)

    def test_packet_receipt_requires_thumbnail_disposition_for_every_candidate(self):
        rows, pictures, summary = self.receipt(coded_clock=False)
        result = self.compare(rows, pictures, summary)
        self.assertEqual(result['c5'], dict(candidates=2, matched=1, unsafe_refusals=1,
                                            duplicate_pts_checked=True, dishonest_sync_checked=False))
        empty = copy.deepcopy(rows)
        empty[-1]['c5'] = dict(complete=True, candidates=0, matched=0, unsafe_refusals=0, results=[],
                               duplicate_pts_checked=False, dishonest_sync_checked=False,
                               unavailable='no unique presented keyframe output in this prefix')
        self.assertEqual(self.compare(empty, pictures, summary)['c5']['candidates'], 0)
        for mutate in (lambda c: c.update(candidates=4),
                       lambda c: c.update(unsafe_refusals=0),
                       lambda c: c.update(unavailable='none'),
                       lambda c: c.update(matched=0, unsafe_refusals=2),
                       lambda c: c['results'][1].update(category='limit'),
                       lambda c: c['results'][1].update(status='skipped'),
                       lambda c: c['results'][0].update(image_md5='x'),
                       lambda c: c['results'][0].update(substituted=1)):
            changed = copy.deepcopy(rows)
            mutate(changed[-1]['c5'])
            with self.subTest(c5=changed[-1]['c5']), self.assertRaises(ValueError):
                self.compare(changed, pictures, summary)
        changed = copy.deepcopy(empty)
        changed[-1]['c5']['unavailable'] = None
        with self.assertRaises(ValueError):
            self.compare(changed, pictures, summary)

    def test_c1_refuses_non_linux_before_reading_receipts(self):
        args = mock.Mock()
        with mock.patch.object(boundary.sys, 'platform', 'darwin'), \
             mock.patch.object(boundary.harness, 'read_json') as read, self.assertRaises(ValueError):
            boundary.c1(args)
        read.assert_not_called()


class BoundaryCensus(unittest.TestCase):
    def files(self):
        files = []
        for item in boundary.harness.roster():
            arms = [dict(packet_limit=limit, complete=True, matched_pictures=120,
                         boundary=dict(complete=True, oracle_matched_pictures=120,
                                       c5=dict(candidates=1, matched=1, unsafe_refusals=0)))
                    for limit in ((120, 0) if 'b5' in item['sets'] else (120,))]
            record = dict(item, status='inspected', decode=dict(complete=True, boundary_required=True, results=arms))
            if item['partition'] == 'development':
                record['intra'] = dict(complete=True, stage='complete')
            files.append(record)
        return files

    def test_boundary_required_census_needs_every_arm_through_the_backend(self):
        files = self.files()
        result = boundary.harness.corpus_result(files)
        self.assertTrue(result['complete'])
        arms = sum(len(record['decode']['results']) for record in files)
        self.assertEqual(result['packet_decode']['boundary'],
                         dict(files=54, arms=arms, refusals=0, c5_candidates=arms, c5_matched=arms,
                              c5_unsafe_refusals=0))
        for mutate in (lambda arm: arm.pop('boundary'),
                       lambda arm: arm['boundary'].update(complete=False),
                       lambda arm: arm['boundary'].update(oracle_matched_pictures=119)):
            changed = copy.deepcopy(files)
            mutate(changed[3]['decode']['results'][0])
            with self.subTest(arm=changed[3]['decode']['results'][0]):
                result = boundary.harness.corpus_result(changed)
                self.assertFalse(result['complete'])
                self.assertFalse(result['passed'])

    def test_boundary_required_profile_refusal_needs_the_backend_refusal(self):
        files = self.files()
        files[5]['decode'] = dict(complete=True, boundary_required=True, expected_refusal=dict(kind='refusal'),
                                  results=[], boundary_refusal=dict(complete=True))
        result = boundary.harness.corpus_result(files)
        self.assertTrue(result['complete'])
        self.assertEqual(result['packet_decode']['boundary']['refusals'], 1)
        del files[5]['decode']['boundary_refusal']
        self.assertFalse(boundary.harness.corpus_result(files)['complete'])


if __name__ == '__main__':
    unittest.main()
