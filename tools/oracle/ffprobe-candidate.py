#!/usr/bin/env python3
"""Example candidate executable: a fresh black-box capture on one fixture."""
import argparse
import importlib.util
import json
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("oracle", Path(__file__).resolve().parents[2] / "scripts" / "oracle.py")
oracle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oracle)
if len(sys.argv) != 2:
    sys.exit("usage: ffprobe-candidate.py FIXTURE_PATH")
try:
    value = oracle.capture_one(Path(sys.argv[1]), argparse.Namespace(ffprobe="ffprobe", ffmpeg="ffmpeg", timeout=120))
    print(json.dumps(value, sort_keys=True))
except (OSError, ValueError, RuntimeError) as error:
    print(str(error), file=sys.stderr)
    sys.exit(2)
