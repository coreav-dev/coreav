"""Independent finite AAC-LC transform checks, not ISO conformance checks."""

from functools import lru_cache
import math

import numpy as np


F32_EPS = 2 ** -16
ENERGY_DB = 0.1
ENERGY_LOW = 10 ** (-ENERGY_DB / 10)
ENERGY_HIGH = 10 ** (ENERGY_DB / 10)
FRAME_LENGTH = 1024
MAX_PACKETS = 16384
SCHEMA = 'coreav.aac-pns-diagnostic/1'
TRANSITIONS = ({0, 1}, {2}, {2, 3}, {0, 1})


def _integer(value, minimum, maximum):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError('invalid metadata integer')
    return value


def _frames(frames):
    if not isinstance(frames, list) or not 1 <= len(frames) <= MAX_PACKETS:
        raise ValueError('invalid stored frame count')
    channels = None
    previous = None
    result = []
    for frame in frames:
        if not isinstance(frame, dict) or not isinstance(frame.get('channels'), list):
            raise ValueError('missing channel metadata')
        rows = frame['channels']
        if channels is None:
            channels = _integer(len(rows), 1, 2)
            previous = [(0, 0)] * channels
        if len(rows) != channels:
            raise ValueError('inconsistent channel count')
        parsed = []
        for channel, row in enumerate(rows):
            if not isinstance(row, dict):
                raise ValueError('invalid channel metadata')
            sequence = _integer(row.get('sequence'), 0, 3)
            shape = _integer(row.get('shape'), 0, 1)
            old_sequence, old_shape = previous[channel]
            if sequence not in TRANSITIONS[old_sequence]:
                raise ValueError('illegal window history')
            noise = row.get('noise')
            if not isinstance(noise, list) or len(noise) > FRAME_LENGTH:
                raise ValueError('missing or excessive noise metadata')
            mask = np.zeros(FRAME_LENGTH, dtype=bool)
            bands = []
            for band in noise:
                if not isinstance(band, dict):
                    raise ValueError('invalid noise band')
                start = _integer(band.get('start'), 0, FRAME_LENGTH - 1)
                end = _integer(band.get('end'), start + 1, FRAME_LENGTH)
                sf = _integer(band.get('sf'), -32768, 32767)
                if sequence == 2 and start // 128 != (end - 1) // 128:
                    raise ValueError('noise band crosses short window')
                if mask[start:end].any():
                    raise ValueError('overlapping noise bands')
                mask[start:end] = True
                size = 128 if sequence == 2 else FRAME_LENGTH
                try:
                    energy = (2.0 ** (sf / 4) / (32768 * size)) ** 2
                except OverflowError:
                    raise ValueError('ill-conditioned noise energy') from None
                if not math.isfinite(energy) or energy <= 0:
                    raise ValueError('ill-conditioned noise energy')
                bands.append((start, end, energy))
            parsed.append((sequence, old_shape, shape, mask, bands))
            previous[channel] = (sequence, shape)
        result.append(parsed)
    return result, channels


def _array(value, shape, label):
    try:
        original = np.asarray(value)
        if original.dtype.kind not in 'fiu':
            raise ValueError('invalid numeric array')
        array = np.asarray(original, dtype=np.float64)
    except (TypeError, ValueError, OverflowError):
        raise ValueError('invalid ' + label) from None
    if array.shape != shape or not np.isfinite(array).all():
        raise ValueError('invalid ' + label + ' shape or nonfinite value')
    return array


@lru_cache(maxsize=4)
def _half(size, shape):
    if shape == 0:
        values = np.sin(np.pi * (np.arange(size) + 0.5) / (2 * size))
    else:
        alpha = 6 if size == 128 else 4
        positions = 2 * np.arange(size + 1) / size - 1
        kernel = np.i0(np.pi * alpha * np.sqrt(np.maximum(0, 1 - positions ** 2)))
        values = np.sqrt(np.cumsum(kernel[:-1]) / np.sum(kernel))
    values.setflags(write=False)
    return values


@lru_cache(maxsize=16)
def _window(sequence, old_shape, shape):
    if sequence == 2:
        values = np.concatenate((_half(128, old_shape), _half(128, shape)[::-1]))
    elif sequence == 0:
        values = np.concatenate((_half(1024, old_shape), _half(1024, shape)[::-1]))
    elif sequence == 1:
        values = np.concatenate((_half(1024, old_shape), np.ones(448),
                                 _half(128, shape)[::-1], np.zeros(448)))
    else:
        values = np.concatenate((np.zeros(448), _half(128, old_shape),
                                 np.ones(448), _half(1024, shape)[::-1]))
    values.setflags(write=False)
    return values


@lru_cache(maxsize=2)
def _phases(size):
    samples = np.exp(1j * np.pi * np.arange(2 * size) / (2 * size))
    bins = np.exp(1j * np.pi / size * ((size + 1) / 2) * (np.arange(size) + 0.5))
    samples.setflags(write=False)
    bins.setflags(write=False)
    return samples, bins


