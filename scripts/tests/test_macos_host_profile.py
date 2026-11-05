import ast
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class PublicHostProfile(unittest.TestCase):
    def test_public_host_provider_uses_the_registered_neutral_label(self):
        path = ROOT / "scripts/encoders/macos/registration.py"
        tree = ast.parse(path.read_text())
        host = next(ast.literal_eval(node.value) for node in tree.body
                    if isinstance(node, ast.Assign) and
                    any(isinstance(target, ast.Name) and target.id == "HOST" for target in node.targets))
        records = [host,
                   json.loads((ROOT / "scripts/encoders/macos/videotoolbox-sources.json").read_text())["host_profile"],
                   json.loads((ROOT / "docs/design/encoders/macos-builds/host.json").read_text())]
        for record in records:
            with self.subTest(record=record["profile"]):
                self.assertEqual(record["provider"], "private-reference",
                                 "Use the registered neutral provider label in public host records")

    def test_reference_host_prose_names_hardware_without_a_supplier(self):
        fragments = {
            "docs/design/encoders.md": "`ENC-MAC-1`, a dedicated Mac mini M1-M",
            "scripts/encoders/macos/videotoolbox-plan.md": "`ENC-MAC-1` is a dedicated Mac mini M1-M:",
            "docs/design/encoders/macos-host-profile.md": "on the same dedicated Mac mini M1-M.",
        }
        for name, expected in fragments.items():
            with self.subTest(path=name):
                self.assertIn(expected, (ROOT / name).read_text(),
                              "Describe only the dedicated Mac hardware in public reference-profile prose")


if __name__ == "__main__":
    unittest.main()
