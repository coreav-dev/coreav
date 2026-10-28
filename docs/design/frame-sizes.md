# Verified picture sizes

Status: Accepted. Code: `crates/coreav-isobmff/src/size.rs`, the public
`frames` module (the readers over any `Source`, also used for Matroska
blocks by `coreav-mkv`, `docs/design/policy-matroska.md`) and
`Movie::scan_frame_sizes`; `coreav-cli` (probe output, `coreav check`);
`coreav-policy` (resolution rules).

## Problem

A visual sample entry declares a width and a height. A decoder does not use
them: it takes the picture size from the codec's own parameter sets (the
sequence parameter set of H.264 and HEVC, the sequence header of AV1) or
from each frame's header (VP9, ProRes). A file can therefore declare
320×240 in its sample entry and decode to 7680×4320, with a 4-byte edit.
Parameter sets can also travel in the samples: always for `avc3`, `hev1`
and AV1 sync samples, and in practice also in `avc1` and `hvc1` tracks,
whose in-band parameter sets decoders apply. A policy gate that compares the
declared size lets such a file through to the encoder behind it, which is
the case the gate exists for.

## Decisions

1. **Two verified sizes, next to the declared one.** The probe keeps
   `width` and `height` as the sample entry declares them, and adds:
   - `parameter_set_size`: the bound, in each dimension, of the picture
     sizes the parameter sets of the codec configuration record define
     (every SPS of `avcC` or `hvcC`, cropped; the maximum frame size of the
     sequence header in `av1C`). Always computed; it costs nothing beyond
     the record probe already reads. `null` when the record holds no
     parameter set (`avc3`-style records, `av1C` without configuration
     OBUs, VP9 and ProRes, which have none) or one of them cannot be read.
   - `scanned_size`: the bound over the configuration and every parameter
     set or frame header in the samples, from a scan of the samples
     (`Movie::scan_frame_sizes`, `coreav probe --scan-frame-sizes`, and
     `coreav check` when the policy needs it). `null` when not scanned, or
     when a sample could not be read: a size that might be exceeded by an
     unread sample is not a bound.
   The declared size stays because it is what the file states and what
   other tools print as the sample entry's size; replacing it would hide
   the disagreement that matters. Rejected: overwriting `width` and
   `height` (changes a `/1` key's meaning and hides lies); a single
   "coded size" from the first parameter set (a decoder may use any of
   them).
2. **What the scan reads, per codec**, from the public specifications only:
   - H.264 and HEVC: every NAL unit of every sample, walked by its length
     prefix (`lengthSizeMinusOne` of the record, ISO/IEC 14496-15); each
     sequence parameter set (H.264 `nal_unit_type` 7, HEVC 33) is read for
     its cropped size (ITU-T H.264 7.3.2.1.1 and 7.4.2.1.1; ITU-T H.265
     7.3.2.2.1, conformance window). Other NAL units are skipped by their
     length; slice data is never read.
   - AV1: every OBU of every sample (each has a size field, AV1 ISOBMFF
     binding 2.4); each sequence header OBU gives `max_frame_width` and
     `max_frame_height` (AV1 5.5.1), which bound every frame that follows
     it (AV1 6.8.2).
   - VP9: the uncompressed header of every frame (VP9 bitstream
     specification 6.2), superframes split by their index (annex B). Key
     frames and intra-only frames code their size; inter frames code it or
     take it from a reference frame, which was itself coded earlier, so
     the bound over coded sizes bounds every frame.
   - ProRes: the frame header at the start of every sample (SMPTE RDD 36,
     `horizontal_size`, `vertical_size`).
   Cropping: the bound is the coded size less a right or bottom crop
   (H.264 `frame_crop_*_offset`, HEVC conformance window) only when that
   crop is padding, under 64 luma samples (a macroblock or a coding tree
   block), and the crop as a whole is valid: in both dimensions it leaves
   at least one sample (`ffmpeg` ignores an HEVC conformance window that is
   invalid in either dimension, in both). A
   left or top crop is never subtracted, and a larger right or bottom crop
   leaves the coded size. Decoders need not apply a crop
   exactly: `ffmpeg`, a black box here, outputs the uncropped width for a
   left crop that is not a multiple of 64, so subtracting it would let a
   1968-wide stream pass a 1920 limit; and a large crop hides how big a
   picture the decoder decodes (a 3840×2160 stream cropped to 320×240).
   A record whose version is not 1 (`avcC`, `hvcC`), whose structure runs
   past its end, or whose parameter set does not parse gives no bound
   either: a decoder may frame the samples differently or apply what this
   crate cannot read. The size comes from the first record of the codec's
   type in the entry.
   A sample whose structure does not parse (a NAL length past its end, an
   OBU size past its end, a VP9 frame without its sync code where one is
   required) makes the scanned size `null`: a decoder might read it some
   other way. Rejected: decoding (out of scope for probe and for use case 7,
   which stream-copies); reading only sync samples (a parameter set may
   come in any sample); trusting `avc1`/`hvc1` to carry parameter sets only
   in the record (decoders apply in-band ones).
3. **The scan is bounded by the limits.** It visits the samples through
   the packet listing, so `max_sample_work` and the table limits bound it,
   with one work budget for the scan of every track of the file
   (`Movie::scan_all_frame_sizes`): a track after one that spent the
   budget is unknown. Rejected: a budget per track, which keeps one long
   track from leaving the others unscanned but multiplies the bound by the
   number of tracks (up to `max_tracks`, so a small file of many video
   tracks could take minutes);
   each parameter set is read up to 4 KiB, each NAL or OBU header up to a
   few bytes, and a VP9 or ProRes header up to 64 bytes plus the superframe
   index. Memory does not grow with the file. Reading time does: the scan
   reads the headers of every sample, and through a range reader that is
   close to the whole object. The verified size bounds the pictures a
   decoder outputs and, through the cropping rule, the pictures it
   decodes; it does not bound every resource a decoder may use, so a
   service behind the gate keeps its decoder's own limits. For AV1 the
   bound rests on the conformance rule that no frame is larger than the
   sequence header's maximum frame size (AV1 6.8.2).
4. **Policy: the largest of what is declared and what is verified.** The
   resolution rules (`max_width`, `max_height`, `max_pixels`) compare the
   largest of the declared size and the verified one, each dimension
   separately, and point at the larger value's path. A new rule
   `streams.video.size_source` chooses what counts as verified:
   - `"scan"` (the default): `scanned_size`. `coreav check` scans the
     samples of video streams when a policy with resolution rules uses it.
   - `"configuration"`: `parameter_set_size`, without reading samples, for
     callers that accept the in-band case (for example a WebAssembly build
     reading a large object by ranges).
   - `"declared"`: the sample entry alone, the behaviour before this
     change.
   A verified size that the chosen source cannot give is `unknown`, so the
   rule rejects (fail closed, as every rule). Under `"configuration"` that
   rejects VP9 and ProRes, which have no parameter sets in the record, and
   `avc3`/`hev1` tracks with an empty record.
5. **A mismatch is not a rule of its own.** A file whose verified size is
   larger than declared is rejected exactly when the larger size breaks a
   limit, with a reason that names the verified value; a file whose sizes
   differ but stay within the limits is accepted, because nothing the
   policy limits is exceeded. The probe still reports the difference as a
   warning (`declared size W×H, parameter sets give W×H`), so callers that
   want to reject any disagreement can. Rejected: rejecting every mismatch
   (legitimate files differ: an `avcC` cropping a few lines more than the
   sample entry declares, AV1 sequence headers allowing a larger maximum
   than the coded frames).

## Verification and bars (fixed before the corpus run)

- Unit tests for each parser, including emulation prevention, cropping,
  field coding, sub-layers, every truncation, and superframes.
- Hostile cases in `crates/coreav-isobmff/tests` and the policy fixtures:
  a sample entry that understates the size of its `avcC`, `hvcC` and `av1C`
  parameter sets; a larger SPS carried in the samples of an `avc1` track;
  a larger VP9 key frame later in a track; a ProRes frame larger than its
  entry. Each is rejected by a policy whose limits sit between the two
  sizes, under the default `size_source`.
- A fuzz target (`frame_sizes`) feeds arbitrary files through
  `scan_frame_sizes` and arbitrary bytes through each parser.
- Corpus v1, every ISO base media file, scanned:

| Bar | Set | Threshold |
|---|---|---|
| S1 crashes | every file | 100%: no crash or timeout (10 s, 1 GiB), the scan returns a size or an error |
| S2 bound | every video stream that `ffprobe` decodes | 100%: `scanned_size` is not smaller than `ffprobe`'s `width` × `height` in either dimension, except where an entry of `docs/known-divergences-sizes.md` covers it |
| S3 agreement | H.264, HEVC, VP9 and ProRes streams with a `scanned_size` | at least 99% equal `ffprobe`'s `width` and `height` exactly (AV1 is reported apart: its bound is the sequence header's maximum, which may exceed the coded frames) |
| S4 configuration | streams with a `parameter_set_size` | at least 99% equal their `scanned_size` (parameter sets rarely change in-band) |
| S5 policy | the policy check's corpus bars (`docs/design/policy-check.md`, B1 to B5), rerun on the files whose verdict changes | as committed there |
| S6 speed | held-out ISO base media files | median ratio of `coreav check` with the default size source (scanning) to `ffprobe -v error -count_packets -show_streams` (which also reads every packet) at most 1.0 |
| S7 memory | every file | peak resident memory of the scanning check at most 300 MiB |

