"""run.py's 0.1.0 modes (R1, R2) and the shared parts of the round in
release_round.py, run against stand-in programs: no FFmpeg, no media."""
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
import run  # noqa: E402

# A stand-in for both tools: it prints its arguments, fails when the
# environment of the round is missing, and as `coreav` prints another value
# for the commands that name `stream=height` and writes a fallback-log line
# when FAKE_LOG_LINE is set.
FAKE = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
if os.environ.get('GCONV_PATH') != '/nonexistent/gconv' or os.environ.get('COREAV_STRICT_SAFE') != '1':
    sys.exit(3)
coreav = args[:1] == ['ffprobe']
if coreav:
    args = args[1:]
    if os.environ.get('FAKE_LOG_LINE'):
        with open(os.environ['COREAV_FALLBACK_LOG'], 'a') as f:
            f.write(os.environ['FAKE_LOG_LINE'] + '\n')
if os.environ.get('FAKE_FFPROBE_FAILS') and not coreav and any('.mkv' in a for a in args):
    sys.exit(1)
names = [a for a in args if not a.startswith('/')]
print(' '.join(names) + (' other' if coreav and any('stream=height' in a for a in args) else ''))
'''


class Shared(unittest.TestCase):
    def test_the_set_has_its_133_commands(self):
        self.assertEqual(len(release_round.commands('ffprobe')), 69)
        self.assertEqual(len(release_round.commands('ffmpeg')), 64)

    def test_family_by_extension(self):
        self.assertEqual(release_round.family('a/b.MOV'), 'mp4-mov')
        self.assertEqual(release_round.family('x.webm'), 'matroska-webm')
        self.assertEqual(release_round.family('x.m2ts'), 'mpegts')
        self.assertIsNone(release_round.family('x.m4b'))

    def test_a_forced_demuxer_narrows_the_families(self):
        forced = [(e, c) for e, _, c in release_round.commands('ffprobe') if 'input_container' in c]
        self.assertTrue(forced)
        for e, c in forced:
            self.assertEqual(release_round.containers(e, c), [c['input_container']])

    def test_the_keys_the_set_prints(self):
        p = release_round.key_patterns()
        for name in ('streams.codec_name', 'streams.tags.rotate', 'streams.tags.language',
                     'format.tags.major_brand', 'packets.pts', 'chapters.start_time'):
            self.assertTrue(release_round.in_set(name, p), name)
        for name in ('programs.program_id', 'programs.streams.codec_name', 'streamsx'):
            self.assertFalse(release_round.in_set(name, p), name)

    def test_the_environment(self):
        os.environ['COREAV_FALLBACK'] = '/usr/bin/ffprobe'
        try:
            env = release_round.environment('/tmp/log')
        finally:
            del os.environ['COREAV_FALLBACK']
        self.assertEqual(env['GCONV_PATH'], release_round.NO_GCONV)
        self.assertEqual(env['COREAV_STRICT_SAFE'], '1')
        self.assertEqual(env['COREAV_FALLBACK_LOG'], '/tmp/log')
        self.assertNotIn('COREAV_FALLBACK', env)

    def test_only_a_line_that_is_not_a_strict_safe_refusal_voids_a_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = pathlib.Path(tmp) / 'log'
            self.assertEqual(release_round.fallback_verdict(log), {'lines': 0, 'refused': 0, 'void': False})
            log.write_text(json.dumps({'ran': None, 'not_run': 'strict_safe'}) + '\n')
            self.assertFalse(release_round.fallback_verdict(log)['void'])
            for line in (json.dumps({'ran': '/usr/bin/ffprobe'}), json.dumps({'ran': None, 'not_run': 'no_program'}),
                         'not json'):
                log.write_text(json.dumps({'not_run': 'strict_safe'}) + '\n' + line + '\n')
                self.assertEqual(release_round.fallback_verdict(log), {'lines': 2, 'refused': 1, 'void': True})


def record(fam, keys, **extra):
    return {'file': 'f', 'family': fam, 'keys': keys, 'via_known_divergence': [], 'rejected': None, **extra}


class KeyRecord(unittest.TestCase):
    def test_only_the_sets_keys_are_kept(self):
        patterns = release_round.key_patterns()
        keys = {'streams.codec_name': True, 'programs.program_id': False, 'streams.tags.language': False}
        examples = {'programs.program_id': ('programs[0].program_id', 1, None),
                    'streams.tags.language': ('streams[0].tags.language', 'eng', None)}
        r = run.key_record(pathlib.Path('x/a.ts'), (keys, examples, False, None, {'programs.program_id'}), patterns)
        self.assertEqual(r['family'], 'mpegts')
        self.assertEqual(r['keys'], {'streams.codec_name': True, 'streams.tags.language': False})
        self.assertEqual(r['via_known_divergence'], [])
        self.assertEqual(list(r['examples']), ['streams.tags.language'])
        self.assertIsNone(r['rejected'])

    def test_rejections(self):
        patterns = release_round.key_patterns()
        self.assertEqual(run.key_record(pathlib.Path('a.mp4'), ({}, {}, True, None, set()), patterns)['rejected'],
                         'uncovered')
        self.assertEqual(run.key_record(pathlib.Path('a.mp4'), ({}, {}, False, run.STRICT, set()),
                                        patterns)['rejected'], run.STRICT)
        self.assertTrue(run.key_record(pathlib.Path('a.mp4'), None, patterns)['ffprobe_failed'])


class KeysReport(unittest.TestCase):
    def test_counting_rule(self):
        records = [record('mpegts', {'streams.codec_name': True, 'streams.width': True,
                                     'streams.tags.language': True, 'streams.tags.title': i > 0})
                   for i in range(4)]
        records += [record('mpegts', {'streams.codec_name': False})]
        records += [{'file': 'g', 'family': 'mpegts', 'ffprobe_failed': True}]
        out = run.keys_report(run.keys_counts(records), 98.0)['families']['mpegts']
        rows = {r['key']: r for r in out['keys']}
        self.assertEqual(out['files'], {'compared': 5, 'ffprobe failed': 1})
        self.assertEqual(rows['streams.codec_name'], {
            'key': 'streams.codec_name', 'files': 5, 'agree': 4, 'via_known_divergence': 0,
            'gated': True, 'rate': 80.0, 'below': True})
        self.assertFalse(rows['streams.width']['gated'])
        self.assertFalse(rows['streams.width']['below'])
        pooled = rows[run.TAGS_POOLED]
        self.assertEqual((pooled['keys'], pooled['occurrences'], pooled['agree']), (2, 8, 7))
        self.assertTrue(pooled['below'])
        self.assertEqual(out['below'], ['streams.codec_name', run.TAGS_POOLED])

    def test_a_key_is_below_on_its_unrounded_share(self):
        records = [record('mp4-mov', {'format.duration': i >= 21}) for i in range(1049)]
        row = run.keys_report(run.keys_counts(records), 98.0)['families']['mp4-mov']['keys'][0]
        self.assertEqual((row['agree'], row['rate'], row['below']), (1028, 98.0, True))

    def test_shards_merge_their_fallback_logs(self):
        records = [{'fallback_log': {'lines': 1, 'refused': 1, 'void': False}},
                   {'fallback_log': {'lines': 2, 'refused': 1, 'void': True}}]
        self.assertEqual(run.keys_counts(records)['fallback_log'], {'lines': 3, 'refused': 2, 'void': True})

    def test_summaries_add_up_to_the_report_of_all_files(self):
        records = [record('mp4-mov', {'format.duration': i % 7 != 0, 'format.tags.title': True}) for i in range(9)]
        records += [{'file': 'g', 'family': 'mp4-mov', 'ffprobe_failed': True}]
        whole = run.keys_report(run.keys_counts(records), 98.0)
        halves = run.merge_counts([run.keys_counts(records[:4]), run.keys_counts(records[4:])])
        split = run.keys_report(halves, 98.0)
        self.assertEqual(split, whole)
        self.assertNotIn("'file'", str(whole))


class Commands010(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        bin_dir = self.dir / 'bin'
        bin_dir.mkdir()
        for name in ('ffprobe', 'coreav'):
            path = bin_dir / name
            path.write_text(FAKE)
            path.chmod(path.stat().st_mode | stat.S_IEXEC)
        media = self.dir / 'media'
        media.mkdir()
        for name in ('a.mp4', 'b.mkv', 'c.ts', 'd.m4b'):
            (media / name).write_bytes(b'')
        (media / 'MANIFEST').write_text(''.join(f'generated {n}\n' for n in ('a.mp4', 'b.mkv', 'c.ts', 'd.m4b')))
        self.env = {**os.environ, 'PATH': f'{bin_dir}:{os.environ["PATH"]}'}
        self.media, self.coreav = media, bin_dir / 'coreav'

    def tearDown(self):
        self.tmp.cleanup()

    def r1(self, **env):
        report = self.dir / 'report.json'
        done = subprocess.run([sys.executable, '-I', str(HERE / 'run.py'), 'commands', '--set', '0.1.0',
                               '--coreav', str(self.coreav), '--media', str(self.media), '--report', str(report)],
                              env={**self.env, **env}, capture_output=True, text=True, timeout=300)
        return done.returncode, json.loads(report.read_text()), done.stdout

    def test_a_different_output_misses_on_each_family(self):
        code, report, _ = self.r1()
        self.assertEqual(code, 1)
        self.assertEqual(report['files'], {'mp4-mov': 1, 'matroska-webm': 1, 'mpegts': 1})
        height = [e['id'] for e in release_round.entries() if e['tool'] == 'ffprobe'
                  and any('stream=height' in ' '.join(c['args']) for c in e['commands'])]
        self.assertTrue(height)
        self.assertTrue(report['missed'])
        self.assertTrue(all(m.split('.')[0] in height for m in report['missed']))
        forced = {(e['id'], n) for e, n, c in release_round.commands('ffprobe') if 'input_container' in c}
        families = {r['family'] for r in report['commands'] if (r['id'], r['command']) in forced}
        self.assertEqual(families, {'mpegts'})
        self.assertFalse(report['fallback_log']['void'])

    def test_a_file_ffprobe_does_not_read_is_not_compared(self):
        _, report, _ = self.r1(FAKE_FFPROBE_FAILS='1')
        mkv = report['families']['matroska-webm']
        self.assertEqual(set(mkv), {'ffprobe fails'})

    def test_a_fallback_voids_the_run(self):
        code, report, out = self.r1(FAKE_LOG_LINE=json.dumps({'ran': '/usr/bin/ffprobe'}))
        self.assertEqual(code, 2)
        self.assertTrue(report['fallback_log']['void'])
        self.assertIn('VOID', out)

    def test_r1_takes_no_baseline(self):
        done = subprocess.run([sys.executable, '-I', str(HERE / 'run.py'), 'commands', '--set', '0.1.0',
                               '--coreav', str(self.coreav), '--media', str(self.media), '--baseline', 'x.json'],
                              env=self.env, capture_output=True, text=True, timeout=60)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('no baseline', done.stderr)


if __name__ == '__main__':
    unittest.main()
