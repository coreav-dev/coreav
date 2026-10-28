"""Tests for the MediaInfo oracle on JSON snapshots of black-box output.

The snapshots are MediaInfo 26.10 `--Output=JSON --Full` output on the
generated fixtures, trimmed to the fields the normalizer reads, and the
stream oracle records of ffprobe 7.0.2 and the CoreAV adapter. No media.
"""
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from fractions import Fraction

HERE = pathlib.Path(__file__).resolve().parent
TOOL = HERE.parent
sys.path.insert(0, str(TOOL))
import mediainfo_oracle as mo  # noqa: E402
import corpus  # noqa: E402

SNAPSHOTS = HERE / 'snapshots'


def raw(name):
    return json.loads((SNAPSHOTS / f'{name}.mediainfo-raw.json').read_text())


def normalized(name):
    return mo.normalize(raw(name), name)


def oracle(name, side):
    return json.loads((SNAPSHOTS / f'{name}.{side}.streams.json').read_text())


class NormalizeTest(unittest.TestCase):
    def test_basic_streams(self):
        record = normalized('basic.mp4')
        self.assertTrue(record['probe_ok'])
        self.assertEqual(record['version'], '26.10')
        self.assertEqual(record['format_duration_s'], '2.000')
        video, audio = record['streams']
        self.assertEqual((video['index'], video['kind'], video['codec_name'], video['codec_fourcc']),
                         (0, 'video', 'h264', 'avc1'))
        self.assertEqual((video['width'], video['height'], video['rotation']), (320, 240, 0))
        self.assertEqual((audio['kind'], audio['codec_name'], audio['codec_fourcc']), ('audio', 'aac', 'mp4a'))
        self.assertEqual((audio['sample_rate'], audio['channels'], audio['rotation']), (44100, 1, None))
        self.assertEqual(audio['source_duration_s'], '2.023')

    def test_rotation_is_counter_clockwise(self):
        self.assertEqual(normalized('rotate_ccw90.mp4')['streams'][0]['rotation'], 90)
        self.assertEqual(normalized('rotate_ccw270.mp4')['streams'][0]['rotation'], 270)

    def test_rotation_rules(self):
        self.assertEqual(mo.rotation({'@type': 'Video'}), 0)
        self.assertIsNone(mo.rotation({'@type': 'Video', 'Rotation': '12.500'}))
        self.assertEqual(mo.rotation({'@type': 'Video', 'Rotation': '-90.000'}), 90)

    def test_colour_through_h273(self):
        video = normalized('color_bt2020_pq.mp4')['streams'][0]
        self.assertEqual((video['color_primaries'], video['color_transfer'], video['color_matrix'],
                          video['color_range']), ('bt2020', 'smpte2084', 'bt2020nc', 'tv'))
        video = normalized('color_full_range.mp4')['streams'][0]
        self.assertEqual((video['color_primaries'], video['color_transfer'], video['color_matrix'],
                          video['color_range']), ('smpte170m', 'smpte170m', 'bt601', 'pc'))

    def test_language_and_text(self):
        streams = normalized('multitrack.mp4')['streams']
        self.assertEqual([s['language'] for s in streams], [None, None, 'eng', 'kor', 'eng'])
        self.assertEqual((streams[4]['kind'], streams[4]['codec_name'], streams[4]['codec_fourcc']),
                         ('subtitle', 'mov_text', 'tx3g'))

    def test_matroska_codec_ids_have_no_fourcc(self):
        streams = normalized('browser.webm')['streams']
        self.assertEqual([(s['codec_name'], s['codec_fourcc']) for s in streams], [('vp9', None), ('opus', None)])
        self.assertEqual(streams[0]['rotation'], 0)

    def test_mpeg_audio_layer_and_pcm(self):
        self.assertEqual(normalized('audio_mp3.mp4')['streams'][0]['codec_name'], 'mp3')
        self.assertEqual(normalized('pcm_f32le.mov')['streams'][0]['codec_name'], 'pcm')

    def test_unknown_names_are_kept_visible(self):
        data = normalized('timecode.mov')['streams'][1]
        self.assertEqual((data['kind'], data['codec_name']), ('data', 'mi:QuickTime TC'))
        self.assertEqual(mo.mapped(mo.PRIMARIES, 'Martian'), 'mi:Martian')
        self.assertEqual(mo.integer('2 / 1'), 'mi:2 / 1')

    def test_truncation_note(self):
        record = normalized('truncated.mp4')
        self.assertTrue(record['probe_ok'])
        self.assertTrue(record['truncated'])
        self.assertTrue(record['conformance_errors'])
        self.assertFalse(normalized('basic.mp4')['truncated'])

    def test_failures(self):
        self.assertEqual(normalized('unrecognized')['failure'], 'unrecognized')
        self.assertFalse(normalized('unrecognized')['probe_ok'])
        empty = mo.normalize({'creatingLibrary': {}, 'media': None}, 'x')
        self.assertEqual((empty['probe_ok'], empty['failure']), (False, 'unrecognized'))
        self.assertEqual(mo.normalize(None, 'x', 'timeout')['failure'], 'timeout')
        self.assertEqual(mo.normalize([], 'x')['failure'], 'invalid-json')
        nested = normalized('nested_10000.mp4')
        self.assertTrue(nested['probe_ok'])
        self.assertEqual(nested['streams'], [])

    def test_pairing_by_stream_order(self):
        tracks = {'creatingLibrary': {'version': '26.10'}, 'media': {'track': [
            {'@type': 'General', 'Format': 'MPEG-4'},
            {'@type': 'Video', 'StreamOrder': '1', 'Format': 'AVC'},
            {'@type': 'Text', 'StreamOrder': '1-608', 'Format': 'EIA-608'},
            {'@type': 'Menu'},
            {'@type': 'Audio', 'StreamOrder': '0', 'Format': 'AAC'},
            {'@type': 'Audio', 'StreamOrder': '0', 'Format': 'AAC'},
        ]}}
        record = mo.normalize(tracks, 'x')
        self.assertEqual([s['index'] for s in record['streams']], [0, 1])
        self.assertEqual([t['reason'] for t in record['unpaired_tracks']],
                         ['no-integer-stream-order', 'no-integer-stream-order', 'duplicate-stream-order'])


