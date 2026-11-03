"""Frozen round-10 B4 schedules and calibration inference, without execution."""
import math
import random
import statistics

import speed

METHOD = 'b4-counterbalanced-v2'
FAMILIES = ('isobmff', 'matroska', 'mpegts')
ARMS = ('aa', 'null')
CALIBRATION_COMMANDS = [['-show_format', '-show_streams'], ['-show_packets']]


def calibration_schedule():
    rng = random.Random(10)
    orientations = {}
    for rank in range(8):
        for family in FAMILIES:
            for command in range(2):
                orders = []
                for _ in range(8):
                    orders.extend(('BA', 'AB') if rng.getrandbits(1) else ('AB', 'BA'))
                orientations[rank, family, command] = orders
    schedule = []
    for block in range(4):
        for rank in range(8):
            for family_index, family in enumerate(FAMILIES):
                reverse = (block + rank * 3 + family_index) % 2
                arms = ARMS[::-1] if reverse else ARMS
                commands = (1, 0) if reverse else (0, 1)
                for arm in arms:
                    for command in commands:
                        base = {'phase': 'calibration', 'arm': arm,
                                'file': f'calibration:{family}:{rank}', 'class': family,
                                'rank': rank, 'command': command, 'block': block}
                        for pair in range(4 if block == 0 else 2):
                            schedule.append(dict(base, pair=pair, timed=False,
                                                 order='AB' if pair % 2 == 0 else 'BA'))
                        for pair in range(block * 4, block * 4 + 4):
                            schedule.append(dict(base, pair=pair, timed=True,
                                                 order=orientations[rank, family, command][pair]))
    return schedule


def product_schedule(selection, commands):
    schedule = []
    for command, args in enumerate(commands):
        for entry in selection:
            if not speed.eligible(args, entry['class']):
                continue
            base = {'phase': 'product', 'arm': 'product', 'file': entry['file'],
                    'class': entry['class'], 'rank': entry['rank'],
                    'command': command, 'block': None}
            for pair in range(4):
                schedule.append(dict(base, pair=pair, timed=False,
                                     order='AB' if pair % 2 == 0 else 'BA'))
            for pair in range(5):
                schedule.append(dict(base, pair=pair, timed=True,
                                     order='AB' if (command + entry['rank'] + pair) % 2 == 0 else 'BA'))
    return schedule


def resampling_indices():
    rng = random.Random(0)
    for _ in range(999):
        blocks = tuple(rng.randrange(4) for _ in range(4))
        files = tuple(tuple(rng.randrange(8) for _ in range(8)) for _ in FAMILIES)
        yield blocks, files


def _list(value, length, label):
    if not isinstance(value, list) or len(value) != length:
        raise ValueError(f'{label} must contain exactly {length} entries')
    return value


def calibration_statistics(log_ratios):
    if not isinstance(log_ratios, dict) or set(log_ratios) != set(ARMS):
        raise ValueError('calibration requires exactly both arms')
    contrasts = {}
    for arm in ARMS:
        families = log_ratios[arm]
        if not isinstance(families, dict) or set(families) != set(FAMILIES):
            raise ValueError('each calibration arm requires exactly all three families')
        contrasts[arm] = []
        for family in FAMILIES:
            files = []
            for file in _list(families[family], 8, 'calibration family'):
                commands = []
                for command in _list(file, 2, 'calibration file'):
                    ratios = _list(command, 16, 'calibration command')
                    for ratio in ratios:
                        if (isinstance(ratio, bool) or not isinstance(ratio, (int, float))):
                            raise ValueError('calibration log ratios must be finite numbers')
                        try:
                            finite = math.isfinite(ratio)
                        except OverflowError:
                            finite = False
                        if not finite:
                            raise ValueError('calibration log ratios must be finite numbers')
                    commands.append([ratios[pair] / 2 + ratios[pair + 1] / 2
                                     for pair in range(0, 16, 2)])
                files.append(commands)
            contrasts[arm].append(files)
    centers = {arm: statistics.median([contrast for family in contrasts[arm]
                                      for file in family for command in file
                                      for contrast in command]) for arm in ARMS}
    bootstrap = {arm: [] for arm in ARMS}
    for blocks, files in resampling_indices():
        for arm in ARMS:
            sample = [contrasts[arm][family][file][command][block * 2 + offset]
                      for family in range(3) for file in files[family]
                      for command in range(2) for block in blocks for offset in range(2)]
            bootstrap[arm].append(statistics.median(sample))
    result = {}
    for arm in ARMS:
        ordered = sorted(bootstrap[arm])
        try:
            estimate = math.exp(centers[arm])
            diagnostic = [math.exp(ordered[index]) for index in (24, 974)]
            gate = [math.exp(ordered[index]) for index in (2, 996)]
        except OverflowError as error:
            raise ValueError('calibration estimates must be finite') from error
        result[arm] = {'estimate': estimate, 'ci95_diagnostic': diagnostic,
                       'ci995_gate': gate, 'point_in_band': 0.98 <= estimate <= 1.02,
                       'gate_contains_one': gate[0] <= 1.0 <= gate[1]}
    return result
