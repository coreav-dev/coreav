# Opus speed, round 4

The registered admission check refused the complete prepared roster with `roster_exceeds_allowance`. No judged shard started, so the round provides no timing statistics or candidate pass/fail verdict. The decoder candidate is not included in the product.

## Candidate and preparation

The [registered plan](../../design/audio-decode.md#opus-speed-round-4) merged before collection. It retained candidate `3f7ed8d3123307ea9fb18db398352828a71912b3` and base `b2725e0b4fbc623278d049cb87daed484cd5df8a`, without changing the decoder or agreement predicate.

[Draw 10](../../corpus.md#draw-10) reached its fixed target of 120 real sources from the frozen candidate list. Preparation completed all 240 inputs: one MP4 and one Matroska/WebM per source. Every derivation has status `ok`, and every prepared hash is distinct. The frozen roster assigns exactly 30 sources and 60 inputs to each of four shards.

Metadata probing used pinned FFmpeg 9.0.2. Only historical input derivation used FFmpeg 7.0.2, as registered. Successful derivation is not a decoder identity, oracle agreement, or speed result. The qualified judging sources and binary identities remained unchanged.

Preparation and admission provenance is bound by commit `0b7c5a9adde58cb8967b609c7282978d14152477`. The [sanitized admission record](evidence/opus-round4-admission.json) binds the complete inputs and roster by SHA-256 and records the independently replayed forecasts. There are no judged per-call receipts to publish.

## Admission refused before judging

The capacity-derived forecast charges three gates, one warmup and five rounds of eight engines, startup, control work, and twofold headroom. Every shard exceeds the registered 16,200-second work allowance:

| Shard | Sources | Prepared inputs | Work forecast, seconds | VM-window charge, seconds |
|---|---:|---:|---:|---:|
| 0 | 30 | 60 | 46,953 | 16,839 |
| 1 | 30 | 60 | 51,339 | 16,795 |
| 2 | 30 | 60 | 44,844 | 16,810 |
| 3 | 30 | 60 | 43,987 | 16,795 |

These are planning quantities, not measured execution times. The VM-window charge adds setup, acquisition, and a 300-second collection margin to the configured work allowance. Each charge fits the separate 18,000-second VM limit, but neither that check nor successful preparation overrides the work forecast.

The registered judged hosts were four otherwise idle, on-demand GCP `c4-standard-4` machines with Intel Xeon Platinum 8581C, SMT off, and work pinned to CPU 1. No such judged invocation was launched. The controller stopped at input admission, before any shard's create-once start claim.

## No speed verdict

The complete roster remains intact. No input was dropped or reassigned, no window was extended, and no replacement list was collected to obtain admission. All registered bars and minimum counts remain unchanged.

No raw-identity, command-identity, or oracle result was obtained by a judged run. There are no medians, bootstrap intervals, or shares of inputs on which CoreAV was faster. The record therefore has `speed_verdict: null` and `stats: []`; admission refusal is neither a measured speed failure nor evidence that the candidate passes.
