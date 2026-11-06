"""Check composed-fixture selection before any build or media generation."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import ANY, patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import composed_audio_fixtures as fixtures
import test_oracle


class FixtureSelection(unittest.TestCase):
    def test_generation_uses_the_owner_returned_program_not_the_selector(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            selected = root / 'selected/ffmpeg'
            pair = test_oracle.Oracle(root / 'verified/ffmpeg', root / 'verified/ffprobe')
            out = root / 'fixtures'
            with patch.object(sys, 'argv', ['fixtures', '--ffmpeg', str(selected), '--out', str(out)]), \
                    patch.object(fixtures.composed, 'check_policy') as policy, \
                    patch.object(fixtures.composed, 'bounded') as legacy, \
                    patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                    patch.object(fixtures, 'build_comparator') as build, \
                    patch.object(fixtures, 'generate') as generate:
                fixtures.main()
                policy.assert_called_once_with()
                qualify.assert_called_once_with(selected)
                legacy.assert_not_called()
                build.assert_called_once_with(out / 'tools', ANY)
                generate.assert_called_once_with(pair.ffmpeg, out, ANY)

    def test_qualification_failure_precedes_output_build_and_generation(self):
        with tempfile.TemporaryDirectory() as temporary:
            out = Path(temporary) / 'uncreated'
            with patch.object(sys, 'argv', ['fixtures', '--ffmpeg', 'missing', '--out', str(out)]), \
                    patch.object(fixtures.composed, 'check_policy'), \
                    patch.object(fixtures.composed, 'bounded'), \
                    patch.object(test_oracle, 'require', side_effect=test_oracle.QualificationError('stage=oracle_qualification')), \
                    patch.object(fixtures, 'build_comparator') as build, \
                    patch.object(fixtures, 'generate') as generate:
                with self.assertRaises(test_oracle.QualificationError):
                    fixtures.main()
                self.assertFalse(out.exists())
                build.assert_not_called()
                generate.assert_not_called()
