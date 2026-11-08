import copy
import itertools
import pathlib
import random
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from hevc_dependency import analyze


def graph(refs, *, active=None, pocs=None, skipped=(), wpp=(), long_term=()):
    count = len(refs)
    pocs = pocs if pocs is not None else list(range(count))
    pictures = []
    for index, targets in enumerate(refs):
        pictures.append({
            'index': index, 'poc': pocs[index],
            'reconstruction_required': index not in skipped,
            'wpp': index in wpp, 'slice_types': [1] if (targets if active is None else active[index]) else [2],
            'rps': [[target, pocs[target], (index, target) in long_term] for target in targets],
            'active': [[target, pocs[target], (index, target) in long_term]
                       for target in (targets if active is None else active[index])],
        })
    return {'mode': 'header_dependencies', 'header_only': True, 'complete': True,
            'coded_pictures': count, 'reconstruction_required': count - len(skipped),
            'output_pictures': count, 'nals': count + 3, 'pictures': pictures}


def brute_force(data, field):
    rows = data['pictures']
    vertices = [row['index'] for row in rows if row['reconstruction_required']]

    def reaches(a, b):
        return any(ref[0] == a or reaches(a, ref[0]) for ref in rows[b][field])

    def can_coexist(a, b):
        return not any(any(ref[0] == a for ref in rows[later][field])
                       for later in vertices if a < later <= b)

    dag_width = admission_width = 0
    for size in range(len(vertices) + 1):
        for subset in itertools.combinations(vertices, size):
            pairs = list(itertools.combinations(subset, 2))
            if all(not reaches(a, b) for a, b in pairs):
                dag_width = size
            if all(can_coexist(a, b) for a, b in pairs):
                admission_width = size
    participants = set()
    non_wpp = set()
    for a, b in itertools.combinations(vertices, 2):
        if can_coexist(a, b):
            participants.update((a, b))
            if not rows[a]['wpp'] and not rows[b]['wpp']:
                non_wpp.update((a, b))
    return dag_width, admission_width, len(participants), len(non_wpp)


def emissions(data, pairs):
    data = copy.deepcopy(data)
    data['output_events'] = [{'picture_index': index, 'poc': data['pictures'][index]['poc'],
                              'before_picture': boundary} for index, boundary in pairs]
    data['output_pictures'] = len(pairs)
    return data


def barrier_interval_oracle(data, field, workers, *, delayed_sealing=False):
    rows = data['pictures']
    starts = {}
    for index, row in enumerate(rows):
        if not row['reconstruction_required']:
            continue
        fences = [event['before_picture'] for event in data['output_events']
                  if event['before_picture'] < index + int(delayed_sealing)]
        for time in range(len(rows) + 1):
            if any(start > time for start in starts.values()):
                continue
            if any(starts[ref[0]] + 1 > time for ref in row[field]):
                continue
            if any(start + 1 > time for earlier, start in starts.items()
                   if any(earlier < boundary for boundary in fences)):
                continue
            if sum(start <= time < start + 1 for start in starts.values()) >= workers:
                continue
            starts[index] = time
            break
        else:
            raise AssertionError('no feasible unit start time')
    slots = [[index for index, start in starts.items() if start == time]
             for time in range(len(rows))]
    overlap = {index for slot in slots if len(slot) > 1 for index in slot}
    non_wpp_slots = [[index for index in slot if not rows[index]['wpp']] for slot in slots]
    non_wpp = {index for slot in non_wpp_slots if len(slot) > 1 for index in slot}
    return {'steps': max(starts.values(), default=-1) + 1,
            'max_workers': max(map(len, slots), default=0),
            'overlap_pictures': len(overlap), 'non_wpp_overlap_pictures': len(non_wpp)}


def lag3_event_oracle(data, field, delayed_sealing):
    rows = data['pictures']
    completed = set()
    running = set()
    slots = {}
    pending = []
    maximum_pending = time = 0

    def tick():
        nonlocal time
        time += 1
        completed.update(running)
        running.clear()

    for call in range(len(rows) + 1):
        index = call - int(delayed_sealing)
        if 0 <= index < len(rows) and rows[index]['reconstruction_required']:
            required = {ref[0] for ref in rows[index][field]}
            while not required <= completed or len(running) == 4:
                tick()
            running.add(index)
            slots.setdefault(time, []).append(index)
        if delayed_sealing and call < len(rows):
            required = {ref[0] for ref in rows[call][field]}
            while not required <= completed:
                tick()
        for event in data['output_events']:
            if event['before_picture'] != call:
                continue
            if len(pending) == 3:
                while not pending[0] <= completed:
                    tick()
                pending.pop(0)
            assert len(pending) < 3
            pending.append({index for index in range(call) if rows[index]['reconstruction_required']})
            maximum_pending = max(maximum_pending, len(pending))
        while pending and pending[0] <= completed:
            pending.pop(0)
    if running:
        tick()
    assert all(fence <= completed for fence in pending)
    pending.clear()
    overlap = {index for slot in slots.values() if len(slot) > 1 for index in slot}
    non_wpp_slots = [[index for index in slot if not rows[index]['wpp']] for slot in slots.values()]
    non_wpp = {index for slot in non_wpp_slots if len(slot) > 1 for index in slot}
    return {'steps': time, 'max_workers': max(map(len, slots.values()), default=0),
            'overlap_pictures': len(overlap), 'non_wpp_overlap_pictures': len(non_wpp),
            'max_pending_outputs': maximum_pending}


