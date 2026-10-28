"""Checks the sanitization requirements of scripts/remux_check.py
(docs/design/sanitize.md) on small files and ffprobe-shaped streams: each
flags a leak and passes a clean output."""
import importlib.util
import pathlib
import struct
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('remux_check', ROOT / 'scripts' / 'remux_check.py')
rc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rc)


def bx(kind, payload):
    return struct.pack('>I', len(payload) + 8) + kind + payload


def full(kind, payload):
    return bx(kind, bytes(4) + payload)


def gapless(value=' 00000000 00000840 000001C0 0000000000A1E7DC' + ' 00000000' * 8, extra=b''):
    """A `meta` box with one `iTunSMPB` item, as sanitization writes it."""
    item = bx(b'----', full(b'mean', b'com.apple.iTunes') + full(b'name', b'iTunSMPB')
              + bx(b'data', struct.pack('>II', 1, 0) + value.encode()) + extra)
    return full(b'meta', bx(b'hdlr', bytes(8) + b'mdirappl' + bytes(9)) + bx(b'ilst', item))


def movie(times=0, udta=False, compressor=b'', track_udta=b'', handler=b'VideoHandler\0', extra_moov=b''):
    mvhd = bx(b'mvhd', bytes(4) + struct.pack('>IIII', times, times, 1000, 0) + bytes(80))
    tkhd = bx(b'tkhd', bytes(4) + struct.pack('>IIIII', times, times, 1, 0, 0) + bytes(60))
    mdhd = bx(b'mdhd', bytes(4) + struct.pack('>IIII', times, times, 1000, 0) + bytes(4))
    name = bytes([len(compressor)]) + compressor + bytes(31 - len(compressor))
    entry = bx(b'avc1', bytes(6) + b'\0\x01' + bytes(34) + name + b'\0\x18\xff\xff' + bx(b'avcC', b'\x01'))
    stsd = bx(b'stsd', bytes(4) + struct.pack('>I', 1) + entry)
    hdlr = bx(b'hdlr', bytes(8) + b'vide' + bytes(12) + handler)
    trak_udta = bx(b'udta', track_udta) if track_udta else b''
    trak = bx(b'trak', tkhd + bx(b'mdia', mdhd + hdlr + bx(b'minf', bx(b'stbl', stsd))) + trak_udta)
    if udta is True:
        udta = bx(b'\xa9xyz', b'+1+2/')
    extra = bx(b'udta', udta) if udta else b''
    return bx(b'ftyp', b'isom\0\0\0\0') + bx(b'moov', mvhd + trak + extra + extra_moov)


