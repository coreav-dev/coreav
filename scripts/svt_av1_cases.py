"""Closed synthetic worker cases and their bounded raw-picture input wire."""

from dataclasses import dataclass
import json
from fractions import Fraction
from pathlib import Path
import re
import struct


ROOT = Path(__file__).resolve().parents[1]
ROSTER = ROOT / 'crates/coreav-backend-svt-av1/tests/native-cases.json'
IDENTITY_MASK = 0xA5A5A5A5
IDENTITY_MULTIPLIER = 0x9E3779B1
SETTINGS = {'dimensions', 'strides', 'rate', 'preset', 'crf', 'color',
            'retained_bytes', 'pattern', 'clock', 'origin', 'steps'}
CASE_FIELDS = SETTINGS | {'id', 'mode', 'pictures', 'capacity', 'outcome'}
OUTCOMES = {
    'encode': {'complete', 'empty-input', 'retained-limit'},
    'abandon-input': {'destroyed'},
    'abandon-output': {'destroyed'},
    'admission-cap': {'destroyed'},
    'inject-picture-error': {'terminal-native-error'},
    'inject-eof-error': {'terminal-native-error'},
    'inject-drain-error': {'terminal-native-error'},
    'inject-destroy-error': {'retained-until-exit'},
    'deadline-control': {'deadline-terminated'},
}


def integer(value, low, high):
    if type(value) is not int or not low <= value <= high:
        raise ValueError('case integer is outside its declared range')
    return value


def numbers(value, count, low, high):
    if not isinstance(value, list) or len(value) != count:
        raise ValueError('case array has the wrong shape')
    return tuple(integer(item, low, high) for item in value)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate roster field')
        result[key] = value
    return result


