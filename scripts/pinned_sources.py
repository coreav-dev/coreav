#!/usr/bin/env python3
"""Source releases pinned by URL and SHA-256, fetched and unpacked for the
scripts that read them by script, never by a person: `scripts/opus_tables.py`
generates the Opus decoder's tables from the code of RFC 6716 Appendix A,
with RFC 8251's changes, through this module.

A pinned release is a dict: `url` and `sha256` of a tarball, and `include`,
the pattern of the member paths (without the tarball's top directory) to
extract. An RFC's text that carries its code as a base64 gzipped tarball,
on the lines that start with `   ###`, adds `embedded_sha1`, the SHA-1 the
RFC states for it; a patch to apply to the members adds `patch_url`,
`patch_sha256` and `hunks`, how many of its hunks touch the selection.

Imported by path (`importlib`), as scripts run with `python3 -I` do not
see their own directory. Python 3.11 standard library only.
"""
import base64
import hashlib
import io
import lzma
import pathlib
import re
import sys
import tarfile
import urllib.request

# The code of RFC 6716 Appendix A, the Opus specification, with the changes
# of RFC 8251, whose patch file that RFC names by its SHA-1 (`patch_sha256`
# pins it too).
RFC6716 = {
    "version": "RFC 6716 with RFC 8251",
    "license": "BSD-3-Clause (IETF Trust)",
    "url": "https://www.rfc-editor.org/rfc/rfc6716.txt",
    "sha256": "41caac5240a4a22661efd0031d5b7aee48f3c0bde3b2cdcee8165932e485f98c",
    "embedded_sha1": "86a927223e73d2476646a1b933fcd3fffb6ecc8c",
    "patch_url": "https://www.ietf.org/proceedings/98/slides/materials-98-codec-opus-update-00.patch",
    "patch_sha256": "2b06d9a981b1feeb98348d981c1e326658f842fff9abc0cf6afbd62a916b1f02",
    "hunks": 9,
    "include": r"\A(?:celt|silk|src|include)/.*\.(?:c|h)\Z",
}

# Where the fetched files are kept between runs.
CACHE = "~/.cache/coreav-pinned-sources"


def fetch(name, url, sha256, cache):
    """A pinned file's bytes, from the cache or the network, checked against
    its SHA-256."""
    path = pathlib.Path(cache) / f"{name}-{sha256[:16]}-{pathlib.PurePosixPath(url).name}"
    data = path.read_bytes() if path.is_file() else None
    if data is None or hashlib.sha256(data).hexdigest() != sha256:
        req = urllib.request.Request(url, headers={"User-Agent": "coreav-pinned-sources"})
        try:
            with urllib.request.urlopen(req, timeout=300) as r:
                data = r.read()
        except OSError as e:
            print(f"error: {name}: cannot fetch {url}: {e}", file=sys.stderr)
            sys.exit(2)
        got = hashlib.sha256(data).hexdigest()
        if got != sha256:
            print(f"error: {name}: {url}: SHA-256 {got}, pinned {sha256}", file=sys.stderr)
            sys.exit(2)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    return data


def rfc_archive(name, spec, text):
    """The gzipped tarball that an RFC's text carries in base64 on its lines
    that start with `   ###`, checked against the SHA-1 the RFC states."""
    b64 = "".join(line[6:] for line in text.decode("ascii", errors="replace").split("\n") if line.startswith("   ###"))
    try:
        data = base64.b64decode(b64, validate=True)
    except ValueError as e:
        print(f"error: {name}: the RFC's base64 does not decode: {e}", file=sys.stderr)
        sys.exit(2)
    got = hashlib.sha1(data).hexdigest()
    if got != spec["embedded_sha1"]:
        print(f"error: {name}: embedded archive SHA-1 {got}, the RFC states {spec['embedded_sha1']}", file=sys.stderr)
        sys.exit(2)
    return data


