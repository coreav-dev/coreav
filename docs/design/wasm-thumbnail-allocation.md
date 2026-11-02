# Bounded thumbnail allocation ownership

`@coreav/wasm-h264` implements the one-IDR subset of the [registered thumbnail plan](wasm-thumbnail-memory-bars.md). This reference describes requested backing capacities and their lifetimes. It is not a hosted-memory measurement, a release verdict or permission to publish the package. The registered bars and the sixteen decoder-free gates are unchanged.

## Planning uses one account

`coreav_isobmff::PlanningBudget` shares one account across ISO or Matroska parsing, indexes, cloned configuration, packet listings, cut-point checks and selector candidates. Before each new backing store, the owner charges its complete requested capacity, including replacement capacity. The account never refunds charges or resets between window and full-listing attempts. Its 4 MiB ceiling therefore conservatively covers freed transients and simultaneous old/new allocations.

The parser retains that account for fragment scans, seekers and packet iterators. A failed charge is sticky and propagates through best-effort metadata/index/window fallbacks. `Allowlist` arrays, descriptions, strings, laces, run/edit arrays and thumbnail `InBand` maps use explicit reservations. The selected MP4 `Track` is borrowed rather than deep-cloned. Unrelated report APIs and caller-created deep clones are not covered by this route.

The capacity argument is specific to the pinned distribution compiler. Rust 1.99.1's `Vec::try_reserve_exact` requests the checked array layout; `RawVec` records the requested capacity, and `Global` returns a slice of that requested size rather than allocator-usable size. This proves requested backing capacity, not physical allocation size. Allocator metadata, alignment, holes, retained pages and page rounding belong to the separate 16 MiB bridge/retention partition. Changing the compiler requires checking this argument again.

The account itself has one `ArcInner<State>` allocation: two reference counts plus the state and target padding. Cloning the account does not allocate. This control block belongs to bridge bookkeeping. Fixed configuration-reader RBSP scratch uses at most 4,096 stack bytes per active reader; H.264 access and SEI parsing can overlap two such buffers. Native paths also pay that bounded stack-zeroing work. No instruction-count improvement is claimed.

The bounded selector uses the unchanged stable-sort comparator only while the pinned standard library uses its 4,096-byte stack scratch. It refuses a larger candidate count before allocating candidates rather than substituting a different order. The threshold is derived from the target's candidate type size, not a new memory bar. Native selection remains unrestricted. This additional planning-work refusal preserves arithmetic-overflow ties and can reject a valid full listing earlier than a lifetime-aware implementation.

Without a planning account, native collection growth stays with the vector's own push or extend operation, and native copies and strings keep their ordinary constructors. Accounting-only preflight does not reserve again on that route. Packet access reads initialize the 32-byte header scratch on every walk; the 4,096-byte SEI scratch is initialized only in the branch that reads SEI. These choices avoid making unrelated native packet listings pay the bounded route's maximum scratch work; the instruction guard still judges their actual cost.

## Decode consumes the selected plan

The bounded operation makes the native choice, then requires an offered keyframe with `start == target`. It restores that access unit through the shared packet reader, with at most 4 MiB including stripped headers and inserted parameter sets. It neither searches for another IDR nor retries with later packets.

`NativeH264::one_idr_with_admission` preflights the complete access unit. The callback receives the coded and cropped geometry of the SPS activated through the selected picture's PPS before picture allocation. The package independently applies the coded and visible side/product caps. Container dimensions and an unused initial SPS cannot authorize a larger picture. Reference declarations remain truthful; the one-IDR mode, not a fabricated one-reference declaration, removes the decoded-picture-buffer requirement.

Caller-lowered frame width, height and pixel limits constrain the codec's visible geometry in that same callback, not its macroblock padding. For example, caller limits of 1,920 by 1,080 and 2,073,600 pixels permit a coded 1,920-by-1,088 grid cropped to 1,080. The fixed coded-grid caps still apply before allocation even if the visible crop is small.

The codec owns its 32 MiB bound, including packed output. The caller owns the restored packet and planning account separately. The operation requires exactly one frame at the selected packet's timestamp. Decoder state and packet bytes drop first, then the plan and account drop before rendering. The source is the codec-produced CPU picture, not an arbitrary `Picture` with unknown capacity or a GPU download. The bounded path does not call the native retry timer or `Instant`; Node owns its external worker deadline.

## Rendering reuses the phase

The native reference and WASM operation call `Picture::resize_to_rgb_single_thread`, which shares the existing coefficient and colour-conversion implementation. Other native callers retain parallel rendering. Resolve SAR, rotation and output dimensions before allocating turned planes, RGB, taps or float channels. Excess output is a structured limit, including an excessive automatically derived dimension.

