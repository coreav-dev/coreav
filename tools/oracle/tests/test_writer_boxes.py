"""Pins the box checks of scripts/writer_boxes.py on small MP4 files
built here: a copy agrees with its source, and each change the writer audit
makes (tools/oracle/writer_kd_audit.py) to the copy is a problem; the rules
of clip and split (media durations, the audio edit, sync flags where the
container claims every sample) reject their near misses."""
import importlib.util
import pathlib
import struct
import sys
import tempfile
import unittest
from fractions import Fraction

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts'))
import track_boxes as tb  # noqa: E402
import writer_boxes as wb  # noqa: E402

SPEC = importlib.util.spec_from_file_location('writer_kd_audit', ROOT / 'tools' / 'oracle' / 'writer_kd_audit.py')
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)


def box(kind, *parts):
    payload = b''.join(parts)
    return struct.pack('>I4s', 8 + len(payload), kind) + payload


def full(kind, version, flags, *parts):
    return box(kind, bytes([version]) + flags.to_bytes(3, 'big'), *parts)


# An AudioSpecificConfig: AAC LC, 44.1 kHz, the given channel configuration.
def asc(channels=2):
    return bytes([(2 << 3) | (4 >> 1), ((4 & 1) << 7) | (channels << 3)])


def esds(config):
    info = b'\x05' + bytes([len(config)]) + config
    decoder = b'\x04' + bytes([13 + len(info)]) + b'\x40\x15' + b'\x00' * 11 + info
    es = b'\x03' + bytes([3 + len(decoder)]) + b'\x00\x01\x00' + decoder
    return full(b'esds', 0, 0, es)


def mp4a(channels=2, rate=44100, config=None):
    fields = b'\x00' * 6 + b'\x00\x01' + b'\x00' * 8 + struct.pack('>HHHHI', channels, 16, 0, 0, rate << 16)
    return box(b'mp4a', fields, esds(config if config is not None else asc(channels)))


# Baseline sequence parameter sets (ITU-T H.264 7.3.2.1.1): id 0,
# log2_max_frame_num 4, pic_order_cnt_type 2, then max_num_ref_frames.
SPS_ONE_REF = bytes([0x67, 66, 0, 30, 0xDA, 0x80])
SPS_16_REFS = bytes([0x67, 66, 0, 30, 0xD8, 0x46])


def avc1(width=64, height=48, length_size=4, sps=SPS_ONE_REF):
    fields = (b'\x00' * 6 + b'\x00\x01' + b'\x00' * 16 + struct.pack('>HH', width, height)
              + struct.pack('>II', 0x480000, 0x480000) + b'\x00' * 4 + b'\x00\x01' + b'\x00' * 32
              + b'\x00\x18\xff\xff')
    config = bytes([1, 66, 0, 30, 0xFC | (length_size - 1), 0xE1]) + struct.pack('>H', len(sps)) + sps + b'\x00'
    return box(b'avc1', fields, box(b'avcC', config))


def nal(header, body=b'\x00' * 8):
    data = bytes(header) + body
    return struct.pack('>I', len(data)) + data


IDR = nal([0x65, 0x88])      # an IDR slice
P = nal([0x41, 0xC0])        # a non-IDR reference slice, slice_type 0 (P)
INTRA = nal([0x41, 0x88])    # a non-IDR reference slice, slice_type 7 (I)
INTRA_NONREF = nal([0x01, 0x88])  # the same with nal_ref_idc 0
# Recovery point SEI messages (D.1.8): recovery_frame_cnt 0 and
# exact_match_flag 1, then recovery_frame_cnt 1.
RECOVERY = nal([0x06, 0x06, 0x01, 0xC4], b'\x80')
RECOVERY_LATER = nal([0x06, 0x06, 0x01, 0x51], b'\x80')


def track(handler, timescale, entry, samples, edits=None, track_id=1, mdhd=None, tkhd=None, stss=True):
    """`samples`: (bytes, duration, composition offset, sync) each."""
    return {'handler': handler, 'timescale': timescale, 'entry': entry, 'samples': samples, 'edits': edits,
            'id': track_id, 'mdhd': mdhd, 'tkhd': tkhd, 'stss': stss}


def build(path, tracks, movie_timescale=1000, mvhd=None):
    """An MP4 file: `ftyp`, `moov`, then `mdat` with each track's samples in
    one chunk."""
    data, offsets = b'', []
    for t in tracks:
        offsets.append(len(data))
        data += b''.join(s[0] for s in t['samples'])
    ftyp = box(b'ftyp', b'isom\x00\x00\x02\x00isom')
    # The movie box's size does not depend on the chunk offsets.
    base = len(ftyp) + len(movie_box(tracks, movie_timescale, mvhd, offsets)) + 8
    moov = movie_box(tracks, movie_timescale, mvhd, [base + at for at in offsets])
    pathlib.Path(path).write_bytes(ftyp + moov + box(b'mdat', data))


