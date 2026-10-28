# Probe colour description: the sample entry and the parameter set

Status: Accepted. Scope: the colour fields of video streams in
`coreav probe` for ISO base media files, and how the stream oracle and
`--ffprobe-compat` map them to `ffprobe`'s fields.

## Two sources

A video stream's colour description (ITU-T H.273 colour primaries,
transfer characteristics, matrix coefficients and the full-range flag) can
be stated in two places:

- the sample entry's colour information box, `colr` (ISO/IEC 14496-12
  12.1.5: `nclx`, or QuickTime's `nclc`, which has no range);
- the parameter set in the codec configuration: the video usability
  information of the first sequence parameter set of an `avcC` or `hvcC`
  record (ITU-T H.264 E.1.1, H.265 E.2.1), or the colour configuration of
  the sequence header in an `av1C` record (AV1 5.5.2). Probe reads these
  records already, so this needs no decoding.

Most H.264 and HEVC files have no `colr` box, and some have one that
disagrees with their parameter set.

## Decisions

1. **Probe reports both sources as stored, side by side.**
   - `color` is the `colr` box, raw, with its range falling back to the
     codec configuration's range flag (`hvcC`, `av1C`, `vpcC`), as before
     this change.
   - `parameter_set_color` is the parameter set's colour description, raw:
     code points as stored, "unspecified" (2) and reserved values included,
     `null` for a field the parameter set does not signal, and `null` as a
     whole when the entry has no readable parameter set.

   Rejected: merging the two into one reported value (by the box first, as
   ISO/IEC 14496-12 suggests, or by the parameter set first). Either loses
   what the other source says, and which one a decoder applies differs by
   codec (below). A raw value next to a derived one would also need the
   derivation rule to be part of the schema.
2. **Values are not normalized.** "Unspecified" stays 2 in
   `parameter_set_color`, because a parameter set that says "unspecified"
   overrides a `colr` box for H.264 decoders, while one that says nothing
   does not (below); `null` and 2 mean different things.
3. **`ffprobe`'s precedence is measured, not assumed, and lives in the
   mapping to `ffprobe`'s fields.** The stream oracle's adapter
   (`tools/isobmff-adapter`) and `coreav probe --ffprobe-compat` report what
   `ffprobe` reports, from the two raw sources. `ffprobe` 7.0.2, measured as
   a black box on files whose `colr` box was rewritten
   (`scripts/colr_craft.py`):

   | Codec | primaries, transfer, matrix | range |
   |---|---|---|
   | H.264 | the parameter set's when its VUI has a colour description (2 included, which reports nothing), else the box's; the box's for a track without frames in the file | the parameter set's when it signals a video signal type, else the box's; the box's for a track without frames |
   | HEVC | always the parameter set's; a missing colour description reports nothing, whatever the box says | the parameter set's, else limited (ITU-T H.265 E.3.1 infers `video_full_range_flag` 0) |
   | AV1 | always the sequence header's | always the sequence header's (`color_range` is always coded) |
   | other | the box's | the box's, else the codec configuration's |

   `ffprobe` applies an H.264 parameter set's colour only to a frame it
   decodes: an initialization segment (a track without samples, or one
   whose data is in another file) reports the box's. HEVC and AV1 report
   the record's either way. ITU-T H.264 E.2.1 infers a missing range as
   limited too, but `ffprobe` reports no range for H.264 then; the mapping
   follows what `ffprobe` reports, and probe itself reports neither
   inference. When the record holds no parameter set (an `avc3` or `hev1`
   entry with empty arrays), the mapping reports the box's: the parameter
   sets in the samples are not read.

   Residual differences, where reading the samples would be needed: a track
   whose samples are missing from the file (a truncated download), which
   `ffprobe` cannot decode and so reports the box's; and an H.264 sample
   carrying an SPS that disagrees with the record's, which `ffprobe`
   decodes and reports.
   Rejected: known divergences for the cases where `ffprobe` and a merged
   value would differ (`KD-COLR-PRECEDENCE`, `KD-HEVC-RANGE` in an earlier
   draft). They would cover a definition choice of CoreAV's, not a
   difference in what the file says.
4. **VP9 is unchanged.** VP9 frames code the matrix and range in every
   frame header, which `ffprobe` reports over the box; probe reads `vpcC`,
   not frame headers, and VP9 streams are not among the colour differences
   of the stage 3 report. Out of scope here.

## Cost

The parameter set is read once per sample entry, from the first record of
the codec's own type, not once per fragment. On the guard's fixtures the
field costs about 7,000 instructions per probe: about 2,000 to read the
sequence parameter set and the rest to write the five keys. The cost the
guard first showed for it was mostly copies of the JSON document, since
removed (`docs/design/benchmarks.md`, "The probe document's nested
values").

## Verification

- Library tests for `parameter_set_color` (raw code points, "unspecified",
  a range without a colour description, no VUI, no parameter set), and unit
  tests of the precedence (`crates/coreav-cli/src/compat.rs`), including
  HEVC without a video signal type, which x265 does not write, and H.264
  tracks without frames (an initialization segment has other differences
  from `ffprobe`, in profile, level and duration, so it is not a fixture
  here).
- Fixtures (`scripts/gen-fixtures.sh`): a box over an H.264 SPS without a
  colour description (`color_colr_only.mp4`), over one that says
  "unspecified" (`color_sps_unspecified.mp4`), over one with only a range
  (`color_sps_range_only.mp4`), over one that disagrees
  (`color_colr_vs_sps.mp4`), over an HEVC SPS that signals a limited range
  (`color_hevc_colr_ignored.mp4`), and over an AV1 sequence header without
  a colour description (`color_av1_colr_ignored.mp4`, when an AV1 encoder is
  available); the stream oracle and the compat check
  agree on all of them without a known divergence.
- The stage 3 corpus report, re-checked on the files with an
  uncovered difference.
