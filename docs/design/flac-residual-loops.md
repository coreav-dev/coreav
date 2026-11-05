# Byte-identical FLAC residual loops

This plan precedes the CPU-only implementation. The reference is the merged residual coder at `e3d1b79286c0764fed27fd9060093f881c3ebd52`, including the exact representation and tie rules in [Complete FLAC residual coding](flac-residual-coding.md).

## Scope

Replace residual construction's nested iterator and collecting pipeline with bounded loops. Specialize fixed orders zero through four with their integer finite differences. Keep the linear predictor's coefficient order, integer accumulation, arithmetic shift and signed residual admission unchanged. Do not change floating-point coefficient estimation, rounding or predictor search.

The stream owns one reusable residual scratch vector. Each candidate clears and fills it. A rejected or invalid candidate leaves the selected subframe unchanged. A winning candidate swaps its residual vector with the scratch vector, so subsequent candidates cannot overwrite selected samples. Reuse the scratch across subframes and frames, retaining at most one block of scratch elements. Keep all code safe Rust.

Leave Rice planning and writing unchanged, including both parameter widths, escaped partitions, exact costs, partition orders zero through four and every tie. Preserve fixed and LPC search order, LPC orders one through eight, precision, wasted bits, stereo assignment, framing, source admission, metadata, MD5 and publication behavior. Add no public encoder option.

## Verification

Compare complete encoded file bytes against the reference in one Actions job, using the same fixture producer, Rust toolchain and release build settings for both checkouts. The producer uses deterministic generated PCM, both widths, all channel counts, empty and tiny inputs, block boundaries and tails, correlated stereo, impulses, noise, polynomial and linear-prediction signals. Compare the complete file inventory and every byte, not only decoded samples or file hashes. Retain a sanitized manifest with both source commits, case identifiers, lengths and hashes. A missing, additional, failed or unequal case fails the gate. Intentional future compression changes need an explicit new baseline rather than silently refreshing it.

Unit regressions compare the new fixed and linear residual loops with scalar coefficient arithmetic. Cover every fixed order, LPC orders one through eight, shifts zero through fifteen, signed and side-channel bounds, overflow rejection and scratch reuse after rejected candidates. Keep existing tests and assertions.

Retain the existing complete-file native and pinned independent decoder checks, CRC, exact samples, MD5, source probe, all 93 CLI comparisons and the tiny-reference boundary. Run local compilation, formatting, clippy and short static checks. Runtime fixture and oracle checks run in Actions. Neither correctness fixture generation nor byte comparison runs judged measurement inputs.

## Sequence and measurement

This plan is the first signed-off commit. The correctness tools and implementation share this pull request. Byte identity is the acceptance condition for this optimization; no timing result or speedup is claimed here.

The registered plan `docs/design/flac-output.md` and driver `scripts/flac_output_measure.py` stay byte-unchanged. After this change merges, the separately authorized single comparison uses that registration and its unchanged bars. Its results are a separate pull request and retain the earlier failed round.
