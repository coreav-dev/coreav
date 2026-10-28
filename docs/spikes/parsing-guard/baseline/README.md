# Isolated libavformat benchmark

libavformat is called in-process only as an isolated black-box benchmark
baseline.
No CoreAV Cargo crate links FFmpeg. No multimedia implementation source or
header was read. The C harness declares only public API contracts and the
AVPacket ABI prefix documented by [FFmpeg 7.0 Doxygen](https://ffmpeg.org/doxygen/7.0/structAVPacket.html).

The baseline dynamically loads a checksum-pinned conda-forge FFmpeg **7.0.2**
LGPL build. `libraries.lock.json` pins the actual relocated shared libraries;
`packages.lock` pins their complete external binary environment. The measured
runtime reports **LGPL version 3 or later**, `--disable-gpl`, `--enable-shared`,
`--disable-static`, and no nonfree flag. The package metadata's broader license
label does not replace the runtime check. An initially inspected PyAV 13.1.0
binary wheel was rejected because its configuration enables GPL; it was never
used as the baseline.

Upstream release: [FFmpeg 7.0.2](https://ffmpeg.org/releases/).
Binary provider: [conda-forge FFmpeg](https://anaconda.org/conda-forge/ffmpeg).
Provider source/build offer: <https://github.com/conda-forge/ffmpeg-feedstock>;
this link is attribution only, and the feedstock/source was not opened.
The binary environment retains its package notices. No linked binary, shared
library, codec implementation or copyleft source is committed or distributed.
Libraries remain replaceable. See [FFmpeg's legal page](https://ffmpeg.org/legal.html).

Prepare on Linux x86-64, with Python, a C compiler and the repository Rust
toolchain, from the repository root, with `S` naming the spike's directory:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
python3 "$S/baseline/prepare.py"
python3 "$S/baseline/check-heap.py"
cargo build --locked --release --manifest-path "$S/Cargo.toml" --bin paired
"$S/ensure-oracle.sh" "$PWD/$S/work/bin"
PATH="$PWD/$S/work/bin:$PATH" COREAV_REQUIRE_ALL_FIXTURES=1 scripts/gen-fixtures.sh "$PWD/$S/work/fixtures"
```

`prepare.py` verifies micromamba and every external package/library, extracts
only the installer executable, and copies only shared binaries. It never opens
multimedia source or headers. The fixed `/tmp/spike-c-lgpl` relocation prefix is
part of the measured pin; a conflicting existing environment is rejected.
The benchmark artifacts stay under ignored `work/`. Existing ordinary spike
reproduction stays separate and preserves the earlier measurements.

The cloud runner uses the immutable Stage 0 CPU image through its immutable
measurement-tools derivative. That derivative adds GNU time 1.9, Valgrind 3.19
and Debian libc 2.36-9+deb12u14; application packages and the base image digest
are inherited. Valgrind crashed under gVisor on the first experiment, retained
in `evidence/in-process/failed-run-01/`. The replacement allocation meter is
explained in [HEAP_METER.md](HEAP_METER.md).
Peak RSS comes from `rss.c`, a fork/exec/`wait4` wrapper: a child spawned
directly from Python would inherit Python's RSS high-water mark.

The Stage 0 evidence Volume supplies its original three hash-verified licensed
Big Buck Bunny files read-only. The runner includes every one of the 37 original
fixtures, plus the generated one-hour MP4 and actual 2.2 GB fragmented MOV.
It generates the large files using the inherited pinned static FFmpeg **binary**;
that binary's GPL build is permitted solely as a fixture-generation black box
and is never linked to the baseline. Corpus files are not trimmed or remuxed.

Both engines open and enumerate packets/timestamps in-process, without decoding,
`avformat_find_stream_info` or per-packet output. The baseline uses
`avformat_open_input` with default format detection and `av_read_frame`;
CoreAV uses its FileSource and default limits. Primary CoreAV samples additionally
read every emitted payload range into one reusable buffer, in chunks up to
1 MiB. Descriptor-only CoreAV results are a separate diagnostic. CoreAV's
seek/read-per-sample I/O, libavformat's buffered file I/O, PCM grouping,
structural rejection and subtitle sample treatment differ and remain visible.

Each of 15 paired rounds alternates engine order, pins one CPU, warms the entire
file before **each** warm engine run, and attempts `DONTNEED` with `mincore`
verification before **each** cold engine run. Host frequency/contention remain
uncontrolled. gVisor may leave all pages resident: those raw attempts are marked
unverified and cannot establish a cold-cache ratio. Process startup, dynamic
loading, final JSON and RSS tooling lie outside both internal timers.

A ratio is CoreAV/libavformat **throughput**, equivalently libavformat time /
CoreAV time; above 1 favors CoreAV. Per-file intervals resample complete pairs
10,000 times (seed 20261013). Group ratios divide summed times and resample
whole paired rounds over the fixed eligible file set. Failures and differing
payload totals remain in evidence but never become throughput claims. PCM's
96,000/94 packet grouping difference is retained; no parser output is rewritten.
Memory runs are separate from timing and use the same payload work. RSS includes
the whole process and mapped libraries; heap counts requested live allocations
through the instrumented allocation APIs. Neither counts the filesystem cache.

Each function has a 1,800 s timeout, 600 s startup bound, one container and
`retries=0`; subprocesses are independently bounded. No failed experiment is retried
automatically. A changed experiment can be run only with `--corrected`; its
failed predecessor stays in the evidence.
