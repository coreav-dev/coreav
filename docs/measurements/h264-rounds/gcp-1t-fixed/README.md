# Calibration of the one-thread class at a fixed frequency

Plan and bars: `docs/design/h264.md`, "The one-thread class at a fixed frequency", committed before the runs. Result: **accepted**, every bar met (`calibration.txt`, `calibration.json`, from `python3 -I calibration.py ../../h264-training/es.json aa na`).

Host: Compute Engine `c4-standard-8`, `threadsPerCore=1`, `turboMode=ALL_CORE_MAX`, STANDARD, `us-central1-a`, Debian 13; Intel Xeon Platinum 8581C @ 2.30 GHz (family 6, model 207, stepping 2), CPU platform "Intel Emerald Rapids" (`instance.json`), 4 vCPUs, SMT off (`host.txt`). Every timed command pinned to core 2. Two VMs, started together, 2.1 hours each. Runner: `scripts/h264_parity_gcp.py --one-thread-class --turbo all-core-max --rounds 20 --seed 20261029` with the reference streams `36501d448eaa.0.720p` (900 frames) and `a6af290d9053.0.1080p` (300), the first also the in-call gate.

Binaries (`binaries.sha256`): `X`, the `decode` example of `main` at `1f221f59` (`cargo build --locked --profile dist --example decode -p coreav-h264`, `RUSTFLAGS` unset), and `X0`, the same tree with `--no-default-features`; `identity.txt`: their raw output is identical on all 32 training streams at the class frame counts. FFmpeg 9.0.2, BtbN `n9.0.2-17-g2a571b6068`. Streams: the 32 training streams of `docs/measurements/h264-training/es.json`, each checked against its SHA-256 before upload.

- `aa/`: arms `a` and `b`, two copies of `X`, null arm `a`.
- `na/`: `base` = `X`, `k` = `X0`, null arm `base`.
- `smoke-acm/`, `smoke-unr/`: the two plumbing runs before them (two streams a class, three rounds, `--turbo all-core-max` and `--turbo unrestricted`); their references, checks and frequency samples only. Not evidence for the bars.

Each run directory holds the four per-call reports (`g.<class>.json`, `.log`), `gate-refs.json`, `gate-checks.log`, `freq.json`, `instance.json`, `verdict.json`, `job.sh`, `progress.txt` and `host.txt` (the VM's name replaced by `VM`).

| bar | result |
|---|---|
| H-1 | **met**: neither verdict void; every report names the 8581C, family 6, model 207; frequency samples 3.074 to 3.085 GHz (AA) and 3.087 to 3.088 GHz (NA) |
| H-2 | **met**: 8 of 8 gate checks passed in each run, no call run again, no confirmed in-call drift |
| A-1 | **met**: `b / a` discard 1.0000, write 1.0000 |
| A-2 | **met**: discard [0.9997, 1.0003] (0.03%), write [0.9995, 1.0007] (0.06%) |
| A-3 | **met**: discard sd 1.0002, 720p 1.0002, 1080p 0.9999, 4k 0.9996; write 0.9994, 1.0002, 0.9997, 1.0008 |
| A-4 | **met**: largest deviation of one stream 1.06% (discard), 0.55% (write) |
| N-1 | **met**: `base~null / base` discard 1.0001 [0.9995, 1.0004], write 1.0001 [0.9999, 1.0008]; classes 0.9995 to 1.0005 |
| N-2 | **met**: no stream unresolved or timed again in either run; largest null deviation 0.69% (AA), 0.54% (NA) |
| V-1 | **met**: references NA against AA +0.02% (720p stream, 1.312 s both), -0.18% (1080p stream, 4.442 and 4.450 s) |
| V-2 | **met**: `X`'s discard ratio to FFmpeg 0.966 (NA) against 0.964 (AA), +0.21% |

Information. `X` against FFmpeg 9.0.2 on the training set, median of per-stream ratios (streams where `X` is faster): AA discard 0.964 (22 of 32; sd 0.923, 720p 1.014, 1080p 1.021, 4k 0.880), write 0.939 (26 of 32; sd 0.920, 720p 0.989, 1080p 0.983, 4k 0.820); NA discard 0.966 (21 of 32), write 0.940 (26 of 32). The decoder without its fast path, `k / base`: discard 1.055 [1.051, 1.063], write 1.051 [1.047, 1.057]. The largest round spread of any command was 1.117 (AA) and 1.068 (NA). The plumbing runs read 3.087 to 3.088 GHz at `ALL_CORE_MAX` and 3.268 to 3.281 GHz unrestricted, with FFmpeg's 720p reference at 1.311 s and 1.236 s.
