import copy
import itertools
import pathlib
import random
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from hevc_row_dependency import analyze
from hevc_dependency import analyze as analyze_header


def trace(counts, reads=None, *, rps=None, active=None, tiles=(), outputs=None):
    reads = reads or {}
    rps = rps if rps is not None else [sorted({target for row in range(count or 0)
                                              for target in reads.get((index, row), {})})
                                      for index, count in enumerate(counts)]
    active = active if active is not None else rps
    headers = []
    pictures = []
    for index, count in enumerate(counts):
        headers.append({'index': index, 'poc': index * 2 - 4, 'wpp': False,
                        'reconstruction_required': count is not None,
                        'slice_types': [1] if active[index] else [2],
                        'rps': [[target, target * 2 - 4, False] for target in rps[index]],
                        'active': [[target, target * 2 - 4, False] for target in active[index]]})
        if count is None:
            continue
        rows = []
        for row in range(count):
            refs = [[target, target * 2 - 4, sample, motion]
                    for target, (sample, motion) in reads.get((index, row), {}).items()]
            rows.append({'row': row, 'references': refs,
                         'sample_calls': sum(ref[2] is not None for ref in refs),
                         'motion_calls': sum(ref[3] is not None for ref in refs)})
        pictures.append({'index': index, 'poc': index * 2 - 4, 'width': 32, 'height': count * 16,
                         'ctb_log2': 4, 'tiles': index in tiles, 'wpp': False, 'rows': rows})
    outputs = outputs if outputs is not None else [(index, len(counts)) for index, count in enumerate(counts)
                                                  if count is not None]
    header = {'mode': 'header_dependencies', 'header_only': True, 'complete': True,
              'coded_pictures': len(counts), 'reconstruction_required': len(pictures),
              'output_pictures': len(outputs), 'nals': len(counts) + 3, 'pictures': headers,
              'output_events': [{'picture_index': index, 'poc': index * 2 - 4, 'before_picture': before}
                                for index, before in outputs]}
    return {'mode': 'row_dependencies', 'complete': True, 'header_only': False,
            'requested_threads': 1, 'thread_limit': 1, 'max_workers': 1,
            'decoded_pictures': len(pictures), 'output_pictures': len(outputs),
            'header_dependencies': header, 'pictures': pictures}


def tick_oracle(data, lag, delayed):
    pictures = {picture['index']: picture for picture in data['pictures']}
    header = data['header_dependencies']
    rows_done = {index: 0 for index in pictures}
    tile_work = {index: 0 for index in pictures}
    finished = set()
    active = set()
    pending = []
    participants = set()
    time = peak = inflight = pending_peak = 0

    def ready(index):
        picture = pictures[index]
        if picture['tiles'] or lag is None:
            return all(ref[0] in finished for ref in header['pictures'][index]['rps'])
        row = picture['rows'][rows_done[index]]
        for target, _, sample, motion in row['references']:
            if pictures[target]['tiles']:
                if target not in finished:
                    return False
                continue
            if sample is not None:
                needed = min(sample + lag, len(pictures[target]['rows']) - 1)
                if rows_done[target] <= needed:
                    return False
            if motion is not None and rows_done[target] <= motion:
                return False
        return True

    def tick():
        nonlocal time, peak
        jobs = [index for index in sorted(active) if ready(index)]
        assert jobs
        assert len(jobs) <= 4
        peak = max(peak, len(jobs))
        if len(jobs) > 1:
            participants.update(jobs)
        for index in jobs:
            picture = pictures[index]
            if picture['tiles']:
                tile_work[index] += 1
                if tile_work[index] == len(picture['rows']):
                    rows_done[index] = len(picture['rows'])
            else:
                rows_done[index] += 1
            if rows_done[index] == len(picture['rows']):
                finished.add(index)
                active.remove(index)
        time += 1

    def fence_ready(boundary):
        return all(index in finished for index in pictures if index < boundary)

    for call in range(header['coded_pictures'] + 1):
        index = call - int(delayed)
        if index in pictures:
            required = {ref[0] for ref in header['pictures'][index]['rps']} if lag is None else set()
            while len(active) == 4 or not required <= finished:
                tick()
            active.add(index)
            inflight = max(inflight, len(active))
        if delayed and lag is None and call < header['coded_pictures']:
            required = {ref[0] for ref in header['pictures'][call]['rps']}
            while not required <= finished:
                tick()
        for event in header['output_events']:
            if event['before_picture'] != call:
                continue
            if len(pending) == 3:
                while not fence_ready(pending[0]):
                    tick()
                pending.pop(0)
            assert len(pending) < 3
            pending.append(call)
            pending_peak = max(pending_peak, len(pending))
        while pending and fence_ready(pending[0]):
            pending.pop(0)
    while active:
        tick()
    assert all(fence_ready(boundary) for boundary in pending)
    return {'steps': time, 'max_workers': peak, 'overlap_pictures': len(participants),
            'max_in_flight_pictures': inflight, 'max_pending_outputs': pending_peak}


