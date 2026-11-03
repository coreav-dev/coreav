# VP9 pixel reconstruction round

## Purpose and scope

Reduce repeated pixel work while preserving every decoded sample and the existing decoder boundary. This is a development-only diagnostic round. Its predictions are not acceptance bars, it casts no performance vote, and it changes none of the correctness, speed or support requirements in [the VP9 design](vp9.md).

This plan and the [single-kernel exception](vp9-interpolation-unsafe.md) are the first commit of the implementation pull request. An independent reviewer clears both before implementation starts. The machine-readable [plan](../measurements/vp9-pixel-round/plan.json) fixes the roster, arms, settings and predictions. This pull request contains the plan, exception, implementation, driver and tests. Its implementation gate is bit-exact output under the unchanged correctness checks, not a timing result. The timed diagnostic runs only after this pull request merges, against its merged commit. Its results go in a separate pull request after that run. Plan review alone never authorizes a pre-merge timed run.

Only these changes are covered:

1. Reuse overlapping wide-loop-filter sums in safe Rust. Compiler autovectorization is allowed, but no explicit filter SIMD kernel is added.
2. Specialize motion compensation and add the one AVX2 horizontal interpolation kernel described below.
3. Copy visible reference rows in bulk instead of extending a vector one sample at a time.

Further explicit SIMD kernels, multithreading, parser changes, release feature changes, backend-selection changes and the next optimization round are out of scope. The native decoder remains opt-in.

## Baseline data flow

`coreav-vp9::tile` resolves references and motion vectors, then supplies `ReferencePlane`, `PredictionMut` and `Sampling` to the kernel crate. Their constructors check visible geometry and storage extents. The baseline inter kernel allocates a horizontal intermediate and evaluates two eight-tap convolutions, including per-tap visible-edge clamping. It rounds and clips between passes. The baseline loop filter calls `filter.rs::wide`, which rereads overlapping neighborhoods for each output. After filtering, `Planes::picture` packs visible rows for the stored reference picture. `Picture::copy` separately copies the shown picture.

The decoder and backend keep ownership, memory charging, publication order and failure behavior. Kernels read samples and filter tables, never bitstream bytes. The new dispatch stays inside the kernel boundary.

## Scalar changes

### Wide-filter reuse

Read the original edge neighborhood before writing any result. Use a bounded local array and a sliding sum for the overlapping replicated windows, with the separate center term retained. The 16-tap case currently performs 224 edge reads for fourteen outputs, and the eight-tap case performs 48 for six outputs. Reuse the original eight or sixteen values instead. Preserve mask decisions, edge order, replicated endpoints, integer rounding, clipping and read-before-write behavior for both sample types and every allowed bit depth.

Keep the literal specification formula as a test reference. Test nonflat and flat neighborhoods, extreme legal sample values, every filter size and strength, and both memory orientations. No SIMD intrinsics or unchecked memory access belong in this change.

### Motion-compensation specialization

Separate integer, horizontal-only, vertical-only and two-pass work on grids with both steps equal to 16. Keep the general safe scalar path for scaled grids. Establish interior support once per block, then use checked row windows rather than repeating coordinate clamps for every tap. Edge blocks retain visible-edge replication. Use bounded scratch for the unscaled specialization and preserve the existing maximum storage and memory-limit behavior.

The public sample trait permits `u16` values above the nominal depth, and callers can pair a sample type with another allowed depth. Preserve those results. In particular, a zero-phase `u16` prediction is not an unchecked copy. Only a proved byte copy may skip the original clipping operations.

### Visible-row packing

Keep `Memory` as the allocation owner and charge retained capacities as before. After validating each visible row, extend the already-budgeted destination with the visible row slice. Preserve odd chroma dimensions, padded working strides, plane order and exact output lengths. Do not redesign the reference pool or remove the shown-picture copy.

## One AVX2 arithmetic kernel

Choose a concrete `u8` wrapper through the private sealed sample trait. Reject generic sample-pointer casts and a byte-conversion buffer for every output group. The alternative prepared generic byte window keeps the sample trait unchanged, but repeatedly converts and copies overlapping windows and admits more sample representations than this round needs.

