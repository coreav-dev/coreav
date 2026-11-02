#!/usr/bin/env python3
"""Media-free synthetic validation of the proposed B4 calibration.

Requires NumPy 1.24.4. This is a design simulation, not a timing driver.
"""
import argparse
import hashlib
import json
import math
import pathlib
import random

import numpy as np

FILES = 8
PAIRS = 16
BLOCK = 4
DRAWS = 999
TRIALS = 2000
SEED = 35612
BIAS = (0.0, 0.005, 0.02, -0.02)
SHAPES = np.array([
    [[0.013642233, 0.043493740, 0.052207898, 0.180469383],
     [1.280762128, 0.038962499, 0.005169962, 0.037595518],
     [0.018379369, 0.005467414, 0.017331889, 0.031415220],
     [0.866131323, -0.000207766, 0.006178765, 0.004110636],
     [0.032396584, 0.003992420, 0.012805059, 0.057156646],
     [0.277473049, -0.000618696, 0.002167403, -0.000633238]],
    [[0.000988074, 0.000936160, 0.022887629, 0.037200406],
     [0.001123047, -0.004401245, 0.024938242, 0.007925532],
     [0.001163474, 0.002430449, 0.075383884, 0.017670083],
     [0.169484140, -0.001135097, 0.004742560, 0.003888765],
     [0.002604283, 0.001277486, 0.010124153, 0.013247994],
     [0.132614783, 0.000205086, 0.002910209, 0.008199477]],
]).reshape(2, 3, 2, 4)
SCENARIOS = {
    'nearly_noiseless_position': dict(rho=0.0, scale=0.0, file_bias=0.0, drift=0.0, outliers=False),
    'recorded_shape': dict(rho=0.0, scale=1.0, file_bias=0.0, drift=0.0, outliers=False),
    'serial_drift': dict(rho=0.8, scale=1.0, file_bias=0.002, drift=0.05, outliers=False),
    'double_noise': dict(rho=0.5, scale=2.0, file_bias=0.005, drift=0.10, outliers=False),
    'shared_shocks': dict(rho=0.8, scale=1.0, file_bias=0.005, drift=0.10, outliers=True),
}


