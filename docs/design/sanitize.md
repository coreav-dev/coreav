# Metadata sanitization

Design note for `sanitize_mp4` (`Metadata::Sanitize` of `remux_mp4`) in
`crates/coreav-isobmff-write` and the `coreav sanitize` command. Scope:
use case 7, which "strips location and device metadata", and the stage 3 primitive "metadata sanitization (only
allowlisted boxes are rewritten, location and device metadata dropped)"; the pattern of sanitizers such as mp4san (use case 5).
Written from ISO/IEC 14496-12 (`udta`, `meta`, `hdlr`, sample entries),
the QuickTime File Format (user data types, metadata `keys` and `ilst`,
free-form `----` items, compressor names), Apple's published iTunes
metadata conventions (`iTunSMPB`) and 3GPP TS 26.244 (`loci`). No multimedia
implementation source is read; `ffprobe`, `ffmpeg` and MediaInfo are
black-box oracles only.

## What it does

A sanitized file is a full-file remux ([`remux.md`](remux.md)): every
stream MP4 can carry, every sample unchanged, the movie box first. It reads
MP4 and MOV files only: a Matroska or WebM input is refused (exit 1,
"sanitizing reads MP4/MOV files; Matroska and WebM are not supported"),
although `remux` converts it, since the allowlist and the requirements
below are written for ISO base media boxes. What
differs is what the output's boxes say besides the media:

- **Written** (the allowlist): the boxes the remux rebuilds (`ftyp`,
  `mvhd`, `tkhd`, `mdhd`, `hdlr` with generic names, media headers,
  `dinf`, sample tables), the edit lists, the per-sample boxes the remux
  copies (`sgpd`, `sbgp`, `sdtp`, ...), and in each sample entry the codec
  configuration and the children that say how to present the picture or
  the sound: `pasp`, `clap`, `colr`, `btrt`, `mdcv`, `clli`, `SmDm`, `CoLL`,
  `dvcC`, `dvvC`, `dvwC`, `fiel`, `st3d`, `sv3d` for video; `btrt`, `chnl`
  for audio. They are written with only the fields their formats define:
  an H.264 or HEVC configuration (`avcC`, `hvcC`) keeps only its parameter
  sets (SPS, PPS and SPS extensions; VPS, SPS and PPS), each NAL unit's own
  header checked against its type, so SEI with the encoder's name, version
  and settings is left out; `colr` is kept only with coded colour
  parameters (`nclx`, `nclc`) and cut to their size, not an ICC profile,
  whose tags can name the device; a spherical video box (`sv3d`) keeps an
  empty header (`svhd`, whose metadata source names the tool or camera)
  and a projection of `prhd`, `equi`, `cbmp` or `mshp` only; the boxes
  with a fixed layout (`pasp`, `btrt`, `clap`, `clli`, `mdcv`, `SmDm`,
  `CoLL`, Dolby Vision configurations, `fiel`, `st3d`) are cut to their
  size, and removed when shorter.
- **Removed and reported**: the movie's and every track's `udta` and `meta`
  boxes (location `\xa9xyz`, `loci` and QuickTime location keys; device
  `\xa9mak`, `\xa9mod`, `\xa9swr`, `\xa9too` and make, model and software
  keys; dates `\xa9day` and creation-date keys; titles, comments, cover
  art, vendor data); sample entry children outside the allowlist (vendor
  and unknown boxes); and, as in any remux, every other box the output does
  not carry (top-level `uuid` such as XMP, `tref`, ...).
- **Cleared and reported**: creation and modification times of the movie,
  track and media headers (written as 0, "not set"); the visual sample
  entries' compressor name (written empty: cameras and encoders put their
  model or name there); what the allowlisted boxes held beyond their
  fields and the spherical metadata source, above; and, unless kept, the track header's matrix (the
  display rotation, written as the identity) and the media header's
  language (written as `und`).
