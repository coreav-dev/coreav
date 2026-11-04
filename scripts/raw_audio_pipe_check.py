"""Compare complete native raw-audio outputs with the pinned oracle."""

import argparse
import os
from pathlib import Path
import platform
import struct
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT


def run(args, *, data=None, env=None):
    result = subprocess.run([str(arg) for arg in args], input=data, capture_output=True,
                            env=env, timeout=30)
    if result.returncode:
        raise AssertionError(f"command failed ({result.returncode}): {args}\n"
                             f"{result.stderr.decode(errors='replace')}")
    return result.stdout


def require_equal(actual, expected, case):
    if actual != expected:
        offset = next((index for index, pair in enumerate(zip(actual, expected))
                       if pair[0] != pair[1]), min(len(actual), len(expected)))
        raise AssertionError(f"complete raw bytes differ: {case}; offset {offset}, "
                             f"lengths {len(actual)}/{len(expected)}; reason=raw-bytes; "
                             f"actual={actual[offset:offset + 16].hex()} "
                             f"expected={expected[offset:offset + 16].hex()}")


def native_command(coreav, before, input_path, after, output):
    return [str(coreav), "ffmpeg", *before, "-i", str(input_path), *after, str(output)]


def wav_samples(path):
    data = path.read_bytes()
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise AssertionError("native decoder reference is not WAV")
    offset = 12
    while offset + 8 <= len(data):
        size = int.from_bytes(data[offset + 4:offset + 8], "little")
        end = offset + 8 + size
        if end > len(data):
            raise AssertionError("native decoder reference has a truncated chunk")
        if data[offset:offset + 4] == b"data":
            return data[offset + 8:end]
        offset = end + size % 2
    raise AssertionError("native decoder reference has no samples")


def require_refusal(command, env, data=None):
    refused = subprocess.run(command, input=data, capture_output=True, env=env, timeout=30)
    if refused.returncode != 4 or refused.stdout:
        raise AssertionError(f"unregistered path did not refuse cleanly: {command}")


WAVE_WIDTHS = {"u8": 1, "s16": 2, "s24": 3, "s32": 4, "f32": 4, "f64": 8}


def wave_samples(representation, channels, frames):
    width = WAVE_WIDTHS[representation]
    if representation.startswith("f"):
        boundaries = [0x00000000, 0x80000000, 0x00000001, 0x80000001,
                      0x3f800000, 0xbf800000, 0x40000000, 0xc0000000]
        for word in [0x37800000, 0x38400000, 0x3f7fff00]:
            boundaries.extend(value | sign for sign in [0, 0x80000000]
                              for value in [word - 1, word, word + 1])
        values = [((i * 7919 + c * 11717) % 16777217 - 8388608) / 8388608
                  for i in range(frames) for c in range(channels)]
        for index, word in enumerate(boundaries[:len(values)]):
            values[index] = struct.unpack("<f", struct.pack("<I", word))[0]
        return b"".join(struct.pack("<f" if width == 4 else "<d", value) for value in values)
    bits = width * 8
    values = [((i * 7919 + c * 11717) % (1 << bits)) - (1 << (bits - 1))
              for i in range(frames) for c in range(channels)]
    extrema = [-(1 << (bits - 1)), (1 << (bits - 1)) - 1, -1, 0, 1, 3]
    values[:min(len(values), len(extrema))] = extrema[:len(values)]
    return b"".join((value + 128 if representation == "u8" else value).to_bytes(
        width, "little", signed=representation != "u8") for value in values)


