# One walk for the Matroska scan

Status: Accepted. Design: `docs/design/policy-matroska.md`, decision 9;
bars W1 to W5 in its "One walk" section, committed before the runs.
Evidence: [`mkv-one-pass/`](mkv-one-pass/).

The change is measured against `main` at fbf5824, where the structure
audit walked the whole file before the packet listing walked it again.

## Results

| Bar | Result |
|---|---|
| W1 same results | **met**: 3,384 of 3,384 scans of the 423 corpus files, and 44,220 of 44,220 of 24,606 hostile, fixture and fuzz-corpus inputs (152,628 more runs where the probe refuses the input), equal value or equal error |
| W2 verdicts | **met**: 1,269 of 1,269 (423 files × 3 policies), the same bytes and exit status |
| W3 reads | **not met**: median ratio 0.629 (bar 0.80), but 42 files read more than on fbf5824 (bar: none) |
| W5 native speed | **met**: median ratio 0.237 [0.220, 0.431] (`webm` 0.236 [0.217, 0.448], `mkv` 0.248 [0.182, 0.416]; bar 1.00) |

### W1 and W2

`harness.rs`, built against each commit, prints `scan_tracks`'s result
(the value, or the error with its offset and reason) for each file whole
and cut in half, with the default `max_block_work` and with 65,536, 40
and 7; the small limits put the two walks' work limits at different
places. The inputs:
- corpus v1, every Matroska and WebM file (209 `real`, 214 `edge`;
  `run_corpus.py`, summary in `corpus.jsonl`);
- the hostile suite's structural, mutation and Matroska parts (206
  files), the generated Matroska fixtures, and the fuzz corpora of the
  bug-finding pass (`fuzz/corpus-0bca9b9`) for the `coreav-mkv` targets,
  `mkv_remux` and `check`, with their findings.

Every result is equal, errors included: the audit's error where both
walks fail (for example `a Cluster runs past the Segment or the file`
before a listing error), the listing's where only it fails (`negative
lace size`), and the work limit where one walk runs out. `coreav check`
under the three example policies prints the same bytes on every corpus
file.

### W3

Bytes read by the scan through `BufferedSource` (the WebAssembly build's
window), against fbf5824's: median 0.629 (`real` 0.608, `edge` 0.649;
`mkv` 0.552, `webm` 0.652), 0.563 in total (12.5 GB against 22.2 GB).
The bar that no file read more is not met, for 42 of 411 files:

- **41 files the scan rejects** (all but two `edge`, most cut short: the
  audit fails at `a Cluster runs past the Segment or the file` near the
  end). The two walks stopped after the audit, which reads element
  headers only; the one walk also lists the blocks and reads the frame
  headers up to where the audit fails, as it does for a file that
  passes. Up to 3.99 times fbf5824's bytes, at most 1.36 times the file.
- **One file the scan accepts**, `9f4dfff5cbff` (a 2.9 MB WebM): 0.65%
  more. Its VP9 frame reader reads a frame's last bytes (the superframe
  index) before its first, after the listing has read the BlockGroup's
  elements behind the frame; both commits move the window back for it.

The cost on rejected files is the price of one walk: the audit's verdict
is not known before the end of the file. It was not traded away; the
bar stays not met.

### W5

M7's method (`speed.py`, after the `speed` run of the policy Matroska
spike's script) on
the 69 held-out Matroska and WebM files of corpus v1, downloaded: one warm-up run each, then five
rounds of the scanning `coreav check --policy web-upload.json` and
`ffprobe -v error -count_packets -show_streams` in alternating order, on
this repository's development host (8 vCPU Intel Xeon). Per-file medians
in `native.jsonl`; the statistics are those of `bench/shared/benchlib.py`.

## Spend

R2 reads only, no cloud host: corpus v1's 423 Matroska and WebM files
(47.4 GB) for W1 to W3, the 69 held-out objects (12.9 GB) for W5. R2 egress is free; the class B operations cost below
0.01 USD.
