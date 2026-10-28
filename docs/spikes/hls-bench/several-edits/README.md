# HLS and sources with several media edits

Measurement code for `docs/design/edit-list-writers.md`, "The HLS
writers": whether the HLS writers can package a source whose track has
several media edits by stream copy into one CMAF rendition. Throwaway,
outside the Cargo workspace. Written from ISO/IEC 14496-12,
14496-15 and 14496-10; `ffprobe` and `ffmpeg` are black boxes.

## Hypothesis and criteria

The writers refuse such sources (`docs/design/edit-lists.md`, decision 6).
The hypothesis is that a stream copy into one rendition can present them
exactly. The criteria, fixed before these runs and recorded in the design
note's "Criteria":

- a reader must apply more than the first media edit of a fragmented
  rendition, or the source's edit list cannot simply be carried;
- every sample a presented sample needs must be one a writer can leave
  out, place outside the edit, or drop as disposable; otherwise a
  rendition with one media edit cannot hold the source.

## Scripts

    fragmented-elst.py INIT OUT DURATION:MEDIA_TIME ...
        Replaces a fragmented init segment's edit list.

    hidden-placement.py DIR RENDITION SEGMENT SAMPLE OFFSET ...
        Rewrites one sample's composition offset in one media segment of a
        `coreav hls package` directory and reads the rendition back.

    hidden-samples.py PATH@INDEX@NAME ...
        Counts, per stream, the samples no media edit presents and what a
        writer could do with each.

Each needs the pinned oracles in `/opt/coreav-oracle-bin` (or the paths
edited at the top) and, for `hidden-samples.py`, `scripts/clip_check.py`
for its box reader and edit arithmetic.

## How they were run

    coreav hls package SOURCE PKG            # SOURCE: a 300-sample H.264
                                             # fixture, 600 Hz, 6 s target
    fragmented-elst.py PKG/video-init.mp4 OUT 3000:600 1800:2400
    cat OUT PKG/video-*.m4s > all.mp4        # then ffprobe -show_packets

    hidden-placement.py PKG video 1 0 -3500 3500 -1 1
    hidden-samples.py FILE@STREAM@NAME ...   # the fixtures and the six
                                             # corpus files with several edits

## Results

In the design note's "What was measured", measurements 1 to 4. The
hypothesis is rejected: a reader applies at most the first media edit, and
five of the six corpus files with several edits hold 2 to 11 samples a
presented sample needs with nowhere on a one-edit timeline to go. The
refusal stands; an earlier change is the design that lifts it.

Spend: 0 USD (local runs; R2 downloads of the six corpus files).
