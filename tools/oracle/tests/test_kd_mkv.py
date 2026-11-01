"""Keeps docs/known-divergences-mkv.md, its JSON twin and
scripts/oracle_kd_mkv.py in sync, and checks that each predicate rejects
near misses."""
import importlib.util
import io
import json
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('oracle_kd_mkv', ROOT / 'scripts' / 'oracle_kd_mkv.py')
kd = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(kd)


def stream(codec='h264', kind='video', **properties):
    return {'codec_name': codec, 'kind': kind, 'properties': properties}


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-mkv.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-mkv.md').read_text()
        documented = set(re.findall(r'^## (KD-[A-Z0-9-]+)$', page, re.M))
        ids = {entry['id'] for entry in twin['entries']}
        self.assertEqual(documented, ids)
        for entry in twin['entries']:
            self.assertIn(entry['predicate'], kd.PREDICATES, entry['id'])
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')
        source = (ROOT / 'scripts' / 'oracle_kd_mkv.py').read_text()
        for entry in ids:
            self.assertIn(f"'{entry}'", source, entry)


class Predicates(unittest.TestCase):
    def test_dts_only_for_reordering_codecs(self):
        ref, cand = {'dts': 40}, {'dts': None}
        self.assertTrue(kd.dts_unknown(stream('h264'), ref, cand))
        self.assertFalse(kd.dts_unknown(stream('vp9'), ref, cand))
        self.assertFalse(kd.dts_unknown(stream('h264'), ref, {'dts': 41}))

    def test_lace_rules(self):
        untimed = stream('opus', 'audio', default_duration_ns=None)
        later = {'lace': 1, 'pts': None, 'dts': None}
        self.assertTrue(kd.lace_without_duration(untimed, {'pts': 19}, later))
        self.assertFalse(kd.lace_without_duration(untimed, {'pts': 19}, dict(later, lace=0)))
        self.assertFalse(kd.lace_without_duration(stream('opus', 'audio', default_duration_ns=20_000_000),
                                                  {'pts': 19}, later))
        timed = stream('aac', 'audio', default_duration_ns=21_333_333, codec_delay_ns=21_333_333)
        packet = {'lace': 7, 'frames_in_block': 8, 'block_timestamp': 0, 'pts': 128, 'dts': 128}
        self.assertTrue(kd.lace_split(timed, {'pts': 127, 'dts': 127}, packet, 1_000_000))
        # Two ticks apart, or a reference that follows neither rule.
        self.assertFalse(kd.lace_split(timed, {'pts': 126, 'dts': 126}, packet, 1_000_000))
        self.assertFalse(kd.lace_split(timed, {'pts': 127, 'dts': 127}, dict(packet, pts=129), 1_000_000))

    def test_keyframes_timestamps_and_sizes(self):
        self.assertTrue(kd.audio_keyframe(stream('aac', 'audio'), {'keyframe': True}, {'keyframe': False}))
        self.assertFalse(kd.audio_keyframe(stream('h264'), {'keyframe': True}, {'keyframe': False}))
        vfw = stream('mpeg4', codec_id='V_MS/VFW/FOURCC')
        self.assertTrue(kd.vfw_timestamps(vfw, {'pts': None, 'dts': 40},
                                          {'pts': 40, 'dts': None, 'block_timestamp': 40}))
        self.assertFalse(kd.vfw_timestamps(stream('mpeg4'), {'pts': None, 'dts': 40},
                                           {'pts': 40, 'dts': None, 'block_timestamp': 40}))
        vtt = stream('webvtt', 'subtitle', codec_id='D_WEBVTT/SUBTITLES')
        self.assertTrue(kd.webm_webvtt_size(vtt, {'size': 5, 'pts': 0}, {'size': 7, 'pts': 0}))
        self.assertFalse(kd.webm_webvtt_size(vtt, {'size': 5, 'pts': 0}, {'size': 6, 'pts': 0}))
        self.assertFalse(kd.webm_webvtt_size(stream('subrip', 'subtitle', codec_id='S_TEXT/UTF8'),
                                             {'size': 5, 'pts': 0}, {'size': 7, 'pts': 0}))

    def test_stream_level_rules(self):
        reference = {'format_duration_us': 2_000_000}
        sub = {'kind': 'subtitle', 'duration_ts': 2000, 'time_base': [1, 1000]}
        self.assertTrue(kd.segment_duration(reference, sub, {'duration_ts': None}))
        self.assertFalse(kd.segment_duration(reference, dict(sub, duration_ts=1999), {'duration_ts': None}))
        self.assertFalse(kd.segment_duration(reference, sub, {'duration_ts': 2000}))
        candidate = {'format_duration_us': 2_980_209,
                     'properties': {'raw_duration': 2980.2085, 'timestamp_scale': 1_000_000}}
        self.assertTrue(kd.format_rounding({'format_duration_us': 2_980_208}, candidate))
        self.assertFalse(kd.format_rounding({'format_duration_us': 2_980_207}, candidate))
        whole = {'format_duration_us': 2_000_000, 'properties': {'raw_duration': 2000.0, 'timestamp_scale': 1_000_000}}
        self.assertFalse(kd.format_rounding({'format_duration_us': 1_999_999}, whole))
        self.assertTrue(kd.unread_bitstream({'codec_name': 'mpeg2video', 'profile': None}, {'profile': 'Main'},
                                            'profile'))
        self.assertFalse(kd.unread_bitstream({'codec_name': 'h264', 'profile': None}, {'profile': 'Main'},
                                             'profile'))
        hevc = {'codec_name': 'hevc', 'color_range': None, 'color_primaries': None, 'color_transfer': None,
                'color_matrix': None}
        self.assertTrue(kd.hevc_unsignalled_range(hevc, {'color_range': 'tv'}))
        self.assertFalse(kd.hevc_unsignalled_range(hevc, {'color_range': 'pc'}))
        self.assertFalse(kd.hevc_unsignalled_range(dict(hevc, color_matrix='bt709'), {'color_range': 'tv'}))

    def test_attachments_must_match_in_number_and_kind(self):
        candidate = {'streams': [{}], 'properties': {'attachments': [{'media_type': 'font/ttf'}]}}
        self.assertTrue(kd.attachments({'streams': [{}, {'kind': 'attachment'}]}, candidate))
        self.assertFalse(kd.attachments({'streams': [{}, {'kind': 'video', 'codec_name': 'h264'}]}, candidate))
        self.assertFalse(kd.attachments({'streams': [{}, {'kind': 'attachment'}, {'kind': 'attachment'}]},
                                        candidate))
        art = {'streams': [{}], 'properties': {'attachments': [{'media_type': 'image/jpeg'}]}}
        self.assertTrue(kd.attachments({'streams': [{}, {'kind': 'video', 'codec_name': 'mjpeg'}]}, art))