class CompareTest(unittest.TestCase):
    def items(self, name):
        items, notes = mo.compare_file(name, oracle(name, 'ffprobe'), oracle(name, 'coreav'), normalized(name))
        return {(i['stream'], i['field']): i for i in items}, notes

    def test_mediainfo_sides_with_coreav_on_declared_durations(self):
        items, _ = self.items('vfr.mp4')
        self.assertEqual(items[(None, 'format_duration')]['class'], 'ffprobe-alone')
        self.assertEqual(items[(0, 'duration')]['class'], 'ffprobe-alone')
        # MediaInfo's Source_Duration (1.800) is the sample-table reading that ffprobe reports.
        self.assertEqual(items[(0, 'duration')]['source_duration_matches'], ['ffprobe'])
        items, _ = self.items('edit_list.mp4')
        self.assertEqual(items[(None, 'format_duration')]['class'], 'ffprobe-alone')
        self.assertEqual(items[(0, 'duration')]['class'], 'mediainfo-alone')

    def test_coreav_rejection_is_classified(self):
        items, notes = self.items('truncated.mp4')
        self.assertEqual(items[(None, 'probe_ok')]['class'], 'coreav-alone')
        self.assertEqual(items[(None, 'stream_count')]['class'], 'not-comparable')
        self.assertTrue(notes['mediainfo_truncated'])

    def test_vocabulary_only_applies_against_mediainfo(self):
        self.assertTrue(mo.equal('codec_name', 'pcm_s16le', 'pcm', True))
        self.assertFalse(mo.equal('codec_name', 'pcm_s16le', 'pcm_s16be', False))
        self.assertTrue(mo.equal('color_matrix', 'bt470bg', 'bt601', True))
        self.assertFalse(mo.equal('color_matrix', 'bt470bg', 'smpte170m', False))
        self.assertTrue(mo.equal('language', 'und', None, True))
        self.assertFalse(mo.equal('language', 'und', None, False))

    def test_tolerances(self):
        self.assertTrue(mo.equal('duration', Fraction(2023, 1000), Fraction(20232, 10000), True))
        self.assertFalse(mo.equal('duration', Fraction(2025, 1000), Fraction(2023, 1000), True))
        self.assertFalse(mo.equal('duration', Fraction(20232, 10000), Fraction(2023, 1000), False))
        self.assertTrue(mo.equal('display_aspect_ratio', Fraction(4, 3), Fraction(1333, 1000), True))
        self.assertFalse(mo.equal('display_aspect_ratio', Fraction(16, 9), Fraction(1333, 1000), True))

    def test_classes(self):
        f = 'width'
        self.assertEqual(mo.classify(f, 1, 1, 1), 'all-agree')
        self.assertEqual(mo.classify(f, 1, 1, 2), 'mediainfo-alone')
        self.assertEqual(mo.classify(f, 1, 2, 1), 'ffprobe-alone')
        self.assertEqual(mo.classify(f, 2, 1, 1), 'coreav-alone')
        self.assertEqual(mo.classify(f, 1, 2, 3), 'all-differ')
        self.assertEqual(mo.classify(f, 1, 2, mo.NA), 'not-comparable')
        d = 'duration'
        self.assertEqual(mo.classify(d, Fraction(20004, 10000), Fraction(19996, 10000), Fraction(2)),
                         'mediainfo-between')

    def test_summary_is_aggregate_only(self):
        names = ['vfr.mp4', 'edit_list.mp4', 'truncated.mp4', 'timecode.mov', 'multitrack.mp4']
        items, notes = mo.three_way(names, lambda n: (oracle(n, 'ffprobe'), oracle(n, 'coreav'), normalized(n)))
        summary = mo.summarize(items, notes)
        self.assertEqual(summary['files'], 5)
        self.assertEqual(summary['triage']['format_duration'], {'ffprobe-alone': 2})
        self.assertEqual(summary['triage']['probe_ok'], {'coreav-alone': 1})
        self.assertEqual(summary['coreav_rejects_ffprobe_reads']['mediainfo_flags_truncated'], 1)
        text = json.dumps(summary) + mo.markdown(summary, 't')
        for name in names:
            self.assertNotIn(name, text)


