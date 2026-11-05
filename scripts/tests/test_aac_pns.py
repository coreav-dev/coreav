import copy
import importlib.util
import itertools
import json
import math
from pathlib import Path
import unittest

import numpy as np


SCRIPT = Path(__file__).resolve().parents[1] / 'aac_pns.py'
SPEC = importlib.util.spec_from_file_location('aac_pns', SCRIPT)
pns = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pns)


def bessel_series(value):
    total = term = 1.0
    for order in range(1, 100):
        term *= (value / (2 * order)) ** 2
        total += term
        if term < total * 1e-17:
            return total
    raise AssertionError('Bessel control failed to converge')


def half_window(size, shape):
    if shape == 0:
        return np.array([math.sin(math.pi * (2 * n + 1) / (4 * size))
                         for n in range(size)])
    alpha = 6 if size == 128 else 4
    weights = [bessel_series(math.pi * alpha * math.sqrt(1 - ((n - size / 2) / (size / 2)) ** 2))
               for n in range(size + 1)]
    return np.sqrt(np.cumsum(weights[:-1]) / math.fsum(weights))


def reference_window(sequence, previous, current, block=0):
    size = 128 if sequence == 2 else 1024
    left = half_window(size, current if block else previous)
    right = half_window(size, current)[::-1]
    if sequence == 1:
        right = np.r_[np.ones(448), half_window(128, current)[::-1], np.zeros(448)]
    elif sequence == 3:
        left = np.r_[np.zeros(448), half_window(128, previous), np.ones(448)]
    return np.r_[left, right]


def cosine_basis(size):
    n = np.arange(2 * size)[:, None]
    k = np.arange(size)[None, :]
    return np.cos((math.pi / size) * (n + 0.5 + size / 2) * (k + 0.5))


def reference_synthesis(spectra, frames):
    pcm = np.zeros(((len(frames) + 1) * 1024, spectra.shape[1]))
    previous = [0] * spectra.shape[1]
    for packet, frame in enumerate(frames):
        for channel, row in enumerate(frame['channels']):
            sequence, shape = row['sequence'], row['shape']
            size = 128 if sequence == 2 else 1024
            for block in range(8 if sequence == 2 else 1):
                values = spectra[packet, channel, block * size:(block + 1) * size]
                nonzero = np.flatnonzero(values)
                n = np.arange(2 * size)[:, None]
                basis = np.cos((math.pi / size) * (n + 0.5 + size / 2) * (nonzero + 0.5))
                data = basis @ values[nonzero]
                data *= reference_window(sequence, previous[channel], shape, block)
                offset = packet * 1024 + (448 + 128 * block if sequence == 2 else 0)
                pcm[offset:offset + 2 * size, channel] += data
            previous[channel] = shape
    return pcm


def frames_for(sequences, shapes=None, channels=1):
    if shapes is None:
        shapes = [0] * len(sequences)
    return [{'channels': [{'sequence': sequence, 'shape': shape, 'noise': []}
                           for _ in range(channels)]}
            for sequence, shape in zip(sequences, shapes)]


def metadata_for(frames, padding=1):
    return {'schema': 'coreav.aac-pns-diagnostic/1', 'rate': 48000,
            'channels': len(frames[0]['channels']), 'frame_length': 1024,
            'packets': len(frames), 'padding_packets': padding,
            'unsafe': [], 'frames': frames}


