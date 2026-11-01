# Release VP9 decode cost

The release's static musl build has additional decode CPU cost compared with a GNU build linked to Debian's libvpx. This round isolates that additional cost. It does not attempt to close libvpx's remaining gap against FFmpeg's native VP9 decoder.

Neither tested candidate meets all acceptance bars. The second meets the large-file and no-regression bars, but its ten-file median still uses more CPU than Debian. It is not accepted as an overall parity fix.

## Plan and acceptance bars

The measured base is the copy-once head `065479c8a6d3a47a965301d4e5eba6ee10c88e68` of coreav-dev/coreav#178. The harness and candidate use that exact base, not the earlier measurement's source. The implementation branch is based on main after the copy-once change merged.

Profile the release-equivalent decode harness and the GNU/Debian harness at function level on a Compute Engine c4-standard-8 with SMT disabled, using the same ten B5 files as the earlier libvpx comparison. Separate profiling from timing. Retain allocation and frame-export work in both timed harnesses, and describe this integration cost beside any decoder comparison. Before choosing a change, test the ceiling of removing the implicated work in a diagnostic build.

The candidate and its exact build commands will be fixed in a commit before the judged run. The judged host is a c4-standard-8, four physical cores with SMT disabled, Debian 13, and the shared pinned FFmpeg 9.0.2. Run five position-balanced rotations per file, reversing their direction on alternate files. Use equal repetition units for short commands. Record CPU time, wall time, exit status and frame count for every decode invocation. Run pixel hashes outside timing on all ten files, requiring frame-by-frame identity between stock, candidate and FFmpeg.

## Fixed candidate and commands

The candidate selects `mimalloc` 0.1.52 and `libmimalloc-sys` 0.1.49 as the Rust global allocator of the Linux x86-64 musl release command. No codec control, codec build flag, target instruction-set floor, or C allocator override changes. The decoder worker and exec launcher receive `MIMALLOC_ARENA_RESERVE=131072` before startup, limiting the initial virtual arena reservation to 128 MiB so it does not already exhaust a small worker's address-space cap. The launcher still clears the environment before running the operator's encoder. The committed value is in `coreav-worker/src/mimalloc-arena-reserve-kib.txt`; candidate decode and hash harness calls receive the same value. Both allocator crates use the existing MIT license allowance, and their notices ship in `NOTICE`. Ordinary source builds and other release targets keep their system allocator.

The second candidate also appends exported libvpx rows in fixed 64-byte chunks on Linux x86-64 musl, with one final short tail. This keeps the copy in safe Rust and gives the compiler constant-size copies to inline instead of sending each variable-width row to musl's `memcpy`. The allocation remains fallible and is reserved once per plane. Other targets keep their existing row copy, and the lending path used by transcode is unchanged. The chunk loop adds a capacity check per chunk rather than per row, a cost the judged comparison must cover. The larger arena is a separate attempt to avoid bypassing allocator reuse for 8K-sized allocations while still passing the real 512 MiB worker tests. Neither change is accepted on its diagnostic rationale alone.

Build libvpx once with `packages/cli/scripts/codecs.sh x86_64-unknown-linux-musl PREFIX libvpx`. Each musl harness uses `cargo build --locked -p coreav-decode-oracle --profile dist --target x86_64-unknown-linux-musl --features FEATURES`, `PKG_CONFIG_PATH=PREFIX/lib/pkgconfig`, `PKG_CONFIG_ALLOW_CROSS=1`, and the `zig.sh` C compiler and archiver wrappers that `platform.sh` uses. The feature sets are committed, opt-in measurement variants; the tool's default hashing behavior does not change:

| Binary | Features |
|---|---|
| Stock timed (`do-rel`) | `libvpx,no-frame-hash` |
| Candidate timed (`do-alloc`) | `libvpx,no-frame-hash,release-allocator` |
| Stock hashes (`do-rel-hash`) | `libvpx` |
| Candidate hashes (`do-alloc-hash`) | `libvpx,release-allocator` |
| Debian timed (`do-deb`) | `libvpx,no-frame-hash`, GNU target, `dist` profile, system libvpx |

