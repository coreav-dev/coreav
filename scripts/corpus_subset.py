#!/usr/bin/env python3
"""Downloads the corpus v1 files that carry given coverage tags.

Selects the entries of corpus v1's manifest whose
`tags` hold every tag of --all and at least one tag of --any, and downloads each object (its
`key` in the R2 bucket) to OUT/<sha256>.<ext>, skipping files already there
with the right size. Writes OUT/selection.json with the selected entries.
R2 credentials come from R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and
R2_ENDPOINT, and are never printed.

Usage:
  corpus_subset.py OUT --all c:isobmff --any subtitle-track chapters
"""

import argparse
import json
import pathlib
import sys
from concurrent.futures import ThreadPoolExecutor

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "corpus" / "v1" / "scripts"))

import common  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("out")
    parser.add_argument("--all", nargs="*", default=[])
    parser.add_argument("--any", nargs="*", default=[])
    parser.add_argument("--jobs", type=int, default=8)
    args = parser.parse_args()

    manifest = json.loads((ROOT / "corpus" / "v1" / "manifest.json").read_text())
    entries = manifest if isinstance(manifest, list) else manifest.get("files", manifest.get("entries"))
    selected = [e for e in entries
                if all(t in e["tags"] for t in args.all) and (not args.any or any(t in e["tags"] for t in args.any))]
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "selection.json").write_text(json.dumps(selected, indent=1) + "\n")

    def fetch(entry):
        dest = out / f"{entry['sha256']}.{entry['ext']}"
        if dest.exists() and dest.stat().st_size == entry["size"]:
            return 0
        part = dest.with_suffix(dest.suffix + ".part")
        common.s3().download_file(common.BUCKET, entry["key"], str(part))
        part.rename(dest)
        return entry["size"]

    with ThreadPoolExecutor(args.jobs) as pool:
        fetched = sum(pool.map(fetch, selected))
    print(f"{len(selected)} files selected, {fetched / 1e9:.2f} GB downloaded to {out}")


if __name__ == "__main__":
    main()
