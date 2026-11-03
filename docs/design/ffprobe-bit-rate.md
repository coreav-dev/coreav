# Fragmented stream bit rates

`coreav ffprobe` estimates a fragmented stream's bit rate from sample
bytes and encoded sample durations that cover the same part of the track.
ISO/IEC 14496-12 section 8.8 distinguishes the sample durations resolved
from `trun`, `tfhd` and `trex` from the decode origin in `tfdt`. A later
origin does not make an encoded sample longer.

## Compatibility policy

Pinned FFmpeg 9.0.2 uses the first sample-bearing fragment's bytes and
duration when a segment index is present. Without an index, it uses all
sample bytes and their summed durations. This sampling choice is observed
black-box behavior, not an ISO requirement. Both estimates round to the
nearest integer. The printed stream start and duration keep their existing
rules; they are not the estimate's denominator.

A generated AAC stream contains 47,240 sample bytes over 145,024 ticks at
48 kHz. Without an index its reported rate is 125,084 bit/s. Adding a
segment index makes the oracle use the first fragment's 7,937 bytes over
24,576 ticks, or 124,016 bit/s. Moving every fragment's `tfdt` forward by
96,000 ticks changes the printed start and duration, not either rate.

## Representation

The fragment scanner retains the first sample-bearing fragment's bytes
and encoded duration as one optional pair. It fixes that identity before
walking its sample runs, so an unknown byte count or zero duration cannot
select a later fragment instead. The scan reuses its existing duration
and byte accounting, with no additional payload reads or relaxed limits.

The compatibility layer selects that pair for indexed fragmented input,
or the complete track totals for other fragmented input. Unknown bytes or
a zero selected duration produce no estimated rate. Existing constant
codec rates and non-fragmented estimates retain precedence and behavior.

## Verification

Generated-byte regressions cover unequal fragments, defaults and explicit
sample fields, leading empty fragments, independent tracks, decode-origin
changes, zero duration, uncountable bytes and repeated scans. CLI
regressions check the selected rate without changing printed timing. The
pinned oracle checks use generated files only. No comparison predicate,
known-divergence record, baseline or threshold changes.
