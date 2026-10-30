"""speed.py's R4 runner and report (docs/design/compat-cli.md, "The 0.1.0
bars"), run against stand-in programs: no FFmpeg, no media, no timing
claims."""
import json
import os
import pathlib
import random
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import release_round  # noqa: E402
import speed  # noqa: E402

# Records every call; `ffprobe -show_streams` lists one video and one audio
# stream; an ffmpeg command writes its output and fails if the file is
# there already; `coreav` refuses a command that seeks (`-ss`).
FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
name, args = pathlib.Path(sys.argv[0]).name, sys.argv[1:]
assert os.environ['GCONV_PATH'] == '/nonexistent/gconv' and os.environ['COREAV_STRICT_SAFE'] == '1'
with open(os.environ['FAKE_CALLS'], 'a') as f:
    f.write(json.dumps([name] + args) + '\n')
if name == 'coreav' and '-ss' in args:
    with open(os.environ['COREAV_FALLBACK_LOG'], 'a') as f:
        f.write(json.dumps({'ran': None, 'not_run': 'strict_safe'}) + '\n')
    sys.exit(4)
tool = args[0] if name == 'coreav' else name
if tool == 'ffprobe' and '-show_streams' in args and '-of' in args and 'json' in args:
    print(json.dumps({'streams': [{'index': 0, 'codec_type': 'video'}, {'index': 1, 'codec_type': 'audio'}]}))
if tool == 'ffmpeg':
    out = pathlib.Path(args[-1].replace('%03d', '000'))
    if out.exists():
        sys.exit(9)
    out.write_bytes(b'x')