HUNK_RE = re.compile(r"@@ -(\d+)(?:,(\d+))? \+\d+(?:,(\d+))? @@")


def patch_hunks(patch):
    """(path, first old line, old lines, new lines) of every hunk of a
    unified diff, paths without git's `b/`. A file's header is its `---`
    and `+++` lines; each hunk takes as many lines as its `@@` line counts,
    a context line that lost its leading space included."""
    out = []
    lines = patch.split("\n")
    path = None
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        if line.startswith("--- ") and i < len(lines) and lines[i].startswith("+++ "):
            path = lines[i][4:].split("\t")[0]
            path = path[2:] if path.startswith("b/") else path
            i += 1
            continue
        m = HUNK_RE.match(line)
        if m is None or path is None:
            continue
        old_left = int(m.group(2) or 1)
        new_left = int(m.group(3) or 1)
        hunk = (path, int(m.group(1)), [], [])
        while (old_left or new_left) and i < len(lines):
            line = lines[i]
            i += 1
            if line.startswith("\\"):
                continue
            kind, body = (line[:1], line[1:]) if line else (" ", "")
            if kind not in (" ", "-", "+"):
                break
            if kind != "+":
                hunk[2].append(body)
                old_left -= 1
            if kind != "-":
                hunk[3].append(body)
                new_left -= 1
        if old_left or new_left:
            print("error: a hunk of the patch is cut short", file=sys.stderr)
            sys.exit(2)
        out.append(hunk)
    return out


def apply_patch(name, files, patch):
    """`files` (path to text) with the hunks of `patch` that touch them
    applied, in order, each where its old lines are found nearest the line
    it names (shifted by what the file's earlier hunks moved); and the
    number of hunks applied. A hunk whose old lines are not found stops the
    run (exit 2)."""
    out = dict(files)
    applied = 0
    shift = {}
    for path, first, old, new in patch_hunks(patch):
        if path not in out:
            continue
        lines = out[path].split("\n")
        want = first - 1 + shift.get(path, 0)
        at = next((p for d in range(len(lines)) for p in (want - d, want + d)
                   if 0 <= p <= len(lines) - len(old) and lines[p : p + len(old)] == old), None)
        if at is None:
            print(f"error: {name}: a hunk of the patch does not apply", file=sys.stderr)
            sys.exit(2)
        out[path] = "\n".join(lines[:at] + new + lines[at + len(old) :])
        # `at` already holds the earlier hunks' shift.
        shift[path] = at - (first - 1) + len(new) - len(old)
        applied += 1
    return out, applied


def members(name, spec, cache, problems):
    """(path inside the release, text) of every member the comparison set
    includes, with the set's patch applied. Paths are used for selection
    only and never printed."""
    data = fetch(name, spec["url"], spec["sha256"], cache)
    if "embedded_sha1" in spec:
        data = rfc_archive(name, spec, data)
    include = re.compile(spec["include"])
    raw = lzma.decompress(data) if spec["url"].endswith(".xz") else data
    mode = "r:" if spec["url"].endswith(".xz") else "r:gz"
    out = []
    with tarfile.open(fileobj=io.BytesIO(raw), mode=mode) as t:
        for m in t.getmembers():
            if not m.isfile():
                continue
            rel = m.name.split("/", 1)[1] if "/" in m.name else m.name
            if not include.search(rel):
                continue
            f = t.extractfile(m)
            if f is None:
                continue
            out.append((rel, f.read().decode("utf-8", errors="replace")))
    if "patch_url" in spec:
        patch = fetch(name, spec["patch_url"], spec["patch_sha256"], cache).decode("utf-8", errors="replace")
        patched, applied = apply_patch(name, dict(out), patch)
        if applied != spec["hunks"]:
            problems.append(f"{name} {spec['version']}: {applied} hunks of the patch applied, pinned {spec['hunks']}")
        out = [(rel, patched[rel]) for rel, _ in out]
    return out