def ebml(eid, payload=b''):
    """An EBML element (RFC 8794): ID bytes, a size, the payload."""
    head = eid.to_bytes((eid.bit_length() + 7) // 8, 'big')
    length = next(n for n in range(1, 9) if len(payload) < (1 << (7 * n)) - 1)
    return head + ((1 << (7 * length)) | len(payload)).to_bytes(length, 'big') + payload


class TagLanguage(unittest.TestCase):
    """KD-MKV-TAG-LANGUAGE: covered only with the file's evidence."""

    def file(self, language=b'eng', tag=b'LANGUAGE', target=b'\x07', bcp47=None):
        stored = ebml(0x22B59C, language) if language is not None else b''
        stored += ebml(0x22B59D, bcp47) if bcp47 is not None else b''
        entry = ebml(0xAE, ebml(0xD7, b'\x01') + ebml(0x73C5, b'\x07') + ebml(0x83, b'\x02') + stored)
        tags = ebml(0x1254C367, ebml(0x7373, ebml(0x63C0, ebml(0x63C5, target))
                                     + ebml(0x67C8, ebml(0x45A3, tag) + ebml(0x4487, b'x'))))
        data = (ebml(0x1A45DFA3, ebml(0x4282, b'matroska'))
                + ebml(0x18538067, ebml(0x1654AE6B, entry) + tags))
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        (pathlib.Path(tmp.name) / 'a.mkv').write_bytes(data)
        return tmp.name

    def covered(self, media, ours='eng', theirs=None):
        theirs = {'LANGUAGE': 'eng'} if theirs is None else theirs
        return kd.tag_language(media, 'a.mkv', 0, {'tags': theirs}, {'tags': {'language': ours}})

    def test_the_stored_language_beside_a_language_tag(self):
        self.assertTrue(self.covered(self.file()))

    def test_mutations_stay_uncovered(self):
        self.assertFalse(self.covered(self.file(), ours='fre'))            # not the stored element
        self.assertFalse(self.covered(self.file(language=None)))           # no stored element
        self.assertFalse(self.covered(self.file(bcp47=b'en')))             # LanguageBCP47 overrides it
        self.assertFalse(self.covered(self.file(tag=b'TITLE')))            # no LANGUAGE tag
        self.assertFalse(self.covered(self.file(target=b'\x09')))          # the tag targets another track
        self.assertFalse(self.covered(self.file(), theirs={'language': 'eng'}))   # ffprobe has the key
        self.assertFalse(self.covered(self.file(), theirs={}))             # ffprobe lists no LANGUAGE
        self.assertFalse(kd.tag_language(self.file(), 'a.mkv', 1, {'tags': {'LANGUAGE': 'x'}},
                                         {'tags': {'language': 'eng'}}))   # no such track

    def test_it_is_the_compat_entry_for_the_language(self):
        self.assertEqual(kd.COMPAT_ENTRIES['streams.tags.language'], ('KD-MKV-TAG-LANGUAGE', kd.tag_language))


if __name__ == '__main__':
    unittest.main()


class DeclaredSize(unittest.TestCase):
    def test_needs_the_header_and_the_decoded_frame(self):
        import tempfile
        from unittest import mock

        def ebml(eid, payload=b''):
            head = eid.to_bytes((eid.bit_length() + 7) // 8, 'big')
            return head + bytes([0x80 | len(payload)]) + payload

        video = ebml(0xE0, ebml(0xB0, b'\xa0') + ebml(0xBA, b'\x78'))
        entry = ebml(0xAE, ebml(0xD7, b'\x01') + ebml(0x83, b'\x01') + video)
        data = ebml(0x1A45DFA3, ebml(0x4282, b'matroska')) + ebml(0x18538067, ebml(0x1654AE6B, entry))
        ours, theirs = {'width': 160, 'height': 120}, {'width': 320, 'height': 240}
        with tempfile.TemporaryDirectory() as media:
            (pathlib.Path(media) / 'f.mkv').write_bytes(data)
            with mock.patch.object(kd, '_first_frame_size', return_value=(320, 240)):
                self.assertTrue(kd.declared_size(media, 'f.mkv', 0, theirs, ours))
                self.assertFalse(kd.declared_size(None, 'f.mkv', 0, theirs, ours), 'no media')
                self.assertFalse(kd.declared_size(media, 'f.mkv', 0, theirs, theirs))
                self.assertFalse(kd.declared_size(media, 'f.mkv', 0, ours, {'width': 1, 'height': 1}))
            with mock.patch.object(kd, '_first_frame_size', return_value=(640, 480)):
                self.assertFalse(kd.declared_size(media, 'f.mkv', 0, theirs, ours))



def ebml(eid, payload=b''):
    return eid.to_bytes((eid.bit_length() + 7) // 8, 'big') + (1 << 56 | len(payload)).to_bytes(8, 'big') + payload


def pinned(tool):
    """Whether the `tool` (`ffmpeg` or `ffprobe`) on the path is the pinned
    oracle build."""
    spec = importlib.util.spec_from_file_location('oracle_pin', ROOT / 'scripts' / 'oracle_pin.py')
    pin = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(pin)
    try:
        version = subprocess.run([tool, '-version'], capture_output=True, timeout=60, check=False).stdout
    except (OSError, subprocess.TimeoutExpired):
        return False
    return version.startswith(f"{tool} version {pin.load()['BUILD']}-".encode())


def ac3_frame(bsid=6, acmod=7, xbsi1e=1, mode=1, levels=(4, 4, 4, 4), crc=True):
    """A 48 kHz AC-3 syncframe of 64 words (frame size code 0) with an LFE
    channel, whose alternate syntax states the preferred downmix `mode` and
    the Lt/Rt and Lo/Ro centre and surround level codes `levels`; its
    `crc1` holds when `crc`."""
    bits = []

    def put(value, n):
        bits.extend(value >> (n - 1 - i) & 1 for i in range(n))

    put(0, 2), put(0, 6), put(bsid, 5), put(0, 3), put(acmod, 3)
    if acmod & 1 and acmod != 1:
        put(0, 2)
    if acmod & 4:
        put(0, 2)
    if acmod == 2:
        put(0, 2)
    put(1, 1), put(31, 5), put(0, 3)
    if acmod == 0:
        put(31, 5), put(0, 3)
    put(0, 2)
    if bsid == 6:
        put(xbsi1e, 1)
        if xbsi1e:
            put(mode, 2)
            for level in (levels[0], levels[1], levels[2], levels[3]):
                put(level, 3)
        put(0, 1)
    else:
        put(0, 2)
    put(0, 1)
    bits += [0] * (-len(bits) % 8)
    head = bytes(int(''.join(map(str, bits[i:i + 8])), 2) for i in range(0, len(bits), 8))
    frame = bytearray(b'\x0b\x77\0\0' + head + bytes(128 - 4 - len(head)))
    covered = 2 * ((64 >> 1) + (64 >> 3))
    # crc1 leads the bytes it covers: find, by the CRC's linearity, the two
    # bytes that leave the register at zero.
    target = kd.ac3_crc(frame[2:covered])
    basis = [kd.ac3_crc((1 << bit).to_bytes(2, 'big') + bytes(covered - 4)) for bit in range(16)]
    for value in range(1 << 16):
        acc = 0
        for bit in range(16):
            if value >> bit & 1:
                acc ^= basis[bit]
        if acc == target:
            break
    frame[2:4] = value.to_bytes(2, 'big')
    if not crc:
        frame[covered - 1] ^= 1
    return bytes(frame)


def ac3_matroska(blocks, codec=b'A_AC3', encoded=False, before=b'', clusters=None):
    """A Matroska file whose track entries are `before` and then one AC-3
    track, number 2 after `before` and 1 otherwise, with one cluster of
    `blocks` (each the bytes after a SimpleBlock's flags, and its lacing
    bits), or the elements `clusters` instead."""
    number = 2 if before else 1
    entry = (ebml(0xD7, bytes([number])) + ebml(0x73C5, bytes([number])) + ebml(0x83, b'\x02') + ebml(0x86, codec)
             + ebml(0xE1, ebml(0xB5, b'\x46\xbb\x80\x00') + ebml(0x9F, b'\x06')))
    if encoded:
        entry += ebml(0x6D80, ebml(0x6240, ebml(0x5034, ebml(0x4254, b'\x03') + ebml(0x4255, b'\x0b\x77'))))
    cluster = b''.join(ebml(0xA3, bytes([0x80 | number, 0, 32 * i, 0x80 | lacing << 1]) + data)
                       for i, (data, lacing) in enumerate(blocks))
    return (ebml(0x1A45DFA3, ebml(0x4282, b'matroska') + ebml(0x4287, b'\x04') + ebml(0x4285, b'\x02'))
            + ebml(0x18538067, ebml(0x1549A966, ebml(0x2AD7B1, b'\x0f\x42\x40'))
                   + ebml(0x1654AE6B, before + ebml(0xAE, entry))
                   + (ebml(0x1F43B675, ebml(0xE7, b'\x00') + cluster) if clusters is None else clusters)))


def ac3_cluster(*frames, tail=b'', known=True):
    """A cluster of track 1 with a SimpleBlock per frame and then `tail`, of
    unknown size unless `known`."""
    payload = ebml(0xE7, b'\x00') + b''.join(ebml(0xA3, bytes([0x81, 0, 32 * i, 0x80]) + frame)
                                             for i, frame in enumerate(frames)) + tail
    return ebml(0x1F43B675, payload) if known else b'\x1f\x43\xb6\x75\x01' + b'\xff' * 7 + payload


def single(*frames):
    return [(frame, 0) for frame in frames]


class AnnexDDownmix(unittest.TestCase):
    def evidence(self, data):
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        path = pathlib.Path(directory) / 'f.mkv'
        path.write_bytes(data)
        return kd.annex_d_downmix(path)

    def test_every_frame_states_the_same_values(self):
        frame = ac3_frame()
        stated = ('1', '0.707107', '0.707107', '0.707107', '0.707107')
        self.assertEqual(self.evidence(ac3_matroska(single(frame, frame, frame))),
                         [kd.Ac3Downmix(0, 1, 3, kd.NO_DOWNMIX, stated)])
        # Each lacing: Xiph (sizes as 255s and a remainder), fixed, EBML.
        laces = [(bytes([2, 128, 128]) + frame * 3, 1), (bytes([2]) + frame * 3, 2),
                 (bytes([2, 0x40, 128, 0xBF]) + frame * 3, 3)]
        self.assertEqual(self.evidence(ac3_matroska(laces)), [kd.Ac3Downmix(0, 1, 9, kd.NO_DOWNMIX, stated)])

    def test_values_only_where_annex_d_defines_them(self):
        # No surround channels (3/0) keeps no surround level; no centre
        # (2/2) no centre level; a muting code is 0.
        self.assertEqual(self.evidence(ac3_matroska(single(ac3_frame(acmod=3, mode=2, levels=(0, 5, 6, 3)))))[0].coreav,
                         ('2', '1.414214', '0.000000', '0.500000', '0.000000'))
        self.assertEqual(self.evidence(ac3_matroska(single(ac3_frame(acmod=6, levels=(0, 7, 6, 3)))))[0].coreav,
                         ('1', '0.000000', '0.000000', '0.000000', '0.840896'))

    def test_track_position_and_number(self):
        video = ebml(0xAE, ebml(0xD7, b'\x01') + ebml(0x83, b'\x01') + ebml(0x86, b'V_VP9'))
        found = self.evidence(ac3_matroska(single(ac3_frame()), before=video))
        self.assertEqual([(e.position, e.number) for e in found], [(1, 2)])

    def test_other_shapes_give_no_evidence(self):
        frame = ac3_frame()
        for name, blocks, options in (
                ('frames that disagree', single(frame, ac3_frame(mode=2)), {}),
                ('a later frame that disagrees', single(frame, frame, ac3_frame(levels=(4, 4, 5, 4))), {}),
                ('bsid 8', single(ac3_frame(bsid=8)), {}),
                ('a later frame of bsid 8', single(frame, ac3_frame(bsid=8)), {}),
                ('two channels', single(ac3_frame(acmod=2)), {}),
                ('xbsi1e 0', single(ac3_frame(xbsi1e=0)), {}),
                ('a reserved surround level', single(ac3_frame(levels=(4, 2, 4, 4))), {}),
                ('crc1 failing', single(ac3_frame(crc=False)), {}),
                ('a frame of two syncframes', single(frame + frame), {}),
                ('a trailing byte', single(frame + b'\0'), {}),
                ('a cut frame', single(frame[:-1]), {}),
                ('no downmix stated', single(ac3_frame(mode=0, levels=(7, 7, 7, 7))), {}),
                ('E-AC-3', single(frame), {'codec': b'A_EAC3'}),
                ('header stripping', single(frame), {'encoded': True}),
                ('lace sizes past the block', [(bytes([2, 200]) + frame * 2, 1)], {}),
                ('no frames', [], {}),
        ):
            self.assertEqual(self.evidence(ac3_matroska(blocks, **options)), [], name)

    def test_a_walk_that_cannot_reach_every_block_gives_no_evidence(self):
        frame, other = ac3_frame(), ac3_frame(mode=2)
        stated = ('1', '0.707107', '0.707107', '0.707107', '0.707107')
        twice = ac3_matroska([], clusters=ac3_cluster(frame) + ac3_cluster(frame))
        self.assertEqual(self.evidence(twice), [kd.Ac3Downmix(0, 1, 2, kd.NO_DOWNMIX, stated)])
        for name, data in (
                # A cluster of unknown size, whatever follows it.
                ('unknown size, then the same', ac3_matroska([], clusters=ac3_cluster(frame, known=False)
                                                             + ac3_cluster(frame))),
                ('unknown size, then another', ac3_matroska([], clusters=ac3_cluster(frame, known=False)
                                                            + ac3_cluster(other))),
                # A child header cut short at the end of its cluster.
                ('a cut header', ac3_matroska([], clusters=ac3_cluster(frame, tail=b'\xa3')
                                              + ac3_cluster(frame))),
                # The file ends before its Segment does.
                ('a cut file', twice[:-1]),
        ):
            self.assertEqual(self.evidence(data), [], name)
        # The element cap refuses a walk it would end early: the Segment has
        # Info, Tracks and four clusters.
        four = ac3_matroska([], clusters=ac3_cluster(frame) * 4)
        with mock.patch.object(kd.policy_ref, 'MAX_ELEMENTS', 6):
            self.assertEqual(len(self.evidence(four)), 1)
        with mock.patch.object(kd.policy_ref, 'MAX_ELEMENTS', 5):
            self.assertEqual(self.evidence(four), [])

    def test_a_block_is_read_only_within_its_element_and_the_size_bound(self):
        frame = ac3_frame()
        whole = ac3_matroska(single(frame))
        # A SimpleBlock that declares 1 TiB in a file of a few hundred bytes.
        huge = b'\xa3' + (1 << 56 | 1 << 40).to_bytes(8, 'big') + bytes([0x81, 0, 0, 0x80]) + frame
        reads = []

        class Recorded(io.BytesIO):
            def read(self, n=-1):
                reads.append(n)
                return super().read(n)

        def recorded(path, mode='r'):
            return Recorded(pathlib.Path(path).read_bytes())
        with mock.patch.object(kd.policy_ref, 'open', recorded, create=True):
            self.assertEqual(self.evidence(ac3_matroska([], clusters=ac3_cluster(frame, tail=huge))), [])
        self.assertLess(max(reads), len(whole) + 64)
        with mock.patch.object(kd.policy_ref, 'MAX_BLOCK', 4 + len(frame)):
            self.assertEqual(len(self.evidence(whole)), 1)
        with mock.patch.object(kd.policy_ref, 'MAX_BLOCK', 3 + len(frame)):
            self.assertEqual(self.evidence(whole), [])

    @unittest.skipUnless(pinned('ffmpeg') and pinned('ffprobe'), 'needs the pinned ffmpeg and ffprobe on the path')
    def test_the_pinned_oracle_still_prints_no_downmix(self):
        directory = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, directory)
        path = directory / 'f.mkv'
        subprocess.run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', 'sine=duration=0.2', '-af',
                        'pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0', '-c:a', 'ac3', '-dmix_mode', 'ltrt',
                        '-ltrt_cmixlev', '0.707', '-ltrt_surmixlev', '0.5', '-loro_cmixlev', '0.595',
                        '-loro_surmixlev', '0.707', str(path)], check=True, timeout=120)
        (found,) = kd.annex_d_downmix(path)
        self.assertEqual(found.coreav, ('1', '0.707107', '0.500000', '0.594604', '0.707107'))
        listed = subprocess.run(['ffprobe', '-v', 'error', '-of', 'json', '-show_entries',
                                 'stream=' + ','.join(kd.DOWNMIX_KEYS), str(path)],
                                capture_output=True, timeout=60, check=True).stdout
        (stream,) = json.loads(listed)['streams']
        printed = tuple(stream.get(key) for key in kd.DOWNMIX_KEYS)
        self.assertNotEqual(printed, found.coreav, 'KD-FF9-ANNEX-D-DOWNMIX: the oracle now agrees; retire the record')
        self.assertEqual(printed, found.oracle, 'KD-FF9-ANNEX-D-DOWNMIX: the oracle values changed')


