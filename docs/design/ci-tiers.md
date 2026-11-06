# Shorter PR validation

Status: planned, not in effect. This first commit changes no executable check. The implementation follows confirmation of this plan in the same pull request.

## The measured problem

The maintainer's direction is a routine PR gate of 20–30 minutes, with full compatibility checks after merge and daily. A correctness PR runs the applicable full checks once at its final head. A regression visible only to those checks can therefore reach main before detection.

The 2026-11-06 API sample contains 2,325 runs and 12,391 jobs across 41 current and historical workflow records. The four required workflows have up to 60 runs per event. Other workflows have up to 60 recent relevant runs. Equal job names in historical workflows are not pooled with the current workflow.

Each successful required cohort uses the latest selected run of `ci.yml`, `deny.yml`, `python.yml` and `npm.yml` at one event and head SHA. Its interval starts at the earliest workflow creation and ends at the last required report completion. A source cohort means Linux `check` actually started. It does not mean every platform job ran. Failed, cancelled and unfinished heads are excluded from successful quantiles, not counted as fast completions.

| Successful cohort | Heads | Median minutes | p90 minutes |
|---|---:|---:|---:|
| Required PR source heads | 32 | 21.27 | 25.78 |
| Required merge-group source heads | 50 | 25.04 | 41.91 |
| HEVC-triggered PR heads, all enumerated PR Actions | 20 of 58 | 43.58 | 79.73 |
| VP9-corpus-triggered PR heads, all enumerated PR Actions | 30 of 59 | 36.65 | 64.85 |
| Audio-triggered PR heads, all enumerated PR Actions | 32 of 59 | 31.66 | 55.27 |

The heavy-path rows use separate exact-SHA enumeration of PR Actions, rather than requiring membership in the 32-head source sample. They overlap. They exclude external app checks, reviews and later runs, so they are not unconditional merge-readiness measurements. In the HEVC sample, 19 trigger runs were cancelled, nine failed and one was still running. The corresponding successful-only quantiles omit those outcomes.

`ci-report` finishes last on 15 of the 32 source PR heads, `npm-report` on 14 and `deny` on three. The main serial costs are fixture command checking and npm WASI parity. Queue delay also matters. One merge-group Linux check waited 31.83 minutes after job creation before executing. Full H.264 acquisition took 24.70–34.10 minutes in four merge-group legs, despite a warm full-conformance median near four minutes. A warm median alone is not a reason to retain the full acquisition on every push.

## Changes to the critical paths

### Keep npm parity complete and use bounded workers

The npm parity step takes 13.78 minutes median on PRs and 16.40 on merge groups. Native package compilation takes 3.00 and 3.40 minutes, respectively. WASI compilation takes 1.85 and 2.12 minutes. The parity test currently runs each native/WASI comparison serially.

Enumerate the existing fixture, command and three path-variant combinations before execution. Use at most four asynchronous workers inside the existing test and job. Each worker owns an exclusive scratch directory. Within each case, native execution, capture, cleanup, WASI execution and byte comparison remain ordered. Preserve the bounded output capture and child-failure behavior. Compare inside each worker and retain counters plus the earliest failure, not all cases' binary outputs. After active workers drain, report the earliest failing enumeration index deterministically.

Keep every existing output, exit-code, stderr, successful-command and temporary-file assertion. Require every enumerated case to execute exactly once. Keep `outputs > 50` and the complete successful-command set. Native launcher and installation lifecycle tests stay whole. One-worker mode remains available for verification. No new runner jobs, artifact transfers or package build variants are needed.

### Split fixture command checks without dropping them

The current `isobmff-fixtures` job takes 18.60 minutes median and 19.50 p90 on PRs. Move its command-coverage step, frame-list step and informational R1 step verbatim into `isobmff-command-compat`, covered by `ci-report`. Their PR medians are 1.77, 2.02 and 3.43 minutes. Keep all comparators, baselines, profiles, flags and R1 verdict handling unchanged.