class ReviewRegressionTest(unittest.TestCase):
    def stream_record(self, ticks, den):
        record = oracle('vfr.mp4', 'ffprobe')
        record = json.loads(json.dumps(record))
        record['streams'][0].update({'duration_ts': ticks, 'time_base': [1, den], 'timescale': den})
        return record

    def test_coreav_and_ffprobe_compare_duration_ticks_exactly(self):
        # Same two seconds in different time bases: the stream oracle compares ticks.
        ffprobe = self.stream_record(25600, 12800)
        coreav = self.stream_record(256000, 128000)
        items, _ = mo.compare_file('x', ffprobe, coreav, normalized('vfr.mp4'))
        duration = next(i for i in items if i['stream'] == 0 and i['field'] == 'duration')
        self.assertEqual(duration['pairs']['coreav_vs_ffprobe'], False)

    def test_equal_ticks_in_different_time_bases_are_not_all_agree(self):
        c = (25600, Fraction(2))   # 25600 ticks at 1/12800
        f = (25600, Fraction(1))   # 25600 ticks at 1/25600
        self.assertEqual(mo.classify('duration', c, f, Fraction(2)), 'mediainfo-split')
        self.assertEqual(mo.pairwise('duration', c, f, Fraction(2))['mediainfo_vs_ffprobe'], False)

    def test_missing_mediainfo_keeps_coreav_ffprobe_agreement(self):
        name = 'multitrack.mp4'
        failed = mo.normalize(None, name, 'timeout')
        items, notes = mo.compare_file(name, oracle(name, 'ffprobe'), oracle(name, 'coreav'), failed)
        summary = mo.summarize(items, [{'file': name, **notes}])
        self.assertEqual(summary['two_way']['width']['coreav_vs_ffprobe'], [5, 5])
        self.assertEqual(summary['two_way']['width']['mediainfo_vs_coreav'], [0, 0])
        self.assertEqual(summary['triage']['width'], {})
        self.assertEqual(summary['triage']['probe_ok'], {})

    def test_tables_apply_at_report_time(self):
        record = normalized('timecode.mov')
        record['streams'][1]['codec_name'] = 'mi:FLAC'
        record['streams'][0]['color_matrix'] = 'mi:BT.709'
        record['streams'][1]['kind'] = 'mi:Audio'
        view = mo.view_mediainfo(record)
        self.assertEqual(view['streams'][0]['color_matrix'], 'bt709')
        self.assertEqual(view['streams'][1]['kind'], 'audio')
        self.assertEqual(mo.remap('codec_name', 'mi:MPEG Audio Layer 2'), 'mp2')
        self.assertEqual(mo.remap('codec_name', 'mi:Nonexistent'), 'mi:Nonexistent')

    def test_language_twins_and_h273_names(self):
        self.assertTrue(mo.equal('language', 'ger', 'deu', True))
        self.assertFalse(mo.equal('language', 'ger', 'deu', False))
        self.assertEqual(mo.remap('color_transfer', 'mi:sRGB/sYCC'), 'iec61966-2-1')
        self.assertEqual(mo.remap('color_matrix', 'mi:BT.470 System B/G'), 'bt601')
        self.assertTrue(mo.equal('color_matrix', 'bt470bg', mo.remap('color_matrix', 'mi:BT.470 System B/G'), True))

    def test_provenance_blocks_mixed_binaries(self):
        with tempfile.TemporaryDirectory() as tmp:
            first = {'binaries': {'mediainfo': 'a'}, 'tool': {'git': '1'}}
            self.assertTrue(corpus.check_provenance(tmp, first))
            self.assertTrue(corpus.check_provenance(tmp, {**first, 'tool': {'git': '2'}}))
            self.assertFalse(corpus.check_provenance(tmp, {**first, 'binaries': {'mediainfo': 'b'}}))


