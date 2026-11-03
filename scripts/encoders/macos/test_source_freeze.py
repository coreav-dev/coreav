"""Synthetic immutable-source and two-stage seal regressions only."""

import copy
import unittest
from unittest import mock

import evidence
import registration
import source_freeze


def freeze_fixture(reg):
    h = lambda text: evidence.sha(text.encode())
    inputs = []
    for item in reg["inputs"]:
        inputs.append({**{key: item[key] for key in source_freeze.INPUT_FIELDS}, "source_bytes": 100000,
                       "preparation": {"source_pixel_format": "yuv420p",
                                       "source_colour": {"color_primaries": "bt709", "color_transfer": "bt709",
                                                         "color_space": "bt709", "color_range": "tv"},
                                       "assumptions": [], "container_name": "mov", "field_order": "progressive",
                                       "oracle_sha256": {name: reg["metric_tools"][name] for name in ("ffmpeg", "ffprobe")}},
                       "source_metadata": {"codec_name": "h264", "pixel_format": "yuv420p",
                                           "field_order": "progressive", "declared_frames": 300,
                                           "observed_colour": {"color_primaries": "bt709", "color_transfer": "bt709",
                                                               "color_space": "bt709", "color_range": "tv"},
                                           "audio_starts": [], "scan": {
                                               "sps": [{"id": 0, "frame_mbs_only_flag": 1, "profile_idc": 100,
                                                        "level_idc": 40}],
                                               "pps": [{"id": 0, "sps_id": 0}], "packet_count": 300,
                                               "slice_count": 300}},
                       "capture": {name: h(name) for name in ("row_sha256", "header_sha256", "packets_sha256",
                                   "frames_sha256", "mapping_sha256", "scan_sha256", "preparation_sha256")}})
    candidates = [{"id": item["id"], "class": item["class"], "disposition": "verified",
                   "reason": None, "evidence_sha256": h("verified")} for item in inputs]
    candidates += [{"id": h(f"excluded-{n}"), "class": 0, "disposition": "header_refused",
                    "reason": "synthetic_header_rule", "evidence_sha256": h("excluded")}
                   for n in range(407 - len(candidates))]
    candidates.sort(key=lambda row: (row["class"], h(source_freeze.SALT + row["id"])))
    selection = {"schema": "coreav.videotoolbox-selection/2", "candidates": candidates}
    uncovered = copy.deepcopy(reg["uncovered_classes"])
    uncovered[0]["failure_ledger"].pop("review_commit")
    frozen = {"schema": "coreav.videotoolbox-source-freeze/1", "scope": registration.SCOPE,
              "covered_classes": list(registration.COVERED_CLASSES), "uncovered_classes": uncovered,
              "plan_sha256": reg["plan_sha256"], "tools": reg["tools"],
              "native_inputs": {"Cargo.toml": h("native")},
              "settings": reg["settings"], "environment": reg["environment"],
              "command_controls": reg["command_controls"], "commands": reg["commands"],
              "host_profile": dict(registration.HOST),
              "oracle_sha256": {name: reg["metric_tools"][name] for name in ("ffmpeg", "ffprobe")},
              "metric_model": {"name": "vmaf_v0.6.1", "sha256": reg["metric_tools"]["model"]},
              "verification": {"method_sha256": h("method"), "plan_sha256": h("old-plan"),
                               "membership_sha256": h("\n".join(sorted(row["id"] for row in candidates)))},
              "selection": {key: reg["eligibility"][key] for key in ("file", "sha256")},
              "inputs": inputs, "fixtures": source_freeze.fixtures()}
    return frozen, selection


