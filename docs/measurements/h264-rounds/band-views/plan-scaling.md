# Plan S1: our 8T/1T scaling against FFmpeg's, per class (written before the run)

Build: main 2a3972b2, `--profile dist --example decode` (bin m-2a3972b). FFmpeg 9.0.2 (ff902). Host: this VM,
Xeon @ 2.90 GHz, 4 cores / 8 logical CPUs (same class as the A/B helper's VM).
Streams: the 32 training streams, class frame counts (sd 1800, 720p 900, 1080p 300, 4k 150).
Arms per stream, interleaved, order rotated per round, 5 rounds + 1 warm-up: us1 (`--threads 1 --discard`, taskset -c 2),
ff1 (`-threads 1 -f null`, taskset -c 2), us8 (`--threads 8 --discard`, unpinned), ff8 (`-threads 8 -f null`, unpinned).
Per command: wall (perf_counter) and from wait4 rusage: user+sys CPU, minflt, nvcsw, nivcsw. Median over rounds.

Decomposition (exact identity per stream): r8 = wall8_us/wall8_ff = W1 * I * B with
  W1 = cpu1_us/cpu1_ff (1T work), I = (cpu8_us/cpu1_us)/(cpu8_ff/cpu1_ff) (CPU inflation from 1T to 8T, ours over theirs),
  B = (cpu8_ff/wall8_ff)/(cpu8_us/wall8_us) (logical CPUs kept busy, theirs over ours: > 1 = we idle more).
"8T-only loss" = I * B. Reported per class as the median per-stream of each factor.
Decision rule (fixed now): a factor I or B whose class median is >= 1.015 (the earlier threshold, ~1.5% of a stream's
work) is attributed further (step 2). If neither factor reaches 1.015 in any class, the 8T axis has no item to build
from scaling alone and the report says so; per-stream outliers (I*B >= 1.10) are listed but do not by themselves open
a round.
Step 2 tools (bound before any build, each with its own plan): callgrind total Ir at 1 vs 8 threads (instruction
inflation: apron copies, extra work), minflt (first touch), nvcsw (blocking waits), diag timers (band-lag diag.patch).
