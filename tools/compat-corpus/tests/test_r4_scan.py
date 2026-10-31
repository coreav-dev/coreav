"""r4_scan.py against stand-in programs: which R4 pairs write to the fallback log, grouped by cause. No FFmpeg,
no media."""
import json
import os
import pathlib
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import release_round  # noqa: E402

# ffprobe lists one video and one audio stream; ffmpeg writes its output and fails on WebM; coreav refuses a
# command that seeks (an option), refuses every ffmpeg command on WebM (the file), and crashes on
# -show_streams of a transport stream with no line in its log.
FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
name, args = pathlib.Path(sys.argv[0]).name, sys.argv[1:]
assert os.environ['GCONV_PATH'] == '/nonexistent/gconv' and os.environ['COREAV_STRICT_SAFE'] == '1'
def refuse(reason):
    with open(os.environ['COREAV_FALLBACK_LOG'], 'a') as f:
        f.write(json.dumps({'reason': reason, 'ran': None, 'not_run': 'strict_safe'}) + '\n')
    sys.exit(4)
webm = any('.webm' in a for a in args)
if name == 'coreav':
    tool, args = args[0], args[1:]
    if '-ss' in args:
        refuse('-ss')
    if tool == 'ffmpeg' and webm:
        refuse('what the input needs')
    if tool == 'ffprobe' and '-show_streams' in args and any(a.endswith('.ts') for a in args):
        sys.exit(101)
    sys.exit(0)
if name == 'ffprobe' and '-show_streams' in args and 'json' in args:
    print(json.dumps({'streams': [{'index': 0, 'codec_type': 'video'}, {'index': 1, 'codec_type': 'audio'}]}))
if name == 'ffmpeg':
    if webm:
        sys.exit(1)
    pathlib.Path(args[-1].replace('%03d', '000')).write_bytes(b'x')
'''


class Scan(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        d = cls.d = pathlib.Path(cls.tmp.name)
        cls.bin = d / 'bin'
        cls.bin.mkdir()
        for name in ('coreav', 'ffmpeg', 'ffprobe'):
            (cls.bin / name).write_text(FAKE)
            (cls.bin / name).chmod(0o755 | stat.S_IEXEC)
        media = d / 'media'
        media.mkdir()
        listing = []
        for name, fam in (('a.mp4', 'mp4-mov'), ('b.webm', 'matroska-webm'), ('c.ts', 'mpegts')):
            (media / name).write_bytes(b'')
            listing.append({'family': fam, 'path': str(media / name)})
        cls.files = d / 'files.json'
        cls.files.write_text(json.dumps(listing))
        cls.code, cls.out, cls.records = cls.scan(cls.files)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    @classmethod
    def scan(cls, files):
        out, records = cls.d / 'scan.json', cls.d / 'scan.jsonl'
        for p in (out, records):
            if p.exists():
                p.unlink()
        done = subprocess.run([sys.executable, '-I', str(HERE / 'r4_scan.py'), '--coreav', str(cls.bin / 'coreav'),
                               '--ffmpeg', str(cls.bin / 'ffmpeg'), '--ffprobe', str(cls.bin / 'ffprobe'),
                               '--files', str(files), '--scratch', str(cls.d / 'scratch'), '--out', str(out),
                               '--records', str(records), '--jobs', '4'],
                              capture_output=True, text=True, timeout=600)
        if not out.exists():
            return done.returncode, done.stdout, None
        return done.returncode, json.loads(out.read_text()), records.read_text()

    def commands(self, test):
        return [f"{e['id']}.{i}" for e in release_round.entries() for i, c in enumerate(e['commands']) if test(e, c)]

    def test_a_refused_option_stops_every_pair(self):
        self.assertEqual(self.code, 1)
        seeks = [n for n in self.commands(lambda e, c: '-ss' in c['args']) if self.out['commands'][n]['pairs']]
        self.assertTrue(seeks)
        for name in seeks:
            c = self.out['commands'][name]
            self.assertEqual(list(c['logged']), ['option: -ss'])
            self.assertEqual(len(c['logged']['option: -ss']['files']), c['pairs'])
            self.assertTrue(c['logged']['option: -ss']['every pair'])

    def test_a_property_of_some_files_is_named_with_them(self):
        names = self.commands(lambda e, c: e['tool'] == 'ffmpeg' and '-ss' not in c['args']
                              and 'matroska-webm' in release_round.containers(e, c))
        logged = [self.out['commands'][n] for n in names if self.out['commands'][n]['pairs']]
        self.assertTrue(logged)
        for c in logged:
            group = c['logged']['property: what the input needs']
            self.assertEqual(group['files'], ['matroska-webm:0'])
            self.assertEqual(group['reference fails on'], ['matroska-webm:0'])
            self.assertEqual(group['every pair'], c['pairs'] == 1)
        self.assertTrue(any(c['pairs'] > 1 for c in logged))

    def test_a_failure_with_no_line_is_listed_apart(self):
        names = self.commands(lambda e, c: e['tool'] == 'ffprobe' and '-show_streams' in c['args']
                              and 'mpegts' in release_round.containers(e, c) and '-ss' not in c['args'])
        self.assertTrue(names)
        for n in names:
            self.assertIn({'file': 'mpegts:0', 'exit': 101, 'reference': 0}, self.out['commands'][n]['failed'])
            self.assertFalse(self.out['commands'][n]['logged'])

    def test_files_the_rule_excludes_are_named(self):
        without_audio = self.commands(lambda e, c: e['tool'] == 'ffmpeg' and 'a' in c.get('input_without', []))
        self.assertTrue(without_audio)
        for n in without_audio:
            c = self.out['commands'][n]
            self.assertEqual(c['pairs'], 0)
            self.assertTrue(c['excluded'])

    def test_only_the_records_name_the_files(self):
        self.assertNotIn('a.mp4', json.dumps(self.out))
        self.assertIn('a.mp4', self.records)
        self.assertEqual(self.out['files'], {'mp4-mov': 1, 'matroska-webm': 1, 'mpegts': 1})
        self.assertFalse(any((self.d / 'scratch').iterdir()))

    def test_no_pair_is_unmeasured(self):
        empty = self.d / 'empty.json'
        empty.write_text('[]')
        code, out, _ = self.scan(empty)
        self.assertEqual(code, release_round.UNMEASURED)
        self.assertIn('UNMEASURED', out)


if __name__ == '__main__':
    unittest.main()