def av1_private(profile=0, level=5, tier=0, depth=8, mono=0, colour=None, full=0, position=0, more=b''):
    """An AV1 `CodecPrivate`: the four octets, then a sequence header OBU of
    a 64x64 stream with one operating point, then `more`. `colour` is the
    described (primaries, transfer, matrix), or None for no description."""
    bits = []

    def put(value, n):
        bits.extend(value >> (n - 1 - i) & 1 for i in range(n))

    put(profile, 3), put(0, 2), put(0, 2), put(0, 5), put(0, 12), put(level, 5)
    if level > 7:
        put(tier, 1)
    put(15, 4), put(15, 4), put(63, 16), put(63, 16), put(0, 4), put(0, 5), put(0, 2), put(0, 3)
    put(int(depth > 8), 1)
    if profile == 2 and depth > 8:
        put(int(depth == 12), 1)
    if profile != 1:
        put(mono, 1)
    put(int(colour is not None), 1)
    for value in colour or ():
        put(value, 8)
    put(full, 1)
    subsampling = (1, 1) if mono or profile == 0 else (0, 0)
    if not mono:
        if subsampling == (1, 1):
            put(position, 2)
        put(0, 1)
    put(0, 1), put(1, 1)
    bits += [0] * (-len(bits) % 8)
    header = bytes(int(''.join(map(str, bits[i:i + 8])), 2) for i in range(0, len(bits), 8))
    flags = (tier << 7 | int(depth > 8) << 6 | int(depth == 12) << 5 | mono << 4
             | subsampling[0] << 3 | subsampling[1] << 2 | (position if subsampling == (1, 1) else 0))
    return bytes([0x81, profile << 5 | level, flags, 0, 0x0A, len(header)]) + header + more