def noise_case(short=False, channels=1, padding=1, sf=80):
    frames = frames_for([1, 2, 2, 3, 0] if short else [0, 0],
                        [1, 0, 1, 1, 0] if short else [1, 0], channels)
    shape = (len(frames), channels, 1024)
    rng = np.random.default_rng(193)
    native = rng.normal(0, 0.0001, shape)
    oracle = native.copy()
    for packet, frame in enumerate(frames):
        for channel, row in enumerate(frame['channels']):
            size = 128 if row['sequence'] == 2 else 1024
            for block in range(8 if size == 128 else 1):
                start, end = block * size + 16, block * size + 32
                row['noise'].append({'start': start, 'end': end, 'sf': sf})
                norm = 2 ** (sf / 4) / (32768 * size)
                for values in (native, oracle):
                    band = rng.normal(size=end - start)
                    values[packet, channel, start:end] = band * (norm / np.linalg.norm(band))
    left = pns.synthesize(native, frames).astype(np.float32)
    right = pns.synthesize(oracle, frames).astype(np.float32)
    if padding > 1:
        left = np.pad(left, ((0, (padding - 1) * 1024), (0, 0)))
        right = np.pad(right, ((0, (padding - 1) * 1024), (0, 0)))
    return left, right, metadata_for(frames, padding), native.astype(np.float32)


