"""Keeps docs/known-divergences-mkv.md, its JSON twin and
scripts/oracle_kd_mkv.py in sync, and checks that each predicate rejects
near misses."""
import importlib.util
import json
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

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


def ac3_matroska(blocks, codec=b'A_AC3', encoded=False, before=b''):
    """A Matroska file whose track entries are `before` and then one AC-3
    track, number 2 after `before` and 1 otherwise, with one cluster of
    `blocks` (each the bytes after a SimpleBlock's flags, and its lacing
    bits)."""
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
                   + ebml(0x1F43B675, ebml(0xE7, b'\x00') + cluster)))


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