class HeaderDependencies(unittest.TestCase):
    def test_literal_active_sps_envelope_is_retained_and_validated(self):
        original = graph([[]])
        envelope = dict(coded_width=64, coded_height=64, width=60, height=62,
                        bit_depth=10, ctb_log2=4, dpb_capacity=4, highest_sps_layer=0)
        original['pictures'][0]['envelope'] = envelope
        self.assertEqual(analyze(original)['graph']['pictures'][0]['envelope'], envelope)
        for field, value in [('coded_width', 0), ('height', 65), ('bit_depth', 12),
                             ('ctb_log2', 3), ('dpb_capacity', 0), ('dpb_capacity', 17),
                             ('highest_sps_layer', 7), ('width', True)]:
            changed = copy.deepcopy(original)
            changed['pictures'][0]['envelope'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                analyze(changed)

    def test_strict_chain_has_no_overlap(self):
        result = analyze(graph([[], [0], [1], [2]]))
        for field in ('rps', 'active'):
            metrics = result[field]
            self.assertEqual((metrics['vertices'], metrics['edges']), (4, 3))
            self.assertEqual(metrics['immediate_predecessor_edges'], 3)
            self.assertEqual(metrics['long_term_edges'], 0)
            self.assertEqual(metrics['critical_path_unit_pictures'], 4)
            self.assertEqual(metrics['maximum_antichain_width'], 1)
            self.assertEqual(metrics['unit_cost_work_span_bound'], 1.0)
            self.assertEqual(metrics['coded_admission_width'], 1)
            self.assertEqual(metrics['potential_overlap_pictures'], 0)
            for schedule in metrics['unit_duration_coded_order'].values():
                self.assertEqual(schedule, {'steps': 4, 'max_workers': 1,
                                           'overlap_pictures': 0, 'non_wpp_overlap_pictures': 0})

    def test_shared_anchor_siblings_count_only_two_participants(self):
        metrics = analyze(graph([[], [0], [0]]))['active']
        self.assertEqual(metrics['maximum_antichain_width'], 2)
        self.assertEqual(metrics['coded_admission_width'], 2)
        self.assertEqual(metrics['potential_overlap_pictures'], 2)
        self.assertEqual(metrics['non_wpp_potential_overlap_pictures'], 2)
        self.assertEqual(metrics['unit_duration_coded_order']['2'],
                         {'steps': 2, 'max_workers': 2, 'overlap_pictures': 2,
                          'non_wpp_overlap_pictures': 2})

    def test_independent_pictures_fill_available_workers(self):
        metrics = analyze(graph([[], [], [], [], []]))['active']
        self.assertEqual(metrics['maximum_antichain_width'], 5)
        self.assertEqual(metrics['coded_admission_width'], 5)
        self.assertEqual(metrics['unit_cost_work_span_bound'], 5.0)
        self.assertEqual(metrics['potential_overlap_pictures'], 5)
        for workers, expected in [('1', (5, 1, 0)), ('2', (3, 2, 4)),
                                  ('4', (2, 4, 4)), ('8', (1, 5, 5))]:
            schedule = metrics['unit_duration_coded_order'][workers]
            self.assertEqual((schedule['steps'], schedule['max_workers'],
                              schedule['overlap_pictures']), expected)

    def test_hierarchical_b_graph_uses_coded_order_not_poc_order(self):
        data = graph([[], [0], [0, 1], [0, 2], [1, 2], [0, 3], [2, 3], [2, 4], [1, 4]],
                     pocs=[-8, 8, 0, -4, 4, -6, -2, 2, 6])
        metrics = analyze(data)['rps']
        self.assertEqual(metrics['edges'], 15)
        self.assertEqual(metrics['critical_path_unit_pictures'], 5)
        self.assertEqual(metrics['maximum_antichain_width'], 4)
        self.assertEqual(metrics['coded_admission_width'], 4)
        self.assertEqual(metrics['potential_overlap_pictures'], 6)
        self.assertEqual(metrics['unit_duration_coded_order']['4'],
                         {'steps': 5, 'max_workers': 4, 'overlap_pictures': 6,
                          'non_wpp_overlap_pictures': 6})

    def test_periodic_independent_cvs_roots_do_not_reset_graph_indices(self):
        data = graph([[], [0], [], [2], [], [4]], pocs=[0, 1, 0, 1, 0, 1])
        metrics = analyze(data)['rps']
        self.assertEqual(metrics['maximum_antichain_width'], 3)
        self.assertEqual(metrics['critical_path_unit_pictures'], 2)
        self.assertEqual(metrics['coded_admission_width'], 3)
        self.assertEqual(metrics['potential_overlap_pictures'], 5)
        self.assertEqual(metrics['unit_duration_coded_order']['8']['steps'], 4)
        self.assertEqual(metrics['unit_duration_coded_order']['8']['max_workers'], 2)
        self.assertEqual(metrics['unit_duration_coded_order']['8']['overlap_pictures'], 4)

    def test_fork_unequal_depth_distinguishes_dag_from_coded_admission(self):
        metrics = analyze(graph([[], [0], [0], [1, 2], [0], [4], [5]]))['rps']
        self.assertEqual(metrics['maximum_antichain_width'], 3)
        self.assertEqual(metrics['coded_admission_width'], 2)
        self.assertEqual(metrics['critical_path_unit_pictures'], 4)
        self.assertEqual(metrics['potential_overlap_pictures'], 6)
        self.assertEqual(metrics['unit_duration_coded_order']['8']['max_workers'], 2)
        self.assertEqual(metrics['unit_duration_coded_order']['8']['steps'], 5)

    def test_long_term_identity_and_active_subset_are_separate_graphs(self):
        data = graph([[], [0], [0, 1], [0]], active=[[], [0], [0], []],
                     pocs=[-10, 4, -2, 8], long_term=[(2, 0), (3, 0)])
        result = analyze(data)
        self.assertEqual(result['rps']['long_term_edges'], 2)
        self.assertEqual(result['active']['long_term_edges'], 1)
        self.assertEqual(result['rps']['edges'], 4)
        self.assertEqual(result['active']['edges'], 2)
        self.assertEqual(result['rps']['critical_path_unit_pictures'], 3)
        self.assertEqual(result['active']['critical_path_unit_pictures'], 2)

    def test_union_across_slice_lists_is_consumed_without_dropping_members(self):
        data = graph([[], [], [0, 1]], active=[[], [], [0, 1]])
        data['pictures'][2]['slice_types'] = [2, 1, 0]
        result = analyze(data)
        self.assertEqual(result['active']['edges'], 2)
        self.assertEqual(result['graph']['pictures'][2]['active'], [[0, 0, False], [1, 1, False]])
        self.assertIn('independently signalled segments', result['semantics']['active'])
        self.assertIn('not entropy-observed', result['semantics']['references'])

    def test_skipped_rows_are_not_vertices_or_scheduled_jobs(self):
        data = graph([[], [], [0], []], skipped=[1, 3])
        metrics = analyze(data)['active']
        self.assertEqual(metrics['vertices'], 2)
        self.assertEqual(metrics['edges'], 1)
        self.assertEqual(metrics['immediate_predecessor_edges'], 0)
        self.assertEqual(metrics['maximum_antichain_width'], 1)
        self.assertEqual(metrics['unit_duration_coded_order']['8']['steps'], 2)
        skipped = analyze(graph([[], []], skipped=[0, 1]))['active']
        self.assertEqual(skipped['vertices'], 0)
        self.assertEqual(skipped['critical_path_unit_pictures'], 0)
        self.assertEqual(skipped['maximum_antichain_width'], 0)
        self.assertEqual(skipped['coded_admission_width'], 0)
        self.assertEqual(skipped['unit_cost_work_span_bound'], 0.0)
        self.assertEqual(skipped['unit_duration_coded_order']['1']['steps'], 0)

    def test_single_picture_has_no_overlap(self):
        metrics = analyze(graph([[]]))['active']
        self.assertEqual(metrics['critical_path_unit_pictures'], 1)
        self.assertEqual(metrics['maximum_antichain_width'], 1)
        self.assertEqual(metrics['coded_admission_width'], 1)
        self.assertEqual(metrics['potential_overlap_pictures'], 0)

    def test_wpp_excludes_only_pairs_with_a_wpp_member(self):
        for wpp, expected in [([0, 1, 2], 0), ([0, 1], 0), ([0], 2), ([], 3)]:
            metrics = analyze(graph([[], [], []], wpp=wpp))['active']
            self.assertEqual(metrics['potential_overlap_pictures'], 3)
            self.assertEqual(metrics['non_wpp_potential_overlap_pictures'], expected)
            self.assertEqual(metrics['unit_duration_coded_order']['4']['overlap_pictures'], 3)
            self.assertEqual(metrics['unit_duration_coded_order']['4']['non_wpp_overlap_pictures'], expected)

    def test_raw_graph_is_preserved_and_detached_for_replay(self):
        data = graph([[], [0], [0]])
        original = copy.deepcopy(data)
        result = analyze(data)
        self.assertEqual(data, original)
        self.assertEqual(result['graph'], original)
        self.assertEqual(analyze(result['graph']), result)
        result['graph']['pictures'][1]['rps'].clear()
        self.assertEqual(data, original)

    def test_widths_match_brute_force_subsets_and_interval_predicate(self):
        generator = random.Random(516)
        cases = [graph([[], [0], [0], [1, 2], [0], [4], [5]])]
        for count in range(1, 6):
            pairs = list(itertools.combinations(range(count), 2))
            for mask in range(1 << len(pairs)):
                refs = [[] for _ in range(count)]
                for bit, (a, b) in enumerate(pairs):
                    if mask & (1 << bit):
                        refs[b].append(a)
                cases.append(graph(refs))
        for count in (6, 7):
            for _ in range(30):
                refs = [[a for a in range(b) if generator.random() < 0.35] for b in range(count)]
                cases.append(graph(refs, wpp=[a for a in range(count) if a % 2]))
        for data in cases:
            result = analyze(data)
            for field in ('rps', 'active'):
                metrics = result[field]
                with self.subTest(refs=[row[field] for row in data['pictures']]):
                    self.assertEqual((metrics['maximum_antichain_width'], metrics['coded_admission_width'],
                                      metrics['potential_overlap_pictures'],
                                      metrics['non_wpp_potential_overlap_pictures']), brute_force(data, field))

    def test_missing_root_and_row_fields_are_rejected(self):
        original = graph([[], [0]])
        for field in original:
            changed = copy.deepcopy(original)
            del changed[field]
            with self.subTest(root=field), self.assertRaises(ValueError):
                analyze(changed)
        for field in original['pictures'][1]:
            changed = copy.deepcopy(original)
            del changed['pictures'][1][field]
            with self.subTest(row=field), self.assertRaises(ValueError):
                analyze(changed)

    def test_root_count_and_completeness_mutations_are_rejected(self):
        original = graph([[], [0]])
        mutations = {'mode': [None, 'decode', True], 'header_only': [False, 1],
                     'complete': [False, 1], 'coded_pictures': [True, 0, 1, 3, 33, 2.0],
                     'reconstruction_required': [True, -1, 1, 3, 2.0],
                     'output_pictures': [True, -1, 3, 2.0], 'nals': [True, 1, -1, 4.0],
                     'pictures': [None, {}, [], original['pictures'][:1],
                                  original['pictures'] * 2, [None, original['pictures'][1]]]}
        for field, values in mutations.items():
            for value in values:
                changed = copy.deepcopy(original)
                changed[field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyze(changed)
        for value in (None, [], 1, True):
            with self.subTest(value=value), self.assertRaises(ValueError):
                analyze(value)
        accepted = graph([[], [0]])
        accepted['output_pictures'] = 0
        accepted['nals'] = 2
        self.assertEqual(analyze(accepted)['rps']['vertices'], 2)
        self.assertEqual(analyze(graph([[] for _ in range(32)]))['rps']['vertices'], 32)

    def test_row_and_reference_mutations_are_rejected(self):
        original = graph([[], [0]])
        mutations = {'index': [True, 0, 2, 1.0], 'poc': [True, 1.0, '1', None, -(1 << 31) - 1, 1 << 31],
                     'reconstruction_required': [0, 1, None, False], 'wpp': [0, 1, None],
                     'slice_types': [None, {}, [], [True], [1.0], [-1], [3], [1, 1], [[1]]],
                     'rps': [None, {}, [None], [[0]], [[0, 0]], [[0, 0, False, 1]],
                             [[True, 0, False]], [[0, True, False]], [[0, 0, 0]],
                             [[0.0, 0, False]], [[0, 0.0, False]], [[0, 9, False]],
                             [[-1, 0, False]], [[1, 1, False]], [[2, 2, False]],
                             [[0, 0, False], [0, 0, False]],
                             [[0, 0, False], [0, 0, True]], [[0, 0, False]] * 17],
                     'active': [None, {}, [], [None], [[0, 0, True]], [[1, 1, False]],
                                [[0, 0, False], [0, 0, False]], [[0, 0, False]] * 17]}
        for field, values in mutations.items():
            for value in values:
                changed = copy.deepcopy(original)
                changed['pictures'][1][field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyze(changed)
        changed = graph([[], [], [1]], skipped=[1])
        with self.assertRaises(ValueError):
            analyze(changed)
        changed = graph([[], [], [1]], active=[[], [], [0]])
        with self.assertRaises(ValueError):
            analyze(changed)
        changed = graph([[], [0], [0]])
        changed['pictures'][0]['rps'] = [[2, 2, False]]
        with self.assertRaises(ValueError):
            analyze(changed)
        changed = graph([[], [0]])
        changed['pictures'][1]['slice_types'] = [2]
        with self.assertRaises(ValueError):
            analyze(changed)

    def test_sixteen_unique_references_are_allowed_but_seventeen_are_not(self):
        data = graph([[] for _ in range(17)] + [list(range(16))])
        self.assertEqual(analyze(data)['rps']['edges'], 16)
        data['pictures'][-1]['rps'].append([16, 16, False])
        with self.assertRaises(ValueError):
            analyze(data)


class ReceiveBarriers(unittest.TestCase):
    def test_absent_events_preserve_legacy_results_and_empty_events_match_old_schedule(self):
        for refs in ([[]], [[], [], [], [], []], [[], [0], [0], [1, 2], [0], [4], [5]]):
            data = graph(refs)
            legacy = analyze(data)
            present = analyze(emissions(data, []))
            self.assertNotIn('unit_duration_receive_barrier', legacy['semantics'])
            for field in ('rps', 'active'):
                metrics = present[field].copy()
                self.assertNotIn('unit_duration_receive_barrier', legacy[field])
                self.assertEqual(metrics.pop('unit_duration_receive_barrier'),
                                 legacy[field]['unit_duration_coded_order'])
                self.assertEqual(metrics.pop('unit_duration_delayed_sealing_receive_barrier'),
                                 legacy[field]['unit_duration_coded_order'])
                lagged = metrics.pop('unit_duration_output_lag3')
                for mode in ('picture_ready', 'delayed_sealing'):
                    expected = {**legacy[field]['unit_duration_coded_order']['4'], 'max_pending_outputs': 0}
                    self.assertEqual(lagged[mode]['full_picture_references'], expected)
                    self.assertEqual(lagged[mode]['row_references_worst_case'], expected)
                self.assertEqual(metrics, legacy[field])

    def test_independent_five_with_every_output_boundary_peak_at_two(self):
        data = emissions(graph([[], [], [], [], []]), [(i, i + 1) for i in range(5)])
        result = analyze(data)
        for field in ('rps', 'active'):
            self.assertEqual(result[field]['unit_duration_receive_barrier']['4'],
                             {'steps': 3, 'max_workers': 2, 'overlap_pictures': 4,
                              'non_wpp_overlap_pictures': 4})
            self.assertEqual(result[field]['unit_duration_coded_order']['4']['steps'], 2)
            self.assertEqual(result[field]['unit_duration_coded_order']['4']['max_workers'], 4)
        self.assertEqual(result['graph']['output_events'], data['output_events'])
        self.assertEqual(analyze(result['graph']), result)
        self.assertIn('one-picture-late job sealing', result['semantics']['unit_duration_receive_barrier'])
        self.assertIn('not measured throughput', result['semantics']['unit_duration_receive_barrier'])

    def test_anchor_with_four_children_has_three_steps_instead_of_two(self):
        data = emissions(graph([[], [0], [0], [0], [0]]), [(i, i + 1) for i in range(5)])
        result = analyze(data)['active']
        self.assertEqual(result['unit_duration_receive_barrier']['4'],
                         {'steps': 3, 'max_workers': 2, 'overlap_pictures': 4,
                          'non_wpp_overlap_pictures': 4})
        self.assertEqual(result['unit_duration_coded_order']['4'],
                         {'steps': 2, 'max_workers': 4, 'overlap_pictures': 4,
                          'non_wpp_overlap_pictures': 4})

    def test_strict_chains_remain_serial_with_every_boundary(self):
        data = emissions(graph([[], [0], [1], [2]]), [(i, i + 1) for i in range(4)])
        for field in ('rps', 'active'):
            metrics = analyze(data)[field]
            self.assertEqual(metrics['unit_duration_receive_barrier'], metrics['unit_duration_coded_order'])
            self.assertEqual(metrics['unit_duration_receive_barrier']['8'],
                             {'steps': 4, 'max_workers': 1, 'overlap_pictures': 0,
                              'non_wpp_overlap_pictures': 0})

    def test_fence_waits_for_unrelated_earlier_jobs_even_if_output_picture_finished(self):
        data = emissions(graph([[], [0], [], [], []]), [(0, 3)])
        metrics = analyze(data)['active']
        self.assertEqual(metrics['unit_duration_receive_barrier']['4'],
                         {'steps': 3, 'max_workers': 3, 'overlap_pictures': 3,
                          'non_wpp_overlap_pictures': 3})
        self.assertEqual(metrics['unit_duration_coded_order']['4']['steps'], 2)
        self.assertEqual(metrics['unit_duration_coded_order']['4']['overlap_pictures'], 4)

    def test_skipped_sends_still_apply_barriers_and_eof_finishes_jobs(self):
        data = emissions(graph([[], [], [], []], skipped=[1]), [(0, 1), (2, 4), (3, 4)])
        metrics = analyze(data)['active']
        self.assertEqual(metrics['unit_duration_receive_barrier']['4'],
                         {'steps': 2, 'max_workers': 2, 'overlap_pictures': 2,
                          'non_wpp_overlap_pictures': 2})
        self.assertEqual(metrics['unit_duration_coded_order']['4']['max_workers'], 3)
        empty = emissions(graph([[], []], skipped=[0, 1]), [])
        self.assertEqual(analyze(empty)['active']['unit_duration_receive_barrier']['4'],
                         {'steps': 0, 'max_workers': 0, 'overlap_pictures': 0,
                          'non_wpp_overlap_pictures': 0})
        eof_only = emissions(graph([[], [], []]), [(2, 3), (0, 3), (1, 3)])
        metrics = analyze(eof_only)['active']
        self.assertEqual(metrics['unit_duration_receive_barrier'], metrics['unit_duration_coded_order'])

    def test_output_bursts_at_one_boundary_are_one_prefix_fence(self):
        data = emissions(graph([[], [], [], []], pocs=[2, -1, 0, 2]),
                         [(1, 2), (0, 2), (3, 4), (2, 4)])
        self.assertEqual(analyze(data)['active']['unit_duration_receive_barrier']['4'],
                         {'steps': 2, 'max_workers': 3, 'overlap_pictures': 3,
                          'non_wpp_overlap_pictures': 3})

    def test_non_wpp_counts_use_real_interval_pairs_not_queued_or_completed_jobs(self):
        for wpp, expected in [([], 4), ([0], 2), ([0, 2], 0), (list(range(5)), 0)]:
            data = emissions(graph([[], [], [], [], []], wpp=wpp), [(i, i + 1) for i in range(5)])
            metrics = analyze(data)['active']['unit_duration_receive_barrier']['4']
            self.assertEqual(metrics['overlap_pictures'], 4)
            self.assertEqual(metrics['non_wpp_overlap_pictures'], expected)

    def test_event_mutations_reject_missing_identity_order_count_and_boolean_numbers(self):
        original = emissions(graph([[], [], []]), [(0, 1), (1, 2), (2, 3)])
        for value in (None, {}, 1, [], original['output_events'][:-1], original['output_events'] * 2):
            changed = copy.deepcopy(original)
            changed['output_events'] = value
            with self.subTest(events=value), self.assertRaises(ValueError):
                analyze(changed)
        for field, values in {'picture_index': [True, False, 0.0, '0', -1, 3, 1],
                              'poc': [True, False, 0.0, '0', -1, 3],
                              'before_picture': [True, False, 1.0, '1', 0, -1, 4]}.items():
            for value in values:
                changed = copy.deepcopy(original)
                changed['output_events'][0][field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyze(changed)
        for field in original['output_events'][0]:
            changed = copy.deepcopy(original)
            del changed['output_events'][0][field]
            with self.subTest(missing=field), self.assertRaises(ValueError):
                analyze(changed)
        for value in (None, [], [0, 0, 1]):
            changed = copy.deepcopy(original)
            changed['output_events'][0] = value
            with self.subTest(event=value), self.assertRaises(ValueError):
                analyze(changed)
        duplicate = copy.deepcopy(original)
        duplicate['output_events'][1] = duplicate['output_events'][0].copy()
        with self.assertRaises(ValueError):
            analyze(duplicate)
        out_of_order = emissions(graph([[], [], []]), [(0, 3), (1, 2)])
        with self.assertRaises(ValueError):
            analyze(out_of_order)
        skipped = emissions(graph([[], [], []], skipped=[1]), [(1, 2)])
        with self.assertRaises(ValueError):
            analyze(skipped)

    def test_small_dags_and_barrier_subsets_match_independent_interval_oracle(self):
        pairs = list(itertools.combinations(range(4), 2))
        for edge_mask in range(1 << len(pairs)):
            refs = [[] for _ in range(4)]
            for bit, (a, b) in enumerate(pairs):
                if edge_mask & (1 << bit):
                    refs[b].append(a)
            active = [targets[::2] for targets in refs]
            for boundary_mask in range(1 << 4):
                data = emissions(graph(refs, active=active, wpp=[1]),
                                 [(b - 1, b) for b in range(1, 5) if boundary_mask & (1 << (b - 1))])
                result = analyze(data)
                for field in ('rps', 'active'):
                    for workers in (1, 2, 4, 8):
                        with self.subTest(edges=edge_mask, barriers=boundary_mask,
                                          field=field, workers=workers):
                            self.assertEqual(result[field]['unit_duration_receive_barrier'][str(workers)],
                                             barrier_interval_oracle(data, field, workers))
                            self.assertEqual(
                                result[field]['unit_duration_delayed_sealing_receive_barrier'][str(workers)],
                                barrier_interval_oracle(data, field, workers, delayed_sealing=True))


class DelayedSealing(unittest.TestCase):
    def test_absent_events_do_not_invent_delayed_sealing_barriers(self):
        result = analyze(graph([[], [], []]))
        field = 'unit_duration_delayed_sealing_receive_barrier'
        self.assertNotIn(field, result['semantics'])
        for kind in ('rps', 'active'):
            self.assertNotIn(field, result[kind])

    def test_no_events_controls_preserve_reference_and_worker_limits(self):
        for refs in ([[]], [[], [], [], [], []], [[], [0], [0], [0], [0]],
                     [[], [0], [1], [2]], [[], [0], [0], [1, 2], [0], [4], [5]]):
            result = analyze(emissions(graph(refs, wpp=[0]), []))
            for field in ('rps', 'active'):
                self.assertEqual(result[field]['unit_duration_delayed_sealing_receive_barrier'],
                                 result[field]['unit_duration_coded_order'])

    def test_anchor_children_are_serial_when_each_receive_fences_the_sealed_prefix(self):
        data = emissions(graph([[], [0], [0], [0], [0]]), [(i, i + 1) for i in range(5)])
        result = analyze(data)
        for field in ('rps', 'active'):
            self.assertEqual(result[field]['unit_duration_receive_barrier']['4'],
                             {'steps': 3, 'max_workers': 2, 'overlap_pictures': 4,
                              'non_wpp_overlap_pictures': 4})
            for schedule in result[field]['unit_duration_delayed_sealing_receive_barrier'].values():
                self.assertEqual(schedule, {'steps': 5, 'max_workers': 1, 'overlap_pictures': 0,
                                           'non_wpp_overlap_pictures': 0})
        label = result['semantics']['unit_duration_delayed_sealing_receive_barrier']
        self.assertIn('before accepting its input', label)
        self.assertIn('actual Scheduler.required_references used-RPS gate', label)
        self.assertIn('not measured throughput', label)

    def test_chain_stays_serial_with_delayed_sealing_and_barriers(self):
        data = emissions(graph([[], [0], [1], [2]]), [(i, i + 1) for i in range(4)])
        for field in ('rps', 'active'):
            result = analyze(data)[field]
            self.assertEqual(result['unit_duration_delayed_sealing_receive_barrier'],
                             result['unit_duration_coded_order'])

    def test_skipped_calls_seal_previous_job_and_still_apply_fences(self):
        data = emissions(graph([[], [], [], []], skipped=[1]), [(0, 1), (2, 4), (3, 4)])
        for field in ('rps', 'active'):
            schedules = analyze(data)[field]['unit_duration_delayed_sealing_receive_barrier']
            self.assertEqual(schedules['4'], {'steps': 2, 'max_workers': 2, 'overlap_pictures': 2,
                                             'non_wpp_overlap_pictures': 2})
        trailing_skips = emissions(graph([[], [], []], skipped=[1, 2]), [(0, 3)])
        self.assertEqual(analyze(trailing_skips)['rps']['unit_duration_delayed_sealing_receive_barrier']['8'],
                         {'steps': 1, 'max_workers': 1, 'overlap_pictures': 0,
                          'non_wpp_overlap_pictures': 0})
        all_skipped = emissions(graph([[], []], skipped=[0, 1]), [])
        self.assertEqual(analyze(all_skipped)['rps']['unit_duration_delayed_sealing_receive_barrier']['8'],
                         {'steps': 0, 'max_workers': 0, 'overlap_pictures': 0,
                          'non_wpp_overlap_pictures': 0})

    def test_eof_submits_final_job_but_output_burst_fences_all_submitted_jobs(self):
        data = emissions(graph([[], [], [], []]), [(1, 2), (0, 2), (3, 4), (2, 4)])
        metrics = analyze(data)['active']
        self.assertEqual(metrics['unit_duration_delayed_sealing_receive_barrier']['4'],
                         {'steps': 2, 'max_workers': 2, 'overlap_pictures': 4,
                          'non_wpp_overlap_pictures': 4})
        eof_only = emissions(graph([[], [], []], wpp=[0, 1, 2]), [(2, 3), (0, 3), (1, 3)])
        self.assertEqual(analyze(eof_only)['rps']['unit_duration_delayed_sealing_receive_barrier']['4'],
                         {'steps': 1, 'max_workers': 3, 'overlap_pictures': 3,
                          'non_wpp_overlap_pictures': 0})


class OutputLagThree(unittest.TestCase):
    def test_legacy_records_do_not_invent_output_lag_results(self):
        result = analyze(graph([[], [], []]))
        self.assertNotIn('unit_duration_output_lag3', result['semantics'])
        for field in ('rps', 'active'):
            self.assertNotIn('unit_duration_output_lag3', result[field])

    def check_models(self, data):
        result = analyze(data)
        for field in ('rps', 'active'):
            lagged = result[field]['unit_duration_output_lag3']
            self.assertEqual(lagged['workers'], 4)
            self.assertEqual(lagged['output_lag_limit'], 3)
            for mode in ('picture_ready', 'delayed_sealing'):
                metrics = lagged[mode]['full_picture_references']
                self.assertEqual(metrics, lag3_event_oracle(data, field, mode == 'delayed_sealing'))
                self.assertEqual(lagged[mode]['row_references_worst_case'], metrics)
                self.assertLessEqual(metrics['max_pending_outputs'], 3)
        return result

    def test_independent_and_anchor_controls_fill_four_workers_without_exceeding_lag(self):
        for refs, steps, overlap in [([[] for _ in range(8)], 2, 8),
                                     ([[], [0], [0], [0], [0]], 2, 4)]:
            result = self.check_models(emissions(graph(refs), [(i, i + 1) for i in range(len(refs))]))
            for mode in ('picture_ready', 'delayed_sealing'):
                metrics = result['rps']['unit_duration_output_lag3'][mode]['full_picture_references']
                self.assertEqual(metrics, {'steps': steps, 'max_workers': 4, 'overlap_pictures': overlap,
                                           'non_wpp_overlap_pictures': overlap, 'max_pending_outputs': 3})

    def test_strict_chain_stays_serial(self):
        result = self.check_models(emissions(graph([[], [0], [1], [2], [3]]),
                                             [(i, i + 1) for i in range(5)]))
        for mode in ('picture_ready', 'delayed_sealing'):
            metrics = result['rps']['unit_duration_output_lag3'][mode]['full_picture_references']
            self.assertEqual((metrics['steps'], metrics['max_workers'], metrics['overlap_pictures']), (5, 1, 0))

    def test_four_record_burst_consumes_oldest_full_prefix_before_fourth_append(self):
        data = emissions(graph([[], [0], [1], [], [], []]), [(0, 4), (1, 4), (2, 4), (3, 4)])
        result = self.check_models(data)['rps']
        metrics = result['unit_duration_output_lag3']['picture_ready']['full_picture_references']
        self.assertEqual(metrics, {'steps': 4, 'max_workers': 3, 'overlap_pictures': 3,
                                   'non_wpp_overlap_pictures': 3, 'max_pending_outputs': 3})
        self.assertEqual(result['unit_duration_coded_order']['4']['steps'], 3)
        self.assertEqual(result['unit_duration_coded_order']['4']['max_workers'], 4)

    def test_skipped_sends_eof_bursts_and_all_skipped_records(self):
        cases = [emissions(graph([[], [], [], [], []], skipped=[1]),
                           [(0, 1), (2, 5), (4, 5), (3, 5)]),
                 emissions(graph([[], [], []], skipped=[1, 2]), [(0, 3)]),
                 emissions(graph([[], []], skipped=[0, 1]), []),
                 emissions(graph([[] for _ in range(8)], wpp=list(range(8))),
                           [(i, 8) for i in reversed(range(8))])]
        for data in cases:
            result = self.check_models(data)
            for mode in ('picture_ready', 'delayed_sealing'):
                metrics = result['rps']['unit_duration_output_lag3'][mode]['full_picture_references']
                self.assertEqual(metrics['steps'], result['rps']['unit_duration_coded_order']['4']['steps'])
        for mode in ('picture_ready', 'delayed_sealing'):
            metrics = result['rps']['unit_duration_output_lag3'][mode]['full_picture_references']
            self.assertEqual(metrics['max_pending_outputs'], 3)
            self.assertEqual(metrics['non_wpp_overlap_pictures'], 0)

    def test_consumption_never_reclaims_a_later_reference_or_invents_row_benefits(self):
        data = emissions(graph([[], [0], [], [], [], [0]]), [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6)])
        result = self.check_models(data)
        label = result['semantics']['unit_duration_output_lag3']
        self.assertIn('not implementability under the current Decoder send/receive API', label)
        self.assertIn('no CTU-row or MV footprints', label)
        self.assertIn('publication is not reclamation', label)
        self.assertIn('logical pending emitted frames, not an allocated-byte/capacity bound', label)

    def test_all_four_vertex_dags_and_event_placements_match_tick_oracle(self):
        pairs = list(itertools.combinations(range(4), 2))
        placements = list(itertools.product(*[range(index + 1, 5) for index in range(4)]))
        for edge_mask in range(1 << len(pairs)):
            refs = [[] for _ in range(4)]
            for bit, (a, b) in enumerate(pairs):
                if edge_mask & (1 << bit):
                    refs[b].append(a)
            for placement in placements:
                data = emissions(graph(refs, active=[targets[::2] for targets in refs], wpp=[0]),
                                 sorted(enumerate(placement), key=lambda pair: pair[1]))
                with self.subTest(edges=edge_mask, placement=placement):
                    self.check_models(data)


if __name__ == '__main__':
    unittest.main()
