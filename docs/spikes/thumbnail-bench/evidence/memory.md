# Decoder memory measurements (docs/design/decode.md, "Memory")

Peak address space (`VmPeak`, MiB) or resident memory (`VmRSS`, MiB) per process, by `memory_peak.py`. Host: 8 CPUs, 16 GB. FFmpeg 7.0.2 is the pinned static build.

The inputs:
- the large file is corpus v1 `ed60cdd24ba9…`: ProRes 4444, 8809×6314, 12 bits with alpha;
- `big.mkv` is its first 40 frames, copied into Matroska;
- the 1080p input is ProRes 4444 at 10 bits with alpha, from `testsrc2`.

Each list below gives the peak per run.

## FFmpeg reading the large file (`-frames:v 20 -f rawvideo /dev/null`), `VmPeak`

| `-threads` | VmPeak (MiB) |
|---|---:|
| 1 | 3486 |
| 2 | 4546 |
| 4 | 5995 |

## The fallback's command on `big.mkv`, output read by `cat`, `VmPeak`

These runs use the command before output and filter threads were pinned.

| `-threads` | VmPeak (MiB) |
|---|---:|
| 1 | 3679 |
| 2 | 4758 |
| 4 | 6548 |

With the output stalled for 30 s (`| (sleep 30; cat)`):

| `-threads` | VmPeak (MiB) |
|---|---:|
| 1 | 13759 |
| 4 | 16747 |

Pinning the output and filter threads (`-filter_threads 1`, output `-threads 1`), with input `-threads 1` throughout:

| CPUs (`taskset`) | Unpinned (MiB) | Pinned (MiB) |
|---|---:|---:|
| 0 | 2194 | 2273 |
| 0-7 | 3679 | 2567 |

## 1080p ProRes 4444, `VmPeak`

| `-threads` | VmPeak (MiB) |
|---|---:|
| 1 | 1165 |
| FFmpeg's own (8 CPUs) | 1749 |

## The corpus run's out-of-memory file, `VmRSS`

| Run | Process | VmRSS (MiB) | Outcome |
|---|---|---:|---|
| Reference `framemd5` | ffmpeg | 15309 | killed (SIGKILL) |
| `decode-oracle`, 120 packets, old code | worker's ffmpeg | 10253 | killed by the kernel |
| | worker | 1765 | |
| | `decode-oracle` | 4117 | |
| `decode-oracle`, budget 8 GiB, 4 threads | worker's ffmpeg | 6747 | `Limit` |
| `coreav thumbnail`, old code | ffmpeg | 890 | image |
| | worker | 602 | |
| | `coreav` | 508 | |

The kernel's log from that host (`dmesg`) records two out-of-memory kills of `ffmpeg`: anon-rss 10003376 kB and 10243908 kB.
