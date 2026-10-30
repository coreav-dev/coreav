#!/usr/bin/env python3
"""Upload a fuzz run's findings to the private bucket.

scripts/fuzz_run.sh leaves each finding in FINDINGS/NAME: the target's own
output (the stack trace among it) and its libFuzzer artifacts (the input).
Neither may reach a public log or artifact (docs/design/fuzz-campaign.md,
"Findings"), so each NAME goes up as one tar.gz, PREFIX/NAME.tar.gz, and
only NAME and that key are printed.

Usage:
  fuzz_upload.py FINDINGS PREFIX

Credentials from R2_ENDPOINT, R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY, the
bucket from R2_BUCKET. A failed upload is tried again, and the other
findings still go up; exits 1 when there is a finding it could not upload.
"""

import io
import os
import pathlib
import sys
import tarfile

ATTEMPTS = 3


def main():
    findings, prefix = pathlib.Path(sys.argv[1]), sys.argv[2].rstrip("/")
    names = sorted(p for p in findings.iterdir() if p.is_dir()) if findings.is_dir() else []
    if not names:
        print("no findings")
        return 0
    env = {k: os.environ.get(k) for k in ("R2_ENDPOINT", "R2_ACCESS_KEY_ID", "R2_SECRET_ACCESS_KEY", "R2_BUCKET")}
    missing = sorted(k for k, v in env.items() if not v)
    if missing:
        print(f"::error::{len(names)} findings not kept: {', '.join(missing)} not set")
        return 1
    import boto3

    s3 = boto3.client(
        "s3",
        endpoint_url=env["R2_ENDPOINT"],
        aws_access_key_id=env["R2_ACCESS_KEY_ID"],
        aws_secret_access_key=env["R2_SECRET_ACCESS_KEY"],
        region_name="auto",
    )
    lost = []
    for path in names:
        body = io.BytesIO()
        with tarfile.open(fileobj=body, mode="w:gz") as tar:
            tar.add(path, arcname=path.name)
        key = f"{prefix}/{path.name}.tar.gz"
        for attempt in range(ATTEMPTS):
            try:
                s3.put_object(Bucket=env["R2_BUCKET"], Key=key, Body=body.getvalue())
                print(f"{path.name}: {key}")
                break
            except Exception as error:
                # The class only: a message could quote the request.
                failure = type(error).__name__
        else:
            print(f"::error::{path.name}: not kept, {ATTEMPTS} uploads failed ({failure})")
            lost.append(path.name)
    return 1 if lost else 0


if __name__ == "__main__":
    sys.exit(main())