def uncovered_fixture():
    reasons = [None] + ["missing_prefix_pictures_or_packet_metadata"] * 5 + ["prefix_picture_duration_not_cfr"] * 5
    reasons += ["trace_warning_or_error_before_sanitization"] * 4 + ["unsupported_codec_progressive_header_reader"] * 7
    return {"schema": "coreav.videotoolbox-uncovered/1", "class": 2160,
            "required_inputs": 10, "verified_inputs": 1,
            "rows": [{"id": evidence.sha(str(index).encode()), "evidence_sha256": evidence.sha(b"proof"),
                      "disposition": "verified" if reason is None else
                                     "reader_unsupported" if reason.startswith("unsupported") else "failed",
                      "reason": reason} for index, reason in enumerate(reasons)]}


class SourceFreeze(unittest.TestCase):
    def setUp(self):
        from test_tools import registered
        self.reg = registered()
        self.frozen, self.selection = freeze_fixture(self.reg)

    def test_public_freeze_and_its_selection_match_the_current_tools(self):
        root = registration.ROOT
        directory = root / "scripts/encoders/macos"
        frozen = source_freeze.validate(evidence.load(directory / "videotoolbox-sources.json"))
        selection = evidence.load(root / frozen["selection"]["file"])
        source_freeze.validate_selection(selection, frozen)
        uncovered = frozen["uncovered_classes"][0]["failure_ledger"]
        source_freeze.validate_uncovered(evidence.load(root / uncovered["file"]), selection)
        for descriptor in (frozen["selection"], uncovered):
            self.assertEqual(evidence.file_hash(root / descriptor["file"]), descriptor["sha256"])
        self.assertEqual(evidence.file_hash(root / registration.PLAN), frozen["plan_sha256"])
        for name, expected in frozen["tools"].items():
            self.assertEqual(evidence.file_hash(root / name), expected, name)

    def test_complete_prepared_sources_and_first_success_order(self):
        source_freeze.validate(self.frozen)
        source_freeze.validate_selection(self.selection, self.frozen)
        self.assertEqual(len(self.frozen["inputs"]), 30)
        self.assertEqual(self.frozen["scope"], "prepared-raw-i420-encoder-only")

    def test_source_coverage_clock_preparation_and_fixture_mutations_refuse(self):
        mutations = [lambda r: r["inputs"].pop(), lambda r: r["inputs"].reverse(),
                     lambda r: r["covered_classes"].append(2160),
                     lambda r: r["uncovered_classes"][0].update(verified_inputs=True),
                     lambda r: r["inputs"][0].update(**{"class": 2160}),
                     lambda r: r["inputs"][0].update(frames=240),
                     lambda r: r["inputs"][0]["rate"].update(num=True),
                     lambda r: r["inputs"][0]["source_frame_sha256"].pop(),
                     lambda r: r["inputs"][1].update(raw_sha256=r["inputs"][0]["raw_sha256"]),
                     lambda r: r["inputs"][0]["preparation"].update(field_order="unknown"),
                     lambda r: r["inputs"][0]["source_metadata"]["scan"]["sps"][0].update(frame_mbs_only_flag=0),
                     lambda r: r["inputs"][0]["source_metadata"]["scan"]["sps"][0].update(frame_mbs_only_flag=True),
                     lambda r: r["inputs"][0]["source_metadata"]["scan"]["pps"][0].update(sps_id=1),
                     lambda r: r["inputs"][0]["source_metadata"]["observed_colour"].pop("color_range"),
                     lambda r: r["inputs"][0]["capture"].pop("scan_sha256"),
                     lambda r: r["settings"].update(warmups=True),
                     lambda r: r["fixtures"]["fixture"].update(frames=4)]
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                value = copy.deepcopy(self.frozen)
                mutate(value)
                with self.assertRaises(evidence.Invalid):
                    source_freeze.validate(value)

    def test_missing_colour_defaults_are_not_observations(self):
        item = self.frozen["inputs"][0]
        item["preparation"]["assumptions"] = [{"field": "color_primaries", "assumed_value": "bt709",
                                                "observed": False, "rule": "absent_tag_bt709"}]
        item["source_metadata"]["observed_colour"].pop("color_primaries")
        source_freeze.validate(self.frozen)
        item["source_metadata"]["observed_colour"]["color_primaries"] = "bt709"
        with self.assertRaises(evidence.Invalid):
            source_freeze.validate(self.frozen)

    def test_membership_failure_prefix_duplicate_and_omission_refuse(self):
        for mutate in (lambda r: r["candidates"].pop(),
                       lambda r: r["candidates"].__setitem__(1, copy.deepcopy(r["candidates"][0])),
                       lambda r: r["candidates"].reverse(),
                       lambda r: next(row for row in r["candidates"] if row["class"] == 360).update(disposition="unvisited")):
            value = copy.deepcopy(self.selection)
            mutate(value)
            with self.assertRaises(evidence.Invalid):
                source_freeze.validate_selection(value, self.frozen)

    def test_uncovered_ledger_cannot_be_a_fourth_passing_class(self):
        value = uncovered_fixture()
        selection = {"candidates": [{**row, "class": 2160, "disposition":
                     "header_refused" if row["disposition"] == "reader_unsupported" else row["disposition"]}
                                    for row in value["rows"]]}
        source_freeze.validate_uncovered(value, selection)
        for mutate in (lambda r: r.update(verified_inputs=10), lambda r: r.update(verified_inputs=True),
                       lambda r: r["rows"].pop(), lambda r: r["rows"][1].update(reason="verified"),
                       lambda r: r["rows"][-1].update(disposition="failed"),
                       lambda r: r["rows"][0].update(id=evidence.sha(b"nonmember")),
                       lambda r: r["rows"].__setitem__(1, copy.deepcopy(r["rows"][0]))):
            changed = copy.deepcopy(value)
            mutate(changed)
            with self.assertRaises(evidence.Invalid):
                source_freeze.validate_uncovered(changed, selection)

    def test_seal_binds_all_immutable_content_but_allows_materialized_locations(self):
        def git(_root, operation, *args):
            self.assertEqual(operation, "show")
            return b"tool" if args[0].endswith(":tool.py") else b"native"
        with mock.patch.object(source_freeze, "read_stage1", return_value=self.frozen), \
                mock.patch.object(source_freeze, "native_paths", return_value={"Cargo.toml"}), \
                mock.patch.object(registration, "git", side_effect=git):
            source_freeze.verify_seal(self.reg, ".")
            moved = copy.deepcopy(self.reg)
            moved["inputs"][0]["raw_file"] = "materialized/source.yuv"
            source_freeze.verify_seal(moved, ".")
            mutations = [lambda r: r["inputs"][0].update(raw_sha256=evidence.sha(b"changed")),
                         lambda r: r["inputs"][0].update(source_start_ticks=1),
                         lambda r: r["inputs"][0]["source_frame_sha256"].__setitem__(0, evidence.sha(b"changed")),
                         lambda r: r["tools"].update(**{"other.py": evidence.sha(b"other")}),
                         lambda r: r["metric_tools"].update(model=evidence.sha(b"other-model")),
                         lambda r: r["metric_tools"].update(ffmpeg=evidence.sha(b"other-oracle")),
                         lambda r: r["uncovered_classes"][0]["failure_ledger"].update(review_commit="a" * 40)]
            for mutate in mutations:
                changed = copy.deepcopy(self.reg)
                mutate(changed)
                with self.assertRaises(evidence.Invalid):
                    source_freeze.verify_seal(changed, ".")
            with mock.patch.object(registration, "git", return_value=b"changed-native"), \
                    self.assertRaises(evidence.Invalid):
                source_freeze.verify_seal(self.reg, ".")

    def test_absent_or_placeholder_freeze_never_passes_full_registration(self):
        registration.validate(self.reg)
        for mutate in (lambda r: r.pop("source_freeze"),
                       lambda r: r["source_freeze"].pop("sha256"),
                       lambda r: r["source_freeze"].update(review_commit="pending")):
            changed = copy.deepcopy(self.reg)
            mutate(changed)
            with self.assertRaises(evidence.Invalid):
                registration.validate(changed)


if __name__ == "__main__":
    unittest.main()