- **Kept by default**: what decides how the file plays and says nothing
  about who made it or where. Rotation and language (a portrait phone
  video turns sideways without its matrix), and two items written again
  from what they say rather than copied:
  - gapless playback information, the `iTunSMPB` free-form item of the
    `com.apple.iTunes` namespace: encoder delay and padding, which players
    trim from the start and end of AAC and MP3 audio. It is parsed (delay,
    padding and length, hexadecimal) and written in its canonical form
    with every other field zero, in a new `udta/meta` (`mdir` handler) of
    the same level holding only it. An item that does not parse is
    removed;
  - track kinds (`kind`, ISO/IEC 14496-12 8.10.4): a track's role, such
    as a forced subtitle or a commentary track, in the track's new `udta`.
    Kept only in a track, as version 0 with a scheme and a value from a
    fixed vocabulary, the roles of ISO/IEC 23009-1 (`urn:mpeg:dash:role:2011`:
    `main`, `alternate`, `commentary`, `forced-subtitle`, ...) and the HTML
    track kinds (`about:html-kind`), and written again from that table, at
    most 16 per track; anything else is removed, since a printable value
    can still be free text.

  `Keep` (`--keep`) chooses: `rotation`, `language`, `gapless`, `kinds`
  and `dates` (creation times), or none. The source's `udta` and `meta`
  boxes are still listed as removed when an item of theirs is kept: the
  output's are new boxes.
- Streams MP4 cannot carry are dropped as in a remux; among them are the
  timed metadata and telemetry tracks (`mebx`, GoPro `gpmd`, DJI data
  tracks), which carry locations sample by sample. Only kept samples reach
  `mdat`.

The summary lists every removed box and item (path, offset, size; for an
item, what it describes, from its type or key: `location`, `device`,
`date` or `other`; for a QuickTime key or a free-form item, its name) and
every cleared field. It never repeats a removed value, so a log of
summaries holds no location: a name is repeated only when it reads as an
identifier (letters, digits, `.`, `_`, `-`, at most 64 bytes, such as
`com.apple.quicktime.location.ISO6709`), else it is `null`. Like any
remux summary, it lists at most 256 boxes and counts the rest; the
removed items beyond that are counted, not kept in memory. The keys of a
QuickTime `keys` box share their box's offset and size.

`coreav sanitize INPUT OUTPUT` prints the summary as `coreav.remux/1`
([`schema/remux-v1.md`](../schema/remux-v1.md)) with `metadata.mode`
`sanitize`, the removed boxes as `sanitized` and the cleared fields.

## Limits of what it removes