def wave_bytes(payload, representation, channels=1, rate=48000, *, container="RIFF",
               extensible=False, mask=None, valid_bits=None, metadata=False, format_tag=None):
    endian = ">" if container == "RIFX" else "<"
    width = WAVE_WIDTHS[representation]
    tag = 3 if representation.startswith("f") else 1
    if format_tag is not None:
        tag = format_tag
    block = width * channels
    fmt = struct.pack(endian + "HHIIHH", 0xfffe if extensible else tag, channels,
                      rate, rate * block, block, width * 8)
    if extensible:
        fmt += struct.pack("<HHI", 22, width * 8 if valid_bits is None else valid_bits,
                           (4 if channels == 1 else 3) if mask is None else mask)
        fmt += struct.pack("<IHH8s", tag, 0, 0x10, b"\x80\x00\x00\xaa\x00\x38\x9b\x71")
    elif tag == 3:
        fmt += struct.pack(endian + "H", 0)

    def chunk(name, content, size=None):
        return name + struct.pack(endian + "I", len(content) if size is None else size) + content + b"\0" * (len(content) % 2)

    chunks = chunk(b"fmt ", fmt)
    if metadata:
        chunks += chunk(b"JUNK", b"\x91\x82\x73")
        chunks += chunk(b"LIST", b"INFO" + chunk(b"INAM", b"Generated PCM\0"))
    if tag == 3:
        chunks += chunk(b"fact", struct.pack(endian + "I", len(payload) // block))
    chunks += chunk(b"data", payload, 0xffffffff if container in ("RF64", "BW64") else None)
    if metadata:
        chunks += chunk(b"LIST", b"INFO" + chunk(b"ICMT", b"Tail\0"))
        chunks += chunk(b"PAD ", b"\x55")
    if container in ("RF64", "BW64"):
        ds64 = struct.pack("<QQQI", 4 + 36 + len(chunks), len(payload), len(payload) // block, 0)
        return container.encode() + struct.pack("<I", 0xffffffff) + b"WAVE" + chunk(b"ds64", ds64) + chunks
    return container.encode() + struct.pack(endian + "I", 4 + len(chunks)) + b"WAVE" + chunks


def wave_identity(payload, representation, output_format):
    if representation == "f32":
        if output_format != "f32le":
            raise ValueError("unregistered WAVE float packing")
        return payload
    width = WAVE_WIDTHS[representation]
    output = bytearray()
    for offset in range(0, len(payload), width):
        sample = payload[offset:offset + width]
        value = int.from_bytes(sample, "little", signed=representation != "u8")
        if representation == "u8":
            value -= 128
        packed = value << 8 if width == 1 else value >> (width * 8 - 16)
        value /= 1 << (width * 8 - 1)
        output += (struct.pack("<h", packed) if output_format == "s16le" else
                   struct.pack("<f", value))
    return bytes(output)


def require_replay(command, encoded, env):
    replay = subprocess.run(command, input=encoded, capture_output=True, env=env, timeout=30)
    expected = ("\n".join(str(arg) for arg in command[2:]) + "\n").encode() + encoded
    if replay.returncode != 7 or replay.stdout != expected:
        raise AssertionError(f"WAVE original-byte replay failed ({replay.returncode}): "
                             f"{replay.stderr[-2000:].decode(errors='replace')}")


def check_wave_case(coreav, ffmpeg, root, env, name, encoded, payload, representation,
                    channels, rate, *, output_rate=None, smoke=False, forms=("f32le", "s16le")):
    source = root / f"wave-{name}.wav"
    source.write_bytes(encoded)
    count = 0
    for form in forms:
        after = ["-ac", str(channels), "-ar", str(rate if output_rate is None else output_rate),
                 "-f", form]
        registered = not (representation == "u8" and form == "s16le"
                          and output_rate not in (None, rate))
        execution_env = env
        if not registered:
            execution_env = {**env, "COREAV_FALLBACK": str(ffmpeg)}
            execution_env.pop("COREAV_STRICT_SAFE", None)
        expected = run([ffmpeg, "-v", "error", *AUDIO_DECODE_INPUT, "-i", source,
                        *after, "pipe:"])
        if output_rate is None:
            require_equal(expected, wave_identity(payload, representation, form),
                          f"WAVE oracle/source identity/{name}/{form}")
        for explicit in (False, True):
            options = [*after, *(["-c:a", f"pcm_{form}"] if explicit else [])]
            modes = [(source, None), ("pipe:0", encoded)]
            if smoke:
                modes = modes[1:] if explicit else modes[:1]
            for input_name, data in modes:
                command = native_command(coreav, ["-threads", "1"], input_name, options, "pipe:1")
                if not registered:
                    require_refusal(command, env, data)
                    count += 1
                actual = run(command, data=data, env=execution_env)
                require_equal(actual, expected, f"WAVE/{name}/{form}/{input_name}/{explicit}")
                count += 1
            output = root / "wave-out.raw"
            command = native_command(coreav, ["-y", "-threads", "1"], "pipe:0", options, output)
            if not registered:
                output.unlink(missing_ok=True)
                require_refusal(command, env, encoded)
                if output.exists():
                    raise AssertionError("unregistered WAVE rate created output")
                output.write_bytes(b"preserve")
                require_refusal(command, env, encoded)
                if output.read_bytes() != b"preserve":
                    raise AssertionError("unregistered WAVE rate replaced output")
                count += 2
            run(command, data=encoded, env=execution_env)
            require_equal(output.read_bytes(), expected, f"WAVE file/{name}/{form}/{explicit}")
            count += 1
    return count


def wave_paths(coreav, oracle, root, smoke=False):
    ffmpeg = oracle / "ffmpeg"
    env = {**os.environ, "COREAV_STRICT_SAFE": "1",
           "COREAV_FALLBACK": str(root / "missing-fallback")}
    count = 0
    for representation in ("u8", "s16", "s24", "s32", "f32"):
        forms = ("f32le",) if representation == "f32" else ("f32le", "s16le")
        for channels in (1, 2):
            rate = 44100 if channels == 1 else 48000
            payload = wave_samples(representation, channels, 4097)
            for container, extensible, mask in [("RIFF", False, None), ("RIFF", True, 0),
                                                 ("RF64", True, None), ("BW64", False, None)]:
                encoded = wave_bytes(payload, representation, channels, rate, container=container,
                                     extensible=extensible, mask=mask, metadata=True)
                count += check_wave_case(coreav, ffmpeg, root, env,
                                         f"{representation}-{channels}-{container}-{extensible}",
                                         encoded, payload, representation, channels, rate,
                                         smoke=smoke, forms=forms)
        for frames in ([0, 1, 33] if smoke else [0, 1, 2, 15, 16, 17, 31, 32, 33]):
            channels = 1 + frames % 2
            payload = wave_samples(representation, channels, frames)
            count += check_wave_case(coreav, ffmpeg, root, env, f"{representation}-short-{frames}",
                                     wave_bytes(payload, representation, channels), payload,
                                     representation, channels, 48000, smoke=smoke, forms=forms)
        if not smoke:
            payload = wave_samples(representation, 2, 32771)
            count += check_wave_case(coreav, ffmpeg, root, env, f"{representation}-chunk-boundary",
                                     wave_bytes(payload, representation, 2, metadata=True), payload,
                                     representation, 2, 48000, forms=forms)
        if representation != "f32" and (sys.platform, platform.machine()) == ("linux", "x86_64"):
            for rate in (44100, 48000):
                for frames in ([4097] if smoke else [0, 1, 2, 15, 16, 17, 31, 32, 33, 4097]):
                    payload = wave_samples(representation, 1, frames)
                    container = ("RIFF", "RF64", "BW64")[frames % 3]
                    encoded = wave_bytes(payload, representation, 1, rate, container=container,
                                         extensible=True, metadata=frames != 0)
                    count += check_wave_case(coreav, ffmpeg, root, env,
                                             f"{representation}-{rate}-to-16000-{frames}", encoded,
                                             payload, representation, 1, rate,
                                             output_rate=16000, smoke=smoke)
                    if representation == "s16":
                        payload = wave_samples(representation, 2, frames)
                        encoded = wave_bytes(payload, representation, 2, rate, container=container,
                                             extensible=True, metadata=frames != 0)
                        count += check_wave_case(coreav, ffmpeg, root, env,
                                                 f"s16-stereo-{rate}-to-16000-{frames}", encoded,
                                                 payload, representation, 2, rate,
                                                 output_rate=16000, smoke=smoke, forms=("s16le",))
    payload = struct.pack("<IIII", 0x7f7fffff, 0xff7fffff, 0x00800000, 0x80800000)
    count += check_wave_case(coreav, ffmpeg, root, env, "finite-float-extrema",
                             wave_bytes(payload, "f32"), payload, "f32", 1, 48000,
                             smoke=smoke, forms=("f32le",))
    return count + wave_refusals(coreav, root, env)


def wave_refusals(coreav, root, env):
    fallback = root / "wave-fallback" / "ffmpeg"
    fallback.parent.mkdir(exist_ok=True)
    fallback.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\ncat\nexit 7\n", encoding="utf-8")
    fallback.chmod(0o700)
    replay_env = {**env, "COREAV_FALLBACK": str(fallback)}
    replay_env.pop("COREAV_STRICT_SAFE", None)
    payload = wave_samples("s16", 1, 65)
    complete = wave_bytes(payload, "s16")
    cases = [("f64", wave_bytes(wave_samples("f64", 1, 65), "f64")),
             ("reduced-bits", wave_bytes(wave_samples("s32", 1, 65), "s32",
                                         extensible=True, valid_bits=24)),
             ("truncated-frame", complete[:-1]), ("truncated-data", complete[:-2]),
             ("partial-frame", wave_bytes(payload + b"\0", "s16")),
             ("compressed", wave_bytes(wave_samples("u8", 1, 65), "u8", format_tag=6)),
             ("rifx", wave_bytes(b"\x12\x34" * 65, "s16", container="RIFX")),
             ("mask", wave_bytes(payload, "s16", extensible=True, mask=1))]
    for word in (0x7f800000, 0xff800000, 0x7fc12345):
        floats = wave_samples("f32", 1, 20001) + struct.pack("<I", word)
        cases.append((f"nonfinite-{word:x}", wave_bytes(floats, "f32")))
    for representation in ("u8", "s16", "f32"):
        for container in ("RIFF", "RF64", "BW64"):
            cases.append((f"empty-tail-{representation}-{container}",
                          wave_bytes(b"", representation, 2, container=container, metadata=True)))
    count = 0
    for name, encoded in cases:
        source = root / f"wave-refused-{name}.wav"
        source.write_bytes(encoded)
        for form in ("f32le", "s16le"):
            after = ["-f", form]
            for input_name, data in [(source, None), ("pipe:0", encoded)]:
                command = native_command(coreav, ["-threads", "1"], input_name, after, "pipe:1")
                require_refusal(command, env, data)
                output = root / "wave-refused.raw"
                output.unlink(missing_ok=True)
                require_refusal(native_command(coreav, ["-threads", "1"], input_name, after, output),
                                env, data)
                if output.exists():
                    raise AssertionError(f"refused WAVE created output: {name}")
                output.write_bytes(b"preserved destination")
                require_refusal(native_command(coreav, ["-y", "-threads", "1"], input_name, after, output),
                                env, data)
                require_equal(output.read_bytes(), b"preserved destination", f"WAVE refusal/{name}")
                count += 3
            command = native_command(coreav, ["-threads", "1"], "pipe:0", after, "pipe:1")
            require_replay(command, encoded, replay_env)
            count += 1
    for name, payload in [
            ("float-rounding", wave_samples("f32", 1, 33)),
            ("float-extrema", struct.pack("<II", 0x7f7fffff, 0xff7fffff))]:
        encoded = wave_bytes(payload, "f32")
        source = root / f"wave-refused-{name}.wav"
        source.write_bytes(encoded)
        for input_name, data in [(source, None), ("pipe:0", encoded)]:
            command = native_command(coreav, ["-threads", "1"], input_name,
                                     ["-f", "s16le"], "pipe:1")
            require_refusal(command, env, data)
            count += 1
        command = native_command(coreav, ["-threads", "1"], "pipe:0", ["-f", "s16le"], "pipe:1")
        require_replay(command, encoded, replay_env)
        count += 1
    for representation in ("u8", "s16", "s24", "s32", "f32"):
        for channels in (1, 2):
            encoded = wave_bytes(wave_samples(representation, channels, 65), representation, channels)
            for form in ("f32le", "s16le"):
                after = ["-ac", str(3 - channels), "-f", form]
                command = native_command(coreav, ["-threads", "1"], "pipe:0", after, "pipe:1")
                require_refusal(command, env, encoded)
                require_replay(command, encoded, replay_env)
                count += 2
        for rate in (44100, 48000):
            encoded = wave_bytes(wave_samples(representation, 1, 65), representation, rate=rate)
            linux = (sys.platform, platform.machine()) == ("linux", "x86_64")
            rates = [22050] if linux and representation != "f32" else [16000, 22050]
            for output_rate in rates:
                for form in ("f32le", "s16le"):
                    if linux and representation == "s16" and form == "s16le":
                        continue
                    after = ["-ar", str(output_rate), "-f", form]
                    command = native_command(coreav, ["-threads", "1"], "pipe:0", after, "pipe:1")
                    require_refusal(command, env, encoded)
                    require_replay(command, encoded, replay_env)
                    count += 2
            encoded = wave_bytes(wave_samples(representation, 2, 65), representation, 2, rate)
            for form in ("f32le", "s16le"):
                if (representation == "s16" and form == "s16le"
                        and (sys.platform, platform.machine()) == ("linux", "x86_64")):
                    continue
                command = native_command(coreav, ["-threads", "1"], "pipe:0",
                                         ["-ar", "16000", "-f", form], "pipe:1")
                require_refusal(command, env, encoded)
                require_replay(command, encoded, replay_env)
                count += 2
    return count


def conformance_paths(coreav, oracle, root):
    ffmpeg = oracle / "ffmpeg"
    env = {**os.environ, "COREAV_STRICT_SAFE": "1",
           "COREAV_FALLBACK": str(root / "missing-fallback")}
    fallback = root / "ffmpeg"
    fallback.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\ncat\nexit 7\n", encoding="utf-8")
    fallback.chmod(0o700)
    replay_env = {**env, "COREAV_FALLBACK": str(fallback)}
    replay_env.pop("COREAV_STRICT_SAFE", None)
    count = 0
    for channels, rate in [(1, 44100), (2, 48000)]:
        samples = b"".join(struct.pack("<h", (i * 7919 % 50001) - 25000)
                           for i in range(8192 * channels))
        source = root / f"mp3-{channels}.s16"
        source.write_bytes(samples)
        encoded = root / f"mp3-{channels}.mp4"
        run([ffmpeg, "-v", "error", "-f", "s16le", "-ar", str(rate), "-ac", str(channels),
             *AUDIO_DECODE_INPUT, "-i", source, "-c:a", "libmp3lame", encoded])
        for form, width in [("f32le", 4), ("s16le", 2)]:
            reference = root / f"native-{channels}-{form}.wav"
            run([coreav, "decode-audio", "--threads", "2", "--format", form[:3],
                 encoded, reference], env=env)
            expected = wav_samples(reference)
            if len(expected) != 8192 * channels * width:
                raise AssertionError("MP3 presentation did not preserve its finite edit count")
            after = ["-f", form]
            for input_path, data in [(encoded, None), ("pipe:", encoded.read_bytes())]:
                actual = run(native_command(coreav, ["-threads", "2"], input_path, after, "pipe:"),
                             data=data, env=env)
                require_equal(actual, expected, "MP3 packing against native conformance decoder")
                count += 1
            output = root / f"mp3-{channels}-{form}.raw"
            run(native_command(coreav, [], encoded, after, output), env=env)
            require_equal(output.read_bytes(), expected, "MP3 native file packing")
            count += 1
            command = native_command(coreav, [], encoded, ["-ar", "16000", *after], "pipe:")
            if channels == 1 and (sys.platform, platform.machine()) == ("linux", "x86_64"):
                floats = root / "native-floats.raw"
                floats.write_bytes(wav_samples(root / "native-1-f32le.wav"))
                converted = run([ffmpeg, "-v", "error", "-f", "f32le", "-ar", str(rate),
                                 "-ac", "1", *AUDIO_DECODE_INPUT, "-i", floats,
                                 "-ar", "16000", *after, "pipe:"])
                require_equal(run(command, env=env), converted,
                              "MP3 exact rate stage over identical native decoded floats")
            else:
                require_refusal(command, env)
            count += 1
        require_refusal(native_command(coreav, [], encoded, ["-ac", str(3 - channels),
                                                            "-f", "f32le"], "pipe:"), env)
        count += 1
    for encoder, rate, containers in [("libmp3lame", 22050, ["mp4"]),
                                       ("libmp3lame", 44100, ["mka"]),
                                       ("aac", 44100, ["mp4", "mka"]),
                                       ("libopus", 48000, ["mp4", "mka"])]:
        for container in containers:
            encoded = root / f"refused-{encoder}-{rate}.{container}"
            run([ffmpeg, "-v", "error", "-f", "s16le", "-ar", str(rate), "-ac", "1",
                 *AUDIO_DECODE_INPUT, "-i", root / "mp3-1.s16", "-c:a", encoder, encoded])
            for form in ["f32le", "s16le"]:
                command = native_command(coreav, [], "pipe:", ["-f", form], "pipe:")
                if encoder == "libopus" and container == "mp4":
                    reference = root / f"qualified-opus-{form}.wav"
                    run([coreav, "decode-audio", "--format", form[:3], encoded, reference], env=env)
                    require_equal(run(command, data=encoded.read_bytes(), env=env), wav_samples(reference),
                                  "qualified Opus MP4 stdin preserves native presentation")
                    count += 1
                    continue
                require_refusal(command, env, encoded.read_bytes())
                count += 1
                replay = subprocess.run(command, input=encoded.read_bytes(), capture_output=True,
                                        env=replay_env, timeout=30)
                expected = ("\n".join(command[2:]) + "\n").encode() + encoded.read_bytes()
                if replay.returncode != 7 or replay.stdout != expected:
                    raise AssertionError(f"decoder replay failed ({replay.returncode}): "
                                         f"{replay.stderr[-2000:].decode(errors='replace')}")
                count += 1
    return count


def check(coreav, oracle, root, smoke=False):
    ffmpeg = oracle / "ffmpeg"
    env = {**os.environ, "COREAV_STRICT_SAFE": "1",
           "COREAV_FALLBACK": str(root / "missing-fallback")}
    count = 0
    for bits, channels, rate in [(16, 1, 44100), (16, 2, 48000), (24, 1, 48000), (24, 2, 44100)]:
        values = [((i * 7919 + c * 11717) % (1 << bits)) - (1 << (bits - 1))
                  for i in range(4097) for c in range(channels)]
        values[:6] = [-(1 << (bits - 1)), (1 << (bits - 1)) - 1, -1, 0, 1, 3]
        samples = b"".join(value.to_bytes(bits // 8, "little", signed=True) for value in values)
        source = root / f"input-{bits}-{channels}.s{bits}"
        source.write_bytes(samples)
        raw_options = ["-f", f"s{bits}le", "-ar", str(rate), "-ac", str(channels)]
        inputs = [(source, raw_options)] if bits == 16 else []
        for container in ("mka", "mp4"):
            compressed = root / f"input-{bits}-{channels}.{container}"
            run([ffmpeg, "-v", "error", *raw_options, *AUDIO_DECODE_INPUT, "-i", source,
                 "-c:a", "flac", "-strict", "-2", compressed])
            inputs.append((compressed, []))
        for input_path, before in inputs:
            for output_format in ("s16le", "f32le"):
                after = ["-ac", str(channels), "-ar", str(rate),
                         "-c:a", f"pcm_{output_format}", "-f", output_format]
                expected = run([ffmpeg, "-v", "error", *before, *AUDIO_DECODE_INPUT,
                                "-i", input_path, *after, "pipe:"])
                packed = (b"".join(struct.pack("<h", value >> (bits - 16)) for value in values)
                          if output_format == "s16le" else
                          b"".join(struct.pack("<f", value / (1 << (bits - 1))) for value in values))
                require_equal(expected, packed, "oracle identity against original samples")
                modes = [(str(input_path), None), ("pipe:", input_path.read_bytes())]
                if smoke:
                    modes = modes[-1:]
                for input_name, data in modes:
                    actual = run(native_command(coreav, ["-threads", "2", *before], input_name,
                                                after, "pipe:"), data=data, env=env)
                    require_equal(actual, expected, f"{input_path.name}/{output_format}/{input_name}")
                    count += 1
                output = root / "out.raw"
                run(native_command(coreav, ["-y", *before], input_path, after, output), env=env)
                require_equal(output.read_bytes(), expected, "file output")
                count += 1
            rate_formats = ["s16le"] if bits == 16 else []
            if channels == 1 and not before:
                rate_formats = ["s16le", "f32le"]
            for output_format in rate_formats if not smoke else []:
                after = ["-ar", "16000", "-ac", str(channels), "-f", output_format]
                expected = run([ffmpeg, "-v", "error", *before, *AUDIO_DECODE_INPUT,
                                "-i", input_path, *after, "pipe:"])
                command = native_command(coreav, ["-threads", "1", *before], "pipe:", after, "pipe:")
                if (sys.platform, platform.machine()) != ("linux", "x86_64"):
                    refused = subprocess.run(command, input=input_path.read_bytes(),
                                             capture_output=True, env=env, timeout=30)
                    if (refused.returncode != 4 or refused.stdout
                            or b"unregistered exact raw" not in refused.stderr
                            or b"rate conversion" not in refused.stderr):
                        raise AssertionError("unregistered platform rate stage did not refuse cleanly")
                    fallback_env = {**env, "COREAV_FALLBACK": str(ffmpeg)}
                    fallback_env.pop("COREAV_STRICT_SAFE", None)
                    for input_name, data in [(input_path, None), ("pipe:", input_path.read_bytes())]:
                        fallback = native_command(coreav, ["-threads", "1", *before],
                                                  input_name, after, "pipe:")
                        actual = run(fallback, data=data, env=fallback_env)
                        require_equal(actual, expected, "unregistered platform oracle fallback")
                        count += 1
                else:
                    actual = run(command, data=input_path.read_bytes(), env=env)
                    require_equal(actual, expected, f"S16 rate/{input_path.name}")
                count += 1
    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coreav", type=Path, required=True)
    parser.add_argument("--oracle-bin", type=Path, required=True)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    if sys.platform == "darwin":
        from oracle_pin_macos import verify_bin
    else:
        from oracle_pin import verify_bin
    problems = list(verify_bin(args.oracle_bin))
    if problems:
        raise SystemExit("\n".join(problems))
    with tempfile.TemporaryDirectory(prefix="coreav-raw-audio-") as directory:
        count = check(args.coreav.resolve(), args.oracle_bin.resolve(), Path(directory), args.smoke)
        conformance = conformance_paths(args.coreav.resolve(), args.oracle_bin.resolve(),
                                        Path(directory))
        wave = wave_paths(args.coreav.resolve(), args.oracle_bin.resolve(), Path(directory), args.smoke)
    print(f"raw audio pipes: {count} exact-output, eligibility and fallback checks passed")
    print(f"raw audio pipes: {conformance} conformance-path and refusal checks passed")
    print(f"raw audio pipes: {wave} WAVE exact-output, eligibility and replay checks passed")


if __name__ == "__main__":
    main()
