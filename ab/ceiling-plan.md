# Arm64 encoder arithmetic ceiling

Reference source: `43742ab7fed01adfc8713b39497a0bd5cebd89b9`.

Collect native AArch64 Callgrind instruction counts on the scalar encoder for five existing prepared inputs, QP22 and QP28, sixty pictures, one thread, GOP32 and enabled deblocking. Preparation uses the qualified pinned x86 oracle on an x86 runner; encrypted transfer preserves the existing prepared hashes. No timing claim comes from profiling or preparation.

Build a plain scalar arm and an attribution arm from that exact source. The attribution arm adds only `inline(never)` to the scalar SAD/SATD, forward/inverse transform and quantization entry points. Both arms use the identical driver and release profile. Require complete stream and reconstruction identity in every cell. Retain both instruction totals so attribution overhead is visible. Collection is confined to the encoder call, not input reads, output writes or independent decoding.

Report individual kernel counts and two disjoint families: SAD/SATD and forward/inverse/zero-check/quantization/scaling. Avoid counting the SATD wrapper's descendants twice. Their entire instruction share is an optimistic removal ceiling, not an achievable speedup. Select the largest family with roughly two percent available; if neither qualifies, stop without an implementation pull request. Any implemented kernel still needs a native arm64 end-to-end comparison beyond run-to-run noise, complete scalar identity and the required arm64 verification job.

Retain metadata and counts, never transfer plaintext media to the development machine, and delete the throwaway workflow runs and branch after collecting the results.
