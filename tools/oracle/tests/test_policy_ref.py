"""The second implementation of `coreav.policy/1` (scripts/policy_ref.py) and
the checks of scripts/policy_check.py: each catches the error it exists to
catch (docs/design/policy-check.md)."""
import copy
import importlib.util
import json
import pathlib
import struct
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts'))
SPEC = importlib.util.spec_from_file_location('policy_check', ROOT / 'scripts' / 'policy_check.py')
pc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pc)
ref = pc.policy_ref


def time(num, timescale, den=1):
    return {'ticks': {'num': num, 'den': den}, 'timescale': timescale,
            'seconds': num / (den * timescale)}


def probe():
    return {
        'schema': 'coreav.probe/1',
        'file': {'size': 6_500_000},
        'container': {'major_brand': 'isom', 'fragmented': False},
        'duration': time(10_000, 1000),
        'streams': [
            {'stream_index': 0, 'kind': 'video', 'codec': 'h264', 'duration': time(300_000, 30_000),
             'bit_rate': {'exact': {'num': 5_000_000, 'den': 1}, 'bits_per_second': 5e6},
             'sample_descriptions': 1, 'tables_valid': True, 'external_data': False,
             'video': {'width': 1920, 'height': 1080, 'rotation': 0,
                       'parameter_set_size': {'width': 1920, 'height': 1080},
                       'scanned_size': {'width': 1920, 'height': 1080}}, 'audio': None},
            {'stream_index': 1, 'kind': 'audio', 'codec': 'aac', 'duration': time(480_000, 48_000),
             'bit_rate': {'exact': {'num': 128_000, 'den': 1}, 'bits_per_second': 128e3},
             'sample_descriptions': 1, 'tables_valid': True, 'external_data': False,
             'video': None, 'audio': {'channels': 2, 'sample_rate': 48_000}},
        ],
        'dropped_streams': [{'stream_index': 2, 'track_id': 3, 'fourcc': 'mebx',
                             'reason': 'codec_not_allowed'}],
    }


def policy(text):
    return ref.load_policy(json.dumps(text) if not isinstance(text, str) else text)


class Evaluate(unittest.TestCase):
    def test_accept(self):
        p = policy({'schema': 'coreav.policy/1',
                    'streams': {'video': {}, 'audio': {}, 'unidentified': 'ignore'}})
        self.assertEqual(ref.evaluate(p, probe()), [])

    def test_order_and_values(self):
        p = policy('{"schema": "coreav.policy/1", "file": {"max_size": 1, "max_bit_rate": 1},'
                   ' "container": {"formats": ["mov"]}, "duration": {"max_seconds": 9.999},'
                   ' "streams": {"max_count": 2, "video": {"codecs": ["hevc"], "max_width": 1},'
                   ' "audio": {"min_count": 2}}}')
        reasons = ref.evaluate(p, probe())
        self.assertEqual([(r['code'], r['rule'], r['path']) for r in reasons], [
            ('above_maximum', '/file/max_size', '/file/size'),
            ('above_maximum', '/file/max_bit_rate', None),
            ('not_allowed', '/container/formats', '/container/major_brand'),
            ('above_maximum', '/duration/max_seconds', '/duration'),
            ('above_maximum', '/streams/max_count', None),
            ('below_minimum', '/streams/audio/min_count', None),
            ('not_allowed', '/streams/video/codecs', '/streams/0/codec'),
            ('above_maximum', '/streams/video/max_width', '/streams/0/video/width'),
            ('not_allowed', '/streams/unidentified', '/dropped_streams/0/fourcc'),
        ])
        self.assertEqual(reasons[1]['exact'], {'num': 5_200_000, 'den': 1})
        self.assertEqual(reasons[2]['value'], 'mp4')
        self.assertEqual(reasons[3]['limit'], 9.999)
        self.assertEqual(reasons[3]['exact'], {'num': 10, 'den': 1})
        self.assertEqual(reasons[-1]['stream_index'], 2)

    def test_decimal_limits_are_exact(self):
        doc = probe()
        doc['duration'] = time(10_010, 1000)
        for limit, count in (('10.01', 0), ('10.009999999', 1), ('1001e-2', 0)):
            p = policy('{"schema": "coreav.policy/1", "duration": {"max_seconds": %s},'
                       ' "streams": {"video": {}, "audio": {}, "unidentified": "ignore"}}' % limit)
            self.assertEqual(len(ref.evaluate(p, doc)), count, limit)

    def test_unknown_and_not_probe_output(self):
        doc = probe()
        doc['streams'][0]['video']['rotation'] = None
        doc['streams'][1]['duration'] = None
        p = policy({'schema': 'coreav.policy/1', 'duration': {'max_seconds': 100},
                    'streams': {'video': {'rotations': [0]}, 'audio': {}, 'unidentified': 'ignore'}})
        reasons = ref.evaluate(p, doc)
        self.assertEqual([(r['code'], r['path']) for r in reasons], [
            ('unknown', '/streams/1/duration'),
            ('not_allowed', '/streams/0/video/rotation'),
        ])
        self.assertEqual(ref.evaluate(p, {'schema': 'coreav.clip/1'})[0]['path'], '/schema')


