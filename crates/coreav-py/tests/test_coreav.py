"""Tests of the Python module `coreav` (docs/design/python.md).

Against the generated fixtures (COREAV_FIXTURES, from scripts/gen-fixtures.sh):
every function on every fixture, on its path and on its bytes. With
COREAV_CLI (the `coreav` binary built from the same commit), every result is
compared with the command's: the same JSON document, the same output bytes,
and an exception whose class matches the command's exit code and whose
`error` is the command's `coreav.error/1` object. Without fixtures, those
tests are skipped; the others need nothing.
"""

import json
import os
import pathlib
import subprocess
import sys
import sysconfig
import tempfile
import threading

import pytest

import coreav

FIXTURES = os.environ.get("COREAV_FIXTURES")
CLI = os.environ.get("COREAV_CLI")
ROOT = pathlib.Path(__file__).resolve().parents[3]
POLICIES = sorted((ROOT / "docs" / "schema" / "policy-examples").glob("*.json"))
EXCEPTIONS = {1: coreav.RejectedError, 2: coreav.UsageError, 3: coreav.IoError}


def fixtures(*suffixes):
    if not FIXTURES:
        return []
    names = [line.split(" ", 1)[1] for line in (pathlib.Path(FIXTURES) / "MANIFEST").read_text().splitlines()
             if line.startswith("generated ")]
    return [pathlib.Path(FIXTURES) / n for n in names if n.endswith(suffixes)]


ISO = fixtures(".mp4", ".mov", ".m4a")
ALL = fixtures(".mp4", ".mov", ".m4a", ".mkv", ".webm")
needs_fixtures = pytest.mark.skipif(not FIXTURES, reason="set COREAV_FIXTURES")
needs_cli = pytest.mark.skipif(not (FIXTURES and CLI), reason="set COREAV_FIXTURES and COREAV_CLI")


def call(function, *args, **kwargs):
    """('ok', result) or ('error', exception)."""
    try:
        return "ok", function(*args, **kwargs)
    except coreav.CoreavError as error:
        return "error", error


def cli(*args):
    """('ok', stdout JSON, written bytes) or ('error', exit code, stderr
    JSON). Writing commands write to a temporary file."""
    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp) / "out"
        args = [str(a) if a != "OUT" else str(out) for a in args]
        result = subprocess.run([CLI, *args], capture_output=True, check=False)
        if result.returncode == 0:
            return "ok", json.loads(result.stdout), out.read_bytes() if out.exists() else None
        try:
            error = json.loads(result.stderr)
        except ValueError:
            error = None
        return "error", result.returncode, error


def same(python, command, output=False):
    """Asserts that a Python result and a command result agree."""
    if command[0] == "ok":
        assert python[0] == "ok", python[1]
        if output:
            data, summary = python[1]
            assert summary == command[1]
            assert data == command[2]
        else:
            assert python[1] == command[1]
    else:
        assert python[0] == "error", f"the command failed with {command[1]}"
        assert isinstance(python[1], EXCEPTIONS[command[1]]), (python[1], command[1])
        if command[2] is not None:
            assert python[1].error == command[2]


# ---------------------------------------------------------------- the CLI


@needs_cli
@pytest.mark.parametrize("path", ALL, ids=lambda p: p.name)
def test_probe_matches_the_command(path):
    command = cli("probe", path)
    same(call(coreav.probe, path), command)
    same(call(coreav.probe, str(path), scan_fragments=True), cli("probe", "--scan-fragments", path))
    data = call(coreav.probe, path.read_bytes())
    if command[0] == "ok":
        # The document holds no file name: bytes give the same one.
        assert data == ("ok", command[1])


@needs_cli
@pytest.mark.parametrize("path", ISO, ids=lambda p: p.name)
def test_writing_commands_match_the_command(path):
    data = path.read_bytes()
    cases = [
        (coreav.extract_audio, {}, ["extract-audio"]),
        (coreav.clip, {"start": "0.5", "end": 1.5}, ["clip", "--start", "0.5", "--end", "1.5"]),
        (coreav.remux, {}, ["remux"]),
        (coreav.extract_subtitles, {}, ["extract-subtitles"]),
    ]
    for function, kwargs, args in cases:
        command = cli(*args, path, "OUT")
        same(call(function, path, **kwargs), command, output=True)
        same(call(function, data, **kwargs), command, output=True)


