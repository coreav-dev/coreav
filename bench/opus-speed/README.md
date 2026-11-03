# Registered Opus speed rounds

`round4.json` fixes the fourth round's bars, source coverage, timing order and resource bounds before its fresh draw. The qualifying capacity files retain complete work, not timings. Nothing here selects or downloads media, changes a decoder, or authorizes another round.

The collector freezes one roster with `draw`, `sources`, four disjoint `shards`, `inputs_sha256` and `exclusions_sha256`. Its complete derivation document has one entry per source and container, including failures. The exclusion document lists earlier source and prepared-input hashes. The collector and its immutable metadata records enforce the corpus rules; the runner independently refuses any listed overlap, missing cell or changed bytes.

Run each shard once on its otherwise idle, on-demand fixed-class VM:

```sh
python bench/opus-speed/speed.py --plan bench/opus-speed/round4.json \
  --roster roster.json --inputs inputs.json --exclusions exclusions.json \
  --tools tools.json --media media --work /mnt/opus --out-dir results --shard 0
```

The tools document has the eight executable roles and `lav_archive` used by the capacity pass. Their hashes, resolved baseline libraries, judging sources and Python/NumPy versions must still match. Install `requirements.txt` with `--require-hashes` and securely supply the R2 environment variables. These requirements retain the capacity pass's runtime and pin the checkpoint transport separately. Never publish credentials, input media or private logs.

`speed.py` owns roster validation, the three gates, timing-call validation and the one-shard state machine. It delegates raw/command identity, the oracle, bounded execution and closed process errors to the already-qualified `bench/audio-decode` owners. It does not copy or alter their judging code. Its result validator is also used by `report.py`, so invalid or partial work cannot acquire a median through the reporting path.

The alternative was to copy the old oracle and execution adapters into the new runner. Reusing their qualified owners keeps the agreement predicate and resource enforcement in one place. The new timing and reporting files live outside the capacity judging-source directory so adding them does not silently change the prerequisite's source identity.

Each invocation creates an exclusive local and durable round/shard start record before CoreAV use. It seeds every expected result slot and synchronously checkpoints between inputs, never during a timed call. An ordinary oracle disagreement is complete untimed work; a binary failure is terminal and leaves the remaining expected slots visible. No CLI switch resumes a judged invocation or overrides a bar. A recorded host fault and any authorized replacement remain separate decisions.

Summarize all four reports against the same frozen roster:

```sh
python bench/opus-speed/report.py --plan bench/opus-speed/round4.json \
  --roster roster.json --inputs inputs.json --exclusions exclusions.json \
  --out summary.json results-0.json results-1.json results-2.json results-3.json
```

Only a complete error-free set can yield a timing judgment. The report recomputes every median and checks every warmup, rotated round, work count, gate and expected cell. Bootstrap intervals and faster shares accompany the unchanged bars and minimums; they never turn a failed bar into a pass.

Synthetic tests use fake commands and fixtures, not a held-out draw. Real-media tool verification, collection and timing run remotely; timed work uses on-demand instances only.
