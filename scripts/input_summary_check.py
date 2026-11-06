#!/usr/bin/env python3
"""Compare native input-summary stderr bytes on small generated media.

Run with --coreav PATH --oracle-dir DIR --directory DIR; --audio adds
standalone WAV/FLAC, and --generate-only prepares fixtures without CoreAV.
For development files, construct Oracle(DIR), then call
compare_file(PATH_TO_COREAV, oracle, media_path, "safe-case-name"). This does
not generate media. Callers must select development inputs, never held-out
inputs. Diagnostics contain only bounded case labels, never child output.
"""

import argparse
from dataclasses import dataclass
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))

import oracle_pin
import oracle_pin_macos
import test_oracle
import standalone_audio_probe_check as audio_probe
import mkv_craft
import stsd_craft


MISSING_OUTPUT = b"At least one output file must be specified\n"
LEVELS = (None, "info", "warning", "error", "quiet")
SPELLINGS = ("ffprobe-bare", "ffprobe-input", "ffmpeg-input")
INPUT = re.compile(rb"(?m)^Input #0, ")
BANNER = re.compile(rb"(?mi)^(?:ffmpeg|ffprobe) version |^configuration: |"
                    rb"^\s*lib(?:avutil|avcodec|avformat|avdevice|avfilter|swscale|swresample)\s")


def case_label(value):
    return value if re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,95}", value) else "case"


class CheckFailure(Exception):
    def __init__(self, case):
        super().__init__(case_label(case))


def run(command, case, *, env=None):
    try:
        return subprocess.run([str(arg) for arg in command], capture_output=True,
                              timeout=30, env=env)
    except (OSError, subprocess.SubprocessError):
        raise CheckFailure(case) from None


class Oracle:
    def __init__(self, directory):
        self.directory = Path(directory).resolve()
        try:
            pair = test_oracle.require(self.directory / "ffmpeg", self.directory / "ffprobe")
        except test_oracle.QualificationError:
            raise CheckFailure("oracle-pin") from None
        self.ffmpeg = pair.ffmpeg
        self.ffprobe = pair.ffprobe


@dataclass(frozen=True)
class Fixture:
    name: str
    path: Path
    native: bool = True
    structured_side_data: bool = False
    structured: bool = False


def extract_summary(stderr, case, *, ffmpeg=False):
    """Retain the entire summary, including trailing metadata and side data."""
    matches = list(INPUT.finditer(stderr))
    if len(matches) != 1:
        raise CheckFailure(case)
    summary = stderr[matches[0].start():]
    if ffmpeg:
        if not summary.endswith(MISSING_OUTPUT):
            raise CheckFailure(case)
        summary = summary[:-len(MISSING_OUTPUT)]
    lines = summary.split(b"\n")
    for number, line in enumerate(lines[1:], 1):
        if line and not line.startswith(b" "):
            if ffmpeg:
                raise CheckFailure(case)
            summary = b"\n".join(lines[:number]) + b"\n"
            break
    if not summary.endswith(b"\n") or b"  Stream #0:" not in summary:
        raise CheckFailure(case)
    return summary


def compare_results(native, reference, case, *, ffmpeg=False, level=None):
    """Require exact summary bytes, exit status and empty stdout on both sides."""
    code = 1 if ffmpeg else 0
    if native.returncode != code or reference.returncode != code:
        raise CheckFailure(case)
    if native.stdout or reference.stdout:
        raise CheckFailure(case)
    if level in ("warning", "error", "quiet"):
        if INPUT.search(reference.stderr) or native.stderr != reference.stderr:
            raise CheckFailure(case)
        if level == "quiet" and reference.stderr:
            raise CheckFailure(case)
        if ffmpeg and level != "quiet" and not reference.stderr.endswith(MISSING_OUTPUT):
            raise CheckFailure(case)
        return
    expected = extract_summary(reference.stderr, case, ffmpeg=ffmpeg)
    actual = extract_summary(native.stderr, case, ffmpeg=ffmpeg)
    native_start = native.stderr.index(actual)
    reference_start = reference.stderr.index(expected)
    if BANNER.search(native.stderr[:native_start]) or actual != expected or \
            native.stderr[native_start:] != reference.stderr[reference_start:]:
        raise CheckFailure(case)


