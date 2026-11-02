#!/usr/bin/env python3
"""Generate crates/coreav-vp9/src/tables.rs and crates/coreav-kernels-vp9/src/tables.rs
from the VP9 specification PDF.

Each table is read from the specification's own text, as pdftotext
(poppler) lays it out: the table's definition in the C-like notation of the
specification, its dimensions resolved with the constants of clause 3, and
its braces checked to nest exactly as those dimensions say. No table is
typed by hand, and nothing comes from another implementation.

    python3 -I crates/coreav-vp9/tools/gen_tables.py \\
        --pdf vp9-bitstream-specification-v0.7-20170222-draft.pdf \\
        --out crates/coreav-vp9/src/tables.rs \\
        --kernels-out crates/coreav-kernels-vp9/src/tables.rs [--check]

The PDF is https://storage.googleapis.com/downloads.webmproject.org/docs/vp9/vp9-bitstream-specification-v0.7-20170222-draft.pdf;
it is not committed. With --check the files are compared, not written, and
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
    ("kf_partition_probs", "KF_PARTITION_PROBS", "10.4", "`partition`'s probabilities in intra frames, by context."),
    ("kf_y_mode_probs", "KF_Y_MODE_PROBS", "10.4", "`default_intra_mode`'s probabilities, by the modes above and to the left."),
    ("kf_uv_mode_probs", "KF_UV_MODE_PROBS", "10.4", "`default_uv_mode`'s probabilities, by `y_mode`."),
    ("pareto_table", "PARETO_TABLE", "10.3", "The token probabilities past the second node, by `(prob - 1) / 2`."),
    ("default_scan_4x4", "DEFAULT_SCAN_4X4", "10.1", "The scan of a 4x4 transform of `DCT_DCT` or `ADST_ADST`."),
    ("col_scan_4x4", "COL_SCAN_4X4", "10.1", "The scan of a 4x4 transform of `DCT_ADST`."),
    ("row_scan_4x4", "ROW_SCAN_4X4", "10.1", "The scan of a 4x4 transform of `ADST_DCT`."),
    ("default_scan_8x8", "DEFAULT_SCAN_8X8", "10.1", "The scan of an 8x8 transform of `DCT_DCT` or `ADST_ADST`."),
    ("col_scan_8x8", "COL_SCAN_8X8", "10.1", "The scan of an 8x8 transform of `DCT_ADST`."),
    ("row_scan_8x8", "ROW_SCAN_8X8", "10.1", "The scan of an 8x8 transform of `ADST_DCT`."),
    ("default_scan_16x16", "DEFAULT_SCAN_16X16", "10.1", "The scan of a 16x16 transform of `DCT_DCT` or `ADST_ADST`."),
    ("col_scan_16x16", "COL_SCAN_16X16", "10.1", "The scan of a 16x16 transform of `DCT_ADST`."),
    ("row_scan_16x16", "ROW_SCAN_16X16", "10.1", "The scan of a 16x16 transform of `ADST_DCT`."),
    ("default_scan_32x32", "DEFAULT_SCAN_32X32", "10.1", "The scan of a 32x32 transform."),
    ("b_width_log2_lookup", "B_WIDTH_LOG2", "10.2", "A block size's width in 4x4 blocks, as a base 2 logarithm."),
    ("b_height_log2_lookup", "B_HEIGHT_LOG2", "10.2", "A block size's height in 4x4 blocks, as a base 2 logarithm."),
    ("num_4x4_blocks_wide_lookup", "NUM_4X4_WIDE", "10.2", "A block size's width in 4x4 blocks."),
    ("num_4x4_blocks_high_lookup", "NUM_4X4_HIGH", "10.2", "A block size's height in 4x4 blocks."),
    ("mi_width_log2_lookup", "MI_WIDTH_LOG2", "10.2", "A block size's width in 8x8 blocks, as a base 2 logarithm."),
    ("num_8x8_blocks_wide_lookup", "NUM_8X8_WIDE", "10.2", "A block size's width in 8x8 blocks."),
    ("num_8x8_blocks_high_lookup", "NUM_8X8_HIGH", "10.2", "A block size's height in 8x8 blocks."),
    ("subsize_lookup", "SUBSIZE", "10.2", "The size of a partition's blocks, by partition and block size."),
    ("max_txsize_lookup", "MAX_TX_SIZE", "6.4.10", "A block size's largest transform size."),
    ("ss_size_lookup", "SS_SIZE", "6.4.23", "A block size in a plane, by block size and horizontal and vertical subsampling."),
    ("coefband_4x4", "COEFBAND_4X4", "10.2", "The band of each scan position of a 4x4 transform."),
    ("coefband_8x8plus", "COEFBAND_8X8PLUS", "10.2", "The band of each scan position of a larger transform."),
    ("energy_class", "ENERGY_CLASS", "10.2", "The context class of each token."),
    ("mode2txfm_map", "MODE2TXFM_MAP", "10.2", "The transform type of each prediction mode."),
    ("extra_bits", "EXTRA_BITS", "6.4.26", "Each token's category, number of extra bits and base value."),
    ("cat_probs", "CAT_PROBS", "6.4.26", "The probabilities of each category's extra bits, padded with 0."),
    ("partition_tree", "PARTITION_TREE", "9.3.1", "The tree of `partition` when the block has rows and columns inside the frame."),
    ("cols_partition_tree", "COLS_PARTITION_TREE", "9.3.1", "The tree of `partition` when only the block's columns are inside the frame."),
    ("rows_partition_tree", "ROWS_PARTITION_TREE", "9.3.1", "The tree of `partition` when only the block's rows are inside the frame."),
    ("intra_mode_tree", "INTRA_MODE_TREE", "9.3.1", "The tree of the intra prediction modes."),
    ("segment_tree", "SEGMENT_TREE", "9.3.1", "The tree of `segment_id`."),
    ("tx_size_32_tree", "TX_SIZE_32_TREE", "9.3.1", "The tree of `tx_size` up to `TX_32X32`."),
    ("tx_size_16_tree", "TX_SIZE_16_TREE", "9.3.1", "The tree of `tx_size` up to `TX_16X16`."),
    ("tx_size_8_tree", "TX_SIZE_8_TREE", "9.3.1", "The tree of `tx_size` up to `TX_8X8`."),
    ("token_tree", "TOKEN_TREE", "9.3.1", "The tree of `token`."),
    ("dc_qlookup", "DC_QLOOKUP", "8.6.1", "The DC quantizer step, by `(BitDepth - 8) >> 1` and quantizer index."),
    ("ac_qlookup", "AC_QLOOKUP", "8.6.1", "The AC quantizer step, by `(BitDepth - 8) >> 1` and quantizer index."),
]

# The tables and constants of crates/coreav-kernels-vp9/src/tables.rs.
KERNEL_TABLES = [
    ("cos64_lookup", "COS64_LOOKUP", "8.7.1.1", "`cos64(angle)` for angles 0 to 32."),
]
KERNEL_CONSTANTS = [
    ("SINPI_1_9", "8.7.1.6"),
    ("SINPI_2_9", "8.7.1.6"),
    ("SINPI_3_9", "8.7.1.6"),
    ("SINPI_4_9", "8.7.1.6"),
]

# Tables whose rows the specification writes short, as C does, the rest 0.
PADDED = {"cat_probs"}


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
    """The constants of clause 3, and the names the semantics of clause 7.4
    give values (block and transform sizes, partitions, intra modes and
    tokens), by name; a name stated twice must agree. The trees of clause
    9.3.1 name the token categories DCT_VAL_CATEGORY1 to 6, which the
    semantics of token (7.4.8) call DCT_VAL_CAT1 to 6."""
    start = text.index("REFS_PER_FRAME ")
    found = {}

    def add(name, value):
        if found.setdefault(name, int(value)) != int(value):
            sys.exit(f"constant {name} is stated as {found[name]} and {value}")

    for name, value in re.findall(r"^([A-Z][A-Z0-9_]+)\s+(\d+)\s", text[start:], re.M):
        add(name, value)
        if name == "INVALID_CASE":
            break
    named = r"(?:BLOCK|TX)_\d+X\d+|PARTITION_[A-Z]+|[A-Z0-9]+_PRED|[A-Z]+_TOKEN|DCT_VAL_CAT\d"
    for value, name in re.findall(rf"^\s*(\d+)\s+({named})\s*$", text, re.M):
        add(name, value)
    for n in range(1, 7):
        add(f"DCT_VAL_CATEGORY{n}", found[f"DCT_VAL_CAT{n}"])
    for name, value in re.findall(r"^\s*(SINPI_\d_9)\s+(\d+)\s*$", text, re.M):
        add(name, value)
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


def braces(body, symbols):
    """The nested lists of numbers of `{ ... }` at the start of body, and
    where it ends. A comma before a closing brace is allowed, a name
    stands for its value in symbols, and either may be negated."""
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
        sign = 1
        if body[pos] == "-":
            sign = -1
            pos += 1
        m = re.compile(r"\d+|[A-Za-z_][A-Za-z0-9_]*").match(body, pos)
        if not m:
            sys.exit(f"not a number: {body[pos:pos + 20]!r}")
        pos = m.end()
        if m.group().isdigit():
            return sign * int(m.group())
        if m.group() not in symbols:
            sys.exit(f"unknown name {m.group()!r}")
        return sign * symbols[m.group()]

    return value()


def shape_of(v):
    if isinstance(v, int):
        return []
    inner = {tuple(shape_of(x)) for x in v}
    if len(inner) != 1:
        sys.exit("a table whose rows differ in shape")
    return [len(v)] + list(inner.pop())


def padded(values, dims):
    """values with each short list filled with zeros to its dimension."""
    if not dims:
        return values
    if not isinstance(values, list) or len(values) > dims[0]:
        sys.exit("a padded table with too many values")
    zero = padded([], dims[1:]) if len(dims) > 1 else 0
    return [padded(v, dims[1:]) for v in values] + [zero] * (dims[0] - len(values))


def table(text, consts, name):
    defs = list(re.finditer(rf"\b{name}((?:\s*\[[^\]]*\])+)\s*=\s*\{{", text))
    if len(defs) != 1:
        sys.exit(f"{name}: {len(defs)} definitions")
    d = defs[0]
    dims = [dimension(x, consts) for x in re.findall(r"\[([^\]]*)\]", d.group(1))]
    values = braces(strip_comments(text[d.end() - 1:]), consts)
    if name in PADDED:
        values = padded(values, dims)
    if shape_of(values) != dims:
        sys.exit(f"{name}: shape {shape_of(values)}, its dimensions say {dims}")
    flat = values
    while flat and isinstance(flat[0], list):
        flat = [x for row in flat for x in row]
    if min(flat) < 0:
        if not all(-128 <= x <= 127 for x in flat):
            sys.exit(f"{name}: a negative value with one outside -128 to 127")
        return dims, values, "i8"
    if max(flat) > 65535:
        sys.exit(f"{name}: a value above 65535")
    return dims, values, "u8" if max(flat) <= 255 else "u16"


def rust_type(dims, element):
    t = element
    for n in reversed(dims):
        t = f"[{t}; {n}]"
    return t


def rust_value(v, indent):
    if isinstance(v[0], int):
        return "[" + ", ".join(str(x) for x in v) + "]"
    pad = "    " * (indent + 1)
    rows = [pad + rust_value(x, indent + 1) + "," for x in v]
    return "[\n" + "\n".join(rows) + "\n" + "    " * indent + "]"


def render(text, tables, scalars):
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
    for name, clause in scalars:
        out.append(f"/// `{name}` (clause {clause}).")
        out.append(f"pub(crate) const {name}: i64 = {consts[name]};")
        out.append("")
    for name, rust, clause, doc in tables:
        dims, values, element = table(text, consts, name)
        ty = rust_type(dims, element)
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
    ap.add_argument("--out", required=True, help="the decoder's tables.rs")
    ap.add_argument("--kernels-out", required=True, help="the kernels' tables.rs")
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    text = spec_text(args.pdf)
    for out, tables, scalars in ((args.out, TABLES, []), (args.kernels_out, KERNEL_TABLES, KERNEL_CONSTANTS)):
        rendered = render(text, tables, scalars)
        if args.check:
            current = open(out, encoding="utf-8").read()
            if current != rendered:
                sys.exit(f"{out} is not what the generator makes from the specification")
            print(f"{out}: generated from the specification")
        else:
            open(out, "w", encoding="utf-8").write(rendered)


if __name__ == "__main__":
    main()
