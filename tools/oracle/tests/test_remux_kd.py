"""Keeps docs/known-divergences-remux.md, its JSON twin and the predicates of
scripts/remux_check.py in sync, and checks that each predicate rejects near
misses (docs/design/remux.md)."""
import importlib.util
import json
import pathlib
import re
import struct
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('remux_check', ROOT / 'scripts' / 'remux_check.py')
rc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rc)


def bx(kind, payload):
    return struct.pack('>I', len(payload) + 8) + kind + payload


def sound_file(children):
    """A movie with one audio track whose version 0 sound entry has these
    children."""
    entry = bx(b'mp4a', bytes(6) + b'\0\x01' + bytes(20) + b''.join(children))
    stsd = bx(b'stsd', bytes(4) + struct.pack('>I', 1) + entry)
    stts = bx(b'stts', bytes(4) + struct.pack('>III', 2, 3, 1024) + struct.pack('>II', 1, 136))
    hdlr = bx(b'hdlr', bytes(8) + b'soun' + bytes(13))
    mdia = bx(b'mdia', hdlr + bx(b'minf', bx(b'stbl', stsd + stts)))
    return bx(b'ftyp', b'isom\0\0\0\0') + bx(b'moov', bx(b'trak', mdia))


class DocumentsInSync(unittest.TestCase):
    def test_ids_and_predicates_match(self):
        twin = json.loads((ROOT / 'docs' / 'known-divergences-remux.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-remux.md').read_text()
        documented = set(re.findall(r'^## (KD-R-[A-Z-]+)$', page, re.M))
        self.assertEqual(documented, {entry['id'] for entry in twin['entries']})
        self.assertEqual({entry['predicate'] for entry in twin['entries']}, set(rc.PREDICATES))
        for entry in twin['entries']:
            self.assertTrue(entry['basis'])
            self.assertEqual(entry['filename_pattern'], '*', 'no fixture-name predicates')


class BoxChecks(unittest.TestCase):
    def test_never_covered(self):
        # box:N:duration matches two entries' field patterns; neither may
        # cover it, whatever their predicates say.
        found = [{'field': 'box:0:duration', 'message': 'stream 0 track duration: 1 != 2'}]
        saved = rc.PREDICATES
        rc.PREDICATES = {name: (lambda item, context: True) for name in saved}
        try:
            problems, covered = rc.apply_known_divergences(found, {})
        finally:
            rc.PREDICATES = saved
        self.assertEqual(problems, found)
        self.assertEqual(covered, [])


class Predicates(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def write(self, name, data):
        path = pathlib.Path(self.dir.name) / name
        path.write_bytes(data)
        return str(path)

    def context(self, fragmented=True, source=None, output=None, dropped=(), streams=()):
        return {
            'entry': {'source': source or '/nonexistent', 'output': output or '/nonexistent',
                      'dropped_streams': list(dropped)},
            'source_top_level': ['ftyp', 'moov', 'moof', 'mdat'] if fragmented else ['ftyp', 'moov', 'mdat'],
            'source_streams': list(streams),
            'kept': [0],
        }

    def test_fragment_frame_count(self):
        found = rc.problem('stream:0:nb_frames', '', source=None, output='50', output_packets=50)
        self.assertTrue(rc.fragment_frame_count(found, self.context()))
        self.assertFalse(rc.fragment_frame_count(found, self.context(fragmented=False)))
        moov_only = rc.problem('stream:0:nb_frames', '', source='25', output='50', output_packets=50)
        self.assertTrue(rc.fragment_frame_count(moov_only, self.context()))
        # The output's count is not what ffmpeg copies, or the source has more.
        off = rc.problem('stream:0:nb_frames', '', source=None, output='49', output_packets=50)
        self.assertFalse(rc.fragment_frame_count(off, self.context()))
        more = rc.problem('stream:0:nb_frames', '', source='60', output='50', output_packets=50)
        self.assertFalse(rc.fragment_frame_count(more, self.context()))

    def test_fragment_duration(self):
        line = '0, {dts}, {pts}, {duration}, 256, 0ef0'
        found = rc.problem('stream:0:packet:0', '', source=line.format(dts=0, pts=0, duration=1024),
                           output=line.format(dts=0, pts=0, duration=3528),
                           next_output=line.format(dts=3528, pts=3528, duration=1024))
        self.assertTrue(rc.fragment_duration(found, self.context()))
        self.assertFalse(rc.fragment_duration(found, self.context(fragmented=False)))
        # The duration does not reach the next packet.
        short = dict(found, data=dict(found['data'], next_output=line.format(dts=4000, pts=4000, duration=1)))
        self.assertFalse(rc.fragment_duration(short, self.context()))
        # Another field differs too.
        moved = dict(found, data=dict(found['data'], output='0, 0, 1, 3528, 256, 0ef0'))
        self.assertFalse(rc.fragment_duration(moved, self.context()))
        other_bytes = dict(found, data=dict(found['data'], output='0, 0, 0, 3528, 256, ffff'))
        self.assertFalse(rc.fragment_duration(other_bytes, self.context()))
        # The last packet: the duration the source's last fragment stores.
        tkhd = bx(b'tkhd', bytes(4) + struct.pack('>IIII', 0, 0, 1, 0) + bytes(68))
        trex = bx(b'trex', bytes(4) + struct.pack('>IIIII', 1, 1, 1024, 0, 0))
        moov = bx(b'moov', bx(b'trak', tkhd) + bx(b'mvex', trex))
        # tfhd: track 1, no defaults; trun: durations and sizes of 2 samples.
        tfhd = bx(b'tfhd', bytes(4) + struct.pack('>I', 1))
        trun = bx(b'trun', b'\0\0\x03\0' + struct.pack('>IIIII', 2, 1024, 10, 136, 10))
        source = self.write('frag_src.mp4', bx(b'ftyp', b'iso6\0\0\0\0') + moov
                            + bx(b'moof', bx(b'traf', tfhd + trun)) + bx(b'mdat', bytes(20)))
        last = rc.problem('stream:0:packet:1', '', source=line.format(dts=1024, pts=1024, duration=1024),
                          output=line.format(dts=1024, pts=1024, duration=136), next_output=None)
        self.assertTrue(rc.fragment_duration(last, self.context(source=source)))
        wrong = dict(last, data=dict(last['data'], output=line.format(dts=1024, pts=1024, duration=137)))
        self.assertFalse(rc.fragment_duration(wrong, self.context(source=source)))
        # Without a duration in the run, the track's default from trex.
        trun = bx(b'trun', b'\0\0\x02\0' + struct.pack('>III', 2, 10, 10))
        default = self.write('frag_default.mp4', bx(b'ftyp', b'iso6\0\0\0\0') + moov
                             + bx(b'moof', bx(b'traf', tfhd + trun)) + bx(b'mdat', bytes(20)))
        self.assertFalse(rc.fragment_duration(last, self.context(source=default)))
        stored = dict(last, data=dict(last['data'], output=line.format(dts=1024, pts=1024, duration=1024),
                                      source=line.format(dts=1024, pts=1024, duration=1000)))
        self.assertTrue(rc.fragment_duration(stored, self.context(source=default)))

    def test_dropped_timecode(self):
        streams = [{'index': 0, 'codec_tag_string': 'avc1'}, {'index': 1, 'codec_tag_string': 'tmcd'}]
        found = rc.problem('stream:0:tag:timecode', '', source='01:00:00:00', output=None)
        self.assertTrue(rc.dropped_timecode(found, self.context(dropped=[1], streams=streams)))
        self.assertFalse(rc.dropped_timecode(found, self.context(dropped=[], streams=streams)))
        changed = rc.problem('stream:0:tag:timecode', '', source='01:00:00:00', output='02:00:00:00')
        self.assertFalse(rc.dropped_timecode(changed, self.context(dropped=[1], streams=streams)))

    def test_dropped_timecode_of_the_file(self):
        streams = [{'index': 0, 'codec_tag_string': 'avc1'}, {'index': 1, 'codec_tag_string': 'tmcd'}]
        found = rc.problem('format:tag:timecode', '', source='17:19:54:30', output=None)
        self.assertTrue(rc.dropped_timecode(found, self.context(dropped=[1], streams=streams)))

    def test_fragment_last_trim(self):
        line = '0, 91592, 91592, 136, 5, 47a1'
        found = rc.problem('stream:0:packet:87', '', source=line, output=line + ', S=1, 10, 3a0a', next_output=None)
        context = dict(self.context(source='src.mp4', output='out.mp4'), ffprobe='ffprobe')

        def covered(item=found, c=context, out=(0, 888), src=None, durations=b'1024,\n1024,\n136,\n'):
            sides = {'out.mp4': out, 'src.mp4': src}
            with mock.patch.object(rc, 'run', return_value=durations), \
                    mock.patch.object(rc, 'last_skip_samples', side_effect=lambda _, path, __: sides[path]):
                return rc.fragment_last_trim(item, c)

        self.assertTrue(covered())
        self.assertFalse(covered(c=dict(context, source_top_level=['ftyp', 'moov', 'mdat'])))
        self.assertFalse(covered(item=dict(found, data=dict(found['data'], next_output=line))))
        # The padding is not the rest of the frame, or the start is skipped too.
        self.assertFalse(covered(out=(0, 887)))
        self.assertFalse(covered(out=(1, 888)))
        self.assertFalse(covered(durations=b'1024,\n1000,\n136,\n'))
        # The source reads the same side data, or another field differs.
        self.assertFalse(covered(src=(0, 888)))
        moved = dict(found, data=dict(found['data'], output='0, 91592, 91592, 136, 6, 47a1, S=1, 10, 3a0a'))
        self.assertFalse(covered(item=moved))
        two = dict(found, data=dict(found['data'], output=line + ', S=1, 10, 3a0a, S=2, 4, 0000'))
        self.assertFalse(covered(item=two))

    def test_index_start(self):
        tkhd = bx(b'tkhd', bytes(4) + struct.pack('>IIII', 0, 0, 1, 0) + bytes(68))
        mdhd = bx(b'mdhd', bytes(4) + struct.pack('>IIII', 0, 0, 12800, 0) + bytes(4))

        def sidx(ept, timescale=12800, track=1):
            refs = struct.pack('>III', 100, 128000, 0x90000000) * 2
            return bx(b'sidx', bytes(4) + struct.pack('>IIIIHH', track, timescale, ept, 0, 0, 2) + refs)

        def source(name, *indexes, edts=b''):
            moov = bx(b'moov', bx(b'trak', tkhd + edts + bx(b'mdia', mdhd)))
            return self.write(name, bx(b'ftyp', b'iso6\0\0\0\0') + moov + b''.join(indexes) + bx(b'moof', b''))

        found = rc.problem('stream:0:duration_ts', '', source=257024, output=256000)
        self.assertTrue(rc.index_start(found, self.context(source=source('a.mp4', sidx(1024)))))
        self.assertFalse(rc.index_start(found, self.context(fragmented=False, source=source('b.mp4', sidx(1024)))))
        off = rc.problem('stream:0:duration_ts', '', source=257024, output=256001)
        self.assertFalse(rc.index_start(off, self.context(source=source('c.mp4', sidx(1024)))))
        self.assertFalse(rc.index_start(found, self.context(source=source('d.mp4', sidx(0)))))
        self.assertFalse(rc.index_start(found, self.context(source=source('e.mp4', sidx(1024, timescale=1000)))))
        self.assertFalse(rc.index_start(found, self.context(source=source('f.mp4', sidx(1024), sidx(1024)))))
        self.assertFalse(rc.index_start(found, self.context(source=source('g.mp4', sidx(1024, track=2)))))
        edts = bx(b'edts', bx(b'elst', bytes(4) + struct.pack('>IIIi', 1, 0, 1024, 0x10000)))
        self.assertFalse(rc.index_start(found, self.context(source=source('h.mp4', sidx(1024), edts=edts))))

    def test_fragment_edit_to_end(self):
        def movie(duration, media_time, timescale=44100):
            elst = bx(b'elst', bytes(4) + struct.pack('>IIiI', 1, duration, media_time, 0x10000))
            mdhd = bx(b'mdhd', bytes(4) + struct.pack('>IIII', 0, 0, timescale, 0) + bytes(4))
            mvhd = bx(b'mvhd', bytes(4) + struct.pack('>IIII', 0, 0, 600, 0) + bytes(80))
            trak = bx(b'trak', bx(b'edts', elst) + bx(b'mdia', mdhd))
            return bx(b'ftyp', b'iso6\0\0\0\0') + bx(b'moov', mvhd + trak)
        source = self.write('frag.mp4', movie(0, 1024))
        tracks = {'source_track': {'Duration': '160.078', 'FrameCount': '6894'},
                  'output_track': {'Duration': '160.055', 'FrameCount': '6893', 'FrameRate': '43.066'}}
        for field in ('mediainfo:0:Duration', 'mediainfo:0:FrameCount'):
            found = rc.problem(field, '', **tracks)
            self.assertTrue(rc.fragment_edit_to_end(found, self.context(source=source)), field)
            self.assertFalse(rc.fragment_edit_to_end(found, self.context(source=source, fragmented=False)))
        # Other fields, a stated edit duration, or a gap that is not the media time.
        self.assertFalse(rc.fragment_edit_to_end(rc.problem('mediainfo:0:FrameRate', '', **tracks),
                                                 self.context(source=source)))
        stated = self.write('stated.mp4', movie(160055, 1024))
        found = rc.problem('mediainfo:0:Duration', '', **tracks)
        self.assertFalse(rc.fragment_edit_to_end(found, self.context(source=stated)))
        far = dict(tracks, output_track=dict(tracks['output_track'], Duration='160.000'))
        self.assertFalse(rc.fragment_edit_to_end(rc.problem('mediainfo:0:Duration', '', **far),
                                                 self.context(source=source)))
        miscounted = dict(tracks, output_track=dict(tracks['output_track'], FrameCount='6890'))
        self.assertFalse(rc.fragment_edit_to_end(rc.problem('mediainfo:0:FrameCount', '', **miscounted),
                                                 self.context(source=source)))
        # ffprobe's duration_ts: apart by the media time within one movie
        # tick (160 media ticks at 96 kHz in a 600 Hz movie).
        video = self.write('video.mp4', movie(0, 8008, timescale=96000))
        ticks = rc.problem('stream:0:duration_ts', '', source=143006864, output=142998880)
        self.assertTrue(rc.fragment_edit_to_end(ticks, self.context(source=video)))
        self.assertFalse(rc.fragment_edit_to_end(ticks, self.context(source=video, fragmented=False)))
        off = rc.problem('stream:0:duration_ts', '', source=143006864, output=142998000)
        self.assertFalse(rc.fragment_edit_to_end(off, self.context(source=video)))

    def test_final_packet_capped(self):
        def movie(stts, mdhd_duration):
            stts_box = bx(b'stts', bytes(4) + struct.pack('>I', len(stts))
                          + b''.join(struct.pack('>II', n, d) for n, d in stts))
            mdhd = bx(b'mdhd', bytes(4) + struct.pack('>IIII', 0, 0, 44100, mdhd_duration) + bytes(4))
            trak = bx(b'trak', bx(b'mdia', mdhd + bx(b'minf', bx(b'stbl', stts_box))))
            return bx(b'ftyp', b'isom\0\0\0\0') + bx(b'moov', trak)
        source = self.write('capped_src.mp4', movie([(379, 1024)], 388095))
        output = self.write('capped_out.mp4', movie([(379, 1024)], 388095))
        line = '0, 387072, 387072, {}, 452, b647'
        found = rc.problem('stream:0:packet:378', '', source=line.format(1024), output=line.format(1023),
                           next_output=None)
        c = self.context(fragmented=False, source=source, output=output)
        self.assertTrue(rc.final_packet_capped(found, c))
        # Not the last packet, a fragmented source, other durations.
        self.assertFalse(rc.final_packet_capped(dict(found, data=dict(found['data'], next_output=line)), c))
        self.assertFalse(rc.final_packet_capped(found, self.context(source=source, output=output)))
        off = dict(found, data=dict(found['data'], output=line.format(1000)))
        self.assertFalse(rc.final_packet_capped(off, c))
        # The output declares another duration.
        other = self.write('capped_other.mp4', movie([(379, 1024)], 388096))
        self.assertFalse(rc.final_packet_capped(found, self.context(fragmented=False, source=source,
                                                                    output=other)))

    def test_base_time(self):
        def movie(edits):
            tkhd = bx(b'tkhd', bytes(4) + struct.pack('>IIII', 0, 0, 1, 0) + bytes(4 + 8 + 8 + 36 + 8))
            stts = bx(b'stts', bytes(4) + struct.pack('>III', 1, 36000, 100))
            mdhd = bx(b'mdhd', bytes(4) + struct.pack('>IIII', 0, 0, 6000, 0) + bytes(4)) + bx(b'minf', bx(b'stbl', stts))
            mvhd = bx(b'mvhd', bytes(4) + struct.pack('>IIII', 0, 0, 600, 0) + bytes(80))
            parts = tkhd
            if edits is not None:
                elst = bx(b'elst', bytes(4) + struct.pack('>I', len(edits))
                          + b''.join(struct.pack('>IiI', d, t, 0x10000) for d, t in edits))
                parts += bx(b'edts', elst)
            trak = bx(b'trak', parts + bx(b'mdia', mdhd))
            return bx(b'ftyp', b'isom\0\0\0\0') + bx(b'moov', mvhd + trak)
        source = self.write('base_src.mp4', movie(None))
        output = self.write('base_out.mp4', movie([(6000, -1), (360000, 0)]))
        c = self.context(source=source, output=output)
        c['first'] = {0: ('0, 60000, 60100, 100, 21185, 21c4', '0, 59900, 60000, 100, 21185, 21c4')}
        packet = rc.problem('stream:0:packet:1', '', source='0, 60100, 60200, 100, 880, 20fb',
                            output='0, 60000, 60100, 100, 880, 20fb', next_output=None)
        self.assertTrue(rc.base_time(packet, c))
        moved = rc.problem('stream:0:packet:1', '', source='0, 60100, 60200, 100, 880, 20fb',
                           output='0, 60000, 60000, 100, 880, 20fb', next_output=None)
        self.assertFalse(rc.base_time(moved, c))
        self.assertTrue(rc.base_time(rc.problem('stream:0:start_pts', '', source=60100, output=60000), c))
        self.assertTrue(rc.base_time(rc.problem('stream:0:duration_ts', '', source=3660000, output=3600000), c))
        self.assertFalse(rc.base_time(rc.problem('stream:0:duration_ts', '', source=3660000, output=3600001), c))
        self.assertTrue(rc.base_time(rc.problem('mediainfo:0:Duration', '', source='600.000', output='610.000'), c))
        self.assertFalse(rc.base_time(rc.problem('mediainfo:0:Duration', '', source='600.000', output='611.000'), c))
        # The source has an edit list, or the output's is something else.
        with_edit = self.write('base_edit.mp4', movie([(360000, 0)]))
        c2 = dict(c, entry=dict(c['entry'], source=with_edit))
        self.assertFalse(rc.base_time(packet, c2))
        c3 = dict(c, entry=dict(c['entry'], output=with_edit))
        self.assertFalse(rc.base_time(packet, c3))

    def test_probe_window(self):
        whole = {'/src': {'profile': 'LC', 'codec_name': 'aac'}, '/out': {'profile': 'LC', 'codec_name': 'aac'}}
        original = rc._probe_all
        rc._probe_all = lambda binary, path, index: whole[path]
        try:
            c = self.context(source='/src', output='/out')
            c['ffprobe'] = 'ffprobe'
            found = rc.problem('stream:0:profile', '', source='unknown', output='LC')
            self.assertTrue(rc.probe_window(found, c))
            # A value neither default probe gave, or other fields.
            self.assertFalse(rc.probe_window(rc.problem('stream:0:profile', '', source='unknown',
                                                        output='HE-AAC'), c))
            self.assertFalse(rc.probe_window(rc.problem('stream:0:pix_fmt', '', source='a', output='b'), c))
            head = rc.problem('stream:0:framemd5:header', '', source=['#tb 0: 1/48000', '#codec_id 0: mp3'],
                              output=['#tb 0: 1/48000', '#codec_id 0: mp2'])
            whole['/src']['codec_name'] = whole['/out']['codec_name'] = 'mp2'
            self.assertTrue(rc.probe_window(head, c))
            # The output may not be the file that misses the value.
            reversed_head = rc.problem('stream:0:framemd5:header', '', source=['#codec_id 0: mp2'],
                                       output=['#codec_id 0: mp3'])
            self.assertFalse(rc.probe_window(reversed_head, c))
            self.assertFalse(rc.probe_window(rc.problem('stream:0:profile', '', source='LC', output='unknown'), c))
            other = rc.problem('stream:0:framemd5:header', '', source=['#tb 0: 1/48000'], output=['#tb 0: 1/44100'])
            self.assertFalse(rc.probe_window(other, c))
            # The whole-file probe still disagrees.
            whole['/out']['profile'] = 'HE-AAC'
            self.assertFalse(rc.probe_window(found, c))
        finally:
            rc._probe_all = original

    def test_vfr_rate(self):
        def movie(stts):
            box = bx(b'stts', bytes(4) + struct.pack('>I', len(stts))
                     + b''.join(struct.pack('>II', n, d) for n, d in stts))
            trak = bx(b'trak', bx(b'mdia', bx(b'minf', bx(b'stbl', box))))
            return bx(b'ftyp', b'isom\0\0\0\0') + bx(b'moov', trak)
        vfr = self.write('vfr.mp4', movie([(10, 1500), (1, 1501), (5, 1500)]))
        cfr = self.write('cfr.mp4', movie([(16, 1500)]))
        c = self.context(fragmented=False, source=vfr, output=vfr,
                         streams=[{'index': 0, 'avg_frame_rate': '60/1'}])
        found = rc.problem('stream:0:r_frame_rate', '', source='60/1', output='60000/1001', output_avg='60/1')
        self.assertTrue(rc.vfr_rate(found, c))
        self.assertFalse(rc.vfr_rate(dict(found, data=dict(found['data'], output_avg='59/1')), c))
        c2 = self.context(fragmented=False, source=cfr, output=cfr,
                          streams=[{'index': 0, 'avg_frame_rate': '60/1'}])
        self.assertFalse(rc.vfr_rate(found, c2))
        c3 = self.context(fragmented=False, source=vfr, output=cfr,
                          streams=[{'index': 0, 'avg_frame_rate': '60/1'}])
        self.assertFalse(rc.vfr_rate(found, c3))

    def test_fragment_stated_edit(self):
        def movie(edit, movie_ts, stts=None):
            tkhd = bx(b'tkhd', bytes(4) + struct.pack('>IIII', 0, 0, 1, 0) + bytes(4 + 8 + 8 + 36 + 8))
            elst = bx(b'elst', bytes(4) + struct.pack('>IIiI', 1, edit, 1600, 0x10000))
            mdhd = bx(b'mdhd', bytes(4) + struct.pack('>IIII', 0, 0, 11988, 0) + bytes(4))
            mvhd = bx(b'mvhd', bytes(4) + struct.pack('>IIII', 0, 0, movie_ts, 0) + bytes(80))
            minf = b''
            if stts:
                box = bx(b'stts', bytes(4) + struct.pack('>I', 1) + struct.pack('>II', *stts))
                minf = bx(b'minf', bx(b'stbl', box))
            trak = bx(b'trak', tkhd + bx(b'edts', elst) + bx(b'mdia', mdhd + minf))
            return bx(b'ftyp', b'isom\0\0\0\0') + bx(b'moov', mvhd + trak)
        source = self.write('stated_src.mp4', movie(122423, 1000))
        output = self.write('stated_out.mp4', movie(366901731, 2997000, stts=(3669, 400)))
        tracks = {'source_track': {'Duration': '122.422', 'FrameCount': '3669'},
                  'output_track': {'Duration': '122.423', 'FrameCount': '3669', 'FrameRate': '29.970'}}
        c = self.context(source=source, output=output)
        self.assertTrue(rc.fragment_stated_edit(rc.problem('mediainfo:0:Duration', '', **tracks), c))
        self.assertTrue(rc.fragment_stated_edit(rc.problem('mediainfo:0:FrameCount', '', **tracks), c))
        self.assertFalse(rc.fragment_stated_edit(rc.problem('mediainfo:0:Duration', '', **tracks),
                                                 self.context(source=source, output=output, fragmented=False)))
        off = dict(tracks, output_track=dict(tracks['output_track'], Duration='122.430'))
        self.assertFalse(rc.fragment_stated_edit(rc.problem('mediainfo:0:Duration', '', **off), c))
        # An edit of duration 0 is KD-R-FRAG-EDIT-END's.
        to_end = self.write('stated_end.mp4', movie(0, 1000))
        self.assertFalse(rc.fragment_stated_edit(rc.problem('mediainfo:0:Duration', '', **tracks),
                                                 self.context(source=to_end, output=output)))

    def test_quicktime_channel_layout(self):
        esds = bx(b'esds', bytes(4))
        chan = bx(b'chan', bytes(16))
        source = self.write('src.mov', sound_file([esds, chan]))
        output = self.write('out.mp4', sound_file([esds]))
        found = rc.problem('mediainfo:0:ChannelLayout', '', source='C', output='M')
        self.assertTrue(rc.quicktime_channel_layout(found, self.context(source=source, output=output)))
        # No chan in the source, or one left in the output.
        self.assertFalse(rc.quicktime_channel_layout(found, self.context(source=output, output=output)))
        self.assertFalse(rc.quicktime_channel_layout(found, self.context(source=source, output=source)))


if __name__ == '__main__':
    unittest.main()
