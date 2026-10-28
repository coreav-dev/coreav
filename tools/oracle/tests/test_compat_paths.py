"""scripts/compat_check.py counts a compat key as covered only where an
overlay entry covered the field it follows, on the same file and stream,
and only when the compat value is the value the entry was checked on, or
its documented conversion (#219)."""
import importlib.util
import json
import pathlib
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('compat_check', ROOT / 'scripts' / 'compat_check.py')
compat = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compat)


def checked(kd, candidate, reference=None):
    return {'kd': [kd], 'candidate': candidate, 'reference': reference}


def overlay(items):
    return {'levels': {'streams': {'all': {'covered_items': items}}}}


class Paths(unittest.TestCase):
    def covered(self, items):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'kd.json'
            path.write_text(json.dumps(overlay(items)))
            return compat.covered_by_file(path)

    def edit(self, index=1, ticks=836_160):
        return self.covered([{'file': 'a.mp4', 'kd': ['KD-EDIT-DURATION'],
                              'paths': {f'streams/{index}/duration_ts': checked('KD-EDIT-DURATION', ticks)}}])

    def test_a_stream_field_covers_its_keys_on_that_stream_only(self):
        c = self.edit()
        stream = {'time_base': '1/48000'}
        self.assertTrue(compat.explained(c, 'a.mp4', 'streams[1].duration_ts', 836_160, stream, None, stream))
        self.assertTrue(compat.explained(c, 'a.mp4', 'streams[1].duration', '17.420000', stream, None, stream))
        # Not another stream, another key, the format, or another file.
        self.assertFalse(compat.explained(c, 'a.mp4', 'streams[0].duration', '17.420000', stream))
        self.assertFalse(compat.explained(c, 'a.mp4', 'streams[1].width', 640, stream))
        self.assertFalse(compat.explained(c, 'a.mp4', 'format.duration', '17.420000', stream))
        self.assertFalse(compat.explained(c, 'b.mp4', 'streams[1].duration', '17.420000', stream))

    def test_a_stream_value_other_than_the_checked_one_is_not_covered(self):
        c = self.edit()
        stream = {'time_base': '1/48000'}
        self.assertFalse(compat.explained(c, 'a.mp4', 'streams[1].duration_ts', 836_161, stream))
        self.assertFalse(compat.explained(c, 'a.mp4', 'streams[1].duration', '17.420021', stream, None, stream))
        # The duration follows ffprobe's time base, not the compat one.
        other = {'time_base': '1/44100'}
        self.assertFalse(compat.explained(c, 'a.mp4', 'streams[1].duration', '17.420000', stream, None, other))

    def format_covered(self, us=8_000_000):
        return self.covered([{'file': 'a.mp4', 'kd': ['KD-EDIT-DURATION'],
                              'paths': {'format_duration_us': checked('KD-EDIT-DURATION', us)}}])

    def test_the_format_duration_covers_the_format_duration_and_bit_rate(self):
        c = self.format_covered()
        fmt = {'size': '10000'}
        self.assertTrue(compat.explained(c, 'a.mp4', 'format.duration', '8.000000', None, fmt))
        self.assertTrue(compat.explained(c, 'a.mp4', 'format.bit_rate', '10000', None, fmt))
        self.assertFalse(compat.explained(c, 'a.mp4', 'format.nb_streams', 2, None, fmt))

    def test_a_format_value_other_than_the_checked_one_is_not_covered(self):
        c = self.format_covered()
        fmt = {'size': '10000'}
        # The review's case: 10,000 bytes over 8 s is 10,000 bit/s, not 1.
        self.assertFalse(compat.explained(c, 'a.mp4', 'format.bit_rate', '1', None, fmt))
        self.assertFalse(compat.explained(c, 'a.mp4', 'format.duration', '9.000000', None, fmt))
        self.assertFalse(compat.explained(c, 'a.mp4', 'format.bit_rate', '10000', None, {'size': '20000'}))

    def test_main_rejects_a_compat_bit_rate_the_covered_duration_does_not_give(self):
        # The review's reproducer through main(): ffprobe 10 s, CoreAV 8 s
        # (covered), size 10,000 bytes; CoreAV's bit rate 1 must stay a
        # difference, its correct 10,000 must not.
        with tempfile.TemporaryDirectory() as tmp:
            media = pathlib.Path(tmp) / 'media'
            media.mkdir()
            (media / 'sample.mp4').write_bytes(b'')
            kd_path = pathlib.Path(tmp) / 'kd.json'
            kd_path.write_text(json.dumps(overlay([{'file': 'sample.mp4', 'kd': ['KD-EDIT-DURATION'],
                                                    'paths': {'format_duration_us': checked(
                                                        'KD-EDIT-DURATION', 8_000_000, 10_000_000)}}])))
            theirs = {'streams': [], 'format': {'duration': '10.000000', 'size': '10000', 'bit_rate': '8000'}}

            def run(bit_rate):
                ours = {'streams': [], 'format': {'duration': '8.000000', 'size': '10000', 'bit_rate': bit_rate}}
                report = pathlib.Path(tmp) / f'report-{bit_rate}.json'
                outputs = iter([(0, theirs), (0, ours)])
                argv = ['compat_check.py', '--coreav', 'coreav', '--media', str(media), '--kd-overlay',
                        str(kd_path), '--min-file-rate', '0', '--report', str(report)]
                with mock.patch.object(compat, 'run_json', lambda *a, **k: next(outputs)), \
                        mock.patch('sys.argv', argv), mock.patch('sys.stdout'):
                    compat.main()
                return json.loads(report.read_text())

            self.assertEqual([d['key'] for d in run('1')['diffs']], ['format.bit_rate'])
            self.assertEqual(run('10000')['diffs'], [])

    def test_a_stream_entry_alone_does_not_cover_the_format(self):
        c = self.edit(index=0)
        self.assertFalse(compat.explained(c, 'a.mp4', 'format.duration', '17.420000', None, {'size': '1'}))

    def test_a_declared_size_covers_the_size_and_the_aspect_ratios_it_gives(self):
        c = self.covered([{'file': 'a.mkv', 'kd': ['KD-MKV-DECLARED-SIZE'],
                           'paths': {'streams/0/width': checked('KD-MKV-DECLARED-SIZE', 1920)}}])
        stream = {'width': 1920, 'height': 1080, 'sample_aspect_ratio': '1:1'}
        theirs = {'width': 1440, 'height': 1080, 'sample_aspect_ratio': '1:1'}
        self.assertTrue(compat.explained(c, 'a.mkv', 'streams[0].width', 1920, stream, None, theirs))
        self.assertTrue(compat.explained(c, 'a.mkv', 'streams[0].display_aspect_ratio', '16:9', stream, None, theirs))
        self.assertFalse(compat.explained(c, 'a.mkv', 'streams[0].width', 1921, stream, None, theirs))
        self.assertFalse(compat.explained(c, 'a.mkv', 'streams[0].display_aspect_ratio', '4:3', stream, None, theirs))
        self.assertFalse(compat.explained(c, 'a.mkv', 'streams[0].height', 1080, stream, None, theirs))

    def test_the_size_never_excuses_a_sample_aspect_ratio(self):
        # The re-review's case: an arbitrary sample ratio, and a display
        # ratio derived from it, under a covered Matroska size.
        c = self.covered([{'file': 'a.mkv', 'kd': ['KD-MKV-DECLARED-SIZE'],
                           'paths': {'streams/0/width': checked('KD-MKV-DECLARED-SIZE', 160),
                                     'streams/0/height': checked('KD-MKV-DECLARED-SIZE', 120)}}])
        ours = {'width': 160, 'height': 120, 'sample_aspect_ratio': '99:1'}
        theirs = {'width': 320, 'height': 240, 'sample_aspect_ratio': '1:1'}
        self.assertFalse(compat.explained(c, 'a.mkv', 'streams[0].sample_aspect_ratio', '99:1', ours, None, theirs))
        self.assertFalse(compat.explained(c, 'a.mkv', 'streams[0].display_aspect_ratio', '132:1', ours, None, theirs))
        # With the sample ratio ffprobe reports, the derived display ratio counts.
        ours['sample_aspect_ratio'] = '1:1'
        self.assertTrue(compat.explained(c, 'a.mkv', 'streams[0].display_aspect_ratio', '4:3', ours, None, theirs))

    def test_an_unmapped_field_covers_nothing(self):
        c = self.covered([{'file': 'a.mp4', 'kd': ['KD-QT-TEXT'],
                           'paths': {'streams/count': checked('KD-QT-TEXT', 1, 2)}}])
        self.assertEqual(c['a.mp4'][0], {})
        self.assertEqual(c['a.mp4'][1], {'KD-QT-TEXT'})

    def test_an_overlay_without_paths_or_values_is_refused(self):
        for item in ({'file': 'a.mp4', 'kd': ['KD-STTS-SHORT']},
                     {'file': 'a.mp4', 'kd': ['KD-STTS-SHORT'], 'paths': {'format_duration_us': ['KD-STTS-SHORT']}}):
            with self.assertRaises(SystemExit) as stopped:
                self.covered([item])
            self.assertEqual(stopped.exception.code, 2)


if __name__ == '__main__':
    unittest.main()