def _analyse(pcm, rows, channels):
    spectra = np.empty((len(rows), channels, FRAME_LENGTH), dtype=np.float64)
    for packet, frame in enumerate(rows):
        for channel, (sequence, old_shape, shape, _, _) in enumerate(frame):
            size = 128 if sequence == 2 else FRAME_LENGTH
            samples, bins = _phases(size)
            for block in range(8 if sequence == 2 else 1):
                window = _window(sequence, old_shape if block == 0 else shape, shape)
                offset = packet * FRAME_LENGTH + (448 + block * 128 if sequence == 2 else 0)
                data = pcm[offset:offset + 2 * size, channel] * window
                coefficients = (np.fft.fft(data * samples.conj())[:size] * bins.conj()).real
                spectra[packet, channel, block * size:(block + 1) * size] = coefficients * (2 / size)
    return spectra


def analyse(pcm, frames):
    """Analyse stored frames using complete following PCM, including closing padding."""
    rows, channels = _frames(frames)
    try:
        samples = np.asarray(pcm).shape[0]
    except (TypeError, ValueError, IndexError):
        raise ValueError('invalid PCM') from None
    if not (len(rows) + 1) * FRAME_LENGTH <= samples <= 2 * MAX_PACKETS * FRAME_LENGTH:
        raise ValueError('missing or excessive following PCM')
    if samples % FRAME_LENGTH:
        raise ValueError('incomplete PCM packet')
    result = _analyse(_array(pcm, (samples, channels), 'PCM'), rows, channels)
    if not np.isfinite(result).all():
        raise ValueError('nonfinite analysis')
    return result


def _synthesize(spectra, rows, channels):
    pcm = np.zeros(((len(rows) + 1) * FRAME_LENGTH, channels), dtype=np.float64)
    for packet, frame in enumerate(rows):
        for channel, (sequence, old_shape, shape, _, _) in enumerate(frame):
            size = 128 if sequence == 2 else FRAME_LENGTH
            samples, bins = _phases(size)
            for block in range(8 if sequence == 2 else 1):
                window = _window(sequence, old_shape if block == 0 else shape, shape)
                coefficients = spectra[packet, channel, block * size:(block + 1) * size]
                data = np.fft.ifft(np.concatenate((coefficients * bins, np.zeros(size))))
                signal = (data * (2 * size) * samples).real * window
                offset = packet * FRAME_LENGTH + (448 + block * 128 if sequence == 2 else 0)
                pcm[offset:offset + 2 * size, channel] += signal
    return pcm


def synthesize(spectra, frames):
    """Synthesize stored frames from zero initial state, retaining their complete tail."""
    rows, channels = _frames(frames)
    values = _array(spectra, (len(rows), channels, FRAME_LENGTH), 'spectra')
    result = _synthesize(values, rows, channels)
    if not np.isfinite(result).all():
        raise ValueError('nonfinite synthesis')
    return result


@lru_cache(maxsize=16)
def _kernel_l1(sequence, old_shape, shape):
    size = 128 if sequence == 2 else FRAME_LENGTH
    window = _window(sequence, old_shape, shape)
    n = np.arange(2 * size)[:, None]
    k = np.arange(size)[None, :]
    basis = np.cos(np.pi / size * (n + (size + 1) / 2) * (k + 0.5))
    norms = np.sum(np.abs((2 / size) * window[:, None] * basis), axis=0)
    norms.setflags(write=False)
    return norms


def quantize_s16(pcm):
    """Round and clip normalized samples independently of either decoder's serializer."""
    return np.rint(np.clip(pcm, -1, 32767 / 32768) * 32768).astype(np.int64)


