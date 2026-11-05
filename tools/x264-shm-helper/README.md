# Operator-built x264 shared-memory helper

This optional Linux executable is not a CoreAV artifact. Operators build it themselves against an independently installed libx264. Its original source uses the repository's MIT OR Apache-2.0 licence. A helper linked with GPL libx264 is a separately licensed GPL executable. Do not publish it as a CoreAV package or include it in a release archive, wheel, npm package, container image or default build.

The helper is outside Cargo's workspace and dependency graph. CoreAV never downloads, builds or links it. The ordinary pipe-fed encoder programs remain the default. Select the helper explicitly with `COREAV_X264_SHM_HELPER=/absolute/path/to/coreav-x264-shm` and `coreav transcode --encoder x264`. An explicit `--x264` continues to select a pipe-fed program instead.

## Build and test

Install libx264 development files independently, then run `make` in this directory with a C11 compiler and `pkg-config`. For a private installation, set `PKG_CONFIG_PATH=/absolute/prefix/lib/pkgconfig`. The build requires libx264; it does not download dependencies or skip a missing library.

Run `make test` to test the built helper on Linux with Python 3. Set `X264=/absolute/path/to/x264` to also compare complete Annex B bytes with an independently installed matching x264 executable. Without an x264 executable, only those optional comparisons are skipped. `make clean` removes the local executable.

## Ownership

CoreAV copies each checked frame into a new anonymous memory file, then seals it against writes, growth and shrinking before publication. The decoder never maps or receives that file. The helper opens it read-only, verifies the seals and exact length, maps it read-only and calls libx264. The parent closes its handle after acknowledgement. A helper that retains or forwards the file still sees immutable bytes, even after the decoder reuses its original slot. Files are never reused for another frame.

The existing process launcher enforces address-space, CPU, descriptor, file-write and deadline limits. Encoded output remains untrusted and passes through the existing H.264 reader and muxer. Protocol control fields travel on pipes, never in decoder-writable storage.

## Protocol version 1

All integers are little-endian. Standard input starts with the eight bytes `CAVSHM01`. Every frame is byte tag `1`, a strictly increasing sequence starting at 1 (`u64`), a presentation timestamp (`i64`), the exact packed frame size (`u64`), a path length (`u16`, at most 128), then that many UTF-8 path bytes. Paths are `/proc/<parent-pid>/fd/<fd>` and refer only to the sealed snapshot, never the decoder arena. EOF requests encoder flush.

Standard output starts with `CAVOUT01`. Tag `1` acknowledges a frame sequence (`u64`), only after `x264_encoder_encode` returns. Tag `2` carries an encoded-byte length (`u32`, 1 to 1,048,576), followed by that many Annex B bytes. Split longer output into bounded messages without altering the byte sequence. Encoded messages may precede their frame's acknowledgement and continue while flushing after input EOF. Unknown tags, out-of-order acknowledgements, short messages and oversized lengths are terminal errors. Diagnostics use standard error.

Configuration uses the existing x264 arguments plus `--coreav-shm`, `--coreav-timebase N/D` and `--coreav-vfr 0|1`. The supplied frame timestamps are in that time base. No compressed input, destination path or writable decoder memory is supplied.

## Timecode clock

Using `--tcfile-in PATH` also requires `--coreav-clock-program /absolute/path/to/x264` and `--coreav-vfr 1`. Supply an independently installed matching x264 CLI. Omit `--fps` in timecode mode; only the CLI's default `25/1` is accepted if it is supplied explicitly. Constant-rate mode retains its explicit frame-rate settings and does not launch a clock program.

Before opening the main encoder or reading frames, the helper runs that CLI without a shell, using exactly these arguments:

```text
--demuxer raw --input-res 16x16 --tcfile-in PATH --log-level info --no-progress --muxer raw --output - -
```

The child reads standard input from `/dev/null` and writes standard output to `/dev/null`, so the probe consumes no frames or video. It inherits the helper's process group and resource limits, including the parent's deadline enforcement. Only its three standard descriptors survive execution. The spawn actions require glibc's `posix_spawn_file_actions_addclosefrom_np`.

The CLI owns timecode clock selection. Its zero-input probe must exit successfully and emit exactly one complete `timecode [info]: automatic timebase generation N/D` line, with both integers between 1 and 2,147,483,647. The helper rejects missing, duplicate, malformed or truncated diagnostics, embedded zero bytes, and more than 8,192 diagnostic bytes. A failed or oversized probe produces no protocol output; oversized probes are killed and reaped. An incompatible CLI is rejected rather than replaced with a guessed clock.

`--coreav-timebase` still defines the protocol timestamps, independently of the probed encoder clock. In timecode mode, the parent reads CoreAV's generated v2 file sequentially, just as the pipe-fed CLI does, and transmits its millisecond values as exact integer nanoseconds. It accepts only nonnegative decimal values with at most six fractional digits and bounded lines; incompatible custom timecode files are rejected. Constant-rate mode uses consecutive frame ticks instead. The helper rescales each nonnegative timestamp to the encoder clock with checked integer rational arithmetic, rounding to the nearest tick with ties upward. Negative timestamps and results above the signed 64-bit range are rejected. This arithmetic requires compiler support for unsigned 128-bit integers.
