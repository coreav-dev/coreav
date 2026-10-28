# CoreAV oracle harness

Test-only implementation of the black-box correctness oracles. It captures stream metadata and packets from `ffprobe` and video frame checksums from `ffmpeg -f framemd5`, then compares a candidate with the same schema. No CoreAV implementation exists yet. Independent FFmpeg captures validate the harness, not a future parser or decoder.

`coreav-oracle` is an unpublished Cargo workspace member. Its `coreav-oracle-json` binary reads the combined JSON schema documented below. Its sole direct dependency is `serde_json` (MIT OR Apache-2.0). `scripts/oracle.py` uses Python's standard library and launches the black boxes in test subprocesses. Nothing links FFmpeg libraries. Only binaries and public CLI documentation were used; no multimedia implementation source or archived prototype was read.

The crate, reference build and CI design were set when the harness was added. The root manifest change adds only this member; Cargo.lock records its dependency graph. This harness schema is independent of the future CoreAV public API.

## Existing level-by-level CLI

The level-by-level harness is retained: `coreav-oracle normalize`, `coreav-oracle compare`, `coreav-oracle selfcheck`, and `scripts/oracle-{capture,ffprobe,selfcheck}.sh` continue to use the separate per-level schema in [SPEC.md](SPEC.md). Those records are not interchangeable with the combined schema below.

The new fixture runner uses `coreav-oracle-json` for its combined capture, independent self-check and candidate adapters. Both binaries are built by `cargo build -p coreav-oracle`; use `--bin` when invoking either with `cargo run`.

## Run from the repository root

```bash
cargo build --locked -p coreav-oracle
scripts/install-oracle-ffmpeg.sh "$PWD/target/oracle/bin"
export PATH="$PWD/target/oracle/bin:$PATH"
export PYTHONDONTWRITEBYTECODE=1
COREAV_REQUIRE_ALL_FIXTURES=1 scripts/gen-fixtures.sh "$PWD/target/oracle/fixtures"
scripts/oracle.py self-check target/oracle/fixtures target/oracle/check
```

Output directories must be new. Reusing a directory fails rather than mixing stale captures with new files. The fixture input directory must have `MANIFEST` entries of the form `generated NAME`, with a unique basename per file. Missing files, malformed entries, and skipped required fixtures fail. The generator's two pre-existing optional interoperability cases (`h264_avc3.mp4`, `pcm_s16le.mp4`) can be marked skipped. No generated media is committed.

The installer downloads Linux x86_64 FFmpeg/ffprobe 7.0.2 from the static build publisher, verifies SHA-256 `abda8d77ce8309141f83ab8edf0596834087c52467f6badf376a6a2a4c87cf67`, and extracts only the two executables. These GPL test binaries are external tools, never Cargo dependencies or product components. Local capture can use different tools through `--ffprobe PATH` and `--ffmpeg PATH`; CI uses the pinned pair.

Self-check performs two independent complete captures, demands 100% agreement at all three levels and at least 1,000 video frames, then separately perturbs stream width, packet PTS, packet presence, packet grouping/size, frame checksum, probe status, and schema completeness. Every perturbation must exit with a comparison failure. It writes the positive report and seven negative reports, then restores the unperturbed candidate.

Capture without comparison:

```bash
scripts/oracle.py capture target/oracle/fixtures target/oracle/captured
```

Compare a directory supplied by an implementation:

```bash
scripts/oracle.py run target/oracle/fixtures target/oracle/run-dir \
  --candidate-dir /path/to/candidate-json
# Or compare two existing capture directories directly:
cargo run --locked -p coreav-oracle --bin coreav-oracle-json -- \
  target/oracle/check/reference target/oracle/check/candidate fixtures
```

Compare an executable candidate:

```bash
scripts/oracle.py run target/oracle/fixtures target/oracle/run-exe \
  --candidate-exe tools/oracle/ffprobe-candidate.py
```

The executable is called once per fixture as `EXECUTABLE ABSOLUTE_FIXTURE_PATH`, without a shell. It must exit zero and emit exactly one normalized JSON object on stdout, even for a reported media/probe rejection. Logs go to stderr. Nonzero exits, signals, invalid JSON, missing executables and timeouts are harness errors. The example executable independently invokes the same black boxes and normalizer; replace it with an adapter to a future implementation. An adapter can translate CoreAV output, but must preserve unknown values and report failures truthfully.

