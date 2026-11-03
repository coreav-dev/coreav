"""Media-free evidence tests. All codec and process execution is mocked."""
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import statistics
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import wave

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import h264_parity_bars as bars
import h264_pgo_diagnostics as diagnostics
import h264_pgo_round as round_driver
from oracle_audio import INPUT_ARGS


def hashed(data):
    return {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}


class PairedCommands(unittest.TestCase):
    def summary(self, base, pgo, resamples=10000):
        return bars.paired_command_summary({'base': dict(enumerate(base)), 'pgo': dict(enumerate(pgo))},
                                           'pgo', 'base', len(base), 20261103, resamples)

    def test_paired_rounds_preserve_constant_ratio_and_values(self):
        base = [1, 100, 3, 90, 5, 80, 7, 70, 9]
        result = self.summary(base, [v * 2 for v in base])
        self.assertEqual(result['median'], 2)
        self.assertEqual(result['ci95'], [2, 2])
        self.assertEqual(result['calls']['base'],
                         {'values': base, 'median': 9, 'min': 1, 'max': 100, 'range': 99})

    def test_ratio_of_medians_not_median_of_ratios_and_linear_percentile(self):
        base, pgo = [1, 10, 100], [2, 60, 90]
        result = self.summary(base, pgo, 17)
        rng = random.Random(20261103)
        samples = []
        for _ in range(17):
            indices = rng.choices(range(3), k=3)
            samples.append(statistics.median(pgo[n] for n in indices) /
                           statistics.median(base[n] for n in indices))
        samples.sort()
        self.assertEqual(result['median'], 6)
        self.assertNotEqual(result['median'], statistics.median(p / b for p, b in zip(pgo, base)))
        self.assertEqual(result['ci95'], [samples[0] + .4 * (samples[1] - samples[0]),
                                         samples[15] + .6 * (samples[16] - samples[15])])

    def test_missing_extra_and_nonfinite_cells_are_refused(self):
        good = {'base': {0: 1, 1: 2}, 'pgo': {0: 1, 1: 2}}
        for bad in (None, 0, -1, True, float('nan'), float('inf')):
            changed = copy.deepcopy(good)
            changed['pgo'][1] = bad
            with self.subTest(value=bad), self.assertRaises(bars.Refused):
                bars.paired_command_summary(changed, 'pgo', 'base', 2, 20261103, 10000)
        for changed in ({'base': {0: 1}, 'pgo': {0: 1, 1: 1}},
                        {**good, 'unexpected': {0: 1, 1: 1}},
                        {'base': {False: 1, 1: 2}, 'pgo': {0: 1, 1: 2}},
                        {'base': {0: 1e-308, 1: 1e-308}, 'pgo': {0: 1e308, 1: 1e308}}):
            with self.assertRaises(bars.Refused):
                bars.paired_command_summary(changed, 'pgo', 'base', 2, 20261103, 10000)


