"""tools/compat-corpus/run.py: a B3 difference on a fixture is set aside only
when the two outputs agree once the keys ffmpeg9.json names for that file,
and nothing else, are taken out of both, in the command's own writer, and
every value that differs on them is the difference the entry accepts.
ffmpeg9.json and docs/known-divergences-ffprobe9.md name the same entries.

    python3 -I -m unittest discover -s tools/compat-corpus/tests -p 'test_*.py'
"""
import argparse
import contextlib
import importlib.util
import io
import json
import pathlib
import re
import sys
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location('run', ROOT / 'tools' / 'compat-corpus' / 'run.py')
run = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(run)


def agree(a, b, args, keys):
    name, options = run.writer_of(args)
    options = {**options, **run.WRITER_FULL[name]}
    return run.without(a, name, options, set(keys)) == run.without(b, name, options, set(keys))


DEFAULT_9 = """[STREAM]
index=0
codec_name=aac
mime_codec_string=mp4a.40.2
initial_padding=1024
DISPOSITION:default=1
DISPOSITION:multilayer=0
TAG:language=und
[/STREAM]
[PACKET]
pts=0
[SIDE_DATA]
side_data_type=Skip Samples
skip_samples=1024
[/SIDE_DATA]
[/PACKET]
"""
DEFAULT_COREAV = """[STREAM]
index=0
codec_name=aac
initial_padding=0
DISPOSITION:default=1
TAG:language=und
TAG:vendor_id=[0][0][0][0]
[/STREAM]
[PACKET]
pts=0
[/PACKET]
"""
KEYS = {'streams.mime_codec_string', 'streams.initial_padding', 'streams.disposition.multilayer',
        'streams.tags.vendor_id', 'packets.side_data_list[Skip Samples]'}


