# Gating request: bundle 3b (B3b-1 to B3b-6)

Bars and method: `plan.md`, "Bars", and `docs/design/h264.md`, "Bundle 3b: bundle 3 with 4-wide luma blocks in
vector registers". B3b-7 (output) is met (`checks.md`); the local screen is met (`local/README.md`).

Arms: `base` = `main` at `1e7dd54b`, `k` = branch `perf/h264-bundle3b` (its code is `bd1f8b30`'s; `36dde216` changed comments
in `crates/` only, and any later commit of the branch before the run changes only `docs/`), null arm = `base`. Build each with `cargo build --locked
--profile dist --example decode -p coreav-h264`, `RUSTFLAGS` unset; FFmpeg 9.0.2 as the reference command. Check the
built `k` against `base` for identical output on the 32 training streams and the 64 held-out streams before timing.

1. B3b-1, B3b-3 and B3b-4, one thread: `scripts/h264_parity.py` (harness of `main`) on the helper VM class (Xeon @
   2.90 GHz, family 6, model 106), pinned to core 2, twenty rounds, the 32 training streams, discard and write,
   `--shuffle-seed 20261029 --null-arm base --gate-stream 36501d448eaa.0.720p.h264 --gate-every 4 --min-seconds 3`,
   host-health gate before and after each call.
2. B3b-2, one thread: the same host and options on the 64 held-out streams
   (`docs/measurements/h264-n2-heldout/list.json`), five rounds, discard.
3. B3b-5 and B3b-6, eight threads: `scripts/h264_parity_gcp.py` (harness of `main`), `c4-standard-16`, one thread a
   core, the 32 training streams, `--threads 8 --rounds 10 --min-seconds 3 --seed 20261029 --null-arm base
   --gate-stream 36501d448eaa.0.720p.h264` (the harness adds the in-call gate at eight threads), discard and write.

One valid run decides each; a re-run of the same arms only for a host fault as the hosts' void rules define it. A
timeout of the run's own script is a host fault only if the calls it cut off are re-run whole with the same binaries,
as bundle 3's held-out calls were. Evidence to `docs/measurements/h264-rounds/bundle3b/gating/` (large files on a
separate branch).
