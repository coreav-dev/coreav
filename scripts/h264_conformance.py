#!/usr/bin/env python3
"""Run the H.264 conformance tests over the JVT suite, one stream at a time.

Bars C1 and C5 of docs/design/h264.md: every in-scope conformance bitstream
decodes bit-exactly, and the keyframe-only mode returns pictures that full
decoding also returns. The suite is about 1.6 GB of zips for the in-scope streams and
far more once extracted, so each stream is fetched (or taken from a cache of
zips), extracted, decoded and removed before the next one.

    python3 -I scripts/h264_conformance.py --zips /tmp/zips --work /tmp/work \\
        [--only NAME ...] [--report report.json] [--kernels scalar] [--mode safe]

Every zip is checked against the SHA-256 in the manifest
(crates/coreav-h264/tests/data/conformance-streams.json) before it is used.
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile

MANIFEST = "crates/coreav-h264/tests/data/conformance-streams.json"
BASE = "https://www.itu.int/wftp3/av-arch/jvt-site/draft_conformance"
# The two directories of the suite; a zip is in one of them.
SUBDIRS = ("AVCv1", "FRExt")


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(zips, stream):
    """The path of the stream's zip, downloaded if it is not there yet."""
    path = os.path.join(zips, stream["zip"])
    if os.path.exists(path) and digest(path) == stream["zip_sha256"]:
        return path
    os.makedirs(zips, exist_ok=True)
    last = None
    # The site answers with a short error page now and then, so every URL
    # is tried a few times and the result is only used once its SHA-256
    # matches the manifest.
    for attempt in range(3):
        for sub in SUBDIRS:
            url = f"{BASE}/{sub}/{stream['zip']}"
            try:
                with urllib.request.urlopen(url, timeout=300) as r, open(path, "wb") as out:
                    shutil.copyfileobj(r, out)
            except Exception as e:  # noqa: BLE001 - any failure tries the next
                last = e
                continue
            got = digest(path)
            if got == stream["zip_sha256"]:
                return path
            last = ValueError(f"{url}: sha256 {got}, expected {stream['zip_sha256']}")
        time.sleep(5 * (attempt + 1))
    raise SystemExit(f"{stream['name']}: could not fetch its zip ({last})")


def extract(zip_path, into):
    """Extracts the zip into `into`, refusing paths that escape it."""
    os.makedirs(into, exist_ok=True)
    with zipfile.ZipFile(zip_path) as z:
        for member in z.namelist():
            target = os.path.realpath(os.path.join(into, member))
            if not target.startswith(os.path.realpath(into) + os.sep):
                raise SystemExit(f"{zip_path}: a member escapes the directory: {member}")
        z.extractall(into)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--zips", help="where the zips are kept")
    ap.add_argument("--work", help="where a stream is extracted")
    ap.add_argument(
        "--extracted",
        help="a directory that already holds the suite, which is then neither fetched nor extracted",
    )
    ap.add_argument("--manifest", default=MANIFEST)
    ap.add_argument("--only", nargs="*", default=[])
    ap.add_argument("--report")
    ap.add_argument("--kernels", choices=["scalar", "detect"], default="detect")
    ap.add_argument(
        "--mode",
        choices=["fast", "safe"],
        default="fast",
        help="safe builds the decoder without its feature `fast`, which compiles no "
        "`unsafe`",
    )
    ap.add_argument(
        "--threads",
        default="1",
        help="comma-separated thread counts; bar C1 decodes each stream once "
        "with each (bar G1 of docs/design/h264.md, step 7)",
    )
    ap.add_argument("--keep", action="store_true", help="keep each extraction")
    args = ap.parse_args()

    manifest = json.load(open(args.manifest))
    streams = [s for s in manifest["streams"] if s["in_scope"]]
    if args.only:
        streams = [s for s in streams if s["name"] in args.only]
    if not streams:
        raise SystemExit("no streams selected")
    if not args.extracted and not (args.zips and args.work):
        raise SystemExit("either --extracted, or both --zips and --work")

    features = ["--no-default-features"] if args.mode == "safe" else []
    # One build, then one run per stream.
    build = subprocess.run(
        ["cargo", "test", "-p", "coreav-h264", "--release", *features, "--test", "conformance", "--no-run"],
        check=False,
    )
    if build.returncode != 0:
        raise SystemExit("the conformance test did not build")

    results = []
    failures = 0
    started = time.time()
    for stream in streams:
        name = stream["name"]
        if args.extracted:
            into = args.extracted
        else:
            zip_path = fetch(args.zips, stream)
            into = os.path.join(args.work, name)
            shutil.rmtree(into, ignore_errors=True)
            extract(zip_path, into)
        env = dict(os.environ)
        env["COREAV_H264_CONFORMANCE"] = into
        env["COREAV_H264_ONLY"] = name
        if args.kernels == "scalar":
            env["COREAV_H264_KERNELS"] = "scalar"
        env["COREAV_H264_THREADS"] = args.threads
        # Bar C1 first, then bar C5 over the same extraction.
        outputs = []
        bars = {}
        passed = True
        for bar, test in (
            ("C1", "every_in_scope_stream_is_bit_exact"),
            ("C5", "the_keyframe_mode_returns_what_full_decoding_returns"),
        ):
            run = subprocess.run(
                [
                    "cargo", "test", "-p", "coreav-h264", "--release", *features,
                    "--test", "conformance", "--",
                    "--ignored", "--nocapture", "--exact", test,
                ],
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            outputs.append(run)
            # What the test said about this stream, for this bar.
            bars[bar] = next(
                (l for l in run.stdout.splitlines() if l.startswith(("pass ", "FAIL "))),
                (run.stdout.strip().splitlines() or [""])[-1],
            )
            if run.returncode != 0:
                bars[bar] = "FAIL " + bars[bar].removeprefix("pass ").removeprefix("FAIL ")
                passed = False
                break
        run = outputs[-1]
        # Every bar that ran, so that a failing one is never reported as
        # another bar's pass.
        detail = "; ".join(f"{bar}: {line}" for bar, line in bars.items())
        print(("pass " if passed else "FAIL ") + f"{name} ({detail})")
        sys.stdout.flush()
        results.append({"name": name, "passed": passed, "detail": detail, "bars": bars})
        if not passed:
            failures += 1
            print(run.stdout[-4000:])
        if not args.keep and not args.extracted:
            shutil.rmtree(into, ignore_errors=True)

    summary = {
        "streams": len(results),
        "passed": len(results) - failures,
        "failed": failures,
        "kernels": args.kernels,
        "mode": args.mode,
        "threads": args.threads,
        "seconds": round(time.time() - started, 1),
        "results": results,
    }
    print(json.dumps({k: v for k, v in summary.items() if k != "results"}))
    if args.report:
        with open(args.report, "w") as f:
            json.dump(summary, f, indent=1)
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as f:
            f.write(
                f"### H.264 conformance: {summary['passed']} of {summary['streams']} "
                f"streams pass bars C1 and C5 ({summary['kernels']} kernels, "
                f"{summary['mode']} mode, "
                f"threads {summary['threads']}, "
                f"{summary['seconds']} s)\n"
            )
            for r in results:
                if not r["passed"]:
                    f.write(f"- FAIL {r['name']}: {r['detail']}\n")
    raise SystemExit(1 if failures else 0)


if __name__ == "__main__":
    main()