class TransformControls(unittest.TestCase):
    def test_requirement_is_exactly_pinned_numpy(self):
        requirement = SCRIPT.with_name('aac-pns-requirements.txt').read_text().replace('\\\n', ' ')
        tokens = requirement.split()
        self.assertEqual(tokens[0], 'numpy==2.3.5')
        self.assertGreater(len(tokens), 1)
        for digest in tokens[1:]:
            self.assertRegex(digest, r'^--hash=sha256:[0-9a-f]{64}$')

    def test_sine_and_kbd_halves_match_independent_series(self):
        for size, shape in itertools.product((128, 1024), (0, 1)):
            with self.subTest(size=size, shape=shape):
                expected = half_window(size, shape)
                np.testing.assert_allclose(pns._half(size, shape), expected, atol=2e-15, rtol=0)
                np.testing.assert_allclose(expected ** 2 + expected[::-1] ** 2,
                                           1, atol=2e-15, rtol=0)

    def test_fft_analysis_matches_all_direct_cosine_bins(self):
        rng = np.random.default_rng(23)
        for sequences, shapes in (([0], [0]), ([0], [1]), ([1], [1]),
                                  ([1, 2], [1, 0]), ([1, 2, 3], [0, 1, 0])):
            frames = frames_for(sequences, shapes)
            pcm = rng.normal(size=((len(frames) + 1) * 1024, 1))
            actual = pns.analyse(pcm, frames)
            previous = 0
            for packet, row in enumerate(frame['channels'][0] for frame in frames):
                size = 128 if row['sequence'] == 2 else 1024
                basis = cosine_basis(size)
                for block in range(8 if size == 128 else 1):
                    with self.subTest(sequences=sequences, packet=packet, block=block):
                        offset = packet * 1024 + (448 + 128 * block if size == 128 else 0)
                        window = reference_window(row['sequence'], previous, row['shape'], block)
                        expected = (2 / size) * ((pcm[offset:offset + 2 * size, 0] * window) @ basis)
                        np.testing.assert_allclose(actual[packet, 0, block * size:(block + 1) * size],
                                                   expected, atol=1e-13, rtol=0)
                previous = row['shape']

    def test_fft_synthesis_matches_independent_cosines_and_boundary_windows(self):
        for sequences, shapes in (([0], [0]), ([0], [1]), ([1], [1]),
                                  ([1, 2, 2, 3, 0], [1, 0, 1, 0, 1])):
            frames = frames_for(sequences, shapes, 2)
            values = np.zeros((len(frames), 2, 1024))
            values[:, 0, [0, 1, 127, 128, 511, 1023]] = [0.7, -0.2, 0.1, -0.6, 0.4, 0.3]
            values[:, 1, [3, 255, 512, 767, 1000]] = [-0.1, 0.5, -0.9, 0.4, -0.3]
            with self.subTest(sequences=sequences):
                expected = reference_synthesis(values, frames)
                np.testing.assert_allclose(pns.synthesize(values, frames), expected, atol=1e-12, rtol=0)
                self.assertGreater(np.max(np.abs(expected[:1024])), 0)
                self.assertGreater(np.max(np.abs(expected[-1024:])), 0)

    def test_every_legal_transition_and_shape_change_has_finite_reconstruction(self):
        prefixes = {0: [0], 1: [1], 2: [1, 2], 3: [1, 2, 3]}
        transitions = ((0, 0), (0, 1), (1, 2), (2, 2), (2, 3), (3, 0), (3, 1))
        rng = np.random.default_rng(451)
        for (previous, current), old_shape, shape in itertools.product(transitions, (0, 1), (0, 1)):
            sequences = prefixes[previous] + [current]
            shapes = [0] * len(sequences)
            shapes[-2:] = [old_shape, shape]
            frames = frames_for(sequences, shapes, 2)
            values = rng.normal(size=(len(frames), 2, 1024))
            with self.subTest(transition=(previous, current), shapes=(old_shape, shape)):
                pcm = pns.synthesize(values, frames)
                np.testing.assert_allclose(pns.analyse(pcm, frames), values, atol=2e-12, rtol=0)
                sparse = values.copy()
                sparse[:, :, 4:1020] = 0
                independent = reference_synthesis(sparse, frames)
                np.testing.assert_allclose(pns.analyse(independent, frames), sparse, atol=2e-12, rtol=0)

    def test_initial_sine_state_and_last_short_tail_are_not_discarded(self):
        frames = frames_for([1, 2], [1, 0])
        values = np.zeros((2, 1, 1024))
        values[0, 0, 0] = 0.5
        values[-1, 0, -1] = 0.8
        pcm = reference_synthesis(values, frames)
        self.assertEqual(pcm.shape, (3 * 1024, 1))
        np.testing.assert_allclose(pns.analyse(pcm, frames), values, atol=1e-12, rtol=0)
        damaged = pcm.copy()
        damaged[-1024:] = 0
        self.assertGreater(np.max(np.abs(pns.analyse(damaged, frames) - values)), 0.1)

    def test_short_l1_bounds_use_current_shape_after_first_window(self):
        frames = frames_for([1, 2], [0, 1])
        pcm = np.zeros((3 * 1024, 1))
        coefficients = np.zeros((2, 1, 1024), dtype=np.float32)
        result = pns.compare(pcm, pcm, metadata_for(frames), coefficients)
        self.assertEqual(result['short_windows'], 8)
        for previous, current in itertools.product((0, 1), repeat=2):
            for sequence in range(4):
                size = 128 if sequence == 2 else 1024
                window = reference_window(sequence, previous, current)
                expected = np.sum(np.abs((2 / size) * window[:, None] * cosine_basis(size)), axis=0)
                np.testing.assert_allclose(pns._kernel_l1(sequence, previous, current), expected,
                                           atol=1e-14, rtol=0)

    def test_transform_does_not_mutate_input(self):
        frames = frames_for([1, 2, 3], [1, 0, 1])
        values = np.random.default_rng(6).normal(size=(3, 1, 1024))
        saved = values.copy()
        saved_frames = copy.deepcopy(frames)
        pcm = pns.synthesize(values, frames)
        saved_pcm = pcm.copy()
        pns.analyse(pcm, frames)
        np.testing.assert_array_equal(values, saved)
        np.testing.assert_array_equal(pcm, saved_pcm)
        self.assertEqual(frames, saved_frames)