class Narrowing(unittest.TestCase):
    def test_the_named_keys_and_nothing_else(self):
        self.assertTrue(agree(DEFAULT_9, DEFAULT_COREAV, ['-show_streams'], KEYS))
        for key in KEYS:
            self.assertFalse(agree(DEFAULT_9, DEFAULT_COREAV, ['-show_streams'], KEYS - {key}), key)
        other = DEFAULT_COREAV.replace('codec_name=aac', 'codec_name=mp3')
        self.assertFalse(agree(DEFAULT_9, other, ['-show_streams'], KEYS))

    def test_the_keys_found_differing(self):
        name, options = run.writer_of([])
        self.assertEqual(run.differing_keys(DEFAULT_9, DEFAULT_COREAV, name, options), sorted(KEYS))

    def test_json_keeps_its_commas(self):
        a = '{\n    "streams": [\n        {\n            "index": 0,\n            "refs": 1,\n' \
            '            "disposition": {\n                "default": 1,\n                "multilayer": 0\n' \
            '            },\n            "tags": {\n                "vendor_id": "x"\n            }\n        }\n    ]\n}\n'
        b = '{\n    "streams": [\n        {\n            "index": 0,\n' \
            '            "disposition": {\n                "default": 1\n            }\n        }\n    ]\n}\n'
        keys = {'streams.refs', 'streams.disposition.multilayer', 'streams.tags.vendor_id'}
        self.assertTrue(agree(a, b, ['-of', 'json'], keys))
        self.assertFalse(agree(a, b.replace('"default": 1', '"default": 0'), ['-of', 'json'], keys))
        compact = '{\n    "streams": [\n        { "index": 0, "refs": 1, "width": 2 }\n    ]\n}\n'
        self.assertTrue(agree(compact, compact.replace('"refs": 1, ', ''), ['-of', 'json=c=1'], {'streams.refs'}))
        self.assertFalse(agree(compact, compact.replace('"refs": 1, ', ''), ['-of', 'json=c=1'], set()))

    def test_json_side_data(self):
        entry = '            "side_data_list": [\n                {\n                    "side_data_type": "MPEGTS Stream ID",\n' \
                '                    "id": 192\n                }\n            ]'
        a = '{\n    "packets": [\n        {\n            "pts": 1,\n' + entry + '\n        }\n    ]\n}\n'
        b = '{\n    "packets": [\n        {\n            "pts": 1\n        }\n    ]\n}\n'
        self.assertTrue(agree(a, b, ['-of', 'json'], {'packets.side_data_list[MPEGTS Stream ID]'}))
        self.assertFalse(agree(a, b, ['-of', 'json'], {'packets.side_data_list[Skip Samples]'}))

    def test_compact_side_data_and_an_empty_entry(self):
        a = 'packet|pts=1|flags=K__|side_datum/mpegts_stream_id:side_data_type=MPEGTS Stream ID' \
            '|side_datum/mpegts_stream_id:id=192\npacket|pts=2|flags=K__|\n'
        b = 'packet|pts=1|flags=K__\npacket|pts=2|flags=K__\n'
        keys = {'packets.side_data_list[MPEGTS Stream ID]', 'packets.side_data_list[]'}
        self.assertTrue(agree(a, b, ['-of', 'compact'], keys))
        self.assertFalse(agree(a, b, ['-of', 'compact'], {'packets.side_data_list[]'}))

    def test_a_stream_group_is_one_item(self):
        a = 'stream_group,stream,codec_type=video\nstream,codec_type=data\n\nstream,codec_type=video\n'
        b = 'stream,codec_type=video\n'
        self.assertTrue(agree(a, b, ['-of', 'csv'], {'stream_groups'}))
        self.assertFalse(agree(a, b, ['-of', 'csv'], set()))

    def test_a_writer_without_keys_is_read_with_them(self):
        args = ['-show_streams', '-of', 'csv=p=0', 'INPUT']
        name, options = run.writer_of(args)
        self.assertEqual(run.full_args(args, name, options),
                         ['-show_streams', '-of', 'csv=nk=0:p=1:s=,:e=csv', 'INPUT'])
        full = 'stream,index=0,codec_name="a,b",refs=1\npacket,pts=1,\n'
        self.assertEqual(run.derive(full, name, options), '0,"a,b",1\n1,\n')
        name, options = run.writer_of(['-of', 'default=nw=1:nk=1'])
        self.assertEqual(run.derive('[FORMAT]\nduration=1.0\n[/FORMAT]\n', name, options), '1.0\n')
        name, options = run.writer_of(['-of', 'flat=s=_'])
        self.assertEqual(run.derive('streams.stream.0.r_frame_rate="25/1"\n', name, options),
                         'streams_stream_0_r_frame_rate="25/1"\n')
        self.assertIsNone(run.full_args(['-of', 'json'], *run.writer_of(['-of', 'json'])))


def packets(*durations):
    """A default-writer packet listing: (codec_type, stream_index, duration
    in ms) per packet."""
    return ''.join(f'[PACKET]\ncodec_type={kind}\nstream_index={index}\nduration={ms}\n'
                   f'duration_time={ms / 1000:.6f}\n[/PACKET]\n' for kind, index, ms in durations)