def compare(native_pcm, oracle_pcm, metadata, native_spectra):
    """Return bounded metrics or refuse; packing and presentation checks live outside this module."""
    if not isinstance(metadata, dict) or metadata.get('schema') != SCHEMA:
        raise ValueError('invalid diagnostic schema')
    rate = _integer(metadata.get('rate'), 1, 192000)
    channels = _integer(metadata.get('channels'), 1, 2)
    _integer(metadata.get('frame_length'), FRAME_LENGTH, FRAME_LENGTH)
    packets = _integer(metadata.get('packets'), 1, MAX_PACKETS)
    padding = _integer(metadata.get('padding_packets'), 1, MAX_PACKETS)
    if not isinstance(metadata.get('unsafe'), list) or metadata['unsafe']:
        raise ValueError('unsafe or missing diagnostic predicates')
    rows, actual_channels = _frames(metadata.get('frames'))
    if len(rows) != packets or actual_channels != channels:
        raise ValueError('inconsistent diagnostic counts')
    samples = (packets + padding) * FRAME_LENGTH
    native = _array(native_pcm, (samples, channels), 'native PCM')
    oracle = _array(oracle_pcm, (samples, channels), 'oracle PCM')
    diagnostic = _array(native_spectra, (packets, channels, FRAME_LENGTH), 'diagnostic spectra')
    if np.asarray(native_spectra).dtype != np.dtype('float32'):
        raise ValueError('diagnostic spectra must be float32')
    native_coefficients = _analyse(native, rows, channels)
    oracle_coefficients = _analyse(oracle, rows, channels)
    if not np.isfinite(native_coefficients).all() or not np.isfinite(oracle_coefficients).all():
        raise ValueError('nonfinite analysis')
    maximum_bin = 0.0
    maximum_diagnostic = 0.0
    maximum_energy = [0.0, 0.0]
    maximum_energy_db = [0.0, 0.0]
    band_count = 0
    noise_bins = 0
    short_windows = 0
    for packet, frame in enumerate(rows):
        for channel, (sequence, old_shape, shape, mask, bands) in enumerate(frame):
            bounds = F32_EPS * _kernel_l1(sequence, old_shape, shape)
            if sequence == 2:
                bounds = np.concatenate((bounds, np.tile(
                    F32_EPS * _kernel_l1(sequence, shape, shape), 7)))
                short_windows += 8
            left = native_coefficients[packet, channel]
            right = oracle_coefficients[packet, channel]
            difference = np.abs(left - right)
            diagnostic_error = np.abs(left - diagnostic[packet, channel])
            maximum_bin = max(maximum_bin, float(np.max(difference[~mask], initial=0)))
            maximum_diagnostic = max(maximum_diagnostic, float(np.max(diagnostic_error)))
            if np.any(difference[~mask] > bounds[~mask]):
                raise ValueError('deterministic coefficient bound exceeded')
            if np.any(diagnostic_error > bounds):
                raise ValueError('native diagnostic coefficient bound exceeded')
            for start, end, energy in bands:
                for index, coefficients in enumerate((left, right)):
                    with np.errstate(over='ignore', invalid='ignore', divide='ignore'):
                        ratio = float(np.sum(coefficients[start:end] ** 2) / energy)
                    if not math.isfinite(ratio) or not ENERGY_LOW <= ratio <= ENERGY_HIGH:
                        raise ValueError('per-window noise energy bound exceeded')
                    relative = abs(ratio - 1)
                    maximum_energy[index] = max(maximum_energy[index], relative)
                    maximum_energy_db[index] = max(maximum_energy_db[index], abs(10 * math.log10(ratio)))
            band_count += len(bands)
            noise_bins += int(np.count_nonzero(mask))
    maximum_reconstruction = 0.0
    for pcm, coefficients in ((native, native_coefficients), (oracle, oracle_coefficients)):
        reconstructed = _synthesize(coefficients, rows, channels)
        error = float(np.max(np.abs(pcm[:len(reconstructed)] - reconstructed)))
        if len(pcm) > len(reconstructed):
            error = max(error, float(np.max(np.abs(pcm[len(reconstructed):]))))
        if not math.isfinite(error) or error > F32_EPS:
            raise ValueError('complete PCM overlap reconstruction bound exceeded')
        maximum_reconstruction = max(maximum_reconstruction, error)
    for packet, frame in enumerate(rows):
        for channel, (_, _, _, mask, _) in enumerate(frame):
            native_coefficients[packet, channel, mask] = 0
            oracle_coefficients[packet, channel, mask] = 0
    native_projection = _synthesize(native_coefficients, rows, channels)
    oracle_projection = _synthesize(oracle_coefficients, rows, channels)
    projection_error = float(np.max(np.abs(native_projection - oracle_projection)))
    if not math.isfinite(projection_error) or projection_error > F32_EPS:
        raise ValueError('non-noise projection F32 bound exceeded')
    s16_error = int(np.max(np.abs(quantize_s16(native_projection) - quantize_s16(oracle_projection))))
    if s16_error > 1:
        raise ValueError('non-noise projection S16 bound exceeded')
    return {
        'rate': rate, 'channels': channels, 'packets': packets, 'padding_packets': padding,
        'samples': samples, 'pns_bands': band_count, 'pns_bins': noise_bins,
        'short_windows': short_windows, 'long_windows': packets * channels - short_windows // 8,
        'max_non_pns_coefficient_error': maximum_bin,
        'max_native_diagnostic_error': maximum_diagnostic,
        'max_native_energy_relative_error': maximum_energy[0],
        'max_oracle_energy_relative_error': maximum_energy[1],
        'max_native_energy_db_error': maximum_energy_db[0],
        'max_oracle_energy_db_error': maximum_energy_db[1],
        'max_reconstruction_error': maximum_reconstruction,
        'max_projection_f32_error': projection_error, 'max_projection_s16_error': s16_error,
    }
