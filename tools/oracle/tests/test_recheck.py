"""The corpus rechecks of tools/stage3-report/corpus.py
(docs/design/stage3-report.md, "Rechecks"): which records each mode makes
again, and that a regression planted in a file that agreed shows in the
totals after a `stale` or an `affected` recheck, and not after an
`uncovered` one, the mode that let such a regression through."""
import argparse
import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools' / 'stage3-report'))
SPEC = importlib.util.spec_from_file_location('corpus', ROOT / 'tools' / 'stage3-report' / 'corpus.py')
corpus = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(corpus)

OLD = {'adapter': 'old', 'oracle': 'o', 'overlay': 'k', 'harness': 'h', 'ffprobe': 'f', 'mediainfo': 'm'}
NEW = {'adapter': 'new', 'oracle': 'o', 'overlay': 'k', 'harness': 'h', 'ffprobe': 'f', 'mediainfo': 'm'}


def levels(uncovered=0):
    counts = {'items': 2, 'direct': 2 - uncovered, 'via_kd': 0, 'uncovered': uncovered, 'by_kd': {},
              'uncovered_items': [], 'covered_items': []}
    return {'streams': dict(counts), 'packets': dict(counts, per_sample=dict(counts))}


def entry(sha, holdout=False):
    return {'sha256': sha, 'ext': 'mp4', 'holdout': holdout, 'size': 1}


class Corpus:
    """Records and kept captures of four files in a temporary directory:
    a agrees and has a text track; b agrees, video only, made by the
    current code; c has an uncovered disagreement; d failed."""

    def __init__(self, root):
        self.out = pathlib.Path(root)
        (self.out / 'files').mkdir()
        self.entries = [entry('a' * 64), entry('b' * 64), entry('c' * 64), entry('d' * 64, holdout=True)]
        records = {'a': (OLD, levels()), 'b': (NEW, levels()), 'c': (OLD, levels(uncovered=1))}
        streams = {'a': [{'codec_name': 'mov_text', 'codec_fourcc': 'tx3g', 'kind': 'subtitle'},
                         {'codec_name': 'aac', 'codec_fourcc': 'mp4a', 'kind': 'audio'}],
                   'b': [{'codec_name': 'h264', 'codec_fourcc': 'avc1', 'kind': 'video'}],
                   'c': [{'codec_name': 'aac', 'codec_fourcc': 'mp4a', 'kind': 'audio'}]}
        for key, (made_by, lv) in records.items():
            self.write(key, {'sha256': key * 64, 'ext': 'mp4', 'holdout': False, 'tools': made_by, 'levels': lv})
            for side in ('ffprobe', 'coreav'):
                (self.out / side).mkdir(exist_ok=True)
                (self.out / side / f'{key * 64}.mp4.streams.json').write_text(json.dumps({'streams': streams[key]}))
        self.write('d', {'sha256': 'd' * 64, 'ext': 'mp4', 'holdout': True, 'tools': OLD, 'error': 'timeout'})

    def write(self, key, record):
        (self.out / 'files' / f'{key * 64}.json').write_text(json.dumps(record))

    def chosen(self, mode, codecs=(), kinds=()):
        return {e['sha256'][0] for e in corpus.select(self.entries, self.out, mode, NEW, codecs, kinds)}