class Diagnostics(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.plan = json.loads(diagnostics.PLAN.read_text())
        self.plan['diagnostics']['fixture']['sample_frames'] = 11
        self.plan_path = self.root / 'registration.json'
        self.plan_path.write_text(json.dumps(self.plan))
        self.patch = mock.patch.object(diagnostics, 'PLAN', self.plan_path)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        self.signal = b''.join(diagnostics.pcm_chunks(self.plan['diagnostics']['fixture']))
        self.calls = []
        for name in ('ffmpeg', 'ffprobe', 'cli-base', 'cli-pgo', 'cli-independent'):
            (self.root / name).write_bytes(b'fixed mock tool')
        self.prepare_fixtures()

    def encode(self, argv, **kwargs):
        self.calls.append((argv, kwargs))
        Path(argv[-1]).write_bytes(b'synthetic fixture ' + argv[argv.index('-c:a') + 1].encode())
        return subprocess.CompletedProcess(argv, 0, b'', b'')

    def prepare_fixtures(self):
        with mock.patch.object(round_driver, 'merged') as merged, \
                mock.patch.object(round_driver, 'reservation') as reservation, \
                mock.patch.object(diagnostics.oracle_pin, 'verify_bin', return_value=iter(())), \
                mock.patch.object(diagnostics.subprocess, 'run', side_effect=self.encode):
            result = diagnostics.prepare(self.root, self.root / 'ffmpeg', self.plan)
            merged.assert_called_once()
            reservation.assert_called_once_with(self.root, self.plan)
            return result

    def execute(self, argv, maximum=0, timeout=180, capture=False):
        self.calls.append((argv, {'maximum': maximum, 'timeout': timeout, 'capture': capture}))
        self.assertEqual(os.environ['COREAV_STRICT_SAFE'], '1')
        self.assertEqual(timeout, 180)
        if argv[0] == str(self.root / 'ffmpeg'):
            self.assertEqual(argv[argv.index('-i') - len(INPUT_ARGS):argv.index('-i')], list(INPUT_ARGS))
            self.assertEqual(maximum, len(self.signal))
            return {'wall': 10, **hashed(self.signal)}
        self.assertEqual(argv[:2], ['taskset', '-c'])
        if 'decode-audio' in argv:
            self.assertEqual(argv[2], '2' if argv[argv.index('--threads') + 1] == '1' else '0-7')
        stdout = b'{"complete":true}\n'
        if 'remux' in argv:
            self.assertFalse(Path(argv[-1]).exists())
            Path(argv[-1]).write_bytes(b'complete MP4 mock output')
        elif 'decode-audio' in argv:
            self.assertFalse(Path(argv[-1]).exists())
            with wave.open(argv[-1], 'wb') as output:
                output.setnchannels(2)
                output.setsampwidth(2)
                output.setframerate(48000)
                output.writeframes(self.signal)
        pgo = Path(argv[3]).name == 'cli-pgo'
        return {'wall': self.ratio if pgo else 1, **hashed(stdout),
                **({'stdout': stdout.decode()} if capture else {})}

    def run_and_audit(self, ratio=1):
        self.ratio = ratio
        self.assertEqual(diagnostics.run(self.root, self.plan, self.execute), 'complete')
        return diagnostics.audit(self.root, self.plan)

    def events(self):
        return [json.loads(line) for line in (self.root / 'diagnostics.jsonl').read_text().splitlines()]

    def write_events(self, events):
        (self.root / 'diagnostics.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in events))

    def test_small_pcm_vector_and_interleaved_count(self):
        fixture = dict(self.plan['diagnostics']['fixture'], sample_frames=3)
        self.assertEqual(b''.join(diagnostics.pcm_chunks(fixture)).hex(), 'c879669b7ea6fc11f4e09983')
        self.assertEqual(diagnostics.signal_identity(fixture)['bytes'], 12)
        fixture['sample_frames'] = 16385
        blocks = list(diagnostics.pcm_chunks(fixture))
        self.assertEqual([len(b) for b in blocks], [65536, 4])
        self.assertNotEqual(blocks[0][:4], blocks[1])

    def test_encode_commands_and_checkpoint_resume(self):
        encodes = [argv for argv, _ in self.calls]
        self.assertEqual(len(encodes), 2)
        for codec, argv in zip(('alac', 'flac'), encodes):
            self.assertEqual(argv[0], str(self.root / 'ffmpeg'))
            self.assertEqual(argv[argv.index('-i') - len(INPUT_ARGS):argv.index('-i')], list(INPUT_ARGS))
            for option, value in (('-map', '0:a:0'), ('-c:a', codec), ('-flags:a', '+bitexact'),
                                  ('-fflags', '+bitexact')):
                self.assertEqual(argv[argv.index(option) + 1], value)
        self.assertEqual(encodes[0][-7:-1], ['-sample_fmt', 's16p', '-movflags', '+faststart', '-f', 'ipod'])
        self.assertEqual(encodes[1][-7:-1], ['-sample_fmt', 's16', '-compression_level', '5', '-f', 'matroska'])
        self.calls.clear()
        self.prepare_fixtures()
        self.assertEqual(self.calls, [])

    def test_partial_preparation_reuses_only_checkpointed_bytes(self):
        manifest = self.root / diagnostics.FILES[0]
        saved = json.loads(manifest.read_text())
        saved['status'] = 'preparing'
        del saved['files'][diagnostics.FILES[3]]
        manifest.write_text(json.dumps(saved))
        (self.root / diagnostics.FILES[3]).write_bytes(b'interrupted partial output')
        self.calls.clear()
        self.prepare_fixtures()
        self.assertEqual(len(self.calls), 1)
        self.assertEqual(self.calls[0][0][self.calls[0][0].index('-c:a') + 1], 'flac')
        self.assertEqual((self.root / diagnostics.FILES[1]).read_bytes(), self.signal)
        diagnostics.verify(self.root, self.plan)

    def test_preparation_gates_precede_oracle_or_generation(self):
        with mock.patch.object(round_driver, 'merged', side_effect=ValueError('unmerged')), \
                mock.patch.object(diagnostics.oracle_pin, 'verify_bin') as oracle, \
                mock.patch.object(diagnostics, 'pcm_chunks') as pcm:
            with self.assertRaisesRegex(ValueError, 'unmerged'):
                diagnostics.prepare(self.root, self.root / 'ffmpeg', self.plan)
            oracle.assert_not_called()
            pcm.assert_not_called()

    def test_verify_is_media_free_and_refuses_changed_evidence(self):
        with mock.patch.object(diagnostics.subprocess, 'run', side_effect=AssertionError('program executed')):
            diagnostics.verify(self.root, self.plan)
            path = self.root / diagnostics.FILES[2]
            path.write_bytes(path.read_bytes() + b'x')
            with self.assertRaisesRegex(ValueError, 'fixture changed'):
                diagnostics.verify(self.root, self.plan)
            with self.assertRaisesRegex(ValueError, 'fixture changed'):
                self.prepare_fixtures()

    def test_unknown_manifest_file_and_changed_spec_or_tool_are_refused(self):
        path = self.root / diagnostics.FILES[0]
        saved = json.loads(path.read_text())
        for change in ('extra', 'fixture', 'tools', 'oracle'):
            value = copy.deepcopy(saved)
            if change == 'extra':
                value['files']['arbitrary-command'] = hashed(b'bad')
            elif change == 'fixture':
                value['fixture']['seed'] += 1
            else:
                value[change] = {}
            path.write_text(json.dumps(value))
            with self.subTest(change=change), self.assertRaises(ValueError):
                diagnostics.verify(self.root, self.plan)

    def test_complete_qualification_roster_affinity_and_non_gating_guards(self):
        with mock.patch.dict(os.environ, {'COREAV_STRICT_SAFE': 'previous'}):
            result = self.run_and_audit(1.01)
            self.assertEqual(os.environ['COREAV_STRICT_SAFE'], 'previous')
        self.assertEqual(result['status'], 'complete')
        self.assertIs(result['gates_h264'], False)
        self.assertNotIn('pass', result)
        self.assertEqual(set(result['commands']), {'probe', 'mp4-remux', 'flac-decode'})
        for table in result['commands'].values():
            self.assertEqual(set(table), {'1', '8'})
            for cell in table.values():
                self.assertEqual(cell['ci95'], [1.01, 1.01])
                self.assertIs(cell['guard'], True)
                self.assertEqual(len(cell['calls']['pgo']['values']), 9)
        events = self.events()
        self.assertEqual([r['kind'] for r in events].count('qualification'), 18)
        self.assertEqual([r['kind'] for r in events].count('warmup'), 12)
        self.assertEqual([r['kind'] for r in events].count('timed'), 108)
        native = [argv for argv, _ in self.calls if argv[0] == 'taskset']
        self.assertEqual({argv[2] for argv in native}, {'2', '0-7'})

    def test_guard_above_boundary_remains_non_gating(self):
        result = self.run_and_audit(1.010001)
        self.assertEqual(result['status'], 'complete')
        self.assertIs(result['gates_h264'], False)
        self.assertTrue(all(not cell['guard'] for table in result['commands'].values() for cell in table.values()))

    def test_missing_duplicate_nonfinite_failed_or_reordered_cells_refused(self):
        self.run_and_audit()
        original = self.events()
        first = next(n for n, r in enumerate(original) if r['kind'] == 'timed')
        mutations = []
        missing = copy.deepcopy(original)
        missing.pop(first)
        mutations.append(missing)
        duplicate = copy.deepcopy(original)
        duplicate.insert(first, duplicate[first])
        mutations.append(duplicate)
        swapped = copy.deepcopy(original)
        swapped[first], swapped[first + 1] = swapped[first + 1], swapped[first]
        mutations.append(swapped)
        mutations.append(original[:-1])
        mutations.append([*original[:-1], 0])
        mutations.append([None, *original[1:]])
        for value in (float('nan'), float('inf'), 0, -1, True):
            rows = copy.deepcopy(original)
            rows[first]['wall'] = value
            mutations.append(rows)
        failed = copy.deepcopy(original)
        failed[first]['kind'] = 'failed'
        mutations.append(failed)
        boolean_round = copy.deepcopy(original)
        boolean_round[first]['round'] = False
        mutations.append(boolean_round)
        for rows in mutations:
            self.write_events(rows)
            with self.assertRaises((ValueError, KeyError)):
                diagnostics.audit(self.root, self.plan)

    def test_output_and_qualification_sample_mutations_refused(self):
        self.run_and_audit()
        original = self.events()
        for field, wanted in (('output', 'probe'), ('samples', 'mp4-remux')):
            rows = copy.deepcopy(original)
            row = next(r for r in rows if r.get('case') == wanted)
            row[field]['sha256'] = '0' * 64
            self.write_events(rows)
            with self.assertRaises(ValueError):
                diagnostics.audit(self.root, self.plan)

    def test_native_failure_has_no_fast_sample_and_restores_environment(self):
        self.ratio = 1
        count = 0
        def failed(argv, **kwargs):
            nonlocal count
            count += 1
            if count == 2:
                raise subprocess.CalledProcessError(1, argv, stderr=b'failure')
            return self.execute(argv, **kwargs)
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual(diagnostics.run(self.root, self.plan, failed), 'failed')
            self.assertNotIn('COREAV_STRICT_SAFE', os.environ)
        result = diagnostics.audit(self.root, self.plan)
        self.assertEqual(result['status'], 'failed')
        self.assertIsNone(result['guards'])
        self.assertTrue(all(cell == {'guard': None, 'status': 'incomplete'}
                            for table in result['commands'].values() for cell in table.values()))
        self.assertEqual(len(self.events()), 2)

    def test_injected_command_failure_during_timing_retains_no_failed_time(self):
        self.ratio = 1
        native_calls = 0
        def failed(argv, **kwargs):
            nonlocal native_calls
            if argv[0] == 'taskset':
                native_calls += 1
                if native_calls == 32:
                    raise subprocess.CalledProcessError(0, argv, stderr=b'unexpected stderr')
            return self.execute(argv, **kwargs)
        self.assertEqual(diagnostics.run(self.root, self.plan, failed), 'failed')
        result = diagnostics.audit(self.root, self.plan)
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['failure']['cell']['kind'], 'timed')
        self.assertEqual(sum(r['kind'] == 'timed' for r in self.events()), 1)
        self.assertNotIn('wall', result['failure'])
        self.assertNotIn('pass', result)

    def test_changed_source_and_independent_artifact_are_contract_failures(self):
        self.ratio = 1
        independent = self.root / 'cli-independent'
        independent.write_bytes(b'changed build')
        executor = mock.Mock()
        with self.assertRaisesRegex(ValueError, 'rebuild differs'):
            diagnostics.run(self.root, self.plan, executor)
        executor.assert_not_called()
        independent.write_bytes((self.root / 'cli-pgo').read_bytes())
        def changed(argv, **kwargs):
            result = self.execute(argv, **kwargs)
            (self.root / diagnostics.FILES[2]).write_bytes(b'changed source')
            return result
        with self.assertRaisesRegex(ValueError, 'source changed'):
            diagnostics.run(self.root, self.plan, changed)
        with self.assertRaises(ValueError):
            diagnostics.audit(self.root, self.plan)

    def test_native_output_disagreement_and_pcm_or_count_failure_are_explicit(self):
        self.ratio = 1
        def changed(argv, **kwargs):
            result = self.execute(argv, **kwargs)
            if argv[0] == str(self.root / 'ffmpeg'):
                result['bytes'] -= 4
            return result
        self.assertEqual(diagnostics.run(self.root, self.plan, changed), 'failed')
        self.assertIn('samples or sample count', self.events()[-1]['reason'])
        (self.root / 'diagnostics.jsonl').unlink()
        def different(argv, **kwargs):
            result = self.execute(argv, **kwargs)
            if 'probe' in argv and Path(argv[3]).name == 'cli-pgo':
                value = b'{"different":true}'
                result.update(hashed(value), stdout=value.decode())
            return result
        self.assertEqual(diagnostics.run(self.root, self.plan, different), 'failed')
        self.assertIn('arm outputs differ', self.events()[-1]['reason'])

    def test_infrastructure_contract_timeout_and_oracle_failures_propagate(self):
        for error in (OSError('host'), subprocess.TimeoutExpired('tool', 180), ValueError('contract')):
            with self.subTest(error=error), self.assertRaises(type(error)):
                diagnostics.run(self.root, self.plan, mock.Mock(side_effect=error))
            with self.assertRaises(ValueError):
                diagnostics.audit(self.root, self.plan)
            (self.root / 'diagnostics.jsonl').unlink()
        self.ratio = 1
        def oracle_failure(argv, **kwargs):
            if argv[0] == str(self.root / 'ffmpeg'):
                raise subprocess.CalledProcessError(1, argv)
            return self.execute(argv, **kwargs)
        with self.assertRaises(subprocess.CalledProcessError):
            diagnostics.run(self.root, self.plan, oracle_failure)

    def test_wav_complete_sample_count(self):
        fixture = self.plan['diagnostics']['fixture']
        path = self.root / 'diagnostics/test.wav'
        with wave.open(str(path), 'wb') as output:
            output.setparams((2, 2, 48000, 0, 'NONE', 'not compressed'))
            output.writeframes(self.signal[:-4])
        with self.assertRaises(diagnostics.DiagnosticFailure):
            diagnostics.wav_identity(path, fixture)


if __name__ == '__main__':
    unittest.main()
