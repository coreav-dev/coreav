#!/usr/bin/env python3
"""Compare generated native WAV/FLAC probe stdout with the pinned oracle."""

import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
import test_oracle


TIMEOUT = 30
WRITERS = ("default", "json")
PCM = (("u8", 1, 8), ("s16", 1, 16), ("s24", 1, 24),
       ("s32", 1, 32), ("f32", 3, 32), ("f64", 3, 64))


class CheckFailure(Exception):
    pass


def run(command, *, env=None, allow_failure=False):
    command = [str(arg) for arg in command]
    try:
        result = subprocess.run(command, capture_output=True, timeout=TIMEOUT, env=env)
    except subprocess.TimeoutExpired as error:
        raise CheckFailure(f"{Path(command[0]).name}: timeout after {TIMEOUT}s") from error
    except OSError as error:
        raise CheckFailure(f"{Path(command[0]).name}: cannot execute ({type(error).__name__})") from error
    if result.returncode and not allow_failure:
        diagnostic = result.stderr[-1600:].decode("utf-8", errors="replace")
        raise CheckFailure(f"{Path(command[0]).name}: exit {result.returncode}\n{diagnostic}")
    return result


def chunk(kind, payload, endian="<", declared=None):
    return kind + struct.pack(endian + "I", len(payload) if declared is None else declared) + payload + b"\0" * (len(payload) % 2)


def wave(*, tag=1, bits=16, channels=2, rate=48000, frames=480,
         endian="<", mask=None, valid_bits=None, before=b"", after=b"",
         declared_frames=None, partial=b"", container=b"RIFF"):
    width = bits // 8
    samples = bytearray()
    for frame in range(frames):
        for channel in range(channels):
            value = ((frame * 17 + channel * 31) % 127) - 63
            if tag == 3:
                samples.extend(struct.pack(endian + ("f" if bits == 32 else "d"), value / 64))
            else:
                samples.extend((value + 128 if bits == 8 else value * (1 << (bits - 8))).to_bytes(
                    width, "little" if endian == "<" else "big", signed=bits != 8))
    samples.extend(partial)
    align = channels * width
    fmt = struct.pack(endian + "HHIIHH", 0xFFFE if mask is not None else tag,
                      channels, rate, rate * align, align, bits)
    if mask is not None:
        fmt += struct.pack("<HHI", 22, bits if valid_bits is None else valid_bits, mask)
        fmt += struct.pack("<IHH", tag, 0, 0x10) + bytes.fromhex("800000aa00389b71")
    data_size = len(samples) if declared_frames is None else declared_frames * align
    body = chunk(b"fmt ", fmt, endian) + before
    body += chunk(b"data", bytes(samples), endian,
                  0xFFFFFFFF if container in (b"RF64", b"BW64") else data_size) + after
    if container in (b"RF64", b"BW64"):
        ds64 = struct.pack("<QQQI", 4 + 36 + len(body), data_size, frames, 0)
        return container + struct.pack("<I", 0xFFFFFFFF) + b"WAVE" + chunk(b"ds64", ds64) + body
    return (b"RIFX" if endian == ">" else container) + struct.pack(endian + "I", 4 + len(body)) + b"WAVE" + body


