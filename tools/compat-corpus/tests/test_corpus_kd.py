"""Round-9 exclusions and audit replay, using only synthetic byte facts."""
import copy
import dataclasses
import hashlib
import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))
import corpus_kd as kd
import run


ENTRY = {'id': 'KD-TEST', 'basis': 'Synthetic specification 1', 'level': ['streams'], 'fields': ['duration_ts']}


def expected(facts, occurrence):
    if facts != b'\x14' or occurrence.path != 'streams[0].duration_ts':
        return None
    return kd.Pair(10, 20)


def rule(identifier='KD-TEST', reader=expected, document='docs/record.json'):
    return kd.Rule(identifier, document, ENTRY, ('streams.duration_ts',), reader, 'mpegts')


class Corpus(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.media = self.root / 'media'
        self.media.write_bytes(b'\x14')
        self.reference = {'streams': [{'index': 0, 'duration_ts': 10, 'width': 100}]}
        self.candidate = {'streams': [{'index': 0, 'duration_ts': 20, 'width': 100}]}
        self.ref_packets = self.root / 'ref.packets'
        self.got_packets = self.root / 'got.packets'
        self.ref_packets.write_text('packet|stream_index=0|pts=1|size=8\n')
        self.got_packets.write_text(self.ref_packets.read_text())
        self.capture = kd.Capture(self.reference, self.candidate, self.ref_packets, self.got_packets)
        self.registry = kd.Registry((rule(),), {'mpegts': lambda path: path.read_bytes()})

    def compare(self, capture=None, registry=None):
        return kd.compare(capture or self.capture, self.media, registry or self.registry, 'mpegts')

    def test_raw_rows_survive_an_exclusion(self):
        result = self.compare()
        self.assertFalse(result['keys']['streams.duration_ts'])
        self.assertEqual(result['excluded']['streams.duration_ts'], ['KD-TEST'])
        self.assertTrue(result['keys']['packets.pts'])

    def test_later_uncovered_occurrence_keeps_the_row(self):
        self.reference['streams'].append({'duration_ts': 10})
        self.candidate['streams'].append({'duration_ts': 21})
        result = self.compare()
        self.assertNotIn('streams.duration_ts', result['excluded'])
        self.assertFalse(result['keys']['streams.duration_ts'])

    def test_a_matching_occurrence_does_not_block_other_covered_differences(self):
        self.reference['streams'].append({'duration_ts': 10})
        self.candidate['streams'].append({'duration_ts': 10})
        self.assertIn('streams.duration_ts', self.compare()['excluded'])

    def test_direct_agreement_never_calls_a_predicate(self):
        self.candidate['streams'][0]['duration_ts'] = 10
        bad = kd.Registry((rule(reader=mock.Mock(side_effect=AssertionError('called'))),),
                          {'mpegts': lambda p: p.read_bytes()})
        self.assertEqual(self.compare(registry=bad)['excluded'], {})

    def test_byte_changes_remove_coverage(self):
        self.media.write_bytes(b'\x15')
        self.assertEqual(self.compare()['excluded'], {})

    def test_unreadable_evidence_covers_nothing(self):
        self.media.unlink()
        self.assertEqual(self.compare()['excluded'], {})

    def test_evidence_reader_bug_aborts(self):
        broken = kd.Registry((rule(),), {'mpegts': mock.Mock(side_effect=RuntimeError('broken reader'))})
        with self.assertRaisesRegex(RuntimeError, 'broken reader'):
            self.compare(registry=broken)

    def test_overlaps_count_one_row_and_keep_both_attributions(self):
        other = dataclasses.replace(rule('KD-OTHER'), record={**ENTRY, 'id': 'KD-OTHER'})
        registry = kd.Registry((rule(), other), {'mpegts': lambda p: p.read_bytes()})
        self.assertEqual(self.compare(registry=registry)['excluded']['streams.duration_ts'],
                         ['KD-OTHER', 'KD-TEST'])

    def test_failed_listing_cannot_be_excluded(self):
        self.capture.section_code = 2
        self.assertEqual(self.compare()['excluded'], {})

    def test_existing_strict_coverage_is_not_a_new_exclusion(self):
        self.capture.strict_keys = {'streams.duration_ts'}
        self.assertEqual(self.compare()['excluded'], {})
        self.assertEqual(self.compare()['covered_keys'], ['streams.duration_ts'])

    def test_packet_occurrences_are_not_truncated_at_the_first_example(self):
        self.ref_packets.write_text(''.join(f'packet|pts={i}\n' for i in range(40)))
        self.got_packets.write_text(''.join(f'packet|pts={i + 1}\n' for i in range(40)))
        def packet_expected(facts, occurrence):
            i = int(occurrence.path.split('[')[1].split(']')[0])
            return kd.Pair(str(i), str(i + 1)) if i < 39 else None
        r = dataclasses.replace(rule(), keys=('packets.pts',), expected=packet_expected,
                                record={**ENTRY, 'level': ['packets'], 'fields': ['pts']})
        result = self.compare(registry=kd.Registry((r,), {'mpegts': lambda p: p.read_bytes()}))
        self.assertNotIn('packets.pts', result['excluded'])

    def test_null_and_missing_are_distinct_for_expected_pairs(self):
        del self.candidate['streams'][0]['duration_ts']
        r = dataclasses.replace(rule(), expected=lambda f, o: kd.Pair(10, None))
        self.assertEqual(self.compare(registry=kd.Registry((r,), {'mpegts': lambda p: b'x'}))['excluded'], {})
        self.candidate['streams'][0]['duration_ts'] = None
        self.assertEqual(self.compare(registry=kd.Registry((r,), {'mpegts': lambda p: b'x'}))['excluded'], {})
        self.reference['streams'][0]['duration_ts'] = None
        self.candidate['streams'][0]['duration_ts'] = 20
        r = dataclasses.replace(r, expected=lambda f, o: kd.Pair(None, 20))
        self.assertEqual(self.compare(registry=kd.Registry((r,), {'mpegts': lambda p: b'x'}))['excluded'], {})

    def test_missing_values_never_qualify_even_if_reader_returns_absence(self):
        del self.candidate['streams'][0]['duration_ts']
        reader = mock.Mock(return_value=kd.Pair(10, None))
        r = dataclasses.replace(rule(), expected=reader)
        registry = kd.Registry((r,), {'mpegts': lambda p: b'x'})
        self.assertEqual(self.compare(registry=registry)['excluded'], {})
        self.assertTrue(kd.audit(self.capture, self.media, registry, 'mpegts')['passed'])
        reader.assert_not_called()

    def test_audit_catches_changed_values_even_when_counts_stay_equal(self):
        altered = copy.deepcopy(self.candidate)
        altered['streams'][0]['duration_ts'] = 21
        attack = kd.Attack('already wrong value changed', sections=altered)
        result = kd.audit(self.capture, self.media, self.registry, 'mpegts', [attack])
        self.assertEqual(result['attacks'][0]['verdict'], 'caught')
        self.assertTrue(result['passed'])

    def test_audit_rejects_a_broad_predicate_even_if_another_bug_is_caught(self):
        def broad(facts, occurrence):
            return kd.Pair(occurrence.reference, occurrence.candidate)
        registry = kd.Registry((rule(reader=broad),), {'mpegts': lambda p: b'x'})
        altered = copy.deepcopy(self.candidate)
        altered['streams'][0].update(duration_ts=21, width=101)
        result = kd.audit(self.capture, self.media, registry, 'mpegts', [kd.Attack('two bugs', sections=altered)])
        self.assertFalse(result['passed'])
        self.assertEqual(result['attacks'][0]['verdict'], 'covered')

    def test_a_mutation_cannot_activate_an_exclusion_on_a_matching_capture(self):
        self.candidate['streams'][0]['duration_ts'] = 10
        changed = copy.deepcopy(self.candidate)
        changed['streams'][0]['duration_ts'] = 20
        result = kd.audit(self.capture, self.media, self.registry, 'mpegts', [kd.Attack('activation', sections=changed)])
        self.assertFalse(result['passed'])

    def test_candidate_evidence_moved_with_value_is_caught(self):
        changed = copy.deepcopy(self.candidate)
        changed['streams'][0].update(duration_ts=21, claimed_duration=21)
        result = kd.audit(self.capture, self.media, self.registry, 'mpegts', [kd.Attack('coupled', sections=changed)])
        self.assertEqual(result['attacks'][0]['verdict'], 'caught')

    def test_extra_candidate_fields_are_reported_as_a_blind_spot(self):
        changed = copy.deepcopy(self.candidate)
        changed['new_field'] = 99
        result = kd.audit(self.capture, self.media, self.registry, 'mpegts', [kd.Attack('extra field', sections=changed)])
        self.assertEqual(result['attacks'][0]['verdict'], 'no effect')
        self.assertIn('reference-listed', result['blind_spots'][0])

    def test_standard_mutations_include_identity_structure_and_packet_changes(self):
        labels = [a.label for a in kd.mutations(self.capture)]
        for part in ('identity', 'drop stream', 'duplicate stream', 'drop packet', 'duplicate packet', 'coupled'):
            self.assertTrue(any(part in label for label in labels), (part, labels))
        self.assertTrue(kd.audit(self.capture, self.media, self.registry, 'mpegts')['passed'])

    def test_report_partition_eligibility_empty_denominator_and_opaque_aliases(self):
        rows = []
        for i in range(5):
            result = self.compare()
            rows.append({'alias': f'f{i:06}', 'container': 'mpegts', 'holdout': bool(i % 2),
                         'status': 'compared', 'audit': {'passed': True}, **result})
        report = kd.report(rows)
        key = report['groups']['mpegts']['streams.duration_ts']
        self.assertEqual((key['files'], key['direct'], key['excluded'], key['uncovered']), (5, 0, 5, 0))
        self.assertTrue(key['eligible'])
        self.assertIsNone(key['gated_rate'])
        self.assertFalse(report['passed'])
        self.assertEqual(report['excluded_rows'][0]['alias'], 'f000000')
        self.assertNotIn(str(self.media), json.dumps(report))

    def test_bad_or_duplicate_alias_and_partition_tampering_are_rejected(self):
        row = {'alias': 'f000001', 'container': 'mpegts', 'holdout': False,
               'status': 'compared', 'audit': {'passed': True}, **self.compare()}
        with self.assertRaises(ValueError):
            kd.report([row, row])
        with self.assertRaises(ValueError):
            kd.report([{**row, 'alias': 'private-source-name'}])
        row['keys']['streams.duration_ts'] = True
        with self.assertRaises(ValueError):
            kd.report([row])

    def test_failed_audit_or_missing_file_cannot_pass_the_report(self):
        rows = [{'alias': f'f{i:06}', 'container': 'mpegts', 'holdout': False,
                 'status': 'compared', 'keys': {'streams.width': True}, 'excluded': {},
                 'covered_keys': [], 'audit': {'passed': True}} for i in range(5)]
        self.assertTrue(kd.report(rows)['passed'])
        rows[0]['audit']['passed'] = False
        self.assertFalse(kd.report(rows)['passed'])
        self.assertFalse(kd.report([])['passed'])

    def test_production_registry_has_no_unlanded_cases(self):
        for rule in kd.production_registry().rules:
            records = json.loads((kd.ROOT / rule.document).read_text())['entries']
            self.assertEqual([entry for entry in records if entry['id'] == rule.identifier], [rule.record])
            self.assertTrue(rule.record['basis'])
            self.assertLessEqual(set(rule.keys), kd.authorized_keys(rule.record))

    def test_ts_streams_keep_index_pairing_and_require_the_same_pid(self):
        ts_rule = dataclasses.replace(rule('KD-TS-SYNTHETIC', document='docs/known-divergences-ts.json'),
                                      record={**ENTRY, 'id': 'KD-TS-SYNTHETIC'})
        registry = kd.Registry((ts_rule,), {'mpegts': lambda p: p.read_bytes()})
        self.reference['streams'][0]['id'] = '0x100'
        self.candidate['streams'][0]['id'] = '0x100'
        self.assertIn('streams.duration_ts', self.compare(registry=registry)['excluded'])
        self.candidate['streams'][0]['id'] = '0x200'
        self.assertNotIn('streams.duration_ts', self.compare(registry=registry)['excluded'])
        self.candidate['streams'][0]['id'] = '0x100'
        self.reference['streams'].append({'id': '0x200', 'duration_ts': 10})
        self.candidate['streams'].append({'id': '0x200', 'duration_ts': 20})
        self.candidate['streams'].reverse()
        result = self.compare(registry=registry)
        self.assertFalse(result['keys']['streams.duration_ts'])
        self.assertFalse(result['keys']['streams.id'])
        self.assertEqual(result['excluded'], {})

    def test_ts_missing_pid_is_not_an_identity_certificate(self):
        ts_rule = dataclasses.replace(rule('KD-TS-SYNTHETIC', document='docs/known-divergences-ts.json'),
                                      record={**ENTRY, 'id': 'KD-TS-SYNTHETIC'})
        self.reference['streams'][0]['id'] = '0x100'
        registry = kd.Registry((ts_rule,), {'mpegts': lambda p: p.read_bytes()})
        self.assertEqual(self.compare(registry=registry)['excluded'], {})

    def test_ts_packets_require_same_index_and_same_pid_without_searching(self):
        self.reference['streams'][0]['id'] = '0x100'
        self.candidate['streams'][0]['id'] = '0x100'
        self.got_packets.write_text('packet|stream_index=0|pts=2|size=8\n')
        ts_rule = dataclasses.replace(rule('KD-TS-SYNTHETIC', document='docs/known-divergences-ts.json'), keys=('packets.pts',),
                                      record={**ENTRY, 'id': 'KD-TS-SYNTHETIC',
                                              'level': ['packets'], 'fields': ['pts']},
                                      expected=lambda f, o: kd.Pair('1', '2'))
        registry = kd.Registry((ts_rule,), {'mpegts': lambda p: p.read_bytes()})
        self.assertIn('packets.pts', self.compare(registry=registry)['excluded'])
        self.candidate['streams'][0]['id'] = '0x200'
        self.assertNotIn('packets.pts', self.compare(registry=registry)['excluded'])
        self.candidate['streams'].append({'id': '0x100'})
        self.got_packets.write_text('packet|stream_index=1|pts=2|size=8\n')
        self.assertNotIn('packets.pts', self.compare(registry=registry)['excluded'])

    def test_ts_pid_guard_comes_from_the_required_record_source_not_the_identifier(self):
        registry = kd.Registry((rule(document='docs/known-divergences-ts.json'),), {'mpegts': lambda p: p.read_bytes()})
        self.reference['streams'][0]['id'] = '0x100'
        self.candidate['streams'][0]['id'] = '0x100'
        self.assertIn('streams.duration_ts', self.compare(registry=registry)['excluded'])
        self.candidate['streams'][0]['id'] = '0x200'
        self.assertEqual(self.compare(registry=registry)['excluded'], {})
        self.assertTrue(registry.records()[0]['requires_pid'])

    def test_compact_and_json_raw_rows_share_the_legacy_comparison(self):
        expected_keys = {}
        for path, key, ref, present, got in run.key_pairs(run.flatten(self.reference), run.flatten(self.candidate)):
            expected_keys[key] = expected_keys.get(key, True) and ref == got
        for item in kd.occurrences(self.capture):
            expected_keys.setdefault(item.key, True)
        self.assertEqual(self.compare()['keys'], expected_keys)

    def test_packet_field_names_are_not_normalized_like_section_array_paths(self):
        pair = list(run.key_pairs({'tags.name[1]': 'x'}, {}, 'packets[2].'))[0]
        self.assertEqual(pair[:2], ('packets[2].tags.name[1]', 'packets.tags.name[1]'))
        pair = list(run.key_pairs({'streams[2].duration_ts': 1}, {}))[0]
        self.assertEqual(pair[:2], ('streams[2].duration_ts', 'streams.duration_ts'))

    def test_extra_trailing_packets_are_not_claimed_as_a_caught_attack(self):
        def append(n, fields):
            return (fields, fields)
        result = kd.audit(self.capture, self.media, self.registry, 'mpegts', [kd.Attack('trailing packet', packets=append)])
        self.assertEqual(result['attacks'][0]['verdict'], 'no effect')

    def test_packet_audit_checks_first_middle_last_of_each_stream(self):
        self.got_packets.write_text(''.join(f'packet|stream_index={stream}|pts={i}\n'
                                            for i in range(5) for stream in (0, 1)))
        labels = [a.label for a in kd.mutations(self.capture)]
        for i in (0, 1, 4, 5, 8, 9):
            self.assertIn(f'packet {i}: pts changed', labels)
        for i in (2, 3, 6, 7):
            self.assertNotIn(f'packet {i}: pts changed', labels)

    def test_raw_eligibility_does_not_shrink_with_the_adjusted_denominator(self):
        rows = [{'alias': f'f{i:06}', 'container': 'mpegts', 'holdout': False, 'status': 'compared',
                 'audit': {'passed': True}, **self.compare()} for i in range(5)]
        rows[0]['excluded'] = {}
        table = kd.report(rows)['groups']['mpegts']['streams.duration_ts']
        self.assertEqual((table['files'], table['gated_files'], table['uncovered']), (5, 1, 1))
        self.assertTrue(table['eligible'])

    def test_container_breakdown_does_not_add_a_new_bar(self):
        rows = [{'alias': f'f{i:06}', 'container': 'isobmff' if i < 995 else 'mpegts',
                 'holdout': False, 'status': 'compared', 'audit': {'passed': True},
                 'keys': {'streams.width': i < 995}, 'excluded': {}, 'covered_keys': []}
                for i in range(1000)]
        result = kd.report(rows)
        self.assertTrue(result['passed'])
        self.assertFalse(result['groups']['mpegts']['streams.width']['passed'])

    def test_common_tags_and_pooled_rare_tags_are_not_hidden_by_headline_counts(self):
        rows = [{'alias': f'f{i:06}', 'container': 'mpegts', 'holdout': False, 'status': 'compared',
                 'audit': {'passed': True}, 'keys': {'streams.width': True, 'streams.tags.language': False,
                                                  f'format.tags.rare{i}': False},
                 'excluded': {}, 'covered_keys': []} for i in range(5)]
        result = kd.report(rows)
        table = result['groups']['all']
        self.assertFalse(result['passed'])
        self.assertTrue(table['streams.tags.language']['eligible'])
        self.assertFalse(table['streams.tags.language']['headline'])
        self.assertEqual(table[kd.POOLED_TAGS]['memberships'], 5)
        self.assertNotIn('files', table[kd.POOLED_TAGS])
        self.assertFalse(table[kd.POOLED_TAGS]['passed'])

    def test_exclusions_have_deduplicated_container_key_and_partition_counts(self):
        row = {'alias': 'f000001', 'container': 'mpegts', 'holdout': False, 'status': 'compared',
               'audit': {'passed': True}, **self.compare()}
        report = kd.report([row])
        self.assertEqual(report['unique_excluded_rows'], 1)
        self.assertEqual(report['exclusion_counts'], [{'kd': 'KD-TEST', 'key': 'streams.duration_ts',
                                                      'container': 'mpegts', 'holdout': False, 'rows': 1}])


class Inventory(unittest.TestCase):
    def test_implemented_scope_cannot_widen_beyond_the_record(self):
        wider = dataclasses.replace(rule(), keys=('streams.width',))
        with self.assertRaises(ValueError):
            kd.Registry((wider,), {'mpegts': lambda p: b'x'})

    def test_regenerating_inventory_cannot_add_fields_cases_or_identifiers(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / 'scripts').mkdir()
            (root / 'scripts/oracle-ffmpeg.env').write_bytes(b'pin')
            def source(root, *args):
                if args[0] == 'merge-base':
                    return b''
                if args[-1].endswith('docs/record.json'):
                    return json.dumps({'entries': [ENTRY]}).encode()
                if args[-1].endswith('scripts/oracle-ffmpeg.env'):
                    return b'pin'
                return b'old plan' if '^:' in args[-1] else kd.AMENDMENT.encode()
            with mock.patch.object(kd, 'git', side_effect=source):
                self.assertEqual(kd.inventory(root, 'a' * 40, kd.Registry((rule(),), {'mpegts': lambda p: b'x'}), '0' * 64)['records'],
                                 kd.Registry((rule(),), {'mpegts': lambda p: b'x'}).records())
                for record, keys in (({**ENTRY, 'fields': ['duration_ts', 'width']}, ('streams.width',)),
                                     ({**ENTRY, 'conditions': {'additional_case': 'another reading'}}, rule().keys),
                                     ({**ENTRY, 'id': 'KD-NEW'}, rule().keys)):
                    wider = dataclasses.replace(rule(), record=record, keys=keys, identifier=record['id'])
                    with self.subTest(record=record), self.assertRaises(ValueError):
                        kd.inventory(root, 'a' * 40, kd.Registry((wider,), {'mpegts': lambda p: b'x'}), '0' * 64)

    def test_wildcard_scope_cannot_authorize_later_fields(self):
        for record in ({**ENTRY, 'fields': ['*']}, {**ENTRY, 'level': ['*']}, {**ENTRY, 'level': []}):
            with self.subTest(record=record), self.assertRaises(ValueError):
                kd.Registry((dataclasses.replace(rule(), record=record),), {'mpegts': lambda p: b'x'})

    def test_unregistered_and_changed_records_fail_closed(self):
        registry = kd.Registry((rule(),), {'mpegts': lambda p: b'x'})
        snapshot = {'records': registry.records()}
        registry.validate(snapshot)
        changed = copy.deepcopy(snapshot)
        changed['records'][0]['sha256'] = hashlib.sha256(b'changed scope').hexdigest()
        with self.assertRaises(ValueError):
            registry.validate(changed)
        with self.assertRaises(ValueError):
            kd.production_registry().validate(snapshot)
        with self.assertRaises(ValueError):
            registry.validate({'records': snapshot['records'] * 2})

    def test_cutoff_is_the_main_commit_introducing_the_amendment(self):
        registry = kd.Registry((rule(),), {'mpegts': lambda p: b'x'})
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / 'scripts').mkdir()
            (root / 'scripts/oracle-ffmpeg.env').write_bytes(b'the shared pin')
            def source(root, *args):
                if args[:1] == ('merge-base',):
                    return b''
                if args[1].endswith('scripts/oracle-ffmpeg.env'):
                    return b'the shared pin'
                if args[1].endswith('docs/record.json'):
                    return json.dumps({'entries': [ENTRY]}).encode()
                return (b'old plan' if '^:' in args[1] else kd.AMENDMENT.encode())
            with mock.patch.object(kd, 'git', side_effect=source) as calls:
                frozen = kd.inventory(root, 'a' * 40, registry, '0' * 64)
                self.assertEqual(frozen['records'], registry.records())
                self.assertEqual(calls.call_args_list[0].args[1:],
                                 ('merge-base', '--is-ancestor', 'a' * 40, 'origin/main'))
            for text in (b'old plan', kd.AMENDMENT.encode()):
                with mock.patch.object(kd, 'git', return_value=text), self.assertRaises(ValueError):
                    kd.inventory(root, 'a' * 40, registry, '0' * 64)
            changed = {**ENTRY, 'fields': ['width']}
            def changed_source(root, *args):
                return json.dumps({'entries': [changed]}).encode() if args[-1].endswith('docs/record.json') else source(root, *args)
            with mock.patch.object(kd, 'git', side_effect=changed_source), self.assertRaises(ValueError):
                kd.inventory(root, 'a' * 40, registry, '0' * 64)
            (root / 'scripts/oracle-ffmpeg.env').write_bytes(b'a different pin')
            with mock.patch.object(kd, 'git', side_effect=source), self.assertRaisesRegex(ValueError, 'oracle pin'):
                kd.inventory(root, 'a' * 40, registry, '0' * 64)


