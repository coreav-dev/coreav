"""Negative evidence-boundary cases for the round-10 driver."""
import copy
import json
import pathlib
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import test_b4_round10 as fixture
import test_b4_sources as source_fixture

b4 = fixture.b4


def chronological(calls):
    end = 0
    for row in calls:
        row['start'] = end + 0.01
        row['end'] = row['start'] + row.get('elapsed', 1) + 0.01
        end = row['end']
    return calls


def metadata(root):
    rows = {name: [] for name in b4.MANIFESTS}
    for phase, entries in fixture.rosters().items():
        for row in entries:
            corpus = b4.speed.CLASSES[row['class']][0]
            rows[corpus].append({**row, 'set': 'real', 'holdout': phase == 'product'})
    for name, entries in rows.items():
        path = root / 'corpus' / name / 'manifest.json'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({'files': entries}))
    return rows


class FallbackCapture(unittest.TestCase):
    def test_deleted_log_cannot_become_a_clean_capture_or_pass_calibration(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = pathlib.Path(temporary) / 'fallback.jsonl'
            path.write_text('{"ran":"ffprobe"}\n')
            path.unlink()
            counts = b4.fallback(path)
        self.assertTrue(counts['void'])
        data = fixture.report_data()
        data['fallback_log'] = counts
        try:
            result = b4.summary(data)
        except ValueError:
            return
        self.assertFalse(result['passes'])


class GlobalReceipts(unittest.TestCase):
    def test_complete_global_product_schedule_retains_the_registered_verdict(self):
        data = fixture.report_data('product')
        schedule = b4.protocol.product_schedule(data['selection']['product'], data['commands'])
        product = fixture.calls_for(schedule)
        for row in product:
            row['elapsed'] = 0.5 if row['side'] == 'A' else 1
        data['receipts'] = chronological(data['receipts'] + product)
        with mock.patch.object(b4.speed, 'interval', return_value=[0.5, 0.5]):
            result = b4.summary(data)
        self.assertTrue(result['b4_passes'])
        self.assertEqual(result['product']['coverage']['timed'], len(schedule) // 9)
        self.assertEqual(result['product']['coverage']['missing'], 0)

    def test_product_before_calibration_cannot_pass_even_with_increasing_clocks(self):
        data = fixture.report_data('product')
        product = fixture.calls_for(b4.protocol.product_schedule(data['selection']['product'], data['commands']))
        for row in product:
            row['elapsed'] = 0.5 if row['side'] == 'A' else 1
        data['receipts'] = chronological(product + data['receipts'])
        with mock.patch.object(b4.speed, 'interval', return_value=[0.5, 0.5]):
            with self.assertRaises(ValueError):
                b4.summary(data)

    def test_product_case_reordering_is_not_an_incomplete_suffix(self):
        data = fixture.report_data('product')
        schedule = b4.protocol.product_schedule(data['selection']['product'], data['commands'])
        first = fixture.calls_for(schedule[:9])
        second = fixture.calls_for(schedule[9:18])
        data['receipts'] = chronological(data['receipts'] + second + first)
        with mock.patch.object(b4.speed, 'interval', return_value=None):
            with self.assertRaises(ValueError):
                b4.summary(data)

    def test_overlapping_or_backwards_call_boundaries_are_rejected(self):
        for delta in (0, -1):
            data = fixture.report_data()
            chronological(data['receipts'])
            data['receipts'][1]['start'] = data['receipts'][0]['start'] + delta
            with self.subTest(delta=delta), self.assertRaises(ValueError):
                b4.summary(data)

    def test_valid_prefix_and_registered_refusal_skip_remain_reportable(self):
        data = fixture.report_data('product')
        schedule = b4.protocol.product_schedule(data['selection']['product'], data['commands'])
        refused = fixture.calls_for(schedule[:1])
        refused[0]['exit'] = 4
        next_case = fixture.calls_for(schedule[9:18])
        data['receipts'] = chronological(data['receipts'] + refused + next_case)
        with mock.patch.object(b4.speed, 'interval', return_value=None):
            result = b4.summary(data)
        self.assertEqual(result['product']['coverage']['refused'], 1)
        self.assertEqual(result['product']['coverage']['timed'], 1)
        self.assertFalse(result['passes'])


class MetadataBinding(unittest.TestCase):
    def test_mutation_after_parsing_cannot_be_receipted_as_the_selected_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            rows = metadata(root)
            target = root / 'corpus/v1/manifest.json'
            selected = rows['v1'][0]['sha256']
            original = json.loads
            changed = False
            def parse(raw, *args, **kwargs):
                nonlocal changed
                data = original(raw, *args, **kwargs)
                if not changed and isinstance(data, dict) and data.get('files', [{}])[0].get('sha256') == selected:
                    changed = True
                    replacement = copy.deepcopy(data)
                    replacement['files'][0]['holdout'] = True
                    target.write_text(json.dumps(replacement))
                return data
            with mock.patch.object(b4.json, 'loads', side_effect=parse):
                with self.assertRaisesRegex(ValueError, 'metadata'):
                    b4.select_rosters(root)
            self.assertTrue(changed)

    def test_freeze_rechecks_metadata_after_source_and_binary_preflight(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            metadata(root)
            selected, _ = b4.select_rosters(root)
            historical = root / 'historical.json'
            historical.write_text(json.dumps([{key: row[key] for key in ('class', 'sha256', 'ext', 'size')}
                                              for row in selected['product']]))
            binary = root / 'coreav'
            binary.write_bytes(b'synthetic executable identity')
            for name in ('ffmpeg', 'ffprobe'):
                (root / name).write_bytes(binary.read_bytes())
            source = root / 'sources.json'
            source.write_text(json.dumps({'builds': {name: {'sha256': b4.digest(binary)}
                                                     for name in ('candidate', 'baseline')}}))
            opts = types.SimpleNamespace(manifests=root, heldout_roster=[], product_roster=historical,
                                         sources=source, coreav=binary, baseline_coreav=binary,
                                         oracle_bin=root, out=root / 'frozen.json')
            def mutate(_):
                target = root / 'corpus/v1/manifest.json'
                target.write_text(target.read_text() + ' ')
            with mock.patch.object(b4, 'verify_sources', side_effect=mutate), \
                    mock.patch.object(b4, 'checked'), mock.patch.object(b4.subprocess, 'run'):
                with self.assertRaisesRegex(ValueError, 'metadata'):
                    b4.freeze(opts)
            self.assertFalse(opts.out.exists())


class RequiredIntegration(unittest.TestCase):
    def verify(self, checks):
        source = source_fixture.Sources()
        source.setUp()
        source.checks = checks
        def checked(command):
            if command[-1].endswith('rules/branches/main'):
                return json.dumps([{'type': 'required_status_checks', 'parameters': {
                    'required_status_checks': [{'context': 'required', 'integration_id': 123}]}}])
            return source.checked(command)
        with mock.patch.object(b4, 'checked', side_effect=checked), mock.patch.object(b4.subprocess, 'run'):
            b4.verify_sources(source.sources)

    def test_wrong_or_missing_app_cannot_satisfy_a_required_integration(self):
        for app in ({'id': 456}, {}):
            with self.subTest(app=app), self.assertRaisesRegex(ValueError, 'CI'):
                self.verify([{'id': 2, 'name': 'required', 'status': 'completed',
                              'conclusion': 'success', 'app': app}])

    def test_newer_unrelated_success_cannot_hide_the_required_app_failure(self):
        with self.assertRaisesRegex(ValueError, 'CI'):
            self.verify([{'id': 1, 'name': 'required', 'status': 'completed', 'conclusion': 'failure', 'app': {'id': 123}},
                         {'id': 2, 'name': 'required', 'status': 'completed', 'conclusion': 'success', 'app': {'id': 456}}])

    def test_newer_unrelated_failure_does_not_replace_the_required_app_success(self):
        self.verify([{'id': 1, 'name': 'required', 'status': 'completed', 'conclusion': 'success', 'app': {'id': 123}},
                     {'id': 2, 'name': 'required', 'status': 'completed', 'conclusion': 'failure', 'app': {'id': 456}}])


if __name__ == '__main__':
    unittest.main()