@needs_cli
@pytest.mark.parametrize("path", fixtures(".mkv", ".webm"), ids=lambda p: p.name)
def test_matroska_remux_matches_the_command(path):
    for kwargs, args in [({}, []), ({"drop_unsupported": True}, ["--drop-unsupported"]),
                         ({"fail_on_unsupported": True}, ["--fail-on-unsupported"])]:
        command = cli("remux", *args, path, "OUT")
        same(call(coreav.remux, path, **kwargs), command, output=True)
        same(call(coreav.remux, path.read_bytes(), **kwargs), command, output=True)


# Each option against its command-line flag, on every fixture.
OPTIONS = [
    (coreav.probe, {"allow": ["h264", "vp9"]}, ["probe", "--allow", "h264,vp9"], False),
    (coreav.probe, {"allow": ["h264"], "strict": True}, ["probe", "--allow", "h264", "--strict"], False),
    (coreav.probe, {"scan_frame_sizes": True}, ["probe", "--scan-frame-sizes"], False),
    (coreav.remux, {"streams": [0]}, ["remux", "--streams", "0"], True),
    (coreav.remux, {"fail_on_unsupported": True}, ["remux", "--fail-on-unsupported"], True),
    (coreav.remux, {"allow": ["h264"], "strict": True}, ["remux", "--allow", "h264", "--strict"], True),
    (coreav.clip, {"start": 0, "audio": None}, ["clip", "--start", "0", "--audio", "none"], True),
    (coreav.clip, {"start": 0.5, "video": 0, "audio": 1}, ["clip", "--start", "0.5", "--video", "0",
                                                          "--audio", "1"], True),
    (coreav.extract_audio, {"stream": 1, "allow": ["aac"]}, ["extract-audio", "--stream", "1",
                                                             "--allow", "aac"], True),
    (coreav.extract_subtitles, {"stream": 2}, ["extract-subtitles", "--stream", "2"], True),
    (coreav.sanitize, {}, ["sanitize"], True),
    (coreav.sanitize, {"keep": []}, ["sanitize", "--keep", "none"], True),
    (coreav.sanitize, {"keep": ["dates", "rotation"], "streams": [0]},
     ["sanitize", "--keep", "dates,rotation", "--streams", "0"], True),
]


@needs_cli
@pytest.mark.parametrize("function, kwargs, args, writes", OPTIONS,
                         ids=[" ".join(o[2]) for o in OPTIONS])
def test_options_match_the_command(function, kwargs, args, writes):
    for path in ALL:
        if function not in (coreav.probe, coreav.remux, coreav.sanitize) and path.suffix in (".mkv", ".webm"):
            continue
        command = cli(*args, path, "OUT") if writes else cli(*args, path)
        same(call(function, path, **kwargs), command, output=writes)


@needs_cli
@pytest.mark.parametrize("path", ISO, ids=lambda p: p.name)
def test_chapters_match_the_command(path):
    command = cli("chapters", path)
    same(call(coreav.chapters, path), command)
    same(call(coreav.chapters, path.read_bytes()), command)


@needs_cli
@pytest.mark.parametrize("policy", POLICIES, ids=lambda p: p.name)
def test_check_matches_the_command(policy):
    text = policy.read_text()
    for path in ISO:
        result = subprocess.run([CLI, "check", "--policy", policy, path], capture_output=True, check=False)
        assert result.returncode in (0, 1), result.stderr
        verdict = json.loads(result.stdout)
        assert coreav.check(path, json.loads(text)) == verdict
        assert coreav.check(path.read_bytes(), text) == verdict
        assert (verdict["verdict"] == "accept") == (result.returncode == 0)


# ------------------------------------------------------- without the CLI


@needs_fixtures
@pytest.mark.parametrize("path", ALL, ids=lambda p: p.name)
def test_every_fixture_gives_a_document_or_a_structured_error(path):
    for source in (path, path.read_bytes()):
        status, result = call(coreav.probe, source)
        if status == "ok":
            assert result["schema"] in ("coreav.probe/1", "coreav.probe-matroska/1")
        else:
            assert result.error["schema"] == "coreav.error/1"
            assert result.kind == result.error["kind"]


