"""Validate the closed synthetic roster without invoking an encoder."""

import importlib.util
import io
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('svt_av1_cases', ROOT / 'scripts/svt_av1_cases.py')
CASES = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CASES
SPEC.loader.exec_module(CASES)


class SvtCases(unittest.TestCase):
    def test_native_positive_roster_covers_each_admitted_colour_axis(self):
        cases = [case for case in CASES.load() if case.outcome == 'complete']
        for axis, expected in enumerate([
            {1, 2, *range(4, 13), 22},
            {1, 2, *range(4, 16), 17},
            {1, 2, *range(4, 15)},
            {0, 1},
            {0, 1, 2},
        ]):
            with self.subTest(axis=axis):
                self.assertEqual({case.color[axis] for case in cases}, expected)

    def changed(self, change):
        document = json.loads(CASES.ROSTER.read_text())
        change(document)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'cases.json'
            path.write_text(json.dumps(document))
            with self.assertRaises(ValueError):
                CASES.load(path)

    def test_roster_has_all_presets_and_both_segment_bounds(self):
        cases = CASES.load()
        self.assertEqual(len(cases), 97)
        presets = [c.preset for c in cases if c.id.startswith('preset-')]
        self.assertEqual(sorted(presets), list(range(14)))
        for prefix, capacity in [('memory-', 3), ('keyint-', 5)]:
            rows = [c for c in cases if c.id.startswith(prefix)]
            self.assertEqual(sorted(c.pictures for c in rows),
                             [capacity - 1, capacity, capacity + 1, 2 * capacity - 1, 2 * capacity,
                              2 * capacity + 1, 3 * capacity + 1])
            self.assertEqual({c.capacity for c in rows}, {capacity})
        fractional, = [c for c in cases if c.id == 'fractional-tail']
        self.assertEqual((fractional.rate, fractional.keyint, fractional.pictures),
                         ((30000, 1001), 149, 150))
        normal = [case for case in cases if case.id.startswith('normal-')]
        self.assertEqual(sorted(case.pictures for case in normal),
                         [149, 150, 151, 299, 300, 301, 451])
        self.assertEqual({case.capacity for case in normal}, {150})
        self.assertEqual({case.crf for case in cases}, {0, 1, 32, 62, 63})
        rows = {case.id: case for case in cases}
        self.assertEqual((rows['one-picture-floor-one'].pictures,
                          rows['one-picture-floor-one'].capacity), (1, 1))
        self.assertEqual(rows['maximum-width'].dimensions, (16384, 64))
        self.assertEqual(rows['maximum-height'].dimensions, (64, 8704))
        self.assertEqual(rows['rate-sixty'].rate, (60, 1))
        self.assertEqual(rows['rate-equivalent'].rate, (60, 2))

    def test_unknown_duplicate_and_incomplete_fields_are_refused(self):
        for change in [
            lambda d: d.update(extra=True),
            lambda d: d.update(version=True),
            lambda d: d['defaults'].update(extra=True),
            lambda d: d['cases'].append(d['cases'][0]),
            lambda d: d['cases'][0].update(extra=True),
            lambda d: d['cases'][0].pop('mode'),
            lambda d: d['cases'][0].update(id='../elsewhere'),
            lambda d: d['cases'][0].update(mode='ignore'),
            lambda d: d['cases'][0].update(outcome='complete'),
            lambda d: d['cases'][0].update(pictures=True),
            lambda d: d['cases'][0].update(capacity=0),
            lambda d: d['defaults'].update(strides=[65, 32, 32]),
            lambda d: d['defaults'].update(color=[1, 16, 1, 0, 0]),
            lambda d: d['defaults'].update(pattern='new-pattern'),
            lambda d: d['cases'][1].update(steps=[2]),
            lambda d: d['cases'][1].update(origin=(1 << 63)),
        ]:
            with self.subTest(change=change):
                self.changed(change)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'duplicate.json'
            path.write_text('{"version":1,"version":1}')
            with self.assertRaisesRegex(ValueError, 'duplicate'):
                CASES.load(path)

    def test_exact_worker_wire_and_distinct_picture_pattern(self):
        one, = [c for c in CASES.load() if c.id == 'one']
        output = io.BytesIO()
        CASES.write_input(output, one)
        wire = output.getvalue()
        self.assertEqual(len(wire), 60 + 17 + 6144 + 9)
        self.assertEqual(wire[:8], b'AV1IN\0\x01\0')
        self.assertEqual(struct.unpack_from('<IIII', wire, 8), (64, 64, 30, 1))
        self.assertEqual(tuple(wire[24:32]), (8, 32, 2, 2, 2, 0, 0, 0))
        self.assertEqual(struct.unpack_from('<BQq', wire, 60), (1, 0, 0))
        row = b''.join(bytes([value]) * 8 for value in [224, 32, 224, 32, 32, 224, 32, 224])
        self.assertEqual(wire[77:77 + 4096], row * 64)
        self.assertEqual(wire[77 + 4096:-9], b'\x80' * 2048)
        self.assertEqual(struct.unpack_from('<BQ', wire, len(wire) - 9), (2, 1))
        pictures = [bytes(CASES.picture(one, i)) for i in range(151)]
        self.assertEqual(len(set(pictures)), 151)
        self.assertEqual(pictures[1][0:8], b'\x20' * 8)
        row = b''.join(bytes([value]) * 8 for value in [32, 32, 224, 32, 224, 32, 32, 32])
        self.assertEqual(pictures[1][:64], row)
        for previous, current in zip(pictures, pictures[1:]):
            self.assertGreater(sum(a != b for a, b in zip(previous[:4096], current[:4096])),
                               4096 // 4)

    def test_variable_fractional_and_nonzero_clocks_cross_segment_boundaries_unchanged(self):
        rows = {case.id: case for case in CASES.load()}
        for name, times in [
            ('variable-clock-tail', [90000, 90001, 90003, 90004, 90007, 90008, 90010]),
            ('negative-origin-tail', [-90000 + i for i in range(7)]),
        ]:
            case = rows[name]
            output = io.BytesIO()
            CASES.write_input(output, case)
            wire = output.getvalue()
            for identity, pts in enumerate(times):
                self.assertEqual(struct.unpack_from('<BQq', wire, 60 + identity * (17 + 6144)),
                                 (1, identity, pts))
        fractional = rows['fractional-tail']
        self.assertEqual((fractional.clock, fractional.steps), ((1, 30000), (1001,)))


if __name__ == '__main__':
    unittest.main()
