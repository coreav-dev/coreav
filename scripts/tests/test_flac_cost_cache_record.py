import hashlib
import importlib.util
import json
import math
from pathlib import Path
import statistics
import unittest


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("flac_output_measure", ROOT / "scripts/flac_output_measure.py")
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class CostCacheRecord(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.path = ROOT / "docs/measurements/flac-cost-cache.json"
        cls.report = json.loads(cls.path.read_text())

    def test_complete_capture_and_every_invocation(self):
        report = self.report
        self.assertEqual(hashlib.sha256(self.path.read_bytes()).hexdigest(),
                         "372000c03218e89cc24c7120ae2261d3fb0297778baa3a4afed3b2ceb3c59180")
        self.assertEqual(report["state"], "complete")
        self.assertIsNone(report["failure"])
        self.assertEqual(report["merged_source"], "62f4af2a4a3adb477583f91d396f6daeaebc2055")
        self.assertEqual(report["build"]["merged_source"], report["merged_source"])
        self.assertEqual(report["build"]["tests"], {"candidate": [61, 7], "before": [58, 7]})
        self.assertEqual(report["host"]["machine_type"], "c4-standard-8")
        self.assertEqual(report["host"]["provisioning"], "STANDARD")
        self.assertEqual(report["host"]["cpu_platform"], "Intel Emerald Rapids")
        self.assertEqual(report["host"]["advanced_machine_features"],
                         {"threadsPerCore": 1, "performanceMonitoringUnit": "STANDARD", "turboMode": "ALL_CORE_MAX"})
        self.assertEqual(report["affinity"], [report["logical_cpu"]])
        old = json.loads((ROOT / "docs/measurements/flac-output-round2.json").read_text())
        self.assertEqual([row["case"] for row in report["rows"]], [row["case"] for row in measure.roster()])
        keys = ("case", "class", "bits", "channels", "rate", "input_pcm_sha256")
        self.assertEqual([{key: row[key] for key in keys} for row in report["rows"]],
                         [{key: row[key] for key in keys} for row in old["rows"]])
        expected = [("reference", None, "null", i, value) for i, value in enumerate(report["null_seconds"])]
        rows = {row["case"]: row for row in report["rows"]}
        for index, row in enumerate(report["rows"]):
            self.assertTrue(row["complete"])
            self.assertEqual(len(row["triples"]), 9)
            self.assertEqual(row["candidate_sha256"], row["before_sha256"])
            self.assertEqual(row["bytes"]["candidate"], row["bytes"]["before"])
            expected.extend((arm, row["case"], "warmup", 0, None) for arm in ("candidate", "reference", "before"))
            if index == 0:
                expected.extend(("reference", row["case"], "aa", i, value)
                                for i, pair in enumerate(report["aa_pairs"]) for value in pair)
            for repetition, triple in enumerate(row["triples"]):
                order = ["candidate", "reference"] if repetition % 2 == 0 else ["reference", "candidate"]
                order.insert(repetition % 3, "before")
                self.assertEqual(triple["order"], order)
                self.assertTrue(triple["verified"])
                for arm in order:
                    self.assertEqual(triple[arm + "_exit"], 0)
                    self.assertTrue(math.isfinite(triple[arm]) and triple[arm] > 0)
                    expected.append((arm, row["case"], "triple", repetition, triple[arm]))
        self.assertEqual(len(report["calls"]), 747)
        self.assertEqual(len(expected), 747)
        for call, identity in zip(report["calls"], expected):
            self.assertEqual(tuple(call[key] for key in ("arm", "case", "phase", "repetition", "seconds")), identity)
            self.assertEqual(call["exit"], 0)
            self.assertTrue(call["verified"])
            if call["phase"] == "null":
                self.assertEqual(call["argv"], ["<build>/oracle/ffmpeg", "-version"])
                continue
            case, arm = call["case"], call["arm"]
            prefix = (["<build>/oracle/ffmpeg", "-v", "error", *measure.AUDIO_INPUT] if arm == "reference"
                      else [f"<build>/{arm}/coreav", "--strict-safe", "ffmpeg"])
            self.assertEqual(call["argv"], [*prefix, "-i", f"<scratch>/{case}.mka", "-y", "-map_metadata", "-1",
                                            "-threads", "1", f"<scratch>/{case}-{arm}.flac"])
            self.assertGreater(call["output_bytes"], 0)
            self.assertRegex(call["output_sha256"], r"^[a-f0-9]{64}$")
            if arm != "reference":
                self.assertEqual(call["output_sha256"], rows[case][arm + "_sha256"])
                self.assertEqual(call["output_bytes"], rows[case]["bytes"][arm])

    def test_both_ratio_axes_replay_with_the_unchanged_summary(self):
        for kind, recorded in self.report["analysis"]["statistics"].items():
            rows = [row for row in self.report["rows"] if kind == "overall" or row["class"] == kind]
            for denominator in ("reference", "before"):
                pairs = [{"complete": True, "pairs": [
                    {"native": triple["candidate"], "reference": triple[denominator],
                     "native_exit": triple["candidate_exit"], "reference_exit": triple[denominator + "_exit"],
                     "verified": triple["verified"]} for triple in row["triples"]]} for row in rows]
                self.assertEqual(measure.summary(pairs), {"cases": len(rows), **recorded[denominator]})

    def test_observed_milestone_does_not_override_calibration_or_reference_miss(self):
        report, analysis = self.report, self.report["analysis"]
        self.assertEqual(len(report["null_seconds"]), 9)
        self.assertEqual(len(report["aa_pairs"]), 9)
        self.assertTrue(all(len(pair) == 2 for pair in report["aa_pairs"]))
        aa = statistics.median(a / b for a, b in report["aa_pairs"])
        self.assertEqual(aa, analysis["aa_ratio"])
        self.assertTrue(0.90 <= aa <= 1.10)
        work = {row["case"]: statistics.median(t["reference"] for t in row["triples"])
                >= 5 * statistics.median(report["null_seconds"]) for row in report["rows"]}
        self.assertEqual(work, analysis["work_dominates_startup"])
        self.assertEqual({case for case, passes in work.items() if not passes},
                         {f"{kind}-16-1-{rate}" for kind in ("polynomial", "noise") for rate in (44100, 48000)})
        self.assertFalse(analysis["calibration_pass"])
        self.assertTrue(all(group["before"]["median_ratio"] <= 0.60 for group in analysis["statistics"].values()))
        self.assertTrue(analysis["coreav_milestone_observed"])
        self.assertFalse(analysis["coreav_milestone"])
        self.assertFalse(analysis["reference_speed_bar_observed"])
        self.assertFalse(analysis["reference_speed_bar"])
        ratio = sum(row["bytes"]["candidate"] for row in report["rows"]) / sum(
            row["bytes"]["reference"] for row in report["rows"])
        self.assertEqual(ratio, analysis["size_ratio"])
        self.assertLessEqual(ratio, 1.05)
        self.assertTrue(all(row["bytes"]["candidate"] <= 1.10 * row["bytes"]["reference"]
                            for row in report["rows"] if row["class"] != "noise"))
        self.assertTrue(analysis["size_bar"])
        self.assertEqual(analysis["timed_triples"], 216)


if __name__ == "__main__":
    unittest.main()
