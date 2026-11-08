"""Check generated encoder VUI with the qualified black-box ffprobe."""
import argparse
from fractions import Fraction
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'scripts'))
import test_oracle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ffmpeg', type=Path, required=True)
    parser.add_argument('--fixtures', type=Path, required=True)
    args = parser.parse_args()
    oracle = test_oracle.require(args.ffmpeg)
    rates = ((24000, 1001), (25, 1), (30000, 1001), (50, 1), (60, 1), (30, 1), (25, 2))
    sars = ((1, 1), (12, 11), (10, 11), (16, 11), (40, 33), (24, 11),
            (20, 11), (32, 11), (80, 33), (18, 11), (15, 11), (64, 33),
            (160, 99), (4, 3), (3, 2), (2, 1), (17, 13), (65535, 65534))
    count = 0
    for num, den in rates:
        for width, height in sars:
            source = args.fixtures / f'{num}-{den}-{width}-{height}.h264'
            result = subprocess.run([
                str(oracle.ffprobe), '-v', 'error', '-select_streams', 'v:0',
                '-show_entries', 'stream=r_frame_rate,avg_frame_rate,sample_aspect_ratio',
                '-of', 'json', str(source),
            ], check=True, capture_output=True, text=True, timeout=30)
            streams = json.loads(result.stdout)['streams']
            if len(streams) != 1:
                raise RuntimeError(f'{source.name}: expected one video stream')
            stream = streams[0]
            for field in ('r_frame_rate', 'avg_frame_rate'):
                if Fraction(stream[field]) != Fraction(num, den):
                    raise RuntimeError(f'{source.name}: {field}={stream[field]}, stream={stream}')
            if Fraction(stream['sample_aspect_ratio'].replace(':', '/')) != Fraction(width, height):
                raise RuntimeError(f'{source.name}: SAR={stream["sample_aspect_ratio"]}')
            count += 1
    print(f'VUI oracle: {count} rate/SAR combinations passed')


if __name__ == '__main__':
    main()
