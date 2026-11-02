"""Source and safety preflight without network, credentials or media."""
import copy
import json
import pathlib
import sys
import tempfile
import types
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import b4_round10 as b4


class Sources(unittest.TestCase):
    def setUp(self):
        self.sources = {'plan': b4.PLAN_COMMIT, 'candidate': 'a' * 40,
                        'baseline': 'b' * 40, 'tooling': 'c' * 40}
        self.sources['builds'] = {name: {'commit': self.sources[name], 'profile': 'release',
                                        'features': 'default', 'sha256': 'd' * 64}
                                  for name in ('candidate', 'baseline')}
        self.checks = [{'id': 2, 'name': 'required', 'status': 'completed', 'conclusion': 'success'}]

    def checked(self, command):
        if command[:3] == ['git', 'rev-parse', 'origin/main']:
            return self.sources['baseline']
        if command[:3] == ['git', 'rev-parse', 'HEAD']:
            return self.sources['tooling']
        if command[:2] == ['git', 'status']:
            return ''
        if command[-1].endswith('rules/branches/main'):
            return json.dumps([{'type': 'required_status_checks', 'parameters': {
                'required_status_checks': [{'context': 'required'}]}}])
        return json.dumps([{'check_runs': self.checks}])

    def verify(self, sources=None):
        with mock.patch.object(b4, 'checked', side_effect=self.checked), \
                mock.patch.object(b4.subprocess, 'run') as git:
            b4.verify_sources(self.sources if sources is None else sources)
        return git

    def test_commits_builds_and_completed_required_checks_are_verified(self):
        self.assertEqual(self.verify().call_count, 4)
        self.checks.insert(0, {'id': 1, 'name': 'required', 'status': 'completed', 'conclusion': 'failure'})
        self.verify()

    def test_wrong_plan_mutable_commits_and_wrong_builds_fail(self):
        for name, value in (('plan', 'e' * 40), ('candidate', 'main'), ('tooling', 'c' * 39)):
            data = copy.deepcopy(self.sources)
            data[name] = value
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.verify(data)
        for field, value in (('profile', 'debug'), ('features', 'all'),
                             ('commit', 'e' * 40), ('sha256', 'd' * 63)):
            data = copy.deepcopy(self.sources)
            data['builds']['candidate'][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.verify(data)

    def test_missing_failed_pending_or_cancelled_ci_never_passes(self):
        for status, conclusion in (('completed', 'failure'), ('in_progress', None),
                                   ('completed', 'cancelled'), ('in_progress', 'success')):
            self.checks[0].update(status=status, conclusion=conclusion)
            with self.subTest(status=status, conclusion=conclusion), self.assertRaises(ValueError):
                self.verify()
        self.checks = []
        with self.assertRaises(ValueError):
            self.verify()

    def test_freeze_rejects_changed_product_identity_before_binary_or_payload_access(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = pathlib.Path(temporary) / 'historical.json'
            path.write_text('[]')
            opts = types.SimpleNamespace(manifests='metadata', heldout_roster=[], product_roster=path)
            with mock.patch.object(b4, 'select_rosters', return_value=({'product': [
                    {'class': 'isobmff', 'sha256': 'a' * 64, 'ext': 'mp4', 'size': 1}]}, [])), \
                    mock.patch.object(b4, 'checked', side_effect=AssertionError('unexpected subprocess')):
                with self.assertRaisesRegex(ValueError, 'historical frozen roster'):
                    b4.freeze(opts)

    def test_safety_limits_fail_before_manifest_or_payload_access(self):
        for field, value in (('max_seconds', float('inf')), ('max_seconds', -1),
                             ('cap_usd', 11), ('cap_usd', 0.1), ('reserve_usd', float('nan'))):
            opts = types.SimpleNamespace(max_seconds=3600, usd_per_hour=1, cap_usd=2, reserve_usd=0.5)
            setattr(opts, field, value)
            with self.subTest(field=field, value=value), \
                    mock.patch.object(b4.platform, 'system', return_value='Linux'), \
                    mock.patch.object(b4.os, 'pidfd_open', create=True), \
                    mock.patch.object(b4, 'load_manifest', side_effect=AssertionError('unexpected manifest access')):
                with self.assertRaises(ValueError):
                    b4.run(opts)


if __name__ == '__main__':
    unittest.main()
