"""scripts/transcode_check.py --shard and --merge: the shards of a run, merged, are judged as the run."""
import io
import hashlib
import json
import math
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
from types import SimpleNamespace

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import composed_audio as composed
import transcode_check as checker

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


class ComposedMerge(unittest.TestCase):
    def runs(self):
        fixtures = {name: hashlib.sha256(name.encode()).hexdigest() for name in FILES}
        tmp, whole, shards = Merge.runs(self, lambda c, k: 'refused', fixtures=lambda _: fixtures)
        self.marker = {'compat_check': composed.check_policy(), 'judging_sha256': composed.method_identity()}
        tools = {name: {'path': name, 'sha256': 'a' * 64}
                 for name in ('coreav', 'ffmpeg', 'ffprobe', 'opus-compare')}

        def upgrade(document):
            document.update(self.marker)
            if 'config' in document:
                document['command'] = checker.COMPOSED_COMMANDS[document['config']]
                document['source_sha256'] = fixtures[document['file']]
                document['composed_outputs'] = []
                for arm in [document['baseline'], *document['arms'].values()]:
                    arm.update(self.marker)
            else:
                document['commands' if 'commands' in document else 'compat'] = checker.COMPOSED_COMMANDS
                document['tools'] = tools
            return document

        for path in [whole / 'run.json', *whole.glob('results/*/*.json')]:
            path.write_text(json.dumps(upgrade(json.loads(path.read_text()))))
        for shard in map(pathlib.Path, shards):
            self.rewrite_archive(shard, lambda name, document: upgrade(document))
            manifest = shard / 'manifest.json'
            manifest.write_text(json.dumps(upgrade(json.loads(manifest.read_text()))))
            rows = []
            with tarfile.open(shard / 'results.tar.gz') as archive:
                for member in archive.getmembers():
                    if not member.name.startswith('results/'):
                        continue
                    record = json.load(archive.extractfile(member))
                    for name, arm in [('baseline', record['baseline']), *record['arms'].items()]:
                        rows.append(self.marker | {'config': record['config'], 'file': record['file'],
                            'arm': name, 'status': arm['status'], 'exit': arm['exit'],
                            'source_sha256': record['source_sha256']})
            (shard / 'calls.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
        return tmp, whole, shards

    def rewrite_archive(self, shard, change):
        path = pathlib.Path(shard) / 'results.tar.gz'
        with tarfile.open(path) as archive:
            records = [(member.name, json.load(archive.extractfile(member))) for member in archive.getmembers()]
        with tarfile.open(path, 'w:gz') as archive:
            for name, record in records:
                data = json.dumps(change(name, record)).encode()
                member = tarfile.TarInfo(name)
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))

    def test_current_shards_and_whole_have_the_same_verdict(self):
        tmp, whole, shards = self.runs()
        options = ('--compat-check', composed.VERSION)
        status, text = check('--summarize', '--out', str(whole), *options)
        merged = tmp / 'merged'
        merged_status, merged_text = check('--merge', *shards, '--out', str(merged), *options)
        self.assertEqual((merged_status, merged_text), (status, text))
        report = json.loads((merged / 'summary.json').read_text())
        self.assertEqual({key: report[key] for key in self.marker}, self.marker)
        self.assertEqual(report['missing'], {})

    def test_lossless_psnr_infinity_survives_resume_export_and_merge_in_both_versions(self):
        for version in ('legacy-b9', composed.VERSION):
            with self.subTest(version=version):
                tmp, whole, shards = (self.runs() if version == composed.VERSION else
                                     Merge.runs(self, lambda c, k: 'refused'))

                def lossless(name, record):
                    if name.startswith('results/'):
                        record['baseline']['Q1'] = {'ok': True, 'average': math.inf}
                        for arm in record['arms'].values():
                            arm.update(status='ok', exit=0, reason=None, B1={'ok': True}, B2={'ok': True},
                                       Q1={'ok': True, 'delta': 0}, Q2={'ok': True, 'average': math.inf},
                                       size_ratio=1.0)
                    return record

                for path in whole.glob('results/*/*.json'):
                    path.write_text(json.dumps(lossless(str(path.relative_to(whole)), json.loads(path.read_text()))))
                for shard in map(pathlib.Path, shards):
                    self.rewrite_archive(shard, lossless)
                    path = shard / 'calls.jsonl'
                    rows = [json.loads(line) | {'status': 'ok', 'exit': 0} for line in path.read_text().splitlines()]
                    path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
                args = SimpleNamespace(out=str(whole), compat_check=version)
                checker.process(args, {'key': 'a.mp4', 'configs': ['x01']})
                options = ('--compat-check', version)
                status, text = check('--summarize', '--out', str(whole), *options)
                self.assertEqual(status, 0, text)
                merged = tmp / 'merged'
                merged_status, merged_text = check('--merge', *shards, '--out', str(merged), *options)
                self.assertEqual((merged_status, merged_text), (status, text))
                fixture_dir = tmp / 'fixtures'
                fixture_dir.mkdir()
                for name in FILES:
                    (fixture_dir / name).write_bytes(name.encode())
                args.fixtures, args.tmp = str(fixture_dir), str(tmp)
                args.coreav, args.ffmpeg, args.ffprobe = '/missing/coreav', '/missing/ffmpeg', '/missing/ffprobe'
                args.arms, args.compat = [('x264', []), ('ffmpeg', [])], True
                info = json.loads((whole / 'run.json').read_text()) | {'ffmpeg_version': ['fixture']}
                checker.export_evidence(args, whole, info)
                exported = [json.loads(line) for line in (whole / 'evidence/calls.jsonl').read_text().splitlines()]
                self.assertTrue(any(row['Q2_db'] == math.inf for row in exported))

    def test_historical_evidence_cannot_be_selected_as_current_or_current_as_historical(self):
        tmp, whole, shards = Merge.runs(self, lambda c, k: 'refused')
        for mode in [('--summarize', '--out', str(whole)), ('--merge', *shards, '--out', str(tmp / 'merged'))]:
            status, text = check(*mode, '--compat-check', composed.VERSION)
            self.assertNotEqual(status, 0)
            self.assertIn('missing, mixed or relabeled', text)
        tmp, whole, shards = self.runs()
        for mode in [('--summarize', '--out', str(whole)), ('--merge', *shards, '--out', str(tmp / 'merged'))]:
            status, text = check(*mode)
            self.assertNotEqual(status, 0)
            self.assertIn('missing, mixed or relabeled', text)

    def test_cached_records_cannot_change_configuration_file_or_command(self):
        for mutation in ('configuration', 'file', 'command'):
            with self.subTest(mutation=mutation):
                tmp, whole, _ = self.runs()
                original = json.loads((whole / 'results/x01/a.mp4.json').read_text())
                original.update(config='x14', command=checker.COMPOSED_COMMANDS['x14'])
                if mutation == 'configuration':
                    original.update(config='x13', command=checker.COMPOSED_COMMANDS['x13'])
                elif mutation == 'file':
                    original['file'] = 'b.mp4'
                else:
                    original['command'] = checker.COMPOSED_COMMANDS['x13']
                path = whole / 'results/x14/a.mp4.json'
                path.parent.mkdir()
                path.write_text(json.dumps(original))
                expected = {'x14': {'a.mp4': 'h264'}}
                args = SimpleNamespace(out=str(whole), compat_check=composed.VERSION)
                with self.assertRaisesRegex(composed.Invalid, 'requested configuration, file or command'):
                    checker.process(args, {'key': 'a.mp4', 'configs': ['x14']})
                with self.assertRaisesRegex(composed.Invalid, 'requested configuration, file or command'):
                    checker.summarize(whole, True, expected, ['x264', 'ffmpeg'], True)
                fixture_dir = tmp / 'fixtures'
                fixture_dir.mkdir()
                for name in FILES:
                    (fixture_dir / name).write_bytes(name.encode())
                args.fixtures, args.tmp = str(fixture_dir), str(tmp)
                args.coreav, args.ffmpeg, args.ffprobe = '/missing/coreav', '/missing/ffmpeg', '/missing/ffprobe'
                args.arms, args.compat = [('x264', [])], True
                info = json.loads((whole / 'run.json').read_text()) | {'ffmpeg_version': ['fixture']}
                with self.assertRaisesRegex(composed.Invalid, 'requested configuration, file or command'):
                    checker.export_evidence(args, whole, info)

    def test_removing_outer_markers_does_not_turn_current_results_into_legacy(self):
        _, whole, _ = self.runs()
        for path in [whole / 'run.json', *whole.glob('results/*/*.json')]:
            record = json.loads(path.read_text())
            record.pop('compat_check')
            record.pop('judging_sha256')
            path.write_text(json.dumps(record))
            if path.name != 'run.json':
                with self.assertRaisesRegex(composed.Invalid, 'relabeled as legacy'):
                    checker.validate_result(record, None)
        status, text = check('--summarize', '--out', str(whole))
        self.assertNotEqual(status, 0)
        self.assertIn('relabeled as legacy', text)

    def test_changed_judging_source_is_rejected_at_every_evidence_boundary(self):
        for location in ('manifest', 'run', 'result', 'row'):
            with self.subTest(location=location):
                tmp, _, shards = self.runs()
                shard = pathlib.Path(shards[-1])
                if location in ('manifest', 'row'):
                    path = shard / ('manifest.json' if location == 'manifest' else 'calls.jsonl')
                    path.write_text(path.read_text().replace(self.marker['judging_sha256'], '0' * 64))
                else:
                    def change(name, record):
                        if (name == 'run.json') == (location == 'run'):
                            record['judging_sha256'] = '0' * 64
                        return record
                    self.rewrite_archive(shard, change)
                status, text = check('--merge', *shards, '--out', str(tmp / 'merged'),
                                     '--compat-check', composed.VERSION)
                self.assertNotEqual(status, 0)
                self.assertIn('call rows differ' if location == 'row' else 'different judging source bytes', text)

    def test_different_candidate_and_oracle_bytes_cannot_be_merged(self):
        for program in ('coreav', 'ffmpeg', 'ffprobe', 'opus-compare'):
            with self.subTest(program=program):
                tmp, _, shards = self.runs()
                def change(name, record):
                    if name == 'run.json':
                        record['tools'][program]['sha256'] = 'b' * 64
                    return record
                self.rewrite_archive(shards[-1], change)
                status, text = check('--merge', *shards, '--out', str(tmp / 'merged'),
                                     '--compat-check', composed.VERSION)
                self.assertNotEqual(status, 0)
                self.assertIn('different candidate or oracle programs', text)

    def test_resume_and_export_refuse_relabeling_changed_source_and_changed_method(self):
        for mutation in ('version', 'source', 'method'):
            with self.subTest(mutation=mutation):
                tmp, whole, _ = self.runs()
                fixture_dir = tmp / 'fixtures'
                fixture_dir.mkdir()
                for name in FILES:
                    (fixture_dir / name).write_bytes(name.encode())
                args = SimpleNamespace(out=str(whole), compat_check=composed.VERSION,
                    fixtures=str(fixture_dir), tmp=str(tmp), coreav='/missing/coreav',
                    ffmpeg='/missing/ffmpeg', ffprobe='/missing/ffprobe', arms=[('x264', [])])
                path = whole / 'results/x01/a.mp4.json'
                record = json.loads(path.read_text())
                field = {'version': 'compat_check', 'source': 'source_sha256', 'method': 'judging_sha256'}[mutation]
                record[field] = None if mutation == 'version' else '0' * 64
                path.write_text(json.dumps(record))
                with self.assertRaises(composed.Invalid):
                    checker.process(args, {'key': 'a.mp4', 'configs': ['x01'], 'path': fixture_dir / 'a.mp4'})
                with self.assertRaises(composed.Invalid):
                    checker.export_evidence(args, whole, json.loads((whole / 'run.json').read_text()))


if __name__ == '__main__':
    unittest.main()