class Accepts(unittest.TestCase):
    """Each entry sets aside only the difference it names, on the stream or
    packet it names, as the whole file's listing shows it."""
    PATH = pathlib.Path('mkv_h264_aac.mkv')

    def covers(self, ref, out, listing):
        def canonical(coreav, path):
            return {f'packets.{key}': [(f'{f / scale:.6f}' if scale > 1 else str(f),
                                        f'{c / scale:.6f}' if scale > 1 else str(c), context)
                                       for f, c, context in listing]
                    for key, scale in (('duration', 1), ('duration_time', 1000))}
        with mock.patch.object(run, 'canonical', canonical):
            return run.ffmpeg9_covers('coreav', ['-show_packets'], self.PATH, ref.encode(), out.encode())[0]

    def test_only_the_last_aac_packet_is_cut(self):
        video = {'codec_type': 'video', 'codec_name': 'h264', 'last': True}
        aac = {'codec_type': 'audio', 'codec_name': 'aac', 'last': True}
        listing = [(40, 40, video), (16, 21, aac)]
        ref = packets(('video', 0, 40), ('audio', 1, 16))
        entry = ['KD-FF9-FIXTURE-MKV-LAST-DURATION']
        self.assertEqual(self.covers(ref, packets(('video', 0, 40), ('audio', 1, 21)), listing), entry)
        # An earlier video packet's duration differs too: not the case named.
        self.assertIsNone(self.covers(ref, packets(('video', 0, 999), ('audio', 1, 21)), listing))
        self.assertIsNone(self.covers(ref, packets(('video', 0, 40), ('audio', 1, 21)),
                                      [(40, 999, video), (16, 21, aac)]))
        # Nor an AAC packet before the last, nor a longer last one.
        self.assertIsNone(self.covers(ref, packets(('video', 0, 40), ('audio', 1, 21)),
                                      [(40, 40, video), (16, 21, dict(aac, last=False))]))
        self.assertIsNone(self.covers(packets(('video', 0, 40), ('audio', 1, 30)),
                                      packets(('video', 0, 40), ('audio', 1, 21)),
                                      [(40, 40, video), (30, 21, aac)]))

    def test_values_as_the_formatting_options_print_them(self):
        F = run.fractions.Fraction
        self.assertEqual(run.plain('0:00:-0.007000'), F(-7, 1000))
        self.assertEqual(run.plain('0:00:20.160000'), F(2016, 100))
        self.assertEqual(run.plain('844.284000 Kbit/s'), 844_284)
        self.assertEqual(run.plain('1.5 Kibyte'), 1536)
        self.assertIsNone(run.plain('N/A'))
        self.assertIsNone(run.plain('unknown'))
        self.assertEqual(run.plain('hexagonal'), 'hexagonal')

    def test_bit_rates_by_their_ratio_not_the_file_size(self):
        rule = {'ratio': ['19.5', '20.5']}
        for f, c in (('749136', '37457'), ('748184', '37409'), ('65892', '3310')):
            self.assertTrue(run.satisfies(rule, f, c, {}), (f, c))
        self.assertFalse(run.satisfies(rule, '749136', '749136', {}))
        self.assertFalse(run.satisfies(rule, '749136', '0', {}))
        rule = {'ratio': ['0.9960', '0.9961']}
        self.assertTrue(run.satisfies(rule, '884913', '888439', {}))
        self.assertFalse(run.satisfies(rule, '884913', '884913', {}))

    def test_side_data_coreav_lists_on_only_some_packets(self):
        rule = {'only': 'part of ffprobe'}
        key = 'packets.side_data_list[MPEGTS Stream ID]'
        a, b = '{"id": 224}', '{"id": 192}'

        def accepts(ref, out, listing):
            with mock.patch.object(run, 'canonical', lambda coreav, path: {key: listing}):
                return run.accepts('coreav', self.PATH, key, rule, ref, out)
        self.assertTrue(accepts([a, b, b], [a, b], [(a, a, {}), (b, b, {}), (b, 'N/A', {})]))
        self.assertFalse(accepts([a, b], [a, 'c'], [(a, a, {}), (b, 'c', {})]))
        # Each value is one ffprobe lists, but on another packet.
        self.assertFalse(accepts([a, b], [b], [(a, b, {}), (b, 'N/A', {})]))