Both jobs consume the same run-bound fixture artifact. On a full run, fixture preparation in `ci-changes` generates the set even on a cache hit before publishing it. Reused main maintenance keeps its existing cache behavior. The full-run generation obligation moves to that producer rather than disappearing. No consumer independently regenerates the shared set.

The base fixture job retains stream and packet comparison, dependent known-divergence checks, schemas, hostile inputs, policy checking, audio and text extraction, and the text known-divergence audit. The latter takes 3.77 minutes but stays in the base job to balance the two branches. The new job builds the same required command variants and retains its reports under a separate artifact name.

## Planned job tiers

These are intended changes, not the active required-check table. Durations below are successful PR job execution median/p90 in minutes unless marked otherwise. They exclude runner queues. Matrix rows group related legs without adding their parallel durations.

| Jobs | Measured execution | Intended tier |
|---|---|---|
| `ci-changes` | 0.32 / 0.42 | PR and merge group, with fresh shared fixtures on full runs |
| `check` | 14.85 / 16.82 | PR and merge group, unchanged configurations |
| `check-macos`, `check-windows` | 11.13 / 13.08; 10.57 / 11.37 | PR and merge group with the existing platform-path filters |
| `msrv` | 0.92 / 1.00 | PR and merge group |
| `fuzz`, two shards | 6.20–8.33 medians | PR compilation and existing short affected-target runs; no queue campaign |
| `isobmff-fixtures` | 18.60 / 19.50 before split | PR and merge group, base fixture checks |
| New `isobmff-command-compat` | modelled near 11–12 | PR and merge group, unchanged moved checks |
| `isobmff-writers` | 10.35 / 11.30 | PR and merge group |
| `isobmff-decode` | 13.38 / 15.21 | PR and merge group, including complete AAC-enabled transcode fixture targets |
| `mkv-fixtures`, `ts-fixtures` | 8.18 / 8.69; 7.55 / 8.09 | PR and merge group |
| `conformance` | 3.87 / 4.10 on PRs; merge-group legs reached 37.93 | PR and merge-group smoke; full H.264 moves after merge, daily and dispatch |
| `vector-width`, `modes-identity`, `kernels-arm64` | 0.83–1.03 medians | PR and merge group, including all current Clippy invocations |
| `training-identity` | 6.00 / 6.99 | PR and merge group |
| `bench-guard` | 14.32 / 21.05 | Required PR and merge-group base comparison, unchanged |
| `svt-av1` | 6.99 / 11.73 | PR and merge group, retaining its existing tests and licence coverage |
| `transcode-compat-shard`, three legs; `transcode-compat` | 8.18–9.18 shard medians; 0.27 merger | After merge, daily and final-head dispatch, outside `ci-report` |
| `conformance-arm64` | main-only today | Full-suite workflow after merge, daily and dispatch |
| `main-cache` | main-only today | Existing reused-main cache maintenance |
| `ci-report` | 0.05 / 0.07 | Required PR and merge-group report |
| `deny-checks`, `deny` | 13.88 / 14.83 work; report seconds | Unfiltered required checks, unchanged |
| `python-changes`, `python-fixtures`, `linux-x86_64`, `python-report` | fixture median 0.60; wheel median 6.37 | Existing required Python path and same-tree reuse |
| `npm-changes`, `package`, `smoke`, `npm-report` | package 20.38 / 24.90 | Existing required npm path, complete parity with bounded workers |
| Label-selected Python and npm platform builds | opt-in today | Existing opt-in, tag, scheduled and manual behavior |
| `pr-text` | short, edit-triggered | Existing PR text gate, not a merge-group ruleset context |
| HEVC `headers`, native synthetic smoke | headers 0.46–0.60 medians | PR validation, with actual native comparisons rather than reference preparation alone |
| HEVC `hevc-vectors`, `profile-census` | 5.50 / 12.73; 19.82 / 27.48 before the full inter census | After merge, daily and final-head dispatch, sequenced after coreav-dev/coreav#429 |
| VP9 `vectors`, `adapter-contract` | vectors 11.80–14.01 medians; contract 0.92 | Existing PR comparisons, including the complete 353-vector set |
| VP9 `interpolation-fuzz` | about 11.1 per platform | Compile and run 30 seconds on PRs; retain the 600-second pass after merge, daily and dispatch |
| VP9 `corpus`, `corpus-summary` | slow shard 19.60 / 24.51 | After merge, daily and final-head dispatch; the contract stays on PRs |
| Audio `raw-pcm`, `flac`, `mp3`, `mp3-transforms`, `opus`, `aac` | 0.20–14.85 medians; AAC p90 17.12 | Existing PR checks, retaining their complete lists and qualification controls |
| `h264-encoder-conformance` | 2.18 / 2.47 | Existing fast encoder checks, including strict Clippy |

