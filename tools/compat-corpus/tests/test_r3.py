"""r3.py and the R3 parts of release_round.py: which files a command can
take, and the counts of a report. No FFmpeg, no media."""
import argparse
import contextlib
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

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
    def test_every_command_is_reported_even_without_a_file_of_its_families(self):
        summary = r3.summarize(r3.counts([]), 98.0)
        expected = sorted(f"{e['id']}.{n}" for e, n, _ in release_round.commands('ffmpeg'))
        self.assertEqual([t['command'] for t in summary['commands']], expected)
        self.assertEqual(summary['taken_by_no_file'], expected)
        self.assertEqual(summary['unmeasured'], [])
        self.assertEqual(summary['missed'], [])
        self.assertTrue(all(t['counted'] == 0 and t['meets'] is None for t in summary['commands']))
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(r3.verdict(summary), release_round.UNMEASURED)

    def test_counts_only_what_ffmpeg_ran(self):
        line = lambda f, cmd, outcome, reason=None, fam='mp4-mov': {  # noqa: E731
            'file': f, 'family': fam, 'id': cmd.split('.')[0], 'command': int(cmd.split('.')[1]),
            'job': 'remux', 'outcome': outcome, 'reason': reason}
        lines = [line(f'f{i}', 'C01.0', 'passed') for i in range(49)]
        lines += [line('g', 'C01.0', 'failed', 'refused'), line('h', 'C01.0', 'ffmpeg fails')]
        lines += [line('f1', 'C01.1', 'passed'), line('f2', 'C01.1', 'failed', 'oracle', 'matroska-webm')]
        lines += [line('f1', 'C02.0', 'cannot take'), line('f2', 'C02.0', 'ffmpeg fails')]
        lines += [{'fallback_log': {'lines': 1, 'refused': 1, 'void': True}}]
        s = r3.summarize(r3.counts(lines), 98.0)
        totals = {t['command']: t for t in s['commands']}
        self.assertEqual((totals['C01.0']['counted'], totals['C01.0']['passed'], totals['C01.0']['meets']),
                         (50, 49, True))
        self.assertFalse(totals['C01.1']['meets'])
        self.assertIsNone(totals['C02.0']['meets'])
        self.assertEqual(s['missed'], ['C01.1'])
        expected = {f"{e['id']}.{n}" for e, n, _ in release_round.commands('ffmpeg')}
        self.assertEqual(s['taken_by_no_file'], sorted(expected - {'C01.0', 'C01.1', 'C02.0'}))
        self.assertEqual(s['unmeasured'], ['C02.0'])
        self.assertEqual(s['files'], {'matroska-webm': 1, 'mp4-mov': 51})
        self.assertNotIn('f1', str(s))
        self.assertEqual(s['fallback_log'], {'lines': 1, 'refused': 1, 'void': True})

    def test_a_command_ffmpeg_fails_on_every_taker_misses(self):
        line = lambda f, cmd, outcome: {  # noqa: E731
            'file': f, 'family': 'mp4-mov', 'id': cmd.split('.')[0], 'command': int(cmd.split('.')[1]),
            'job': 'remux', 'outcome': outcome, 'reason': None}
        lines = [line('f1', 'C01.0', 'passed'), line('f1', 'C02.0', 'ffmpeg fails'),
                 line('f2', 'C02.0', 'cannot take'), line('f1', 'C03.0', 'cannot take'),
                 line('f1', 'C04.0', 'ffprobe fails'),
                 {'fallback_log': {'lines': 0, 'refused': 0, 'void': False}}]
        s = r3.summarize(r3.counts(lines), 98.0)
        self.assertEqual(s['unmeasured'], ['C02.0', 'C04.0'])
        expected = {f"{e['id']}.{n}" for e, n, _ in release_round.commands('ffmpeg')}
        self.assertEqual(s['taken_by_no_file'], sorted(expected - {'C01.0', 'C02.0', 'C04.0'}))
        self.assertEqual(s['missed'], [])
        with tempfile.TemporaryDirectory() as tmp:
            records = pathlib.Path(tmp) / 'r.jsonl'
            for kept, code in ((lines, 1), ([x for x in lines if x.get('id') not in ('C02', 'C04')], 0)):
                records.write_text(''.join(json.dumps(x) + '\n' for x in kept))
                done = subprocess.run([sys.executable, '-I', str(HERE / 'r3.py'), 'report', '--records', str(records)],
                                      capture_output=True, text=True, timeout=60)
                self.assertEqual(done.returncode, code, done.stdout)

    def test_summaries_add_up(self):
        line = lambda f, cmd, outcome, reason=None: {  # noqa: E731
            'file': f, 'family': 'mp4-mov', 'id': cmd.split('.')[0], 'command': int(cmd.split('.')[1]),
            'job': 'remux', 'outcome': outcome, 'reason': reason}
        lines = [line(f'f{i}', 'C01.0', 'passed' if i % 9 else 'failed', None if i % 9 else 'oracle')
                 for i in range(30)]
        lines += [{'fallback_log': {'lines': 0, 'refused': 0, 'void': False}}]
        whole = r3.summarize(r3.counts(lines), 98.0)
        split = r3.summarize(r3.merge([r3.counts(lines[:10]), r3.counts(lines[10:])]), 98.0)
        self.assertEqual(split, whole)

    def test_nothing_counted_is_unmeasured(self):
        with tempfile.TemporaryDirectory() as tmp:
            records = pathlib.Path(tmp) / 'r.jsonl'
            records.write_text(''.join(json.dumps(x) + '\n' for x in (
                {'file': 'f', 'family': 'mp4-mov', 'id': 'C01', 'command': 0, 'job': 'remux',
                 'outcome': 'ffmpeg fails', 'reason': None},
                {'fallback_log': {'lines': 0, 'refused': 0, 'void': False}})))
            empty = pathlib.Path(tmp) / 'media'
            empty.mkdir()
            for argv in (['report'], ['report', '--records', str(records)],
                         ['run', '--coreav', 'x', '--harness', 'x', '--ffmpeg', 'x', '--ffprobe', 'x',
                          '--media', str(empty), '--work', str(pathlib.Path(tmp) / 'w'),
                          '--records', str(pathlib.Path(tmp) / 'out.jsonl')]):
                done = subprocess.run([sys.executable, '-I', str(HERE / 'r3.py')] + argv,
                                      capture_output=True, text=True, timeout=60)
                self.assertEqual(done.returncode, release_round.UNMEASURED, argv)
                self.assertIn('UNMEASURED', done.stdout)

    def test_the_binarys_exit_statuses(self):
        self.assertEqual([r3.binary_outcome(c) for c in (0, 4, 1, None)], ['written', 'refused', 'failed', 'failed'])