@needs_fixtures
def test_outputs_are_written_to_files(tmp_path):
    path = next(p for p in ISO if p.name == "basic.mp4")
    out = tmp_path / "audio.m4a"
    summary = coreav.extract_audio(path, out)
    assert out.stat().st_size == summary["output"]["bytes"]
    assert coreav.probe(out)["streams"][0]["codec"] == "aac"
    # An existing output needs force; the input is never an output.
    with pytest.raises(coreav.UsageError, match="force=True"):
        coreav.extract_audio(path, out)
    assert coreav.extract_audio(path, out, force=True) == summary
    with pytest.raises(coreav.UsageError):
        coreav.remux(path, path, force=True)
    # A 255-byte name, and no temporary file left behind.
    long = tmp_path / ("a" * 251 + ".mp4")
    coreav.remux(path, long)
    assert sorted(p.name for p in tmp_path.iterdir()) == sorted(["audio.m4a", long.name])


def test_errors_are_structured(tmp_path):
    with pytest.raises(coreav.RejectedError) as raised:
        coreav.probe(b"not a movie")
    error = raised.value
    assert isinstance(error, coreav.CoreavError)
    assert error.error["schema"] == "coreav.error/1"
    assert error.kind == "unsupported_container" and error.limit is None and error.offset is None
    assert str(error) == error.error["message"]
    with pytest.raises(coreav.IoError) as raised:
        coreav.probe(tmp_path / "missing.mp4")
    assert raised.value.kind == "io"
    with pytest.raises(coreav.UsageError):
        coreav.probe(b"", allow=["no-such-codec"])
    for start in ("1.5s", "-1", True, [1]):
        with pytest.raises(coreav.UsageError):
            coreav.clip(b"", start=start)
    with pytest.raises(coreav.UsageError):
        coreav.clip(b"", start=0, audio="second")
    with pytest.raises(coreav.UsageError):
        coreav.remux(b"", fail_on_unsupported=True, drop_unsupported=True)
    with pytest.raises(coreav.UsageError):
        coreav.sanitize(b"", keep=["everything"])
    with pytest.raises(coreav.UsageError):
        coreav.check(b"", "not json")
    with pytest.raises(coreav.UsageError):
        coreav.check(b"", {"schema": "coreav.policy/1", "streams": {"video": {"codecs": [object()]}}})
    with pytest.raises(TypeError):
        coreav.probe(12)
    # Indexes are ints of at least 0, not bools.
    for index in (True, False, -1, "1"):
        with pytest.raises(coreav.UsageError):
            coreav.extract_audio(b"", stream=index)
        with pytest.raises(coreav.UsageError):
            coreav.clip(b"", start=0, audio=index)
    with pytest.raises(coreav.UsageError):
        coreav.remux(b"", streams=[0, True])
    for start in (-1, float("nan"), float("inf")):
        with pytest.raises(coreav.UsageError):
            coreav.clip(b"", start=start)
    # A dict policy has the same size limit as JSON text.
    with pytest.raises(coreav.UsageError):
        coreav.check(b"", {"schema": "coreav.policy/1", "name": "x" * (1 << 21)})
    with pytest.raises(coreav.UsageError):
        coreav.check(b"", "\ud800")


def test_times_are_decimal_seconds():
    class Seconds(float):
        def __repr__(self):
            return f"Seconds({float(self)})"

    # A float subclass (as NumPy's) and computed floats are read as their
    # value, rounded to nanoseconds; the junk source is rejected after that.
    for start in (0.1 + 0.2, 1 / 3, Seconds(0.5), 12, "12.5"):
        with pytest.raises(coreav.RejectedError):
            coreav.clip(b"junk", start=start)


def test_bytes_like_sources():
    for source in (b"x", bytearray(b"x")):
        with pytest.raises(coreav.RejectedError):
            coreav.probe(source)


@needs_fixtures
def test_threads_run_in_parallel():
    sources = [p.read_bytes() for p in ISO]
    expected = [call(coreav.probe, s)[0] for s in sources]
    results = [None] * 8

    def work(slot):
        results[slot] = [call(coreav.probe, s)[0] for s in sources]

    threads = [threading.Thread(target=work, args=(i,)) for i in range(len(results))]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert all(r == expected for r in results)


# ------------------------------------------------- the `coreav` command


def installed_command():
    """The `coreav` console script installed with the wheel."""
    path = pathlib.Path(sysconfig.get_path("scripts")) / ("coreav.exe" if os.name == "nt" else "coreav")
    return path if path.exists() else None


