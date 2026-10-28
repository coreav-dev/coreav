# Probe metadata against ffprobe 7.0.2

Maintainer-run evidence for probe metadata, logged in the
repository as the oracle policy allows for real-corpus criteria. Run on 2026-10-15, locally (no cloud host
spend), on code at commit `e6891e3` of the `stage3/probe-metadata` branch.
Re-run at commit `8bd9c0b` (stricter `esds` bounds, fragment byte-count
overflow handling): the adapter output for all 746 files is byte-identical.

## Setup

- Host: 8 vCPU Intel Xeon, Linux 6.12.
- Oracle: John Van Sickle static `ffprobe` 7.0.2, binary SHA-256
  `4f231a1960d83e403d08f7971e271707bec278a9ae18e21b8b5b03186668450d`. The
  pinned archive download in `scripts/install-oracle-ffmpeg.sh` returned an
  error page that run, so the binary was copied from an earlier install of
  the same archive; three earlier installs have identical binary hashes.
- Fixtures: `scripts/gen-fixtures.sh` with `COREAV_REQUIRE_ALL_FIXTURES=1`.
- Real corpus: corpus v1, development split only: `real`
  files with `holdout: false` and an `mp4`, `mov`, `m4a`, `m4v`, `m4b` or
  `3gp` extension, 747 files. No held-out file was downloaded or probed.
- Scripts: `tools/isobmff-adapter/corpus/harvest.py` (download, SHA-256 check,
  ffprobe on the full file, sparse copy with top-level `mdat` payloads as
  holes), `compare.py` (normalize, run the adapter, `coreav-oracle compare
  --profile corpus --levels streams`) and `summarize.py`.

```bash
harvest.py target/oracle/bin/ffprobe R2_ENV_FILE HARVEST_DIR   # manifest.json in HARVEST_DIR
compare.py target/release/coreav-oracle target/release/isobmff-adapter HARVEST_DIR RUN_DIR
summarize.py RUN_DIR
```

## Fixtures

The fixture workflow steps run locally (`oracle-capture.sh`, `coreav-oracle
compare --profile fixtures`, `scripts/oracle_kd.py`):

| Check | Result |
|---|---|
| Profile and level rate | 29 of 29 files (100%) |
| Stream and packet oracles, in scope, after known divergences | passed, 0 uncovered (unchanged from `main`) |
| Self-check (`scripts/oracle-selfcheck.sh`), new perturbations | `profile=null` (31/31), `level=null` (21/21), `bit_rate+1` (36/36) detected |

## Real corpus, development split

746 of 747 files compared. One object
(`184becaaec565182ef8d7d28eb682100b1d98dc2f8983063405f131320cb2a78.mp4`) did not
match its manifest SHA-256 on two downloads and was excluded; it is reported
as an open issue, not counted.

### Profile and level (gated: at least 98%)

| Run | Pairing | Files agreeing | Rate |
|---|---|---:|---:|
| First run | by array position | 635 / 709 | 89.56% |
| Second run | by stream index | 660 / 677 | 97.49% |
| Final (the rules as amended) | by stream index, streams both tools list, parameter set wins | **674 / 680** | **99.12%** |

The main changes between runs are in the pairing column. The
final rate leaves out 63 streams that carry a profile or level but are
listed by one tool only (streams CoreAV drops as not allowlisted, such as MPEG-4 Visual, and cover pictures `ffprobe` lists as `mjpeg` streams); the
stream count field of the stream oracle already gates them. No file fails
the stream oracle on profile or level alone (27 files have a profile or
level difference among other gating differences).

The six remaining disagreements, all AAC profile:

| Count | `ffprobe` | CoreAV | Reason |
|---:|---|---|---|
| 3 | `HE-AAC` | `LC` | The `AudioSpecificConfig` declares object type 2 at a 24 kHz core rate with no SBR signalling (implicit signalling); `ffprobe` finds SBR by decoding. CoreAV does not decode. |
| 3 | none | `LC` | Valid two-byte LC configurations (object type 2); `ffprobe` prints no profile for these streams. CoreAV follows ISO/IEC 14496-3. |

Both kinds are registered as known divergences: `KD-AAC-IMPLICIT-SBR` and `KD-AAC-LC-UNNAMED` in
`docs/known-divergences.md`. They cover all six disagreements above; the
direct rate (674 of 680) is unchanged and is still the reported figure.

### Bit rate (informational)

CoreAV's exact bit rate rounded to an integer, against `ffprobe`'s stream
`bit_rate`, for streams where both report one:

| Files | Streams | Exact | Within 0.1% | Within 1% | Median relative difference |
|---|---:|---:|---:|---:|---:|
| Regular, all | 1,065 | 490 | 943 | 1,014 | 1.8e-8 |
| Regular, video | 581 | 275 | 558 | 564 | 6.4e-9 |
| Regular, audio | 414 | 161 | 331 | 396 | 7.8e-6 |
| Fragmented, all | 57 | 2 | 2 | 2 | 87 (8,700%) |

- Regular files: most differences come from a different duration: CoreAV
  divides by the presentation duration after the edit list,
  `ffprobe` by its own duration (for example AAC priming).
- Fragmented files: `ffprobe`'s stream value is far below CoreAV's. Against
  `ffprobe`'s own format bit rate (file size × 8 / format duration), on the
  55 fragmented files that keep every stream: the sum of CoreAV's stream bit
  rates is within 5% of it on 52 files, while the sum of `ffprobe`'s stream
  bit rates is within 5% on 6 (median 1.1% of the format rate), so `ffprobe`
  appears to count only part of the samples. Of CoreAV's three files outside
  5%, one is at 0.89 and two are 51 and 64 times the format rate: there
  CoreAV's observed sample span is about 2 s while `ffprobe`
  reports 102 s and 140 s, and the bit rate follows the stream duration by
  definition. That duration difference belongs to the stream oracle, not to
  this change. 24 more fragmented files have every stream dropped
  and so report no bit rate. Not gated, no divergence recorded.
- Known limitation: QuickTime version 0 sound entries store one `stsz` byte
  per audio frame for uncompressed audio, so 8-bit stereo PCM reports half
  the payload rate (one file). The definition (stored sample sizes) is kept.

Declared bit rates on video and audio streams: `btrt` on 197, `esds` only on
336, none on 519.

### Creation time (informational)

Stream creation time (`mdhd`) against `ffprobe`'s stream tag: 608 equal, 528
unset on both sides, 7 differ by exactly 66 years. In those 7 the stored
value looks like seconds since 1970 written into the 1904-based field;
`ffprobe` shows the later date, CoreAV reports the value as stored
(ISO/IEC 14496-12 8.4.2).

## Robustness

- Unit and integration tests: `crates/coreav-isobmff/src/codec.rs` (parsers,
  every prefix of every record), `tests/metadata.rs` (profiles, bit rates,
  creation times, retained-bytes and work limits, every truncation of each
  configuration box).
- The `probe` fuzz target now also evaluates the bit rate and creation times
  and asserts the codec configuration bounds. `cargo fuzz` could not build
  locally (no C++ compiler for libFuzzer); CI runs it for 60 seconds. As a
  local substitute, a throwaway mutation loop (not committed) ran the same
  checks over the 29 fixture files under 200 kB for 300 s: 64.7 million
  inputs, 14.6 million probed, no panic and no input above 2 s. A first,
  shorter run stopped on one input above 2 s while builds ran in parallel;
  the same deterministic input sequence ran below 2 s in the 300 s rerun.

Machine-readable results: `0025-probe-metadata/summary.json`,
`tables.json` and `metadata-report.json` (file names are manifest SHA-256
digests; no media content).
