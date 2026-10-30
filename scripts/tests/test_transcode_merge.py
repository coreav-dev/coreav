"""scripts/transcode_check.py --shard and --merge: the shards of a run, merged, are judged as the run."""
import io
import json
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'transcode_check.py'
FILES = {'a.mp4': 'h264', 'b.mov': 'h264', 'c.mp4': 'hevc', 'd.webm': 'vp9', 'e.mp4': 'h264'}
CONFIGS = ('x01', 'x02')


def result(config, key, status):
    arm = {'status': status, 'exit': 0 if status == 'ok' else 1, 'reason': None if status == 'ok' else 'why'}
    return {'config': config, 'class': FILES[key], 'command': '-i IN OUT.mp4', 'file': key, 'complete': True,
            'has_audio': False, 'b2_known_miss': False, 'seconds': 1.0,
            'arms': {'x264': dict(arm), 'ffmpeg': dict(arm)}, 'baseline': {'status': 'ok', 'exit': 0}}


def run_info(expected, shard=None):
    info = {'mode': 'fixtures', 'compat': {c: '-i IN OUT.mp4' for c in CONFIGS},
            'arms': {'x264': [], 'ffmpeg': []}, 'tools': {'coreav': {'path': 'coreav', 'sha256': '00'}},
            'expected': expected}
    return info | ({'shard': shard} if shard else {})


def check(*args):
    p = subprocess.run([sys.executable, '-I', str(SCRIPT), *args], capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


class Merge(unittest.TestCase):
    def runs(self, status_of, n=2, fixtures=lambda i: {'a.mp4': 'aa'}):
        """A whole run's OUT, and the evidence directories of its n shards."""
        tmp = pathlib.Path(self.enterContext(tempfile.TemporaryDirectory()))
        expected = {c: dict(FILES) for c in CONFIGS}
        whole = tmp / 'whole'
        results = {(c, k): result(c, k, status_of(c, k)) for c in CONFIGS for k in FILES}
        for (c, k), r in results.items():
            path = whole / 'results' / c / f'{k}.json'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(r))
        (whole / 'run.json').write_text(json.dumps(run_info(expected)))
        shards = []
        for i in range(1, n + 1):
            mine = sorted(FILES)[i - 1::n]
            d = tmp / f'shard{i}'
            d.mkdir()
            part = {c: {k: FILES[k] for k in mine} for c in CONFIGS}
            with tarfile.open(d / 'results.tar.gz', 'w:gz') as tar:
                members = [('run.json', run_info(part, [i, n]))]
                members += [(f'results/{c}/{k}.json', results[c, k]) for c in CONFIGS for k in mine]
                for name, value in members:
                    data = json.dumps(value).encode()
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    tar.addfile(info, io.BytesIO(data))
            (d / 'calls.jsonl').write_text(''.join(
                json.dumps({'config': c, 'file': k, 'arm': 'x264'}) + '\n' for c in CONFIGS for k in mine))
            (d / 'manifest.json').write_text(json.dumps({'commands': {}, 'fixtures': fixtures(i), 'tools': {}}))
            shards.append(str(d))
        return tmp, whole, shards

    def assert_same_verdict(self, status_of):
        tmp, whole, shards = self.runs(status_of)
        whole_status, whole_text = check('--summarize', '--out', str(whole))
        merged = tmp / 'merged'
        merged_status, text = check('--merge', *shards, '--out', str(merged))
        self.assertEqual(merged_status, whole_status, text)
        self.assertEqual(text, whole_text)
        summary = json.loads((merged / 'summary.json').read_text())
        self.assertEqual(summary['run']['expected'], {c: dict(FILES) for c in CONFIGS})
        self.assertEqual(summary['missing'], {})
        calls = (merged / 'evidence' / 'calls.jsonl').read_text().splitlines()
        self.assertEqual(len(calls), len(CONFIGS) * len(FILES))
        return merged_status

    def test_a_run_every_file_refuses_fails_merged_as_whole(self):
        self.assertEqual(self.assert_same_verdict(lambda c, k: 'refused'), 1)

    def test_a_failure_in_one_shard_fails_the_merged_run(self):
        self.assertEqual(self.assert_same_verdict(lambda c, k: 'failed' if k == 'e.mp4' else 'refused'), 1)

    def test_a_missing_shard_is_refused(self):
        tmp, _, shards = self.runs(lambda c, k: 'refused', n=3)
        status, text = check('--merge', shards[0], shards[2], '--out', str(tmp / 'merged'))
        self.assertEqual(status, 1)
        self.assertIn('not shards 1 to N', text)

    def test_a_shard_given_twice_is_refused(self):
        tmp, _, shards = self.runs(lambda c, k: 'refused')
        status, text = check('--merge', shards[0], shards[0], shards[1], '--out', str(tmp / 'merged'))
        self.assertEqual(status, 1)
        self.assertIn('twice', text)

    def test_shards_on_different_fixture_bytes_are_refused(self):
        tmp, _, shards = self.runs(lambda c, k: 'refused', fixtures=lambda i: {'a.mp4': 'aa', 'd.webm': f'd{i}'})
        status, text = check('--merge', *shards, '--out', str(tmp / 'merged'))
        self.assertEqual(status, 1)
        self.assertIn('different bytes of d.webm', text)

    def test_a_fixture_one_shard_lacks_is_refused(self):
        tmp, _, shards = self.runs(lambda c, k: 'refused', fixtures=lambda i: {'a.mp4': 'aa'} if i == 1 else {})
        status, text = check('--merge', *shards, '--out', str(tmp / 'merged'))
        self.assertEqual(status, 1)
        self.assertIn('different bytes of a.mp4', text)


if __name__ == '__main__':
    unittest.main()
