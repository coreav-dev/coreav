# Compatibility judging registration amendment

This is a forward-only amendment to the [registered compatibility round](../measurements/compat-0.1.0/README.md). The maintainer approved registering the existing judging changes for future measurements, with their behavioral differences stated below. It does not rejudge earlier runs, change published numbers, replace the candidate, or authorize repeating a missed bar.

## Fixed experiment and source roles

The candidate remains `4062842fc42babef9e6661a37bbb18f7e375a80c`. The workflow already builds both the candidate executable and the observer harness from that candidate checkout. The observer's driver and path-module sources must therefore be bound to the candidate, not to unused copies in the tooling checkout. Every frozen input consumed by tooling must still match the candidate exactly.

Keep the complete frozen and judging rosters, compatibility workflow, commands, input selection, oracle pins, installers and comparison tolerances unchanged. Correct source-role binding in planning, artifact recording and consumer verification. Retain explicit source identities so a verifier cannot substitute tooling-built observer artifacts or change source ownership.

Judging registration version 2 retains the same 36 `files` entries and separately registers the candidate commit and the Git objects for the two observer source paths. Artifact provenance version 2 retains all eight frozen paths, each with its owner and object. Planning and the media-free check recompute the candidate objects with full history. Measurement and report checkouts are shallow: they compare observer objects with the committed registration rather than requiring unavailable candidate history. A valid-looking but substituted observer object must fail that comparison. Version 1 registrations and bundles are rejected rather than interpreted with the new ownership rules.

## Reviewed judging changes

The original registration was committed in `660f830f8f7ceeaf25658caf3cc6cfc0b30a6208`. The reviewed main snapshot is `4464759f2ceda9ca572a0d04e27b13da2c3c5c71`.

### Checker process ownership and deadlines

The `r3.py` changes in coreav-dev/coreav#249 and coreav-dev/coreav#251 retain the process leader until group cleanup, reap adopted descendants, and replace command and stream-probe capture with bounded process-group handling. Keep those PID-safety repairs. Cleanup and descriptor failures can become infrastructure errors rather than successful results or leaked processes; unchanged output predicates do not imply identical failure outcomes.

There is a demonstrated cell-judgment difference in the checker deadline. The registered implementation starts its wait budget after `Popen` returns. The newer Linux implementation subtracts elapsed launch time from the existing total deadline. A media-free control executes the real `check_copies` function with a generated successful checker and a scaled budget:

| Control | Value |
|---|---:|
| Total checker budget | 0.8 seconds |
| Delay before the launcher returns the child | 0.6 seconds |
| Child time before writing its successful report | 1.0 seconds |

The registered code accepts the report after about 1.07 seconds and returns `passed`. Both newer revisions stop after about 0.80 seconds and return `failed: copy_check timeout`. This demonstrates a possible judgment change near the deadline; it does not establish an affected historical run. The production 8,580-second limit is unchanged. Future judging includes launch time in that total budget.

### MPEG-TS policy reference

coreav-dev/coreav#252 changes `policy_ref.py` beyond its former two-schema evaluator:

- It recognizes `coreav.probe-mpegts/2`; unknown-schema diagnostics now list three accepted schemas. A generated TS document under an empty policy changes from an unknown-schema reason to no reasons.
- TS times use integral ticks divided by timescale, container-format checks use `mpegts`, and stream bit-rate checks read `scan/bit_rate`.
- TS streams with codec `other` leave ordinary per-kind counts, duration candidates and stream checks; the unidentified-stream rule handles them instead. TS dropped-stream reasons use `codec` and do not use the non-TS declared-codec exemption.
- A TS null rotation follows the unknown-value branch rather than the non-TS not-allowed branch.
- Ten global codec-family mappings are added: `vvc`, `aac_latm`, `mpeg_audio`, `bluray_pcm`, `scte35`, `id3`, `epg`, `dvb_subtitle`, `dvb_teletext` and `arib_caption`. They can change policy interpretation in any accepted schema when a stream's `kind` is missing or conflicts with its codec family. A generated `vvc` stream with `kind=data` changes from data-by-kind to video-by-codec.

These evaluator changes do not reach the registered R2/R3 media comparison through its Matroska overlay. That consumer calls only `matroska_headers` and `matroska_block_frames`. Their complete twelve-function dependency closure and eight non-callable module globals are identical between the registered and reviewed revisions. A generated Matroska metadata/block control also returns identical nonempty tracks and frame bytes. This establishes neutrality of that comparison path, not of policy evaluation generally.

The record-provenance helper hashes the complete policy module for Matroska records, so that identity changes even though its reachable metadata helpers are unchanged. This amendment does not authorize rechecking cached historical records.

### VP9 source-validator exemption

coreav-dev/coreav#253 adds only `('scripts/vp9_corpus.py', 'ffmpeg_command')` to `oracle_audio.py`'s video-only validator exceptions. The reviewed function selects `0:v:0`, the VP9 decoder and video `framemd5` output. Running the old and new validator against that source removes one static audio-input diagnostic; the same source under a neighboring path retains it.

Runtime audio input arguments remain `-threads:a 1`. The argument-file owner, recognized formats and validator functions are unchanged. This changes the source-policy result of `oracle_pin.py --check`, not its binary verification or a decoded-sample comparison. It changes the registered source identity but not an R2/R3 media-cell predicate.

## Registration and verification

Commit the plan before implementation. Review and commit the source-binding and media-free digest-check changes before generating the judging registration from that immutable code commit. Generate the JSON in its own commit with the existing `compat_sources.py register --tooling` procedure. The registration remains explicit; CI never regenerates it automatically.

The existing unfiltered `deny` workflow must check the real registration's complete digest roster without building binaries, opening media or requiring corpus credentials. A stale-hash control must fail through the same command CI executes. Role-aware tests must reject changed tooling-consumed frozen paths, altered candidate observer provenance, missing sources and incomplete rosters while allowing unrelated development copies of the observer source to differ. The actual PR plan stage must pass before review. No new measurement job or required-check context is needed.