# Stand-ins for the harness, the binary and copy_check.py. The harness writes
# `out.mp4` for every pair (or refuses every one with FAKE_STATUS=refused); the
# binary does the same, writing a fallback-log line when it refuses; the check
# writes a fresh report, or with FAKE_CHECK=crash exits 1 and writes none, or
# with FAKE_CHECK=slow takes 3 seconds first.
HARNESS = r'''#!/usr/bin/env python3
import json, os, pathlib
status = os.environ.get('FAKE_STATUS', 'written')
records = []
for p in json.load(open(os.environ['COREAV_R3_PAIRS'])):
    pathlib.Path(p['work']).mkdir(parents=True, exist_ok=True)
    files = []
    if status == 'written':
        pathlib.Path(p['output']).write_bytes(b'x')
        files = [p['output']]
    records.append({'id': p['id'], 'command': p['command'], 'job': p['job'], 'source': p['source'],
                    'input': p['input'], 'work': p['work'], 'status': status, 'files': files, 'detail': {}})
json.dump(records, open(os.environ['COREAV_R3_RECORDS'], 'w'))
'''
BINARY = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
if os.environ.get('FAKE_STATUS') == 'refused':
    with open(os.environ['COREAV_FALLBACK_LOG'], 'a') as f:
        f.write(json.dumps({'reason': 'a codec that CoreAV does not copy', 'ran': None, 'not_run': 'strict_safe'}) + '\n')
    sys.exit(4)
pathlib.Path(sys.argv[-1]).write_bytes(b'x')
'''
CHECK = r'''#!/usr/bin/env python3
import collections, json, os, sys
args = dict(zip(sys.argv[1::2], sys.argv[2::2]))
if os.environ.get('FAKE_CHECK') == 'crash':
    sys.exit(1)
if os.environ.get('FAKE_CHECK') == 'slow':
    import time
    time.sleep(3)
counts = collections.defaultdict(collections.Counter)
for r in json.load(open(args['--records'])):
    counts[r['id']][r['status']] += 1