def check_refusal(result, case):
    if result.returncode != 4 or result.stdout or b"Input #" in result.stderr:
        raise CheckFailure(case)


def command_args(spelling, media, level):
    options = [] if level is None else ["-loglevel", level]
    return [*options, *([] if spelling == "ffprobe-bare" else ["-i"]), str(media)]


def compare_file(coreav, oracle, media, name, *, file_url=False):
    """Check a caller-selected development file without generating anything.

    Return sanitized failing command labels; an empty list means every check
    passed. Oracle construction verifies the platform pin before any use.
    """
    failures = []
    env = {**os.environ, "COREAV_STRICT_SAFE": "1"}
    media = Path(media).resolve()
    if file_url:
        media = "file:" + str(media)
    for spelling in SPELLINGS:
        tool = "ffmpeg" if spelling == "ffmpeg-input" else "ffprobe"
        for level in LEVELS:
            case = f"{case_label(name)[:48]}-{spelling}-{level or 'default'}"
            args = command_args(spelling, media, level)
            try:
                reference = run([getattr(oracle, tool), "-hide_banner", *args], case)
                native = run([coreav, tool, *args], case, env=env)
                compare_results(native, reference, case, ffmpeg=tool == "ffmpeg", level=level)
            except CheckFailure as error:
                failures.append(str(error))
    return failures


def compare_refusals(coreav, media, name):
    """Debug and trace are explicitly unsupported summary command families."""
    failures = []
    env = {**os.environ, "COREAV_STRICT_SAFE": "1"}
    for spelling in SPELLINGS:
        tool = "ffmpeg" if spelling == "ffmpeg-input" else "ffprobe"
        for level in ("debug", "trace"):
            case = f"{case_label(name)[:48]}-unsupported-{spelling}-{level}"
            try:
                result = run([coreav, tool, *command_args(spelling, media, level)], case, env=env)
                check_refusal(result, case)
            except CheckFailure as error:
                failures.append(str(error))
    return failures


def compare_structured_side_data(coreav, oracle, media, name):
    case = f"{case_label(name)}-structured-side-data"
    args = ["-v", "error", "-select_streams", "v:0", "-show_entries",
            "stream_side_data=side_data_type", "-of", "json", str(media)]
    try:
        reference = run([oracle.ffprobe, *args], case)
        native = run([coreav, "ffprobe", *args], case,
                     env={**os.environ, "COREAV_STRICT_SAFE": "1"})
        if reference.returncode or native.returncode or reference.stderr or native.stderr or \
                not reference.stdout or native.stdout != reference.stdout:
            raise CheckFailure(case)
    except CheckFailure as error:
        return [str(error)]
    return []


def compare_structured(coreav, oracle, media, name):
    case = f"{case_label(name)}-structured"
    options = ["-v", "quiet", "-of", "json", "-show_format", "-show_streams", str(media)]
    try:
        native = run([coreav, "ffprobe", *options], case,
                     env={**os.environ, "COREAV_STRICT_SAFE": "1"})
        reference = run([oracle.ffprobe, *options], case)
        if native.returncode or reference.returncode or not reference.stdout or \
                native.stdout != reference.stdout or native.stderr != reference.stderr:
            raise CheckFailure(case)
    except CheckFailure as error:
        return [str(error)]
    return []


def compare_unsupported(coreav, oracle, media, name):
    failures = []
    env = {**os.environ, "COREAV_STRICT_SAFE": "1"}
    for spelling in SPELLINGS:
        tool = "ffmpeg" if spelling == "ffmpeg-input" else "ffprobe"
        case = f"{case_label(name)}-unqualified-{spelling}"
        try:
            result = run([coreav, tool, *command_args(spelling, media, None)], case, env=env)
            check_refusal(result, case)
            args = command_args(spelling, media, None)
            fallback_env = {**env, "COREAV_FALLBACK": str(oracle.directory)}
            fallback_env.pop("COREAV_STRICT_SAFE", None)
            reference = run([getattr(oracle, tool), "-hide_banner", *args], case)
            fallback = run([coreav, tool, *args], case, env=fallback_env)
            compare_results(fallback, reference, case, ffmpeg=tool == "ffmpeg")
        except CheckFailure as error:
            failures.append(str(error))
    return failures


