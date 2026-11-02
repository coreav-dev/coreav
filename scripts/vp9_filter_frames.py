"""Generated keyframe controls from VP9 draft 0.7, clauses 6.2.1 and 6.2.8–6.2.13."""
from dataclasses import dataclass
from fractions import Fraction
import struct


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def number(value, low, high):
    return type(value) is int and low <= value <= high


def literal(value, width):
    require(number(value, 0, (1 << width) - 1), 'literal outside its bit width')
    return f'{value:0{width}b}'


class Bits:
    def __init__(self, data):
        require(0 < len(data) <= 1 << 20, 'generated packet size outside its bound')
        self.bits = ''.join(f'{byte:08b}' for byte in data)
        self.position = 0

    def read(self, width):
        require(self.position + width <= len(self.bits), 'truncated generated header')
        value = int(self.bits[self.position:self.position + width], 2)
        self.position += width
        return value


@dataclass(frozen=True)
class KeyFrame:
    width: int
    height: int
    prefix: str
    quantization: str
    suffix: str
    entropy: bytes
    control: dict


def keyframe(data):
    r = Bits(data)
    require(r.read(2) == 2 and r.read(2) == 0 and r.read(1) == 0,
            'expected profile 0 coded frame')
    require(r.read(1) == 0 and r.read(1) == 1, 'expected a shown keyframe')
    resilient = r.read(1)
    require(r.read(24) == 0x498342, 'invalid keyframe sync code')
    require(r.read(3) != 7, 'RGB is outside the generated subset')
    r.read(1)
    width, height = r.read(16) + 1, r.read(16) + 1
    require(number(width, 16, 128) and number(height, 16, 128)
            and width % 2 == height % 2 == 0, 'generated dimensions outside the subset')
    if r.read(1):
        require((r.read(16) + 1, r.read(16) + 1) == (width, height), 'scaled render size')
    if not resilient:
        r.read(2)
    r.read(2)
    start = r.position
    level, sharpness, deltas = r.read(6), r.read(3), r.read(1)
    if deltas and r.read(1):
        for _ in range(6):
            if r.read(1):
                r.read(7)
    quant = r.position
    require(r.read(8) > 0, 'lossless input cannot witness a loop filter')
    for _ in range(3):
        if r.read(1):
            r.read(5)
    segment = r.position
    enabled, absolute, alt = bool(r.read(1)), False, None
    if enabled:
        require(r.read(1) == 0, 'a coded segmentation map changes entropy syntax')
        require(r.read(1) == 1, 'generated segmentation must define its data')
        absolute = bool(r.read(1))
        for index in range(8):
            for feature in range(4):
                if r.read(1):
                    require(index == 0 and feature == 1, 'unexpected segment feature')
                    magnitude, sign = r.read(6), r.read(1)
                    require(not (absolute and sign), 'negative absolute filter level')
                    alt = -magnitude if sign else magnitude
    tiles = r.position
    require(r.read(1) == 0, 'generated input must have one tile row')
    header_size = r.read(16)
    end = r.position
    padding = (-end) % 8
    if padding:
        require(r.read(padding) == 0, 'nonzero uncompressed-header padding')
    entropy = data[r.position // 8:]
    require(0 < header_size < len(entropy), 'invalid compressed-header size')
    return KeyFrame(width, height, r.bits[:start], r.bits[quant:segment], r.bits[tiles:end],
                    entropy, dict(level=level, sharpness=sharpness, deltas=bool(deltas),
                                  segmentation=enabled, alt_l=alt, absolute=absolute))


def rewrite(template, level, setting):
    require(number(level, 1, 63) and set(setting) == {'sharpness', 'segmentation', 'alt_l', 'absolute'},
            'invalid filter control')
    sharp, enabled, alt, absolute = (setting[k] for k in ('sharpness', 'segmentation', 'alt_l', 'absolute'))
    require(number(sharp, 0, 7) and type(enabled) is bool and type(absolute) is bool,
            'invalid filter control types')
    require(alt is None or number(alt, 0 if absolute else -63, 63), 'invalid segment filter value')
    require((enabled or (alt is None and not absolute)) and (not absolute or alt is not None),
            'segment data without an enabled feature')
    segment = '0'
    if enabled:
        segment = '101' + str(int(absolute))
        for index in range(8):
            for feature in range(4):
                active = index == 0 and feature == 1 and alt is not None
                segment += str(int(active))
                if active:
                    segment += literal(abs(alt), 6) + str(int(alt < 0))
    bits = (template.prefix + literal(level, 6) + literal(sharp, 3) + '0'
            + template.quantization + segment + template.suffix)
    bits += '0' * ((-len(bits)) % 8)
    packet = bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8)) + template.entropy
    checked = keyframe(packet)
    require((checked.prefix, checked.quantization, checked.suffix, checked.entropy)
            == (template.prefix, template.quantization, template.suffix, template.entropy),
            'rewrite changed non-filter syntax or entropy bytes')
    require(checked.control == dict(level=level, deltas=False, **setting), 'rewritten control differs')
    return packet


def ivf(data, *, tick=None):
    require(44 < len(data) <= (1 << 20) + 44, 'invalid generated IVF length')
    magic, version, length, codec, width, height, rate, scale, count, unused = struct.unpack('<4sHH4sHHIIII', data[:32])
    require((magic, version, length, codec, count, unused)
            == (b'DKIF', 0, 32, b'VP90', 1, 0) and rate > 0 and scale > 0,
            'unexpected generated IVF header')
    require(tick is None or Fraction(scale, rate) == tick, 'unexpected source IVF clock')
    size, timestamp = struct.unpack('<IQ', data[32:44])
    require(size == len(data) - 44 and timestamp == 0, 'expected exactly one IVF packet at zero')
    packet = data[44:]
    frame = keyframe(packet)
    require((frame.width, frame.height) == (width, height), 'IVF dimensions differ from the packet')
    return packet


def wrap(packet):
    frame = keyframe(packet)
    return (struct.pack('<4sHH4sHHIIII', b'DKIF', 0, 32, b'VP90', frame.width, frame.height, 1, 1, 1, 0)
            + struct.pack('<IQ', len(packet), 0) + packet)


def picture(pattern, width, height):
    require(pattern in ('steps', 'ramp', 'texture') and number(width, 16, 128)
            and number(height, 16, 128) and width % 2 == height % 2 == 0,
            'invalid generated picture')
    def sample(x, y):
        if pattern == 'steps':
            return 80 + 4 * ((x // 8 + y // 8) % 4) + (3 * x + 5 * y) % 3
        if pattern == 'ramp':
            return 64 + (3 * x + 2 * y) % 64
        return 96 + ((17 * x) ^ (11 * y)) % 32
    return bytes(sample(x, y) for y in range(height) for x in range(width)) + bytes([128]) * (width * height // 2)