def wave_fixtures():
    for name, tag, bits in PCM:
        yield f"wav-{name}-stereo", wave(tag=tag, bits=bits)
    yield "wav-u8-mono-odd-data", wave(bits=8, channels=1, rate=8000, frames=257)
    yield "wav-s16-mono-unusual-rate", wave(channels=1, rate=12345, frames=257)
    for channels, mask, label in ((1, 4, "mono"), (2, 3, "stereo"), (3, 7, "3.0"),
                                  (4, 0x33, "quad"), (6, 0x3F, "5.1-back"),
                                  (6, 0x60F, "5.1-side"), (8, 0x63F, "7.1"),
                                  (2, 0, "unspecified")):
        yield f"wav-extensible-{label}", wave(channels=channels, mask=mask)
    yield "wav-extensible-s24-valid20", wave(bits=24, valid_bits=20, mask=3)
    yield "wav-extensible-s32-valid24", wave(bits=32, valid_bits=24, mask=3)
    yield "wav-extensible-s32-valid32", wave(bits=32, valid_bits=32, mask=3)
    yield "wav-extensible-f32-5.1", wave(tag=3, bits=32, channels=6, mask=0x3F)
    info = b"INFO" + chunk(b"INAM", b"Generated title\0")
    info += chunk(b"IART", b"Fixture artist\0") + chunk(b"ICMT", b"odd text\0")
    yield "wav-odd-padding-info", wave(before=chunk(b"JUNK", b"abc") + chunk(b"LIST", info))
    yield "wav-info-after-data", wave(after=chunk(b"LIST", info))
    yield "wav-truncated-data", wave(frames=240, declared_frames=480)
    yield "wav-partial-sample", wave(frames=240, declared_frames=480, partial=b"\x01")
    yield "wav-rifx-s16", wave(endian=">")
    yield "wav-rifx-f32", wave(endian=">", tag=3, bits=32)
    yield "wav-rf64-s16", wave(container=b"RF64")
    yield "wav-bw64-s16", wave(container=b"BW64")


def split_flac(data):
    if data[:4] != b"fLaC":
        raise CheckFailure("generated FLAC lacks its marker")
    at, blocks = 4, []
    while True:
        if at + 4 > len(data):
            raise CheckFailure("generated FLAC has a truncated metadata header")
        header = data[at:at + 4]
        size = int.from_bytes(header[1:], "big")
        at += 4
        if at + size > len(data):
            raise CheckFailure("generated FLAC has truncated metadata")
        blocks.append((header[0] & 0x7F, data[at:at + size]))
        at += size
        if header[0] & 0x80:
            break
    if blocks[0][0] != 0 or len(blocks[0][1]) != 34 or not data[at:]:
        raise CheckFailure("generated FLAC lacks STREAMINFO or audio frames")
    return blocks[0][1], data[at:]


def flac(streaminfo, frames, blocks=()):
    metadata = [(0, streaminfo), *blocks]
    result = bytearray(b"fLaC")
    for index, (kind, payload) in enumerate(metadata):
        result.append(kind | (0x80 if index == len(metadata) - 1 else 0))
        result.extend(len(payload).to_bytes(3, "big"))
        result.extend(payload)
    return bytes(result) + frames


def comments(entries):
    vendor = b"CoreAV generated fixture"
    result = struct.pack("<I", len(vendor)) + vendor + struct.pack("<I", len(entries))
    for entry in entries:
        value = entry.encode("utf-8")
        result += struct.pack("<I", len(value)) + value
    return result