def matroska_without_default_duration(data, parent=None):
    out = []
    for kind, body, end, _ in mkv_craft.children(data, 0, len(data)):
        if parent == mkv_craft.TRACK_ENTRY and kind == mkv_craft.DEFAULT_DURATION:
            continue
        if parent == mkv_craft.SEGMENT and kind in (mkv_craft.SEEK_HEAD, mkv_craft.CUES, mkv_craft.TAGS):
            continue
        payload = data[body:end]
        if kind in (mkv_craft.SEGMENT, mkv_craft.TRACKS, mkv_craft.TRACK_ENTRY):
            payload = matroska_without_default_duration(payload, kind)
        out.append(mkv_craft.element(kind, payload))
    return b''.join(out)


def rewrite_avcc(data, change):
    changed = 0

    def rewrite(start, end):
        nonlocal changed
        out = []
        for kind, at, stop in stsd_craft.boxes(data, start, end):
            body = at + 8
            if kind == b"avcC":
                payload = change(data[body:stop])
                changed += 1
            elif kind in (b"moov", b"trak", b"mdia", b"minf", b"stbl"):
                payload = rewrite(body, stop)
            elif kind in (b"stsd", b"avc1"):
                prefix = 8 if kind == b"stsd" else 78
                payload = data[body:body + prefix] + rewrite(body + prefix, stop)
            else:
                out.append(data[at:stop])
                continue
            out.append(stsd_craft.box(kind, payload))
        return b"".join(out)

    top = list(stsd_craft.boxes(data, 0, len(data)))
    if not top or top[-1][0] != b"moov":
        raise CheckFailure("avcc-fixture-layout")
    result = rewrite(0, len(data))
    if changed != 1:
        raise CheckFailure("avcc-fixture-count")
    return result


def avcc_pps(config, change, *, suffix=True):
    if config[5] & 31 != 1:
        raise CheckFailure("avcc-fixture-sps")
    at = 8 + int.from_bytes(config[6:8], "big")
    if config[at] != 1:
        raise CheckFailure("avcc-fixture-pps")
    end = at + 3 + int.from_bytes(config[at + 1:at + 3], "big")
    pps = change(config[at + 3:end])
    return config[:at + 1] + len(pps).to_bytes(2, "big") + pps + (config[end:] if suffix else b"")


def unused_pps(pps):
    rbsp = pps[1:].replace(b"\x00\x00\x03", b"\x00\x00")
    bits = "".join(f"{byte:08b}" for byte in rbsp)
    if not bits.startswith("1") or "1" not in bits:
        raise CheckFailure("avcc-fixture-pps-id")
    bits = "010" + bits[1:bits.rindex("1") + 1]
    bits += "0" * (-len(bits) % 8)
    raw = bytes(int(bits[at:at + 8], 2) for at in range(0, len(bits), 8))
    escaped = bytearray(pps[:1])
    zeros = 0
    for byte in raw:
        if zeros == 2 and byte <= 3:
            escaped.append(3)
            zeros = 0
        escaped.append(byte)
        zeros = zeros + 1 if byte == 0 else 0
    return bytes(escaped)


def hevc_sample_types(path):
    _, _, samples, _ = stsd_craft.read_source(str(path))
    types = set()
    for sample, *_ in samples:
        at = 0
        while at < len(sample):
            size = int.from_bytes(sample[at:at + 4], "big")
            if at + 6 > len(sample) or size < 2 or at + 4 + size > len(sample):
                raise CheckFailure("hevc-fixture-framing")
            types.add((sample[at + 4] >> 1) & 63)
            at += 4 + size
    return types


