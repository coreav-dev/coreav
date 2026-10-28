# Track N1: H.264 decoder conformance spike

Track N, step N1 of the roadmap. A
clean-room H.264 decoder for High profile, 8-bit 4:2:0, progressive frames, written in safe
Rust from Rec. ITU-T H.264 (08/2024) alone. Throwaway spike code: outside the Cargo workspace, never moved into `crates/`, not wired into
any build, not published. Where and when a native H.264 decoder ships as an
opt-in package is the maintainer's decision.

## Verdict

Status: Accepted.

Pass. C1, S1 and S2 are met. On C2, the 3 streams that lose reference
pictures are not judged: the specification defines no output for them, so
concealment that differs from FFmpeg's is not a decoding error. The 10 known
divergences follow the specification.

Known divergences: [`known-divergences.md`](known-divergences.md).

## Hypothesis

A decoder written only from the H.264 specification, with bitstream parsing,
CAVLC and CABAC, reference management and error handling in safe Rust and the
hot kernels behind a replaceable kernel interface, is bit-exact on every
in-scope JVT conformance bitstream and agrees frame for frame with FFmpeg's
decoder on the real-file corpus. N2 (performance) builds on it; N1 sets no
speed bar.

## Scope

In scope: the streams a High profile decoder must decode (clause A.2.4:
`profile_idc` 77 or 100, or `constraint_set1_flag` = 1), with
`chroma_format_idc` = 1, 8-bit luma and chroma and `frame_mbs_only_flag` = 1
(progressive, frame-only: no field pictures, no MBAFF). The rule is applied
to every sequence parameter set of a stream.

Out of scope, and rejected with an "unsupported" error rather than decoded:
field and MBAFF coding, 4:0:0, 4:2:2 and 4:4:4, bit depths above 8, slice
groups (FMO), arbitrary slice order, data partitioning, SP/SI slices,
redundant pictures (redundant slices are skipped), SVC and MVC.