COMMAND = installed_command()
needs_command = pytest.mark.skipif(COMMAND is None, reason="the coreav script is not installed")


@needs_command
def test_ctrl_c_stops_the_command(tmp_path):
    import signal
    import time
    fifo = tmp_path / "fifo"
    os.mkfifo(fifo)
    process = subprocess.Popen([COMMAND, "probe", fifo], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(0.5)  # blocked opening the FIFO, which has no writer
    process.send_signal(signal.SIGINT)
    assert process.wait(timeout=5) == -signal.SIGINT


@needs_command
def test_the_command_runs(tmp_path):
    version = subprocess.run([COMMAND, "--version"], capture_output=True, check=True, text=True)
    assert version.stdout.strip() == f"coreav {coreav.__version__}"
    junk = tmp_path / "junk.mp4"
    junk.write_bytes(b"not a movie")
    for args, code in [(["probe", junk], 1), (["probe", "--allow", "bogus", junk], 2),
                       (["probe", tmp_path / "missing.mp4"], 3), (["no-such-command"], 2)]:
        result = subprocess.run([COMMAND, *args], capture_output=True, check=False)
        assert result.returncode == code, (args, result.stderr)
    error = json.loads(subprocess.run([COMMAND, "probe", junk], capture_output=True).stderr)
    assert error["schema"] == "coreav.error/1"


@needs_command
@pytest.mark.skipif(sys.platform == "win32", reason="POSIX pipes and /dev/full")
def test_the_command_reports_standard_output_errors():
    """A closed standard output exits 141 without a message, as the binary
    does; a full one (Linux) exits 3 with the error document."""
    import socket
    ours, theirs = socket.socketpair()
    theirs.close()
    with ours:
        result = subprocess.run([COMMAND, "--version"], stdout=ours, stderr=subprocess.PIPE, check=False)
    assert (result.returncode, result.stderr) == (141, b"")
    if os.path.exists("/dev/full"):
        with open("/dev/full", "wb") as full:
            result = subprocess.run([COMMAND, "--version"], stdout=full, stderr=subprocess.PIPE, check=False)
        assert result.returncode == 3
        assert json.loads(result.stderr)["kind"] == "io"


@needs_command
@needs_cli
@pytest.mark.parametrize("path", ALL, ids=lambda p: p.name)
def test_the_command_matches_the_binary(path):
    for args in (["probe", path], ["extract-audio", path, "-"], ["remux", path, "-"]):
        ours = subprocess.run([COMMAND, *args], capture_output=True, check=False)
        theirs = subprocess.run([CLI, *args], capture_output=True, check=False)
        assert (ours.returncode, ours.stdout, ours.stderr) == (theirs.returncode, theirs.stdout, theirs.stderr)


def _range_server(data):
    import http.server
    import re as _re

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self):
            spec = self.headers.get("Range", "")
            suffix = _re.fullmatch(r"bytes=-(\d+)", spec)
            if suffix is not None:
                n = int(suffix.group(1))
                spec = f"bytes={max(0, len(data) - n)}-{len(data) - 1}"
            match = _re.fullmatch(r"bytes=(\d+)-(\d+)", spec)
            if match is None:
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
            start, end = int(match.group(1)), min(int(match.group(2)), len(data) - 1)
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("ETag", '"v1"')
            self.end_headers()
            self.wfile.write(data[start:end + 1])

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


@needs_fixtures
def test_urls_are_read_by_byte_ranges():
    """A URL source gives what the file gives; private addresses need
    private_network=True (docs/design/remote.md)."""
    path = next(p for p in ALL if p.suffix == ".mp4")
    data = path.read_bytes()
    server = _range_server(data)
    try:
        url = f"http://127.0.0.1:{server.server_address[1]}/{path.name}"
        with pytest.raises(coreav.IoError) as refused:
            coreav.probe(url)
        assert "https" in str(refused.value)
        with pytest.raises(coreav.IoError) as refused:
            coreav.probe(url.replace("http://", "https://"))
        assert "not a public address" in str(refused.value)
        local = coreav.probe(path)
        remote = coreav.probe(url, private_network=True)
        local.pop("file", None)
        remote.pop("file", None)
        assert remote == local
        assert coreav.remux(url, private_network=True) == coreav.remux(path)
        assert coreav.chapters(url, private_network=True) == coreav.chapters(path)
    finally:
        server.shutdown()