Other workflows retain their existing scope and events. In particular, publication and release procedures do not change. The frozen legacy compatibility workflow and judging registration stay byte-identical. A separately registered current-head R3 regression is a follow-up, not part of this implementation.

### Smoke selection does not change registered lists

Select H.264 smoke streams by exact name from `crates/coreav-h264/tests/data/conformance-streams.json`: `BA1_Sony_D`, `CABACI3_Sony_B`, `cavlc_mot_frm0_full_B`, `HCHP1_HHI_B`, `HPCV_BRCM_A`, `alphaconformanceG`, `CVPCMNL1_SVA_C` and `MR1_BT_A`. Apply the same selection to acquisition and both fast and safe decoding, with the existing thread counts and comparisons. Require exactly those eight distinct in-scope names and results. Unknown, absent or extra results fail. Full checks still use all 99 admitted streams and unchanged hashes.

The HEVC smoke uses the four existing `bench/hevc-conformance/synthetic.json` recipes: `main10-ctu16-tu4`, `main10-ctu32-tu16`, `main10-bypass` and `main10-bypass-changed`. Verify the pinned references and compare actual native output for every recipe. This is Main10 synthetic smoke, not the complete C1 or C2 verdict. Existing header and unit tests remain. Full checks retain all 144 admitted C1 streams and all 54 C2 roster identities once coreav-dev/coreav#429 is integrated.

VP9 and audio retain their complete existing conformance lists because a new subset adapter would add more risk than their measured execution justifies. These are supersets of smoke coverage. No roster, baseline, tolerance or frozen judging file changes.

## Full checks and final-head evidence

Keep heavy codec checks in their owning workflows where practical. A non-required full-suite workflow owns the H.264 full runs and the moved transcode shards. It prepares the registered fixture artifact once and keeps exact shard completeness, byte binding and merger checks. Full work runs after relevant main pushes, daily without path filtering, and by manual dispatch. It does not reuse a merge-group smoke result as evidence of full coverage.

One trusted `workflow_run` reporter observes completed full-suite workflows. It executes main's reporting code, not code or artifacts from the observed branch. It accepts only this repository's main-push and scheduled runs and reads complete latest-attempt job metadata. Its isolated token needs `actions: read` and `issues: write`, with no new secret. PR, merge-group and branch-dispatch results cannot create issues.

Use fixed workflow/job/result labels and public run links, never arbitrary logs, stderr, media names or artifacts. Create or update one bot-owned issue per failing job class. Deduplicate run attempts and reconcile simultaneous issue creation without cancelling another report. API failure or incomplete result evidence fails reporting rather than producing a false success. The owning team fixes or reverts within a day. No automatic revert or issue closure is introduced.

