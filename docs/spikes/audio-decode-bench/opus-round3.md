# Opus speed, round 3

The round failed its error and completeness requirements. It provides no valid speed verdict, and the decoder candidate is not included in the product.

## Candidate and method

The [registered plan](../../design/audio-decode.md#opus-speed-round-3) was merged before collection. The fixed decoder candidate was `3f7ed8d3123307ea9fb18db398352828a71912b3`, based on `b2725e0b4fbc623278d049cb87daed484cd5df8a`. No candidate revision was tested on the draw.

Draw 9 supplied 160 real files, giving 320 expected derived inputs: one MP4 and one Matroska/WebM per file. Four duration-balanced shards ran on standard GCP `c4-standard-4` hosts, with SMT off. Every recorded host passed the Intel Xeon Platinum 8581C and tmpfs checks. Both CLI builds used the `dist` profile; both decoder harness builds used fat LTO and one codegen unit.

The decoder baseline explicitly selected libavcodec 9.0.2's native `opus` decoder. Comparisons and metadata used the pinned FFmpeg 9.0.2. Only historical input derivation used FFmpeg 7.0.2. Decoder-only MP4 packet acquisition and TOC counting ignored edit lists, so both sides read the complete coded packet sequence; presentation and library comparisons kept their normal edit handling.

Each input required raw per-packet `f32` identity, command `s16` identity, and the existing oracle agreement check before timing. Eligible inputs received one warmup and five rotated-order rounds. A real-capture negative test rejected a changed low sample bit whose `s16` value was unchanged before the draw was opened. Eight injected baseline errors were also rejected during preflight.

## Recorded outcomes

All four jobs reached their host deadlines before finishing. Their last persisted checkpoints cover every expected input, including inputs without completed outcomes.

| Required check | Passed | Output disagreement | Worker error | No recorded outcome |
|---|---:|---:|---:|---:|
| Raw per-packet identity | 239 | 0 | 0 | 81 |
| Command `s16` identity | 239 | 0 | 0 | 81 |
| Oracle agreement | 153 | 29 | 57 | 81 |

The 57 oracle workers exited with status 1 after both identity checks passed. Their stderr was not retained in the checkpoints, so these records cannot identify which internal child or adapter operation failed. They are fatal worker errors, not measured decoder output disagreements or exclusions.

The checkpoints contain 153 inputs with all five timing rounds, 29 ordinary oracle disagreements, 57 worker errors, and 81 inputs without completed outcomes. The [sanitized checkpoints](evidence/opus-round3-checkpoints.json) retain those input records and every completed timing call. Partial timings are diagnostic records, not a speed result.

## Judgment

The registered method rejects an incomplete shard's numbers. All four shards were incomplete, so the [strict judgment](evidence/opus-round3-summary.json) uses zero timed inputs and reports no medians, bootstrap intervals, or shares faster. Neither the CELT speed bar nor the SILK, Hybrid, or library speed guards received a valid numerical verdict.

The oracle-worker errors independently fail the round; the later host deadlines do not erase them. No replacement run was launched. Draw 9 is spent, and this candidate is not presented as faster than libavcodec.