'''


class Runner(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        d = pathlib.Path(cls.tmp.name)
        cls.bin = d / 'bin'
        cls.bin.mkdir()
        for name in ('coreav', 'ffmpeg', 'ffprobe'):
            (cls.bin / name).write_text(FAKE)
            (cls.bin / name).chmod(0o755 | stat.S_IEXEC)
        media = d / 'media'
        media.mkdir()
        for name in ('a.mp4', 'b.webm', 'c.ts'):
            (media / name).write_bytes(b'')
        cls.files = d / 'files.json'
        cls.files.write_text(json.dumps([{'family': 'mp4-mov', 'path': str(media / 'a.mp4')},
                                          {'family': 'matroska-webm', 'path': str(media / 'b.webm')},
                                          {'family': 'mpegts', 'path': str(media / 'c.ts')}]))
        cls.d = d
        cls.arms = {arm: cls.run_arm(arm) for arm in speed.ARMS}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    @classmethod
    def run_arm(cls, arm):
        calls, out = cls.d / f'{arm}.calls', cls.d / f'{arm}.json'
        cpu = sorted(os.sched_getaffinity(0))[0]
        subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4', '--arm', arm,
                        '--coreav', str(cls.bin / 'coreav'), '--ffmpeg', str(cls.bin / 'ffmpeg'),
                        '--ffprobe', str(cls.bin / 'ffprobe'), '--files', str(cls.files),
                        '--scratch', str(cls.d / 'scratch'), '--out', str(out), '--rounds', '1',
                        '--cpus', str(cpu)],
                       env={**os.environ, 'FAKE_CALLS': str(calls)}, check=True, capture_output=True, timeout=600)
        lines = [json.loads(line) for line in calls.read_text().splitlines()]
        return json.loads(out.read_text()), [c for c in lines if '-show_streams' not in c or len(c) > 7]

    def arm(self, arm):
        return self.arms[arm]

    def test_each_arm_times_its_sides_in_turn(self):
        for arm, sides in (('r4', ('coreav', 'ffmpeg')), ('aa', ('ffmpeg', 'ffmpeg')), ('null', ('coreav', 'coreav'))):
            data, calls = self.arm(arm)
            self.assertEqual(data['sides'], list(sides))
            timed = [r for r in data['results'] if 'ratio' in r]
            self.assertTrue(timed)
            for r in timed:
                self.assertEqual((len(r['a']), len(r['b'])), (1, 1))
            # Each pair: one untimed run of each side, then the rounds in turn.
            first = [c[0] for c in calls[:4]]
            self.assertEqual(first, [sides[0], sides[1]] * 2, arm)
            self.assertEqual(calls[0][1:] if sides[0] == 'coreav' else calls[0], calls[1][1:] if sides[1] == 'coreav' else calls[1])
            self.assertFalse(any((self.d / 'scratch' / 'out').iterdir()))

    def test_a_refusal_has_no_ratio_and_voids_the_run(self):
        data, _ = self.arm('r4')
        refused = [r for r in data['results'] if r.get('refused')]
        self.assertTrue(refused)
        self.assertTrue(all('ratio' not in r for r in refused))
        self.assertEqual(data['fallback_log'], {'lines': len(refused), 'refused': len(refused), 'void': True})
        self.assertFalse(self.arm('aa')[0]['fallback_log']['void'])

    def test_no_pair_is_unmeasured(self):
        empty = self.d / 'empty.json'
        empty.write_text('[]')
        done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4', '--arm', 'r4',
                               '--coreav', str(self.bin / 'coreav'), '--ffmpeg', str(self.bin / 'ffmpeg'),
                               '--ffprobe', str(self.bin / 'ffprobe'), '--files', str(empty),
                               '--scratch', str(self.d / 'scratch'), '--out', str(self.d / 'none.json'),
                               '--cpus', str(sorted(os.sched_getaffinity(0))[0])],
                              env={**os.environ, 'FAKE_CALLS': str(self.d / 'none.calls')},
                              capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, release_round.UNMEASURED)
        self.assertIn('UNMEASURED', done.stdout)
        self.assertFalse((self.d / 'none.json').exists())

    def test_commands_run_on_their_families_and_name_no_file(self):
        data, _ = self.arm('aa')
        families = {r['family'] for r in data['results'] if r['class'] in ('clip', 'join', 'split')}
        self.assertEqual(families, {'mp4-mov'})
        self.assertNotIn('a.mp4', json.dumps(data))
        self.assertEqual({r['file'] for r in data['results']}, {'mp4-mov:0', 'matroska-webm:0', 'mpegts:0'})


def results(arm, ratios):
    return {'arm': arm, 'sides': list(speed.ARMS[arm]), 'files': {}, 'fallback_log': {'void': False},
            'results': [{'class': c, 'family': f, 'id': 'C01', 'command': 0, 'ratio': x} for c, f, x in ratios]}


class Report(unittest.TestCase):
    def test_r4_meets_at_or_below_one(self):
        rows = [('remux', 'mp4-mov', 0.9)] * 10 + [('remux', 'matroska-webm', 1.1)] * 10
        s = speed.r4_summary(results('r4', rows), gate=False)
        by = {(r['class'], r['family']): r for r in s['classes']}
        self.assertTrue(by[('remux', 'mp4-mov')]['meets'])
        self.assertTrue(by[('remux', 'mp4-mov')]['faster'])
        self.assertEqual(by[('remux', 'mp4-mov')]['faster_share'], 1.0)
        self.assertFalse(by[('remux', 'matroska-webm')]['meets'])
        self.assertEqual(s['failing'], ['remux matroska-webm'])

    def test_the_gate(self):
        rng = random.Random(1)
        steady = [('probe', 'mpegts', 1 + rng.uniform(-0.01, 0.01)) for _ in range(40)]
        self.assertEqual(speed.r4_summary(results('aa', steady), gate=True)['failing'], [])
        biased = [('probe', 'mpegts', 1.03 + rng.uniform(-0.01, 0.01)) for _ in range(40)]
        self.assertEqual(speed.r4_summary(results('null', biased), gate=True)['failing'], ['probe all'])
        # A median inside the band whose interval misses 1.00 fails too.
        tight = [('probe', 'mpegts', 1.015 + rng.uniform(-0.001, 0.001)) for _ in range(40)]
        self.assertEqual(speed.r4_summary(results('null', tight), gate=True)['failing'], ['probe all'])

    def test_a_group_with_pairs_and_no_ratio_fails(self):
        rows = [('remux', 'mp4-mov', 0.9)] * 10
        data = results('r4', rows)
        data['results'].append({'class': 'remux', 'family': 'mpegts', 'id': 'C01', 'command': 0, 'exit': [1, 0]})
        data['results'].append({'class': 'clip', 'family': 'mpegts', 'id': 'C05', 'command': 0, 'exit': [1, 0]})
        s = speed.r4_summary(data, gate=False)
        self.assertEqual(s['unmeasured'], ['clip all', 'clip mpegts', 'remux mpegts'])
        self.assertEqual(s['failing'], s['unmeasured'])
        steady = [('remux', f, 1.0) for f in ('mp4-mov', 'mpegts') for _ in range(20)]
        gate = results('aa', steady)
        gate['results'].append({'class': 'remux', 'family': 'matroska-webm', 'id': 'C01', 'command': 0, 'exit': [1, 0]})
        self.assertEqual(speed.r4_summary(gate, gate=True)['failing'], [])
        gate['results'].append({'class': 'clip', 'family': 'mpegts', 'id': 'C05', 'command': 0, 'exit': [1, 0]})
        self.assertEqual(speed.r4_summary(gate, gate=True)['failing'], ['clip all'])

    def test_no_ratio_is_unmeasured(self):
        with tempfile.TemporaryDirectory() as tmp:
            for arm, gate in (('r4', []), ('aa', ['--gate'])):
                data = results(arm, [])
                data['results'] = [{'class': 'remux', 'family': 'mpegts', 'id': 'C01', 'command': 0, 'exit': [1, 0]}]
                path = pathlib.Path(tmp) / f'{arm}.json'
                path.write_text(json.dumps(data))
                done = subprocess.run([sys.executable, '-I', str(HERE / 'speed.py'), 'r4-report',
                                       '--results', str(path)] + gate, capture_output=True, text=True, timeout=60)
                self.assertEqual(done.returncode, release_round.UNMEASURED, arm)
                self.assertIn('UNMEASURED', done.stdout)

    def test_cpu_lists(self):
        self.assertEqual(speed.cpu_list('4-7'), {4, 5, 6, 7})
        self.assertEqual(speed.cpu_list('0,2-3'), {0, 2, 3})


if __name__ == '__main__':
    unittest.main()