For a correctness PR, require successful manual full-suite evidence at its exact final head before enqueueing. Use the latest dispatch and latest attempt from the same repository and require every applicable heavy job and matrix leg to succeed, not skip. A later failure or new commit invalidates the evidence. Do not infer it from a same-named green `ci-report`. A read-only verifier checks this predicate for the merge desk. The manual work is once per final head, not on every push or in the merge queue.

Do not edit `hevc-conformance.yml` until coreav-dev/coreav#429 merges. If the other changes are ready first, exclude that file and state that HEVC automatic latency remains unchanged. Apply its tiering in a small follow-up. Never claim the conditional HEVC improvement as shipped by the other changes.

## Forecast and its limits

Replay each of the 32 PR and 50 merge-group successful source cohorts using its own job and step timestamps. Keep observed queues, unchanged jobs and report admission delays. Do not add median stage times together or assume that moving work eliminates hosted-runner queues.

The model adds 0.4 minutes for fresh common fixture production. The new fixture job receives its moved measured steps plus four minutes for duplicate setup, compilation, transfer and admission. Npm native tests receive a conservative 0.5-minute allowance plus 0.1 minutes of worker overhead. The pool's effective speedup is an assumption, not a measured result. H.264 smoke is assumed to take at most five execution minutes; that assumption must be checked on Actions. No queue improvement is credited to the heavy-tier moves.

| Source cohort scenario | PR median / p90 minutes | Merge-group median / p90 minutes |
|---|---|---|
| Observed baseline | 21.27 / 25.78 | 25.04 / 41.91 |
| Planned changes, effective 1.5× npm parity | 18.63 / 23.40 | 20.47 / 41.83 |
| Planned changes, effective 2× npm parity | 17.17 / 23.40 | 19.06 / 39.81 |

These forecasts describe routine required validation on the matched source cohorts, not every heavy-triggered PR, an unconditional service guarantee, or the final-head manual exception. Heavy-path measurements are reported separately above. Moving full suites prevents their automatic execution on every PR revision, but a numerical forecast for their full final-head manual duration is not justified.

The plan does not forecast a sub-30-minute merge-group p90 with the observed queues. It reduces serial execution and repeated full-suite demand without inventing queue relief. Measure ordinary PR and merge-group runs after implementation, report whether worker speedup and smoke assumptions held, and retain failed and unfinished outcomes. Any later runner or merge-queue policy change needs its own decision.

## Atomic implementation and verification

After plan confirmation, change the active required-check table, workflow dependencies and triggers, `workflow_check.py` tier/merge-group rules, `test_main_ci_plan.py` and the affected `docs/agents` rows together. This first commit only links the plan from the current design note. Rows must not claim enforcement before it exists.

Preserve the prescribed filtered-workflow per-job groups, matrix suffixes, PR-only cancellation, independent main/merge-group run groups, report behavior and deny's existing whole-run exception. New required jobs use the same forms. A main push may reuse only a successful same-tree result of the same workflow. That reuse covers the reduced gate, never the full tier. Required context names stay `ci-report`, `deny`, `python-report` and `npm-report`.

Do not remove or weaken deny, licence checks, DCO, pr-text, workflow checking, unsafe-site checking or any strict Clippy configuration. Bench-guard remains a head-versus-base comparison in PRs and merge groups and a baseline comparison on main. The intentional reductions are full H.264 and composed-transcode coverage on each PR/queue run, VP9's per-PR fuzz duration, and full HEVC/corpus work on every codec push. Their original full predicates remain in the full tier.

Verification includes complete workflow-checker self-tests, main-plan scheduling and matrix tests, changed-path tests, exact smoke membership and result-completeness tests, shared-fixture binding, reporter trust/idempotence tests, and final-head evidence tests. Test the worker executor on synthetic children for exact case coverage, isolated outputs, deterministic failures, bounded output, launch errors and cleanup. On Actions, compare one-worker and parallel npm runs over identical generated bytes and require equal complete case coverage. Run the resulting ordinary PR gate and observe actual queue outcomes. Keep all heavy verification on Actions.