class RowDependencies(unittest.TestCase):
    def check_oracle(self, data):
        result = analyze(data)
        for mode in ('picture_ready', 'delayed_sealing'):
            for name, lag in (('filter_lag1', 1), ('filter_lag2', 2), ('whole_reference_control', None)):
                model = result['models'][mode][name]
                expected = tick_oracle(data, lag, mode == 'delayed_sealing')
                self.assertEqual({key: model[key] for key in expected}, expected)
                self.assertLessEqual(model['max_workers'], 4)
                self.assertLessEqual(model['max_in_flight_pictures'], 4)
                self.assertLessEqual(model['max_pending_outputs'], 3)
                self.assertLessEqual(model['critical_path_row_units'], model['steps'])
                self.assertLessEqual(model['serial_work_over_steps'], model['four_worker_work_span_bound'])
                self.assertLessEqual(model['four_worker_work_span_bound'], 4.0)
        return result

    def test_forward_chain_has_explicit_sample_filter_margin_and_whole_reference_control(self):
        data = trace([4, 4], {(1, row): {0: (row, None)} for row in range(4)})
        result = self.check_oracle(data)
        for mode in result['models'].values():
            self.assertEqual(mode['filter_lag1']['steps'], 6)
            self.assertEqual(mode['filter_lag1']['critical_path_row_units'], 6)
            self.assertEqual(mode['filter_lag1']['max_workers'], 2)
            self.assertEqual(mode['filter_lag1']['serial_row_work'], 8)
            self.assertEqual(mode['filter_lag1']['normalized_picture_work'], 2)
            self.assertEqual(mode['filter_lag1']['normalized_picture_span'], 1.5)
            self.assertAlmostEqual(mode['filter_lag1']['serial_work_over_steps'], 8 / 6)
            self.assertAlmostEqual(mode['filter_lag1']['four_worker_work_span_bound'], 8 / 6)
            self.assertEqual(mode['filter_lag2']['steps'], 7)
            self.assertEqual(mode['whole_reference_control']['steps'], 8)
            self.assertEqual(mode['whole_reference_control']['max_workers'], 1)

    def test_far_future_reads_wait_for_complete_reference_not_just_current_row(self):
        result = self.check_oracle(trace([4, 3], {(1, row): {0: (3, None)} for row in range(3)}))
        for mode in result['models'].values():
            self.assertEqual(mode['filter_lag1']['steps'], 7)
            self.assertEqual(mode['filter_lag2']['steps'], 7)
            self.assertEqual(mode['filter_lag1']['overlap_pictures'], 0)

    def test_tmvp_waits_only_for_motion_row_and_may_reference_rps_outside_active(self):
        data = trace([4, 4], {(1, row): {0: (None, row)} for row in range(4)})
        result = self.check_oracle(data)
        for mode in result['models'].values():
            self.assertEqual(mode['filter_lag1']['steps'], 5)
            self.assertEqual(mode['filter_lag2']['steps'], 5)
        data = trace([2, 2, 2], {(2, 0): {0: (None, 1), 1: (0, None)}},
                     rps=[[], [], [0, 1]], active=[[], [], [1]])
        self.assertEqual(self.check_oracle(data)['graph'], data)

    def test_header_references_without_actual_reads_do_not_become_row_dependencies(self):
        data = trace([4, 4], rps=[[], [0]])
        result = self.check_oracle(data)
        for mode in result['models'].values():
            self.assertEqual(mode['filter_lag1']['steps'], 4)
            self.assertEqual(mode['filter_lag1']['max_workers'], 2)
            self.assertEqual(mode['whole_reference_control']['steps'], 8)

    def test_unit_row_whole_reference_control_matches_existing_lag3_model(self):
        for refs in ([[], [], [], [], []], [[], [0], [1], [2]], [[], [0], [0], [0], [0]],
                     [[], [0], [0], [1, 2], [0], [4], [5]]):
            data = trace([1] * len(refs), {(index, 0): {target: (0, None) for target in targets}
                                          for index, targets in enumerate(refs)},
                         outputs=[(index, index + 1) for index in range(len(refs))])
            result = self.check_oracle(data)
            control = analyze_header(data['header_dependencies'])['rps']['unit_duration_output_lag3']
            for mode in ('picture_ready', 'delayed_sealing'):
                actual = result['models'][mode]['whole_reference_control']
                for key, value in control[mode]['full_picture_references'].items():
                    if key != 'non_wpp_overlap_pictures':
                        self.assertEqual(actual[key], value)

    def test_tiled_current_or_reference_uses_atomic_whole_picture_fallback(self):
        for tiles in ([0], [1], [0, 1]):
            result = self.check_oracle(trace([4, 4], {(1, row): {0: (0, None)} for row in range(4)},
                                             tiles=tiles))
            self.assertEqual(result['tiled_picture_indices'], tiles)
            for mode in result['models'].values():
                self.assertEqual(mode['filter_lag1']['steps'], 8)
                self.assertEqual(mode['filter_lag1']['critical_path_row_units'], 8)
                self.assertEqual(mode['filter_lag1']['max_workers'], 1)
        self.assertIn('no raster-row entropy order', result['semantics']['tiles'])

    def test_unequal_row_counts_keep_equal_row_cost_normalization(self):
        result = self.check_oracle(trace([4, 2], {(1, 0): {0: (0, None)}, (1, 1): {0: (3, None)}}))
        metrics = result['models']['picture_ready']['filter_lag1']
        self.assertEqual(metrics['steps'], 5)
        self.assertEqual(metrics['serial_row_work'], 6)
        self.assertAlmostEqual(metrics['normalized_picture_steps'], 5 / 3)
        self.assertEqual(metrics['normalized_picture_work'], 2)

    def test_four_worker_bound_is_distinct_from_scheduled_ratio_and_capped_at_four(self):
        metrics = self.check_oracle(trace([2] * 8))['models']['picture_ready']['filter_lag1']
        self.assertEqual(metrics['unit_row_work_span_bound'], 8.0)
        self.assertEqual(metrics['four_worker_work_span_bound'], 4.0)
        self.assertEqual(metrics['serial_work_over_steps'], 4.0)
        data = trace([1, 1, 1, 5, 1, 1, 1], outputs=[(0, 4), (1, 4), (2, 4), (3, 4)])
        metrics = self.check_oracle(data)['models']['picture_ready']['filter_lag1']
        self.assertGreater(metrics['four_worker_work_span_bound'], metrics['serial_work_over_steps'])

    def test_geometry_rejects_bigints_and_accepts_positive_u32_width_boundary(self):
        for field in ('width', 'height', 'ctb_log2'):
            for value in (1 << 32, 1 << 128):
                data = trace([1])
                data['pictures'][0][field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyze(data)
        data = trace([1])
        data['pictures'][0]['width'] = (1 << 32) - 1
        self.assertEqual(analyze(data)['models']['picture_ready']['filter_lag1']['steps'], 1)

    def test_skipped_sends_output_bursts_and_eof_preserve_lag_and_inflight_limits(self):
        for data in (trace([2, None, 3, 1, 2, 4], outputs=[(0, 1), (2, 5), (3, 5), (4, 5), (5, 6)]),
                     trace([None, None], outputs=[]), trace([3]),
                     trace([2] * 8, outputs=[(index, 8) for index in reversed(range(8))])):
            self.check_oracle(data)
        result = analyze(trace([2] * 8))
        for mode in result['models'].values():
            self.assertEqual(mode['filter_lag1']['steps'], 4)
            self.assertEqual(mode['filter_lag1']['max_workers'], 4)
            self.assertEqual(mode['filter_lag1']['max_pending_outputs'], 3)

    def test_output_fences_include_unrelated_earlier_picture_rows(self):
        data = trace([1, 1, 1, 5, 1, 1, 1], outputs=[(0, 4), (1, 4), (2, 4), (3, 4)])
        result = self.check_oracle(data)
        self.assertEqual(result['models']['picture_ready']['filter_lag1']['steps'], 6)
        self.assertIn('all-rows-of-all-earlier-pictures', result['semantics']['outputs'])

    def test_raw_capture_is_detached_and_replayable_with_explicit_hypothetical_limits(self):
        data = trace([3, 2], {(1, 0): {0: (0, 0)}})
        original = copy.deepcopy(data)
        result = analyze(data)
        self.assertEqual(data, original)
        self.assertEqual(analyze(result['graph']), result)
        result['graph']['pictures'][0]['rows'].clear()
        self.assertEqual(data, original)
        self.assertIn('not current whole-picture filtering', result['semantics']['filter_lag1'])
        self.assertIn('not implementability', result['semantics']['limits'])
        self.assertIn('replace whole-reference input gates', result['semantics']['admission'])

    def test_small_exhaustive_read_maxima_and_random_multi_picture_traces_match_tick_oracle(self):
        for reference_rows in range(1, 4):
            for current_rows in range(1, 4):
                for maxima in itertools.product(range(-1, reference_rows), repeat=current_rows):
                    for motion in (False, True):
                        reads = {(1, row): {0: (None, q) if motion else (q, None)}
                                 for row, q in enumerate(maxima) if q >= 0}
                        self.check_oracle(trace([reference_rows, current_rows], reads, rps=[[], [0]]))
        generator = random.Random(704)
        for _ in range(80):
            counts = [generator.randrange(1, 5) for _ in range(6)]
            reads = {}
            for index, count in enumerate(counts):
                for row in range(count):
                    refs = {}
                    for target in range(index):
                        if generator.random() < 0.25:
                            refs[target] = (generator.randrange(counts[target]),
                                            generator.randrange(counts[target]) if generator.random() < 0.5 else None)
                    reads[index, row] = refs
            outputs = sorted(((index, generator.randrange(index + 1, 7)) for index in range(6)),
                             key=lambda pair: pair[1])
            self.check_oracle(trace(counts, reads, outputs=outputs,
                                    tiles=[index for index in range(6) if generator.random() < 0.1]))

    def test_missing_root_picture_row_and_header_fields_are_value_errors(self):
        original = trace([2, 2], {(1, 0): {0: (0, 0)}})
        for path in ((), ('pictures', 0), ('pictures', 1, 'rows', 0), ('header_dependencies',)):
            value = original
            for key in path:
                value = value[key]
            for field in value:
                changed = copy.deepcopy(original)
                target = changed
                for key in path:
                    target = target[key]
                del target[field]
                with self.subTest(path=path, field=field), self.assertRaises(ValueError):
                    analyze(changed)

    def test_root_counts_modes_and_capture_worker_mutations_fail_closed(self):
        original = trace([2, 2])
        mutations = {'mode': [None, 'header_dependencies', True], 'complete': [False, 1],
                     'header_only': [True, 0], 'decoded_pictures': [True, -1, 1, 3, 2.0],
                     'output_pictures': [None, True, -1, 1, 3, 2.0],
                     'requested_threads': [True, 0, 2, 1.0], 'thread_limit': [True, 0, 2, 1.0],
                     'max_workers': [True, 0, 2, 1.0], 'pictures': [None, {}, [], original['pictures'][::-1]],
                     'header_dependencies': [None, {}, []]}
        for field, values in mutations.items():
            for value in values:
                changed = copy.deepcopy(original)
                changed[field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyze(changed)
        for value in (None, [], True, 1):
            with self.assertRaises(ValueError):
                analyze(value)
        for mutation in ('wpp', 'events'):
            changed = copy.deepcopy(original)
            if mutation == 'wpp':
                changed['header_dependencies']['pictures'][0]['wpp'] = True
            else:
                del changed['header_dependencies']['output_events']
            with self.assertRaises(ValueError):
                analyze(changed)

    def test_geometry_row_counts_flags_and_calls_reject_numeric_booleans(self):
        original = trace([2, 2], {(1, 0): {0: (0, 0)}})
        mutations = {'index': [True, 0, 2, 1.0], 'poc': [True, 0, -2.0],
                     'width': [True, 0, -1, 32.0], 'height': [True, 0, 16, 48, 32.0, 32769],
                     'ctb_log2': [True, 3, 7, 4.0], 'tiles': [0, 1, None], 'wpp': [True, 0, None],
                     'rows': [None, {}, [], original['pictures'][1]['rows'][::-1]]}
        for field, values in mutations.items():
            for value in values:
                changed = copy.deepcopy(original)
                changed['pictures'][1][field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyze(changed)
        for field, values in {'row': [True, 1, -1, 0.0], 'sample_calls': [True, -1, 0, 1.0],
                              'motion_calls': [True, -1, 0, 1.0]}.items():
            for value in values:
                changed = copy.deepcopy(original)
                changed['pictures'][1]['rows'][0][field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    analyze(changed)
        changed = trace([2])
        changed['pictures'][0]['rows'][0]['sample_calls'] = 1
        with self.assertRaises(ValueError):
            analyze(changed)

    def test_reference_shapes_identity_poc_footprints_and_counts_are_closed(self):
        original = trace([2, 2], {(1, 0): {0: (0, None)}})
        for refs in (None, {}, [None], [[0]], [[0, -4, 0]], [[0, -4, 0, None, 0]],
                     [[True, -4, 0, None]], [[0.0, -4, 0, None]], [[0, True, 0, None]],
                     [[0, -3, 0, None]], [[-1, -4, 0, None]], [[1, -2, 0, None]],
                     [[2, 0, 0, None]], [[0, -4, True, None]], [[0, -4, 0.0, None]],
                     [[0, -4, -1, None]], [[0, -4, 2, None]], [[0, -4, None, None]],
                     [[0, -4, 0, True]], [[0, -4, 0, -1]], [[0, -4, 0, 2]],
                     [[0, -4, 0, None]] * 2, [[0, -4, 0, None]] * 17):
            changed = copy.deepcopy(original)
            changed['pictures'][1]['rows'][0]['references'] = refs
            with self.subTest(refs=refs), self.assertRaises(ValueError):
                analyze(changed)
        changed = trace([2, None, 2], {(2, 0): {0: (0, None)}})
        changed['pictures'][1]['rows'][0]['references'][0][:2] = [1, -2]
        with self.assertRaises(ValueError):
            analyze(changed)
        changed = trace([2, 2, 2], {(2, 0): {1: (0, None)}})
        changed['pictures'][2]['rows'][0]['references'][0][:2] = [0, -4]
        with self.assertRaises(ValueError):
            analyze(changed)
        changed = trace([2, 2])
        changed['pictures'][1]['rows'][0].update(references=[[0, -4, 0, None]], sample_calls=1)
        with self.assertRaises(ValueError):
            analyze(changed)

    def test_2048_rows_sixteen_refs_and_partial_edge_ctu_geometry_are_supported(self):
        data = trace([2048])
        data['pictures'][0]['height'] -= 1
        self.assertEqual(analyze(data)['models']['picture_ready']['filter_lag1']['steps'], 2048)
        data = trace([1] * 17, {(16, 0): {index: (0, None) for index in range(16)}})
        self.check_oracle(data)
        data['pictures'][-1]['rows'][0]['sample_calls'] = 15
        with self.assertRaises(ValueError):
            analyze(data)


if __name__ == '__main__':
    unittest.main()