def av1_matroska(private, colour=None, codec=b'V_AV1', encoded=False, before=b'', declared=None, entry=0,
                 tracks=0):
    """A Matroska file whose track entries are `before` and then one AV1
    video track, number 2 after `before` and 1 otherwise, with `colour` (by
    element id) as its `Colour`, and one cluster of one empty key frame.
    With `declared`, the `CodecPrivate` comes last in its entry and declares
    that size; `entry` and `tracks` add to the sizes the entry and `Tracks`
    declare."""
    number = 2 if before else 1
    video = ebml(0xB0, b'\x40') + ebml(0xBA, b'\x40')
    if colour is not None:
        video += ebml(0x55B0, b''.join(ebml(eid, bytes([value])) for eid, value in colour.items()))
    fields = ebml(0xD7, bytes([number])) + ebml(0x73C5, bytes([number])) + ebml(0x83, b'\x01') + ebml(0x86, codec)
    if declared is None:
        fields += ebml(0x63A2, private) + ebml(0xE0, video)
    else:
        fields += ebml(0xE0, video) + b'\x63\xa2' + (1 << 56 | declared).to_bytes(8, 'big') + private
    if encoded:
        fields += ebml(0x6D80, ebml(0x6240, ebml(0x5034, ebml(0x4254, b'\x03') + ebml(0x4255, b'\x00'))))
    entry = b'\xae' + (1 << 56 | len(fields) + entry).to_bytes(8, 'big') + fields
    block = ebml(0xA3, bytes([0x80 | number, 0, 0, 0x80]))
    return (ebml(0x1A45DFA3, ebml(0x4282, b'matroska') + ebml(0x4287, b'\x04') + ebml(0x4285, b'\x02'))
            + ebml(0x18538067, ebml(0x1549A966, ebml(0x2AD7B1, b'\x0f\x42\x40'))
                   + b'\x16\x54\xae\x6b' + (1 << 56 | len(before + entry) + tracks).to_bytes(8, 'big') + before + entry
                   + ebml(0x1F43B675, ebml(0xE7, b'\x00') + block)))