class FromFfprobe(unittest.TestCase):
    FFPROBE = {
        'format': {'size': '1000', 'duration': '2.023991', 'tags': {'major_brand': 'qt  '}},
        'streams': [
            {'index': 0, 'codec_type': 'video', 'codec_name': 'h264', 'codec_tag_string': 'avc1',
             'width': 320, 'height': 240, 'time_base': '1/12800', 'duration_ts': 25600,
             'start_time': '0.500000', 'duration': '2.000000', 'bit_rate': '288144',
             'side_data_list': [{'side_data_type': 'Display Matrix', 'rotation': -90}]},
            {'index': 1, 'codec_type': 'audio', 'codec_name': 'pcm_s16le',
             'codec_tag_string': 'sowt', 'sample_rate': '48000', 'channels': 2},
            {'index': 2, 'codec_type': 'data', 'codec_tag_string': 'tmcd'},
            {'index': 3, 'codec_type': 'data', 'codec_tag_string': 'mebx'},
            {'index': 4, 'codec_type': 'video', 'codec_name': 'mjpeg',
             'disposition': {'attached_pic': 1}},
        ],
    }

    def test_mapping(self):
        doc = ref.from_ffprobe(self.FFPROBE, False)
        self.assertEqual(doc['container'], {'major_brand': 'qt  ', 'fragmented': False})
        # The presentation ends at the video's start plus its duration.
        self.assertEqual(doc['duration']['ticks'], {'num': 5, 'den': 2})
        kinds = [(s['stream_index'], s['kind'], s['codec']) for s in doc['streams']]
        self.assertEqual(kinds, [(0, 'video', 'h264'), (1, 'audio', 'pcm'),
                                 (2, 'timecode', 'timecode')])
        self.assertEqual(doc['streams'][0]['video'], {
            'width': 320, 'height': 240, 'rotation': 90,
            'parameter_set_size': {'width': 320, 'height': 240},
            'scanned_size': {'width': 320, 'height': 240}})
        self.assertEqual(doc['streams'][0]['duration']['ticks'], {'num': 2, 'den': 1})
        self.assertEqual(doc['dropped_streams'],
                         [{'stream_index': 3, 'fourcc': 'mebx', 'duration': None}])

    def test_tags_and_tracks(self):
        ffprobe = copy.deepcopy(self.FFPROBE)
        ffprobe['streams'][1]['codec_name'] = 'pcm_mulaw'
        ffprobe['streams'][1]['codec_tag_string'] = 'ulaw'
        ffprobe['streams'][4]['index'] = 2
        del ffprobe['format']['tags']
        doc = ref.from_ffprobe(ffprobe, False, {'major_brand': 'isom', 'tracks': 4})
        self.assertEqual(doc['container']['major_brand'], 'isom')
        self.assertEqual(sorted(d['stream_index'] for d in doc['dropped_streams']), [1, 2, 3])

    def test_unobservable_rules(self):
        self.assertIn('/streams/video/max_bit_rate', ref.unobservable({}, True))
        self.assertIn('/container/fragmented', ref.unobservable({}, None))
        self.assertEqual(ref.unobservable({}, False), set(ref.UNOBSERVABLE))
        p = policy({'schema': 'coreav.policy/1', 'container': {'fragmented': False},
                    'streams': {'video': {'max_bit_rate': 1}}})
        stripped = ref.observable(p, ref.unobservable(p, None))
        self.assertNotIn('fragmented', stripped['container'])
        self.assertNotIn('max_bit_rate', stripped['streams']['video'])
        self.assertEqual(stripped['streams']['invalid_tables'], 'ignore')
        self.assertIn('max_bit_rate', p['streams']['video'], 'the original is unchanged')


