# Operator-built x264 shared-memory helper

This optional Linux executable is not a CoreAV artifact. Operators build it themselves against an independently installed libx264. Its original source uses the repository's MIT OR Apache-2.0 licence. A helper linked with GPL libx264 is a separately licensed GPL executable. Do not publish it as a CoreAV package or include it in a release archive, wheel, npm package, container image or default build.

The helper is outside Cargo's workspace and dependency graph. CoreAV never downloads, builds or links it. The ordinary pipe-fed encoder programs remain the default. Select the helper explicitly with `COREAV_X264_SHM_HELPER=/absolute/path/to/coreav-x264-shm` and `coreav transcode --encoder x264`. An explicit `--x264` continues to select a pipe-fed program instead.

## Ownership

CoreAV copies each checked frame into a new anonymous memory file, then seals it against writes, growth and shrinking before publication. The decoder never maps or receives that file. The helper opens it read-only, verifies the seals and exact length, maps it read-only and calls libx264. The parent closes its handle after acknowledgement. A helper that retains or forwards the file still sees immutable bytes, even after the decoder reuses its original slot. Files are never reused for another frame.

The existing process launcher enforces address-space, CPU, descriptor, file-write and deadline limits. Encoded output remains untrusted and passes through the existing H.264 reader and muxer. Protocol control fields travel on pipes, never in decoder-writable storage.

## Protocol version 1

All integers are little-endian. Standard input starts with the eight bytes `CAVSHM01`. Every frame is byte tag `1`, a strictly increasing sequence starting at 1 (`u64`), a presentation timestamp (`i64`), the exact packed frame size (`u64`), a path length (`u16`, at most 128), then that many UTF-8 path bytes. Paths are `/proc/<parent-pid>/fd/<fd>` and refer only to the sealed snapshot, never the decoder arena. EOF requests encoder flush.

Standard output starts with `CAVOUT01`. Tag `1` acknowledges a frame sequence (`u64`), only after `x264_encoder_encode` returns. Tag `2` carries an encoded-byte length (`u32`, 1 to 1,048,576), followed by that many Annex B bytes. Split longer output into bounded messages without altering the byte sequence. Encoded messages may precede their frame's acknowledgement and continue while flushing after input EOF. Unknown tags, out-of-order acknowledgements, short messages and oversized lengths are terminal errors. Diagnostics use standard error.

Configuration uses the existing x264 arguments plus `--coreav-shm`, `--coreav-timebase N/D` and `--coreav-vfr 0|1`. The supplied frame timestamps are in that time base. No compressed input, destination path or writable decoder memory is supplied.