class MalformedOutput(unittest.TestCase):
    """Output of the JSON writer that is not JSON fails B3 for that command
    on that file, and the run goes on to the next file."""

    def test_output_that_is_not_json_is_a_b3_failure(self):
        args = ['-v', 'error', '-of', 'json', '-show_streams', 'INPUT']
        ref = b'{\n    "streams": [\n        {\n            "index": 0\n        }\n    ]\n}\n'
        files = [pathlib.Path('a.mp4'), pathlib.Path('b.mp4')]
        for bad in (ref[:30], ref.replace(b'"index"', b'"in\\qdex"')):
            with self.subTest(bad=bad), tempfile.TemporaryDirectory() as tmp:
                def fake_run(cmd):
                    return 0, bad if cmd[0] == 'coreav' and cmd[-1] == 'a.mp4' else ref, b''
                opts = argparse.Namespace(media=[], coreav='coreav', jobs=2, report=f'{tmp}/report.json', top=0,
                                          write_baseline=None, baseline=None)
                with mock.patch.object(run, 'run', fake_run), \
                        mock.patch.object(run, 'media_files', lambda dirs: files), \
                        mock.patch.object(run, 'load_commands',
                                          lambda: [{'tool': 'ffprobe', 'label': 'native', 'args': args}]), \
                        contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(run.cmd_commands(opts), 1)
                report = json.loads(pathlib.Path(opts.report).read_text())
                self.assertEqual(report['B3'], {'pass': 1, 'fail': 1})
                self.assertEqual([(f['bar'], f['file'], f['keys']) for f in report['failures']], [('B3', 'a.mp4', None)])


