# Skip unusable AVC summary proofs

Review this plan commit before the implementation.

ISO probing accumulates summary eligibility in `summary_complete`, starting with the existing refusal of fragmented movies. The H.264 first-sample authority proof only updates that value. Its boolean result cannot restore eligibility once the value is false, and its source failures are already converted to a false proof result. It does not supply structured stream fields.

Short-circuit that proof with the current accumulated value rather than copying the fragment predicate or creating another admission rule. Keep eligible proofs, video information, summary refusal reasons, packet iteration and every output unchanged. Do not change HEVC, startup, the guard baseline or its tolerances.

Add source-read regression controls through the real ISO probe. A non-fragmented eligible H.264 track must read its complete first sample for authority. A fragmented H.264 track must not perform that read. Preserve the existing structured-output and summary fixture checks, including compatibility commands and exact oracle comparisons.

Use the existing GitHub Actions bench-guard PR-versus-base comparison on the same generated fixtures and release settings. Before judging, require successful commands and complete counts for ffprobe-mp4, C05, C13 and C20. Report each base/head instruction count and delta, including any cost. The expected recovery is fewer instructions on fragmented H.264 probes without output differences, and no guarded rise beyond the existing 5% tolerance. This is an instruction-count recovery, not a wall-clock or FFmpeg speed claim. A missing recovery is a final result, not grounds to retune the baseline or rerun a narrow miss.
