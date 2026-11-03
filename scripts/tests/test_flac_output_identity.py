import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

SCRIPT = Path(__file__).resolve().parents[1] / "flac_output_identity.py"
spec = importlib.util.spec_from_file_location("flac_output_identity", SCRIPT)
identity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(identity)
NAME = "s16-c1-n1-p0.flac"


class FlacIdentity(unittest.TestCase):
    def test_roster_rejects_empty_duplicate_replaced_and_unsafe_cases(self):
        self.assertEqual(identity.roster((NAME + "\n").encode()), [NAME])
        for data in (b"", (NAME + "\n" + NAME).encode(), b"../source.flac", b"\xff",
                     NAME.replace("c1", "c9").encode(), NAME.replace("p0", "p8").encode()):
            with self.subTest(data=data), self.assertRaises(identity.IdentityFailure):
                identity.roster(data)

    def test_complete_bytes_are_compared_even_when_digests_match(self):
        with tempfile.TemporaryDirectory() as root:
            left, right = Path(root) / "left", Path(root) / "right"
            left.mkdir()
            right.mkdir()
            (left / NAME).write_bytes(b"fLaC\x00\x01")
            (right / NAME).write_bytes(b"fLaC\x00\x01")
            self.assertTrue(identity.compare(left, right, [NAME])[0]["equal"])
            (right / NAME).write_bytes(b"fLaC\x00\x02")
            with patch.object(identity.hashlib, "sha256", return_value=Mock(hexdigest=lambda: "same")):
                row = identity.compare(left, right, [NAME])[0]
            self.assertEqual(row["reference_sha256"], row["candidate_sha256"])
            self.assertFalse(row["equal"])

    def test_missing_extra_empty_and_aliased_files_fail(self):
        with tempfile.TemporaryDirectory() as root:
            left, right = Path(root) / "left", Path(root) / "right"
            left.mkdir()
            right.mkdir()
            (left / NAME).write_bytes(b"fLaCdata")
            with self.assertRaises(identity.IdentityFailure):
                identity.compare(left, right, [NAME])
            (right / NAME).write_bytes(b"fLaCdata")
            extra = right / "extra.flac"
            extra.write_bytes(b"fLaCdata")
            with self.assertRaises(identity.IdentityFailure):
                identity.compare(left, right, [NAME])
            extra.unlink()
            (right / NAME).write_bytes(b"")
            with self.assertRaises(identity.IdentityFailure):
                identity.compare(left, right, [NAME])
            (right / NAME).unlink()
            (right / NAME).symlink_to(left / NAME)
            with self.assertRaises(identity.IdentityFailure):
                identity.compare(left, right, [NAME])

    def test_process_errors_are_bounded_and_do_not_expose_arguments(self):
        errors = [OSError("credential-marker"),
                  subprocess.CalledProcessError(1, ["credential-marker"], stderr=b"secret"),
                  subprocess.TimeoutExpired(["credential-marker"], 180, stderr=b"secret")]
        for error in errors:
            with patch.object(identity.subprocess, "run", side_effect=error) as run:
                with self.assertRaisesRegex(identity.IdentityFailure, "^reference-generation$"):
                    identity.run(Path("producer"), "output", "reference-generation")
                self.assertEqual(run.call_args.kwargs["timeout"], 180)

    def test_capture_records_success_changed_bytes_and_incomplete_runs(self):
        for failure in (None, "changed", "missing", "roster", "process"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root:
                root = Path(root)
                reference, candidate, producer = root / "reference", root / "candidate", root / "producer"
                for path in (reference, candidate, producer):
                    path.write_bytes(b"fixture source")
                output = root / "report.json"

                def run(binary, argument, stage):
                    if stage.endswith("roster"):
                        if failure == "roster" and binary == candidate:
                            return NAME.replace("n1", "n2").encode()
                        return NAME.encode()
                    if failure == "process":
                        raise identity.IdentityFailure("reference-generation")
                    argument.mkdir()
                    if failure != "missing" or binary == reference:
                        suffix = b"changed" if failure == "changed" and binary == candidate else b"same"
                        (argument / NAME).write_bytes(b"fLaC" + suffix)
                    return b""

                with patch.object(identity, "run", side_effect=run):
                    passed = identity.capture(reference, candidate, "a" * 40, producer, output)
                report = json.loads(output.read_text())
                self.assertEqual(passed, failure is None)
                self.assertEqual(report["complete"], failure in (None, "changed"))
                self.assertEqual(report["equal"], failure is None)
                self.assertEqual(report["reference_commit"], identity.BASELINE)
                if failure == "changed":
                    self.assertEqual(report["failure"], "changed-bytes")
                    self.assertFalse(report["cases"][0]["equal"])

    def test_workflow_uses_one_pinned_baseline_and_one_fixture_producer(self):
        workflow = (SCRIPT.parents[1] / ".github/workflows/flac-output.yml").read_text()
        self.assertIn("baseline=$(python3 -I scripts/flac_output_identity.py --baseline)", workflow)
        self.assertIn('git worktree add --detach "$reference" "$baseline"', workflow)
        self.assertIn('test "$(git -C "$reference" rev-parse HEAD)" = "$baseline"', workflow)
        self.assertIn('cp crates/coreav-audio/examples/flac_identity.rs "$reference/crates/coreav-audio/examples/flac_identity.rs"', workflow)
        self.assertEqual(workflow.count("cargo build --locked --release"), 3)
        self.assertIn("${{ runner.temp }}/flac-identity.json", workflow)
        self.assertIn("- scripts/flac_output_identity.py", workflow)
        self.assertIn("- scripts/tests/test_flac_output_identity.py", workflow)


if __name__ == "__main__":
    unittest.main()
