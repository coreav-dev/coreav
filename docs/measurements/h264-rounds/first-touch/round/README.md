# First-touch round: checks and local timing

Design and bars FT-1 to FT-7, committed before any timing: `docs/design/h264.md`, "Threading round: a new picture's
first touch on the thread that decodes it". `base` = `main` at `538265c4` (decoder code of `13e4dfc4`, binary
`m-13e4`), `ft` = code commit `68c8f059`, null arm = a second copy of `base`'s binary (`m-13e4-n2`); all built with
`cargo build --locked --profile dist --example decode -p coreav-h264`, rustc 1.99.1, `RUSTFLAGS` unset
(`binaries.sha256`).

## Checks (FT-6)

- Identity (`identity.txt`, `ident.sh`): `ft` at 1 and 8 threads against `base` at one thread, the 32 training
  streams at the class frame counts, md5 of the written frames: 64 of 64 identical.
- Conformance (`conformance.txt`): 97 of 97 at 1, 2 and 8 threads, `--kernels detect` and `scalar`, `--mode fast` and
  `safe` (`scripts/h264_conformance.py` on this branch).
- `cargo test -p coreav-h264`, clippy `-D warnings`, fmt: clean. New unit test
  `a_canvas_sizes_its_bands_as_it_reaches_them`.

## Local timing (`local/`)

A development VM of the helper VM class (Intel Xeon @ 2.90 GHz, family 6, model 106, 8 vCPUs), builds rotated each
round, the 32 training streams at the class frame counts, discard, nothing else running.

| | all 32 | faster | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|---|
| eight threads, wall, five rounds (`ab8.py`): `ft` | **0.9940** [0.9904, 0.9984] | 24 of 32 | 0.9951 | 0.9891 | 0.9963 | 0.9969 |
| the null arm | 0.9983 [0.9964, 1.0025] | 20 of 32 | 0.9991 | 0.9975 | 0.9979 | 0.9981 |
| eight threads, CPU time: `ft` | 0.9972 [0.9960, 0.9995] | 23 of 32 | 1.0001 | 0.9969 | 0.9943 | 0.9988 |
| one thread pinned, CPU time, six rounds (`ab1t.py`): `ft` | **0.9965** [0.9954, 0.9975] | 30 of 32 | 0.9986 | 0.9964 | 0.9952 | 0.9959 |
| the null arm | 0.9996 [0.9992, 1.0005] | 18 of 32 | 1.0003 | 0.9995 | 0.9994 | 1.0002 |

The gating rule's two conditions are met: eight threads 0.9940 (at most 0.995) with the upper end 0.9984 below
1.000, and one thread 0.9965 (at most 1.005). The gating run is requested.

On this host the 4k class moved by 0.3%, far less than the probe's doubling cost there (1.4162 on `c4-standard-16`).
So FT-2 (4k at most 0.985) may be missed; it stays as committed. One thread gained 0.35%, which the round did not
expect (FT-5 is a guard): a new picture's chroma is no longer written before it is decoded. One to four pictures a
stream are new at one thread, so this is likely the one-thread share of the same cut.