def schedule(rng, files=FILES, pairs=PAIRS):
    first = rng.choice([-1, 1], size=(3, files, 2, pairs // 2))
    return np.stack((first, -first), axis=-1).reshape(3, files, 2, pairs)


def synthetic(rng, scenario):
    cfg = SCENARIOS[scenario]
    position = schedule(rng)[None, ...]
    shape = SHAPES[:, :, None, :, :]
    base, gap, noise, transient = [shape[..., i, None] for i in range(4)]
    if scenario == 'nearly_noiseless_position':
        gap = np.full_like(gap, math.log(1.04))
        transient = np.zeros_like(transient)
    size = (2, 3, FILES, 2, PAIRS, 2)
    innovations = rng.normal(size=size)
    serial = np.empty(size)
    serial[..., 0, :] = innovations[..., 0, :]
    for i in range(1, PAIRS):
        serial[..., i, :] = (cfg['rho'] * serial[..., i - 1, :] +
                             math.sqrt(1 - cfg['rho'] ** 2) * innovations[..., i, :])
    residual = serial * noise[..., None] * cfg['scale'] / math.sqrt(2)
    file_effect = rng.normal(0, cfg['file_bias'], size=(2, 3, FILES, 1, 1))
    shared = np.zeros((1, 1, 1, 1, PAIRS, 2))
    if cfg['rho']:
        shared[..., 0, :] = rng.normal(0, 0.01, size=(1, 1, 1, 1, 2))
        for i in range(1, PAIRS):
            shared[..., i, :] = (0.8 * shared[..., i - 1, :] +
                                 math.sqrt(1 - 0.8 ** 2) * rng.normal(0, 0.01, size=(1, 1, 1, 1, 2)))
    if cfg['outliers']:
        shared += (rng.random(shared.shape) < 0.02) * rng.exponential(0.1, shared.shape)
        residual += (rng.random(size) < 0.01) * rng.exponential(0.1, size)
        shared[..., 4:12, 0] += rng.normal(0, 0.02)
    call_index = np.arange(PAIRS * 2).reshape(PAIRS, 2)
    conditioning_index = call_index + 8 + 4 * (call_index // (BLOCK * 2))
    trend = (np.log1p(transient)[..., None] * np.exp(-conditioning_index / 4) -
             cfg['drift'] * call_index / (PAIRS * 2))
    log_times = (np.log(base)[..., None] + residual + shared + trend +
                 gap[..., None] * np.array([0.5, -0.5]))
    if scenario != 'nearly_noiseless_position':
        log_times = np.logaddexp(log_times, np.log(rng.exponential(0.000005, size=size)))
    chronological_difference = log_times[..., 0] - log_times[..., 1]
    return chronological_difference * position + file_effect


def resampling_indices(seed=0, draws=DRAWS, files=FILES, pairs=PAIRS):
    rng = random.Random(seed)
    file_index = np.empty((draws, 3, files), dtype=int)
    blocks = np.empty((draws, pairs // BLOCK), dtype=int)
    for draw in range(draws):
        for block in range(pairs // BLOCK):
            blocks[draw, block] = rng.randrange(pairs // BLOCK)
        for family in range(3):
            for slot in range(files):
                file_index[draw, family, slot] = rng.randrange(files)
    return file_index, blocks


def interval(log_ratios, indices):
    file_index, blocks = indices
    contrasts = log_ratios.reshape(2, 3, FILES, 2, PAIRS // 2, 2).mean(axis=-1)
    family = np.arange(3)[None, :, None, None, None, None]
    file_index = file_index[..., None, None, None]
    commands = np.arange(2)[None, None, None, :, None, None]
    rounds = (blocks[..., None] * (BLOCK // 2) + np.arange(BLOCK // 2))[:, None, None, None, :, :]
    sample = contrasts[:, family, file_index, commands, rounds]
    centers = np.median(sample.reshape(2, len(blocks), -1), axis=-1)
    centers.sort(axis=-1)
    return np.exp(centers[:, [2, 996]]), np.exp(centers[:, [24, 974]])


def wilson(successes, trials, z=2.3263478740408408):
    p = successes / trials
    den = 1 + z * z / trials
    mid = (p + z * z / (2 * trials)) / den
    radius = z * math.sqrt(p * (1 - p) / trials + z * z / (4 * trials * trials)) / den
    return [max(0, mid - radius), min(1, mid + radius)]


def simulate(trials=TRIALS, seed=SEED, bootstrap_seed=0):
    rng = np.random.Generator(np.random.PCG64(seed))
    indices = resampling_indices(bootstrap_seed)
    rows = []
    for name in SCENARIOS:
        counts = {bias: np.zeros(3, dtype=int) for bias in BIAS}
        widths = []
        for _ in range(trials):
            x = synthetic(rng, name)
            ci, _ = interval(x, indices)
            contrasts = x.reshape(2, 3, FILES, 2, PAIRS // 2, 2).mean(axis=-1)
            point = np.exp(np.median(contrasts.reshape(2, -1), axis=1))
            widths.append(ci[:, 1] - ci[:, 0])
            for bias in BIAS:
                factor = 1 + bias
                shifted_ci, shifted_point = ci * factor, point * factor
                rejected = ((shifted_ci[:, 0] > 1) | (shifted_ci[:, 1] < 1) |
                            (shifted_point < 0.98) | (shifted_point > 1.02))
                counts[bias] += [*rejected, np.any(rejected)]
        rows.append({
            'scenario': name,
            'median_ci_width': np.median(widths, axis=0).tolist(),
            'bias_results': [{
                'multiplicative_bias': bias,
                'rejections': dict(zip(('aa', 'null', 'either'), count.tolist())),
                'rates': dict(zip(('aa', 'null', 'either'), (count / trials).tolist())),
                'wilson99': {arm: wilson(int(n), trials)
                             for arm, n in zip(('aa', 'null', 'either'), count)},
                'either_wilson99': wilson(int(count[2]), trials),
            } for bias, count in counts.items()],
        })
    return {
        'schema': 'b4-calibration-design-simulation-v1',
        'numpy': np.__version__,
        'seed': seed, 'trials_per_scenario': trials,
        'bootstrap_seed': bootstrap_seed,
        'files_per_container': FILES, 'timed_pairs_per_case': PAIRS,
        'pairs_per_resampling_block': BLOCK, 'bootstrap_draws': DRAWS,
        'synchronized_global_blocks': True,
        'point_estimator': 'exp(median(mean(log(A/B) within each two-pair crossover)))',
        'gate_confidence': 0.995, 'diagnostic_confidence': 0.95,
        'scenarios': SCENARIOS, 'results': rows,
        'all_null_upper99_at_most_005': all(
            row['bias_results'][0]['either_wilson99'][1] <= 0.05 for row in rows),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--trials', type=int, default=TRIALS)
    parser.add_argument('--seed', type=int, default=SEED)
    parser.add_argument('--bootstrap-seed', type=int, default=0)
    args = parser.parse_args()
    if args.trials < 1:
        parser.error('--trials must be positive')
    result = simulate(args.trials, args.seed, args.bootstrap_seed)
    result['script_sha256'] = hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
    print(json.dumps({'null_validation_passes': result['all_null_upper99_at_most_005'],
                      'trials': args.trials}))
    if not result['all_null_upper99_at_most_005']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