@dataclass(frozen=True)
class Case:
    id: str
    mode: str
    dimensions: tuple
    strides: tuple
    rate: tuple
    preset: int
    crf: int
    color: tuple
    retained_bytes: int
    pattern: str
    pictures: int
    capacity: int
    outcome: str
    clock: tuple
    origin: int
    steps: tuple

    @property
    def keyint(self):
        return max(1, 5 * self.rate[0] // self.rate[1])

    @property
    def picture_bytes(self):
        return self.dimensions[0] * self.dimensions[1] * 3 // 2


def load(path=ROSTER):
    with Path(path).open(encoding='utf-8') as source:
        document = json.load(source, object_pairs_hook=unique_object)
    if not isinstance(document, dict) or set(document) != {'version', 'defaults', 'cases'}:
        raise ValueError('invalid roster root')
    if type(document['version']) is not int or document['version'] != 1:
        raise ValueError('unsupported roster version')
    defaults = document['defaults']
    if not isinstance(defaults, dict) or set(defaults) != SETTINGS:
        raise ValueError('invalid roster defaults')
    rows = document['cases']
    if not isinstance(rows, list) or not rows:
        raise ValueError('empty roster')
    result = []
    ids = set()
    for row in rows:
        if not isinstance(row, dict) or set(row) - CASE_FIELDS:
            raise ValueError('unknown case field')
        if not {'id', 'mode', 'pictures', 'capacity', 'outcome'} <= set(row):
            raise ValueError('incomplete case')
        values = defaults | row
        name = values['id']
        if not isinstance(name, str) or not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', name):
            raise ValueError('invalid case identity')
        if name in ids:
            raise ValueError('duplicate case identity')
        ids.add(name)
        mode = values['mode']
        if not isinstance(mode, str) or mode not in OUTCOMES:
            raise ValueError('unknown case mode')
        if not isinstance(values['outcome'], str) or values['outcome'] not in OUTCOMES[mode]:
            raise ValueError('invalid case outcome')
        width, height = numbers(values['dimensions'], 2, 64, 16384)
        if height > 8704 or width % 2 or height % 2:
            raise ValueError('invalid case dimensions')
        strides = numbers(values['strides'], 3, 1, 16384)
        if strides != (width, width // 2, width // 2):
            raise ValueError('worker cases require exact packed strides')
        rate = numbers(values['rate'], 2, 1, (1 << 32) - 1)
        keyint = max(1, 5 * rate[0] // rate[1])
        if keyint > (1 << 31) - 1:
            raise ValueError('case keyframe interval overflows')
        color = numbers(values['color'], 5, 0, 255)
        primaries, transfer, matrix, full_range, chroma = color
        if (primaries not in {1, 2, *range(4, 13), 22}
                or transfer not in {1, 2, *range(4, 16), 17}
                or matrix not in {1, 2, *range(4, 15)}
                or full_range not in (0, 1) or chroma not in (0, 1, 2)
                or (primaries == 2 and matrix in (12, 13))):
            raise ValueError('case colour is outside the SDR subset')
        if values['pattern'] != 'binary-picture-identity-v1':
            raise ValueError('unknown synthetic picture pattern')
        pictures = integer(values['pictures'], 0, (1 << 32) - 1)
        clock = (1, rate[0]) if values['clock'] == 'frame-rate' else numbers(values['clock'], 2, 1, (1 << 32) - 1)
        origin = integer(values['origin'], -(1 << 63), (1 << 63) - 1)
        if not isinstance(values['steps'], list) or not 1 <= len(values['steps']) <= 16:
            raise ValueError('invalid case clock steps')
        steps = numbers(values['steps'], len(values['steps']), 1, (1 << 31) - 1)
        if values['clock'] == 'frame-rate':
            steps = tuple(step * rate[1] for step in steps)
        cycles, remainder = divmod(max(pictures - 1, 0), len(steps))
        if origin + cycles * sum(steps) + sum(steps[:remainder]) > (1 << 63) - 1:
            raise ValueError('case presentation times overflow')
        cycles, remainder = divmod(pictures, len(steps))
        duration = cycles * sum(steps) + sum(steps[:remainder])
        if pictures and Fraction(pictures * clock[1], duration * clock[0]) != Fraction(*rate):
            raise ValueError('case encoder rate differs from its exact source clock')
        capacity = integer(values['capacity'], 0, keyint)
        if ((values['outcome'] == 'empty-input') != (pictures == 0)
                or (values['outcome'] == 'retained-limit') != (capacity == 0)):
            raise ValueError('case count or admission outcome is inconsistent')
        result.append(Case(
            name, mode, (width, height), strides, rate,
            integer(values['preset'], 0, 13), integer(values['crf'], 0, 63), color,
            integer(values['retained_bytes'], 1, 1 << 30), values['pattern'],
            pictures, capacity, values['outcome'],
            clock, origin, steps,
        ))
    return tuple(result)


def picture(case, identity):
    integer(identity, 0, (1 << 32) - 1)
    identity = ((identity * IDENTITY_MULTIPLIER) ^ IDENTITY_MASK) & 0xFFFFFFFF
    width, height = case.dimensions
    luma = bytearray()
    for row in range(height):
        band = 8 * (4 * row // height)
        luma.extend(224 if identity & (1 << (band + 8 * column // width)) else 32
                    for column in range(width))
    luma.extend(b'\x80' * (width * height // 2))
    return luma


def write_input(writer, case):
    width, height = case.dimensions
    writer.write(struct.pack(
        '<8sIIII8BQQQI', b'AV1IN\0\x01\0', width, height, *case.rate,
        case.preset, case.crf, *case.color, 0,
        case.retained_bytes, 64 << 30, max(1, case.pictures), 64 << 20,
    ))
    pts = case.origin
    for identity in range(case.pictures):
        writer.write(struct.pack('<BQq', 1, identity, pts))
        writer.write(picture(case, identity))
        pts += case.steps[identity % len(case.steps)]
    writer.write(struct.pack('<BQ', 2, case.pictures))