- **Inside the coded streams.** The remux copies samples; it does not parse
  them. Data that encoders and cameras put into the samples themselves
  (H.264 and HEVC SEI messages in the first frames: encoder settings, some
  cameras' GPS or timestamps) stays. Removing it needs bitstream rewriting
  (`coreav-bitstream`), which is not part of stage 3. SEI in a
  configuration box is removed (above).
- **Inside the boxes copied as stored.** `esds` (its ES descriptor can
  carry a URL string and further descriptors), `dOps`, `chnl`, `av1C` (its
  configuration OBUs can include metadata OBUs), `vpcC`, the parameter sets
  themselves, and the sample group descriptions (`sgpd`, whose entries of
  an unknown grouping type are opaque bytes). Encoders do not put names
  there, but a crafted file can.
- **Numbers kept as numbers.** The rotation matrix (nine 32-bit values),
  the gapless delay, padding and length (128 bits) and the fields of the
  fixed-layout boxes are copied without a check that they are plausible.
- **Names that look like identifiers.** The summary repeats a key or item
  name that reads as an identifier, so a crafted name such as
  `Owner_Alice` is repeated; values are never repeated.
- **Sample data at odd offsets.** A malformed source whose chunk offsets
  point into its own metadata makes those bytes "samples" of a kept stream;
  they are copied as such (found by the mutation tests; the output's box
  tree still holds no metadata box).

## How the removed metadata is listed

`udta` children are listed one by one; a `meta` box (ISO full box, or
QuickTime's without version and flags) is listed with its `hdlr`, each
key of a `keys` box by name, and each `ilst` item by type, or for a keyed
item by the key it refers to, for a free-form item by its name. The walk
reads box headers and the payloads of `keys`, `kind` and `----` boxes
(bounded by `max_box_bytes` and smaller per-box bounds), counts every box
and every key against `max_total_boxes`, together with the boxes the
remux's own scan counted, and stops at a list that does not parse (the
parent is still listed as removed). What an item describes is a hint from its type or
key name, not a guarantee: an unknown key holding a location is listed as
`other` and removed all the same.

## Alternatives considered

- **A denylist** (remove location and device boxes, keep the rest): vendor
  boxes and keys unknown to the list would leak. The allowlist keeps only
  what the remux understands.
- **Clearing values in place** (zeroing the coordinates inside `\xa9xyz`):
  needs every format's layout, and the box would still say there was a
  location.
- **Clearing rotation and language by default**, as the issue's example
  "keep a configured set" could suggest: a default that turns portrait
  videos sideways would push callers to keep everything. Both stay by
  default; `--keep none` removes them.
- **Removing gapless information and track kinds with the rest**, as the
  first design did: the corpus run showed what it costs. AAC files lost
  their encoder delay (2,112 priming samples played as sound, the last
  packet untrimmed), and a forced track lost its role. Neither identifies
  anyone, so they are kept like the rotation.
- **Copying the `iTunSMPB` item or the `kind` box as stored**, or keeping
  any `kind` of printable text: the item's value is free text past its
  first four fields, and a `kind` value can be a sentence. Writing the
  parsed numbers again, and a `kind` only from a vocabulary, keeps the
  allowlist a list of meanings rather than of box types.
- **Copying allowlisted sample entry boxes as stored** (the mp4san rule):
  every sanitized HEVC file would keep x265's build string and options,
  and crafted files can append text to any box. Writing only the defined
  fields is a box-level rewrite; players need only the parameter sets in a
  configuration.
- **Keeping titles or chapters on request**: not offered yet; a caller
  that needs a title can write it after sanitizing.

## Verification

- Integration tests in `crates/coreav-isobmff-write/tests/remux.rs`
  (sanitization section): a camera-like movie with location, device and
  date metadata as user data, QuickTime keys and items, a track `udta`,
  vendor sample entry children, a compressor name, a rotation and a
  language; every removed box and its kind and key name, every cleared
  field, and the samples and times of the round trip; `Keep::NOTHING` and
  kept dates; gapless information (written canonically, the other fields
  dropped) and track kinds (outside the vocabulary or the track removed),
  removed with `Keep::NOTHING` and stable when sanitized twice; names that
  are not identifiers not repeated; an ICC `colr` removed and a spherical
  source cleared; a summary of 600 items listing 256, and keys counted
  against `max_total_boxes`; deterministic mutations, whose outputs' box
  trees hold no metadata box. Unit tests of the H.264 and HEVC
  configuration, `colr`, fixed-layout and spherical rewrites, with SEI in
  parameter set arrays and appended text.
- `fuzz/fuzz_targets/sanitize.rs`: arbitrary input either fails or gives
  an output the parser accepts, with every sample of every kept stream and
  no metadata in its tree but, as `Keep` allows, track `udta` boxes
  holding `kind` boxes of the vocabulary and a `udta` holding exactly the
  canonical `iTunSMPB` `meta` box.
- Fixtures, in CI: every MP4, MOV and M4A fixture is sanitized and checked
  by `scripts/remux_check.py` in its sanitize mode: the media checks of a
  remux, plus, from the box headers: only allowlisted boxes in the movie
  box tree (by parent), generic handlers, the kept items in their exact
  forms, H.264 and HEVC configurations equal to the source's parameter
  sets, `colr`, `sv3d` and fixed-layout boxes in their written forms, an empty compressor name and zero creation
  times; no `ffprobe` tag but the brands, a canonical `iTunSMPB`,
  `language`, and `handler_name` and `vendor_id` with their generic
  values; the packet checksums show that the kept gapless information
  trims as the source's does (the extradata of an HEVC stream whose SEI
  arrays were removed is compared at the box level instead); no attached
  picture; no MediaInfo key but structural ones in the general track and
  no title, date, location, device or application key in a stream's
  track. Keys MediaInfo reads from the samples (`Encoded_Library*`) are
  allowed, by the first limit above. Threshold: 100% of the fixtures.