def generate(oracle, directory):
    paths = []
    for name, data in wave_fixtures():
        path = directory / f"{name}.wav"
        path.write_bytes(data)
        paths.append(path)
    for bits, channels, rate, sample_count in ((16, 1, 44100, 480), (16, 2, 48000, 480),
                                              (24, 2, 48000, 480), (24, 6, 48000, 480),
                                              (16, 2, 32000, 9600)):
        label = f"flac-s{bits}-{channels}ch-{rate}"
        source, encoded = directory / f"{label}-source.wav", directory / f"{label}-encoded.flac"
        source.write_bytes(wave(bits=bits, channels=channels, rate=rate, frames=sample_count,
                                mask=0x3F if channels == 6 else None))
        run([oracle.ffmpeg, "-v", "error", "-nostdin", "-y", *AUDIO_DECODE_INPUT,
             "-i", source, "-map_metadata", "-1", "-c:a", "flac", encoded])
        streaminfo, frames = split_flac(encoded.read_bytes())
        tags = comments(("TITLE=Generated title", "ARTIST=Fixture artist", "DESCRIPTION=Fixture comment",
                         "TRACKNUMBER=7", "custom_key=quote \" slash \\ café", "TITLE=Second title"))
        seek = struct.pack(">QQH", 0, 0, min(sample_count, int.from_bytes(streaminfo[2:4], "big")))
        seek += struct.pack(">QQH", 0xFFFFFFFFFFFFFFFF, 0, 0)
        for suffix, blocks in (("minimal", ()), ("comments-padding", ((4, tags), (1, b"\0" * 31))),
                               ("seektable-comments", ((3, seek), (4, tags)))):
            path = directory / f"{label}-{suffix}.flac"
            path.write_bytes(flac(streaminfo, frames, blocks))
            paths.append(path)
    prefixed = directory / "flac-empty-id3-prefix.flac"
    base = paths[-1].read_bytes()
    prefixed.write_bytes(b"ID3\x04\0\0\0\0\0\0" + base)
    accepted = run([oracle.ffprobe, "-v", "error", "-show_format", "-show_streams", prefixed],
                   allow_failure=True)
    if accepted.returncode == 0:
        paths.append(prefixed)
    else:
        print("SKIP flac-empty-id3-prefix: pinned oracle refuses empty ID3v2.4 prefix", file=sys.stderr)
    footer = directory / "flac-empty-id3-footer.flac"
    footer.write_bytes(b"ID3\x04\0\x10\0\0\0\0" + b"3DI\x04\0\x10\0\0\0\0" + base)
    run([oracle.ffprobe, "-v", "error", "-show_format", "-show_streams", footer])
    paths.append(footer)
    return paths


def native_environment(coreav):
    env = {**os.environ, "COREAV_STRICT_SAFE": "1", "COREAV_FALLBACK": str(coreav / "disabled-fallback")}
    env.pop("COREAV_FALLBACK_ACTIVE", None)
    env.pop("COREAV_FALLBACK_LOG", None)
    return env


def compare(coreav, oracle, paths):
    env = native_environment(coreav)
    comparisons = 0
    for path in paths:
        for writer in WRITERS:
            options = ["-v", "error", "-show_format", "-show_streams", "-of", writer, path]
            try:
                expected = run([oracle.ffprobe, *options]).stdout
                actual = run([coreav, "--strict-safe", "ffprobe", *options], env=env).stdout
            except CheckFailure as error:
                raise CheckFailure(f"{path.name} ({writer}): {error}") from error
            if not expected or actual != expected:
                offset = next((i for i, pair in enumerate(zip(actual, expected)) if pair[0] != pair[1]),
                              min(len(actual), len(expected)))
                raise CheckFailure(f"{path.name} ({writer}): stdout differs at byte {offset}; "
                                   f"native={len(actual)} bytes oracle={len(expected)} bytes\n"
                                   f"native: {actual[max(0, offset - 60):offset + 160]!r}\n"
                                   f"oracle: {expected[max(0, offset - 60):offset + 160]!r}")
            comparisons += 1
    return comparisons


