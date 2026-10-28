# `coreav-mkv` against `ffprobe` and MediaInfo

Results of the corpus run whose pass bars `docs/design/mkv.md` fixed before
the run (commit `7b5db09`). Counts only; per-file
captures stay outside git.

## Method

- Binaries: `ffprobe` 7.0.2 and MediaInfo 26.10 (the pinned oracles), and
  `tools/mkv-adapter` built from the commit that adds this file.
- `tools/mkv-adapter/corpus.py run` downloads each file from R2, checks its
  SHA-256, captures the `streams` and `packets` levels from both sides and
  the MediaInfo record with a 120 s timeout per capture, then deletes the
  file. `corpus.py report` compares each file with `coreav-oracle compare`
  and applies `scripts/oracle_kd_mkv.py` (`docs/known-divergences-mkv.md`).
- Inputs: corpus v1 `real` files with extension `mkv` or `webm`
  (development split 140, held-out split 69); 41 Chrome 155 MediaRecorder
  recordings ([`docs/corpus.md`](../corpus.md#mediarecorder-recordings)); corpus v1 `edge` Matroska files (214,
  robustness only). Corpus v1 has no `hostile` Matroska file.
- The held-out split was compared once, with the final binary, after the
  last code change; an earlier capture, made before the review fixes, was
  discarded without being compared. No change was made in response to it.

## Results

Against `ffprobe` 7.0.2, after the documented known divergences
(`docs/known-divergences-mkv.md`). "Direct" agrees field for field; "via KD"
differs only in ways a documented divergence covers and checks.

| Set | Files | Streams: files agreeing (direct + via KD) | Packets agreeing (direct + via KD) | Crashes, panics, timeouts |
|---|---|---|---|---|
| Development split | 140 | 140/140, 100% (130 + 10) | 6,420,727/6,423,612, 99.955% (5,059,929 + 1,360,798) | 0 |
| Held-out split | 69 | 69/69, 100% (66 + 3) | 3,265,258/3,265,259, 99.99997% (2,467,571 + 797,687) | 0 |
| MediaRecorder (Chrome 155) | 41 | 41/41, 100% (29 + 12) | 8,586/8,586, 100% (7,221 + 1,365) | 0 |
| **Real files together** | **250** | **250/250, 100%** | **9,694,571/9,697,457, 99.970%** | **0** |
| Edge (robustness only) | 214 | 159/210, 75.7% (82 + 77; 4 `ffprobe` failures) | 1,158,488/1,367,794, 84.7% | 0 |

Every bar of `docs/design/mkv.md` holds: at least 98% of files and 99% of
packets on the real files together and on the held-out split alone, no
crash, panic or hang on any Matroska file, and 100% of the fixtures in CI.

Divergences used, by entry (files for streams, packets for packets):

| Entry | Development | Held-out | MediaRecorder |
|---|---|---|---|
| `KD-MKV-DTS` | 1,147,598 | 755,938 | 1,365 |
| `KD-MKV-LACE-UNTIMED` | 108,742 | 0 | 0 |
| `KD-MKV-LACE-SPLIT` | 24,748 | 41,749 | 0 |
| `KD-MKV-VFW-TIMESTAMPS` | 73,368 | 0 | 0 |
| `KD-MKV-ENCRYPTED` | 4,143 | 0 | 0 |
| `KD-MKV-AUDIO-KEYFRAME` | 2,165 | 0 | 0 |
| `KD-MKV-WEBM-WEBVTT` | 34 | 0 | 0 |
| `KD-MKV-STREAM-DURATION` (files) | 8 | 2 | 0 |
| `KD-MKV-FMT-ROUNDING` (files) | 1 | 1 | 12 |
| `KD-MKV-UNREAD-BITSTREAM` (files) | 1 | 1 | 0 |
| `KD-MKV-HEVC-RANGE` (files) | 1 | 0 | 0 |

Uncovered disagreements:

- Development: 2,885 packets in three files. A VobSub track (792 packets)
  and a PGS track (22) are zlib-compressed, which CoreAV does not undo, so
  the track has no packets (`docs/design/mkv.md`); a partially encrypted
  WebM Opus track has 2,071 packets whose sizes `ffprobe` reports
  differently (it cannot decrypt them).
- Held-out: one packet, the first block of a Vorbis track stored at -3 ms,
  which `ffprobe` reports at -2 ms. Found by the held-out run and left as
  is.
- Edge (not gated): conformance and crash-test samples. 10 files are
  rejected as damaged where `ffprobe` reads part of them; tracks with codecs
  CoreAV does not name (VC-1, RealVideo, MS-MPEG4 v3, TTA) are dropped,
  which shifts the stream pairing; DTS and TrueHD profiles, which only their
  bitstreams carry.

MediaInfo (informational), on the development split: every
file probes on all three sides; where CoreAV and `ffprobe` disagree on a
gating field, MediaInfo sides with CoreAV on the stream durations in 21 of 33
streams (`KD-MKV-STREAM-DURATION`) and on the color range in both cases
(`KD-MKV-HEVC-RANGE`).

## Speed (informational)

`tools/mkv-adapter/bench.py` on the development split and the
MediaRecorder recordings (181 files, 26.5 GB), on the 8-core x86_64 VM that
ran the corpus, one command at a time, each the fastest of three runs, files
in the page cache. Both sides start a process per run. No bar applies: these
are not held-out numbers and make no claim.

| Task | Files | `ffprobe` median | CoreAV median | Median per-file speedup | `ffprobe` total | CoreAV total |
|---|---|---|---|---|---|---|
| Probe (`coreav probe` against `ffprobe -show_format -show_streams`) | 181 | 5.0 ms | 1.0 ms | 4.5x | 2.0 s | 0.21 s |
| Every packet (`mkv-adapter count` against `ffprobe -count_packets`) | 181 | 20.4 ms | 3.0 ms | 5.7x | 20.9 s | 2.7 s |

The packet task is not the same work on both sides: CoreAV lists packets
from the block headers without reading the frame bytes, while `ffprobe`
reads every packet's data. A caller that copies the frames (the remux)
reads them as well.

## Spend

R2 reads only, which have no egress fees: about 76 GB downloaded across all runs, a few thousand read requests (well under 0.01 USD).
No other infrastructure.