### Corpus v1 bars (fixed before the corpus run)

- **Inputs:** the 1,043 files of the remux's corpus run (every corpus v1
  `real` file with an ISO base media extension), each sanitized with the
  default `Keep` (rotation and language when the bars were fixed; see
  below), from a local file.
- **Checks:** the library round trip (with the cleared fields), then
  `scripts/remux_check.py` in its sanitize mode with the known divergences
  of [`known-divergences-remux.md`](../known-divergences-remux.md).
- **Refusals** (nothing MP4 can carry) are counted apart, as for the
  remux; every other failure counts as disagreement.
- **Pass:**
  - **100%** of sanitized outputs meet every sanitization requirement
    above, with no known divergence allowed to cover one: a single leaked
    tag fails the bar;
  - at least **99% of sanitized files** agree with every check, directly
    or through a known divergence;
  - at least **99.9% of packets** agree;
  - no panic, crash or timeout.
- **Reported, not gated:** how many files had location, device and date
  metadata removed (by the kinds in the summary), and the cleared fields.
- Memory is the remux's: the same copy path, with less metadata copied.

### Speed bar (fixed before the run)

Sanitizing walks and rewrites the metadata boxes the remux copies, so its
speed is measured on its own:

- **Inputs:** the 301 held-out files of the remux's performance runs
  (`docs/spikes/remux-bench/evidence/inputs.json`). Sanitize has not been timed
  on this set before; the `npx` rounds 1 and 2 timed the command line on
  held-out files of at most 128 MiB, most of them in this set
  (`docs/measurements/npx-coreav.md`).
- **Engines:** `coreav sanitize SRC OUT` against `ffmpeg -nostdin -v error
  -y -i SRC -map 0:N ... -c copy -map_metadata -1 -map_chapters -1 -movflags
  +faststart OUT`, the streams the sanitize keeps, both command lines on
  the same machine, outputs on a tmpfs. Per file, one warm-up of each, then
  five rounds with the order alternated; a file's time is the median of its
  rounds.
- **Classes:** the remux's: unfragmented, fragmented (tags `fragmented` or
  `cmaf`), longer than one hour.
- **Bar:** the median of the per-file ratios ours / ffmpeg is
  at most **1.00**, overall and in each class. "Faster" is said only where
  the upper bound of the bootstrap 95% interval is below 1.00. Files either
  side refuses are listed apart.

**Result: met** (`docs/spikes/remux-bench/`, run `sanitize-perf-01` on the crates of `1114aa6`):
`coreav sanitize` takes 0.181 (95% CI 0.169 to 0.194) of FFmpeg's time over
270 files. Unfragmented files are at 0.181, fragmented at 0.202 (11 files)
and those longer than one hour at 0.166 (8 files). Every upper bound is
below 1.00, so sanitizing is faster than FFmpeg's copy with metadata
stripped, overall and in every class; no file is slower (at most 0.68).
`coreav sanitize` refused 31 files, the ones the remux refuses too.

The default `Keep` grew after the first corpus run (gapless information
and track kinds, see Results), and the requirements with it: the outputs
may hold those items in their exact forms. The thresholds stayed.

### Results

Evidence: [`docs/spikes/remux-bench/`](../spikes/remux-bench/README.md),
"Sanitization". The first corpus run missed the files bar (97.2%): removing
the gapless information and track kinds changed how 17 files play. With
both kept by default (above), and with what is kept and what the checker
requires tightened since, the final run meets every bar:
no metadata left in any of 933 outputs, 924 of 933 files (99.04%) and
every packet agree, no crash.
