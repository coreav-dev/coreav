import copy
import importlib.util
import os
from pathlib import Path
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "flac_output_measure.py"
spec = importlib.util.spec_from_file_location("flac_output_measure", SCRIPT)
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class FlacMeasurement(unittest.TestCase):
    def test_judged_inputs_require_main_dispatch_and_the_exact_event_checkout(self):
        valid = {"GITHUB_ACTIONS": "true", "GITHUB_EVENT_NAME": "workflow_dispatch",
                 "GITHUB_REF": "refs/heads/main", "GITHUB_REPOSITORY": "coreav-dev/coreav",
                 "GITHUB_SHA": "a" * 40}
        for key, replacement in [("GITHUB_ACTIONS", "false"), ("GITHUB_EVENT_NAME", "pull_request"),
                                 ("GITHUB_REF", "refs/heads/feature"), ("GITHUB_REPOSITORY", "other/repo"),
                                 ("GITHUB_SHA", "b" * 40)]:
            with self.subTest(key=key), patch.dict(os.environ, {**valid, key: replacement}, clear=True), \
                    patch.object(measure, "run", return_value=("a" * 40 + "\n").encode()):
                with self.assertRaises(RuntimeError):
                    measure.require_main()
        with patch.dict(os.environ, valid, clear=True), \
                patch.object(measure, "run", return_value=("a" * 40 + "\n").encode()):
            self.assertEqual(measure.require_main(), "a" * 40)

    def test_roster_is_complete_and_smoke_generation_is_deterministic(self):
        rows = measure.roster()
        self.assertEqual(len(rows), 24)
        self.assertEqual(len({row["case"] for row in rows}), 24)
        self.assertTrue(all(not row["complete"] and not row["pairs"] for row in rows))
        for kind in ("tones", "polynomial", "noise"):
            row = {"class": kind, "bits": 24, "channels": 2, "rate": 32}
            result = measure.samples(row, seconds=1)
            self.assertEqual(len(result), 32 * 2 * 3)
            self.assertEqual(result, measure.samples(row, seconds=1))

    def test_missing_failed_unverified_and_zero_time_pairs_cannot_be_statistics(self):
        good = {"complete": True, "pairs": [{"native": 2.0, "reference": 1.0,
                "native_exit": 0, "reference_exit": 0, "verified": True} for _ in range(9)]}
        bad = []
        short = copy.deepcopy(good)
        short["pairs"].pop()
        bad.append(short)
        for key, value in [("verified", False), ("native_exit", 1), ("reference_exit", None),
                           ("native", 0), ("reference", float("nan"))]:
            row = copy.deepcopy(good)
            row["pairs"][0][key] = value
            bad.append(row)
        for row in bad:
            with self.assertRaises(RuntimeError):
                measure.summary([row])
        stats = measure.summary([good])
        self.assertEqual(stats["median_ratio"], 2.0)
        self.assertEqual(stats["interval_95"], [2.0, 2.0])
        self.assertEqual(stats["share_faster"], 0.0)



class OracleSelection(unittest.TestCase):
    def test_smoke_executes_and_validates_the_owner_returned_pair(self):
        import contextlib
        import io
        import sys
        import tempfile
        import test_oracle
        pair = test_oracle.Oracle(Path('/qualified/ffmpeg'), Path('/qualified/ffprobe'))
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'report.json'
            argv = ['flac_output_measure', '--coreav', 'coreav', '--oracle-dir', '/requested',
                    '--out', str(out), '--smoke']
            with patch.object(sys, 'argv', argv), patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}), \
                    patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                    patch.object(measure, 'make_source', return_value=Path('source')) as source, \
                    patch.object(measure, 'run', return_value=b'') as run, \
                    patch.object(measure, 'validate') as validate, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(measure.main(), 0)
            qualify.assert_called_once_with(Path('/requested/ffmpeg'), Path('/requested/ffprobe'))
            self.assertTrue(all(call.args[0] == pair.ffmpeg for call in source.call_args_list))
            self.assertEqual([call.args[0][0] for call in run.call_args_list][1::2],
                             [str(pair.ffmpeg)] * 3)
            self.assertTrue(all(call.args[:2] == (pair.ffmpeg, pair.ffprobe)
                                for call in validate.call_args_list))

    def test_pin_failure_retains_only_an_aborted_report(self):
        import json
        import sys
        import tempfile
        import test_oracle
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'report.json'
            argv = ['flac_output_measure', '--coreav', 'coreav', '--oracle-dir', '/requested',
                    '--out', str(out), '--smoke']
            with patch.object(sys, 'argv', argv), patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}), \
                    patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('private pin detail')), \
                    patch.object(measure, 'make_source') as source, patch.object(measure, 'run') as run:
                with self.assertRaisesRegex(RuntimeError, '^oracle verification failed$'):
                    measure.main()
            source.assert_not_called()
            run.assert_not_called()
            report = json.loads(out.read_text())
            self.assertEqual(report, {'state': 'aborted', 'rows': [], 'judged': False, 'error': 'RuntimeError'})


if __name__ == "__main__":
    unittest.main()
