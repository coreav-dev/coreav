# Complete FLAC residual coding

This change extends the native encoder's residual representations under RFC 9639 Sections 9.2.7–9.2.7.2. The current four-bit-only coder can choose verbatim output when sparse large residuals make its available Rice codes expensive. This plan precedes implementation.

## Scope

Represent the residual method explicitly: four-bit parameters permit Rice parameters 0–14, while five-bit parameters permit 0–30. Represent each partition as either a Rice parameter or an escaped signed width. The reserved all-ones parameter identifies an escape, followed by a five-bit width from 0 through 31. Width zero represents an all-zero partition. Residuals that need 32 signed bits cannot use an escape and retain Rice coding.

Choose the complete residual representation by exact encoded bit cost, including method, partition order, every parameter, escape width and payload. A mean-derived Rice parameter may seed an exact search, but must not cap the result at 14 or replace cost comparison. Rice cost is discrete convex in its parameter, so walking to the adjacent minimum gives the global minimum without evaluating every legal parameter. Tests compare that result with independent exhaustive enumeration. Keep deterministic ties: the lower parameter wins within a partition, Rice wins an equal-cost tie with an escape, the lower partition order wins, and the four-bit method wins equal complete costs.

Keep the existing partition-order search from zero through four and its block divisibility and predictor warm-up rules. Keep fixed orders zero through four, LPC orders one through eight, coefficient precision, shifts, residual bounds, stereo selection, block sizes, metadata, sample feeding and atomic publication unchanged. Do not add an encoder option or widen source admission. Do not optimize residual generation in this change.

The writer emits the selected method and either the existing Rice code or the exact signed escaped residual. Folding and unfolding use integer arithmetic and retain the RFC's signed-32 residual bound, excluding the most negative value. All code remains safe Rust.

## Verification

- Compare selected costs with exhaustive legal-method, partition, parameter and escape enumeration, including ties and predictor warm-up accounting.
- Exercise Rice parameters 0, 14, 15 and 30 and escape widths 0, 1 and 31. Prove that a 32-bit signed width never becomes an escaped width or a reserved Rice parameter.
- Decode emitted residual bytes with the existing decoder and require exact samples and exact charged bit lengths, including mixed partitions, short blocks and partial-byte endings.
- Add complete-file fixtures that select escaped and five-bit paths and independently decode every sample with pinned FFmpeg. Preserve CRC, total-sample, MD5 and source-probe requirements.
- Retain the existing 93 CLI comparisons, failure tests and tiny-reference boundary without changing any assertion or tolerance. Runtime oracle gates run in Actions. Local verification covers compilation, formatting, clippy and short static checks.

## Sequence and measurement

This plan is the first commit of the residual-coder pull request. A separate later pull request will plan and optimize bounded residual loops, fixed prediction and scratch reuse. Its output must be byte-identical to this change's output, not to the former restricted coder's output.

Neither change makes a timing claim. The registered plan and bars in `docs/design/flac-output.md` and the measurement driver stay byte-unchanged. After both changes merge, one newly authorized comparison records a new round while retaining the former failed result. No judged input is run during these pull requests and no threshold is changed.