**S6 again, per class (fixed before the run).** S6 was judged
overall only. It is re-run on current `main` on the same 303 held-out
files with the same method (the spike's `speed` run, one warm-up and five
alternating rounds), round 2 of this measurement (the first passed and
was never used for tuning). Bar: the median ratio is at most
**1.00** overall and in each class, unfragmented and fragmented (the
`fragmented` flag of each file's record); "faster" only where the upper
bound of the bootstrap 95% interval is below 1.00.
**Result: met** (`docs/spikes/frame-sizes/`, `speed-02` on the crates of `1114aa6`): 0.150 (95%
CI 0.132 to 0.160) over 303 files; unfragmented 0.150, fragmented 0.156.
Every upper bound is below 1.00, so the scanning check is faster than
`ffprobe -count_packets` in both classes.

## Results

The corpus v1 run is recorded in
[`docs/spikes/frame-sizes/`](../spikes/frame-sizes/README.md), with its
evidence in `docs/spikes/frame-sizes/evidence/`. Every bar is met:

- S1: no crash or timeout in 2,356 files.
- S2: no stream's scanned size is smaller than `ffprobe`'s, over 1,255
  streams; no known divergence was needed
  ([`known-divergences-sizes.md`](../known-divergences-sizes.md)).
- S3: 99.24% equal `ffprobe`'s size exactly; every difference is larger on
  CoreAV's side.
- S4: 100%.
- S5: no failure in the policy cases the change can move.
- S6: median ratio 0.135 against `ffprobe -count_packets`.
- S7: 43,528 KiB (44.6 MB) at most.
