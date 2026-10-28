# W1 probe baselines and sandbox

Throwaway spike code. Three small C programs written for this
benchmark from public interfaces only; no FFmpeg header or implementation
source was read.

- `libavformat_probe.c` — engine B of the W1 benchmark:
  dynamically loads the pinned LGPL FFmpeg 7.0.2 `libavformat` shared library
  and times `avformat_open_input`/`avformat_close_input` per request. With
  `--stream-info` it also calls `avformat_find_stream_info` (diagnostic engine
  B'). Only the public avformat/avutil API names documented by FFmpeg 7.0
  Doxygen are declared. The library is a black-box baseline;
  no CoreAV crate links FFmpeg.
- `sandbox_launch.c` — engine D sandbox: sets
  no-new-privileges, resource limits (1 GiB address space, 10 s CPU, no file
  writes, 16 open files, no core), closes standard input, and installs a
  seccomp filter that allows only the system calls a minimal `ffprobe` needs
  to read one file and write its output, killing the process on any other.
  The allowed list was determined by black-box syscall tracing of the static
  `ffprobe` on the fixtures (`evidence/ffprobe-syscalls.txt`); `execve` is
  allowed for the launcher's own transition into `ffprobe`.
- `rss.c` — a fork/exec/`wait4` wrapper that reports one child's peak resident
  set size (`ru_maxrss`), so memory per request is measured per process.

The minimal FFmpeg build (engine D's `ffprobe`) is compiled on a cloud host from the
checksum-pinned FFmpeg 7.0.2 release tarball.