The measured host installs Debian's libvpx 1.15.0. Only timed harnesses omit MD5 generation and packed-frame hashing. The untimed hash binaries retain both. The stock CLI binary uses `cargo build --locked -p coreav-cli --features libvpx --profile dist --target x86_64-unknown-linux-musl`; the candidate adds the `release-allocator` feature, on the same copy-once base.

The second comparison reuses all five control binaries from the first comparison, replacing only `do-alloc`, `do-alloc-hash` and `coreav-alloc`. The stock command uses the measured base without the candidate. The stock and Debian harnesses use that base with the opt-in measurement variants added in `222c095`; their hashes are retained in both manifests. Build those controls before applying the second candidate's copy change. Building a stock musl harness from the second candidate's source would include its copy change even without the allocator feature and would not reproduce this comparison.

The five arms, in their initial order, are FFmpeg native VP9, stock musl, candidate musl, Debian, and a second identical FFmpeg command. Rotate this order by `(round + file_index) % 5` for five rounds, reversing the order on odd file indices. Each arm occupies every position once per file. A short file uses `ceil(2 / warm_FFmpeg_wall_seconds)` invocations per arm and round; longer files use one. Sum invocations within each unit, take the median of five units per arm and file, then compute ratios. The timed commands are:

```text
ffmpeg -nostdin -noautorotate -flags +unaligned -i FILE -map 0:VIDEO_STREAM -fps_mode passthrough -f null -
decode-oracle FILE --packets 100000000 --backend libvpx --max-memory-gib 16 --in-process
```

The host gate is three medium-preset H.264 transcodes of a fixed, generated 20-second 720p input, after one warm-up, before and after decode timing. Pixel-hash comparisons, end-to-end checks and function-level candidate profiles run after that gate. The end-to-end check transcodes each of the three large files with stock and candidate commands, `--encoder ffmpeg --preset medium --crf 23 --audio none`, and requires the encoded H.264 bytes to match. It records one wall time per side for context, not a transcode speed verdict.

The runner verifies and records all input and binary hashes before timing, records every invocation including repeated short-file calls, and aborts on any command error, missing or inconsistent frame count, pixel mismatch, or changed transcode bytes. FFmpeg's timed and hash outputs use passthrough frame timing so duplicate timestamps cannot drop frames. Reported intervals resample files 10,000 times with seed 518. The complete measurement and analysis runner is `scripts/vp9_release_cost.py`; it refuses incomplete, duplicate or aborted reports before computing the verdict. Its five rotations balance positions but not each arm's preceding neighbor.

Acceptance requires all of the following:

- The median candidate/Debian CPU ratio on the three 4K and 8K files is at most 1.02.
- No file's median candidate/stock CPU ratio exceeds 1.02, allowing only measurement noise rather than a regression.
- The median candidate/Debian CPU ratio across all ten files is at most 1.03.
- Every frame hash agrees, all commands succeed, and all frame counts agree.
- The before/after host gate drifts no more than 5%, and the identical-command control has a median ratio within 2% of 1.00.

Report each file's median and range, the median ratios with file-bootstrap 95% confidence intervals, and the fraction of files cheaper than FFmpeg. These bars target release/Debian parity, not a claim of being faster than FFmpeg. Such a claim needs an interval whose upper bound is below 1.00. A completed missed candidate is not rerun; only a recorded host fault permits a replacement run.

## First candidate: rejected

The allocator-only candidate at `f292e08605ae9d58d33161c511a4b192d0615b55`, with a 32 MiB initial arena, completed all 3,925 timed invocations with no command failures. All 5,534 frames matched stock and FFmpeg, and all three large-file transcodes produced identical H.264 bytes. The host gate ratio was 0.9834 and the identical-command median was 1.0011, so the result is valid and is not rerun.

