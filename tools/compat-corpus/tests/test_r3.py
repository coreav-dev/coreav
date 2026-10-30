"""r3.py and the R3 parts of release_round.py: which files a command can
take, and the counts of a report. No FFmpeg, no media."""
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import r3  # noqa: E402
import release_round  # noqa: E402

STREAMS = [{'index': 0, 'codec_type': 'video'},
           {'index': 1, 'codec_type': 'audio', 'tags': {'language': 'eng'}},
           {'index': 2, 'codec_type': 'audio', 'tags': {'language': 'fra'}},
           {'index': 3, 'codec_type': 'subtitle'}]
ENTRY = {'containers': ['mp4-mov']}


def command(*args, without=()):
    c = {'args': ['-i', 'INPUT', *args, 'OUTPUT']}
    if without:
        c['input_without'] = list(without)
    return c


class Selection(unittest.TestCase):
    def test_specifiers(self):
        self.assertEqual(release_round.selected('0', STREAMS), {0, 1, 2, 3})
        self.assertEqual(release_round.selected('0:2', STREAMS), {2})
        self.assertEqual(release_round.selected('0:7', STREAMS), set())
        self.assertEqual(release_round.selected('0:a', STREAMS), {1, 2})
        self.assertEqual(release_round.selected('0:a:1', STREAMS), {2})
        self.assertEqual(release_round.selected('0:a:3', STREAMS), set())
        self.assertEqual(release_round.selected('0:m:language:eng', STREAMS), {1})
        self.assertEqual(release_round.selected('0:d', STREAMS), set())

    def test_what_a_file_can_take(self):
        can = lambda c, streams=STREAMS, fam='mp4-mov': release_round.can_take(ENTRY, c, fam, streams)  # noqa: E731
        self.assertTrue(can(command('-c', 'copy')))
        self.assertFalse(can(command('-c', 'copy'), fam='mpegts'))
        self.assertFalse(can(command('-c', 'copy'), streams=[]))
        self.assertFalse(can(command('-c', 'copy', without='s')))
        self.assertTrue(can(command('-c', 'copy', without='d')))
        self.assertFalse(can(command('-map', '0:a:3', '-c', 'copy')))
        self.assertTrue(can(command('-map', '0', '-map', '-0:a:3', '-c', 'copy')))
        self.assertFalse(can(command('-map', '0:a', '-map', '-0:m:language:eng', '-map', '-0:m:language:fra')))
        self.assertTrue(can(command('-map', '0:v', '-map', '0:m:language:eng')))

    def test_every_command_of_the_set_reads_its_specifiers(self):
        for entry, _, c in release_round.commands('ffmpeg'):
            for fam in entry['containers']:
                release_round.can_take(entry, c, fam, STREAMS)

    def test_a_concat_list_names_the_file_twice(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = pathlib.Path(tmp) / 'a b.mp4'
            source.write_bytes(b'x')
            listing = release_round.concat_list(pathlib.Path(tmp) / 'list', source)
            self.assertEqual(listing.read_text(), "file 'a b.mp4'\nfile 'a b.mp4'\n")
            self.assertEqual((listing.parent / 'a b.mp4').read_bytes(), b'x')


class Report(unittest.TestCase):
    def test_counts_only_what_ffmpeg_ran(self):
        line = lambda f, cmd, outcome, reason=None, fam='mp4-mov': {  # noqa: E731
            'file': f, 'family': fam, 'id': cmd.split('.')[0], 'command': int(cmd.split('.')[1]),
            'job': 'remux', 'outcome': outcome, 'reason': reason}
        lines = [line(f'f{i}', 'C01.0', 'passed') for i in range(49)]
        lines += [line('g', 'C01.0', 'failed', 'refused'), line('h', 'C01.0', 'ffmpeg fails')]
        lines += [line('f1', 'C01.1', 'passed'), line('f2', 'C01.1', 'failed', 'oracle', 'matroska-webm')]
        lines += [line('f1', 'C02.0', 'cannot take'), line('f2', 'C02.0', 'ffmpeg fails')]
        lines += [{'fallback_log': {'lines': 1, 'refused': 1, 'void': False}}]
        s = r3.summarize(lines, 98.0)
        totals = {t['command']: t for t in s['commands']}
        self.assertEqual((totals['C01.0']['counted'], totals['C01.0']['passed'], totals['C01.0']['meets']),
                         (50, 49, True))
        self.assertFalse(totals['C01.1']['meets'])
        self.assertIsNone(totals['C02.0']['meets'])
        self.assertEqual(s['missed'], ['C01.1'])
        self.assertEqual(s['taken_by_no_file'], ['C02.0'])
        self.assertEqual(s['files'], {'matroska-webm': 1, 'mp4-mov': 51})
        self.assertNotIn('f1', str(s))
        self.assertEqual(s['fallback_log'], {'lines': 1, 'refused': 1, 'void': False})

    def test_the_binarys_exit_statuses(self):
        self.assertEqual([r3.binary_outcome(c) for c in (0, 4, 1, None)], ['written', 'refused', 'failed', 'failed'])


if __name__ == '__main__':
    unittest.main()