Conformance set: the H.264.1 bitstreams in the `AVCv1` and `FRExt`
directories of the ITU-T site
(<https://www.itu.int/wftp3/av-arch/jvt-site/draft_conformance/>), 204 zips.
`scripts/scope.py` reads each stream's SPS with this decoder's parser, applies
the rule, cross-checks profile and pixel format against `ffprobe`, and writes
[`conformance/streams.json`](conformance/streams.json): **97 streams in
scope, 107 excluded**, each with the zip's SHA-256, the chosen bitstream and
reference YUV, and the reason for exclusion. Exclusions (a stream can have
several reasons): 77 field or MBAFF coded (`frame_mbs_only_flag` 0), 21 with
4:2:2 chroma, 13 above 8 bits, 2 monochrome (4:0:0), and 32 whose profile a
High profile decoder need not decode: High 10 and High 4:2:2, and 9 Baseline
or Extended streams without `constraint_set1_flag`.

Reference choices (recorded per stream in `streams.json`):

- `CVFC1_Sony_C`: `_nogray.yuv`, the cropped output (the other file shows the
  full coded frame with the cropped area in grey).
- `CVSE2_Sony_B`, `CVSE3_Sony_H`, `CVSEFDFT3_Sony_E`: `-norpt.yuv`, each
  decoded frame once. The other file repeats frames per the pic_struct SEI
  (3:2 pull-down), a display process outside decoding.
- `alphaconformanceG`: the package's `test8b43.264` with `test8b43.yuv`, the
  primary video. The combined primary-plus-alpha stream has no decoded
  reference and the alpha stream is 4:0:0.

## Bars

Fixed before the measured runs: the bars, the scope rule and `streams.json`
were committed with the decoder in the first commit of this work, before the runs recorded
in `evidence/`, and are not changed by results.

| Bar | Set | Threshold |
|---|---|---|
| C1 conformance | the 97 in-scope conformance streams | 100%: decoder reports no error, and its output (cropped yuv420p frames in output order) is byte-identical to the reference YUV |
| C2 corpus | every H.264 video stream of corpus v1 (real and edge sets, `encrypted` files skipped) that is in scope by the rule above | 100% `framemd5` agreement with FFmpeg over the compared frames, except recorded known divergences where the specification supports CoreAV's output |
| S1 clean room | `src/` except the generated `src/spec_tables.rs` | the automated similarity check finds no shared span of 50 or more normalized tokens with FFmpeg, x264, OpenH264, rusty_h264 or rust_h264, unless the span reproduces an equation of the specification as written and is listed with its equation number in `similarity/allowlist.json` |
| S2 tables | `src/spec_tables.rs` | regenerated from the specification PDF by `tools/gen_tables.py`, byte-identical |
| I1 speed (information) | the held-out in-scope corpus streams of the C2 run | per stream, the time of this decoder's `--framemd5` run over the time of FFmpeg's `-threads 1 ... -f framemd5` run, both in the same container one after the other; median ratio overall and for 1920x1080; reported, not judged |

C2 method, fixed with the bar: each H.264 video stream is extracted to an
Annex B elementary stream with `ffmpeg -map 0:v:N -c:v copy -bsf:v
h264_mp4toannexb -f h264` (FFmpeg as a black box), stopping after 9,064
access units (`-frames:v`), which hold the first 9,000 output frames
whatever the reordering delay. Both decoders read that
same file: `ffmpeg -threads 1 -i stream.h264 -map 0:v:0 -fps_mode passthrough
-f framemd5 -` and this decoder's output hashed per frame with MD5. The
compared frames are the first 9,000 frames of each stream (5 minutes at 30
frames per second; whole streams are compared when shorter). A stream agrees
when both MD5 sequences are identical in count and order. Streams FFmpeg
cannot decode, or decodes with errors, are reported apart and not judged;
operationally, a stream that disagrees while FFmpeg exits with an error or
prints messages at its `error` log level is reported apart, and a stream
whose sequences agree counts as agreeing whatever FFmpeg logs. Streams the
in-scope rule excludes, and streams FFmpeg cannot extract, are counted, not
judged.

When N2 sets speed bars, I1 follows the default speed bar:
at most 1.00 against FFmpeg.
N1 has no speed bar, so its numbers are information only.

## Clean room

- Source of truth: Rec. ITU-T H.264 (08/2024), the freely available PDF
  (`T-REC-H.264-202408-S!!PDF-E`, SHA-256 `647d6bb2…389d`), and H.264.1 for
  the conformance streams. No FFmpeg, x264, OpenH264, rusty_h264 or rust_h264
  source was opened while writing this code; the `ffmpeg`/`ffprobe` binaries
  are used only as black boxes in scripts.
- Constant tables (CAVLC code tables, CABAC initialization values and state
  transitions, scans, default scaling lists, deblocking thresholds,
  macroblock type tables, CABAC bin strings, level limits) are not typed by
  hand: `tools/gen_tables.py` reads them from the specification's tables with
  pdfplumber, checks shape, continuity and prefix-freeness, and writes
  `src/spec_tables.rs`. One erratum in the 08/2024 PDF is corrected and
  checked against its neighbouring rows: Table 9-26 prints ctxIdx 541 as 641.
- rusty_h264 and rust_h264, which the roadmap names for evaluation as
  dependencies or baselines, were neither built nor
  run here. Using either as a
  dependency would replace the clean-room decoder this spike tests, and as
  black-box baselines they belong to N2's speed comparison.
- Similarity check (bar S1): `similarity/check.py`, run in CI, fetches the
  sources of the five projects into a temporary directory and compares them
  with `src/` by token fingerprints. Neither a person nor an agent reads the
  fetched sources: the script prints only locations in this spike's code and
  the length of each shared span. Method: [Similarity check](#similarity-check).

## Design

Modules, in decoding order:

| Module | Clauses | Content |
|---|---|---|
| `bits.rs` | 7.2, 7.3.1, 9.1 | Annex B splitting, emulation prevention removal, bit reader, Exp-Golomb |
| `params.rs` | 7.3.2, 7.4.2, 8.5.9 | SPS (with VUI to the DPB fields), PPS, scaling list fall-back rules A and B, LevelScale |
| `slice.rs` | 7.3.3, 7.4.3 | slice header, list modification, weight tables, marking operations |
| `cavlc.rs` | 9.2 | coeff_token, levels, total_zeros and run_before through lookup tables built from the generated code tables |
| `cabac.rs` | 9.3 | arithmetic decoding engine, context initialization, binarizations (table-driven bin strings for mb_type and sub_mb_type) |
| `mb.rs` | 7.3.4, 7.3.5, 8.3, 8.5 | slice data loop, macroblock layer for both entropy coders through one `Syn` trait, neighbour derivation, intra mode prediction, residual reconstruction |
| `inter.rs` | 8.4 | motion vector prediction, P_Skip, spatial and temporal direct, implicit and explicit weighted prediction, motion compensation |
| `intra.rs` | 8.3 | Intra_4x4, Intra_8x8 (with reference filtering), Intra_16x16 and chroma prediction |
| `deblock.rs` | 8.7 | boundary strength, thresholds, edge order |
| `dpb.rs` | 8.2.4, 8.2.5, C.4 | reference lists, sliding window and adaptive marking, bumping output |
| `decoder.rs` | 7.4.1.2.4, 8.2.1, 8.2.5.2 | picture boundaries, picture order count types 0 to 2, frame_num gaps |
| `kernels.rs` | 8.4.2.2, 8.5.12, 8.5.13, 8.7.2 | the kernel interface and its safe Rust implementation |

Decisions:

- **One macroblock layer for both entropy coders.** The syntax of clause
  7.3.5 is written once, generic over a `Syn` trait with a CAVLC and a CABAC
  implementation. Rejected: two parsers, which duplicate the condition logic
  that most often hides decoder bugs. The residual structure differs (CAVLC
  codes an 8x8 block as four interleaved 4x4 lists) and is handled where the
  syntax differs.
- **Per-macroblock neighbour state, per-4x4 motion.** Each macroblock keeps
  what its neighbours need (type, coded block pattern, QP, intra modes, CAVLC
  coefficient counts, CABAC coded_block_flag bits, mvd magnitudes); each
  picture keeps motion vectors, reference indices and the identity of the
  referenced picture per 4x4 block, which serves motion vector prediction,
  co-located lookups for direct prediction and deblocking. Rejected: a
  partition tree, which every consumer would have to walk.
- **Kernel interface.** `Kernels` covers the inverse transforms, luma and
  chroma interpolation and the edge filter, the pure-arithmetic code a
  later `coreav-kernels-*` crate implements in assembly. Each kernel gets
  exactly the samples it may touch; the `checked` wrappers validate block
  geometry, strides and ranges first and return an error otherwise.
  `Scalar` is the safe Rust implementation the results are measured with. No
  assembly and no intrinsics in this spike.
- **Output order by the bumping process.** Frames leave the DPB as clause
  C.4.5.3 specifies, with the DPB size from `max_dec_frame_buffering` or the
  level's MaxDpbMbs. Rejected: sorting each coded video sequence by picture
  order count, which needs the whole sequence in memory.
- **Unsupported features fail loudly.** A stream outside scope returns
  `Error::Unsupported` per slice; nothing is guessed.
- **Concealment for non-conforming input** (decided after the first corpus
  run; the specification leaves these cases to the decoder):
  - A frame_num gap without `gaps_in_frame_num_value_allowed_flag` infers no
    "non-existing" frames: clause 8.2.5.2 runs only for conforming streams,
    and inferred frames would evict the real reference frames. Rejected:
    running the gap process anyway, which corrupted every following picture
    of the encoders that increment frame_num on non-reference pictures.
  - Inferred "non-existing" frames copy the previous reference frame (their
    samples "may be set to any value"), and long gaps share one placeholder
    for the frames the sliding window drops at once.
  - A stream that starts at a non-IDR picture (random access) decodes its
    leading pictures, those with a picture order count below the first
    picture's until the next IDR or memory_management_control_operation 5,
    but does not output them: they may predict from pictures before the
    start (clause D.2.8, recovery point).
  - Missing co-located references in temporal direct use index 0; adaptive
    marking that leaves more than `max_num_ref_frames` references drops the
    oldest short-term ones; `max_dec_frame_buffering` is clamped to
    MaxDpbFrames.

Limits: `Limits::max_mbs` (default: level 6.2 MaxFS) bounds the picture size;
every syntax element is range-checked against its semantics. The spike may
panic on hostile input; production code will not.

## Similarity check

`similarity/check.py` (bar S1) runs in CI on every change to the spike.

1. It fetches the sources into a temporary directory: FFmpeg's
   `libavcodec` H.264, CABAC and Exp-Golomb files (shallow sparse clone),
   x264's `common/` and `encoder/`, OpenH264's `codec/decoder` and
   `codec/common`, and the crates rusty_h264 (with rusty_h264-common,
   -decoder and -encoder) 0.16.1 and rust_h264 0.4.0. The report records
   each project's revision.
2. It tokenizes C, C++ and Rust alike and normalizes away what differs
   between the languages: comments are dropped, identifiers become one token
   class, type names, declaration keywords, Rust `as` casts, semicolons and
   commas are dropped, numbers become their decimal value.
3. It hashes every run of 20 normalized tokens of each project and marks
   the runs of the spike's `src/` (without the generated tables) that occur
   in a project. Overlapping marks merge into spans; a span of 50 tokens or
   more fails unless `similarity/allowlist.json` pins it by file, enclosing
   item and the digest of its tokens, with the specification text it
   reproduces.
4. A self-test plants a 200-token excerpt of each project into a copy of a
   spike file and requires the same logic to report it as a failing span.

The script prints locations in the spike's code, span lengths and project
names, never third-party code. Limits: a translation that restructures code
(different statement order or temporaries) is not caught; the check is a
guard against verbatim or near-verbatim reuse, on top of the process in
principle 4.

## How it runs

    cargo build --release
    scripts/fetch_conformance.sh ZIPDIR
    python3 -I scripts/scope.py --zips ZIPDIR --work WORK \
        --h264dec target/release/h264dec --ffprobe FFPROBE --out conformance/streams.json
    python3 -I scripts/conformance.py --work WORK --h264dec target/release/h264dec \
        --out evidence/conformance.json

The corpus run (bar C2) on a cloud host, resumable, and its summary:

    # the cloud launcher (not published) writes evidence/corpus.jsonl
    python3 -I scripts/summarize_corpus.py evidence/corpus.jsonl \
        --rerun evidence/corpus-4k-rerun.jsonl --json evidence/corpus-summary.json
    venv/bin/python scripts/investigate.py --summary evidence/corpus-summary.json \
        --h264dec target/release/h264dec --ffmpeg FFMPEG --work DIR > evidence/divergences.jsonl

`h264info INPUT.h264` prints the slice headers of a stream, and
`H264DEC_POC=1 h264dec INPUT --framemd5` adds each frame's picture order
count, for investigating divergences.

Regenerating the tables (bar S2):

    python3 -m venv venv && venv/bin/pip install pdfplumber==0.11.10
    venv/bin/python -I tools/gen_tables.py --pdf h264.pdf --out src/spec_tables.rs

## Results

Measured with the decoder binary of this pull request (SHA-256
`58655aef…c39fb35b`, recorded in `conformance.json`, `corpus-run.json` and
`corpus-4k-rerun.jsonl`).

| Bar | Result | Status |
|---|---|---|
| C1 conformance | 97 of 97 in-scope streams byte-identical to the reference YUV, no decoder error (`evidence/conformance.json`) | **Met** |
| C2 corpus | 933 of 946 judged streams agree with FFmpeg frame for frame (2,152,086 frames compared); 10 disagreements are recorded known divergences ([`known-divergences.md`](known-divergences.md)); 3 non-conforming streams with lost pictures differ in concealment | **Not met as written** (933 of 936 outside the known divergences) |
| S1 clean room | no shared span of 50+ tokens with FFmpeg, x264 or OpenH264; self-test detects a planted excerpt of every project | **Met** |
| S2 tables | regenerated from the ITU-T PDF in CI, byte-identical | **Met** |
| I1 speed (information) | median time ratio to `ffmpeg -threads 1` **3.01** over 183 held-out streams, **2.92** over the 31 held-out 1920x1080 streams (higher is slower) | not judged |

C2 in detail (`evidence/corpus-summary.json`, every line of the run in
`evidence/corpus.jsonl.gz`):

- 1,113 files (every corpus v1 file tagged `v:h264`, minus encrypted ones;
  none missing), 1,135 H.264 video streams: 946 judged, 11 reported apart
  because FFmpeg itself reports errors, 127 out of scope by the SPS rule
  (Baseline or Extended without `constraint_set1_flag`, 4:2:2 and 4:4:4,
  above 8 bits, field or MBAFF coding, AVC-Intra files without an in-band
  SPS), 41 that FFmpeg could not extract as an elementary stream.
- 8 streams: FFmpeg omits one picture while it enlarges its reordering
  buffer; this decoder outputs every picture in picture order count order
  (KD-N1-REORDER, clauses C.4.5.1 to C.4.5.3).
- 2 streams change resolution; the FFmpeg command rescales frames after the
  change, and with `-noautoscale` the hashes agree (KD-N1-AUTOSCALE).
- 3 streams lose reference pictures (`frame_num` skips while gaps are not
  allowed). The specification specifies no output for them, so they do not
  qualify as known divergences, and they are why C2 is not met as written.
- 11 agreeing streams make this decoder report errors (`frame_num` gaps,
  list modifications naming lost pictures, a slice past the picture end);
  the output agrees with FFmpeg's concealment anyway. They are listed in the
  summary.
- The 4K streams `…fc3fa8012cc7424380115327649d6cfa66d8c.mp4` (3840x2160)
  and `…d37ad5ae87a9cc00dc85fd4a2aa7b5c4b5e9.mp4` (4096x2160) hit the
  1,500 s per-step timeout on a cloud host; they were run again on the development
  machine with the same script and binary and no timeout, and agree on all
  7,313 and 8,929 frames (this decoder 1,768 s and 2,021 s, FFmpeg 403 s and
  453 s). Their records, in `evidence/corpus-4k-rerun.jsonl`, replace the
  timeout records in the summary (`summarize_corpus.py --rerun`).

How the run went: the first full run (`evidence/corpus-run1.jsonl.gz`,
earlier binary) had 23 disagreements of 942 judged streams. Investigating
them found that inferring "non-existing" frames for `frame_num` gaps the SPS
does not allow corrupted every following picture, and that leading pictures
of a stream starting at a non-IDR picture were output; both are concealment
choices for non-conforming input, now decided as in Design. An independent
review asked for robustness fixes (DPB size clamp, PPS activation, picture
boundaries, bounded gap filling) and for a check of the denominator. The
second run (`evidence/corpus-run2.jsonl.gz`) found a regression from one of
those fixes (a motion vector difference range in the wrong unit), and the
third run is the measured one. The bars did not change.

The corpus runs used 1 CPU and 4 GiB per file on a cloud host. Conformance runs and
the similarity check ran on the development machine and in CI.

Information outside the bars: the four Baseline conformance streams excluded
only by the profile rule (`MR2_TANDBERG_E` to `MR5_TANDBERG_C`) also decode
bit-exact, and a 1080p x264 High profile stream (CABAC, B-frames, 8x8
transform) is byte-identical to FFmpeg's output.

What N1 shows for the hypothesis: a decoder written from the specification
alone, with every table generated from it, decodes all in-scope conformance
streams exactly and agrees with FFmpeg on 99.7% of in-scope real-file
streams once FFmpeg's own output-order and rescaling behaviour are set
aside; the remaining differences are concealment of corrupted input. It is
about three times slower than FFmpeg's single-threaded decoder with scalar
kernels, which is N2's subject.
