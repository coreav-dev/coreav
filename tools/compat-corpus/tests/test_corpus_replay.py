"""Cached replay preserves the streaming audit's exact judgments."""
import copy
import dataclasses
import json
import pathlib
import random
import sys
import unittest
import weakref
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import test_corpus_kd as base
import test_corpus_absence as absence
import test_corpus_iso as iso_fixtures
import test_corpus_mkv as mkv_fixtures
import test_corpus_ts as ts_fixtures
import corpus_kd as kd


def streaming_audit(capture, media, registry, container, attacks):
    facts = registry.evidence(media, container)
    results = []
    for attack in attacks:
        caught, covered, changed = 0, 0, 0
        for before, after in zip(kd.occurrences(capture, summarize=registry.summarizes(facts)),
                                 kd.occurrences(capture, attack, summarize=registry.summarizes(facts)), strict=True):
            old_ids, new_ids = registry.covers(before, facts), registry.covers(after, facts)
            new_difference = after.different and (not before.different or
                              (before.present, before.candidate) != (after.present, after.candidate))
            if new_difference:
                changed += 1
                if new_ids or after.key in capture.strict_keys:
                    covered += 1
                else:
                    caught += 1
            elif after.different and not old_ids and new_ids:
                covered += 1
        verdict = 'covered' if covered else 'caught' if caught else 'no effect'
        results.append({'label': attack.label, 'verdict': verdict, 'changed': changed,
                        'caught': caught, 'covered': covered})
    return {'passed': all(r['verdict'] != 'covered' for r in results), 'attacks': results,
            'blind_spots': kd.BLIND_SPOTS}