def generate_fixtures(oracle, directory, *, audio=False):
    """Regenerate only owned names; publish no partial fixture generation."""
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    fixtures = []
    with tempfile.TemporaryDirectory(prefix=".input-summary-", dir=directory) as temporary:
        root = Path(temporary)
        video = ["-f", "lavfi", "-i", "color=c=blue:s=64x48:r=10:d=0.4"]
        sound = ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=0.4"]
        h264 = ["-c:v", "libx264", "-threads:v", "1", "-pix_fmt", "yuv420p"]
        av = [*video, *sound, "-map", "0:v", "-map", "1:a", *h264, "-c:a", "aac"]

        def make(name, extension, options, *, side_data=(), native=True, structured_side_data=False, structured=False, duration="0.4"):
            path = root / f"{name}.{extension}"
            result = run([oracle.ffmpeg, "-hide_banner", "-v", "error", "-nostdin", "-y",
                          *options, "-t", duration, path], f"generate-{name}")
            if result.returncode or not path.is_file() or path.stat().st_size == 0:
                raise CheckFailure(f"generate-{name}")
            if side_data:
                probe = run([oracle.ffprobe, "-hide_banner", path], f"generate-{name}")
                if probe.returncode or any(field not in probe.stderr for field in side_data):
                    raise CheckFailure(f"generate-{name}")
            fixtures.append(Fixture(name, path, native, structured_side_data, structured))

        make("mp4-av", "mp4", av)
        hevc_params = "pools=none:frame-threads=1:log-level=error"
        hevc = ["-c:v", "libx265", "-threads:v", "1", "-pix_fmt", "yuv420p",
                "-x265-params", hevc_params]
        make("mp4-hevc-hev1", "mp4", [*video, *hevc, "-tag:v", "hev1"], structured=True)
        make("mp4-hevc-hvc1", "mp4", [*video, *hevc, "-tag:v", "hvc1"], structured=True)
        make("mp4-hevc-main10", "mp4", [*video, *hevc, "-pix_fmt", "yuv420p10le",
                                           "-tag:v", "hev1"], structured=True)
        make("mp4-hevc-metadata", "mp4", [*video, *hevc, "-tag:v", "hev1",
             "-metadata", "title=HEVC summary", "-metadata:s:v:0", "handler_name=Video: café"], structured=True)
        make("mp4-hevc-hev1-cra", "mp4", ["-f", "lavfi", "-i",
             "testsrc2=size=64x48:rate=10:duration=1.2", *hevc, "-tag:v", "hev1",
             "-x265-params", hevc_params + ":keyint=4:min-keyint=4:scenecut=0"], structured=True, duration="1.2")
        if 21 not in hevc_sample_types(root / "mp4-hevc-hev1-cra.mp4"):
            raise CheckFailure("hevc-cra-fixture")
        for label, option, expected in (("inband", "repeat-headers=1", {32, 33, 34}),
                                         ("aud", "aud=1", {35}), ("hash", "hash=1", {40})):
            name = "mp4-hevc-hev1-" + label
            make(name, "mp4", [*video, *hevc, "-tag:v", "hev1", "-x265-params",
                               hevc_params + ":" + option], native=False)
            if not expected.issubset(hevc_sample_types(root / (name + ".mp4"))):
                raise CheckFailure("hevc-" + label + "-fixture")
        make("mp4-hevc-hev1-with-audio", "mp4", [*video, *sound,
             "-map", "0:v", "-map", "1:a", *hevc, "-tag:v", "hev1", "-c:a", "aac"], native=False)
        make("mp4-unspecified-aspect", "mp4", [*av, "-vf", "setsar=0"])
        for label, change, native in (
            ("header-only-pps", lambda pps: b"\x68", False),
            ("truncated-pps", lambda pps: pps[:-1], False),
            ("unused-pps", unused_pps, False),
            ("without-config-suffix", lambda pps: pps, True),
        ):
            name = "mp4-unspecified-aspect-" + label
            path = root / (name + ".mp4")
            path.write_bytes(rewrite_avcc((root / "mp4-unspecified-aspect.mp4").read_bytes(),
                lambda config: avcc_pps(config, change, suffix=label != "without-config-suffix")))
            fixtures.append(Fixture(name, path, native))
        make("mov-unspecified-aspect", "mov", [*av, "-vf", "setsar=0"])
        make("mp4-unspecified-codec-stream-aspect", "mp4", [
            "-i", root / "mp4-unspecified-aspect.mp4", "-map", "0", "-c", "copy", "-aspect", "8:3"])
        make("mp4-unspecified-aspect-irregular-clock", "mp4", [
            *av, "-vf", "setsar=0", "-r", "30", "-video_track_timescale", "9999"], native=False)
        make("mp4-unspecified-aspect-fragmented", "mp4", [
            *av, "-vf", "setsar=0", "-movflags", "frag_keyframe+empty_moov"], native=False)
        make("mp4-unspecified-aspect-avc3", "mp4", [
            *av, "-vf", "setsar=0", "-tag:v", "avc3"], native=False)
        make("mov-pcm", "mov", [*video, *sound, "-map", "0:v", "-map", "1:a",
                                *h264, "-c:a", "pcm_s16le"], native=False)
        make("mov-aac", "mov", av)
        make("mp4-fragmented", "mp4", [*av, "-movflags", "frag_keyframe+empty_moov"], native=False)
        make("mp4-irregular-clock", "mp4", [*av, "-r", "30", "-video_track_timescale", "9999"], native=False)
        make("mp4-metadata", "mp4", [*av, "-metadata", "title=Summary title",
                                     "-metadata", "comment=first line\nsecond line\rthird line",
                                     "-metadata:s:v:0", "handler_name=Video: café",
                                     "-metadata:s:a:0", "language=fra"])
        make("mp4-long-metadata", "mp4", [*av, "-metadata", "title=" + "a" * 300])
        make("mp4-split-utf8", "mp4", [*av, "-metadata", "title=" + "a" * 254 + "é" + "z" * 20], native=False)
        for label, frame_rate in [("hundredths", "303/10"), ("fraction", "1/5"),
                                   ("small-rate", "1/199")]:
            make("mp4-" + label, "mp4", ["-f", "lavfi", "-i",
                 "color=c=blue:s=64x48:r=" + frame_rate + ":d=0.4", *h264])
        chapters = root / "chapters.ffmetadata"
        chapters.write_text(";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=200\n"
                            "title=Opening\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=200\nEND=400\n"
                            "title=Ending\n", encoding="utf-8")
        make("mp4-chapters", "mp4", [*video, *sound, "-f", "ffmetadata", "-i", chapters,
                                     "-map", "0:v", "-map", "1:a", "-map_chapters", "2",
                                     *h264, "-c:a", "aac"])
        make("mkv-multistream", "mkv", [*video, *sound, "-f", "ffmetadata", "-i", chapters,
                                       "-map", "0:v", "-map", "0:v", "-map", "1:a", "-map", "1:a",
                                       "-map_chapters", "2", *h264, "-c:a", "aac",
                                       "-disposition:v:0", "default", "-disposition:v:1", "0",
                                       "-disposition:a:0", "default",
                                       "-disposition:a:1", "forced+hearing_impaired",
                                       "-metadata", "title=Matroska title",
                                       "-metadata", "comment=first line\nsecond line",
                                       "-metadata:s:v:1", "title=Alternate video",
                                       "-metadata:s:a:0", "language=eng",
                                       "-metadata:s:a:1", "language=fra"])
        large_title = root / "track-name.txt"
        large_title.write_text("title=" + "a" * 1_048_577, encoding="ascii")
        make("mkv-large-track-title", "mkv", [*sound, "-c:a", "pcm_s16le",
             "-/metadata:s:a:0", large_title], native=False)
        make("webm-vp9-opus", "webm", [*video, *sound, "-map", "0:v", "-map", "1:a",
                                       "-c:v", "libvpx-vp9", "-deadline", "realtime",
                                       "-cpu-used", "8", "-threads:v", "1", "-c:a", "libopus"])
        make("ts-av", "ts", [*av, "-f", "mpegts"], native=False)
        make("ts-audio", "ts", [*sound, "-map", "0:a", "-c:a", "aac", "-f", "mpegts"], native=False)
        make("ts-programs", "ts", [*video, *sound, "-map", "0:v", "-map", "1:a", "-map", "1:a",
                                   *h264, "-c:a", "aac", "-program", "program_num=7:st=0:st=1",
                                   "-program", "program_num=12:st=2",
                                   "-metadata:p:0", "service_name=Video service",
                                   "-metadata:p:0", "service_provider=Summary generator",
                                   "-metadata:p:1", "service_name=Audio service", "-f", "mpegts"], native=False)
        make("ts-offset", "ts", [*av, "-output_ts_offset", "3.25", "-f", "mpegts"], native=False)
        mp2 = [*video, *sound, "-map", "0:v", "-map", "1:a", *h264, "-c:a", "mp2", "-b:a", "128k"]
        make("ts-mp2", "ts", [*mp2, "-f", "mpegts"])
        make("ts-mp2-offset", "ts", [*mp2, "-output_ts_offset", "3.25", "-f", "mpegts"])
        make("ts-mp2-programs", "ts", [*mp2, "-program", "program_num=7:st=0:st=1",
                                       "-metadata:p:0", "service_name=Video service",
                                       "-metadata:p:0", "service_provider=Summary generator", "-f", "mpegts"])
        make("ts-mp2-unassigned", "ts", [*mp2, "-program", "program_num=7:st=0", "-f", "mpegts"],
             side_data=(b"  No Program\n",), native=False)
        make("mkv-negative-start", "mkv", [*av, "-avoid_negative_ts", "disabled",
                                          "-output_ts_offset", "-0.2"], native=False)
        make("mkv-analysis-packet-cap", "mkv", [
            "-f", "lavfi", "-i", "testsrc2=size=64x48:rate=24:duration=2",
            "-f", "lavfi", "-i", "anullsrc=r=48000:cl=mono:d=0.5",
            "-map", "0:v", "-map", "1:a", "-vf", r"setpts=if(eq(N\,0)\,0\,PTS+0.35/TB)",
            "-af", "asetnsamples=n=1:p=0", "-fps_mode", "passthrough",
            "-c:v", "libvpx", "-threads:v", "1", "-c:a", "pcm_s16le",
        ], native=False, duration="2.4")
        capped = root / "mkv-analysis-packet-cap.mkv"
        capped.write_bytes(matroska_without_default_duration(capped.read_bytes()))
        make("mp4-rotation", "mp4", ["-display_rotation", "90", "-i", root / "mp4-av.mp4",
                                     "-map", "0", "-c", "copy"], side_data=(b"Display Matrix:",))
        make("mp4-fractional-rotation", "mp4", ["-display_rotation", "12.5", "-i", root / "mp4-av.mp4",
                                                "-map", "0", "-c", "copy"], side_data=(b"Display Matrix:",), native=False)
        make("mp4-hdr", "mp4", ["-mastering_display", "G(13250,34500)B(7500,3000)"
                                "R(34000,16000)WP(15635,16450)L(10000000,1)",
                                "-content_light", "1000,400", "-i", root / "mp4-av.mp4",
                                "-map", "0", "-c", "copy"],
             side_data=(b"Mastering display metadata:", b"Content light level metadata:"))
        make("mp4-hdr-rotation", "mp4", ["-display_rotation", "90", "-i", root / "mp4-hdr.mp4",
                                         "-map", "0", "-c", "copy"],
             side_data=(b"Display Matrix:", b"Mastering display metadata:",
                        b"Content light level metadata:"), structured_side_data=True)
        if audio:
            make("wav-pcm", "wav", [*sound, "-c:a", "pcm_s16le", "-metadata", "title=WAV title"], native=False)
            make("flac-audio", "flac", [*sound, "-c:a", "flac", "-metadata", "title=FLAC title",
                                       "-metadata", "comment=first line\nsecond line"])
        for fixture in fixtures:
            fixture.path.replace(directory / fixture.path.name)
    return [Fixture(fixture.name, directory / fixture.path.name, fixture.native,
                    fixture.structured_side_data, fixture.structured) for fixture in fixtures]