FAKE = '''#!/usr/bin/env python3
import json, sys
if '-show_packets' in sys.argv:
    print('packet|stream_index=0|pts=1|size=8')
else:
    print(json.dumps({'streams': [{'index': 0, 'duration_ts': 10}], 'programs': [],
                      'chapters': [], 'format': {'format_name': 'synthetic'}}))
'''


class Driver(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        for name in ('coreav', 'ffprobe', 'ffmpeg'):
            path = self.root / name
            path.write_text(FAKE)
            path.chmod(0o755)
        self.rows = []
        for i in range(3):
            path = self.root / f'media{i}'
            path.write_bytes(bytes([i]))
            self.rows.append({'alias': f'f{i:06}', 'container': 'mpegts', 'holdout': False,
                              'path': str(path), 'sha256': kd.file_hash(path)})
        self.files = self.root / 'files.json'
        self.files.write_text(json.dumps(self.rows))
        self.frozen = {'schema': kd.SCHEMA, 'plan_commit': 'a' * 40,
                       'records': kd.production_registry().records(),
                       'selection_sha256': kd.file_hash(self.files)}
        self.inventory = self.root / 'inventory.json'
        self.inventory.write_text(json.dumps(self.frozen))
        self.output = self.root / 'out'

    def complete_population(self):
        for i in range(len(self.rows), 6):
            path = self.root / f'media{i}'
            path.write_bytes(bytes([i]))
            self.rows.append({'alias': f'f{i:06}', 'container': 'mpegts', 'holdout': False,
                              'path': str(path), 'sha256': kd.file_hash(path)})
        for i, row in enumerate(self.rows):
            row['container'] = kd.CONTAINERS[i % 3]
        self.files.write_text(json.dumps(self.rows))
        self.freeze_selection()

    def freeze_selection(self):
        self.frozen['selection_sha256'] = kd.file_hash(self.files)
        self.inventory.write_text(json.dumps(self.frozen))

    def execute(self):
        sys.path.insert(0, str(kd.ROOT / 'scripts'))
        import oracle_pin
        args = ['corpus_kd.py', 'compare', '--inventory', str(self.inventory), '--files', str(self.files),
                '--coreav', str(self.root / 'coreav'), '--oracle-dir', str(self.root), '--out', str(self.output)]
        with mock.patch.object(sys, 'argv', args), mock.patch.dict(os.environ), \
                mock.patch.object(run, 'ENV', run.ENV.copy()), \
                mock.patch.object(kd, 'inventory', return_value=self.frozen), \
                mock.patch.object(oracle_pin, 'verify_bin', return_value=[]) as checked:
            result = kd.main()
            checked.assert_called_once_with(self.root)
            return result

    def test_real_standin_capture_and_report_preserve_the_manifest_and_do_not_pass_a_subset(self):
        self.assertEqual(self.execute(), 1)
        result = json.loads((self.output / 'report.json').read_text())
        self.assertFalse(result['aborted'])
        self.assertFalse(result['registered_population'])
        self.assertEqual(result['processed_files'], 3)
        self.assertEqual(result['unique_excluded_rows'], 0)
        self.assertEqual(result['groups']['mpegts']['packets.pts']['direct'], 3)
        self.assertNotIn(str(self.root), json.dumps(result))
        self.assertEqual(json.loads((self.output / 'private/files.json').read_text()), self.rows)

    def test_abort_retains_completed_error_and_unprocessed_rows(self):
        real_capture = kd.capture_file
        calls = []
        def capture(*args):
            calls.append(args)
            if len(calls) == 2:
                raise OSError('synthetic infrastructure failure')
            return real_capture(*args)
        with mock.patch.object(kd, 'capture_file', side_effect=capture), self.assertRaises(OSError):
            self.execute()
        result = json.loads((self.output / 'report.json').read_text())
        self.assertTrue(result['aborted'])
        self.assertFalse(result['passed'])
        self.assertEqual(result['by_status'], {'compared': 1, 'error': 1, 'unprocessed': 1})
        private = json.loads((self.output / 'private/rows.json').read_text())
        self.assertEqual([r['alias'] for r in private], ['f000000', 'f000001', 'f000002'])

    def test_changed_media_and_duplicate_manifest_cannot_silently_reduce_population(self):
        self.rows[1]['sha256'] = '0' * 64
        self.files.write_text(json.dumps(self.rows))
        self.freeze_selection()
        with self.assertRaisesRegex(ValueError, 'checksum'):
            self.execute()
        result = json.loads((self.output / 'report.json').read_text())
        self.assertEqual(result['by_status'], {'compared': 1, 'error': 1, 'unprocessed': 1})
        self.assertFalse(result['passed'])

    def test_tampered_inventory_is_rejected_before_a_capture(self):
        self.inventory.write_text(json.dumps({**self.frozen, 'records': [{'id': 'unregistered'}]}))
        with self.assertRaises(ValueError), mock.patch.object(kd, 'capture_file') as capture:
            self.execute()
        capture.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_duplicate_manifest_aliases_are_rejected_before_capture(self):
        self.files.write_text(json.dumps(self.rows + [self.rows[0]]))
        with self.assertRaisesRegex(ValueError, 'duplicate'), mock.patch.object(kd, 'capture_file') as capture:
            self.execute()
        capture.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_changed_selection_with_the_same_population_is_rejected_before_capture(self):
        self.complete_population()
        self.rows[0]['holdout'] = True
        self.files.write_text(json.dumps(self.rows))
        with mock.patch.object(kd, 'ROUND_FILES', 6), mock.patch.object(kd, 'capture_file') as capture, \
                self.assertRaisesRegex(ValueError, 'selection'):
            self.execute()
        capture.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_freeze_records_the_selection_digest(self):
        args = ['corpus_kd.py', 'freeze', '--plan-commit', 'a' * 40, '--files', str(self.files),
                '--out', str(self.output)]
        with mock.patch.object(sys, 'argv', args), mock.patch.object(kd, 'inventory', return_value=self.frozen) as freeze:
            self.assertEqual(kd.main(), 0)
        self.assertEqual(freeze.call_args.args[-1], kd.file_hash(self.files))
        self.assertEqual(json.loads(self.output.read_text())['selection_sha256'], kd.file_hash(self.files))

    def test_missing_or_nonstring_manifest_path_is_rejected_before_capture(self):
        for value in (None, 1, [], ''):
            with self.subTest(path=value):
                self.rows[1]['path'] = value
                self.files.write_text(json.dumps(self.rows))
                with mock.patch.object(kd, 'capture_file') as capture, self.assertRaisesRegex(ValueError, 'invalid'):
                    self.execute()
                capture.assert_not_called()
                self.assertFalse(self.output.exists())
        del self.rows[1]['path']
        self.files.write_text(json.dumps(self.rows))
        with self.assertRaisesRegex(ValueError, 'invalid'):
            self.execute()

    def test_malformed_successful_packet_capture_aborts_an_otherwise_passing_population(self):
        self.complete_population()
        (self.root / 'ffprobe').write_text(FAKE.replace('packet|stream_index=0|pts=1|size=8',
                                                      'this is a malformed packet record'))
        with mock.patch.object(kd, 'ROUND_FILES', 6), self.assertRaises(ValueError):
            self.execute()
        result = json.loads((self.output / 'report.json').read_text())
        self.assertTrue(result['registered_population'])
        self.assertTrue(result['aborted'])
        self.assertFalse(result['passed'])
        self.assertEqual(result['unique_excluded_rows'], 0)

    def test_malformed_or_missing_successful_captures_abort_on_either_side(self):
        for tool in ('ffprobe', 'coreav'):
            for payload in ('', 'not a packet', 'packet|pts=1', 'packet|stream_index=0|size=8|garbage',
                            'packet|side_data', 'packet|stream_index=0|size=8\\npacket|side_data'):
                with self.subTest(tool=tool, payload=payload):
                    for name in ('ffprobe', 'coreav'):
                        (self.root / name).write_text(FAKE)
                    (self.root / tool).write_text(FAKE.replace('packet|stream_index=0|pts=1|size=8', payload))
                    self.output = self.root / f'out{tool}{len(list(self.root.glob("out*")))}'
                    with self.assertRaises(ValueError):
                        self.execute()
                    result = json.loads((self.output / 'report.json').read_text())
                    self.assertTrue(result['aborted'])
                    self.assertFalse(result['passed'])
                    self.assertEqual(result['unique_excluded_rows'], 0)
            for payload in ('{}', '{"streams":{}}', 'null', 'malformed'):
                with self.subTest(tool=tool, payload=payload):
                    for name in ('ffprobe', 'coreav'):
                        (self.root / name).write_text(FAKE)
                    (self.root / tool).write_text('#!/usr/bin/env python3\nprint(' + repr(payload) + ')\n')
                    self.output = self.root / f'out{tool}{len(list(self.root.glob("out*")))}'
                    with self.assertRaises(ValueError):
                        self.execute()
                    result = json.loads((self.output / 'report.json').read_text())
                    self.assertTrue(result['aborted'])
                    self.assertFalse(result['passed'])

    def test_valid_complete_standin_population_passes_before_injecting_malformed_output(self):
        self.complete_population()
        with mock.patch.object(kd, 'ROUND_FILES', 6):
            self.assertEqual(self.execute(), 0)
        result = json.loads((self.output / 'report.json').read_text())
        self.assertTrue(result['registered_population'])
        self.assertFalse(result['aborted'])
        self.assertTrue(result['passed'])


class CaptureValidation(unittest.TestCase):
    def test_ambiguous_json_and_nonfinite_values_are_malformed(self):
        valid = json.dumps({'streams': [], 'chapters': [], 'programs': [], 'format': {'format_name': 'x'}})
        self.assertEqual(kd.section_document(valid)['format']['format_name'], 'x')
        for fields in ('"format_name":"x","format_name":"y"',
                       '"format_name":"x","bad":NaN', '"format_name":"x","bad":1e999'):
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                kd.section_document(valid.replace('"format_name": "x"', fields))

    def test_compact_escaping_and_side_data_are_validated_without_changing_raw_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / 'packets'
            path.write_text('packet|stream_index=0|size=8|side_data|side_data_type=thing|label=a\\|b\n\n')
            kd.validate_packets(path, {'streams': [{'index': 0}]})
            self.assertEqual(list(kd.packet_rows(path))[0], run.packet_fields(path.read_text().splitlines()[0]))
            path.write_bytes(b'packet|stream_index=0|size=8|label=\xff\n')
            with self.assertRaises(ValueError):
                kd.validate_packets(path, {'streams': [{'index': 0}]})


if __name__ == '__main__':
    unittest.main()