json.dump({'counts': counts, 'failures': []}, open(args['--report'], 'w'))
'''


class Judge(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        d = self.d = pathlib.Path(self.tmp.name)
        for name, text in (('harness', HARNESS), ('coreav', BINARY), ('copy_check.py', CHECK)):
            (d / name).write_text(text)
            (d / name).chmod(0o755)
        (d / 'src.mp4').write_bytes(b'')
        self.log = d / 'fallback.log'
        self.opts = argparse.Namespace(harness=str(d / 'harness'), coreav=str(d / 'coreav'), ffmpeg='ffmpeg',
                                       ffprobe='ffprobe', mediainfo=None, log=str(d / 'check.log'))
        here, r3.HERE = r3.HERE, d
        self.addCleanup(setattr, r3, 'HERE', here)

    def tearDown(self):
        self.tmp.cleanup()

    def pairs(self, ids):
        return [{'id': i, 'job': 'remux', 'command': 0, 'args': ['-i', 'INPUT', '-c', 'copy', 'OUTPUT'],
                 'source': str(self.d / 'src.mp4'), 'family': 'mp4-mov', 'input': str(self.d / 'src.mp4')}
                for i in ids]

    def judge(self, ids, **env):
        scratch = self.d / 'scratch'
        scratch.mkdir(exist_ok=True)
        return r3.judge(self.opts, self.pairs(ids), scratch, {**release_round.environment(self.log), **env})

    def test_a_crashed_check_fails_its_chunk_and_never_reads_an_old_report(self):
        self.assertEqual(self.judge(['C01']), {('C01', 0): ('passed', None)})
        self.assertEqual(self.judge(['C02'], FAKE_CHECK='crash'), {('C02', 0): ('failed', 'copy_check crashed')})

    def test_a_slow_check_is_not_cut_short(self):
        timeout, r3.TIMEOUT = r3.TIMEOUT, 1
        self.addCleanup(setattr, r3, 'TIMEOUT', timeout)
        self.assertEqual(self.judge(['C01'], FAKE_CHECK='slow'), {('C01', 0): ('passed', None)})

    def test_a_refusal_runs_the_binary_and_reaches_the_fallback_log(self):
        self.assertEqual(self.judge(['C01'], FAKE_STATUS='refused'), {('C01', 0): ('failed', 'refused')})
        self.assertEqual(release_round.fallback_verdict(self.log), {'lines': 1, 'refused': 1, 'void': True})

    def test_a_pair_the_check_sets_aside_is_not_passed(self):
        source = self.d / 'size_inband_sps.mp4'
        source.write_bytes(b'')
        scratch = self.d / 'scratch'
        scratch.mkdir()
        pairs = [{**p, 'job': 'clip', 'source': str(source), 'input': str(source)} for p in self.pairs(['C07'])]
        self.assertEqual(r3.judge(self.opts, pairs, scratch, release_round.environment(self.log)),
                         {('C07', 0): ('set aside', 'no oracle models a picture size change')})


class Run(unittest.TestCase):
    def run_lines(self, lines, log_line=None, stale_line=None):
        with tempfile.TemporaryDirectory() as tmp:
            d = pathlib.Path(tmp)
            (d / 'a.mp4').write_bytes(b'')
            if stale_line:
                (d / 'w').mkdir()
                (d / 'w' / 'fallback.log').write_text(stale_line + '\n')

            def one_file(opts, path, env):
                if log_line:
                    pathlib.Path(env['COREAV_FALLBACK_LOG']).write_text(log_line + '\n')
                return lines

            opts = argparse.Namespace(list=None, media=[str(d)], work=str(d / 'w'), records=str(d / 'r.jsonl'),
                                      jobs=1, bar=98.0)
            original, r3.one_file = r3.one_file, one_file
            try:
                with contextlib.redirect_stdout(io.StringIO()):
                    return r3.cmd_run(opts)
            finally:
                r3.one_file = original

    def line(self, outcome, reason=None):
        return {'file': 'a.mp4', 'family': 'mp4-mov', 'id': 'C01', 'command': 0, 'job': 'remux',
                'outcome': outcome, 'reason': reason}

    def test_run_exits_as_its_report(self):
        self.assertEqual(self.run_lines([self.line('ffprobe fails')]), release_round.UNMEASURED)
        self.assertEqual(self.run_lines([self.line('passed')]), 0)
        self.assertEqual(self.run_lines([self.line('passed')], json.dumps({'not_run': 'strict_safe'})), 2)
        self.assertEqual(self.run_lines([self.line('failed', 'oracle')]), 1)

    def test_a_log_left_by_an_earlier_run_does_not_void_the_run(self):
        self.assertEqual(self.run_lines([self.line('passed')], stale_line=json.dumps({'not_run': 'strict_safe'})), 0)

    def test_a_set_aside_pair_leaves_its_command_unmeasured(self):
        lines = [self.line('passed')] * 99 + [self.line('set aside', 'no oracle models a picture size change')]
        summary = r3.summarize(r3.counts(lines), 98.0)
        self.assertEqual(summary['unmeasured'], ['C01.0'])
        self.assertEqual(summary['commands'][0]['counted'], 99)
        self.assertEqual(self.run_lines(lines), 1)


if __name__ == '__main__':
    unittest.main()