def box(kind, payload=b''):
    return struct.pack('>I', 8 + len(payload)) + kind + payload


class BoxWalk(unittest.TestCase):
    def walk(self, data):
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            return ref.fragmented(f.name)

    def test_fragmented(self):
        ftyp = box(b'ftyp', b'isom\0\0\0\0')
        moov = box(b'moov', box(b'mvhd', bytes(100)) + box(b'mvex', box(b'trex', bytes(24))))
        plain = box(b'moov', box(b'mvhd', bytes(100)))
        self.assertTrue(self.walk(ftyp + moov + box(b'moof') + box(b'mdat', b'x')))
        self.assertFalse(self.walk(ftyp + moov + box(b'mdat', b'x')), 'mvex without moof')
        self.assertFalse(self.walk(ftyp + plain + box(b'moof')), 'moof without mvex')
        # A 64-bit size and a box that runs to the end of the file.
        large = struct.pack('>I4sQ', 1, b'free', 16)
        self.assertTrue(self.walk(ftyp + large + moov + struct.pack('>I4s', 0, b'moof')))


    def test_origins_and_mehd(self):
        def full(kind, version, payload):
            return box(kind, bytes([version, 0, 0, 0]) + payload)
        tkhd = full(b'tkhd', 0, bytes(8) + struct.pack('>I', 7) + bytes(68))
        mdhd = full(b'mdhd', 0, bytes(8) + struct.pack('>II', 1000, 0) + bytes(4))
        moov = box(b'moov', box(b'trak', tkhd + box(b'mdia', mdhd))
                   + box(b'mvex', full(b'mehd', 1, struct.pack('>Q', 5000))))
        traf = box(b'traf', full(b'tfhd', 0, struct.pack('>I', 7))
                   + full(b'tfdt', 1, struct.pack('>Q', 2500)))
        data = box(b'ftyp', b'isom\0\0\0\0') + moov + box(b'moof', traf) + box(b'mdat')
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            self.assertEqual(ref.fragment_origins(f.name), {0: 2.5})
            self.assertEqual(ref.fragment_duration(f.name), 5000)
            self.assertTrue(ref.fragmented(f.name))


class Checks(unittest.TestCase):
    def reasons(self, p, doc):
        return ref.evaluate(p, doc)

    def test_precision_catches_a_wrong_value(self):
        p = policy({'schema': 'coreav.policy/1', 'file': {'max_size': 1},
                    'streams': {'video': {'max_width': 1}, 'audio': {}, 'unidentified': 'ignore'}})
        doc = probe()
        for reason in self.reasons(p, doc):
            self.assertIsNone(pc.precision(reason, doc))
        wrong = copy.deepcopy(self.reasons(p, doc))
        wrong[1]['value'] = 1919
        self.assertIn('holds 1920', pc.precision(wrong[1], doc))
        moved = copy.deepcopy(self.reasons(p, doc))
        moved[1]['path'] = '/streams/7/video/width'
        self.assertIn('not in the probe output', pc.precision(moved[1], doc))

    def test_spec_comparison(self):
        p = policy({'schema': 'coreav.policy/1', 'file': {'max_bit_rate': 1},
                    'streams': {'video': {}, 'audio': {}, 'unidentified': 'ignore'}})
        want = self.reasons(p, probe())
        self.assertTrue(pc.same_reasons(copy.deepcopy(want), want))
        close = copy.deepcopy(want)
        close[0]['value'] *= 1 + 1e-15
        self.assertTrue(pc.same_reasons(close, want), 'a rounded convenience copy')
        for key, value in (('code', 'unknown'), ('rule', '/x'), ('exact', None), ('limit', 2)):
            changed = copy.deepcopy(want)
            changed[0][key] = value
            self.assertFalse(pc.same_reasons(changed, want), key)
        self.assertFalse(pc.same_reasons([], want))

    def test_outcome_ignores_the_stream_of_the_duration_rule(self):
        reasons = [{'code': 'above_maximum', 'rule': '/duration/max_seconds', 'stream_index': 1},
                   {'code': 'not_allowed', 'rule': '/streams/invalid_tables', 'stream_index': 0}]
        self.assertEqual(ref.outcome(reasons, ref.UNOBSERVABLE),
                         [('above_maximum', '/duration/max_seconds', None)])


