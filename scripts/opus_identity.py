#!/usr/bin/env python3
"""Compare the per-packet raw Opus dumps registered for speed round 3."""
import argparse
import json
import pathlib
import struct


class InvalidDump(Exception):
    pass


def packets(path, expected, channels):
    with pathlib.Path(path).open('rb') as source:
        count = source.read(8)
        if len(count) != 8 or struct.unpack('<Q', count)[0] != expected:
            raise InvalidDump
        for index in range(expected):
            header = source.read(32)
            if len(header) != 32:
                raise InvalidDump
            position, frames, coded_channels, scalars = struct.unpack('<4Q', header)
            if (position != index or not 0 < frames <= 5760
                    or coded_channels != channels or scalars != frames * channels):
                raise InvalidDump
            payload = source.read(scalars * 4)
            if len(payload) != scalars * 4:
                raise InvalidDump
            yield header, payload
        if source.read(1):
            raise InvalidDump


def compare(base, head, expected_packets, channels):
    if expected_packets < 1 or not 1 <= channels <= 255:
        raise ValueError('positive packet count and 1 to 255 channels required')
    readers = [packets(base, expected_packets, channels), packets(head, expected_packets, channels)]
    mismatch = None
    index = 0
    side = 0
    try:
        for index in range(expected_packets):
            values = []
            for side, reader in enumerate(readers):
                values.append(next(reader))
            (base_header, base_data), (head_header, head_data) = values
            kind = 'counts' if base_header != head_header else 'sample_bits' if base_data != head_data else None
            if kind and mismatch is None:
                mismatch = {'packet': index, 'kind': kind}
        for side, reader in enumerate(readers):
            if next(reader, None) is not None:
                raise InvalidDump
    except (InvalidDump, StopIteration):
        return {'passed': False, 'packets': expected_packets,
                'first_mismatch': {'packet': index, 'kind': 'malformed_dump', 'side': ('base', 'head')[side]}}
    except OSError:
        return {'passed': False, 'packets': expected_packets,
                'first_mismatch': {'packet': index, 'kind': 'capture_io', 'side': ('base', 'head')[side]}}
    finally:
        for reader in readers:
            reader.close()
    return {'passed': mismatch is None, 'packets': expected_packets, 'first_mismatch': mismatch}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('base', type=pathlib.Path)
    parser.add_argument('head', type=pathlib.Path)
    parser.add_argument('--packets', type=int, required=True)
    parser.add_argument('--channels', type=int, required=True)
    args = parser.parse_args()
    result = compare(args.base, args.head, args.packets, args.channels)
    print(json.dumps(result))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