The only arithmetic entry is `horizontal8_avx2(&[u8; 15], &[i16; 8]) -> [i32; 8]`. It computes eight horizontal dot products on an unscaled eight-bit grid. A private detected capability proves AVX2 availability. Its safe wrapper reads exactly fifteen visible bytes through checked fixed-size arrays and returns eight sums. It forms no raw pointer and writes no destination memory. The [exception record](vp9-interpolation-unsafe.md) fixes its arithmetic proof and unsafe boundary.

Detect once per admitted prediction block. Preserve the public generic entry and select this concrete wrapper through the existing sealed representation boundary. Admit only eight-bit `u8` requests, steps of 16, a checked whole-block interior support and at least one full eight-output group. Source and destination constructors do not promise trailing row padding. Width tails, edge blocks, other depths, `u16` storage, scaled steps, unsupported CPUs and other architectures use scalar code. A fast-path refusal writes nothing. Once processing begins, scalar tails finish the admitted block.

Horizontal phase zero uses the scalar specialization. Rounding, clipping, vertical filtering and compound averaging remain shared safe Rust. No second SIMD kernel is hidden in those stages. The `safe` feature excludes the SIMD module and forbids unsafe code in the kernel crate. It also supplies the scalar-only diagnostic arm without changing decoder environment handling.

## Verification before timing

- Preserve every existing test and comparator. New optimized scalar tests compare with the literal scalar formula, not with the optimized implementation itself.
- Compare all four filters and all phase pairs, compound on and off, dimensions from 1 through 64 in a bounded coverage matrix, byte extremes, impulses, ramps and alternating samples. Include cases where intermediate clipping and per-pass rounding affect the answer.
- Check widths around each eight-output boundary, exact-minimum slices, nonzero destination offsets, row-gap sentinels, the final visible row, every edge and corner, negative and extreme origins, steps from 1 through 80, `u16` samples through 65535 and mismatched type-depth pairs.
- On x86-64 with AVX2, directly exercise the single kernel and compare its sums with the safe twin. Also exercise detection refusal and fallback. Automatic-versus-scalar comparisons alone do not prove that SIMD ran.
- Run automatic and `safe` kernel tests and unchanged C1 vector and boundary checks on hosted Linux x86-64 and macOS arm64. Keep the existing oracle and corpus checks strict. Add the interpolation differential fuzz target and keep C3's required duration. Add the exact unsafe inventory rows and run its checker and the similarity guard.

Only trusted arithmetic unit tests run on a local development computer. Media, oracle execution, fuzz campaigns and measurements run on hosted CI or the measurement host.

## Frozen diagnostic method

### Inputs and source binding

Use exactly the six original SHA-256 identities in `plan.json`, in order, with two inputs per resolution class. They belong to the existing development roster and not its held-out partition. Verify each original hash before preparing the first 120 packet-copy frames. Bind the prepared bytes and packet work once, then use the same prepared file for every arm. Refuse replaced, omitted, duplicate, reordered or relabeled identities. Do not choose substitute files after a failure.

The baseline source is `8f894f51d02795a16c63c35a2361dc1a9783281b`. Before any timed capture, the driver verifies that the implementation pull request named in the plan is merged and that its recorded merge commit equals the requested candidate commit. An open or merely approved pull request, a pull-request head artifact or a different commit cannot satisfy this boundary. Build the candidate timing binaries from that merged commit.

Record the reviewed plan commit, its content hash, the verified merge receipt and merged commit, source snapshots, compiler version, Cargo features, profile overrides, binary hashes, oracle pin, host receipt and exact commands. The old profile motivates predictions only. Fresh baseline and merged-candidate measurements share the new host. A later implementation change makes the result historical, not evidence for the changed source.

### Arms and host

