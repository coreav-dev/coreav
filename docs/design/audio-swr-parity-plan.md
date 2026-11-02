# Exact default-path audio resampling investigation

Status: characterization plan, not a support claim. Refs coreav-dev/coreav#25.

The goal is native raw PCM input, verified equal-rate handling and a default swr-compatible rate path whose complete command outputs pass the unchanged exact decoded-output oracle. Dither and soxr are excluded. Commit this plan before any experiment. Keep the plan and any eventual implementation in one pull request; the reviewer reads this commit first. Synthetic characterization may run before review, but no held-out, corpus or judged run may run before the pull request merges.

## Sources and constraints

Derive a candidate from the [bandlimited interpolation equation](https://www.dsprelated.com/freebooks/pasp/Windowed_Sinc_Interpolation.html), the [modified Bessel series](https://dlmf.nist.gov/10.25.E2), and [FFmpeg's public resampler options](https://ffmpeg.org/ffmpeg-resampler.html). Read no FFmpeg or libswresample source, other implementation source, disassembly or source-level profiles. Measured coefficient tables must not become production constants.

The ideal kernel is `sinc(x) = sin(pi*x)/(pi*x)`, with its limit 1 at zero. A finite Kaiser-windowed version uses `I0(beta*sqrt(1-u*u))/I0(beta)` inside its support, with beta 9. The documented swr defaults constrain filter size 32, cutoff 0.97, phase limit 1024, exact rational phases when possible and linear interpolation otherwise. They do not fully specify finite arithmetic, support geometry, padding or draining.

Use only the binaries pinned in `scripts/oracle-ffmpeg.env`. Verify them with the shared pin checker before each capture, retain their hashes and version banners with the candidate source identity, and use `scripts/oracle-audio-args.txt` for audio input threads. The installed system FFmpeg is not an alternative oracle. Scalar CPU flags are diagnostic comparisons only; the default pinned invocation remains the target.

## Bounded characterization

Use deterministic synthetic PCM only, with a maximum of 768 oracle subprocesses, a 10-second timeout per call, a 300-second capture deadline and a 32 MiB output cap. Run captures in GitHub Actions. Local work is limited to small generation, replay and unit checks. Run no benchmark, fuzz campaign, corpus draw or performance acceptance test.

The first pass captures these pairs in this order: 44,100 to 48,000, 22,050 and 16,000; 48,000 to 44,100, 22,050 and 16,000; 8,000 to 44,100; and identity at 8,000, 44,100 and 48,000 Hz.

- Use one, two and three channels with different values per channel. Keep channel counts unchanged to separate rate conversion from rematrixing.
- Capture signed-16 and signed-32 integer paths and F32 paths. Include F32 output, integer output and diagnostic F64 output because the selected internal precision may depend on both formats.
- Use impulses at the first, middle and last samples and at neighboring interior positions; zero and nonzero steps; a signed alternating sequence; a deterministic low-amplitude irregular sequence; and very short lengths on both sides of the filter and input-frame boundaries.
- Compare a normal flush with deterministic filter-frame partitions of 1, 7 and 31 samples, without padding. Capture zero-length input as an outcome rather than assuming it must succeed.
- Capture default CPU behavior first. Repeat the floating path with SIMD disabled to locate arithmetic dependence, never to substitute a more convenient reference.

Retain raw synthetic output bytes, complete lengths, the exact argument vector, return status and input digest. A subprocess error, timeout, cap or incomplete capture is an experiment failure, not evidence of parity.

The confirmation wave uses a full-amplitude deterministic LCG sequence with seed `0x51A7`, multiplier 1,664,525 and increment 1,013,904,223 modulo 2^32. Its signed values are scaled by 0.75/2^31. Capture 1,021 frames for mono/stereo S16, S32, F32 and diagnostic F64 paths, with separate F32-to-integer paths; repeat the stereo F32 path in blocks of 1, 7 and 31. On changed rates, add constant lengths 18–31, 45–51, 64–68 and 92–101; isolated half-scale impulses at positions 64, 65, 95, 128, 159, 191, 223, 255, 288, 319, 351 and 383 of a 512-frame input; and mono/stereo LCG lengths 511, 512, 513, 1,023, 1,024 and 1,025. These 570 calls check arithmetic and raw-input packet boundaries without a corpus or judged input.

## Independent candidate and hypotheses

The third and final capture tests the narrowed S16-to-S16 candidate before any product implementation is pushed. The corrected strict startup hypothesis matches all 470 confirmation F32 outputs, and all 50 captured S16 pairs match; direct full-amplitude S32 and F64 paths still differ and are not candidates for implementation. This final capture omits the explicit dither option so it tests unmodified defaults. For every original pair and the additional rational pairs 11,025 to 48,000, 48,000 to 32,000, 96,000 to 44,100 and 44,100 to 96,000 Hz, capture mono/stereo LCG inputs at lengths 0, 1, 2, around `floor(2L/3)` (minus one through plus two), `L` and `2L` (minus one through plus one), 511–513, 1,023–1,025 and 2,053. Here `L` is the candidate's even `ceil(32/min(1,.97*output/input))`. Also capture alternating and exact S16 extrema sequences at 257 and 1,025 frames, and stereo LCG at 1,025 frames in blocks of 1, 7 and 31. The hypotheses are that strict startup and half-pending drain apply equally to S16, raw packet boundaries do not change the result, integer clipping follows the stored S16 width, and the rational kernel is not specific to the initial ratios. The existing call, duration and byte caps remain unchanged; this is synthetic evidence, not command coverage.

Keep the initial numerical prototype outside the product. Compute coefficients from the cited equations rather than recorded outputs. Evaluate two support models derived from the documented size: a continuous 32-tap window at the lower rate, and a finite tap-grid-centered support of `ceil(32/a)` coefficients adjusted for the cutoff. For the first model use half-width `16/min(1, output/input)` with the window centered on the output position. For the second use a window centered at `(length-1)/2` on the integer tap grid while the sinc follows the fractional position.

Use two cutoff interpretations, `a=.97*min(1, output/input)` and `a=min(1, .97*output/input)`. Use either per-phase normalization or one shared gain derived from the zero phase. These are eight independently computed recipes, not a parameter search. Use rational input positions so rate stepping does not accumulate floating-point position error. First compare the double-precision kernel geometry, then independently examine coefficient rounding and accumulation if a geometry survives. Do not embed harvested coefficient tables or pursue undocumented numerical tuning without a concrete hypothesis.

Interior impulses first distinguish support, phase center, cutoff and coefficient quantization. Steps distinguish normalization. Edge impulses distinguish zero extension, repeated endpoints and mirror extension. Short signals and complete output lengths distinguish startup delay and flush behavior. Irregular signals distinguish accumulation order after the coefficient and edge hypotheses have survived.

Record each hypothesis, its decisive cases and its result. Do not search an unbounded parameter space or alter the synthetic set to hide a mismatch. Permit at most three characterization captures: the initial pass and two further passes answering concrete hypotheses from the previous result. Float comparisons use stored bits and exact lengths; no error tolerance is an acceptance criterion.

## Implementation only after the evidence supports it

If the necessary behavior can be pinned down, add a typed raw signed-16 input declaration with input-scoped sample rate and channel count. Validate positive integral rates, numeric suffix syntax, frame boundaries, limits and option placement before output creation. Preserve complete frames without converting them through float on the equal-rate path. Do not silently accept rate options with stream copy or an unsupported decoder, encoder or container.

Keep FFmpeg-compatible resampling separate from the native decoder's current silence-extension and ceil-length contract. Its tests must cover rational stepping, coefficient construction, startup, chunk splits, drain, allocation limits and all accepted sample formats. No unsafe code is planned. Equal-rate bypass must independently preserve bytes and metadata and must not ignore a requested format or channel conversion.

Before claiming a new command, execute its complete unmodified registered argument vector and compare every selected output stream using the unchanged same-command V2 oracle. A matching numerical prototype or rate-only raw PCM command is not a full-command pass. A finite matching synthetic set is evidence, not proof of general parity.

## Stop and report

Stop without pushing a product implementation if a necessary choice in coefficients, normalization, accumulation, edges, delay or flush cannot be pinned down by the experiments, or if the candidate still differs after the bounded pass and there is no further concrete hypothesis. Report the unresolved choice, decisive sample bits and lengths, attempted derivations, source identities and what further work would require. Keep unsuccessful prototypes out of the product pull request.

High-pass triangular dither would additionally need an exact noise sequence, seed, scaling, per-channel progression and filter state. soxr would need a separate engine/version investigation and potentially a dependency and licence decision. Neither is part of this investigation. No tolerance, baseline, fixture, roster, frozen judging file or registered predicate may change.