The orchestrator applies `--timeout SECONDS` (default 120) per subprocess. A timeout/signal is an infrastructure error and never becomes an excluded ordinary ffprobe failure. The example executable has a 120-second internal timeout too. This is an offline test tool for the trusted generated corpus; it is not a sandbox for hostile candidate executables or a production media service.

The `run` command writes `reference/`, `candidate/` (only with an executable), and `report.json`. It prints the report on stdout. Exit codes are 0 for agreement, 1 for a failed comparison, and 2 for invocation/infrastructure errors. The comparator reports unreadable or invalid per-file JSON as a gating schema diff and exits 1. Its own invalid arguments/directory access errors exit 2 with JSON on stderr.

## Normalized schema version 1

Each directory contains `FIXTURE_NAME.json`, including the media extension (for example `basic.mp4.json`). Every documented key below is required. Unknown/absent values are `null`; false and zero remain real values. Integers must fit signed 64 bits. Rational values are reduced two-element integer arrays `[numerator, positive_denominator]`; zero is `[0, 1]`. No floating-point tolerance or timestamp rounding is introduced. Array order may differ between captures; identities and per-track ordinals determine pairing.

Top-level object:

| Key | Type and meaning |
| --- | --- |
| `schema_version` | Integer `1` |
| `fixture` | Original fixture basename, equal to the JSON filename without its final `.json` |
| `probe_success` | Boolean, successful stream/format ffprobe exit |
| `packet_success` | Boolean, both presentation and media packet probes exited successfully |
| `frames_success` | Boolean, all video tracks decoded successfully; `null` for no video tracks |
| `format_duration` | Rational seconds from ffprobe `format.duration`, or `null` |
| `streams` | Array of stream objects; unique nonnegative `index` |
| `packets` | Array of packet objects; per-track unique nonnegative `ordinal` |
| `frames` | Array of video frame objects; per-track unique nonnegative `ordinal` |
| `warnings` | Array of diagnostic strings, not agreement fields |

Stream objects:

| Normalized key | ffprobe mapping | Level |
| --- | --- | --- |
| `index` | `index`; stream identity and count | Gating |
| `kind` | `codec_type` | Gating |
| `codec_fourcc` | `codec_tag_string`, preserving ffprobe's printable representation | Gating |
| `codec_name` | `codec_name` | Gating |
| `width`, `height` | Integer `width`, `height` | Gating |
| `rotation` | Clockwise quarter-turn degrees from `Display Matrix`; 0 if absent, `null` for other matrices | Gating |
| `time_base` | Reduced `time_base`; denominator is the timescale when numerator is 1 | Gating |
| `duration` | Rational seconds: `duration_ts × time_base` when available, otherwise parsed `duration` | Gating |
| `color_primaries` | `color_primaries` | Gating |
| `color_transfer` | `color_transfer` | Gating |
| `color_matrix` | `color_space` | Gating |
| `color_range` | `color_range` | Gating |
| `sample_rate`, `channels` | Integer `sample_rate`, `channels` | Informational |
| `language`, `handler_name` | `tags.language`, `tags.handler_name` | Informational |
| `aspect_ratio` | `sample_aspect_ratio` string | Informational |

Rotation normalization handles the exact unit-scale 0/90/180/270 matrices generated by the fixture script. General-angle rotations and transforms are currently unknown, rather than inferred from ffprobe's rounded angle. Extending this mapping is a follow-up for this harness before broader real-corpus use.

Packet objects:

| Normalized key | Mapping and meaning | Level |
| --- | --- | --- |
| `stream_index` | Integer `stream_index`, references an existing stream | Gating |
| `position` | Integer byte `pos`, or `null` if ffprobe cannot report it | Gating |
| `ordinal` | Zero-based packet sequence number within that track | Gating |
| `pts`, `dts` | Signed integer ticks from ordinary `-show_packets`, including ffprobe's presentation/edit behavior | Gating |
| `media_pts`, `media_dts` | Ticks from a separate `-ignore_editlist 1 -show_packets` probe, paired by track/byte position | Gating |
| `pts_offset`, `dts_offset` | Presentation tick minus corresponding media tick; `null` if either is unknown | Gating |
| `size` | Integer `size` in bytes | Gating |
| `keyframe` | `flags` contains `K` | Gating |
| `duration` | Integer `duration` in track ticks, or `null` | Informational |
| `discard` | `flags` contains `D` | Informational |