Use one otherwise idle on-demand `c4-standard-16`, Intel Emerald Rapids, SMT off, `ALL_CORE_MAX` and the standard PMU. Verify the actual instance receipt and CPU, require AVX2, and pin every decoder to the same logical CPU. Both native and FFmpeg decoder thread counts are one. No spot timing and no global `target-cpu=native` or target-feature overrides.

Build baseline, merged-candidate automatic and merged-candidate `safe` on hosted Linux CI with Rust 1.99.0, the locked dependencies and `dist`, which inherits release with fat LTO and one codegen unit. Build the attribution binary and count-only ceiling there too. Transfer only media-free binaries and content-bound provenance to the measurement host. Retain symbols without changing optimization. Use the same scratch receive-and-drop adapter in all native arms. It consumes and counts every shown frame and does no hashing during timing. A separate mode of that adapter computes hashes before timing. Record the adapter hash and mode. It changes no decoder or kernel behavior.

Use the shipped pinned FFmpeg 9.0.2 native `vp9` decoder with `-threads 1`, null output, passthrough frame timing and explicit demux encoder time base. Repeat its prefix 80, 40 or 5 times for 720p, 1080p or 4K and normalize to 120 frames. Its frame count must equal `120 * loops`. Do not hide mux warnings or decoder diagnostics.

For FFmpeg component attribution only, build symbols from the exact commit in the pin, using the source-blind recipe. Delete the source tree, archive and build log unread before profiling, disable documentation, strip no symbols and retain no debug sections. Request function names only, never source lines. Time the attribution build against the shipped asset and disclose drift. The shipped asset remains the timing baseline. Component costs scaled from the attribution build remain estimates even when whole-program drift is small.

### Ordered execution and failure handling

After plan review, first check a throwaway safe count-only ceiling that removes the targeted repeated pixel work. Record its source and instruction counts on the development inputs before implementing the real candidates. This untimed instruction-count check reports no elapsed-time comparison or speed ratio and is not the post-merge timed diagnostic. Its nonidentical pixels are expected and disqualify it from all correctness and before-after timing results. This is not an additional product arm or a new acceptance threshold.

The `ceiling` mode of the conformance workflow runs this checkpoint on hosted Linux, using Callgrind's instruction count and no elapsed-time metric. It takes the first registered input of each resolution class and feeds eight original packets, with no substitute selection. Both scratch builds use the registered baseline source and the same receive-and-drop adapter. The cutout removes interpolation and wide-filter arithmetic and replaces reference packing with a zero-filled allocation of the same size. It preserves frame consumption and allocation ownership, but its changed reference pixels may also change later pixel-dependent branches. Its instruction difference is an optimistic diagnostic, not a bound on the real implementation's time. Full source identities, commands, counts and acquisition records are sealed; the public summary contains only counts and work totals by resolution. This smaller checkpoint does not change the six-input, 120-frame timed method.

Only after the implementation pull request merges, verify the merged-commit boundary and run a small preflight on the same driver and inputs before the timed diagnostic. Every real native arm and the symbol build must match the pinned oracle's complete 120-frame hash sequence and dimensions. Check two repeated oracle sequences and the full repeated null-output work count, including the input that previously exposed timestamp quantization. Exercise each source, feature and profile combination before freezing its build. No timer result substitutes for a pixel or work check.

After one warm-up per arm and input, collect five unprofiled rounds for baseline, merged-candidate `safe`, merged-candidate automatic, pinned FFmpeg and its attribution build. Use the plan's arm list as the order seed for even-numbered inputs and its reverse for odd-numbered inputs, counting from zero. Round `r` rotates that seed left by `r` positions. Each arm occupies each order position once per input. Record wall, user and system time, work counts, exit status and diagnostics. Use paired native ratios within each round, report per-input medians and ranges, then the median of those input estimates per class. Report both total candidate gain over baseline and its remaining time ratio to pinned FFmpeg. Compare the observed automatic gain with the frozen prediction, including misses. Report the incremental automatic gain over the same merged commit's `safe` arm separately.

