# Fuzz campaign 2: the release gate on 82b02d3b

Plan: [`docs/design/fuzz-campaign.md`](../design/fuzz-campaign.md), the gate
plan fixed before this run (two 30-minute shards per target). Data:
[`fuzz-campaign-2.json`](fuzz-campaign-2.json) (`campaign.py report`).

- **Code:** `main` at 82b02d3b (`crates` tree `0619ad24`; the root
  `Cargo.toml` and `Cargo.lock` SHA-256 in the data); rustc 1.101.0-nightly
  (07e526db9 2026-10-19), cargo-fuzz 0.13.2.
- **Targets:** 63, 2 shards of 30 minutes each, 126 shards, 63 CPU-hours
  planned.
- **Seeds:** 91 MP4, MOV and M4A fixtures, 39 Matroska and WebM fixtures, 44
  transport stream fixtures, 320 hostile-suite files, 65 HLS indexes, and
  the corpus carried from campaign 1.
- **Completed:** 124 of 126 shards, 60.48 CPU-hours; the two others ended
  on a finding (0.52 CPU-hours more).
- **Corpus pack:** `fuzz-corpus.tar.zst`, 130,687,872 bytes, SHA-256
  `6f8bcb5095534146626aaad3be3c251e6b71399dd1bd3765329a8d8b9ed4483c`.
- **Verdict: not passed.** Two findings; the gate needs none, and every
  target's shards to complete.

| Target, shard | Kind | Cause (first reading) |
|---|---|---|
| `coreav-wasm` `ops`, 0 (after 293 s) | crash (harness assertion) | A read failure injected into the source does not fail a Matroska probe: it returns with "segment size runs past the end of the file", taking an I/O error for the end of the data. |
| `coreav-mpegts` `packets`, 1 (after 1,668 s) | out of memory (2,640 MB against the 2,048 MB limit) | Memory grew across runs: the artifact libFuzzer wrote is empty. |

Both are fixed before the gate runs again, on the fixed commit and with the
same plan; their reproducers are kept for the fixes.