def movie_box(tracks, movie_timescale, mvhd, chunks):
    traks, durations = [], []
    for t, chunk in zip(tracks, chunks):
        n = len(t['samples'])
        media = sum(s[1] for s in t['samples'])
        movie_duration = sum(d for d, _ in t['edits']) if t['edits'] else media * movie_timescale // t['timescale']
        durations.append(t['tkhd'] if t['tkhd'] is not None else movie_duration)
        stbl = [full(b'stsd', 0, 0, struct.pack('>I', 1), t['entry']),
                full(b'stts', 0, 0, struct.pack('>I', n), b''.join(struct.pack('>II', 1, s[1]) for s in t['samples'])),
                full(b'ctts', 1, 0, struct.pack('>I', n), b''.join(struct.pack('>Ii', 1, s[2]) for s in t['samples'])),
                full(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, n, 1)),
                full(b'stsz', 0, 0, struct.pack('>II', 0, n), b''.join(struct.pack('>I', len(s[0])) for s in t['samples'])),
                full(b'stco', 0, 0, struct.pack('>II', 1, chunk))]
        if t['stss']:
            sync = [i + 1 for i, s in enumerate(t['samples']) if s[3]]
            stbl.append(full(b'stss', 0, 0, struct.pack('>I', len(sync)), b''.join(struct.pack('>I', i) for i in sync)))
        mdhd = t['mdhd'] if t['mdhd'] is not None else media
        mdia = box(b'mdia', full(b'mdhd', 0, 0, struct.pack('>IIII', 0, 0, t['timescale'], mdhd), b'\x00' * 4),
                   full(b'hdlr', 0, 0, b'\x00' * 4, t['handler'], b'\x00' * 12, b'\x00'),
                   box(b'minf', box(b'stbl', *stbl)))
        parts = [full(b'tkhd', 0, 3, struct.pack('>IIIII', 0, 0, t['id'], 0, durations[-1]), b'\x00' * 60)]
        if t['edits'] is not None:
            parts.append(box(b'edts', full(b'elst', 0, 0, struct.pack('>I', len(t['edits'])),
                                            b''.join(struct.pack('>IiHH', d, m, 1, 0) for d, m in t['edits']))))
        traks.append(box(b'trak', *parts, mdia))
    header = full(b'mvhd', 0, 0, struct.pack('>IIII', 0, 0, movie_timescale,
                                              mvhd if mvhd is not None else max(durations)), b'\x00' * 80)
    return box(b'moov', header, *traks)


def av_tracks(video_sync=(1, 0, 0, 0, 1, 0, 0, 0), audio_count=10, edits=True):
    video = [((IDR if s else P) + bytes([i]), 512, 512 if i else 0, s) for i, s in enumerate(video_sync)]
    audio = [(bytes([0x21, i]) * 20, 1024, 0, 1) for i in range(audio_count)]
    return [track(b'vide', 12800, avc1(), video, [(320, 512)] if edits else None, 1),
            track(b'soun', 44100, mp4a(), audio, [(232, 0)] if edits else None, 2, stss=False)]


