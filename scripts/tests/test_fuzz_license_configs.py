"""Every libFuzzer project selects the existing fuzz-only license policy."""

from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[2]


class FuzzLicenseConfigs(unittest.TestCase):
    def test_libfuzzer_projects_have_an_explicit_fuzz_only_license_configuration(self):
        manifests = sorted(ROOT.glob('crates/*/fuzz/Cargo.toml'))
        self.assertTrue(manifests)
        for manifest in manifests:
            package = tomllib.loads(manifest.read_text())
            if 'libfuzzer-sys' not in package.get('dependencies', {}):
                continue
            config = manifest.with_name('deny.toml')
            with self.subTest(project=manifest.parent.relative_to(ROOT)):
                self.assertTrue(config.is_file(),
                                f'add {config.relative_to(ROOT)} using the existing fuzz-only license policy')
                policy = tomllib.loads(config.read_text())
                self.assertIn('NCSA', policy['licenses']['allow'])
        root_policy = tomllib.loads((ROOT / 'deny.toml').read_text())
        self.assertNotIn('NCSA', root_policy['licenses']['allow'])


if __name__ == '__main__':
    unittest.main()