Let coded-source area be `Q <= 2,088,960`, output area `P <= 921,600`, maximum source side `S=1920` and maximum output side `D=1280`. A conservative packed 4:2:0 plane bound allowing odd crops is `Y=ceil((3Q+2S+1)/2)`. Mono uses less.

Simultaneous render payload is bounded by `2Y + 3P + 12P + 4Q + 4S + 4D`: source/turned YUV, RGB, all float channels, cached/spare rows, accumulator and neutral row. This deliberately combines some mutually exclusive lifetimes. The RGB-first final rotation holds less memory after float channels have dropped.

For the largest implemented filter radius, two axes retain at most `8*(4S+D)=71,680` weight bytes. Trimming happens in place. Add `(D_w+D_h)*sizeof(Taps)`, `H*(sizeof(&[u8])+sizeof((usize,Vec<f32>))+sizeof(Vec<f32>))`, seven plane descriptors and three channel descriptors. Complete cache/spare containers reserve once; cached rows transfer between them rather than duplicating buffers. Using the larger ordinary 64-bit layouts gives 28,755,498 bytes before allocator overhead. The WASM layouts are smaller. Requested payload and descriptor capacities fit the 32 MiB phase; this calculation does not prove page retention or hosted total memory.

## Encoding has bounded output and finite scratch

The decoded frame drops before encoding. Each bounded encoder starts with one fallibly reserved output store of at most 4 MiB. The writer checks length before every write and never grows that store. A format limit returns a structured error, not a partially successful image. Library-private allocations are admitted from their pinned source bounds; the wrappers do not promise to translate every dependency allocation failure into a structured error.

For JPEG, baseline interleaved `F_2_2` encoding keeps one packed RGB copy and three MCU-row buffers, not full-image coefficients. At width 1,280 the row buffers total 61,440 bytes. The RGB, packed copy, reserved output and audited heap arrays total at most 9,785,728 bytes, plus fixed target objects/descriptors.

For PNG, filtered input has `(3w+1)h` bytes. Compression writes to one fallibly reserved slice of at most `output_limit-57`; 57 bytes cover the signature and three chunks. An incomplete compression result is a limit failure. Filtered input and compressor state drop before container copying. The RGB, filtered bytes, row workspaces, compressed slice, final store and fixed compressor heap arrays total at most 14,173,045 bytes, plus target-sized fixed local state. The former growable compression helper remains only on the unrestricted native path.

For pinned `image-webp` 0.2.4, RGB literals use at most 45 bits per pixel; runs consume no more per covered pixel. Tree/header overhead is bounded by 5,843 bits. The private encoded frame therefore has `B <= ceil((45P+5843)/8)`, at most 5,184,731 bytes. Under the pinned vector-growth rule, eight-byte writes need at most 8 MiB capacity, with at most 4+8 MiB old/new overlap. The RGB picture, packed RGB, internal RGBA, private growth overlap and reserved final store total 25,993,216 bytes. Tree heaps and named fixed arrays raise the audited subtotal to 26,018,328 bytes. An outer writer alone would not establish that private bound. Some images can exceed the final 4 MiB output policy and are refused.

These bounds leave room below 32 MiB for the named fixed encoder objects and locals. Startup/stack readings, native counting-allocator evidence and the WASM page gates remain separate checks. Dependency or compiler changes require rechecking the source-owned formulas, not changing a bar to fit a result.

## Host ownership outlives the response

One lease covers source opening, Rust execution, output backpressure, deferred work and cleanup across every API of the H.264 instance. Root and Workers cancellation is cooperative during synchronous WASM. The Node entry keeps callbacks in a parent-side bounded I/O broker and executes the module in a dedicated worker. Its timer starts before opening; abort/deadline terminates that worker. Parent-side admission survives until termination, source cleanup and per-request port cleanup settle. Successful EOF and summary wait for cleanup. Cleanup failure quarantines the host.

Fixed memory input is charged by actual backing capacity. Platform range sessions use one native BYOB response and separate pre-allocation reader/result/response charges. Writes are at most 256 KiB; the exclusive stream has zero prefetch. Private test diagnostics observe actual lease/session counters, operation identities, source capacity peaks and output charges. Combined host peaks conservatively sum independently observed source/output peaks rather than claiming an exact simultaneous total. A custom producer or fake R2 binding does not establish a production platform-memory guarantee.

The installed Linux preflight binds every image, refusal and post-cleanup reading to its input, options, exact optimized artifact and isolate. Native counting-allocator tests qualify a 4K refusal by requiring total requested allocation below the source-derived minimum luma allocation with zero trough. They do not infer zero decoded pictures from WASM pages. Generated fixtures and raw captures stay outside the checkout; the driver prints only counts or a sanitized failure label. Hosted Workers memory, provider outcomes and the 32 MiB runtime reserve still require the separately authorized complete validation.