class KnownDivergences(unittest.TestCase):
    def test_documents_in_sync(self):
        import re
        twin = json.loads((ROOT / 'docs' / 'known-divergences-policy.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-policy.md').read_text()
        documented = set(re.findall(r'^## (KD-P-[A-Z-]+)$', page, re.M))
        ids = {entry['id'] for entry in twin['entries']}
        self.assertEqual(documented, ids)
        every = {**ref.KNOWN_DIVERGENCES, **ref.MATROSKA_DIVERGENCES}
        self.assertEqual(ids, set(every))
        for entry in twin['entries']:
            self.assertIs(getattr(ref, entry['adjustment']), every[entry['id']])
            self.assertEqual(entry['filename_pattern'], '*')

    def test_protected_covers_only_protected_streams(self):
        p = policy({'schema': 'coreav.policy/1',
                    'streams': {'video': {'min_count': 1}, 'audio': {}}})
        ours_doc = probe()
        ours_doc['streams'] = ours_doc['streams'][1:]
        ours_doc['dropped_streams'] = [{'stream_index': 0, 'fourcc': 'encv'}]
        ours = ref.outcome(ref.evaluate(p, ours_doc), ref.UNOBSERVABLE)
        oracle = probe()
        oracle['dropped_streams'] = []
        excluded = set(ref.UNOBSERVABLE)
        self.assertEqual(ref.covered(p, excluded, oracle, {'protected': {0: 'encv'}}, ours),
                         ['KD-P-PROTECTED'])
        self.assertIsNone(ref.covered(p, excluded, oracle, {'protected': {}}, ours))
        # The wrong stream is protected: the adjusted verdict still differs.
        self.assertIsNone(ref.covered(p, excluded, oracle, {'protected': {1: 'enca'}}, ours))

    def test_fragment_origin(self):
        oracle = probe()
        oracle['streams'][0]['duration'] = time(610, 1)
        oracle['streams'][1]['duration'] = time(610, 1)
        oracle['duration'] = time(610, 1)
        evidence = {'fragmented': True, 'origins': {0: 10, 1: 10}}
        adjusted = ref._kd_fragment_origin(oracle, evidence)
        self.assertEqual(adjusted['duration']['ticks'], {'num': 600, 'den': 1})
        self.assertEqual(adjusted['streams'][1]['duration']['ticks'], {'num': 600, 'den': 1})
        self.assertIsNone(ref._kd_fragment_origin(oracle, {'fragmented': False,
                                                           'origins': {0: 10, 1: 10}}))
        # Every listed stream needs a walked origin.
        self.assertIsNone(ref._kd_fragment_origin(oracle, {'fragmented': True,
                                                           'origins': {0: 10}}))

    def test_edited_bit_rate(self):
        # 10 s of video at 600 Hz, 6,250,000 bytes; one media edit of 5 s.
        track = {'timescale': 600, 'mdhd': 6000, 'edits': [(5000, 0, 0x10000)],
                 'bytes': 6_250_000, 'btrt': 5_000_000}
        evidence = {'timing': {0: track}, 'headers': {'timescale': 1000}}
        adjusted = ref._kd_edited_bit_rate(probe(), evidence)
        # Every byte over the 5 s the edit presents.
        self.assertEqual(adjusted['streams'][0]['bit_rate']['exact'], {'num': 10_000_000, 'den': 1})
        self.assertEqual(adjusted['streams'][1], probe()['streams'][1])
        # The bytes over the media header duration count as the stored rate.
        self.assertIsNotNone(ref._kd_edited_bit_rate(probe(), {**evidence, 'timing': {0: dict(track, btrt=None)}}))
        # ffprobe's rate is neither stored value, the list presents all the
        # media, the list is outside CoreAV's model, or the file is
        # fragmented: not covered.
        for changed in (dict(track, btrt=4_000_000, bytes=5_000_000),
                        dict(track, edits=[(10_000, 0, 0x10000)]),
                        dict(track, edits=[(5000, 0, 0x8000)]),
                        dict(track, edits=[(3000, 0, 0x10000), (3000, 600, 0x10000)])):
            self.assertIsNone(ref._kd_edited_bit_rate(probe(), {**evidence, 'timing': {0: changed}}))
        self.assertIsNone(ref._kd_edited_bit_rate(probe(), {**evidence, 'fragmented': True}))

    def test_decision13_duration(self):
        track = {'timescale': 600, 'mdhd': 6000, 'edits': [], 'bytes': 1, 'btrt': None}
        cases = [([(1000, -1, 0x10000), (5000, 0, 0x10000)], 3000),
                 ([(3000, 0, 0x10000), (1000, -1, 0x10000), (2000, 2400, 0x10000)], 3000),
                 ([(20_000, 0, 0x10000)], 6000),
                 ([(3000, 1200, 0x10000), (3000, 0, 0x10000)], None),
                 ([], None)]
        for edits, expected in cases:
            self.assertEqual(ref.decision13_duration(dict(track, edits=edits), 1000), expected, edits)

    def test_track_timing_reads_the_boxes(self):
        def bx(kind, body):
            return struct.pack('>I', 8 + len(body)) + kind + body
        elst = bx(b'elst', bytes(4) + struct.pack('>I', 1) + struct.pack('>Iii', 5000, 0, 0x10000))
        mdhd = bx(b'mdhd', bytes(4) + struct.pack('>IIII', 0, 0, 600, 6000) + bytes(4))
        btrt = bx(b'btrt', struct.pack('>III', 0, 9, 4000))
        stsd = bx(b'stsd', bytes(4) + struct.pack('>I', 1) + bx(b'mp4a', bytes(28) + btrt))
        stsz = bx(b'stsz', bytes(4) + struct.pack('>II', 0, 3) + struct.pack('>III', 10, 20, 30))
        trak = bx(b'trak', bx(b'edts', elst) + bx(b'mdia', mdhd + bx(b'minf', bx(b'stbl', stsd + stsz))))
        with tempfile.NamedTemporaryFile(suffix='.mp4') as f:
            f.write(bx(b'ftyp', b'isom' + bytes(4)) + bx(b'moov', trak))
            f.flush()
            self.assertEqual(ref.track_timing(f.name), {0: {
                'timescale': 600, 'mdhd': 6000, 'edits': [(5000, 0, 0x10000)], 'bytes': 60, 'btrt': 4000}})

    def test_declared_duration(self):
        oracle = probe()
        evidence = {'fragmented': True, 'mehd': 66_000, 'headers': {'timescale': 100}}
        self.assertEqual(ref._kd_declared_duration(oracle, evidence)['duration']['ticks'],
                         {'num': 660, 'den': 1})
        self.assertIsNone(ref._kd_declared_duration(oracle, dict(evidence, mehd=0)))

    def test_chapter_text(self):
        oracle = probe()
        oracle['dropped_streams'] = [{'stream_index': 2, 'fourcc': 'tx3g', 'duration': None}]
        evidence = {'headers': {'chapters': {2}}, 'entries': {2: ('text', 'tx3g')}}
        adjusted = ref._kd_chapter_text(oracle, evidence)
        self.assertEqual(adjusted['dropped_streams'], [])
        self.assertEqual(adjusted['streams'][-1]['codec'], 'timed_text')
        self.assertIsNone(ref._kd_chapter_text(oracle, dict(evidence, headers={'chapters': set()})))
        self.assertIsNone(ref._kd_chapter_text(oracle, dict(evidence, entries={2: ('meta', 'tx3g')})))

    def test_unidentified_duration(self):
        oracle = probe()
        oracle['streams'] = []
        self.assertIsNone(ref._kd_unidentified_duration(oracle, {'fragmented': True,
                                                                 'mehd': 1000}))
        self.assertIsNone(ref._kd_unidentified_duration(oracle, {'fragmented': False}))
        self.assertIsNone(ref._kd_unidentified_duration(probe(), {'fragmented': True}))
        adjusted = ref._kd_unidentified_duration(oracle, {'fragmented': True, 'mehd': None})
        self.assertIsNone(adjusted['duration'])

    def test_protected_entries_from_box_headers(self):
        def trak(entry):
            stsd = box(b'stsd', bytes(4) + struct.pack('>I', 1) + box(entry, bytes(20)))
            return box(b'trak', box(b'mdia', box(b'minf', box(b'stbl', stsd))))
        data = box(b'ftyp', b'isom\0\0\0\0') + box(b'moov', trak(b'avc1') + trak(b'enca'))
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            self.assertEqual(ref.protected_streams(f.name), {1: 'enca'})


if __name__ == '__main__':
    unittest.main()


def ebml(eid, payload=b'', unknown=False):
    """An EBML element (RFC 8794): ID bytes, a size, the payload."""
    head = eid.to_bytes((eid.bit_length() + 7) // 8, 'big')
    if unknown:
        return head + b'\x01\xff\xff\xff\xff\xff\xff\xff' + payload
    size = len(payload)
    length = next(n for n in range(1, 9) if size < (1 << (7 * n)) - 1)
    return head + ((1 << (7 * length)) | size).to_bytes(length, 'big') + payload


def mkv_probe():
    return {
        'schema': 'coreav.probe-matroska/1',
        'file': {'size': 2_620_000},
        'container': {'doc_type': 'webm', 'live': False},
        'duration': time(10_000, 1000),
        'streams': [
            {'stream_index': 0, 'kind': 'video', 'codec': 'vp8', 'codec_id': 'V_VP8',
             'encoding': 'none', 'packets_available': True, 'frames_listed': True,
             'duration': time(10_000, 1000),
             'bit_rate': {'exact': {'num': 2_000_000, 'den': 1}, 'bits_per_second': 2e6},
             'video': {'width': 1280, 'height': 720, 'rotation': 0, 'parameter_set_size': None,
                       'scanned_size': {'width': 1280, 'height': 720}}, 'audio': None},
            {'stream_index': 1, 'kind': 'audio', 'codec': 'opus', 'codec_id': 'A_OPUS',
             'encoding': 'none', 'packets_available': True, 'frames_listed': True,
             'duration': time(9_990, 1000),
             'bit_rate': {'exact': {'num': 96_000, 'den': 1}, 'bits_per_second': 96e3},
             'video': None, 'audio': {'channels': 2, 'sample_rate': 48_000}},
        ],
        'dropped_streams': [],
    }


class Matroska(unittest.TestCase):
    def test_rules_read_the_matroska_keys(self):
        doc = mkv_probe()
        doc['container'] = {'doc_type': 'matroska', 'live': True}
        doc['streams'][1]['encoding'] = 'encrypted'
        doc['streams'][1]['packets_available'] = False
        doc['streams'][1]['frames_listed'] = False
        doc['dropped_streams'] = [{'stream_index': 2, 'codec_id': 'V_QUICKTIME'}]
        p = policy({'schema': 'coreav.policy/1',
                    'container': {'formats': ['webm'], 'fragmented': False},
                    'streams': {'video': {}, 'audio': {}, 'unidentified': ['V_QUICKTIME']}})
        self.assertEqual([(r['code'], r['rule'], r['path']) for r in ref.evaluate(p, doc)], [
            ('not_allowed', '/container/formats', '/container/doc_type'),
            ('not_allowed', '/container/fragmented', '/container/live'),
            ('not_allowed', '/streams/invalid_tables', '/streams/1/frames_listed'),
            ('not_allowed', '/streams/unidentified', '/streams/1/encoding'),
        ])

    def test_a_missing_duration_takes_the_streams(self):
        doc = mkv_probe()
        doc['duration'] = None
        p = policy({'schema': 'coreav.policy/1', 'file': {'max_bit_rate': 2_000_000},
                    'duration': {'max_seconds': 9.995}, 'streams': {'video': {}, 'audio': {}}})
        reasons = ref.evaluate(p, doc)
        self.assertEqual([(r['code'], r['path']) for r in reasons], [
            ('above_maximum', None), ('above_maximum', '/streams/0/duration')])
        self.assertEqual(reasons[0]['exact'], {'num': 2_096_000, 'den': 1})
        doc['streams'][1]['duration'] = None
        reasons = ref.evaluate(p, doc)
        self.assertEqual([(r['code'], r['path']) for r in reasons], [
            ('unknown', None), ('unknown', '/duration')])

    def test_ffprobe_mapping(self):
        ffprobe = {
            'format': {'size': '1000', 'duration': '2.000000'},
            'streams': [
                {'index': 0, 'codec_type': 'video', 'codec_name': 'vp8', 'width': 320,
                 'height': 240, 'time_base': '1/1000'},
                {'index': 1, 'codec_type': 'subtitle', 'codec_name': 'text',
                 'time_base': '1/1000'},
                {'index': 2, 'codec_type': 'video', 'codec_name': 'rv40',
                 'time_base': '1/1000'},
                {'index': 3, 'codec_type': 'video', 'codec_name': 'mjpeg',
                 'disposition': {'attached_pic': 1}},
            ],
        }
        headers = {'doc_type': 'webm', 'live': False, 'tracks': [
            {'number': 1, 'codec_id': 'V_VP8', 'encoding': 'none', 'default_duration': None},
            {'number': 2, 'codec_id': 'S_TEXT/ASCII', 'encoding': 'none', 'default_duration': None},
            {'number': 3, 'codec_id': 'V_REAL/RV40', 'encoding': 'none', 'default_duration': None},
        ]}
        rows = [(0, 0, 40, 500), (0, 40, 40, 500), (0, None, 40, 7)]
        spans = ref.packet_spans(rows, {0: ref.Fraction(1, 1000)})
        self.assertEqual(spans, {0: (None, 1007, None)})
        spans = ref.packet_spans(rows[:2], {0: ref.Fraction(1, 1000)})
        self.assertEqual(spans, {0: (ref.Fraction(2, 25), 1000, ref.Fraction(1, 25))})
        doc = ref.from_ffprobe_matroska(ffprobe, headers, spans)
        self.assertEqual(doc['container'], {'doc_type': 'webm', 'live': False})
        self.assertEqual([(s['stream_index'], s['codec']) for s in doc['streams']],
                         [(0, 'vp8'), (1, 'subrip')])
        self.assertEqual(doc['streams'][0]['bit_rate']['exact'], {'num': 100_000, 'den': 1})
        # No packets: no frames, a span of 0.
        self.assertEqual(doc['streams'][1]['duration']['ticks'], {'num': 0, 'den': 1})
        self.assertEqual(doc['dropped_streams'], [{'stream_index': 2, 'codec_id': 'V_REAL/RV40'}])

    def test_ebml_walk(self):
        header = ebml(0x1A45DFA3, ebml(0x4282, b'webm\x00\x00'))
        entry = ebml(0xAE, ebml(0xD7, b'\x01') + ebml(0x83, b'\x02') + ebml(0x86, b'A_OPUS')
                     + ebml(0x6D80, ebml(0x6240, ebml(0x5033, b'\x01'))))
        block = ebml(0xA3, b'\x81\x00\x00\x82\x01\x02ab')
        data = header + ebml(0x18538067, ebml(0x1654AE6B, entry)
                             + ebml(0x1F43B675, ebml(0xE7, b'\x00') + block), unknown=True)
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            found = ref.matroska_headers(f.name)
            self.assertEqual(found['doc_type'], 'webm')
            self.assertTrue(found['live'])
            self.assertEqual(found['tracks'], [{'number': 1, 'uid': None, 'type': 2, 'codec_id': 'A_OPUS',
                                                'default_duration': None, 'pixels': None, 'language': None,
                                                'language_bcp47': None, 'encoding': 'encrypted'}])
            self.assertEqual(ref.laced_tracks(f.name), {1})
        with tempfile.NamedTemporaryFile() as f:
            f.write(b'\x00\x00\x00\x18ftypisom')
            f.flush()
            self.assertIsNone(ref.matroska_headers(f.name))

    def test_ebml_track_tags(self):
        # Two tracks; Tags after the cluster: a LANGUAGE SimpleTag for track
        # UID 7, statistics for UID 8, and a tag with no track target.
        header = ebml(0x1A45DFA3, ebml(0x4282, b'matroska'))
        first = ebml(0xAE, ebml(0xD7, b'\x01') + ebml(0x73C5, b'\x07') + ebml(0x83, b'\x02')
                     + ebml(0x22B59C, b'eng'))
        second = ebml(0xAE, ebml(0xD7, b'\x02') + ebml(0x73C5, b'\x08') + ebml(0x83, b'\x02'))
        simple = lambda name: ebml(0x67C8, ebml(0x45A3, name) + ebml(0x4487, b'x'))
        tags = ebml(0x1254C367,
                    ebml(0x7373, ebml(0x63C0, ebml(0x63C5, b'\x07')) + simple(b'LANGUAGE'))
                    + ebml(0x7373, ebml(0x63C0, ebml(0x63C5, b'\x08')) + simple(b'BPS'))
                    + ebml(0x7373, ebml(0x63C0, b'') + simple(b'TITLE')))
        data = header + ebml(0x18538067, ebml(0x1654AE6B, first + second)
                             + ebml(0x1F43B675, ebml(0xE7, b'\x00')) + tags)
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            found = ref.matroska_headers(f.name, tags=True)
            self.assertEqual([(t['uid'], t['language'], t['tag_names']) for t in found['tracks']],
                             [(7, 'eng', ['LANGUAGE']), (8, None, ['BPS'])])
            # Without `tags`, nothing past the tracks is read.
            self.assertNotIn('tag_names', ref.matroska_headers(f.name)['tracks'][0])

    def test_known_divergences(self):
        mapped = mkv_probe()
        mapped['streams'][0]['codec'] = 'mpeg2video'
        mapped['streams'][1]['codec'] = 'vorbis'
        evidence = {'matroska': {'tracks': [{'number': 1, 'default_duration': 40_000_000},
                                            {'number': 2, 'default_duration': None}]},
                    'laced': {2},
                    'spans': {1: (ref.Fraction(999, 100), 10, ref.Fraction(997, 100))}}
        unsized = ref._kd_mkv_unsized(mapped, evidence)
        self.assertIsNone(unsized['streams'][0]['video']['scanned_size'])
        laced = ref._kd_mkv_lace_times(mapped, evidence)
        self.assertIsNone(laced['streams'][1]['duration'])
        self.assertIsNone(ref._kd_mkv_frame_end(laced, evidence), 'already unknown')
        end = ref._kd_mkv_frame_end(mapped, evidence)
        self.assertEqual(end['streams'][1]['duration']['ticks'], {'num': 997, 'den': 100})
        evidence['matroska']['tracks'][1]['encoding'] = 'unsupported'
        unlisted = ref._kd_mkv_unlisted(mapped, evidence)
        self.assertIsNone(unlisted['streams'][1]['duration'])
        self.assertIsNone(unlisted['streams'][1]['bit_rate'])
        self.assertIsNone(ref._kd_mkv_unlisted(unlisted, evidence))
        mapped['streams'][1]['codec'] = 'opus'
        self.assertIsNone(ref._kd_mkv_lace_times(mapped, evidence))
        self.assertIsNone(ref._kd_mkv_frame_end(mapped, evidence))


class MatroskaAudit(unittest.TestCase):
    def audit(self, data):
        with tempfile.NamedTemporaryFile() as f:
            f.write(data)
            f.flush()
            return ref.matroska_audit(f.name)

    def test_structures_that_hide_frames(self):
        header = ebml(0x1A45DFA3, ebml(0x4282, b'webm'))
        tracks = ebml(0x1654AE6B, ebml(0xAE, ebml(0xD7, b'\x01') + ebml(0x83, b'\x01')))
        cluster = ebml(0x1F43B675, ebml(0xE7, b'\x00') + ebml(0xA3, b'\x81\x00\x00\x80ab'))
        self.assertIsNone(self.audit(header + ebml(0x18538067, tracks + cluster)))
        self.assertEqual(self.audit(header + ebml(0x18538067, tracks + cluster) + cluster),
                         'data after the Segment')
        self.assertEqual(self.audit(header + ebml(0x18538067, tracks + tracks + cluster)),
                         'second Info or Tracks')
        stray = ebml(0x1F43B675, ebml(0xA3, b'\x82\x00\x00\x80ab'))
        self.assertEqual(self.audit(header + ebml(0x18538067, tracks + stray)),
                         'block of an undefined track')
        self.assertEqual(self.audit(header + ebml(0x18538067, tracks + cluster)[:-2]),
                         'Cluster past the Segment or the file')

    def test_unverified_adjustment(self):
        mapped = mkv_probe()
        self.assertIsNone(ref._kd_mkv_unverified(mapped, {'audit': None}))
        adjusted = ref._kd_mkv_unverified(mapped, {'audit': 'second Info or Tracks'})
        self.assertTrue(all(s['duration'] is None and s['bit_rate'] is None
                            for s in adjusted['streams']))
        self.assertIsNone(adjusted['streams'][0]['video']['scanned_size'])