def verify(coreav, oracle, paths):
    refusals = {"wav-rifx-s16", "wav-rifx-f32", "wav-partial-sample"}
    divergent = "wav-extensible-s32-valid24"
    parity = [path for path in paths if path.stem not in refusals | {divergent}]
    counts = {"parity": compare(coreav, oracle, parity), "refusals": 0, "divergences": 0}
    env = native_environment(coreav)
    for path in paths:
        if path.stem not in refusals:
            continue
        for writer in WRITERS:
            result = run([coreav, "--strict-safe", "ffprobe", "-v", "error", "-show_format", "-show_streams",
                          "-of", writer, path], env=env, allow_failure=True)
            if result.returncode != 4 or result.stdout or not result.stderr:
                raise CheckFailure(f"{path.name} ({writer}): expected exit 4, empty stdout "
                                   f"and a diagnostic; got exit {result.returncode}")
            counts["refusals"] += 1
    for path in paths:
        if path.stem != divergent:
            continue
        original = path.read_bytes()
        header = struct.pack("<HHIIHHHHI", 0xFFFE, 2, 48000, 384000, 8, 32, 22, 24, 3)
        guid = struct.pack("<IHH", 1, 0, 0x10) + bytes.fromhex("800000aa00389b71")
        if (len(original) != 3908 or original[:12] != b"RIFF" + struct.pack("<I", 3900) + b"WAVE"
                or original[12:20] != b"fmt " + struct.pack("<I", 40)
                or original[20:44] != header or original[44:60] != guid
                or original[60:68] != b"data" + struct.pack("<I", 3840)):
            raise CheckFailure("PCM32 valid24: expected 480 stereo frames, PCM subtype, storage 32, valid 24 and alignment 8")
        outputs, references = {}, {}
        for writer in WRITERS:
            options = ["-v", "error", "-show_format", "-show_streams", "-of", writer, path]
            references[writer] = run([oracle.ffprobe, *options]).stdout
            outputs[writer] = run([coreav, "--strict-safe", "ffprobe", *options], env=env).stdout
        try:
            observed = json.loads(references["json"])["streams"][0]
            signature = (observed["codec_name"], observed["sample_fmt"], observed["duration_ts"],
                         observed["duration"], observed["bits_per_sample"], observed["channels"], observed["sample_rate"])
        except (ValueError, KeyError, IndexError, TypeError) as error:
            raise CheckFailure("PCM32 valid24: invalid oracle JSON report") from error
        if signature != ("pcm_f24le", "flt", 640, "0.013333", 32, 2, "48000"):
            raise CheckFailure("PCM32 valid24: pinned oracle's registered divergence changed")
        if not all(field in references["default"] for field in (
                b"codec_name=pcm_f24le\n", b"sample_fmt=flt\n", b"duration_ts=640\n", b"duration=0.013333\n")):
            raise CheckFailure("PCM32 valid24: default oracle divergence changed")
        control = bytearray(original)
        control[38:40] = struct.pack("<H", 32)
        try:
            path.write_bytes(control)
            for writer in WRITERS:
                expected = run([oracle.ffprobe, "-v", "error", "-show_format", "-show_streams",
                                "-of", writer, path]).stdout
                if outputs[writer] != expected or outputs[writer] == references[writer]:
                    raise CheckFailure(f"PCM32 valid24 ({writer}): native must match the full PCM32 control report, "
                                       "not the oracle's float reinterpretation")
                counts["divergences"] += 1
        finally:
            path.write_bytes(original)
    return counts


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coreav", type=Path, required=True)
    parser.add_argument("--oracle-dir", type=Path, required=True)
    parser.add_argument("--directory", type=Path, help="retain generated fixtures here; safe to rerun")
    args = parser.parse_args(argv)
    coreav, selected = args.coreav.resolve(), args.oracle_dir.resolve()
    try:
        try:
            oracle = test_oracle.require(selected / "ffmpeg", selected / "ffprobe")
        except test_oracle.QualificationError as error:
            raise CheckFailure(str(error)) from None
        if args.directory:
            directory = args.directory.resolve()
            directory.mkdir(parents=True, exist_ok=True)
            paths = generate(oracle, directory)
            counts = verify(coreav, oracle, paths)
        else:
            with tempfile.TemporaryDirectory(prefix="coreav-standalone-audio-") as temporary:
                paths = generate(oracle, Path(temporary))
                counts = verify(coreav, oracle, paths)
        print(f"Standalone audio probe: {len(paths)} generated fixtures, {counts['parity']} byte-identical "
              f"default/JSON comparisons, {counts['refusals']} strict-safe refusals, "
              f"{counts['divergences']} known-divergence checks")
        return 0
    except (CheckFailure, OSError) as error:
        print(f"standalone audio probe: FAIL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