The comparator pairs packets by `(stream_index, position)`, preserving occurrence order when multiple packets share that position. With an unknown position it pairs unknown-position packets in track occurrence order and still gates their ordinals. Missing and extra packets enter the denominator as disagreeing packets. Size, count, byte positions and ordinals detect merging/splitting/grouping differences. Stream-count differences also make that file's packets disagree.

The two packet views expose observable edit-list effects and PTS/DTS offsets. ffprobe does not expose the raw edit-list entries in these CLI outputs, and its media view is still ffprobe's interpretation of timestamps. This harness does not claim to recover `elst` or the stored `ctts` table. A future CoreAV adapter must map its raw and presentation timestamp models explicitly; any spec-justified difference needs a documented known divergence. There are no blanket mismatch exemptions in this comparator.

Video frame objects:

| Key | Mapping |
| --- | --- |
| `stream_index` | Source track index (each track is decoded separately with `-map 0:INDEX`) |
| `ordinal` | Zero-based decoded-frame sequence number in that track |
| `time_base` | Rational `#tb 0:` header from framemd5 |
| `dts`, `pts`, `duration`, `size` | Integer columns of the framemd5 row |
| `md5` | Lowercase 32-character decoded-frame checksum |

Frames are paired by `(stream_index, ordinal)`; all frame keys gate. Capture uses `-noautorotate`, `-fps_mode passthrough`, `-threads 1` and `-bitexact`, preserves decoded pixel format, and excludes subtitles, data and audio from video-frame comparisons. Audio streams and their compressed packets still participate in the stream/packet levels. Candidate adapters must hash the same raw decoded pixels, layout and pixel format that the reference's framemd5 muxer receives. Partial frame output is retained with `frames_success: false` and a warning; it is excluded from agreement and reported as a reference decode failure. A candidate decode status differing from the reference always fails.

## Gates and reports

| Profile | Streams | Packets | Video frames |
| --- | --- | --- | --- |
| `fixtures` | 100% agreeing files | 100% agreeing packets | 100% and at least 1,000 compared frames |
| `real` | At least 98%, at least 100 input files | At least 99% | 100% and at least 1,000 compared frames |

Stream agreement uses agreeing files divided by files minus reference ffprobe stream failures. Packet agreement uses agreeing packets divided by agreeing plus disagreeing packets. An empty eligible stream or packet denominator fails; frame counts below 1,000 fail. Probe/packet/decode status mismatches, malformed schemas, missing JSON files and unexpected JSON files always fail, including in `real` mode. A failed packet oracle on an otherwise successfully probed file fails the harness rather than silently shrinking packet coverage. Informational diffs are emitted but do not reduce agreement. These thresholds are the project's correctness bars, unchanged; no tolerance thresholds were added.

Reports include exact counts, agreement percentages, excluded reference failures, `passed`, and `diffs` with fixture name, JSON-pointer location, expected/actual values and a `gating` flag. Paired diffs use the reference array's index; an extra record uses its candidate index. A schema error uses the diagnostic location `/schema`.

The `real` comparator profile is ready for privately captured JSON directories. This PR neither supplies nor claims a real-file corpus result. See [corpus rules](../../docs/corpus.md); files must remain unmodified and outside git. The capture runner currently consumes the generated fixture text manifest, not the private corpus JSON manifest.

## Verification and CI

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
~/.cargo/bin/cargo-deny check
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools/oracle/tests -p 'test_*.py'
```

The Rust tests cover informational fields, missing/extra packets, position pairing, signed timestamp/edit offsets, reference failures, malformed schemas, and exact threshold boundaries. Python tests cover tick precision, rotation direction, packet-track pairing, framemd5 parsing and manifest validation. End-to-end self-check validates real black-box outputs and mutation detection.

The new `fixtures.yml` installs/caches the pinned binaries, generates all required fixtures on every run even after a cache restore, caches generated files, builds the comparator, runs capture tests and the independent self-check, checks the executable candidate adapter, and uploads normalized JSON/diffs. It does not change the existing workflow. The frame minimum is met by generated fixtures (1,293 video frames with the pinned tool pair in the local run).

Follow-ups: define spec-justified divergence entries when a real candidate exists; extend general-angle transform mapping and private corpus manifest ingestion; run the real-file corpus once available. The harness does not implement or certify CoreAV decoding.

Public CLI references: [ffprobe documentation](https://ffmpeg.org/ffprobe.html), [framemd5 format](https://ffmpeg.org/ffmpeg-formats.html#framemd5), [static binary publisher](https://johnvansickle.com/ffmpeg/).