Profile separately from those timings. Capture one complete call graph for each native arm and the symbol build on each input, with `cycles:u`, DWARF stacks of 32 KiB, native frequency 199 Hz and FFmpeg frequency 999 Hz. Use a sufficiently sized recording buffer and record loss diagnostics. Normalize exclusive period weights to each arm's unprofiled user CPU, not profiler-inflated elapsed time. Freeze the classifier with the driver before the run. Follow actual function owners and semantic callers, never a type merely mentioned in a generic helper. Keep interpolation, other inter work, filtering, transforms, intra, entropy and coefficients, reference packing, shown-picture copying and unresolved work separate. Do not add inclusive parent and child costs. Disclose unknown leaves, lost samples, missing callers, small sample bins and unequal reference-storage work.

Keep all required cells. A subprocess error, unexpected diagnostic, missing frame, pixel mismatch, missing input or incomplete profile makes the capture incomplete. Do not exclude the cell or retune the arm from its result. A recorded host fault can resume only affected work. Otherwise retain the incomplete outcome and seek a new decision before a replacement run. The driver refuses to overrun its resource and lifetime bounds. No held-out input is opened and no performance bar is judged.

## Predictions fixed before timing

These are hypotheses for the class median of paired `baseline time / merged-candidate automatic time`, using unprofiled `dist` user CPU. Wall-clock gains are reported beside them. They are not measured ceilings or promises. Keep these ranges fixed through implementation review and the post-merge run.

| Resolution | Predicted total speedup |
|---|---:|
| 720p | 1.5 to 3.5 times |
| 1080p | 1.6 to 4.0 times |
| 4K | 1.5 to 3.2 times |

The model assumes roughly twofold to eightfold less interpolation cost, twofold to sixfold less wide-filter cost and fourfold to tenfold less packing cost. All three factors are unmeasured. Interpolation dominates the smaller classes, while wide filtering dominates 4K. Unchanged filtering decisions, transforms, entropy, frame management and scalar vertical work limit the total. The one horizontal AVX2 kernel does not make the whole interpolation component eightfold faster by itself.

In the separate results pull request after the merged run, report a missed prediction without changing this range. Report all before-and-after component times and ratios, the scalar contribution, the measured SIMD increment and the remaining FFmpeg gap. These six development prefixes cannot establish general parity or clear the later held-out speed gates.

## Driver

After merge, dispatch `vp9-conformance` on `main` with mode `build-round` and the full implementation merge commit as `candidate`. The build verifies the live merge receipt and unchanged reviewed plan before creating media-free artifacts. Its [build recipe](../measurements/vp9-pixel-round/build.json) pins the source-only symbol build and native symbol-retention overrides. The compile-only adapter contract runs before merge without creating timing artifacts.

On a Linux staging host with corpus credentials, use `scripts/vp9_pixel_round.py fetch --candidate COMMIT --media BUNDLE/files` from the merged source checkout. This verifies the merge boundary again and fetches only the six frozen identities, with shared request and byte caps and a retained acquisition receipt. Credentials are not transferred to the measurement host.

Run the bundle's `scripts/vp9_pixel_job.sh` through the existing VM job runner on the registered host, staging its observed `instance.json` before execution. The script installs the profiler on Debian 13, caps subsequent outbound traffic at 1 GiB and bounds capture to 6,000 seconds. The driver checks binary and script bytes, claims the bundle exclusively, binds original and prepared packet bytes, completes every pixel and repeated-work preflight, then runs the frozen timing and profile schedules. Failure retains an incomplete capture and deletes temporary clips; an existing claim is not silently reusable.

Replay with `python3 -I BUNDLE/scripts/vp9_pixel_round.py replay --out CAPTURE --summary SUMMARY.json`. Replay checks retained subprocess output rather than trusting stored pass flags, including complete pixels, packet-copy binding, frame work, profile graphs and loss counters. It recomputes paired gains, ranges, symbol-build drift, exclusive component estimates and frozen-prediction matches. A failed replay replaces a stale summary with an incomplete result. Raw captures remain private; the generated summary contains only class labels, input indexes, commands' numeric outcomes and aggregate components.
