#!/usr/bin/env python3
"""Generate crates/coreav-vp9/src/tables.rs from the VP9 specification PDF.

Each table is read from the specification's own text, as pdftotext
(poppler) lays it out: the table's definition in the C-like notation of the
specification, its dimensions resolved with the constants of clause 3, and
its braces checked to nest exactly as those dimensions say. No table is
typed by hand, and nothing comes from another implementation.

    python3 -I crates/coreav-vp9/tools/gen_tables.py \\
        --pdf vp9-bitstream-specification-v0.7-20170222-draft.pdf \\
        --out crates/coreav-vp9/src/tables.rs [--check]

The PDF is https://storage.googleapis.com/downloads.webmproject.org/docs/vp9/vp9-bitstream-specification-v0.7-20170222-draft.pdf;
it is not committed. With --check the file is compared, not written, and
any difference fails (.github/workflows/vp9-conformance.yml).
"""
import argparse
import hashlib
import re
import subprocess
import sys

PDF_SHA256 = "92a5bc772d11572c2f119680998ee202e249d075469515fe43bbb7febb51b963"

# The Rust name of a table type too complex to spell out at each use.
ALIASES = {"default_coef_probs": "CoefProbs"}

# (name in the specification, Rust constant, clause, what it holds)
TABLES = [
    ("inv_map_table", "INV_MAP_TABLE", "6.3.5", "The probability deltas of `inv_remap_prob`, by `deltaProb`."),
    ("default_partition_probs", "DEFAULT_PARTITION_PROBS", "10.5", "Default `partition_probs`."),
    ("default_y_mode_probs", "DEFAULT_Y_MODE_PROBS", "10.5", "Default `y_mode_probs`."),
    ("default_uv_mode_probs", "DEFAULT_UV_MODE_PROBS", "10.5", "Default `uv_mode_probs`."),
    ("default_skip_prob", "DEFAULT_SKIP_PROB", "10.5", "Default `skip_prob`."),
    ("default_is_inter_prob", "DEFAULT_IS_INTER_PROB", "10.5", "Default `is_inter_prob`."),
    ("default_comp_mode_prob", "DEFAULT_COMP_MODE_PROB", "10.5", "Default `comp_mode_prob`."),
    ("default_comp_ref_prob", "DEFAULT_COMP_REF_PROB", "10.5", "Default `comp_ref_prob`."),
    ("default_single_ref_prob", "DEFAULT_SINGLE_REF_PROB", "10.5", "Default `single_ref_prob`."),
    ("default_mv_sign_prob", "DEFAULT_MV_SIGN_PROB", "10.5", "Default `mv_sign_prob`."),
    ("default_mv_bits_prob", "DEFAULT_MV_BITS_PROB", "10.5", "Default `mv_bits_prob`."),
    ("default_mv_class0_bit_prob", "DEFAULT_MV_CLASS0_BIT_PROB", "10.5", "Default `mv_class0_bit_prob`."),
    ("default_tx_probs", "DEFAULT_TX_PROBS", "10.5", "Default `tx_probs_8x8`, `tx_probs_16x16` and `tx_probs_32x32`, by largest transform size, padded with 0."),
    ("default_inter_mode_probs", "DEFAULT_INTER_MODE_PROBS", "10.5", "Default `inter_mode_probs`."),
    ("default_interp_filter_probs", "DEFAULT_INTERP_FILTER_PROBS", "10.5", "Default `interp_filter_probs`."),
    ("default_mv_joint_probs", "DEFAULT_MV_JOINT_PROBS", "10.5", "Default `mv_joint_probs`."),
    ("default_mv_class_probs", "DEFAULT_MV_CLASS_PROBS", "10.5", "Default `mv_class_probs`."),
    ("default_mv_class0_fr_probs", "DEFAULT_MV_CLASS0_FR_PROBS", "10.5", "Default `mv_class0_fr_probs`."),
    ("default_mv_class0_hp_prob", "DEFAULT_MV_CLASS0_HP_PROB", "10.5", "Default `mv_class0_hp_prob`."),
    ("default_mv_fr_probs", "DEFAULT_MV_FR_PROBS", "10.5", "Default `mv_fr_probs`."),
    ("default_mv_hp_prob", "DEFAULT_MV_HP_PROB", "10.5", "Default `mv_hp_prob`."),
    ("default_coef_probs", "DEFAULT_COEF_PROBS", "10.5", "Default `coef_probs`, by transform size, plane type, reference type, band and context; band 0 has 3 contexts, padded with 0."),
]


def spec_text(pdf):
    data = open(pdf, "rb").read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != PDF_SHA256:
        sys.exit(f"{pdf}: SHA-256 {digest}, expected {PDF_SHA256}")
    text = subprocess.run(["pdftotext", "-layout", pdf, "-"], check=True, capture_output=True, text=True).stdout
    kept = [
        line
        for line in text.replace("\f", "\n").splitlines()
        if "Copyright ©" not in line and "VP9 Bitstream & Decoding Process Specification" not in line
    ]
    return "\n".join(kept)