class Selection(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.c = Corpus(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_stale_takes_every_record_other_code_made(self):
        self.assertEqual(self.c.chosen('stale'), {'a', 'c', 'd'})

    def test_affected_takes_the_streams_a_change_touches(self):
        self.assertEqual(self.c.chosen('affected', codecs=['mov_text']), {'a', 'c', 'd'})
        self.assertEqual(self.c.chosen('affected', codecs=['avc1']), {'b', 'c', 'd'})
        self.assertEqual(self.c.chosen('affected', kinds=['video', 'subtitle']), {'a', 'b', 'c', 'd'})
        self.assertEqual(self.c.chosen('affected', codecs=['vp9']), {'c', 'd'})

    def test_uncovered_cannot_see_files_that_agreed(self):
        self.assertEqual(self.c.chosen('uncovered'), {'c', 'd'})

    def test_errors_and_all(self):
        self.assertEqual(self.c.chosen('errors'), {'d'})
        self.assertEqual(self.c.chosen('all'), {'a', 'b', 'c', 'd'})

    def test_a_missing_record_is_always_made(self):
        (self.c.out / 'files' / f"{'b' * 64}.json").unlink()
        self.assertIn('b', self.c.chosen('affected', codecs=['vp9']))
        self.assertIn('b', self.c.chosen('uncovered'))


class PlantedRegression(unittest.TestCase):
    """The new code makes file a, which agreed, disagree. The totals show it
    after a recheck that makes a's record again."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.c = Corpus(self.tmp.name)
        self.saved = (corpus.entries, corpus.tools, corpus.check_file, corpus.sha256_file)
        corpus.entries = lambda family: self.c.entries
        corpus.tools = lambda family: NEW
        corpus.sha256_file = lambda path: '0' * 64

        def check_file(e, family, out, work, made_by, local=None):
            # The new code: a regresses, the others come out as before.
            uncovered = 1 if e['sha256'][0] in 'ac' else 0
            return {'sha256': e['sha256'], 'ext': e['ext'], 'holdout': e['holdout'], 'tools': made_by,
                    'levels': levels(uncovered)}
        corpus.check_file = check_file

    def tearDown(self):
        corpus.entries, corpus.tools, corpus.check_file, corpus.sha256_file = self.saved
        self.tmp.cleanup()

    def recheck(self, mode, codec=None):
        args = argparse.Namespace(family='isobmff', out=str(self.c.out), work=str(self.c.out / 'work'), workers=1,
                                  limit=0, recheck=mode, codec=codec, kind=None, only=None, local=None)
        self.assertEqual(corpus.cmd_run(args), 0)
        return corpus.summarize(self.c.out, 'isobmff')['all']

    def agreeing(self, summary):
        return summary['checked'] - len(summary['files_with_uncovered'])

    def test_stale_catches_it(self):
        summary = self.recheck('stale')
        self.assertIn('a' * 64, summary['files_with_uncovered'])
        self.assertEqual(self.agreeing(summary), 2)  # b, and d, which no longer fails
        self.assertFalse(summary['mixed_tools'])

    def test_affected_catches_it_when_the_change_names_its_streams(self):
        summary = self.recheck('affected', codec='mov_text')
        self.assertIn('a' * 64, summary['files_with_uncovered'])
        # a, c and d were made again; b, video only, was not.
        run = json.loads((self.c.out / 'runs.jsonl').read_text().splitlines()[-1])
        self.assertEqual((run['files'], run['codecs']), (3, ['mov_text']))

    def test_uncovered_misses_it(self):
        summary = self.recheck('uncovered')
        self.assertNotIn('a' * 64, summary['files_with_uncovered'])
        self.assertTrue(summary['mixed_tools'])

    def run_with(self, recheck, codec=None, kind=None):
        args = argparse.Namespace(family='isobmff', out=str(self.c.out), work=str(self.c.out / 'work'), workers=1,
                                  limit=0, recheck=recheck, codec=codec, kind=kind, only=None, local=None)
        return corpus.cmd_run(args)

    def test_affected_needs_a_codec_or_a_kind_that_some_stream_has(self):
        self.assertEqual(self.run_with('affected'), 2)
        # A misspelt codec matches no stream: refused, not a quiet
        # `uncovered` recheck.
        self.assertEqual(self.run_with('affected', codec='mov-text'), 2)
        self.assertFalse((self.c.out / 'runs.jsonl').exists())

    def test_codec_and_kind_go_with_affected_only(self):
        self.assertEqual(self.run_with('stale', codec='mov_text'), 2)
        self.assertEqual(self.run_with('uncovered', kind='video'), 2)

    def test_a_record_without_tools_is_stale(self):
        record = json.loads((self.c.out / 'files' / f"{'b' * 64}.json").read_text())
        del record['tools']
        self.c.write('b', record)
        self.assertIn('b', self.c.chosen('stale'))

    def test_a_plain_resume_makes_only_missing_records(self):
        (self.c.out / 'files' / f"{'b' * 64}.json").unlink()
        self.assertEqual(self.c.chosen(None), {'b'})


class Fingerprint(unittest.TestCase):
    """`tools()` covers the capture scripts, corpus.py, the overlay's
    imports and the ffprobe and MediaInfo binaries, so `stale` makes again
    the records a change to any of them affects."""

    def setUp(self):
        import os
        import shutil
        self.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.tmp.name) / 'root'
        for rel in corpus.HARNESS + corpus.OVERLAY_IMPORTS['isobmff'] + corpus.OVERLAY_IMPORTS['matroska'] + (
                'scripts/oracle_kd.py', 'scripts/oracle_kd_mkv.py'):
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / rel, root / rel)
        release = root / 'release'
        release.mkdir()
        for name in ('isobmff-adapter', 'mkv-adapter', 'coreav-oracle', 'coreav'):
            (release / name).write_bytes(name.encode())
        self.bin = pathlib.Path(self.tmp.name) / 'bin'
        self.bin.mkdir()
        self.tool('ffprobe', 'ffprobe version n9.0.2-test')
        self.tool('mediainfo', 'MediaInfo Command line,\nMediaInfoLib - v26.10')
        self.root = root
        saved = (corpus.ROOT, dict(os.environ))

        def restore():
            corpus.ROOT = saved[0]
            os.environ.clear()
            os.environ.update(saved[1])
            self.tmp.cleanup()
        self.addCleanup(restore)
        corpus.ROOT = root
        os.environ.update({'COREAV_ADAPTER_DIR': str(release), 'COREAV_ORACLE': str(release / 'coreav-oracle'),
                           'PATH': f"{self.bin}:{os.environ.get('PATH', '')}"})
        os.environ.pop('FFPROBE', None)
        # Two records made by the current tools.
        self.out = pathlib.Path(self.tmp.name) / 'out'
        (self.out / 'files').mkdir(parents=True)
        self.entries = [entry('a' * 64), entry('b' * 64)]
        for e in self.entries:
            (self.out / 'files' / f"{e['sha256']}.json").write_text(json.dumps(
                {'sha256': e['sha256'], 'ext': 'mp4', 'holdout': False, 'tools': corpus.tools('isobmff'),
                 'levels': levels()}))

    def tool(self, name, version):
        """A fake tool on PATH that prints `version`, one line per line."""
        path = self.bin / name
        path.write_text('#!/bin/sh\n' + ''.join(f"echo '{line}'\n" for line in version.split('\n')))
        path.chmod(0o755)

    def stale(self, family='isobmff'):
        return len(corpus.select(self.entries, self.out, 'stale', corpus.tools(family)))

    def test_nothing_is_stale_with_the_same_tools(self):
        self.assertEqual(self.stale(), 0)

    def test_a_changed_capture_script_makes_every_record_stale(self):
        with open(self.root / 'scripts' / 'oracle-capture.sh', 'a') as script:
            script.write('\n# changed\n')
        self.assertEqual(self.stale(), 2)

    def test_a_changed_ffprobe_or_mediainfo_makes_every_record_stale(self):
        self.tool('ffprobe', 'ffprobe version 7.1')
        self.assertEqual(self.stale(), 2)

    def test_a_rebuilt_binary_of_the_same_version_makes_every_record_stale(self):
        with open(self.bin / 'mediainfo', 'a') as tool:
            tool.write('# rebuilt\n')
        self.assertEqual(self.stale(), 2)

    def test_a_changed_coreav_command_makes_every_record_stale(self):
        # The compat check runs `coreav probe --ffprobe-compat`.
        (self.root / 'release' / 'coreav').write_bytes(b'coreav, rebuilt')
        self.assertEqual(self.stale(), 2)

    def test_a_changed_compat_check_makes_every_record_stale(self):
        with open(self.root / 'scripts' / 'compat_check.py', 'a') as script:
            script.write('\n# changed\n')
        self.assertEqual(self.stale(), 2)

    def test_the_overlay_imports_count_for_their_family(self):
        before = corpus.tools('matroska')['harness']
        with open(self.root / 'scripts' / 'policy_ref.py', 'a') as module:
            module.write('\n# changed\n')
        # policy_ref.py is the Matroska overlay's import, not the MP4 one's.
        self.assertNotEqual(corpus.tools('matroska')['harness'], before)
        self.assertEqual(self.stale('isobmff'), 0)

    def test_the_mp4_overlay_counts_the_transport_stream_module_it_imports(self):
        matroska = corpus.tools('matroska')['harness']
        with open(self.root / 'scripts' / 'oracle_kd_ts.py', 'a') as module:
            module.write('\n# changed\n')
        self.assertEqual(self.stale('isobmff'), 2)
        self.assertEqual(corpus.tools('matroska')['harness'], matroska)

    def test_a_run_without_ffprobe_on_path_is_refused(self):
        import argparse
        (self.bin / 'ffprobe').unlink()
        import os
        os.environ['PATH'] = str(self.bin)
        args = argparse.Namespace(family='isobmff', out=str(self.out), work=str(self.out / 'work'), workers=1,
                                  limit=0, recheck='stale', codec=None, kind=None, only=None, local=None)
        self.assertEqual(corpus.cmd_run(args), 2)

    def test_the_versions_are_recorded(self):
        made = corpus.tools('isobmff')
        self.assertTrue(made['ffprobe'].startswith('ffprobe version n9.0.2-test sha256:'))
        self.assertTrue(made['mediainfo'].startswith('MediaInfoLib - v26.10 sha256:'))

if __name__ == '__main__':
    unittest.main()