def pinned_ffprobe():
    """Whether the `ffprobe` on the path is the pinned oracle build."""
    spec = importlib.util.spec_from_file_location('oracle_pin', ROOT / 'scripts' / 'oracle_pin.py')
    pin = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(pin)
    try:
        version = subprocess.run(['ffprobe', '-version'], capture_output=True, timeout=60, check=False).stdout
    except (OSError, subprocess.TimeoutExpired):
        return False
    return version.startswith(f"ffprobe version {pin.load()['BUILD']}-".encode())


# Range 1 is studio swing; ChromaSitingHorz 1 and ChromaSitingVert 2 are
# chroma sample position 1 (the AV1 codec mapping in Matroska).
BT709 = {0x55B9: 1, 0x55BB: 1, 0x55BA: 1, 0x55B1: 1}
VERTICAL = {0x55B9: 1, 0x55B7: 1, 0x55B8: 2}
RECORDS = {kd.av1_default_suffix: 'KD-FF9-AV1-DEFAULT-SUFFIX', kd.av1_chroma_position: 'KD-FF9-AV1-CHROMA-POSITION'}


class Av1CodecStrings(unittest.TestCase):
    SHAPES = {
        'stated BT.709': (av1_private(colour=(1, 1, 1)), BT709, kd.av1_default_suffix,
                          'av01.0.05M.08.0.110.01.01.01.0', 'av01.0.05M.08'),
        'high tier, no Colour': (av1_private(level=23, tier=1, colour=(1, 1, 1)), None, kd.av1_default_suffix,
                                 'av01.0.23H.08.0.110.01.01.01.0', 'av01.0.23H.08'),
        'chroma position 1': (av1_private(position=1), VERTICAL, kd.av1_chroma_position,
                              'av01.0.05M.08', 'av01.0.05M.08.0.111.01.01.01.0'),
        'chroma position 1, no Colour': (av1_private(level=12, position=1), None, kd.av1_chroma_position,
                                         'av01.0.12M.08', 'av01.0.12M.08.0.111.01.01.01.0'),
        'chroma position 1, 10 bits': (av1_private(level=12, depth=10, position=1), None, kd.av1_chroma_position,
                                       'av01.0.12M.10', 'av01.0.12M.10.0.111.01.01.01.0'),
    }

    def evidence(self, data):
        """The file's path, and its default-suffix and chroma-position evidence."""
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        path = pathlib.Path(directory) / 'f.mkv'
        path.write_bytes(data)
        return path, kd.av1_default_suffix(path), kd.av1_chroma_position(path)

    def test_each_shape_gives_only_its_record_and_strings(self):
        for name, (private, colour, record, oracle, coreav) in self.SHAPES.items():
            _, *found = self.evidence(av1_matroska(private, colour))
            expected = [kd.Av1CodecString(0, 1, oracle, coreav)]
            self.assertEqual(found, [expected, []] if record is kd.av1_default_suffix else [[], expected], name)

    def test_track_position_and_number(self):
        opus = ebml(0xAE, ebml(0xD7, b'\x01') + ebml(0x83, b'\x02') + ebml(0x86, b'A_OPUS'))
        _, _, found = self.evidence(av1_matroska(av1_private(position=1), before=opus))
        self.assertEqual([(e.position, e.number) for e in found], [(1, 2)])

    def test_the_string_comes_from_the_sequence_header(self):
        # Octets stating another level and tier, and a last OBU without its
        # size: the codec string's level and tier are the sequence header's.
        private = bytearray(av1_private(level=23, tier=1, colour=(1, 1, 1)))
        private[1], private[2], private[4] = 13, private[2] & 0x7F, 0x08
        private = bytes(private[:5]) + bytes(private[6:])
        _, found, _ = self.evidence(av1_matroska(private))
        self.assertEqual(found, [kd.Av1CodecString(0, 1, 'av01.0.23H.08.0.110.01.01.01.0', 'av01.0.23H.08')])
        metadata = bytes([5 << 3 | 2, 2, 1, 0])
        _, _, found = self.evidence(av1_matroska(av1_private(position=1, more=metadata)))
        self.assertEqual(len(found), 1)


    def test_other_shapes_give_no_evidence(self):
        private = av1_private(colour=(1, 1, 1))
        for name, data in (
                ('position 1 with stated colour', av1_matroska(av1_private(position=1, colour=(1, 1, 1)))),
                ('position 0 without colour', av1_matroska(av1_private())),
                ('position 2 without colour', av1_matroska(av1_private(position=2))),
                ('other colour', av1_matroska(av1_private(level=13, tier=1, depth=10, colour=(9, 16, 9)))),
                ('full range', av1_matroska(av1_private(colour=(1, 1, 1), full=1))),
                ('monochrome', av1_matroska(av1_private(mono=1, colour=(1, 1, 1)))),
                ('4:4:4', av1_matroska(av1_private(profile=1, colour=(1, 1, 1)))),
                ('reserved level', av1_matroska(av1_private(level=24, colour=(1, 1, 1)))),
                ('full range Colour', av1_matroska(private, BT709 | {0x55B9: 2})),
                ('other primaries Colour', av1_matroska(private, BT709 | {0x55BB: 9})),
                ('other depth Colour', av1_matroska(private, {0x55B2: 10})),
                ('stated primaries without description', av1_matroska(av1_private(position=1), {0x55BB: 1})),
                ('other siting', av1_matroska(av1_private(position=1), VERTICAL | {0x55B8: 1})),
                ('siting for position 0', av1_matroska(private, {0x55B7: 1, 0x55B8: 2})),
                ('octets stating another position', av1_matroska(private[:2] + bytes([private[2] | 1]) + private[3:])),
                ('another codec', av1_matroska(private, codec=b'V_VP9')),
                ('content encoding', av1_matroska(private, encoded=True)),
                ('no sequence header', av1_matroska(private[:4])),
                ('two sequence headers', av1_matroska(private + private[4:])),
                ('a temporal delimiter', av1_matroska(private + b'\x12\x00')),
                ('a short OBU', av1_matroska(private[:-1])),
                ('a short sequence header', av1_matroska(private[:5] + bytes([4]) + private[6:10])),
                ('another marker', av1_matroska(b'\x80' + private[1:])),
        ):
            self.assertEqual(self.evidence(data)[1:], ([], []), name)

    def test_a_child_that_does_not_fit_its_parent_gives_no_evidence(self):
        # A CodecPrivate last in its TrackEntry that declares 100 octets more
        # than it holds, every other size unchanged; a TrackEntry that
        # declares 100 more than its Tracks holds; Tracks that declare 100
        # more than their Segment and the file hold. Each beside its control.
        for name in ('stated BT.709', 'chroma position 1'):
            private, colour, record, oracle, coreav = self.SHAPES[name]
            expected = [kd.Av1CodecString(0, 1, oracle, coreav)]
            expected = [expected, []] if record is kd.av1_default_suffix else [[], expected]
            self.assertEqual(list(self.evidence(av1_matroska(private, colour, declared=len(private)))[1:]),
                             expected, name)
            self.assertEqual(self.evidence(av1_matroska(private, colour, declared=len(private) + 100))[1:],
                             ([], []), name)
            self.assertEqual(self.evidence(av1_matroska(private, colour, entry=100))[1:], ([], []), name)
            # Tracks declaring 100 octets past its Segment and the file.
            self.assertEqual(self.evidence(av1_matroska(private, colour, tracks=100))[1:], ([], []), name)

    def test_a_sequence_header_without_its_mandatory_tail_gives_no_evidence(self):
        # Headers that end before separate_uv_delta_q, or before
        # film_grain_params_present, and their trailing bits.
        for private in ('81050c000a0c0000002ac7e3f00008080808', '81050d000a090000002ac7e3f00001',
                        '81050c000a0c0000002abfe7e00010101010', '81050d000a090000002abfe7e00002'):
            self.assertEqual(self.evidence(av1_matroska(bytes.fromhex(private)))[1:], ([], []), private)
        private = bytearray(av1_private(position=1))
        private[-1] |= 1
        self.assertEqual(self.evidence(av1_matroska(bytes(private)))[1:], ([], []), 'a set bit after the trailing one')

    @unittest.skipUnless(pinned_ffprobe(), 'needs the pinned ffprobe on the path')
    def test_the_pinned_oracle_still_gives_its_string(self):
        for name, (private, colour, record, _, _) in self.SHAPES.items():
            path, *_ = self.evidence(av1_matroska(private, colour))
            (found,) = record(path)
            record = RECORDS[record]
            listed = subprocess.run(['ffprobe', '-v', 'error', '-of', 'json', '-show_entries',
                                     'stream=mime_codec_string', str(path)],
                                    capture_output=True, timeout=60, check=True).stdout
            strings = [s.get('mime_codec_string') for s in json.loads(listed)['streams']]
            self.assertNotEqual(strings, [found.coreav], f'{record} ({name}): the oracle now agrees; retire the record')
            self.assertEqual(strings, [found.oracle], f'{record} ({name}): the oracle string changed')
