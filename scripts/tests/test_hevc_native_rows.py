import copy
import json
import pathlib
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_threading as threading


class NativeRows(unittest.TestCase):
    def receipt(self, threads=4, check=True):
        color = dict.fromkeys(threading.hevc_boundary.COLOR_FIELDS)
        picture = dict(type='picture', timestamp=0, width=8, height=8, bit_depth=8,
                       packing='yuv420p', color=color, output_bytes=96, output_md5='a' * 32)
        semantic = dict(headers=dict(nals=3, pictures=2, output_pictures=1,
                                    warnings=dict(profile_bit_depth=False, invalid_sample_aspect_ratio=False,
                                                  temporal_nesting_nal_type=False)), decoded_pictures=2,
                        skipped_pictures=0, skipped_picture_hashes=0, skipped_reserved_hashes=0,
                        stats=dict(bins=7, ctus=2, memory_bytes=0), inter=dict(prediction_units=0))
        admission = dict(sps_id=0, sps_sha256=[0] * 32, ceiling=threads or 8, workers=2, lag=1,
                         frame_bytes=96, reserved_bytes=1 << 20, limit_bytes=1 << 30)
        summary = dict(type='summary', complete=True, boundary=True, backend='native', semantic=semantic,
                       pictures=1, output_pictures=1, decoded_pictures=2, bins=7, requested_threads=threads,
                       host_threads=8, thread_limit=threads or 8, max_workers=1 if threads == 1 else 2,
                       parallel_pictures=0 if threads == 1 else 2, frame_parallel_pictures=0 if threads == 1 else 2,
                       max_memory_bytes=1 << 30, memory_bytes=1 << 20, input_bytes=128, sent_packets=2,
                       inherited_color=color, output_bytes=96 if check else None,
                       output_md5='a' * 32 if check else None, admissions=[] if threads == 1 else [admission])
        return [picture, summary] if check else [summary]

    def encoded(self, records):
        return '\n'.join(json.dumps(record) for record in records).encode()

    def test_native_receipt_cannot_credit_the_old_complete_reference_protocol(self):
        records = self.receipt()
        frames, summary = threading.native_receipt(self.encoded(records), 4, check=True)
        self.assertEqual(len(frames), 1)
        self.assertEqual(summary['frame_parallel_pictures'], 2)
        for key in ('boundary', 'backend', 'semantic', 'admissions'):
            changed = copy.deepcopy(records)
            changed[-1].pop(key)
            with self.subTest(key=key), self.assertRaises(ValueError):
                threading.native_receipt(self.encoded(changed), 4, check=True)

    def test_counter_types_admission_bytes_and_timed_hashing_fail_closed(self):
        mutations = [lambda row: row.update(requested_threads=8),
                     lambda row: row.update(max_workers=5),
                     lambda row: row.update(bins=True),
                     lambda row: row['semantic']['stats'].update(bins=True),
                     lambda row: row['admissions'][0].update(lag=2),
                     lambda row: row['admissions'][0].update(reserved_bytes=1 << 30),
                     lambda row: row['admissions'][0].update(sps_sha256=[0]),
                     lambda row: row.update(pictures=2)]
        for mutate in mutations:
            records = self.receipt()
            mutate(records[-1])
            with self.subTest(mutate=mutate), self.assertRaises(ValueError):
                threading.native_receipt(self.encoded(records), 4, check=True)
        records = self.receipt(check=False)
        threading.native_receipt(self.encoded(records), 4, check=False)
        records[-1]['output_md5'] = 'a' * 32
        with self.assertRaisesRegex(ValueError, 'disable_hashing'):
            threading.native_receipt(self.encoded(records), 4, check=False)
        records = self.receipt()
        records[0].update(bit_depth=10, packing='yuv420p10le', output_bytes=192)
        records[-1]['output_bytes'] = 192
        threading.native_receipt(self.encoded(records), 4, check=True)
        records[0]['timestamp'] = True
        with self.assertRaisesRegex(ValueError, 'frame_receipt'):
            threading.native_receipt(self.encoded(records), 4, check=True)

    def conformance(self, directory, mutation=None):
        source = directory / 'input'
        source.write_bytes(b'fixture')
        expected = self.receipt()
        expected[0].update(picture_index=0, color_declarations=expected[0]['color'])
        raw = dict(complete=True, passed=True, scope='complete_c1',
                   identities=dict.fromkeys(('registration', 'census', 'parser', 'acquisition'), 'b' * 64),
                   denominator=[dict(test='fixture', sha256='b' * 64)],
                   results=[dict(complete=True, test='fixture', sha256='b' * 64,
                                 pictures=expected[:-1], summary={**expected[-1], **expected[-1]['semantic']})],
                   budgets=dict(max_memory_bytes=1 << 30))
        scalar = directory / 'scalar.json'
        scalar.write_text(json.dumps(raw))
        census = directory / 'census.json'
        census.write_text('{}')
        calls = []
        def child(command, out, label, **kwargs):
            calls.append(command)
            threads = int(command[3])
            records = self.receipt(threads)
            records[-1]['input_bytes'] = source.stat().st_size
            if mutation:
                mutation(records, threads)
            out.mkdir(parents=True, exist_ok=True)
            output = out / (label + '.stdout')
            output.write_bytes(self.encoded(records))
            return 0, output, out / (label + '.stderr')
        args = ['--registration', str(directory / 'registration'), '--census', str(census),
                '--data', str(directory), '--out', str(directory / 'out'), '--parser', '/headers',
                '--native', '/native', '--scalar-result', str(scalar)]
        selected = [dict(case=dict(test='fixture'), member=dict(sha256='b' * 64), source=source)]
        with patch.object(threading.sys, 'platform', 'linux'), \
                patch.object(threading.hevc_decode, 'streams', return_value=selected), \
                patch.object(threading.conformance, 'digest', return_value='b' * 64), \
                patch.object(threading.conformance, 'child', side_effect=child):
            threading.native_conformance(args)
        return calls

    def test_complete_native_conformance_executes_all_counts_and_the_frame_owner(self):
        with tempfile.TemporaryDirectory() as temp:
            calls = self.conformance(pathlib.Path(temp))
            self.assertEqual([call[3] for call in calls], ['1', '2', '4', '8', '0'])
            self.assertTrue(all(call[2] == 'check' for call in calls))
            result = json.loads((pathlib.Path(temp) / 'out/result.json').read_text())
            self.assertTrue(result['complete'])
            self.assertEqual(result['coverage']['4']['admitted_streams'], 1)
            self.assertEqual(result['coverage']['0']['admitted_streams'], 1)
        mutations = [lambda records, count: records[-1]['semantic']['stats'].update(ctus=3) if count == 4 else None,
                     lambda records, count: records[0].update(timestamp=1) if count == 4 else None,
                     lambda records, count: records[-1].update(admissions=[])]
        for mutate in mutations:
            with self.subTest(mutate=mutate), tempfile.TemporaryDirectory() as temp, self.assertRaises(ValueError):
                self.conformance(pathlib.Path(temp), mutate)

    def test_native_timing_validates_before_calls_and_rechecks_every_timed_work_record(self):
        with tempfile.TemporaryDirectory() as temp:
            source = pathlib.Path(temp) / 'input'
            source.write_bytes(b'fixture')
            commands = []
            changed_work = [False]
            def child(command, **kwargs):
                commands.append(command)
                if command[0] == '/native':
                    records = self.receipt(int(command[3]), check=command[2] == 'check')
                    records[-1]['input_bytes'] = source.stat().st_size
                    if changed_work[0] and command[2] == 'decode':
                        records[-1]['semantic']['stats']['ctus'] += 1
                    return self.encoded(records), dict(wall=1.0, user=0.9, system=0.1)
                return b'frame=1\n', dict(wall=1.0, user=0.9, system=0.1)
            args = types.SimpleNamespace(native='/native', repeats=3)
            oracle = types.SimpleNamespace(ffmpeg='/qualified/ffmpeg')
            verified = dict(frames=1, depth=8, output_md5='a' * 32)
            with patch.object(threading.reference, 'verify', return_value=verified), \
                    patch.object(threading.reference, 'command', side_effect=child):
                row = threading.measure_native(args, oracle, source, 'd00')
            self.assertEqual(row['status'], 'timed')
            self.assertEqual([call[3] for call in commands[:5]], [1, 2, 4, 8, 0])
            self.assertEqual(len(row['warmups']), 5)
            self.assertEqual(len(row['calls']), 15)
            self.assertTrue(all(call[0] in ('/native', oracle.ffmpeg) for call in commands))
            changed_work[0] = True
            with patch.object(threading.reference, 'verify', return_value=verified), \
                    patch.object(threading.reference, 'command', side_effect=child), \
                    self.assertRaisesRegex(ValueError, 'timed_work_mismatch'):
                threading.measure_native(args, oracle, source, 'd00')

    def test_workflows_use_the_real_bridge_and_sanitizer_feature(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        native = (root / '.github/workflows/hevc-native-rows.yml').read_text()
        dispatch = (root / '.github/workflows/hevc-reference.yml').read_text()
        reusable = (root / '.github/workflows/hevc-threading.yml').read_text()
        self.assertIn("inputs.baseline == 'native'", dispatch)
        self.assertIn('uses: ./.github/workflows/hevc-native-rows.yml', dispatch)
        self.assertIn('workflow_call:', native)
        self.assertIn('hevc_threading.py native-reference', native)
        self.assertIn('--features native-hevc,test-support --example reference', native)
        self.assertLess(native.index('git branch -r --contains'), native.index('cargo test'))
        self.assertLess(native.index('git branch -r --contains'), native.index('secrets.'))
        self.assertIn('hevc_threading.py native-conformance', reusable)
        self.assertIn("counts='1 2 4 8'", reusable)
        self.assertIn('if [ -z "$BASELINE" ]; then counts=1; fi', reusable)
        self.assertIn('if [ -n "$BASELINE" ]; then\n            python3 -I scripts/hevc_threading.py compare-conformance', reusable)
        self.assertIn('--features native-hevc-rows-private --lib "$group"', reusable)
        self.assertIn('dependent_raster_job_reads_a_building_predecessor_before_its_first_publication ... ok', reusable)
        self.assertIn('capacity_probe_mismatch_preserves_legacy_frames_and_summary ... ok', reusable)
        self.assertIn('rsa_oaep_md:sha256', native)
        self.assertNotIn('path: ${{ runner.temp }}/private', native)


if __name__ == '__main__':
    unittest.main()