class ComparisonControls(unittest.TestCase):
    def test_different_noise_passes_for_long_and_each_short_window(self):
        for short, channels in itertools.product((False, True), (1, 2)):
            with self.subTest(short=short, channels=channels):
                native, oracle, metadata, diagnostic = noise_case(short, channels)
                self.assertGreater(float(np.max(np.abs(native - oracle))), 2 ** -16)
                result = pns.compare(native, oracle, metadata, diagnostic)
                self.assertEqual(result['pns_bands'], (19 if short else 2) * channels)
                self.assertEqual(result['pns_bins'], result['pns_bands'] * 16)
                self.assertLess(result['max_native_energy_relative_error'], 1 / 1024)
                self.assertLess(result['max_oracle_energy_relative_error'], 1 / 1024)
                self.assertLess(result['max_projection_f32_error'], 2 ** -16)
                self.assertLessEqual(result['max_projection_s16_error'], 1)
                self.assertEqual(json.loads(json.dumps(result, allow_nan=False)), result)

    def test_additional_zero_closing_packets_are_counted_but_not_analysed(self):
        native, oracle, metadata, diagnostic = noise_case(True, 2, padding=3)
        result = pns.compare(native, oracle, metadata, diagnostic)
        self.assertEqual(result['packets'], 5)
        self.assertEqual(result['padding_packets'], 3)
        self.assertEqual(result['samples'], 8192)
        self.assertEqual(pns.analyse(native, metadata['frames']).shape, (5, 2, 1024))

    def test_changed_deterministic_coefficient_is_rejected(self):
        native, _, metadata, diagnostic = noise_case()
        values = diagnostic.astype(np.float64)
        values[0, 0, 500] += 0.01
        oracle = pns.synthesize(values, metadata['frames'])
        with self.assertRaisesRegex(ValueError, 'deterministic coefficient'):
            pns.compare(native, oracle, metadata, diagnostic)

    def test_projection_rejects_coherent_errors_that_pass_every_bin_bound(self):
        frames = frames_for([0])
        window = reference_window(0, 0, 0)
        basis = cosine_basis(1024)
        bounds = (2 ** -16) * np.sum(np.abs((2 / 1024) * window[:, None] * basis), axis=0)
        delta = 0.25 * bounds * np.sign(basis[700])
        oracle = reference_synthesis(delta.reshape(1, 1, 1024), frames)
        native = np.zeros_like(oracle)
        actual = pns.analyse(oracle, frames)[0, 0]
        self.assertTrue(np.all(np.abs(actual) < bounds))
        self.assertGreater(np.max(np.abs(oracle)), 2 ** -16)
        with self.assertRaisesRegex(ValueError, 'projection F32'):
            pns.compare(native, oracle, metadata_for(frames), np.zeros((1, 1, 1024), np.float32))

    def test_projection_quantization_is_independent_rounded_and_clipped(self):
        values = np.array([-2, -1, -2.5 / 32768, -1.5 / 32768, -0.5 / 32768,
                           0.5 / 32768, 1.5 / 32768, 2.5 / 32768, 32767 / 32768, 1, 2])
        np.testing.assert_array_equal(pns.quantize_s16(values), [-32768, -32768, -2, -2, 0,
                                                       0, 2, 2, 32767, 32767, 32767])

    def test_energy_change_beyond_fixed_db_bar_is_rejected_for_both_outputs(self):
        for index, change in itertools.product((0, 1), (-0.11, 0.11)):
            native, oracle, metadata, diagnostic = noise_case()
            values = pns.analyse((native, oracle)[index], metadata['frames'])
            values[0, 0, 16:32] *= math.sqrt(10 ** (change / 10))
            changed = pns.synthesize(values, metadata['frames'])
            if index == 0:
                native, diagnostic = changed, values.astype(np.float32)
            else:
                oracle = changed
            with self.subTest(index=index, change=change), self.assertRaisesRegex(ValueError, 'noise energy'):
                pns.compare(native, oracle, metadata, diagnostic)

    def test_energy_change_inside_fixed_db_bar_is_accepted(self):
        for change in (-0.09, 0.09):
            native, oracle, metadata, diagnostic = noise_case()
            values = pns.analyse(oracle, metadata['frames'])
            values[0, 0, 16:32] *= math.sqrt(10 ** (change / 10))
            oracle = pns.synthesize(values, metadata['frames'])
            result = pns.compare(native, oracle, metadata, diagnostic)
            self.assertAlmostEqual(result['max_oracle_energy_db_error'], 0.09, places=6)

    def test_projection_bar_stays_at_two_to_the_minus_sixteen(self):
        frames = frames_for([0])
        pcm = np.zeros((2048, 1))
        diagnostic = np.zeros((1, 1, 1024), np.float32)
        values = np.zeros_like(diagnostic, dtype=np.float64)
        values[0, 0, 0] = 1
        basis_pcm = reference_synthesis(values, frames)
        for ratio in (0.99, 1.01):
            oracle = basis_pcm * ((2 ** -16) * ratio / np.max(np.abs(basis_pcm)))
            with self.subTest(ratio=ratio):
                if ratio < 1:
                    result = pns.compare(pcm, oracle, metadata_for(frames), diagnostic)
                    self.assertGreater(result['max_projection_f32_error'], 0.98 * (2 ** -16))
                else:
                    with self.assertRaises(ValueError):
                        pns.compare(pcm, oracle, metadata_for(frames), diagnostic)

    def test_short_window_energy_cannot_be_pooled_across_group(self):
        native, oracle, metadata, diagnostic = noise_case(True)
        values = pns.analyse(oracle, metadata['frames'])
        values[1, 0, 16:32] *= math.sqrt(1.1)
        values[1, 0, 144:160] *= math.sqrt(0.9)
        oracle = pns.synthesize(values, metadata['frames'])
        with self.assertRaisesRegex(ValueError, 'noise energy'):
            pns.compare(native, oracle, metadata, diagnostic)

    def test_quiet_missing_noise_has_no_absolute_energy_exception(self):
        frames = frames_for([0])
        frames[0]['channels'][0]['noise'] = [{'start': 20, 'end': 24, 'sf': -400}]
        pcm = np.zeros((2048, 1))
        with self.assertRaisesRegex(ValueError, 'noise energy'):
            pns.compare(pcm, pcm, metadata_for(frames), np.zeros((1, 1, 1024), np.float32))

    def test_native_diagnostic_must_match_all_bins_including_noise(self):
        for bin_index in (16, 800):
            native, oracle, metadata, diagnostic = noise_case()
            diagnostic[0, 0, bin_index] += 0.01
            with self.subTest(bin=bin_index), self.assertRaisesRegex(ValueError, 'diagnostic coefficient'):
                pns.compare(native, oracle, metadata, diagnostic)

    def test_wrong_noise_mask_is_not_a_whole_waveform_tolerance(self):
        native, oracle, metadata, diagnostic = noise_case()
        metadata['frames'][0]['channels'][0]['noise'][0].update(start=32, end=48)
        with self.assertRaises(ValueError):
            pns.compare(native, oracle, metadata, diagnostic)

    def test_count_drift_and_missing_final_overlap_fail_for_either_output(self):
        for index, damage in itertools.product((0, 1), ('short', 'long', 'tail')):
            native, oracle, metadata, diagnostic = noise_case(True)
            arrays = [native, oracle]
            if damage == 'short':
                arrays[index] = arrays[index][:-1]
            elif damage == 'long':
                arrays[index] = np.pad(arrays[index], ((0, 1), (0, 0)))
            else:
                arrays[index][-1024:] = 0
            with self.subTest(index=index, damage=damage), self.assertRaises(ValueError):
                pns.compare(*arrays, metadata, diagnostic)

    def test_unobserved_closing_padding_is_checked_completely(self):
        for index in (0, 1):
            native, oracle, metadata, diagnostic = noise_case(padding=3)
            arrays = [native, oracle]
            arrays[index][-1] = 0.01
            with self.subTest(index=index), self.assertRaisesRegex(ValueError, 'overlap reconstruction'):
                pns.compare(*arrays, metadata, diagnostic)

    def test_initial_out_of_basis_pcm_is_rejected(self):
        frames = frames_for([0])
        pcm = np.zeros((2048, 1))
        vector = np.random.default_rng(7).normal(0, 0.01, (2048, 1))
        residual = vector - pns.synthesize(pns.analyse(vector, frames), frames)
        with self.assertRaisesRegex(ValueError, 'overlap reconstruction'):
            pns.compare(pcm, residual, metadata_for(frames), np.zeros((1, 1, 1024), np.float32))

    def test_stereo_channel_swaps_and_uncoded_zero_bins_fail(self):
        frames = frames_for([0], channels=2)
        diagnostic = np.zeros((1, 2, 1024), np.float32)
        diagnostic[0, 0, 3] = 0.1
        diagnostic[0, 1, 7] = -0.2
        native = pns.synthesize(diagnostic, frames)
        for oracle in (native[:, ::-1], native + pns.synthesize(
                np.full_like(diagnostic, 0.001), frames)):
            with self.assertRaisesRegex(ValueError, 'deterministic coefficient'):
                pns.compare(native, oracle, metadata_for(frames), diagnostic)

    def test_complete_comparison_does_not_mutate_any_input(self):
        native, oracle, metadata, diagnostic = noise_case(True, 2)
        saved = [value.copy() for value in (native, oracle, diagnostic)]
        saved_metadata = copy.deepcopy(metadata)
        pns.compare(native, oracle, metadata, diagnostic)
        for before, after in zip(saved, (native, oracle, diagnostic)):
            np.testing.assert_array_equal(before, after)
        self.assertEqual(metadata, saved_metadata)