class Reader(unittest.TestCase):
    def test_reads_what_it_built(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'a.mp4'
            build(path, av_tracks())
            movie = tb.read_movie(str(path))
            video, audio = movie['tracks']
            self.assertEqual((video['timescale'], video['edits'], video['entry']['type']), (12800, [(320, 512, 1)], 'avc1'))
            self.assertEqual(list(video['samples']['sync']), [1, 0, 0, 0, 1, 0, 0, 0])
            self.assertEqual(list(video['samples']['cto'][:2]), [0, 512])
            self.assertEqual((audio['entry']['channels'], audio['entry']['rate']), (2, 44100))
            first = next(tb.iter_sample_bytes(str(path), video['samples']['offsets'], video['samples']['sizes'], 0, 1))
            self.assertTrue(first.startswith(IDR))

    def test_compressed_movie(self):
        """A QuickTime compressed movie box reads as its inflated movie."""
        import zlib
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'a.mp4'
            build(path, av_tracks())
            data = path.read_bytes()
            at = data.find(b'moov') - 4
            size = struct.unpack('>I', data[at:at + 4])[0]
            moov = data[at:at + size]
            packed = box(b'cmov', box(b'dcom', b'zlib'), box(b'cmvd', struct.pack('>I', len(moov)), zlib.compress(moov)))
            # The same size, padded with a free box, so no offset moves.
            pad = size - 8 - len(packed)
            self.assertGreaterEqual(pad, 8)
            path.write_bytes(data[:at] + box(b'moov', packed, box(b'free', b'\x00' * (pad - 8))) + data[at + size:])
            movie = tb.read_movie(str(path))
            self.assertEqual([t['handler'] for t in movie['tracks']], ['vide', 'soun'])
            self.assertEqual(len(movie['tracks'][0]['samples']['sizes']), 8)


class CopyChanges(unittest.TestCase):
    """A remux of the file is the file itself; the audit's changes to it
    must each be a problem."""

    def test_every_audit_change_is_seen(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = pathlib.Path(tmp) / 'src.mp4'
            build(src, av_tracks())
            wb.forget()
            self.assertEqual(wb.check_copy(str(src), str(src), [0, 1], 'remux'), [])
            unseen = []
            for kind in ('video', 'audio'):
                for name, mutation in audit.MUTATIONS + audit.BY_KIND[kind]:
                    out = pathlib.Path(tmp) / f'{kind}-{len(unseen)}-{name.replace(" ", "_")}.mp4'
                    if not audit.mutate_file(src, out, kind, mutation):
                        continue
                    wb.forget()
                    problems = wb.check_copy(str(src), str(out), [0, 1], 'remux')
                    # Remux's oracle checks its track and movie durations itself.
                    if not problems and name not in ('tkhd duration +1', 'mvhd duration +7', 'elst duration +1',
                                                     'elst duration -1', 'elst media time +1'):
                        unseen.append((kind, name))
            self.assertEqual(unseen, [])

    def test_extraction_headers(self):
        with tempfile.TemporaryDirectory() as tmp:
            src, out = pathlib.Path(tmp) / 'src.mp4', pathlib.Path(tmp) / 'out.m4a'
            tracks = av_tracks()
            build(src, tracks)
            build(out, tracks[1:])
            wb.forget()
            self.assertEqual(wb.check_copy(str(src), str(out), [1], 'extract'), [])
            for name in ('tkhd duration +1', 'mvhd duration +7', 'elst duration +1'):
                bad = pathlib.Path(tmp) / 'bad.m4a'
                self.assertTrue(audit.mutate_file(out, bad, 'audio', dict(audit.MUTATIONS)[name]))
                wb.forget()
                self.assertNotEqual(wb.check_copy(str(src), str(bad), [1], 'extract'), [], name)

    def test_he_aac_extraction_at_the_decoded_rate(self):
        """An extracted track whose configuration signals SBR must be
        written at twice its core rate, every duration doubled; one left at
        the core's rate, or half-scaled, is a problem. A track that signals
        none must keep its rate."""
        # SBR signalled hierarchically and backward-compatibly; LC at 22.05
        # kHz with no SBR signalled (SBR only in its frames, if at all); LC
        # at 44.1 kHz; SBR at 96 kHz, beyond the entry's 16.16 rate.
        for config, core, signalled in ((bytes([0x2b, 0x92, 0x08]), 22050, True),
                                        (bytes([0x13, 0x90, 0x56, 0xe5, 0xa0]), 22050, True),
                                        (bytes([0x13, 0x90]), 22050, False), (asc(), 44100, False),
                                        (bytes([0x29, 0x90, 0x08, 0x00]), 48000, False)):
            with tempfile.TemporaryDirectory() as tmp:
                src, out = pathlib.Path(tmp) / 'src.mp4', pathlib.Path(tmp) / 'out.m4a'
                frames = [(bytes([i]) * 9, 1024, 0, True) for i in range(4)]
                build(src, [track(b'soun', core, mp4a(rate=core, config=config), frames)])
                doubled = [(b, d * 2, c, s) for b, d, c, s in frames]
                # Without an edit list, the media duration in movie ticks, rounded.
                tkhd = round(Fraction(4096 * 1000, core))
                build(out, [track(b'soun', core, mp4a(rate=core, config=config), frames, tkhd=tkhd)])
                wb.forget()
                problems = wb.check_copy(str(src), str(out), [0], 'extract')
                self.assertEqual(problems == [], not signalled, (config, problems))
                if core * 2 > 65535:
                    continue
                build(out, [track(b'soun', core * 2, mp4a(rate=core * 2, config=config), doubled, tkhd=tkhd)])
                wb.forget()
                problems = wb.check_copy(str(src), str(out), [0], 'extract')
                self.assertEqual(problems == [], signalled, (config, problems))
                if not signalled:
                    continue
                # Durations left at the core rate, or the entry's rate left
                # at the core's, are problems.
                build(out, [track(b'soun', core * 2, mp4a(rate=core * 2, config=config), frames, tkhd=tkhd)])
                wb.forget()
                self.assertTrue(wb.check_copy(str(src), str(out), [0], 'extract'))
                build(out, [track(b'soun', core * 2, mp4a(rate=core, config=config), doubled, tkhd=tkhd)])
                wb.forget()
                self.assertTrue(wb.check_copy(str(src), str(out), [0], 'extract'))

    def test_a_sample_outside_the_edit(self):
        """A sample no edit presents is still compared."""
        with tempfile.TemporaryDirectory() as tmp:
            src, out = pathlib.Path(tmp) / 'src.mp4', pathlib.Path(tmp) / 'out.mp4'
            tracks = av_tracks()
            tracks[0]['edits'] = [(240, 2048)]  # starts after the first key frame
            build(src, tracks)
            changed = [dict(tracks[0], samples=[(tracks[0]['samples'][0][0][:-1] + b'\xff',) + tracks[0]['samples'][0][1:]]
                            + tracks[0]['samples'][1:]), tracks[1]]
            build(out, changed)
            wb.forget()
            self.assertIn('box:video:samples', {f for f, _ in wb.check_copy(str(src), str(out), [0, 1], 'remux')})


class Entries(unittest.TestCase):
    def test_channels(self):
        src = {'entry': {'type': 'mp4a', 'channels': 2, 'rate': 44100.0, 'payload': mp4a(2)[8:], 'sound_version': 0}}
        same = {'entry': dict(src['entry'])}
        self.assertEqual(wb.compare_entry('audio', src, same), [])
        self.assertTrue(wb.compare_entry('audio', src, {'entry': dict(src['entry'], channels=3)}))
        self.assertTrue(wb.compare_entry('audio', src, {'entry': dict(src['entry'], rate=22050.0)}))
        # A 5.1 configuration under a stereo entry: either count.
        six = {'entry': {'type': 'mp4a', 'channels': 2, 'rate': 44100.0, 'payload': mp4a(2, config=asc(6))[8:],
                         'sound_version': 0}}
        self.assertEqual(wb.compare_entry('audio', six, {'entry': dict(six['entry'], channels=6)}), [])
        self.assertTrue(wb.compare_entry('audio', six, {'entry': dict(six['entry'], channels=5)}))
        # A mono configuration: 2 only under a stereo entry (parametric
        # stereo the bitstream may signal) or with object type 29.
        mono = {'entry': {'type': 'mp4a', 'channels': 1, 'rate': 44100.0, 'payload': mp4a(1)[8:], 'sound_version': 0}}
        self.assertTrue(wb.compare_entry('audio', mono, {'entry': dict(mono['entry'], channels=2)}))
        stereo = {'entry': dict(mono['entry'], channels=2, payload=mp4a(2, config=asc(1))[8:])}
        for count in (1, 2):
            self.assertEqual(wb.compare_entry('audio', stereo, {'entry': dict(stereo['entry'], channels=count)}), [])
        ps = bytes([(29 << 3) | (4 >> 1), ((4 & 1) << 7) | (1 << 3)])
        explicit = {'entry': dict(mono['entry'], payload=mp4a(1, config=ps)[8:])}
        self.assertEqual(wb.compare_entry('audio', explicit, {'entry': dict(explicit['entry'], channels=2)}), [])

    def test_configurations(self):
        entry = {'type': 'mp4a', 'channels': 2, 'rate': 44100.0, 'sound_version': 0}
        unread = dict(entry, payload=box(b'mp4a', mp4a(2)[8:36], full(b'esds', 0, 0, b'\x03'))[8:])
        self.assertIsNone(wb.aac_config(unread))
        self.assertTrue(wb.compare_entry('audio', {'entry': unread}, {'entry': dict(unread)}), 'neither read')
        # MPEG-1 audio in `mp4a`: a decoder configuration without
        # DecoderSpecificInfo, the same in both.
        decoder = b'\x04' + bytes([13]) + b'\x6b\x15' + b'\x00' * 11
        mp3 = full(b'esds', 0, 0, b'\x03' + bytes([3 + len(decoder)]) + b'\x00\x01\x00' + decoder)
        plain = dict(entry, payload=mp4a(2)[8:36] + mp3)
        self.assertEqual(wb.aac_config(plain), b'')
        self.assertEqual(wb.compare_entry('audio', {'entry': plain}, {'entry': dict(plain)}), [])
        # ISO/IEC 14496-12's AudioSampleEntryV1 keeps the boxes at 28;
        # QuickTime's version 1 has 16 more bytes before them.
        v1 = bytearray(mp4a(2)[8:])
        v1[8:10] = b'\x00\x01'
        self.assertEqual(wb.aac_config(dict(entry, payload=bytes(v1), sound_version=1, stsd_version=1)), asc(2))
        qt = bytes(v1[:28]) + b'\x00' * 16 + bytes(v1[28:])
        self.assertEqual(wb.aac_config(dict(entry, payload=qt, sound_version=1, stsd_version=0)), asc(2))

    def test_ac3_and_eac3_channels(self):
        # dac3 of acmod 7 with the LFE (5.1), and dec3 of the same, under
        # stereo entries: either count, and nothing else.
        dac3 = int('00' + '01000' + '000' + '111' + '1' + '01010' + '00000', 2).to_bytes(3, 'big')
        dec3 = int('0000110000000' + '000' + '00' + '10000' + '0' + '0' + '000' + '111' + '1' + '000' + '0000'
                   + '0', 2).to_bytes(5, 'big')
        for kind, config, name in ((b'ac-3', dac3, b'dac3'), (b'ec-3', dec3, b'dec3')):
            payload = mp4a(2)[8:36] + box(name, config)
            src = {'entry': {'type': kind.decode(), 'channels': 2, 'rate': 48000.0, 'payload': payload,
                             'sound_version': 0}}
            self.assertEqual(wb.ac3_channels(src['entry']), 6)
            for count in (2, 6):
                self.assertEqual(wb.compare_entry('audio', src, {'entry': dict(src['entry'], channels=count)}), [])
            self.assertTrue(wb.compare_entry('audio', src, {'entry': dict(src['entry'], channels=5)}))
            short = dict(src['entry'], payload=mp4a(2)[8:36] + box(name, config[:1]))
            self.assertIsNone(wb.ac3_channels(short))

    def test_program_config_element(self):
        # channelConfiguration 0, then a GASpecificConfig and a program
        # config element of 2 channel pairs and 1 LFE: 5 channels.
        bits = '00010' + '0100' + '0000' + '000' + '0000' + '00' + '0100' + '0010' + '0000' + '0000' + '01' \
            + '000' + '0000' + '000' + '1' + '0000' + '1' + '0001' + '0000'
        bits += '0' * (-len(bits) % 8)
        config = int(bits, 2).to_bytes(len(bits) // 8, 'big')
        self.assertEqual(wb.config_channels(config), 5)
        self.assertEqual(wb.config_channels(asc(1)), 1)


class Sync(unittest.TestCase):
    def runs(self, flags, out_flags, samples=None, sps=SPS_ONE_REF, stss=None):
        with tempfile.TemporaryDirectory() as tmp:
            src, out = pathlib.Path(tmp) / 'src.mp4', pathlib.Path(tmp) / 'out.mp4'
            data = samples or [IDR, P, INTRA, P, IDR, P]
            video = [(d + bytes([i]), 512, 0, s) for i, (d, s) in enumerate(zip(data, flags))]
            build(src, [track(b'vide', 12800, avc1(sps=sps), video, None,
                              stss=not all(flags) if stss is None else stss)])
            build(out, [track(b'vide', 12800, avc1(sps=sps), [v[:3] + (s,) for v, s in zip(video, out_flags)], None)])
            wb.forget()
            movie, copy = tb.read_movie(str(src)), tb.read_movie(str(out))
            return wb.compare_samples('video', str(src), movie['tracks'][0],
                                      [(str(out), copy['tracks'][0]['samples'])], 0, None, 'run')

    def test_flags_as_stored(self):
        self.assertEqual(self.runs((1, 0, 0, 0, 1, 0), (1, 0, 0, 0, 1, 0)), [])
        self.assertTrue(self.runs((1, 0, 0, 0, 1, 0), (1, 1, 0, 0, 1, 0)), 'a flag the table does not state')
        self.assertTrue(self.runs((1, 0, 0, 0, 1, 0), (1, 0, 0, 0, 0, 1)), 'a flag moved')

    def test_a_claim_of_every_sample(self):
        """Without a sync sample table the output marks exactly the frames
        that start decoding (cut-points.md)."""
        claim = (1, 1, 1, 1, 1, 1)
        # One reference frame: an intra reference picture starts decoding.
        self.assertEqual(self.runs(claim, (1, 0, 1, 0, 1, 0)), [])
        self.assertTrue(self.runs(claim, (1, 0, 0, 0, 1, 0)), 'an intra reference picture unmarked')
        self.assertTrue(self.runs(claim, (1, 0, 1, 0, 0, 1)), 'an IDR picture unmarked, a P frame marked')
        self.assertTrue(self.runs(claim, (1, 1, 1, 0, 1, 0)), 'a P frame marked')
        # Sixteen reference frames: it does not (the flash fixture of the cut-point checks).
        self.assertEqual(self.runs(claim, (1, 0, 0, 0, 1, 0), sps=SPS_16_REFS), [])
        self.assertTrue(self.runs(claim, (1, 0, 1, 0, 1, 0), sps=SPS_16_REFS), 'an intra frame of 16 references')
        # Not a reference picture: it does not either.
        frames = [IDR, P, INTRA_NONREF, P, IDR, P]
        self.assertTrue(self.runs(claim, (1, 0, 1, 0, 1, 0), frames), 'a non-reference intra frame')

    def test_recovery_points(self):
        claim = (1, 1, 1, 1)
        frames = [IDR, RECOVERY + P, RECOVERY_LATER + P, P]
        self.assertEqual(self.runs(claim, (1, 1, 0, 0), frames, sps=SPS_16_REFS), [])
        self.assertTrue(self.runs(claim, (1, 1, 1, 0), frames, sps=SPS_16_REFS), 'recovery_frame_cnt 1')
        self.assertTrue(self.runs(claim, (1, 0, 0, 0), frames, sps=SPS_16_REFS), 'recovery point unmarked')

    def test_unreadable_frames(self):
        """A frame whose NAL units cannot be read keeps a stated flag; an
        implied one is no cut point."""
        broken = struct.pack('>I', 99) + b'\x41\xc0'
        frames = [IDR, broken, P]
        self.assertEqual(self.runs((1, 1, 1), (1, 1, 0), frames, stss=True), [])
        self.assertTrue(self.runs((1, 1, 1), (1, 0, 0), frames, stss=True), 'a stated flag dropped')
        self.assertEqual(self.runs((1, 1, 1), (1, 0, 0), frames, stss=False), [])
        self.assertTrue(self.runs((1, 1, 1), (1, 1, 0), frames, stss=False), 'an implied flag kept')


    def test_other_codecs_keep_the_container_flags(self):
        """VP9, or a configuration record that cannot be read: flags as
        stored, even where the container claims every sample."""
        with tempfile.TemporaryDirectory() as tmp:
            src, out = pathlib.Path(tmp) / 'src.mp4', pathlib.Path(tmp) / 'out.mp4'
            entry = avc1().replace(b'avc1', b'vp09', 1).replace(b'avcC', b'vpcC', 1)
            video = [(bytes([0x82, i]) * 8, 512, 0, 1) for i in range(4)]
            build(src, [track(b'vide', 12800, entry, video, None, stss=False)])
            build(out, [track(b'vide', 12800, entry, [v[:3] + (s,) for v, s in zip(video, (1, 0, 0, 0))], None)])
            wb.forget()
            movie, copy = tb.read_movie(str(src)), tb.read_movie(str(out))
            self.assertTrue(wb.compare_samples('video', str(src), movie['tracks'][0],
                                               [(str(out), copy['tracks'][0]['samples'])], 0, None, 'run'))


class Fragments(unittest.TestCase):
    """Movie fragments (ISO/IEC 14496-12 8.8): `tfhd` and `trex` defaults,
    `default-base-is-moof` with a run's data offset, the first sample's
    flags, and `tfdt` continuity."""

    def fragmented(self, path, second_tfdt=3 * 1024):
        trex = full(b'trex', 0, 0, struct.pack('>IIIII', 1, 1, 1024, 0, 0x00010000))
        moov = box(b'moov', full(b'mvhd', 0, 0, struct.pack('>IIII', 0, 0, 1000, 0), b'\x00' * 80),
                   box(b'trak', full(b'tkhd', 0, 3, struct.pack('>IIIII', 0, 0, 1, 0, 0), b'\x00' * 60),
                       box(b'mdia', full(b'mdhd', 0, 0, struct.pack('>IIII', 0, 0, 44100, 0), b'\x00' * 4),
                           full(b'hdlr', 0, 0, b'\x00' * 4, b'vide', b'\x00' * 13),
                           box(b'minf', box(b'stbl', full(b'stsd', 0, 0, struct.pack('>I', 1), avc1()),
                                            full(b'stts', 0, 0, b'\x00' * 4), full(b'stsc', 0, 0, b'\x00' * 4),
                                            full(b'stsz', 0, 0, b'\x00' * 8), full(b'stco', 0, 0, b'\x00' * 4))))),
                   box(b'mvex', trex))
        out = box(b'ftyp', b'isom\x00\x00\x02\x00isom') + moov
        for n, (tfdt, sizes) in enumerate(((0, (5, 6, 7)), (second_tfdt, (8, 9)))):
            def moof(offset):
                tfhd = full(b'tfhd', 0, 0x020000 | 0x10, struct.pack('>II', 1, 4))  # default size 4
                run = full(b'trun', 0, 0x01 | 0x04 | 0x200, struct.pack('>IiI', len(sizes), offset, 0x02000000),
                           b''.join(struct.pack('>I', z) for z in sizes))
                return box(b'moof', full(b'mfhd', 0, 0, struct.pack('>I', n + 1)),
                           box(b'traf', tfhd, full(b'tfdt', 1, 0, struct.pack('>Q', tfdt)), run))
            size = len(moof(0))
            out += moof(size + 8) + box(b'mdat', b''.join(bytes([n * 16 + i]) * z for i, z in enumerate(sizes)))
        pathlib.Path(path).write_bytes(out)

    def test_samples(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'f.mp4'
            self.fragmented(path)
            samples = tb.read_movie(str(path))['tracks'][0]['samples']
            self.assertEqual(list(samples['sizes']), [5, 6, 7, 8, 9])
            self.assertEqual(list(samples['durations']), [1024] * 5, 'the trex default')
            self.assertEqual(list(samples['sync']), [1, 0, 0, 1, 0], 'first-sample flags, then the trex default')
            data = path.read_bytes()
            got = list(tb.iter_sample_bytes(str(path), samples['offsets'], samples['sizes'], 0, 5))
            self.assertEqual(got, [bytes([i]) * z for i, z in enumerate((5, 6, 7))]
                             + [bytes([16 + i]) * z for i, z in enumerate((8, 9))])
            self.assertTrue(data)

    def test_a_gap(self):
        """A gap between fragments keeps the samples comparable, and refuses
        the checks that derive times from the durations."""
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'f.mp4'
            self.fragmented(path)
            movie = tb.read_movie(str(path))
            self.assertFalse(movie['tracks'][0]['samples']['gap'])
            self.fragmented(path, second_tfdt=4 * 1024)
            movie = tb.read_movie(str(path))
            track = movie['tracks'][0]
            self.assertTrue(track['samples']['gap'])
            self.assertEqual(len(track['samples']['sizes']), 5)
            with self.assertRaises(tb.Unreadable):
                wb.source_window(movie, track)


class SoundEntries(unittest.TestCase):
    def test_quicktime_versions(self):
        v0 = mp4a(2, 48000)
        stsd = full(b'stsd', 0, 0, struct.pack('>I', 1), v0)
        entry = tb.sample_entry(memoryview(stsd), (8, len(stsd)))
        self.assertEqual((entry['channels'], entry['rate'], entry['sound_version']), (2, 48000.0, 0))
        self.assertEqual(wb.aac_config(entry), asc(2))
        # QuickTime version 2: the values in its extension, the esds after it.
        fields = (b'\x00' * 6 + b'\x00\x01' + struct.pack('>HHI', 2, 0, 0) + struct.pack('>HHhHI', 3, 16, -2, 0, 65536)
                  + struct.pack('>IdIIIIII', 72, 96000.0, 6, 0x7F000000, 16, 0, 0, 1024))
        v2 = box(b'mp4a', fields, esds(asc(6)))
        stsd = full(b'stsd', 0, 0, struct.pack('>I', 1), v2)
        entry = tb.sample_entry(memoryview(stsd), (8, len(stsd)))
        self.assertEqual((entry['channels'], entry['rate'], entry['sound_version']), (6, 96000.0, 2))
        self.assertEqual(wb.aac_config(entry), asc(6))


class ClipRules(unittest.TestCase):
    def movie(self, edits, durations, mdhd, timescale=44100, movie_timescale=88200, fragmented=False):
        samples = {'sizes': [1] * len(durations), 'durations': list(durations), 'base_dts': 0}
        trak = {'timescale': timescale, 'edits': edits, 'mdhd_duration': mdhd, 'samples': samples,
                'tkhd_duration': sum(d for d, _, _ in edits or [])}
        return {'movie_timescale': movie_timescale, 'fragmented': fragmented, 'tracks': [trak]}, trak

    def test_media_duration_carries_the_source_difference(self):
        src_movie, src = self.movie([(88200 * 2, 0, 1)], [1024] * 80, 1024 * 80 - 500)
        _, out = self.movie([(88200, 1024, 1)], [1024] * 40, 1024 * 40 - 500)
        self.assertEqual(wb.kept_duration(src_movie, src, out, 40), 1024 * 40 - 500)
        self.assertEqual(wb.kept_duration(src_movie, src, out, 0), 1024 * 40, 'the last sample is not kept')

    def test_edit_rate(self):
        src_movie, src = self.movie([(88200 * 2, 0, 1)], [1024] * 80, 1024 * 80)
        out_movie, out = self.movie([(88200, 0, 1)], [1024] * 40, 1024 * 40)
        self.assertEqual(wb.clip_headers('audio', src_movie, src, out_movie, out, 0), [])
        out['edits'] = [(88200, 0, 2)]
        self.assertTrue(wb.clip_headers('audio', src_movie, src, out_movie, out, 0))

    def test_audio_edit(self):
        src_movie, src = self.movie([(88200 * 2, 2112, 1)], [1024] * 100, 102400)
        out_movie, out = self.movie([(88200, 1088, 1)], [1024] * 45, 46080)
        zero, end = Fraction(0), Fraction(1)
        self.assertEqual(wb.audio_edit(src_movie, src, out_movie, out, 1, zero, end), [])
        for edits in ([(88201, 1088, 1)], [(88200, 1089, 1)], [(10, -1, 1), (88190, 1088, 1)]):
            out['edits'] = edits
            self.assertTrue(wb.audio_edit(src_movie, src, out_movie, out, 1, zero, end), edits)

    def test_audio_edits_of_several_windows(self):
        # Source: 1 s of media from 0, a 0.5 s empty edit, then 1 s of media
        # from 2 s (44.1 kHz media, 600 Hz movie). A clip from 0.5 s to
        # 2.25 s keeping samples from 0.48 s (first kept sample 20).
        src_movie, src = self.movie([(600, 0, 1), (300, -1, 1), (600, 88200, 1)], [1058] * 150, 158700,
                                    movie_timescale=600)
        out_movie, out = self.movie(None, [1058] * 100, 105800)
        zero, end = Fraction(1, 2), Fraction(9, 4)
        first_dts = 1058 * 20
        expected = [(44100, 22050 - first_dts), (44100, -1), (66150, 88200 - first_dts)]
        out['edits'] = [(d, t, 1) for d, t in expected]
        self.assertEqual(wb.audio_edits_several(src_movie, src, out_movie, out, 20, zero, end), [])
        for d, t in ((1, 0), (0, 1)):
            out['edits'] = [(expected[0][0] + d, expected[0][1] + t, 1)] + [(x, y, 1) for x, y in expected[1:]]
            self.assertTrue(wb.audio_edits_several(src_movie, src, out_movie, out, 20, zero, end))
        out['edits'] = [(d, t, 1) for d, t in expected[:1]] + [(d, t, 1) for d, t in expected[2:]]
        self.assertTrue(wb.audio_edits_several(src_movie, src, out_movie, out, 20, zero, end), 'the empty edit')



    def test_split_audio_of_several_windows(self):
        # Source: 1 s of media from 0, a 0.5 s empty edit, then 1 s of media
        # from 1.95 s (1 kHz media and movie, samples of 0.1 s). Sample 19
        # starts in the gap and is shown from the window's start, 1.5 s;
        # samples 10 to 18 are shown by no window.
        src_movie, src = self.movie([(1000, 0, 1), (500, -1, 1), (1000, 1950, 1)], [100] * 30, 3000,
                                    timescale=1000, movie_timescale=1000)
        self.assertEqual(wb.split_audio_range(src_movie, src, 19, 11), (Fraction(3, 2), Fraction(5, 2)))
        self.assertEqual(wb.split_audio_range(src_movie, src, 10, 20), (Fraction(3, 2), Fraction(5, 2)))
        self.assertEqual(wb.split_audio_range(src_movie, src, 0, 19), (Fraction(0), Fraction(1)))
        self.assertIsNone(wb.split_audio_range(src_movie, src, 10, 9))
        # A segment from 1 s (the gap is the later segment's, split decision
        # 3) holding samples 10 to 29: an empty edit to 1.5 s, then the
        # window from sample 19's showing.
        out_movie, out = self.movie(None, [100] * 20, 2000, timescale=1000, movie_timescale=1000)
        begin, end = wb.split_audio_range(src_movie, src, 10, 20)
        expected = [(500, -1), (1000, 950)]
        out['edits'] = [(d, t, 1) for d, t in expected]
        self.assertEqual(wb.audio_edits_several(src_movie, src, out_movie, out, 10, Fraction(1), end, begin), [])
        for edits in ([(501, -1), (1000, 950)], [(500, -1), (1000, 951)], [(500, -1), (999, 950)],
                      [(1000, 950)]):
            out['edits'] = [(d, t, 1) for d, t in edits]
            self.assertTrue(wb.audio_edits_several(src_movie, src, out_movie, out, 10, Fraction(1), end, begin),
                            edits)
        # A sample across a window's end is shown to that end only.
        src_movie, src = self.movie([(1050, 0, 1), (500, -1, 1), (1000, 1950, 1)], [100] * 30, 3000,
                                    timescale=1000, movie_timescale=1000)
        self.assertEqual(wb.split_audio_range(src_movie, src, 0, 11), (Fraction(0), Fraction(21, 20)))


class HlsStarts(unittest.TestCase):
    """Where an HLS rendition of a trimmed source starts (hls.md,
    "Timing")."""

    def track(self, sync, cto=None, base=0, entry=None):
        n = len(sync)
        samples = {'sizes': [1] * n, 'durations': [100] * n, 'cto': list(cto or [0] * n),
                   'sync': bytearray(sync), 'stated': bytearray([1] * n), 'base_dts': base, 'offsets': [0] * n}
        return {'samples': samples, 'timescale': 1000, 'entry': entry or {'type': 'vp09', 'payload': b''}}

    def test_video(self):
        track = self.track([1, 0, 0, 1, 0, 0, 1, 0])
        start = lambda m0: wb.hls_video_start(None, track, (Fraction(m0), Fraction(800), Fraction(0)))  # noqa: E731
        self.assertEqual(start(350), 3, 'the latest sync sample before the edit\'s start')
        self.assertEqual(start(300), 3, 'one presented exactly at the start')
        self.assertEqual(start(299), 0)
        self.assertEqual(start(-1), 0, 'none before: the first sample')

    def test_video_reordered(self):
        # A sync sample presented before an earlier sample in decode order
        # cannot start the rendition.
        track = self.track([1, 0, 1, 0, 1, 0], cto=[0, 150, 0, 0, 0, 0])
        self.assertEqual(wb.hls_video_start(None, track, (Fraction(350), Fraction(600), Fraction(0))), 0)
        self.assertEqual(wb.hls_video_start(None, track, (Fraction(450), Fraction(600), Fraction(0))), 4)

    def test_audio(self):
        # The audio edit starts 2 s into the media (samples of 0.1 s): the
        # rendition still starts at the first sample.
        audio = self.track([1] * 30)
        window = (Fraction(2000), Fraction(3000), Fraction(0))
        self.assertEqual(wb.hls_start(None, audio, window, 'audio'), 0)
        video = self.track([1, 0, 0, 1] * 6 + [0] * 6)
        self.assertEqual(wb.hls_start(None, video, window, 'video'), 20)

    def test_signed_offsets(self):
        self.assertEqual(wb.signed((1 << 32) - 512), -512)
        self.assertEqual(wb.signed(512), 512)


if __name__ == '__main__':
    unittest.main()