class Av1Nclx(unittest.TestCase):
    COLOUR = b'nclx\0\6\0\6\0\6\x80'
    SHORT = 'av01.0.00M.08'
    FULL = 'av01.0.00M.08.0.110.06.06.06.1'

    def setUp(self):
        run.av1_nclx_fixture.cache_clear()

    def fixture(self, colour=COLOUR, record=b'\x81\0\x0c\0'):
        def box(kind, body):
            return (len(body) + 8).to_bytes(4, 'big') + kind + body
        data = box(b'av01', bytes(78) + box(b'av1C', record) + box(b'colr', colour))
        data = box(b'stsd', b'\0\0\0\0\0\0\0\1' + data)
        for kind in (b'stbl', b'minf', b'mdia', b'trak', b'moov'):
            data = box(kind, data)
        return data

    def listed(self, mime, tag='av01'):
        return json.dumps({'streams': [{'codec_name': 'av1', 'codec_tag_string': tag,
                                       'mime_codec_string': mime}]}).encode()

    def test_the_exact_fixture_bytes_and_pair_are_required(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'color_av1_colr_ignored.mp4'
            path.write_bytes(self.fixture())
            def command(argv):
                return 0, self.listed(self.SHORT if argv[0] == 'ffprobe' else self.FULL), b''
            with mock.patch.object(run, 'run', command):
                self.assertTrue(run.av1_nclx_fixture('coreav', path))
                self.assertFalse(run.av1_nclx_fixture('coreav', path.with_name('another.mp4')))
                self.assertFalse(run.accepts('coreav', path, 'streams.codec_tag_string',
                                             {'evidence': 'av1 nclx'}, ['av01'], ['wrong']))
                ref = (f'[STREAM]\ncodec_name=av1\ncodec_type=video\ncodec_tag_string=av01\n'
                       f'mime_codec_string={self.SHORT}\n[/STREAM]\n').encode()
                got = ref.replace(self.SHORT.encode(), self.FULL.encode())
                canonical = {'streams.mime_codec_string': [
                    (self.SHORT, self.FULL, {'codec_type': 'video', 'codec_name': 'av1'})]}
                with mock.patch.object(run, 'canonical', return_value=canonical):
                    self.assertEqual(run.ffmpeg9_covers('coreav', ['-show_streams'], path, ref, got)[0],
                                     ['KD-FF9-AV1-NCLX'])
                    self.assertIsNone(run.ffmpeg9_covers('coreav', ['-show_streams'], path, ref,
                                                        got.replace(b'codec_tag_string=av01',
                                                                    b'codec_tag_string=wrong'))[0])
            for at in range(len(self.COLOUR)):
                changed = bytearray(self.COLOUR)
                changed[at] ^= 1
                path.write_bytes(self.fixture(bytes(changed)))
                run.av1_nclx_fixture.cache_clear()
                with self.assertRaisesRegex(SystemExit, 'nclx colour or range bytes'):
                    run.av1_nclx_fixture('coreav', path)
            path.write_bytes(self.fixture(record=b'\x81\1\x0c\0'))
            with self.assertRaisesRegex(SystemExit, 'changed AV1 configuration'):
                run.av1_nclx_fixture('coreav', path)

    def test_changed_outputs_or_a_stale_exception_fail_loudly(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'color_av1_colr_ignored.mp4'
            path.write_bytes(self.fixture())
            for ref, got in [(self.FULL, self.FULL), (self.SHORT, self.SHORT),
                             (self.SHORT, self.FULL + '.0'), (self.SHORT + '.0', self.FULL)]:
                run.av1_nclx_fixture.cache_clear()
                def command(argv):
                    return 0, self.listed(ref if argv[0] == 'ffprobe' else got), b''
                with mock.patch.object(run, 'run', command), self.assertRaises(SystemExit):
                    run.av1_nclx_fixture('coreav', path)
            run.av1_nclx_fixture.cache_clear()
            with mock.patch.object(run, 'run', return_value=(0, self.listed(self.FULL), b'')), \
                    mock.patch.object(run, 'media_files', return_value=[path]), \
                    self.assertRaisesRegex(SystemExit, 'oracle now agrees'):
                run.cmd_commands(argparse.Namespace(coreav='coreav', media=[]))
            run.av1_nclx_fixture.cache_clear()
            with mock.patch.object(run, 'run', return_value=(0, self.listed(self.FULL), b'')), \
                    mock.patch.object(run, 'media_files', return_value=[path]), \
                    self.assertRaisesRegex(SystemExit, 'oracle now agrees'):
                run.cmd_commands_010(argparse.Namespace(coreav='coreav', media=[], baseline=None,
                                                       write_baseline=None))
            run.av1_nclx_fixture.cache_clear()
            def wrong_tag(argv):
                return 0, self.listed(self.SHORT if argv[0] == 'ffprobe' else self.FULL,
                                      tag='wrong'), b''
            with mock.patch.object(run, 'run', wrong_tag), self.assertRaises(SystemExit):
                run.av1_nclx_fixture('coreav', path)


class EmptyEditPadding(unittest.TestCase):
    """KD-FF9-EMPTY-EDIT-PADDING covers only the last audio packet's
    Skip Samples side data, with the padding ISO/IEC 14496-12 8.6.6 and the
    fixture's bytes give, where ffprobe lists none; any other side data
    difference stays a failure."""
    PADDING = 888

    def setUp(self):
        run.empty_edit_padding_fixture.cache_clear()

    @staticmethod
    def fixture(empty=2691328, last=136, config=b'\x12\x10', edits=None, scale=44100, index=1):
        """`edit_list.mp4`'s audio track as FFmpeg 9.0.2 writes it: an
        empty edit, then a media edit from 0 over the media's 89,224 ticks at
        44.1 kHz, the last of its 88 samples lasting 136 ticks, all in one
        chunk of sample description `index`."""
        def box(kind, body):
            return (len(body) + 8).to_bytes(4, 'big') + kind + body

        def full(kind, body):
            return box(kind, bytes(4) + body)

        def u32(*values):
            return b''.join(v.to_bytes(4, 'big') for v in values)

        movie = 5644800
        edits = edits if edits is not None else [(empty, 0xFFFFFFFF), (11420672, 0)]
        elst = u32(len(edits)) + b''.join(u32(d, t, 0x10000) for d, t in edits)
        es = bytes([5, len(config)]) + config
        decoder = bytes([4, 13 + len(es), 0x40, 0x15]) + bytes(11) + es
        esds = full(b'esds', bytes([3, 3 + len(decoder) + 3, 0, 1, 0]) + decoder + bytes([6, 1, 2]))
        mp4a = box(b'mp4a', bytes(28) + esds)
        stbl = box(b'stbl', full(b'stsd', u32(1) + mp4a) + full(b'stts', u32(2, 87, 1024, 1, last))
                   + full(b'stsc', u32(1, 1, 88, index)))
        mdia = box(b'mdia', full(b'mdhd', u32(0, 0, scale, 87 * 1024 + last, 0))
                   + full(b'hdlr', u32(0) + b'soun' + bytes(12)) + box(b'minf', stbl))
        trak = box(b'trak', full(b'tkhd', bytes(80)) + box(b'edts', full(b'elst', elst)) + mdia)
        video = box(b'trak', box(b'mdia', full(b'hdlr', u32(0) + b'vide' + bytes(12))))
        return box(b'ftyp', b'isom') + box(b'moov', full(b'mvhd', u32(0, 0, movie, 0) + bytes(84))
                                                 + video + trak)

    @staticmethod
    def side(padding, skip=0):
        return json.dumps({'discard_padding': padding, 'discard_reason': 0, 'side_data_type': 'Skip Samples',
                           'skip_reason': 0, 'skip_samples': skip}, sort_keys=True)

    def listing(self, last=None, first=None, video=None, last_ref='N/A'):
        """The Skip Samples pairs (ffprobe's, CoreAV's) of a video packet and
        two audio packets, the second its stream's last at position 137."""
        audio = {'codec_type': 'audio', 'codec_name': 'aac'}
        return {'packets.side_data_list[Skip Samples]': [
            ('N/A', video or 'N/A', {'codec_type': 'video', 'codec_name': 'h264', 'last': True, 'position': 0}),
            ('N/A', first or 'N/A', dict(audio, last=False, position=1)),
            (last_ref, last or self.side(self.PADDING), dict(audio, last=True, position=137))]}

    def covered(self, listing):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'edit_list.mp4'
            path.write_bytes(self.fixture())
            run.empty_edit_padding_fixture.cache_clear()
            with mock.patch.object(run, 'canonical', return_value=listing):
                return run.empty_edit_padding_fixture('coreav', path)

    def test_the_bytes_give_the_padding_the_clause_explains(self):
        self.assertEqual(run.empty_edit_padding(self.fixture()), self.PADDING)
        self.assertEqual(run.empty_edit_padding(self.fixture(config=b'\x12\x14')), 960 - 136)
        # The extension that rules SBR out, as FFmpeg writes it.
        self.assertEqual(run.empty_edit_padding(self.fixture(config=b'\x12\x10\x56\xe5\x00')), self.PADDING)
        whole = [(2691328, 0xFFFFFFFF), (11534336, 0)]
        # The media's 89,224 ticks at 24 kHz are 20,985,484.8 movie ticks.
        at_24k = [(2691328, 0xFFFFFFFF), (20985485, 0)]
        self.assertEqual(run.empty_edit_padding(self.fixture(config=b'\x13\x10\x56\xe5\x00', scale=24000,
                                                             edits=at_24k)), self.PADDING)
        bad = [self.fixture(last=1024, edits=whole),  # no padding
               self.fixture(empty=44 * 128),  # ffprobe would list padding too
               self.fixture(edits=[(11420672, 0)]),  # no empty edit
               self.fixture(edits=[(2691328, 0xFFFFFFFF), (11420672, 0), (100, 0)]),
               self.fixture(config=b'\x12\x10\x56\xe5\x98'),  # SBR at 48 kHz
               self.fixture(config=b'\x11\x90'),  # 48 kHz in a 44.1 kHz track
               self.fixture(index=2),  # a chunk of another description
               # A media edit from inside the last sample, or past it, after
               # an empty edit of 3 s.
               self.fixture(edits=[(3 * 5644800, 0xFFFFFFFF), (11420672, 87 * 1024 + 12)]),
               self.fixture(edits=[(3 * 5644800, 0xFFFFFFFF), (11420672, 100000)]),
               # LC at 24 kHz without the extension: implicit SBR is open.
               self.fixture(config=b'\x13\x10', scale=24000, edits=at_24k)]
        for data in bad:
            with self.assertRaises(ValueError):
                run.empty_edit_padding(data)

    def test_only_the_exact_tail_padding_is_covered(self):
        self.assertTrue(self.covered(self.listing()))
        # Invented side data on another packet, a moved or a changed padding.
        for listing in [self.listing(first=self.side(999999, 999999)),
                        self.listing(video=self.side(0, 1)),
                        self.listing(last='N/A', first=self.side(self.PADDING)),
                        self.listing(last=self.side(self.PADDING - 1)),
                        self.listing(last=self.side(self.PADDING + 1)),
                        self.listing(last=self.side(self.PADDING, 1)),
                        self.listing(last_ref=self.side(self.PADDING + 1)),
                        {}]:
            self.assertFalse(self.covered(listing), listing)
        with self.assertRaisesRegex(SystemExit, 'oracle now agrees'):
            self.covered(self.listing(last_ref=self.side(self.PADDING)))
        self.assertFalse(run.empty_edit_padding_fixture('coreav', pathlib.Path('another.mp4')))

    def test_changed_bytes_fail_loudly(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'edit_list.mp4'
            path.write_bytes(self.fixture(last=1024, edits=[(2691328, 0xFFFFFFFF), (11534336, 0)]))
            with mock.patch.object(run, 'canonical', return_value=self.listing()), \
                    self.assertRaisesRegex(SystemExit, 'KD-FF9-EMPTY-EDIT-PADDING: changed sample timing'):
                run.empty_edit_padding_fixture('coreav', path)

    def test_both_writer_forms_follow_the_evidence(self):
        rules = run.ffmpeg9_rules()['KD-FF9-EMPTY-EDIT-PADDING']
        side = 'packets.side_data_list[Skip Samples]'
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'edit_list.mp4'
            path.write_bytes(self.fixture())
            for listing, expected in [(self.listing(), True),
                                      (self.listing(first=self.side(999999, 999999)), False),
                                      (self.listing(last=self.side(self.PADDING + 1)), False)]:
                run.empty_edit_padding_fixture.cache_clear()
                with mock.patch.object(run, 'canonical', return_value=listing):
                    last = listing[side][-1][1]
                    self.assertEqual(run.accepts('coreav', path, side, rules[side], ['N/A'], [last]),
                                     expected)
                    # The writer form that selects none of its keys: an
                    # empty side data element.
                    self.assertEqual(run.accepts('coreav', path, 'packets.side_data_list[]',
                                                 rules['packets.side_data_list[]'], [], ['']), expected)


class Table(unittest.TestCase):
    def test_every_entry_is_documented_and_every_pair_listed_once(self):
        table = json.loads((ROOT / 'tools' / 'compat-corpus' / 'ffmpeg9.json').read_text())
        page = (ROOT / 'docs' / 'known-divergences-ffprobe9.md').read_text()
        headings = re.findall(r'^## (KD-FF9-[A-Z0-9-]+)$', page, re.M)
        self.assertEqual([e['id'] for e in table['entries']], headings)
        seen = set()
        for entry in table['entries']:
            for key, files in entry['keys'].items():
                self.assertEqual(files, sorted(set(files)), key)
                for name in files:
                    self.assertNotIn((key, name), seen)
                    seen.add((key, name))
            self.assertEqual(sorted(entry['accepts']), sorted(entry['keys']), entry['id'])
            for key, rule in entry['accepts'].items():
                if isinstance(rule, dict) and 'packets' in rule:
                    self.assertEqual(sorted(rule['packets']), entry['keys'][key], key)


if __name__ == '__main__':
    unittest.main()
