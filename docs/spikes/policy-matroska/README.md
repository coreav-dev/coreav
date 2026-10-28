# Corpus run: upload policy checks of Matroska and WebM files

Measurement for [`docs/design/policy-matroska.md`](../../design/policy-matroska.md). Spike rules: throwaway measurement code, outside the
workspace.

## Hypothesis

`coreav check` gives Matroska and WebM files the verdicts ISO base media
files get: no crash, the same bytes on every run, the reasons the policy
reference gives over the probe output, each reason's value at its path, and
the verdict `ffprobe`'s output implies; the block scan's verified sizes bound
what `ffprobe` decodes; and the scanning check is no slower than
`ffprobe -count_packets` and stays within 300 MiB.

## Bars (from the design note, fixed before the run)

| Bar | Set | Threshold |
|---|---|---|
| M1 crashes | every file × policy | 100%: exit 0 or 1 within 10 s and 1 GiB, one valid verdict |
| M2 determinism | every file × policy | 100%: two runs print the same bytes |
| M3 spec | every file × policy | 100%: reasons equal `scripts/policy_ref.py`'s over the probe output |
| M4 precision | every reason | 100%: its value is the probe value at its path or its documented derivation |
| M5 oracle | `real` Matroska and WebM files both CoreAV and `ffprobe` read, × policy | at least 98% agree with the verdict `ffprobe`'s output implies, directly or through a predicate-checked entry of `docs/known-divergences-policy.md` |
| M6 bound | every video stream `ffprobe` decodes | 100%: `scanned_size` is not smaller than `ffprobe`'s `width` × `height` |
| M7 speed | held-out `real` Matroska and WebM files | median ratio of the scanning `coreav check` to `ffprobe -v error -count_packets -show_streams` at most 1.00, overall and for each class (`webm` and `mkv` files) |
| M8 memory | every file | peak resident memory of `coreav check` at most 300 MiB |

The set is every corpus v1 file with a `mkv` or `webm` extension: 423 files,
209 `real` and 214 `edge`, 69 of them held out. The policies are the three
examples in `docs/schema/policy-examples/`.

## Procedure

From the repository root, with `S` naming the spike's directory:

    cargo build --release -p coreav-cli
    python3 -I "$S/run.py" check --work WORK \
        --coreav target/release/coreav --ffprobe ffprobe
    python3 -I "$S/run.py" speed --work WORK \
        --coreav target/release/coreav --ffprobe ffprobe
    python3 -I "$S/summarize.py" --work WORK

`ffprobe` is the pinned 7.0.2 build of `scripts/install-oracle-ffmpeg.sh`.
After a fix, only the files the fix can change are checked again
(`run.py check --only LIST`), and the summary takes their new results.

## Results

Final run: every corpus v1 Matroska and WebM file (423), the three example
policies, with the binary of this change's final code (the structure audit
with its string and binary bounds), `ffprobe` 7.0.2. Earlier runs with intermediate
binaries found the bugs fixed before it (below); their results are
superseded and not kept. Evidence: `evidence/summary.json`,
`evidence/check.jsonl.gz`, `evidence/speed.jsonl.gz`.

| Bar | Result | Threshold | Met |
|---|---|---|---|
| M1 crashes | 0 of 1,269 cases | 100% | yes |
| M2 determinism | 0 differences | 100% | yes |
| M3 spec | 0 differences from `scripts/policy_ref.py` | 100% | yes |
| M4 precision | 0 reasons off their path | 100% | yes |
| M5 oracle | 627 `real` cases: 609 agree directly (97.13%), 18 through predicate-checked entries (KD-P-MKV-UNSIZED 6, KD-P-MKV-UNLISTED 6, KD-P-MKV-UNVERIFIED 6): 100% | at least 98% | yes |
| M6 bound | 0 of 279 scanned video streams below `ffprobe`'s size | 100% | yes |
| M7 speed | 69 held-out `real` files: median ratio 0.344 (95% CI 0.236 to 0.369; 4.5 ms against 21.4 ms), `webm` 0.338 (59 files, CI 0.194 to 0.368), `mkv` 0.375 (10 files, CI 0.160 to 0.423); slowest file 0.775 | at most 1.00, overall and per class | yes |
| M8 memory | 3,772 KiB at most (median 3,384 KiB) | 300 MiB | yes |

Verdicts: 317 accept, 952 reject; 36 cases are `unreadable` (the probe
rejects 12 files: invalid structure, truncated metadata, a limit, or not
Matroska despite the extension).

What the runs found and what changed (each fixed before the final run,
with unit tests and, where it applies, fixtures):

- Laced Opus without `DefaultDuration` (3 `real` files): stream durations unknown. Opus packets are now timed by their
  TOC byte (RFC 6716 3.1).
- Multi-gigabyte streams: the exact bit rate's numerator passed 2^63, so
  the rate was printed `null` and the rule rejected as `unknown`; exact
  values now print numerators up to 2^64 - 1.
- Harness: NUL-padded `DocType` (RFC 8794 7.4), packets with decode times
  only (VfW), streams without packets, `text` subtitles and cover art in
  the oracle mapping.
- Chrome writes H.264 in Matroska without `CodecPrivate`, as an Annex B
  byte stream (a Chromium test file in `edge`): the scan now walks start
  codes for such tracks.
- Independent review (four rounds): crafted files hid Clusters from the
  block listing in ways `ffmpeg` reads past. The scan now audits the
  structure first (design note, decision 8); a failed audit makes every
  stream's `frames_listed` `false`. In the corpus it fails on 2 `real`
  files, both truncated uploads, and on 51 `edge` files (truncations, a
  second `Tracks` or `Info`, unknown top-level IDs, a 5-octet ID).

Streams without a duration in the final run (by codec, all sets) are the
streams of files whose audit failed, laced audio without `DefaultDuration`
in codecs other than Opus, and compressed tracks (PGS, VobSub): each is
rejected as `unknown`, as decision 3 requires.