def constants(text):
    """The constants of clause 3, by name; a name stated twice must agree."""
    start = text.index("REFS_PER_FRAME ")
    found = {}
    for name, value in re.findall(r"^([A-Z][A-Z0-9_]+)\s+(\d+)\s", text[start:], re.M):
        if found.setdefault(name, int(value)) != int(value):
            sys.exit(f"constant {name} is stated as {found[name]} and {value}")
        if name == "INVALID_CASE":
            break
    return found


def dimension(expr, consts):
    tokens = re.findall(r"[A-Za-z_][A-Za-z0-9_]*|\d+|[-+]", expr)
    if "".join(tokens) != re.sub(r"\s", "", expr):
        sys.exit(f"dimension {expr!r} is not a sum of constants")
    total, sign = 0, 1
    for t in tokens:
        if t in "+-":
            sign = 1 if t == "+" else -1
        else:
            total += sign * (int(t) if t.isdigit() else consts[t])
    return total


def strip_comments(s):
    s = re.sub(r"/\*.*?\*/", " ", s, flags=re.S)
    return re.sub(r"//[^\n]*", " ", s)


def braces(body):
    """The nested lists of numbers of `{ ... }` at the start of body, and
    where it ends. A comma before a closing brace is allowed."""
    pos = 0

    def value():
        nonlocal pos
        while body[pos].isspace():
            pos += 1
        if body[pos] == "{":
            pos += 1
            items = []
            while True:
                while body[pos].isspace():
                    pos += 1
                if body[pos] == "}":
                    pos += 1
                    return items
                items.append(value())
                while body[pos].isspace():
                    pos += 1
                if body[pos] == ",":
                    pos += 1
                elif body[pos] != "}":
                    sys.exit(f"unexpected {body[pos:pos + 20]!r}")
        m = re.compile(r"\d+").match(body, pos)
        if not m:
            sys.exit(f"not a number: {body[pos:pos + 20]!r}")
        pos = m.end()
        return int(m.group())

    return value()


def shape_of(v):
    if isinstance(v, int):
        return []
    inner = {tuple(shape_of(x)) for x in v}
    if len(inner) != 1:
        sys.exit("a table whose rows differ in shape")
    return [len(v)] + list(inner.pop())


def table(text, consts, name):
    defs = list(re.finditer(rf"\b{name}((?:\s*\[[^\]]*\])+)\s*=\s*\{{", text))
    if len(defs) != 1:
        sys.exit(f"{name}: {len(defs)} definitions")
    d = defs[0]
    dims = [dimension(x, consts) for x in re.findall(r"\[([^\]]*)\]", d.group(1))]
    values = braces(strip_comments(text[d.end() - 1:]))
    if shape_of(values) != dims:
        sys.exit(f"{name}: shape {shape_of(values)}, its dimensions say {dims}")
    flat = values
    while flat and isinstance(flat[0], list):
        flat = [x for row in flat for x in row]
    if not all(0 <= x <= 255 for x in flat):
        sys.exit(f"{name}: a value outside 0 to 255")
    return dims, values


def rust_type(dims):
    t = "u8"
    for n in reversed(dims):
        t = f"[{t}; {n}]"
    return t


def rust_value(v, indent):
    if isinstance(v[0], int):
        return "[" + ", ".join(str(x) for x in v) + "]"
    pad = "    " * (indent + 1)
    rows = [pad + rust_value(x, indent + 1) + "," for x in v]
    return "[\n" + "\n".join(rows) + "\n" + "    " * indent + "]"


def render(text):
    consts = constants(text)
    out = [
        "// @generated by crates/coreav-vp9/tools/gen_tables.py from the VP9 Bitstream &",
        f"// Decoding Process Specification v0.7, PDF sha256 {PDF_SHA256}.",
        "// Do not edit by hand.",
        "",
        "//! Constant tables of the VP9 specification (version 0.7), read from the",
        "//! specification's own text. See the generator for how each one is found",
        "//! and checked.",
        "",
    ]
    for name, rust, clause, doc in TABLES:
        dims, values = table(text, consts, name)
        ty = rust_type(dims)
        if name in ALIASES:
            out.append(f"/// The type of `{name}`.")
            out.append(f"pub(crate) type {ALIASES[name]} = {ty};")
            out.append("")
            ty = ALIASES[name]
        out.append(f"/// `{name}` (clause {clause}). {doc}")
        out.append("#[rustfmt::skip]")
        out.append(f"pub(crate) const {rust}: {ty} = {rust_value(values, 0)};")
        out.append("")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pdf", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    rendered = render(spec_text(args.pdf))
    if args.check:
        current = open(args.out, encoding="utf-8").read()
        if current != rendered:
            sys.exit(f"{args.out} is not what the generator makes from the specification")
        print(f"{args.out}: generated from the specification")
        return
    open(args.out, "w", encoding="utf-8").write(rendered)


if __name__ == "__main__":
    main()
