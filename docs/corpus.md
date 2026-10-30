# Corpora

CoreAV is measured against a private measurement corpus of real media files,
against one synthetic set of browser recordings, and against one set of real
command lines. This page is the public record of what those measurements
rest on: for each set, what it is for, how its files were selected by their
structure, the oracle that sorts them, the holdout rule and the counts it
reached. What is not here: the files, any per-file record, and anything about
how the files were obtained.

The media corpora are private and measurement-only:

- No media file is committed to git, and no corpus is published or
  redistributed. Neither is any per-file artefact of one: media, frames,
  thumbnails or crops.
- Reports publish aggregate numbers and structural facts about the files
  (size, extension, set, codec, container and the like), never the
  file.

A reproduction outside the project's storage needs its own corpus. Files
an outside contributor sends follow [`CONTRIBUTING.md`](../CONTRIBUTING.md):
only files you may share, no identifiable people without consent, and
never committed to the repository.

| Corpus | For | `real` files | Held out |
|---|---|---:|---:|
| [v1](#corpus-v1) | the oracles and the speed measurements in general | 1,351 | 407 |
| [v1-fmp4-large](#top-up-v1-fmp4-large) | adaptive routing of large fragmented MP4 | 21 | 6 |
| [v1-ts-audio](#top-up-v1-ts-audio) | the MPEG-TS demux, the audio containers | per family, below | per family |
| [v2-heldout](#v2-heldout) | held-out confirmation runs, in draws | 40, 46, 46, 52, 37 and 300 | all |
| [mediarecorder-v1](#mediarecorder-recordings) | the Matroska and WebM run | 41 (synthetic) | none |
| [commands-v1](#command-coverage-set-commands-v1) | the compatibility command | 1,147 commands | none |

## Rules the media corpora share

Each corpus below states its own rules in full, before collection. Most
of them are these, and the sections below name where a corpus differs.
Files are named by content (the SHA-256 and the extension) and stored
unchanged: never re-muxed, trimmed or re-encoded. Each corpus has a manifest
that holds, per file, the hash, size, extension, set, structural
tags and the oracle's outcome (and, for `v2-heldout`, the operation a draw
was selected for).

**Sets.** Every file is in exactly one set:

- `real`: files that the pinned `ffprobe` 7.0.2 reads without error and that
  count toward the minimum of 100 real files behind the real-file oracle
  thresholds (98% of files for the stream oracle, 99% of packets for the
  packet oracle).
- `edge`: conformance vectors, deliberately odd files, single DASH or HLS
  initialization and media segments, and any `real` candidate that `ffprobe`
  rejects. A top-up that serves one operation also puts here what that
  operation does not read.
- `hostile`: files on which `ffprobe` crashes (is killed by a signal) or
  runs for more than 30 s.

**Oracle.** Each file is probed once with `ffprobe -show_format
-show_streams -show_chapters -count_packets` under a 30 s timeout. In
corpus v1, a file over 1 GB that times out is probed again without
`-count_packets` before it is classified as `hostile`.

**Diversity.** A file whose structural signature is already represented is
skipped. The signature is defined per corpus; it is made of structural facts
only (container, brands, codecs, profiles, sizes, tags).

**Personal data.** The manifests do not record faces, GPS positions or device
serials, and that does not mean a file is free of them. This holds only
because the corpora are private; any corpus, or part of one, that is published
or redistributed must have consent for identifiable people and mark personal
data per file.

**Holdout.** A holdout is assigned once, before anyone benchmarks. Files
are ordered by `sha256(<salt> + <file sha256>)`, lowercase hex,
ascending, within each group (a group of the corpus, a container family or a
draw's class). Each corpus has its own salt, fixed before collection or
before any benchmark:

| Corpus | Salt | Held out |
|---|---|---|
| v1 | `coreav-corpus-v1-holdout:` | first `round(0.3 × n)` of each `real` group |
| v1-fmp4-large | `coreav-corpus-v1-fmp4-large-holdout:` | first `round(0.3 × n)` of each `real` group |
| v1-ts-audio | `coreav-corpus-v1-ts-audio-holdout:` | first `round(0.3 × n)` of each container family's `real` files |
| v2-heldout draw 1 | `coreav-corpus-v2-heldout:` | every `real` file |
| v2-heldout draw 2 | `coreav-corpus-v2-heldout-draw2:` | every `real` file |
| v2-heldout draw 3 | `coreav-corpus-v2-heldout-draw3:` | every `real` file |
| v2-heldout draw 4 | `coreav-corpus-v2-heldout-draw4:` | every `real` file |
| v2-heldout draw 5 | `coreav-corpus-v2-heldout-ts:` | every `real` file |
| v2-heldout draw 6 | `coreav-corpus-v2-heldout-compat:` | every `real` file |
| v2-heldout draw 7 | `coreav-corpus-v2-heldout-ts-speed:` | every `real` file |
| v2-heldout draw 8 | `coreav-corpus-v2-heldout-audio-decode:` | every `real` file |

Held-out files are used only for reported numbers, never for profiling
or tuning. Where a whole draw is held out, the salt still orders each
class, so that any later partition of it is fixed before it is drawn.

**Spent.** A held-out set is spent once a number is reported from it:
the same inputs cannot report the result of a change they motivated. A
spent set becomes a tuning pool. It may be profiled against, and no
reported number may come from it. A frozen corpus is never extended; a
new need gets a new top-up with a new salt.

## Corpus v1

The real-file corpus that the oracles and most speed measurements run on.
The structural diversity the oracles need (edit lists, fragmentation, HDR,
ProRes, telemetry, chapters, many encoders) is wider than own recordings
reach, so v1 holds files the project may measure but may not redistribute.
v1 is frozen.

The sets, oracle and holdout are the shared rules above.

### Size and coverage

The full report, with every structural tag, is
[`corpus-coverage.md`](corpus-coverage.md). Files per set:

| Set | Files | GB |
|---|---:|---:|
| `real` | 1351 | 217.8 |
| `edge` | 1619 | 29.8 |
| `hostile` | 0 | 0.0 |

- **Held out:** 407 of the 1,351 `real` files, as the manifest marks
  them. The rule applied to the corpus's groups gives 406; the difference
  is the manifest's, and the measurements used the manifest's marks.
- **Oracle outcomes:** `real` 1,325 ok and 26 ok with warnings; `edge`
  1,336 ok, 213 ok with warnings and 70 errors.
- **Gaps**, structures with fewer than 5 files in the whole `real` set:
  media segment without a movie box (4 real, 21 edge), Dolby Vision (3,
  15), MV-HEVC or stereo (0, 21), ProRes Proxy (0, 1), ProRes LT (1, 3),
  ProRes 422 (2, 3), ProRes 4444 XQ (0, 1), vendor telemetry (1, 0),
  interlaced (3, 70), Motion JPEG 2000 (0, 1), E-AC-3 (4, 7).

Not in v1 at all: transport streams (`.ts`, `.m2ts`, `.mts`), WAV, FLAC
and ADTS AAC; Ogg Opus and Ogg FLAC are few. `v1-ts-audio` was collected
for them.

### What has spent it

- The held-out set for the `hls index` comparison: run `w1-01` of
  [`spikes/w1-remeasure/README.md`](spikes/w1-remeasure/README.md) used it. The replacement is
  `v2-heldout`.
- The thumbnail and decode bars were measured on v1, and every
  change since was made in answer to those runs, so another run of v1 is
  no estimate for files the code has not seen. The replacement is
  `v2-heldout` draw 4.

## Top-up v1-fmp4-large

Large, natively fragmented MP4 and CMAF single files with a leading
segment index (`sidx`), for the adaptive-routing measurement
([`spikes/spike-a/h7-adaptive/README.md`](spikes/spike-a/h7-adaptive/README.md)).
Corpus v1 had too few large fragmented MP4s for its large-object claim,
whose floor is 250 MB. v1 is frozen, so this is a separate set under v1's
rules, with these:

- **Selection.** Fragmented MP4 or CMAF single files whose top-level
  boxes put at least one `sidx` before the first `moof`, 100 MB to 4 GB.
  Target 40 files, at least 20 over 250 MB, at most 60 GB in total.
- **Diversity.** At most 3 files that show the same content. A file whose
  structural signature is already in the top-up is skipped; the signature
  is the brands, the codec strings of every sample entry, the `sidx` layout
  (number of leading `sidx` boxes, reference ids, versions, hierarchy) and
  the subsegment count band (1–32, 33–100, 101–200, 201–500,
  501–1000, over 1000).
- **Not taken.** Encrypted files (every stream would be dropped by the
  codec allowlist, so the index lists nothing and the file cannot be
  gated), files without a leading `sidx`, and files that could not be read at a usable rate.
- **Sets.** Only `real` files are kept.
- **Holdout.** Salt `coreav-corpus-v1-fmp4-large-holdout:`, 30% of each
  `real` group, assigned before any benchmark.

### Result

21 `real` files, 10.25 GB; 16 are over 250 MB (102 MB to
1.96 GB). The 40-file target was not reached: the diversity rules bind
before candidates run out, because the large single-file candidates
available are few distinct contents with several files each, and many
candidates have no leading `sidx`.

- **Held out:** 6 files (102, 299, 340, 372, 407 and 523 MB); 5 of them
  are at or above the 250 MB floor.
- **Codecs:** H.264 (17 files, Main and High profiles, levels 3.0 to 5.1)
  and HEVC (4 files); one file also carries AAC. Subsegments per file: 4
  to 372.
- **Not taken**, of 42 candidates: no leading `sidx` 7, a title already at
  its cap 3, signature already represented 2, `ffprobe` error 1 (kept out
  of `real`), and the rest by the diversity caps.

## Top-up v1-ts-audio

MPEG transport streams and the common audio containers, for the MPEG-TS
demux ([`design/mpegts.md`](design/mpegts.md)) and the audio
containers. v1 is frozen, so this is a separate set under v1's
rules. The rules were fixed before collection.

- **Segments.** A segmented stream contributes its media segments
  unchanged: the first media segment of a rendition, and the init segment
  where there is one. Files over 200 MB need a reason in the manifest.
- **Targets.** At least 75 `real` transport streams (`ts`, `m2ts`, `mts`,
  `m2t`); at least 40 `real` files each of WAV (RIFF and RF64), native
  FLAC and ADTS AAC; Ogg Opus and Ogg FLAC to at least 15 and 10 `real`
  files counting v1's. A target the rules do not allow is not reached, and
  the shortfall is reported.
- **Diversity**, per container family (transport stream, WAV, FLAC, ADTS,
  Ogg): a file whose structural signature is already in the family is
  skipped. The signature is the container variant (packet size, programs,
  PCR layout), the stream types and codecs, and the encoder or packager
  family.
- **Sets.** A file `ffprobe` reads is `real` if it is a recording or an
  encode, `edge` if it is a conformance vector or if `ffprobe` rejects it;
  a crash or a timeout makes it `hostile`.
- **Holdout.** Salt `coreav-corpus-v1-ts-audio-holdout:`, 30% of each
  container family's `real` files, assigned before any benchmark or
  oracle run of CoreAV on these files.

### Result

485 files, 4.92 GB.

| Family | `real` (held out) | Target | `edge` |
|---|---:|---:|---:|
| Transport stream | 57 (17) | 75 | 55 |
| WAV | 39 (12) | 40 | 93 |
| FLAC | 37 (11) | 40 | 103 |
| Ogg (Opus, FLAC) | 16 (5) | 25 | 43 |
| ADTS AAC | 8 (2) | 40 | 34 |

- **Not taken:** 139 candidates with a signature already in the family, 43
  over 200 MB and 11 duplicates, besides those over the diversity caps.
- **Shortfalls**, kept rather than met by relaxing a rule. The ADTS AAC
  files that qualify are almost all conformance vectors (`edge`). The
  transport streams and Ogg files stop at the diversity and signature
  rules. v1's Ogg files (38 `real`, mostly Vorbis and Theora) and MP3
  files (55 `real`) stay as they are.
- **Spent:** the transport-stream holdout was compared once and is spent
  ([`measurements/ts-demux.md`](measurements/ts-demux.md)).
  Its replacement is `v2-heldout` draw 5.

## v2-heldout

Held-out top-ups, collected in draws, each fixed before its first file
was fetched and each held out whole. A draw is spent once a number is
reported from it. Each draw is selected for one operation, which the
manifest carries per file as `operation`, and a run takes only an
unspent draw of its own operation: no draw is a holdout for another
draw's operation.

| Draw | For | Operation | `real` | Files | GB | Status |
|---|---|---|---:|---:|---:|---|
| 1 | `hls index` | `hls` | 40 | 96 | 4.85 | spent: runs `w2-01` to `w2-04` of [`spikes/w2-hls-index/README.md`](spikes/w2-hls-index/README.md) |
| 2 | `hls index` and packet index, re-measurement | `hls` | 46 | 121 | 3.32 | spent: run `w3-01` of [`spikes/w2-hls-index/README.md`](spikes/w2-hls-index/README.md) |
| 3 | `hls index` and packet index | `hls` | 46 | 102 | 3.13 | spent: run `w4-01` of [`spikes/w3-sample-walk/README.md`](spikes/w3-sample-walk/README.md) |
| 4 | thumbnail and decode | `thumbnail` | 52 | 80 | 1.71 | spent: the joint confirmation of [`spikes/thumbnail-bench/README.md`](spikes/thumbnail-bench/README.md) |
| 5 | MPEG-TS demux | `ts-probe` | 37 | 48 | 1.7 | spent: [`measurements/ts-demux.md`](measurements/ts-demux.md) |
| 6 | the 0.1.0 bars R2 to R4 | `compat` | 300 | 332 | 13.62 | unspent |
| 7 | MPEG-TS demux speed, round 2 | `ts-probe` | | | | rules committed, not yet collected |
| 8 | Opus speed, round 2 | `audio-decode` | | | | rules committed, not yet collected |

No `hls` draw is unspent, so the next `hls index` measurement needs a
draw of its own before it can run.

### Rules of draws 1 to 3

- **Size.** Files over 200 MB are not taken: the comparison runs many
  rounds per input, and the set must fit one machine's disk.
- **Coverage.** The groups the `hls index` comparison reports. Target 60 to
  100 `real` files in the draw, at least 10 in each group and at most 20. A
  group the rules cannot fill is not filled.
- **Usable by the operation.** A `real` file must be one `coreav hls` can
  package: an ISO base media file (`mp4`, `mov`, `m4v`, `3gp`, `m4a`,
  `m4b`) whose video, if it has any, is H.264, HEVC, AV1 or VP9, or whose
  audio alone is AAC. Others are `edge`.
- **Diversity**, per draw: a file whose structural signature is already in
  the draw is skipped. The signature is the container brands, the streams'
  codecs, profiles and sizes (channels and sample rates for audio), the
  structural tags (`elst`, fragmented, `sidx`, a compressed movie box) and
  the encoder family.
- **Oracle and sets** as shared, with files outside the operation's
  inputs in `edge`.
- **Holdout:** the whole draw, with the salts listed above.

### Draw 1

40 `real` and 56 `edge` files, 4.85 GB. The `edge` files are mostly WebM
and Matroska, which `ffprobe` reads and `coreav hls` does not package.

- **Shortfalls.** Some groups stayed under their target of 10 (one was empty). The draw reached
  40 `real` files against 60 to 100. Single segments have no movie box and
  are not inputs for this operation.

### Draw 2

46 `real` and 75 `edge` files, 3.32 GB.

- Six files carry a segment index and seven are fragmented.
- **Not taken:** 173 duplicates of a file the corpus already holds, 86 over the
  cap on one kind of capture, 39 repeating a structural signature and 11 over 200 MB,
  besides those over the diversity caps and those of a group already at its
  ceiling of 20.
- **Shortfalls.** Two groups stayed well under their target of ten. The
  draw reached 46 `real` files against 60 to 100.
- Two rules were not held to; see
  [Rules withdrawn or not held](#rules-withdrawn-or-not-held).

### Draw 3

Collected for the packet-index comparison
([`design/sample-tables.md`](design/sample-tables.md)) under draw 2's
rules. The groups are filled in an order that keeps one kind of file from
filling one group first; caps and targets are unchanged.

46 `real` and 56 `edge` files, 3.13 GB (2.96 GB of `real` files).

- **Shortfalls.** Two groups are empty or nearly so and carry no verdict in
  the measurement. The draw reached 46 `real` files against 60 to 100.
- **Four audio-only files are refused by `coreav probe`**, each with a
  top-level box that runs past the end of the file at the offset the
  error names, which `ffprobe` reads. This is the covered divergence
  `KD-STRICT` ([`known-divergences.md`](known-divergences.md)). Draw 2 had
  three of the same shape. They stay in the draw and are reported apart.

### Draw 4

The held-out set for the thumbnail and decode measurements
([`design/thumbnails.md`](design/thumbnails.md)). Draws 1 to 3 are no
replacement for v1 here: a draw selected for what `coreav hls` packages
is not a sample of what `coreav thumbnail` decodes. The draw has its own
rules, groups and salt.

- **Size and form:** the 200 MB ceiling and files unchanged, as draws 1 to 3.
- **Usable by the operations.** A `real` file is one `coreav thumbnail`
  reads: an ISO base media file (`mp4`, `mov`, `m4v`, `3gp`) or a
  Matroska or WebM file (`mkv`, `webm`), whose video stream the thumbnail
  would take (the highest resolution, the first on a tie) has a codec on
  the default decode allowlist: H.264, HEVC, AV1, VP9, VP8 or ProRes
  ([`design/decode.md`](design/decode.md)). Everything else is `edge`,
  audio-only files included.
- **Groups.** Target 80 to 120 `real` files, at least 10 and at most 30
  per group.
- **Codecs.** No codec over 60% of the draw's `real` files, and at least
  5 files each of H.264, HEVC, AV1, VP9 and ProRes where candidates allow
  one. VP8 is counted where it turns up, with no target.
- **Durations.** `short` is a presented duration under 60 s, `long` is
  60 s or more, read from the stream the thumbnail takes or else from the
  container. Neither bucket over 70% of the `real` files, at least 20 in
  each. A file that states no duration is `unknown`, in no bucket.
- **The structural signature holds across draws.** A file repeating a
  signature of draws 1 to 3, or of this draw, is skipped. The signature is
  draw 2's, with pixel formats: the container brands, the streams'
  codecs, profiles, sizes and pixel formats, the structural tags and the
  encoder family. A Matroska or WebM file has no ISO boxes, so its
  codecs, sizes and encoder key carry the signature.
- **Nothing the decoder has already seen.** A candidate is dropped when
  its SHA-256 is in v1, `v1-fmp4-large`, `v1-ts-audio` or draws 1 to 3.
- **The collection runs no measurement.** `ffprobe` was the only tool run
  over the files; no `coreav` command ran on the draw before the
  measurement that opened it.

80 files, 1.71 GB: 52 `real` and 28 `edge`.

| Codec | `real` | Target |
|---|---:|---:|
| H.264 | 31 | 5+ |
| HEVC | 7 | 5+ |
| VP8 | 7 | — |
| VP9 | 4 | 5+ |
| AV1 | 3 | 5+ |
| ProRes | 0 | 5+ |

- **Durations:** 33 `short`, 19 `long`, against 20 or more each. No codec
  is over 60% (H.264 is 31 of 52, at its cap) and neither bucket is over
  70%.
- **Containers:** 13 of the 52 `real` files are Matroska or WebM, 7 of
  them VP8.
- **Not taken:** 146 candidates of a group already at its ceiling of 30, 58 repeating a structural signature, 37 over the
  codec share, 7 duplicates and 6 over 200 MB, besides those over the
  diversity caps and those already held by an earlier corpus.
- **Shortfalls.** 52 `real` files against 80 to 120; two groups are empty or
  nearly so. ProRes has no file, AV1 three and VP9 four; the report
  carries no AV1, VP9 or ProRes verdict from this draw. The `long` bucket
  misses its bar by one file.

### Draw 5

The replacement holdout for the MPEG transport stream demux
([`design/mpegts.md`](design/mpegts.md)), after the `v1-ts-audio`
transport-stream holdout was spent. Operation `ts-probe`. The rules were
committed before any file was fetched.

- **Form:** files unchanged, at most 200 MB.
- **Coverage.** At least 30 `real` transport streams (`ts`, `m2ts`, `mts`,
  `m2t`), the number the design note's bars named before collection, with
  at least 5 in each of the two groups the speed report gates on; the
  third group is reported at whatever it reaches.
- **Nothing the reader has already seen.** A file whose SHA-256 is in
  `v1-ts-audio` or an earlier draw is dropped.
- **Diversity.** A file whose structural signature is already held is
  skipped, across draws and against `v1-ts-audio`'s transport streams (111
  files, 105 distinct signatures, as the draw counted them; that top-up's
  table above counts 112). The signature is the container variant (packet
  size, the number of programs, whether the clock rides on a stream's own
  PID), the stream types and codecs with their profiles and picture sizes,
  and the encoder or packager family.
- **Holdout:** every `real` transport stream, salt
  `coreav-corpus-v2-heldout-ts:`.

**Result.** 48 files, 1.7 GB: 37 `real`, every one held out, and 11 `edge`. All three bars of the coverage
rule are met, one of them by a single file.

- **Not taken:** 44 over 200 MB, 27 bytes the draw already held and 15
  repeating a structure `v1-ts-audio` holds, besides those over the
  diversity caps and 17 after a group had its 20 (a ceiling the collector
  fixed with these rules, which the written rules did not state).
- **The extension list.** The candidate listing matched a wider list than
  the rules (`tsv`, `tsa` and `trp` too). Three `.tsv` tables taken as
  `edge` were struck from the draw after the collection. Widening the
  rules to admit more files after seeing the draw is what pre-registration
  forbids.

The draw was opened once, on 2026-10-21, and is spent.

### Draw 6

The held-out draw of the 0.1.0 bars R2, R3 and R4
([`design/compat-cli.md`](design/compat-cli.md), "The held-out draw"),
operation `compat`, salt `coreav-corpus-v2-heldout-compat:`. The rules are
that note's, committed with it before any file was fetched; the collector's
choices below, where the note leaves one open, were committed here before
the first fetch.

- **Form:** files unchanged, at most 200 MB.
- **Families**, by what the pinned `ffprobe` names the container:
  `mov,mp4,m4a,3gp,3g2,mj2` is ISO base media, `matroska,webm` is Matroska
  and WebM, and `mpegts` is a transport stream. Candidates are taken only
  with an extension the rules list (`mp4`, `mov`, `m4v`, `m4a`, `3gp`,
  `mkv`, `webm`, `ts`, `m2ts`, `mts`). A file whose container is not
  the family its extension names is `edge`.
- **Coverage:** 60 to 100 `real` files per family. A family stops taking
  `real` files at 100; the families are collected side by side, and the
  draw stops when each family holds 100 or the candidates run out. A
  family under 60 is reported short, not filled by relaxing a rule.
- **Sets:** the shared rules. A file `ffprobe` reads without error and
  lists a stream in is `real`; a conformance vector, an ISO base media
  initialization segment (no sample) or media segment (no movie box), and
  a file `ffprobe` rejects are `edge`; a crash or a run over 30 s is
  `hostile`. A transport-stream media segment of an HLS stream is a
  whole transport stream and is `real`, as in `v1-ts-audio` and draw 5.
- **Nothing seen before:** a candidate whose SHA-256 is in corpus v1,
  `v1-fmp4-large`, `v1-ts-audio`, `mediarecorder-v1` or draws 1 to 5 is
  dropped.
- **Diversity:** draw 4's structural signature (the container brands, the
  streams' codecs, profiles, sizes, channels, sample rates and pixel
  formats, the structural tags and the encoder or device key), across this
  draw and draws 1 to 5. Draws 1 to 4 recorded that signature when they
  were collected; draw 5 recorded its own, so draw 4's is computed for its
  files with `ffprobe` before the collection starts.
- **No cap on a source or a device** beyond the signature: the note sets
  none. Candidates are taken in turn across their sources, so that no
  source fills a family first, and the result reports the largest share
  one source holds in each family.
- **Holdout:** every `real` file. Each family's `real` files are ordered
  by `sha256("coreav-corpus-v2-heldout-compat:" + sha256)`; R4's own
  order is the note's.
- **The collection runs no measurement:** `ffprobe` is the only tool run
  over the files, and no `coreav` command runs on the draw before the
  round that opens it.

**Result.** 332 files, 13.62 GB: 300 `real`, every one held out, and 32
`edge`; no `hostile` file. Every family reached its ceiling.

| Family | `real` | GB | Video codecs of the `real` files | Largest share of one source |
|---|---:|---:|---|---:|
| ISO base media | 100 | 3.68 | H.264 87, HEVC 3, AV1 3, others and audio only 7 | 34% |
| Matroska and WebM | 100 | 5.66 | VP9 64, VP8 26, AV1 8, H.264 2 | 94% |
| Transport stream | 100 | 4.28 | H.264 73, MPEG-2 18, audio only 6, others 3 | 18% |

- The 32 `edge` files are ISO base media initialization segments, which
  list streams and hold no sample.
- **Not taken:** 137 repeating a structural signature, 81 bytes an earlier
  corpus or this draw already held, 31 downloads that failed and 14 over
  200 MB, besides the candidates left unread once their family held 100.
- **One source holds 94% of the Matroska and WebM family.** The rules set
  no source cap, and few other candidates of that family carried a
  signature not already held. R2, R3 and R4 report that family as it is;
  its results speak mostly for one publisher's encodes.

### Draw 7

The held-out draw of the second round of the transport stream speed bars ([`design/mpegts.md`](design/mpegts.md), "Speed bars, round 2"), after draw 5 was spent. Operation `ts-probe`, salt `coreav-corpus-v2-heldout-ts-speed:`. The rules were committed before any file was fetched.

- **Form:** files unchanged, at most 200 MB (1 MB = 10^6 bytes).
- **Extensions:** `ts`, `m2ts`, `mts` and `m2t`, and nothing else.
- **Classes**, by size: `small` under 16 MB, `medium` 16 MB to under 64 MB, `large` 64 MB to 200 MB.
- **Coverage:** 40 to 60 `real` transport streams; at least 10 and at most 20 in each class, and at least 15 in `large`. A class stops taking files at 20, and the draw stops when each class holds 20 or the candidates run out. A class under its minimum is reported short, not filled by relaxing a rule.
- **Candidates:** the transport stream candidates of the listings of draws 5 and 6 that neither draw fetched, and new listings made under the same rules, taken in turn across their sources so that no source fills a class first; the result reports the largest share one source holds in each class.
- **Sets:** the shared rules. A file the pinned `ffprobe` reads without error and lists a stream in is `real`, run with `GCONV_PATH` as `scripts/oracle-ffprobe.sh` sets it; a file it rejects is `edge`; a crash or a run over 30 s is `hostile`. A transport stream media segment of an HLS stream is a whole transport stream and is `real`.
- **Nothing seen before:** a candidate whose SHA-256 or URL is in corpus v1, `v1-fmp4-large`, `v1-ts-audio`, `mediarecorder-v1` or draws 1 to 6 is dropped.
- **Diversity:** draw 5's structural signature, across this draw, draw 5 and `v1-ts-audio`'s transport streams.
- **Holdout:** every `real` file. Each class's `real` files are ordered by `sha256("coreav-corpus-v2-heldout-ts-speed:" + sha256)`.
- **The collection runs no measurement:** `ffprobe` is the only tool run over the files, and no `coreav` command runs on the draw before the round that opens it.

### Draw 8

The held-out draw of the second Opus speed round ([`design/audio-decode.md`](design/audio-decode.md), "Opus speed, round 2"). Corpus v1's held-out files with audio are spent: they are the sources of the FLAC, MP3 and Opus derived sets. Operation `audio-decode`, salt `coreav-corpus-v2-heldout-audio-decode:`. The rules were committed before any file was fetched.

- **Form:** files unchanged, at most 200 MB (1 MB = 10^6 bytes).
- **Usable by the operation:** the pinned `ffprobe` lists an audio stream, the first of which lasts 5 seconds to 30 minutes, and the pinned `ffmpeg` 7.0.2 decodes that stream to the derived set's FLAC intermediate without an error. Any other file is `edge`. The round re-encodes every source, so the source's own codec and structure never reach the decoder under test.
- **Classes**, by the container the pinned `ffprobe` names: `mov,mp4,m4a,3gp,3g2,mj2` is ISO base media, `matroska,webm` is Matroska and WebM, `ogg` is Ogg, and every other container (MP3, FLAC, WAV and the rest) is `other`. The round's own classes are the derived inputs' containers, MP4 and Matroska/WebM, of which every source gives one each; the draw's classes keep one kind of source from taking the draw.
- **Coverage:** 100 to 160 `real` files. A class stops taking files at 60, and the draw stops when it holds 160 or the candidates run out. A draw under 100 is reported short, not filled by relaxing a rule.
- **Caps:** at most 16 `real` files from one source (a site, an archive or a data set), which is 10% of a full draw and 16% of the smallest; at most 3 from one uploader, album, collection or recording device. Candidates are taken in turn across their sources, so that no source fills a class first, and the result reports the largest share one source holds.
- **No structural signature:** the caps stand in for it, since the round re-encodes every source.
- **Candidates:** new listings only. A candidate whose SHA-256 or URL is in corpus v1, `v1-fmp4-large`, `v1-ts-audio`, `mediarecorder-v1`, or in any listing or ledger of draws 1 to 7, is dropped, whether or not that draw fetched it.
- **Sets:** the shared rules, with the oracle run with `GCONV_PATH` as `scripts/oracle-ffprobe.sh` sets it. A file a device, an application or a publisher produced that meets the rule above is `real`; a conformance vector, a test signal or a file the oracle rejects is `edge`; a crash or a run over 30 s is `hostile`.
- **Holdout:** every `real` file. Each class's `real` files are ordered by `sha256("coreav-corpus-v2-heldout-audio-decode:" + sha256)`.
- **The collection runs no measurement:** `ffprobe` and the pinned `ffmpeg` 7.0.2's FLAC decode are the only tools run over the files, and no `coreav` command runs on the draw before the round that opens it.

### Rules withdrawn or not held

Each of these is recorded rather than corrected after the fact: taking
files out of a held-out draw after its numbers are seen would be choosing
a subset of it.

- **A draw-1 rule on how files were accepted** was replaced from draw 2
  on, because it left two groups empty or nearly so.
- **The diversity cap** was implemented as a flat count in draws 1 to 3
  rather than a share. Draw 2 reached 46 files, so the largest groups of
  files are 22% of it each. This matters for draw 2's audio-only class,
  which misses its comparison bar: 7 of its 12 measured inputs are
  recordings of one kind. Draws 4 and 5 implement the share.
- **The structural signature across draws** was stated for draw 2 but
  compared only within the draw. Five of draw 2's 46 `real` files repeat
  a draw-1 signature: four audio-only files and the single input
  of one group that the comparison measures, which is why draws 1 and 2
  report that group at 0.2711 and 0.2709. The five stay in draw 2.
- **The cross-draw signature for draw 3** was tried first and withdrawn
  before any file of draw 3 was measured: 23 of the 25 audio-only
  candidates it reached were skipped and that class stood at two files. The
  audio signature (brand, codecs, channels, sample rates, structural tags,
  encoder key) cannot tell apart what the comparison is sensitive to:
  sample count, chunking and table size. Draw 3 was collected again from
  scratch under draw 2's within-draw rule, before any speed number
  existed. Draws 4 and 5 hold the rule across draws.
- **Draw 4's first collection run was discarded** before the draw was
  finalized: the collector re-checked only one share for a candidate it
  had held back, so one group reached 44 against its ceiling of 30. The
  collector was fixed, nothing had been reported, and the draw was
  collected again from scratch.
- **Draw 5's first pass was discarded** when more candidates were added,
  and its candidate lists were narrowed to the four extensions the rules
  name.
- **Draw 6's oracle crashed on 30 transport streams** in the first part of
  the collection: the pinned static `ffprobe` loaded the collecting host's
  gconv modules ([`design/mpegts.md`](design/mpegts.md), "The oracle's
  environment"), and the collector filed the 30 files as `hostile`. Before
  the collection went on, `GCONV_PATH` was set as
  `scripts/oracle-ffprobe.sh` sets it, the 30 files were judged again (all
  30 are `real`) and the decisions after them were replayed in order, which
  dropped 5 files taken later whose signature was then already held. Draw
  5's signatures, computed with the same crash, were computed again first.
- **Draw 6's first 16 candidates** were taken in an order that counted each
  data set of one site as its own source; the collector then counted the
  site once, as the rule above intends. The 16 decisions stand.

## MediaRecorder recordings

`mediarecorder-v1`: 41 browser recordings for the Matroska and WebM run
([`design/mkv.md`](design/mkv.md)), written by Chrome 155's own
`MediaRecorder` muxer from a synthetic canvas and oscillator stream. They
show no people and hold no personal data. The repository's generator,
[`tools/mediarecorder/`](../tools/mediarecorder/README.md), made them;
its README lists the configurations and what this Chrome writes.

- The set is generated, not collected. Its manifest holds, per recording,
  the requested and reported MIME types, the recording parameters, the
  SHA-256, the size, the Chrome version and what `ffprobe` read.
- Every file's size and SHA-256 were checked against the manifest after
  the upload.
- Chrome's output is not byte-identical between runs (timing), so a set
  regenerated with the tool gets new hashes and is a new set. The set is
  frozen; a newer Chrome's recordings become a new set (`v2`) rather than
  replacing this one.
- The whole set is a development set: it is used for tuning as well as
  for the reported numbers, and has no held-out part.

## Command-coverage set commands-v1

A fixed set of real `ffmpeg` and `ffprobe` invocations, used to measure
how much of the way people call those tools the compatibility command
answers, and to check that it refuses the rest
([`design/compat-cli.md`](design/compat-cli.md), where the bars are). It
holds command lines, not media, and nothing in it is private: the set
and its own record are committed in full under
[`tools/compat-corpus/commands-v1/`](../tools/compat-corpus/commands-v1/README.md),
beside the tools that collected and classified it. The summary below is
that record's.

| | |
|---|---|
| Commands | 1,147 distinct option sequences |
| From | 894 answers to 398 Stack Overflow questions, 1,428 command-and-answer pairs |
| Snapshot taken | 2026-10-20 |
| `ffprobe` | 143 commands: 93 native, 48 out of scope, 2 unusable |
| `ffmpeg` | 1,004 commands: 79 native, 897 out of scope, 28 unusable |

- **Where it comes from.** Two queries of the public Stack Exchange API on
  Stack Overflow, by votes: the whole `ffprobe` tag (449 questions, every
  question it had on the snapshot date) and the 500 highest-voted
  questions of the `ffmpeg` tag. The `ffmpeg` sample is deliberately not
  enriched toward stream-copy questions, so the share the compatibility
  command reaches is an honest number.
- **How a command is chosen.** Only `<code>` and `<pre>` blocks of
  answers scoring 1 or more; never prose, never a question. Continuations
  are joined, comments, prompts and a leading `sudo` dropped, the line
  split at `|`, `&&`, `;` and `&` outside quotes and cut at a redirection,
  and each part whose program is `ffmpeg` or `ffprobe` kept, its
  arguments split as a POSIX shell would. 413 candidates were dropped as
  not an invocation.
- **File names become placeholders** (`INPUT`, `INPUT2`, `OUTPUT`), in the
  positions that are file names only; each measurement substitutes a
  corpus file. Two invocations that differ only in file names are one
  record. In 21 of the 1,147 records the value of an option the
  classifier does not know is itself a file name, a host name or a URL an
  answer used.
- **Each record** holds `tool`, `args`, a `label` with the `reason` (the
  token that decided it), and `sources`: every answer the option sequence
  was found in, with question and answer ids, link, score, licence and
  which query found it. 918 of the 1,428 command-and-answer pairs are
  under CC BY-SA 4.0, 487 under 3.0 and 23 under 2.5.
- **Labels**, by a rule written before anything was measured: `native`
  (every option in the compatibility command's vocabulary, one input and
  one output in containers CoreAV reads and writes, a job it does; the
  command must run it), `out-of-scope` (an encode, a filter, several
  inputs, a device, an image sequence, another container or an option not
  implemented; the command must refuse it with a clear error), and
  `unusable` (cut short by the extraction, or a shell variable never
  expanded; 30 of 1,147, outside both bars).
- **Native share** of the usable commands: 66.0% of `ffprobe` (93 of 141)
  and 8.1% of `ffmpeg` (79 of 976). Most highly-voted `ffmpeg` questions
  are about encoding.
- **Fixed.** A measurement quotes `commands-v1`. A new snapshot becomes
  `commands-v2`. Only a record's `label` and `reason` may change in place,
  when the compatibility command's vocabulary widens, and a test
  recomputes every label so that the change shows in its pull request.

## Adding a corpus

A new corpus, top-up or draw follows the same split, in the pull request
that adds it:

- **Public, on this page:** what the set is and what it is for, its sets
  and oracle, its groups, its targets, its structural signature, its
  holdout salt and rule, and, after collection, its counts and its
  shortfalls. The rules, the targets and the salt are committed before the
  first file is fetched, so that a result cannot move them.
- **Not public:** the files, the manifests, and everything else about the
  files beyond the structural facts above.
- A frozen corpus is not extended. A new need, or a spent holdout, gets a
  new top-up or draw with its own salt.
- A set that is generated rather than collected has no such records, and
  nothing of it is private but the media.