class Requirements(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def write(self, name, data):
        path = pathlib.Path(self.dir.name) / name
        path.write_bytes(data)
        return str(path)

    def test_boxes(self):
        self.assertEqual(rc.sanitized_boxes(self.write('clean.mp4', movie()), set()), [])
        leaks = [p['message'] for p in rc.sanitized_boxes(self.write('udta.mp4', movie(udta=True)), set())]
        self.assertEqual(len(leaks), 1)
        self.assertIn('moov/udta holds', leaks[0])
        timed = [p['message'] for p in rc.sanitized_boxes(self.write('times.mp4', movie(times=7)), set())]
        self.assertEqual(len(timed), 3)
        self.assertEqual(rc.sanitized_boxes(self.write('kept.mp4', movie(times=7)), {'dates'}), [])
        named = rc.sanitized_boxes(self.write('name.mp4', movie(compressor=b'Camera')), set())
        self.assertEqual(len(named), 1)
        self.assertIn('compressor name', named[0]['message'])
        # Boxes outside the allowlisted tree, and handler names other than
        # the generic ones.
        tree = rc.sanitized_boxes(self.write('uuid.mp4', movie(extra_moov=bx(b'uuid', bytes(20)))), set())
        self.assertEqual(len(tree), 1)
        handler = rc.sanitized_boxes(self.write('hdlr.mp4', movie(handler=b'Core Media Video\0')), set())
        self.assertEqual(len(handler), 1)

    def test_kept_user_data(self):
        keep = {'gapless', 'kinds'}
        role = full(b'kind', b'urn:mpeg:dash:role:2011\0forced-subtitle\0')
        kept = self.write('kept.mp4', movie(udta=gapless(), track_udta=role))
        self.assertEqual(rc.sanitized_boxes(kept, keep), [])
        # Not kept unless asked for.
        self.assertEqual(len(rc.sanitized_boxes(kept, {'kinds'})), 1)
        self.assertEqual(len(rc.sanitized_boxes(kept, {'gapless'})), 1)
        # Kinds only in a track and from the vocabulary.
        self.assertEqual(len(rc.sanitized_boxes(self.write('movie_kind.mp4', movie(udta=role)), keep)), 1)
        for kind in [b'urn:x\0\xa9\0', b'urn:x\0value\0more',
                     b'urn:mpeg:dash:role:2011\0Alice at +37.7\0']:
            leaks = rc.sanitized_boxes(self.write('kind.mp4', movie(track_udta=full(b'kind', kind))), keep)
            self.assertEqual(len(leaks), 1, kind)
        for udta in [gapless(value=' 00000000 00000840 000001C0 0000000000A1E7DC DEADBEEF'),
                     gapless(extra=bx(b'free', b'x')),
                     gapless() + bx(b'\xa9nam', b'Title'),
                     gapless().replace(b'mdirappl', b'mdirAlic')]:
            leaks = rc.sanitized_boxes(self.write('leak.mp4', movie(udta=udta)), keep)
            self.assertEqual(len(leaks), 1, udta)

    def test_hevc_parameter_sets(self):
        def array(nal_type, *nalus):
            return bytes([0x80 | nal_type]) + struct.pack('>H', len(nalus)) + b''.join(
                struct.pack('>H', len(n)) + n for n in nalus)
        # NAL unit headers carry the type in bits 1 to 6.
        sets = array(32, b'\x40vps') + array(33, b'\x42sps') + array(34, b'\x44pps')
        clean = bytes(22) + b'\x03' + sets
        self.assertEqual(rc.hevc_parameter_sets(clean), clean)
        noisy = bytes(22) + b'\x04' + array(39, b'\x4ex265 options') + sets + b'tail'
        self.assertEqual(rc.hevc_parameter_sets(noisy), clean)
        hidden = bytes(22) + b'\x03' + array(32, b'\x40vps') + array(33, b'\x42sps', b'\x4eSECRET') \
            + array(34, b'\x44pps')
        self.assertEqual(rc.hevc_parameter_sets(hidden), clean)
        self.assertIsNone(rc.hevc_parameter_sets(clean[:-1]))

    def test_avc_parameter_sets(self):
        def units(*nalus):
            return b''.join(struct.pack('>H', len(n)) + n for n in nalus)
        clean = bytes([1, 77, 0, 30, 0xFF, 0xE1]) + units(b'\x67sps') + b'\x01' + units(b'\x68pps')
        self.assertEqual(rc.avc_parameter_sets(clean), clean)
        noisy = bytes([1, 77, 0, 30, 0xFF, 0xE2]) + units(b'\x67sps', b'\x06SECRET') + b'\x01' \
            + units(b'\x68pps') + b'tail'
        self.assertEqual(rc.avc_parameter_sets(noisy), clean)
        self.assertIsNone(rc.avc_parameter_sets(clean[:-1]))

    def test_spherical_and_fixed_sizes(self):
        written = bx(b'svhd', bytes(5)) + bx(b'proj', bx(b'prhd', bytes(16)))
        self.assertTrue(rc.spherical_written(written))
        self.assertFalse(rc.spherical_written(bx(b'svhd', bytes(4) + b'Cam\0') + bx(b'proj', b'')))
        self.assertFalse(rc.spherical_written(written + bx(b'xtra', b'SECRET')))
        self.assertEqual(rc.FIXED_SIZES['pasp'], 8)

    def test_stream_tags(self):
        source = {'tags': {'language': 'kor', 'creation_time': 'x', 'encoder': 'Cam'}}
        clean = {'tags': {'language': 'kor', 'handler_name': 'VideoHandler', 'vendor_id': '[0][0][0][0]'}}
        self.assertEqual(rc.sanitized_stream_tags(0, source, clean, {'language'}), [])
        for key, value in [('handler_name', 'Core Media Video'), ('vendor_id', 'appl')]:
            named = dict(clean, tags=dict(clean['tags'], **{key: value}))
            self.assertEqual(len(rc.sanitized_stream_tags(0, source, named, {'language'})), 1)
        leaked = dict(clean, tags=dict(clean['tags'], encoder='Cam'))
        self.assertEqual(len(rc.sanitized_stream_tags(0, source, leaked, {'language'})), 1)
        # The language is cleared unless kept.
        self.assertEqual(len(rc.sanitized_stream_tags(0, source, clean, set())), 1)
        cleared = dict(clean, tags=dict(clean['tags'], language='und'))
        self.assertEqual(rc.sanitized_stream_tags(0, source, cleared, set()), [])
        # Creation times only with the dates.
        dated = dict(clean, tags=dict(clean['tags'], creation_time='x'))
        self.assertEqual(len(rc.sanitized_stream_tags(0, source, dated, {'language'})), 1)
        self.assertEqual(rc.sanitized_stream_tags(0, source, dated, {'language', 'dates'}), [])


if __name__ == '__main__':
    unittest.main()
