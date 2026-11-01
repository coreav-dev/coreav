# Release VP9 decode cost

The release's static musl build has additional decode CPU cost compared with a GNU build linked to Debian's libvpx. This round isolates that additional cost. It does not attempt to close libvpx's remaining gap against FFmpeg's native VP9 decoder.

## Plan and acceptance bars

The measured base is the copy-once head `065479c8a6d3a47a965301d4e5eba6ee10c88e68` of coreav-dev/coreav#178. The harness and candidate use that exact base, not the earlier measurement's source. The implementation branch is based on main after the copy-once change merged.

Profile the release-equivalent decode harness and the GNU/Debian harness at function level on a Compute Engine c4-standard-8 with SMT disabled, using the same ten B5 files as the earlier libvpx comparison. Separate profiling from timing. Retain allocation and frame-export work in both timed harnesses, and describe this integration cost beside any decoder comparison. Before choosing a change, test the ceiling of removing the implicated work in a diagnostic build.

The candidate and its exact build commands will be fixed in a commit before the judged run. The judged host is a c4-standard-8, four physical cores with SMT disabled, Debian 13, and the shared pinned FFmpeg 9.0.2. Run five position-balanced rotations per file, reversing their direction on alternate files. Use equal repetition units for short commands. Record CPU time, wall time, exit status and frame count for every decode invocation. Run pixel hashes outside timing on all ten files, requiring frame-by-frame identity between stock, candidate and FFmpeg.

## Fixed candidate and commands

The candidate selects `mimalloc` 0.1.52 and `libmimalloc-sys` 0.1.49 as the Rust global allocator of the Linux x86-64 musl release command. No codec control, codec build flag, target instruction-set floor, or C allocator override changes. The decoder worker and exec launcher receive `MIMALLOC_ARENA_RESERVE=32768` before startup, limiting the initial virtual arena reservation to 32 MiB so it does not already exhaust a small worker's address-space cap. The launcher still clears the environment before running the operator's encoder. The committed value is in `coreav-worker/src/mimalloc-arena-reserve-kib.txt`; candidate decode and hash harness calls receive the same value. Both allocator crates use the existing MIT license allowance, and their notices ship in `NOTICE`. Ordinary source builds and other release targets keep their system allocator.

Build libvpx once with `packages/cli/scripts/codecs.sh x86_64-unknown-linux-musl PREFIX libvpx`. Each musl harness uses `cargo build --locked -p coreav-decode-oracle --profile dist --target x86_64-unknown-linux-musl --features FEATURES`, `PKG_CONFIG_PATH=PREFIX/lib/pkgconfig`, `PKG_CONFIG_ALLOW_CROSS=1`, and the `zig.sh` C compiler and archiver wrappers that `platform.sh` uses. The feature sets are committed, opt-in measurement variants; the tool's default hashing behavior does not change:

| Binary | Features |
|---|---|
| Stock timed (`do-rel`) | `libvpx,no-frame-hash` |
| Candidate timed (`do-alloc`) | `libvpx,no-frame-hash,release-allocator` |
| Stock hashes (`do-rel-hash`) | `libvpx` |
| Candidate hashes (`do-alloc-hash`) | `libvpx,release-allocator` |
| Debian timed (`do-deb`) | `libvpx,no-frame-hash`, GNU target, `dist` profile, system libvpx |

The measured host installs Debian's libvpx 1.15.0. Only timed harnesses omit MD5 generation and packed-frame hashing. The untimed hash binaries retain both. The stock CLI binary uses `cargo build --locked -p coreav-cli --features libvpx --profile dist --target x86_64-unknown-linux-musl`; the candidate adds the `release-allocator` feature, on the same copy-once base.

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
