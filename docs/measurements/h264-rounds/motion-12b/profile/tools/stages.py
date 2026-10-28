#!/usr/bin/env python3
"""Maps function names to the common stages of docs/measurements/h264-gap-2.

Ours by module and function; FFmpeg by symbol name alone (its source was not
read: ADR 0037). A name no rule matches goes to `other`; `report.py` writes
the largest of those into the evidence, so a wrong or missing rule shows.
The rules are tried in order, the first match wins. `memset`, `memcpy` and
`memmove` go to `other` on both sides: FFmpeg's are in libc, where `perf`
cannot name them by FFmpeg's symbols, so they are kept out of every stage.
"""
import re

STAGES = ["entropy", "mb control", "luma MC", "chroma MC", "combine", "idct/dequant/add",
          "intra pred", "deblock filter", "deblock strength/driver", "picture/edge/output", "other"]

OURS = [
    ("entropy", r"coreav_h264::cabac::|CabacSyntax|coreav_h264::cavlc::|CavlcSyntax|parse_residual|parse_mvd|BitReader|::decode_mb::<"),
    ("other", r"memmove|memcpy|memset"),
    ("deblock filter", r"::filter_edge|filter_luma|filter_chroma"),
    ("deblock strength/driver", r"coreav_h264::deblock::"),
    ("chroma MC", r"chroma_mc|average_rows|copy_rows"),
    ("luma MC", r"luma_mc|tap_rows|::horizontal|x86::widen|x86::centre"),
    ("combine", r"::combine|combine_predictions"),
    ("idct/dequant/add", r"idct|CoeffScale|prepare_level_scales|reconstruct_inter|reconstruct_chroma"),
    ("intra pred", r"coreav_h264::intra::|coreav_kernels_h264::intra::|x86::intra|reconstruct_intra|derive_4x4_mode"),
    ("picture/edge/output", r"edge_window|start_picture|finish_picture|Picture>::|copy_from"),
    ("mb control", r"SliceDecoder|PictureWork|MbData|coreav_h264::mb::|coreav_h264::inter::|coreav_h264::work::|direct_|motion"),
]

FFMPEG = [
    ("entropy", r"cabac|cavlc|decode_residual|decode_significance|get_ue|get_se|golomb|ff_h2645_extract_rbsp|coeff_token|decode_mb_skip"),
    ("other", r"memmove|memcpy|memset"),
    ("deblock strength/driver", r"^loop_filter$|loop_filter_strength|ff_h264_filter_mb|filter_mb_|check_mv|h264_filter_mb"),
    ("deblock filter", r"loop_filter_|ff_deblock_|h264_loop_filter"),
    ("chroma MC", r"chroma_mc"),
    ("combine", r"weight"),
    ("luma MC", r"qpel|put_pixels|avg_pixels|put_h264_qpel|avg_h264_qpel|pixels16|pixels8"),
    ("idct/dequant/add", r"idct|dequant"),
    ("intra pred", r"^pred|_pred(4x4|8x8|16x16)|ff_pred|pred8x8l|pred16x16|pred4x4"),
    ("picture/edge/output", r"emulated_edge|ff_prefetch|av_frame|null_|wrapped_avframe|copy_plane|image_copy"),
    ("mb control", r"hl_decode_mb|fill_decode|mc_part|mc_dir_part|pred_direct|decode_slice|ff_h264_hl|pred_motion|direct_|ff_h264_decode_ref|h264_slice|xchg_mb_border|backup_mb_border"),
]


def stage(name, side):
    """The stage of a function name: `side` is "ours" or "ffmpeg"."""
    for st, pat in (OURS if side == "ours" else FFMPEG):
        if re.search(pat, name):
            return st
    return "other"
