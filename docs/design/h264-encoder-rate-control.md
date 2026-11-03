# H.264 encoder picture-budget rate control

Status: Design only, after [one-reference P pictures](h264-encoder-p.md). No controller is implemented or authorized by this note. The fixed-QP path remains unchanged. CABAC, lookahead, per-macroblock QP adaptation, I_PCM, product commands, and quality or speed evaluation remain separate work.

## Budget contract

Propose a checked target of `B > 0` NAL bits per accepted picture, excess-bit capacity `C >= 0`, and initial luma QP in 10 through 51. Timing stays 30/1. Count eight bits per byte in every complete emitted NAL array, including repeated parameter sets, NAL headers, trailing bits and emulation prevention. Exclude container headers and Annex B delimiters.

Excess bits `F` start at zero. A successful picture of `S` bits commits `F_next = max(0, F + S - B)` only when `F_next <= C`. All arithmetic is checked. This bounds every prefix of `N` accepted pictures by `N * B + C` NAL bits. A GOP boundary does not reset `F`. The contract supplies neither a lower rate bound nor decoder HRD conformance. Do not signal HRD parameters or describe this setting as a transport bitrate.

## Transactional QP trials

Start at the preceding successful picture's QP, or the initial QP for the first picture. Raise the starting QP by one if that picture exceeded `B`. Lower it by one if that picture cost less than `B` and `F` is zero. Otherwise retain it. Clamp this policy choice to 10 through 51, never a syntax value or motion vector.

Encode a tentative complete picture. Retry at the next larger QP only for excess-bit pressure, the picture's access-unit byte ceiling, or typed `MacroblockLimit` refusal. These causes need distinct trial outcomes before implementation; a generic `Error::Limit` is not a retry signal. Frame-count, cumulative-output, syntax, coefficient, allocation and internal failures remain errors. At most 42 trials reach QP 51. No fitting trial means refusal, not partial output.

Each trial has an immutable picture QP shared by prediction, transforms, slice syntax and deblocking. PPS initial QP stays 26, the slice carries its QP delta, and macroblock QP deltas stay zero. Keep the existing Annex A macroblock-bit and consecutive-motion-count admission. Reuse bounded scratch, not a full reconstruction per QP.

Commit the chosen reconstruction, reference, GOP identity, frame number, carried motion count, byte counters, last successful QP and `F` together. Failed trials and calls change none of them. The fixed-QP policy owns no controller state. Pictures remain synchronous, so flush emits no delayed picture, filler or budget refund.

## Acceptance before implementation

- Preserve fixed-QP snapshot bytes and complete three-decoder conformance.
- Independently parse each selected picture QP and replay complete NAL bytes to verify every prefix bound, exact capacity, byte-rounding boundaries, zero capacity, IDR bursts and checked-arithmetic refusal.
- Force all trials through QP 51 without a fit. Verify no output or state change, then retry a fitting picture against a fresh encoder at the same accepted prefix. Inject non-rate failures to prove they cannot trigger retries.
- Use synthetic flat and textured sequences to exercise both QP directions, frame-number wrap, crop, chroma and deblocking. Every accepted picture must completely match its reconstruction in CoreAV, pinned FFmpeg and pinned JM.
- Keep bounded fallible allocation, safe scalar production code, the unchanged default feature graph, DCO and exact-head main-allowlist verification. No quality or speed result follows from these correctness checks.
