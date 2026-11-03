# Registered Opus speed rounds

`round4.json` fixes the fourth round's bars, source coverage, timing order and resource bounds before its fresh draw. The qualifying capacity files retain complete work, not timings. Nothing here selects or downloads media, changes a decoder, or authorizes another round.

The collector freezes one roster with `draw`, `sources`, four disjoint `shards`, `inputs_sha256` and `exclusions_sha256`. Its complete derivation document has one entry per source and container. The only terminal statuses are `ok`, `flac_encode_failed`, `no_audio_stream` and `encode_failed`. Pending, running, interrupted and unknown states are refused before claiming a shard, not counted as completed exclusions. Successful entries retain `raw_frames` from complete Opus packet durations and `file_bytes` from the frozen file, as well as its hash and packet counts. The runner checks both against the bytes and decoded work before timing. The exclusion document lists earlier source and prepared-input hashes. The collector and its immutable metadata records enforce the corpus rules; the runner independently refuses any listed overlap, missing cell or changed bytes.

Run each shard once on its otherwise idle, on-demand fixed-class VM:

```sh
python bench/opus-speed/speed.py --plan bench/opus-speed/round4.json \
  --roster roster.json --inputs inputs.json --exclusions exclusions.json \
  --tools tools.json --media media --work /mnt/opus --out-dir results --shard 0
```

The tools document has the eight executable roles and `lav_archive` used by the capacity pass. Their hashes, resolved baseline libraries, judging sources and Python/NumPy versions must still match. Install `requirements.txt` with `--require-hashes` and securely supply the R2 environment variables. These requirements retain the capacity pass's runtime and pin the checkpoint transport separately. Never publish credentials, input media or private logs.

`speed.py` owns roster validation, the three gates, timing-call validation and the one-shard state machine. It delegates raw/command identity, the oracle, bounded execution and closed process errors to the already-qualified `bench/audio-decode` owners. It does not copy or alter their judging code. Its result validator is also used by `report.py`, so invalid or partial work cannot acquire a median through the reporting path.

The alternative was to copy the old oracle and execution adapters into the new runner. Reusing their qualified owners keeps the agreement predicate and resource enforcement in one place. The new timing and reporting files live outside the capacity judging-source directory so adding them does not silently change the prerequisite's source identity. A change to the shared execution owner still requires fresh qualification; older completed evidence remains unchanged and never qualifies different source bytes.

Each invocation creates an exclusive local and durable round/shard start record before CoreAV use. It seeds every expected result slot and synchronously checkpoints between inputs, never during a timed call. An ordinary oracle disagreement is complete untimed work; a binary failure is terminal and leaves the remaining expected slots visible. No CLI switch resumes a judged invocation or overrides a bar. A recorded host fault and any authorized replacement remain separate decisions.

Shard records are private. A failed leaf retains at most 2 MiB of stderr as base64, with capture counts, truncation and redaction flags. Configured credential literals are removed before retention. A downstream parser failure also retains the last leaf diagnostic. Capacity execution drops previous-operation diagnostics before preparing each new operation, so repeated failures cannot accumulate old buffers or inherit stale context. Interrupted calls remain fatal; their diagnostic byte counts and truncation flags may be incomplete even when stderr was retained. Local and durable checkpoints retain these bytes, but `report.py` publishes only the closed sanitized projection. Never publish a raw shard record or its logs. Linux command timing watches process exit as well as pipe activity, so closed pipes do not add a polling delay.

The plan binds separate planning-only observations from the spent-input qualification. `allowance.py` replays every recorded complete operation and startup call against its complete capacity receipt. Whole-job-truncated attempts remain in the provenance and their time is charged to the lifecycle, not used as completed calibration calls. It uses the larger of the new/reference channel-sample and packet-count ratios for each mode/container cell and takes the largest forecast across every TOC mode present in the input. A call allowance is its observed short-input startup cost plus the full-input operation cost multiplied by that ratio. Every input charges three gates and six passes of all eight engines, with the command's startup bounded by the two-command identity operation. Three observed control gaps cover the two runtime verifications and checkpoint. A factor of two covers planning headroom.

Before a start claim, all assigned cells must fit the registered shard window. The VM forecast separately charges setup, acquisition scaled by both frozen bytes and file count, the full shard window and a 300-second collection margin. The reporter recomputes these allowances rather than trusting a shard's numbers. If any shard does not fit, the complete roster is refused: no dropping long inputs, repartitioning, resampling or extending a window after the assignments are frozen. These are empirical planning allowances, not speed evidence or guarantees of completion; the actual per-call, shard and VM deadlines remain hard limits.

Summarize all four reports against the same frozen roster:

```sh
python bench/opus-speed/report.py --plan bench/opus-speed/round4.json \
  --roster roster.json --inputs inputs.json --exclusions exclusions.json \
  --out summary.json results-0.json results-1.json results-2.json results-3.json
```

Only a complete error-free set can yield a timing judgment. The report recomputes every median and checks every warmup, rotated round, work count, gate and expected cell. Bootstrap intervals and faster shares accompany the unchanged bars and minimums; they never turn a failed bar into a pass.

Synthetic tests use fake commands and fixtures, not a held-out draw. Real-media tool verification, collection and timing run remotely; timed work uses on-demand instances only.