It missed every CPU acceptance bar. The large-file candidate/Debian median was 1.0382, above 1.02. The ten-file median was 1.1783, above 1.03. The 8K candidate/stock ratio was 1.0271, above 1.02. Candidate/stock across files was 0.9231, with a file-bootstrap 95% interval of 0.8234 to 0.9488, but that improvement does not meet the requested parity goal. Candidate/FFmpeg was 1.4602, with an interval of 1.1372 to 1.5621. Only two of ten files used less CPU than FFmpeg.

The post-timing profiles still attribute 26.16% of the odd-width file's cycles to `memcpy`, and 6.78% on the profiled large file. The prior no-export diagnostic established room to reduce export cost, but changing the allocator alone did not remove it. The second candidate changes the safe row-copy implementation and the arena size, keeps these bars unchanged, and is judged separately.

The complete per-file medians, ranges, peak RSS and ratios are in [`first/analysis.json`](vp9-release-cost/first/analysis.json). Sanitized invocation records are in [`first/calls.jsonl.gz`](vp9-release-cost/first/calls.jsonl.gz), with the exact binary hashes in [`first/builds.json`](vp9-release-cost/first/builds.json), host class in [`first/host.json`](vp9-release-cost/first/host.json), and the two function-level profiles beside them. [`first/frames.json.gz`](vp9-release-cost/first/frames.json.gz) retains stock and candidate frame metadata and hashes, plus FFmpeg's per-frame hashes. This is an integration measurement with allocation and export, not a decoder-kernel benchmark. Each end-to-end transcode has only one observation per side and is retained as byte-identity and timing context, not a speed verdict.

Recompute the first verdict with `python3 -I scripts/vp9_release_cost.py --analyse docs/measurements/vp9-release-cost/first`. This needs no media or binaries. Exit status 1 is the recorded bar miss; status 3 would instead mean invalid host controls.

## Second candidate: overall parity still missing

The copy-and-128-MiB candidate was fixed at `67b0db8` before its comparison. All 4,900 timed invocations succeeded. All 5,534 decoded frames match stock and FFmpeg, and all three large-file transcodes produce identical H.264 bytes. The host gate ratio is 0.9825 and the identical-command median is 0.9994, so the result is valid and final for this composition.

The large-file candidate/Debian median is 1.0177, passing the 1.02 bar. Every file uses less CPU than stock, passing the no-regression bar. The ten-file candidate/Debian median is 1.1816, with a 95% file-bootstrap interval of 1.0700 to 1.2232, so the 1.03 overall bar fails. The candidate/stock median is 0.9289, with an interval of 0.8335 to 0.9529. Candidate/FFmpeg is 1.4460, with an interval of 1.0642 to 1.7071; two of ten files are cheaper than FFmpeg. The three large-file candidate/FFmpeg ratios are 1.1899, 1.1821 and 1.4440.

All seven files outside the large-file group remain at least 12% above Debian. The odd-width profile now attributes 17.56% of cycles to CoreAV's `LibvpxDecoder::send` and 11.29% to `memcpy`. Moving the export copy into constant-size Rust copies did not establish an overall CPU gain over Debian. The allocator and copy changes were measured together, so this comparison does not identify their individual effects. Comparisons between the first and second hosts are not paired timing evidence.

The replayable evidence is in [`second/analysis.json`](vp9-release-cost/second/analysis.json), [`second/calls.jsonl.gz`](vp9-release-cost/second/calls.jsonl.gz), [`second/frames.json.gz`](vp9-release-cost/second/frames.json.gz), [`second/builds.json`](vp9-release-cost/second/builds.json), and [`second/host.json`](vp9-release-cost/second/host.json). The per-file statistics retain medians and ranges for CPU and wall time, plus peak child RSS. CPU includes process startup, and child RSS can include its pre-exec measurement process. The two profiles and original host-gate values are beside the reports.

Recompute this verdict with `python3 -I scripts/vp9_release_cost.py --analyse docs/measurements/vp9-release-cost/second`; exit status 1 records the failed overall bar. Further work must test a different explanation for the remaining cost rather than repeat either completed candidate.
