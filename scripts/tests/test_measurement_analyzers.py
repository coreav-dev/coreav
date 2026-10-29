"""No new file named analyze*.py under docs/measurements: a round's bars go through scripts/h264_parity_bars.py.

Each round of the H.264 speed work used to copy the previous round's `analyze.py` beside its reports, so a fix to
how an analyzer reads a report (a stream the harness failed on, missing from the rows, still passed the bars) reached
no later round. The copies already committed are the evidence of their rounds and stay as they are; they are listed
in `analyzers-grandfathered.txt` beside this file. The check reads file names only: a copy under another name, or a
name added to that list, passes it, and review is what stops those (docs/design/h264.md, "A round's bars: one
analyzer, complete reports").

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
MEASUREMENTS = ROOT / 'docs' / 'measurements'
GRANDFATHERED = pathlib.Path(__file__).resolve().parent / 'analyzers-grandfathered.txt'
FIX = ('write the round\'s bars as a JSON file and run "python3 -I scripts/h264_parity_bars.py BARS.json DIR" '
       '(its docstring gives the format; docs/design/h264.md, "A round\'s bars: one analyzer, complete reports"). '
       'If the shared analyzer lacks something the round needs, extend scripts/h264_parity_bars.py so every later '
       'round has it. Do not add the file to scripts/tests/analyzers-grandfathered.txt: it lists the analyzers committed before '
       'the shared one')


def analyzers(root=MEASUREMENTS):
    """Every analyze*.py under root, as a path relative to the repository root."""
    return sorted(p.relative_to(ROOT).as_posix() for p in root.rglob('analyze*.py'))


def new_analyzers(found, listed):
    return sorted(set(found) - set(listed))


class Analyzers(unittest.TestCase):
    def test_no_new_per_round_analyzer(self):
        listed = GRANDFATHERED.read_text().split()
        added = new_analyzers(analyzers(), listed)
        if added:
            self.fail(f'new per-round analyzer(s) {", ".join(added)}: {FIX}')

    def test_the_list_is_sorted_and_names_no_file_twice(self):
        listed = GRANDFATHERED.read_text().split()
        self.assertEqual(listed, sorted(set(listed)))

    def test_a_copied_analyzer_is_named(self):
        listed = GRANDFATHERED.read_text().split()
        found = listed + ['docs/measurements/h264-rounds/round25/gating/analyze.py']
        self.assertEqual(new_analyzers(found, listed), ['docs/measurements/h264-rounds/round25/gating/analyze.py'])
        self.assertEqual(new_analyzers(listed[1:], listed), [])


if __name__ == '__main__':
    unittest.main()