class CommandTest(unittest.TestCase):
    def test_report_reads_nested_captures(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            name = 'sub/vfr.mp4'
            for side in ('ffprobe', 'coreav'):
                (tmp / side / 'sub').mkdir(parents=True)
                (tmp / side / f'{name}.streams.json').write_text(json.dumps(oracle('vfr.mp4', side)))
            (tmp / 'mi' / 'sub').mkdir(parents=True)
            (tmp / 'mi' / f'{name}.mediainfo.json').write_text(json.dumps(normalized('vfr.mp4')))
            self.assertEqual(mo.main(['report', '--ffprobe', str(tmp / 'ffprobe'), '--coreav', str(tmp / 'coreav'),
                                      '--mediainfo', str(tmp / 'mi'), '--out', str(tmp / 'r.json')]), 0)
            report = json.loads((tmp / 'r.json').read_text())
            self.assertEqual(report['summary']['probe_outcomes'], {'coreav ok, ffprobe ok, mediainfo ok': 1})
            self.assertEqual(report['summary']['triage']['format_duration'], {'ffprobe-alone': 1})

    def test_report_command(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            for side in ('ffprobe', 'coreav'):
                (tmp / side).mkdir()
            (tmp / 'mi').mkdir()
            for name in ('vfr.mp4', 'edit_list.mp4'):
                for side in ('ffprobe', 'coreav'):
                    (tmp / side / f'{name}.streams.json').write_text(json.dumps(oracle(name, side)))
                (tmp / 'mi' / f'{name}.mediainfo.json').write_text(json.dumps(normalized(name)))
            done = subprocess.run([sys.executable, '-I', str(TOOL / 'mediainfo_oracle.py'), 'report',
                                   '--ffprobe', str(tmp / 'ffprobe'), '--coreav', str(tmp / 'coreav'),
                                   '--mediainfo', str(tmp / 'mi'), '--out', str(tmp / 'r.json'),
                                   '--markdown', str(tmp / 'r.md')], capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            report = json.loads((tmp / 'r.json').read_text())
            self.assertFalse(report['gating'])
            self.assertIn('edit_list.mp4', (tmp / 'r.md').read_text())

    def test_corpus_selection_never_mixes_holdout(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = pathlib.Path(tmp) / 'm.json'
            manifest.write_text(json.dumps({'files': [
                {'set': 'real', 'holdout': True, 'sha256': 'a'}, {'set': 'real', 'holdout': False, 'sha256': 'b'},
                {'set': 'edge', 'holdout': False, 'sha256': 'c'}, {'set': 'hostile', 'holdout': False, 'sha256': 'd'},
            ]}))
            pick = lambda split: [f['sha256'] for f in corpus.select(manifest, split, ['real', 'edge'])]  # noqa: E731
            self.assertEqual(pick('tuning'), ['b', 'c'])
            self.assertEqual(pick('holdout'), ['a'])
            self.assertEqual(pick('all'), ['a', 'b', 'c'])


if __name__ == '__main__':
    unittest.main()