class Replay(unittest.TestCase):
    def fixture(self, cls):
        fixture = cls()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        return fixture

    def equivalent(self, capture, fixture, container, attacks=None):
        attacks = list(kd.mutations(capture)) if attacks is None else attacks
        expected = json.dumps(streaming_audit(capture, fixture.media, fixture.registry, container, attacks))
        fresh = kd.compare(capture, fixture.media, fixture.registry, container)
        for limit in (kd.PACKET_CACHE_BYTES, 0, 1000):
            with self.subTest(cache_bytes=limit), mock.patch.object(kd, 'PACKET_CACHE_BYTES', limit):
                prepared = kd.prepare(capture, fixture.media, fixture.registry, container)
                self.assertEqual(kd.compare(capture, fixture.media, fixture.registry, container, prepared=prepared), fresh)
                actual = kd.audit(capture, fixture.media, fixture.registry, container, attacks, prepared=prepared)
                self.assertEqual(json.dumps(actual), expected)

    def test_generated_container_cases_match_streaming_judgments(self):
        iso = self.fixture(iso_fixtures.Iso)
        for absence, flag in ((False, False), (True, False), (True, True)):
            self.equivalent(iso.make(absence, flag), iso, 'isobmff')
        mkv = self.fixture(mkv_fixtures.Matroska)
        for case in mkv_fixtures.CASES:
            for options in ({}, {'before': True, 'count': 2}, {'lacing': 1}, {'lacing': 2, 'group': True},
                            {'lacing': 3}, {'empty': True}):
                with self.subTest(case=case, options=options):
                    self.equivalent(mkv.make(case, **options), mkv, 'matroska')
        ts = self.fixture(ts_fixtures.TransportStreams)
        for make in (ts.vbi, ts.wrap, ts.downmix, lambda: ts.vbi(False), lambda: ts.downmix(True)):
            self.equivalent(make(), ts, 'mpegts')

    def test_absence_failures_strict_keys_blank_lines_and_failed_captures(self):
        fixture = self.fixture(absence.Absence)
        fixture.reference.write_text('\n' + fixture.reference.read_text())
        fixture.candidate.write_text('\n' + fixture.candidate.read_text())
        for section_code, packet_code, strict in ((0, 0, set()), (1, 0, {kd.ABSENCE_KEY}), (0, 2, set())):
            capture = dataclasses.replace(fixture.capture, section_code=section_code, packet_code=packet_code,
                                          strict_keys=strict)
            self.equivalent(capture, fixture, 'isobmff')

    def test_section_attack_can_activate_coverage_on_an_unchanged_packet(self):
        fixture = self.fixture(base.Corpus)
        fixture.got_packets.write_text('packet|stream_index=0|pts=2|size=8\n')
        def expected(facts, item):
            return kd.Pair('1', '2') if item.context.candidate['streams'][0]['width'] == 101 else None
        packet_rule = dataclasses.replace(base.rule(), keys=('packets.pts',), expected=expected,
                                          record={**base.ENTRY, 'level': ['packets'], 'fields': ['pts']})
        fixture.registry = kd.Registry((packet_rule,), {'mpegts': lambda p: p.read_bytes()})
        sections = copy.deepcopy(fixture.candidate)
        sections['streams'][0]['width'] = 101
        attacks = [kd.Attack('activate packet coverage', sections=sections)]
        self.equivalent(fixture.capture, fixture, 'mpegts', attacks)
        result = kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', attacks)
        self.assertEqual(result['attacks'][0]['verdict'], 'covered')

    def test_seeded_opaque_packet_transforms_keep_order_and_summary_binding(self):
        fixture = self.fixture(absence.Absence)
        rng = random.Random(732)
        attacks = []
        for _ in range(24):
            index, action = rng.randrange(3), rng.choice(('drop', 'duplicate', 'append', 'change'))
            def transform(i, fields, index=index, action=action):
                if i != index:
                    return (fields,)
                if action == 'drop':
                    return ()
                if action in ('duplicate', 'append'):
                    return fields, fields
                return ({**fields, 'side_data_list.discard_padding': '21', 'stream_index': '1'},)
            attacks.append(kd.Attack(f'{action} {index}', packets=transform))
        self.equivalent(fixture.capture, fixture, 'isobmff', attacks)

    def test_baseline_coverage_cache_has_a_streaming_fallback(self):
        fixture = self.fixture(absence.Absence)
        with mock.patch.object(kd, 'COVERAGE_CACHE_ENTRIES', 0):
            self.equivalent(fixture.capture, fixture, 'isobmff')

    def test_section_paths_cannot_supply_a_packets_baseline_coverage(self):
        fixture = self.fixture(base.Corpus)
        fixture.reference['packets'] = [{'pts': '1'}]
        fixture.candidate['packets'] = [{'pts': '2'}]
        fixture.got_packets.write_text('packet|stream_index=0|pts=2|size=8\n')
        def expected(facts, item):
            if item.context.packet_index is None or item.context.candidate['streams'][0]['width'] == 101:
                return kd.Pair('1', '2')
            return None
        packet_rule = dataclasses.replace(base.rule(), keys=('packets.pts',), expected=expected,
                                          record={**base.ENTRY, 'level': ['packets'], 'fields': ['pts']})
        fixture.registry = kd.Registry((packet_rule,), {'mpegts': lambda p: p.read_bytes()})
        sections = copy.deepcopy(fixture.candidate)
        sections['streams'][0]['width'] = 101
        self.equivalent(fixture.capture, fixture, 'mpegts', [kd.Attack('activate packet only', sections=sections)])

    def test_shared_preparation_parses_each_listing_and_reads_evidence_once(self):
        fixture = self.fixture(base.Corpus)
        reader = mock.Mock(side_effect=lambda p: p.read_bytes())
        fixture.registry.readers['mpegts'] = reader
        with mock.patch.object(kd.run, 'packet_fields', wraps=kd.run.packet_fields) as parsed:
            prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
            kd.compare(fixture.capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)
            kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)
        self.assertEqual(parsed.call_count, 2)
        self.assertEqual(reader.call_count, 1)
        retained = weakref.ref(prepared)
        del prepared
        self.assertIsNone(retained())

    def test_only_changed_or_adapter_reachable_packet_leaves_are_replayed(self):
        fixture = self.fixture(base.Corpus)
        packet = 'packet|stream_index=0|pts=1|size=8\n'
        fixture.ref_packets.write_text(packet * 40)
        fixture.got_packets.write_text(packet * 40)
        fixture.registry = kd.Registry((), {})
        def change(i, fields):
            return ({**fields, 'pts': '2'},) if i == 20 else (fields,)
        prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
        with mock.patch.object(kd, 'Occurrence', wraps=kd.Occurrence) as occurrence:
            result = kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts',
                              [kd.Attack('one packet', packets=change)] * 5, prepared=prepared)
        self.assertEqual([row['changed'] for row in result['attacks']], [1] * 5)
        self.assertEqual(occurrence.call_count, 123 + 10)

    def test_no_attacks_do_not_evaluate_coverage(self):
        fixture = self.fixture(base.Corpus)
        with mock.patch.object(fixture.registry, 'covers', side_effect=AssertionError('unexpected coverage')):
            self.assertTrue(kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', [])['passed'])

    def test_packet_cache_budget_discards_an_overflowing_listing(self):
        fixture = self.fixture(base.Corpus)
        packet = 'packet|stream_index=0|pts=1|size=8\n'
        fixture.ref_packets.write_text(packet * 20)
        fixture.got_packets.write_text(packet * 20)
        with mock.patch.object(kd, 'PACKET_CACHE_BYTES', 1):
            prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
        self.assertIsNone(prepared.reference.cached)
        self.assertIsNone(prepared.candidate.cached)
        self.assertEqual(len(list(prepared.reference.rows())), 20)

    def test_cached_rows_cannot_be_changed_in_place(self):
        fixture = self.fixture(base.Corpus)
        def mutate(i, fields):
            fields['pts'] = '2'
            return (fields,)
        with self.assertRaises(TypeError):
            kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', [kd.Attack('mutate', packets=mutate)])

    def test_replaced_capture_or_changed_packet_bytes_cannot_reuse_preparation(self):
        fixture = self.fixture(base.Corpus)
        prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
        with self.assertRaisesRegex(ValueError, 'different capture'):
            kd.compare(copy.deepcopy(fixture.capture), fixture.media, fixture.registry, 'mpegts', prepared=prepared)
        fixture.got_packets.write_text('packet|stream_index=0|pts=2|size=8\n')
        with self.assertRaisesRegex(ValueError, 'capture changed'):
            kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)

    def test_one_changed_media_byte_cannot_reuse_downmix_evidence(self):
        fixture = self.fixture(ts_fixtures.TransportStreams)
        capture = fixture.downmix()
        prepared = kd.prepare(capture, fixture.media, fixture.registry, 'mpegts')
        self.assertEqual(len(kd.compare(capture, fixture.media, fixture.registry, 'mpegts',
                                       prepared=prepared)['excluded']), 5)
        original = fixture.media.read_bytes()
        fixture.downmix(corrupt=True)
        corrupt = fixture.media.read_bytes()
        self.assertEqual(len(original), len(corrupt))
        self.assertEqual(sum(a != b for a, b in zip(original, corrupt, strict=True)), 1)
        self.assertEqual(kd.compare(capture, fixture.media, fixture.registry, 'mpegts')['excluded'], {})
        with self.assertRaisesRegex(ValueError, 'evidence input changed'):
            kd.compare(capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)
        with self.assertRaisesRegex(ValueError, 'evidence input changed'):
            kd.audit(capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)

    def test_media_changes_during_evidence_acquisition_abort(self):
        fixture = self.fixture(base.Corpus)
        def changed_reader(path):
            facts = path.read_bytes()
            path.write_bytes(b'\x15')
            return facts
        fixture.registry.readers['mpegts'] = changed_reader
        with self.assertRaisesRegex(ValueError, 'evidence input changed'):
            kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')

    def test_media_changes_during_audit_abort_before_a_result_is_returned(self):
        fixture = self.fixture(base.Corpus)
        prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
        def attacks():
            yield kd.Attack('unchanged')
            fixture.media.write_bytes(b'\x15')
        with self.assertRaisesRegex(ValueError, 'evidence input changed'):
            kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', attacks(), prepared=prepared)

    def test_missing_media_stays_uncovered_but_its_appearance_invalidates_preparation(self):
        fixture = self.fixture(base.Corpus)
        fixture.media.unlink()
        prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
        self.assertEqual(kd.compare(fixture.capture, fixture.media, fixture.registry, 'mpegts',
                                   prepared=prepared)['excluded'], {})
        fixture.media.write_bytes(b'\x14')
        with self.assertRaisesRegex(ValueError, 'evidence input changed'):
            kd.compare(fixture.capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)

    def test_reader_bytes_and_registry_changes_invalidate_shared_evidence(self):
        fixture = self.fixture(base.Corpus)
        helper = fixture.root / 'reader.py'
        helper.write_bytes(b'original')
        fixture.registry.reader_paths = (str(helper),)
        prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
        helper.write_bytes(b'changed!')
        with self.assertRaisesRegex(ValueError, 'evidence input changed'):
            kd.compare(fixture.capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)
        helper.write_bytes(b'original')
        fixture.registry.readers['mpegts'] = lambda p: b'other'
        with self.assertRaisesRegex(ValueError, 'registry changed'):
            kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', prepared=prepared)

    def test_capture_and_registry_changes_during_audit_cannot_certify_cached_coverage(self):
        for target in ('capture', 'registry'):
            fixture = self.fixture(base.Corpus)
            fixture.registry.rules = tuple(dataclasses.replace(rule, record=copy.deepcopy(rule.record))
                                           for rule in fixture.registry.rules)
            prepared = kd.prepare(fixture.capture, fixture.media, fixture.registry, 'mpegts')
            def attacks():
                yield kd.Attack('unchanged')
                if target == 'capture':
                    fixture.capture.candidate['streams'][0]['width'] = 101
                else:
                    fixture.registry.rules[0].record['basis'] = 'changed'
            with self.subTest(target=target), self.assertRaisesRegex(ValueError, 'capture or registry changed'):
                kd.audit(fixture.capture, fixture.media, fixture.registry, 'mpegts', attacks(), prepared=prepared)

    def test_driver_reads_evidence_once_per_file(self):
        driver = self.fixture(base.Driver)
        driver.complete_population()
        evidence = mock.Mock(return_value={})
        with mock.patch.object(kd, 'ROUND_FILES', 6), mock.patch.object(kd.Registry, 'evidence', evidence):
            self.assertEqual(driver.execute(), 0)
        self.assertEqual(evidence.call_count, 6)


if __name__ == '__main__':
    unittest.main()
