"""tools/compat-corpus/run.py: a B3 difference on a fixture is set aside only
when the two outputs agree once the keys ffmpeg9.json names for that file,
and nothing else, are taken out of both, in the command's own writer, and
every value that differs on them is the difference the entry accepts.
ffmpeg9.json and docs/known-divergences-ffprobe9.md name the same entries.

    python3 -I -m unittest discover -s tools/compat-corpus/tests -p 'test_*.py'
"""
import importlib.util
import json
import pathlib
import re
import sys
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
