"""The deterministic, full-file-identified FLAC concat fixture for current CI."""
import argparse
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))

import test_oracle


def command(ffmpeg, output):
    return [str(ffmpeg), '-v', 'error', '-y', '-cpuflags', '0', '-cpucount', '1',
            '-f', 'lavfi', '-i', 'testsrc2=size=320x240:rate=25:duration=2',
            '-f', 'lavfi', '-i', 'sine=frequency=440:duration=2',
            '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-g', '25',
            '-threads:v', '1', '-x264-params', 'asm=0',
            '-c:a', 'flac', '-strict', '-2', str(output)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output')
    args = parser.parse_args()
    tools = test_oracle.require()
    subprocess.run(command(tools.ffmpeg, args.output), check=True, timeout=120)


if __name__ == '__main__':
    main()