def flac_constant_frame(number, variable, samples, *, reserved=False, overlong=False, bits=16, bad_padding=False):
    coded = bytes((0xc0 | (number >> 6), 0x80 | (number & 63))) if overlong else bytes((number,))
    frame = bytes((0xff, 0xf8 | int(variable), 0x69, ({16: 4, 20: 5}[bits] << 1) | int(reserved)))
    frame += coded + bytes((samples - 1,))
    crc = 0
    for byte in frame:
        crc ^= byte
        for _ in range(8):
            crc = ((crc << 1) ^ (7 if crc & 0x80 else 0)) & 0xff
    frame += bytes((crc, 0)) + bytes((bits + 7) // 8)
    if bad_padding:
        frame = frame[:-1] + bytes((frame[-1] | 1,))
    crc = 0
    for byte in frame:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ (0x8005 if crc & 0x8000 else 0)) & 0xffff
    return frame + crc.to_bytes(2, "big")


def generate_audio_fixtures(oracle, directory):
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".input-summary-", dir=directory) as temporary:
        root = Path(temporary)
        try:
            paths = audio_probe.generate(oracle, root)
        except audio_probe.CheckFailure:
            raise CheckFailure("generate-standalone-audio") from None
        unqualified = {"wav-extensible-s24-valid20", "wav-extensible-s32-valid24",
                       "wav-truncated-data", "wav-partial-sample", "wav-rifx-s16", "wav-rifx-f32"}
        qualified_wave = {"wav-extensible-mono", "wav-extensible-stereo", "wav-extensible-3.0",
                          "wav-extensible-quad", "wav-extensible-5.1-back", "wav-extensible-5.1-side",
                          "wav-extensible-7.1", "wav-extensible-s32-valid32", "wav-extensible-f32-5.1"}
        for name, data in (
            ("wav-empty", audio_probe.wave(mask=3, frames=0)),
            ("wav-fact", audio_probe.wave(mask=3, before=audio_probe.chunk(b"fact", b"\0" * 4))),
            ("wav-language", audio_probe.wave(mask=3, before=audio_probe.chunk(
                b"LIST", b"INFO" + audio_probe.chunk(b"ILNG", b"eng\0")))),
        ):
            path = root / f"{name}.wav"
            path.write_bytes(data)
            paths.append(path)
            unqualified.add(name)
        base = root / "flac-s16-2ch-48000-minimal.flac"
        info, frames = audio_probe.split_flac(base.read_bytes())
        for name, fields in (
            ("flac-language", ("language=eng",)),
            ("flac-alias-collision", ("DESCRIPTION=first", "comment=second")),
            ("flac-invalid-layout", ("WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x4",)),
            ("flac-duplicate-layout", ("WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x3",) * 2),
        ):
            path = root / f"{name}.flac"
            path.write_bytes(audio_probe.flac(info, frames, ((4, audio_probe.comments(fields)),)))
            paths.append(path)
            unqualified.add(name)
        info, frames = audio_probe.split_flac((root / "flac-s24-6ch-48000-minimal.flac").read_bytes())
        for label, mask in (("back", "0x3f"), ("side", "0x60f")):
            path = root / f"flac-explicit-{label}.flac"
            fields = (f"WAVEFORMATEXTENSIBLE_CHANNEL_MASK={mask}",)
            path.write_bytes(audio_probe.flac(info, frames, ((4, audio_probe.comments(fields)),)))
            paths.append(path)
        original = (root / "flac-s16-2ch-32000-minimal.flac").read_bytes()
        info, frames = audio_probe.split_flac(original)
        bad_info = bytearray(info)
        bad_info[18] ^= 1
        prefix = original[:-len(frames)]
        short = (root / "flac-s16-2ch-48000-minimal.flac").read_bytes()
        _, short_frames = audio_probe.split_flac(short)
        for label, data in (
            ("flac-zero-payload", prefix + bytes(len(frames))),
            ("flac-single-zero-payload", short[:-len(short_frames)] + bytes(len(short_frames))),
            ("flac-bad-frame-crc", prefix + frames[:-1] + bytes([frames[-1] ^ 1])),
            ("flac-truncated-frame", original[:-1]),
            ("flac-trailing-bytes", original + b"invalid trailing bytes"),
            ("flac-bad-md5", audio_probe.flac(bytes(bad_info), frames)),
        ):
            path = root / f"{label}.flac"
            path.write_bytes(data)
            paths.append(path)
            unqualified.add(label)
        for label, sequence, native in (
            ("flac-fixed-one", ((0, False, 16),), True),
            ("flac-fixed-two", ((0, False, 16), (1, False, 16)), True),
            ("flac-fixed-short-tail", ((0, False, 16), (1, False, 8)), True),
            ("flac-variable-two", ((0, True, 16), (16, True, 16)), True),
            ("flac-fixed-offset", ((1, False, 16), (2, False, 16)), False),
            ("flac-variable-offset", ((16, True, 16), (32, True, 16)), False),
            ("flac-duplicate-number", ((0, False, 16), (0, False, 16)), False),
            ("flac-number-gap", ((0, False, 16), (2, False, 16)), False),
            ("flac-mixed-strategy", ((0, False, 16), (16, True, 16)), False),
            ("flac-reserved-header", ((0, False, 16), (1, False, 16)), False),
            ("flac-overlong-number", ((0, False, 16), (1, False, 16)), False),
            ("flac-20bit-valid", ((0, False, 16), (1, False, 16)), True),
            ("flac-20bit-padding", ((0, False, 16), (1, False, 16)), False),
        ):
            total = sum(samples for _, _, samples in sequence)
            bits = 20 if label.startswith("flac-20bit") else 16
            packed = (44100 << 44) | ((bits - 1) << 36) | total
            info = b"\0\x10\0\x10" + bytes(6) + packed.to_bytes(8, "big") + bytes(16)
            frames = b"".join(flac_constant_frame(*frame, reserved=label == "flac-reserved-header",
                              overlong=label == "flac-overlong-number", bits=bits,
                              bad_padding=label == "flac-20bit-padding") for frame in sequence)
            path = root / f"{label}.flac"
            path.write_bytes(audio_probe.flac(info, frames))
            paths.append(path)
            if not native:
                unqualified.add(label)
        fixtures = [Fixture(path.stem, directory / path.name,
                            path.stem not in unqualified and "-6ch-" not in path.stem
                            and (path.suffix != ".wav" or path.stem in qualified_wave)) for path in paths]
        for path in paths:
            path.replace(directory / path.name)
    return fixtures


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--coreav", required=True, type=Path)
    parser.add_argument("--oracle-dir", required=True, type=Path)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--audio", action="store_true", help="include standalone WAV and FLAC")
    parser.add_argument("--generate-only", action="store_true", help="do not invoke CoreAV yet")
    args = parser.parse_args(argv)
    try:
        oracle = Oracle(args.oracle_dir)
        fixtures = generate_fixtures(oracle, args.directory, audio=args.audio)
        if args.audio:
            fixtures.extend(generate_audio_fixtures(oracle, args.directory))
    except (OSError, ValueError):
        print("FAIL fixture-io", file=sys.stderr)
        return 1
    except CheckFailure as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    failures = []
    for fixture in fixtures:
        if not args.generate_only:
            if fixture.native:
                failures.extend(compare_file(args.coreav.resolve(), oracle, fixture.path, fixture.name))
            else:
                failures.extend(compare_unsupported(args.coreav.resolve(), oracle, fixture.path, fixture.name))
            if fixture.structured_side_data:
                failures.extend(compare_structured_side_data(args.coreav.resolve(), oracle,
                                                             fixture.path, fixture.name))
            if fixture.structured:
                failures.extend(compare_structured(args.coreav.resolve(), oracle, fixture.path, fixture.name))
        print(f"GENERATED {fixture.name}" if args.generate_only else f"CHECKED {fixture.name}")
    if not args.generate_only:
        failures.extend(compare_file(args.coreav.resolve(), oracle, fixtures[0].path,
                                     "mp4-file-url", file_url=True))
        failures.extend(compare_refusals(args.coreav.resolve(), fixtures[0].path, fixtures[0].name))
    for failure in failures:
        print(f"FAIL {failure}", file=sys.stderr)
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