class ValidationControls(unittest.TestCase):
    def test_missing_root_and_channel_fields_are_refused(self):
        native, oracle, metadata, diagnostic = noise_case()
        for field in metadata:
            changed = copy.deepcopy(metadata)
            del changed[field]
            with self.subTest(field=field), self.assertRaises(ValueError):
                pns.compare(native, oracle, changed, diagnostic)
        for field in ('sequence', 'shape', 'noise'):
            changed = copy.deepcopy(metadata)
            del changed['frames'][0]['channels'][0][field]
            with self.subTest(field=field), self.assertRaises(ValueError):
                pns.compare(native, oracle, changed, diagnostic)

    def test_inconsistent_or_unsupported_metadata_is_refused(self):
        native, oracle, metadata, diagnostic = noise_case()
        mutations = {'schema': 'other', 'rate': 0, 'channels': 3, 'frame_length': 960,
                     'packets': 3, 'padding_packets': 0, 'unsafe': None, 'frames': []}
        for field, value in mutations.items():
            changed = copy.deepcopy(metadata)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                pns.compare(native, oracle, changed, diagnostic)
        changed = copy.deepcopy(metadata)
        changed['frames'][1]['channels'].append(copy.deepcopy(changed['frames'][1]['channels'][0]))
        with self.assertRaises(ValueError):
            pns.compare(native, oracle, changed, diagnostic)

    def test_every_unsafe_class_and_unknown_reason_is_rejected(self):
        native, oracle, metadata, diagnostic = noise_case()
        for reason in ('tns_noise_intersection', 'ms_correlated_noise', 'intensity_from_noise',
                       'coupling', 'all_ones_ms_intensity', 'damaged_frame', 'extension_data',
                       'unrecognized_reason'):
            changed = copy.deepcopy(metadata)
            changed['unsafe'] = [reason]
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, 'unsafe'):
                pns.compare(native, oracle, changed, diagnostic)

    def test_all_illegal_window_transitions_are_rejected(self):
        prefixes = {0: [], 1: [1], 2: [1, 2], 3: [1, 2, 3]}
        allowed = {(0, 0), (0, 1), (1, 2), (2, 2), (2, 3), (3, 0), (3, 1)}
        for previous, current in itertools.product(range(4), repeat=2):
            if (previous, current) in allowed:
                continue
            frames = frames_for(prefixes[previous] + [current])
            with self.subTest(transition=(previous, current)), self.assertRaisesRegex(ValueError, 'history'):
                pns.synthesize(np.zeros((len(frames), 1, 1024)), frames)

    def test_shape_sequence_and_integer_metadata_are_not_coerced(self):
        native, oracle, metadata, diagnostic = noise_case()
        for field, value in (('shape', 2), ('shape', True), ('sequence', 4), ('sequence', 0.0)):
            changed = copy.deepcopy(metadata)
            changed['frames'][0]['channels'][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                pns.compare(native, oracle, changed, diagnostic)
        for field in ('rate', 'channels', 'frame_length', 'packets', 'padding_packets'):
            changed = copy.deepcopy(metadata)
            changed[field] = True
            with self.subTest(field=field), self.assertRaises(ValueError):
                pns.compare(native, oracle, changed, diagnostic)

    def test_malformed_noise_ranges_scalefactors_and_overlaps_fail(self):
        native, oracle, metadata, diagnostic = noise_case(True)
        invalid = ([{'start': -1, 'end': 4, 'sf': 80}],
                   [{'start': 3, 'end': 3, 'sf': 80}],
                   [{'start': 3, 'end': 1025, 'sf': 80}],
                   [{'start': 127, 'end': 129, 'sf': 80}],
                   [{'start': 3, 'end': 4}],
                   [{'start': 3, 'end': 4, 'sf': 80.0}],
                   [{'start': 3, 'end': 4, 'sf': 32767}],
                   [{'start': 3, 'end': 4, 'sf': -32768}],
                   [{'start': 3, 'end': 6, 'sf': 80}, {'start': 5, 'end': 7, 'sf': 80}])
        for bands in invalid:
            changed = copy.deepcopy(metadata)
            changed['frames'][1]['channels'][0]['noise'] = bands
            with self.subTest(bands=bands), self.assertRaises(ValueError):
                pns.compare(native, oracle, changed, diagnostic)

    def test_nonfinite_pcm_and_spectra_never_produce_metrics(self):
        for target, bad in itertools.product(range(3), (float('nan'), float('inf'), -float('inf'))):
            native, oracle, metadata, diagnostic = noise_case()
            arrays = [native, oracle, diagnostic]
            arrays[target].flat[-1] = bad
            with self.subTest(target=target, bad=bad), self.assertRaises(ValueError):
                pns.compare(arrays[0], arrays[1], metadata, arrays[2])

    def test_diagnostic_count_dtype_and_channel_shape_are_exact(self):
        native, oracle, metadata, diagnostic = noise_case()
        for bad in (diagnostic[:-1], diagnostic.astype(np.float64), diagnostic[:, 0],
                    np.zeros((2, 2, 1024), np.float32)):
            with self.subTest(shape=bad.shape, dtype=bad.dtype), self.assertRaises(ValueError):
                pns.compare(native, oracle, metadata, bad)

    def test_public_transform_inputs_require_finite_complete_arrays(self):
        frames = frames_for([0])
        for bad in (None, [], np.zeros((1024, 1)), np.zeros((2049, 1)),
                    np.zeros((2048, 2)), np.zeros(2048),
                    np.full((2048, 1), float('nan')), np.zeros((2048, 1), complex)):
            with self.subTest(shape=np.shape(bad)), self.assertRaises(ValueError):
                pns.analyse(bad, frames)
        for bad in (np.zeros((1, 1024)), np.zeros((1, 1, 128)),
                    np.full((1, 1, 1024), float('inf'))):
            with self.subTest(shape=bad.shape), self.assertRaises(ValueError):
                pns.synthesize(bad, frames)

    def test_finite_inputs_that_overflow_the_transform_are_refused(self):
        frames = frames_for([0])
        with np.errstate(over='ignore', invalid='ignore'):
            with self.assertRaisesRegex(ValueError, 'nonfinite analysis'):
                pns.analyse(np.full((2048, 1), 1e308), frames)
            with self.assertRaisesRegex(ValueError, 'nonfinite synthesis'):
                pns.synthesize(np.full((1, 1, 1024), 1e308), frames)


if __name__ == '__main__':
    unittest.main()
