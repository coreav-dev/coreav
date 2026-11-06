# Shorter PR validation

Status: implemented in this branch. The first commit recorded the plan and changed no executable check; implementation follows in the same pull request. The forecasts below are not measured worker improvements. HEVC tiering remains deferred.

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

`ci-report` finishes last on 15 of the 32 source PR heads, `npm-report` on 14 and `deny` on three. The main serial costs are fixture command checking and npm WASI parity. Queue delay also matters. At head `815ab24b`, Linux `check` queued for 31.83 minutes and executed for 14.93. At head `28b2365a`, `isobmff-decode` queued for 28.55 minutes and executed for 13.42. The worst five merge-group cohorts all used attempt 1. Only one of 50 had any rerun (`deny`, attempt 2). Cross-SHA ejection is not established by this sample. Full H.264 acquisition took 24.70–34.10 minutes in four merge-group legs, despite a warm full-conformance median near four minutes.

## Changes to the critical paths

### Keep npm parity complete and use bounded workers

Before this change, npm parity took 13.78 minutes median on PRs and 16.40 on merge groups. Native package compilation took 3.00 and 3.40 minutes, respectively. WASI compilation took 1.85 and 2.12 minutes. The parity test now uses bounded workers instead of serial comparisons.

The test enumerates the existing fixture, command, and three path-variant combinations before execution. At most four asynchronous workers run inside the existing job. Each owns an exclusive scratch directory. Native execution, capture, cleanup, WASI execution, and byte comparison remain ordered within each case. Bounded output capture and child-failure behavior remain. Workers retain counters and the earliest failure, not all cases' binary outputs. After active workers drain, the test reports the earliest failing enumeration index deterministically.

Every existing output, exit-code, stderr, successful-command, and temporary-file assertion remains. Every enumerated case must execute exactly once. `outputs > 50` and the complete successful-command set remain. Native launcher and installation lifecycle tests stay whole. One-worker mode remains available for verification. The worker change adds no runner jobs, artifact transfers, or package build variants.

### Split fixture command checks without dropping them

Before the split, `isobmff-fixtures` took 18.60 minutes median and 19.50 p90 on PRs. Its command-coverage, frame-list, and informational R1 steps now run in `isobmff-command-compat`, covered by `ci-report`. Their pre-change PR medians are 1.77, 2.02, and 3.43 minutes. All comparators, baselines, profiles, flags, and R1 verdict handling are unchanged.

Both jobs consume the same run-bound fixture artifact. On a full run, fixture preparation in `ci-changes` generates the set even on a cache hit before publishing it. Reused main maintenance keeps its existing cache behavior. The full-run generation obligation moves to that producer rather than disappearing. No consumer independently regenerates the shared set.

The base fixture job retains stream and packet comparison, dependent known-divergence checks, schemas, hostile inputs, policy checking, audio and text extraction, and the text known-divergence audit. The latter takes 3.77 minutes but stays in the base job to balance the two branches. The new job builds the same required command variants and retains its reports under a separate artifact name.

### Reuse the exact tested tree and base in the queue

The longest ten projected merge-group cohorts are limited by Linux `check` four times, decode fixtures twice, and macOS checks, npm smoke, deny and bench-guard once each. These are the projected last prerequisites plus retained report admission, not proof of a single cause. Full H.264 and transcode moves alone cannot remove this tail.

The required workflows record the actual ready-PR checkout tree, first-parent base and attempt through the existing `merge-group-result` action. A merge group reuses only the newest successful same-workflow PR run at its current head when both recorded identities match. This preserves changed-path selection and the head-versus-base instruction comparison. A new base, missing receipt, newer failure or pending run, rerun without a fresh attempt receipt, fork, Dependabot run, incomplete index or API failure runs the normal suite. Main cache maintenance and fixture generation do not start on queue hits; main's baseline guard and all independent full suites retain their existing obligations. Advisory checks remain fresh on a hit for the root and every discovered fuzz manifest; an explicit outcome witness refuses a skipped scan. A failed-job-only rerun without a new changes-job receipt safely misses rather than borrowing the earlier attempt's receipt.

The non-required H.264 encoder workflow no longer repeats in the queue. It retains PR, main and manual validation. The workflow checker enforces the absence of merge-group triggers on non-required workflows, including quoted and shorthand triggers.

## Implemented job tiers

These are this branch's job tiers. Durations are pre-change successful PR job execution median/p90 in minutes unless marked otherwise. They exclude runner queues. Matrix rows group related legs without adding their parallel durations. The required names remain in [the required-check table](ci-checks.md#required-status-checks-exact-names).

| Jobs | Measured execution | Tier |
|---|---|---|
| `ci-changes` | 0.32 / 0.42 | PR and merge group, with fresh shared fixtures on full runs |
| `check` | 14.85 / 16.82 | PR and merge group, unchanged configurations |
| `check-macos`, `check-windows` | 11.13 / 13.08; 10.57 / 11.37 | PR and merge group with the existing platform-path filters |
| `msrv` | 0.92 / 1.00 | PR and merge group |
| `fuzz`, two shards | 6.20–8.33 medians | PR compilation and existing short affected-target runs; no queue campaign |
| `isobmff-fixtures` | 18.60 / 19.50 before split | PR and merge group, base fixture checks |
| `isobmff-command-compat` | modelled near 11–12, not measured | Required PR and merge group, unchanged moved checks |
| `isobmff-writers` | 10.35 / 11.30 | PR and merge group |
| `isobmff-decode` | 13.38 / 15.21 | PR and merge group, including complete AAC-enabled transcode fixture targets |
| `mkv-fixtures`, `ts-fixtures` | 8.18 / 8.69; 7.55 / 8.09 | PR and merge group |
| `conformance` | 3.87 / 4.10 on PRs; merge-group legs reached 37.93 | PR and merge-group smoke; full H.264 moves after merge, daily and dispatch |
| `vector-width`, `modes-identity`, `kernels-arm64` | 0.83–1.03 medians | PR and merge group, including all current Clippy invocations |
| `training-identity` | 6.00 / 6.99 | PR and merge group |
| `bench-guard` | 14.32 / 21.05 | Required PR and merge-group base comparison, unchanged |
| `svt-av1` | 6.99 / 11.73 | PR and merge group, retaining its existing tests and licence coverage |
| `transcode-compat-shard`, three legs; `transcode-compat` | 8.18–9.18 shard medians; 0.27 merger | After merge, daily and final-head dispatch, outside `ci-report` |
| `conformance-arm64` | formerly main-only | Full-suite workflow after merge, daily and dispatch |
| `main-cache` | main-only today | Existing reused-main cache maintenance |
| `ci-report` | 0.05 / 0.07 | Required PR and merge-group report |
| `deny-checks`, `deny` | 13.88 / 14.83 work; report seconds | Unfiltered required checks, unchanged |
| `python-changes`, `python-fixtures`, `linux-x86_64`, `python-report` | fixture median 0.60; wheel median 6.37 | Existing required Python path and same-tree reuse |
| `npm-changes`, `package`, `smoke`, `npm-report` | package 20.38 / 24.90 | Existing required npm path, complete parity with bounded workers |
| Label-selected Python and npm platform builds | opt-in today | Existing opt-in, tag, scheduled and manual behavior |
| `pr-text` | short, edit-triggered | Existing PR text gate, not a merge-group ruleset context |
| HEVC workflow | headers 0.46–0.60; vectors 5.50 / 12.73; census 19.82 / 27.48 before the full inter census | Unchanged; tiering deferred while coreav-dev/coreav#429 remains open and draft |
| VP9 `vectors`, `adapter-contract` | vectors 11.80–14.01 medians; contract 0.92 | Existing PR comparisons, including the complete 353-vector set |
| VP9 `interpolation-fuzz` | about 11.1 per platform | Compile and run 30 seconds on PRs; retain the 600-second pass after merge, daily and dispatch |
| VP9 `contract`, `corpus`, `corpus-summary` | slow shard 19.60 / 24.51 | Contract and exact-eight original-file smoke on PRs; full corpus on relevant main pushes, daily and dispatch |
| Audio `raw-pcm`, `flac`, `mp3`, `mp3-transforms`, `opus`, `aac` | 0.20–14.85 medians; AAC p90 17.12 | Existing PR checks, retaining their complete lists and qualification controls |
| `h264-encoder-conformance` | 2.18 / 2.47 | Existing fast encoder checks, including strict Clippy |

Other workflows retain their existing scope and events. In particular, publication and release procedures do not change. The frozen legacy compatibility workflow and judging registration stay byte-identical. A separately registered current-head R3 regression is a follow-up, not part of this implementation.

### Smoke selection does not change registered lists

H.264 smoke selects exact names from `crates/coreav-h264/tests/data/conformance-streams.json`: `BA1_Sony_D`, `CABACI3_Sony_B`, `cavlc_mot_frm0_full_B`, `HCHP1_HHI_B`, `HPCV_BRCM_A`, `alphaconformanceG`, `CVPCMNL1_SVA_C`, and `MR1_BT_A`. Acquisition and both fast and safe decoding use the same selection, existing thread counts, and comparisons. Exactly those eight distinct in-scope names and results are required. Unknown, absent, or extra results fail. Full checks still use all 99 admitted streams and unchanged hashes.

`hevc-conformance.yml` is unchanged. The proposed HEVC synthetic-smoke and full-census split is deferred, not shipped. Existing HEVC checks and owning-area full-check obligations remain in effect.

VP9 retains all 353 vectors in routine `vp9-conformance.yml` validation. Separately, `vp9-corpus.yml` runs `scripts/vp9_smoke.py` over exactly eight existing pinned public vectors as original files, using the corpus comparator and pinned oracle. Hash, membership, positive frame count, and complete exact coverage are required. Full corpus shards run only on relevant main pushes, daily, and dispatch. Interpolation fuzzing compiles and runs for 30 seconds on PRs and 600 seconds in the full tier. Audio retains its complete existing coverage and qualification controls; no reduction is approved. No roster, baseline, tolerance or frozen judging file changes.

The original-file smoke uses inputs whose container metadata satisfies the unchanged C2 admission checks. The intra-only vector remains in the complete 353-vector suite, but its container probe exposes unknown bit depth and subsampling, so it receives no original-file pixel credit. The smoke selects the existing `vp90-2-07-frame_parallel-1.webm` instead; it never skips an admission refusal or substitutes metadata from the oracle. Failure reports name the stage and a closed reason without exposing arbitrary exception arguments.

## Full checks and final-head evidence

`ci-full.yml` owns all 99 admitted H.264 streams in fast and safe modes at thread counts 1, 2, and 8, arm64 conformance, and the unchanged three transcode shards and merger. Its fresh producer prepares the registered fixture sets once and keeps exact shard completeness, byte binding, and merger checks. Full work runs after relevant main pushes, daily without path filtering, and by manual dispatch, independently of required-workflow same-tree reuse.

`ci-full-failure.yml` observes completed `ci-full`, `vp9-corpus`, and `vp9-conformance` runs. It executes main's reporting code, not code from the observed branch. It accepts only this repository's main-push and scheduled runs and reads complete observed-attempt job and proof-step metadata, never logs or artifacts. Its isolated token needs `actions: read` and `issues: write`, with no new secret. PR, merge-group, dispatch, and fork results cannot create issues.

The reporter uses fixed workflow, job, and result labels and public run links. It creates or updates bot-owned area issues, deduplicates attempts, and reconciles simultaneous creation without cancelling another report. Complete pagination and successful non-skipped proof steps are required. API failure or incomplete evidence fails reporting rather than producing a false success. The owning area fixes or reverts within one day. There is no automatic revert or regression-issue closure; only duplicate issues are reconciled.

For a correctness PR, require successful manual evidence at its exact final head before enqueueing. The current verifier requires **all three suites**, not an area-selected subset. Every registered matrix leg and proof step must succeed without skipping. It uses the newest dispatch and latest attempt, with complete pagination, and rechecks that the open PR head is unchanged. A later failed or pending dispatch or a new commit invalidates the evidence. A green `ci-report` is not full-suite evidence.

The operational procedure is in [Verify a correctness PR's final head](ci-checks.md#verify-a-correctness-prs-final-head).

`hevc-conformance.yml` remains unchanged while coreav-dev/coreav#429 is open and draft. HEVC automatic latency is unchanged. Its tiering is a deferred follow-up, not part of this implementation.

## Forecast and its limits

Replay each of the 32 PR and 50 merge-group successful source cohorts using its own job and step timestamps. Keep observed queues, unchanged jobs and report admission delays. Do not add median stage times together or assume that moving work eliminates hosted-runner queues.

The model adds 0.4 minutes for fresh common fixture production. The new fixture job receives its moved measured steps plus four minutes for duplicate setup, compilation, transfer and admission. Npm native tests receive a conservative 0.5-minute allowance plus 0.1 minutes of worker overhead. The pool's effective speedup is an assumption, not a measured result. H.264 smoke is assumed to take at most five execution minutes; that assumption must be checked on Actions. No queue improvement is credited to the heavy-tier moves.

| Source cohort scenario | PR median / p90 minutes | Merge-group median / p90 minutes |
|---|---|---|
| Observed baseline | 21.27 / 25.78 | 25.04 / 41.91 |
| Forecast with assumed effective 1.5× npm parity | 18.63 / 23.40 | 20.47 / 41.83 |
| Forecast with assumed effective 2× npm parity | 17.17 / 23.40 | 19.06 / 39.81 |

These forecasts describe routine required validation on the matched source cohorts, not every heavy-triggered PR, an unconditional service guarantee, or the final-head manual exception. Heavy-path measurements are reported separately above. Moving full suites prevents their automatic execution on every PR revision, but a numerical forecast for their full final-head manual duration is not justified.

The queue-reuse extension has separate conditional projections on the same 50 successful source cohorts. It retains each gate's observed queue and each report's admission/execution lag, removes only the work skipped by proven reuse, retains deny's container preparation, expression validation and cleanup, and adds 1.1 minutes per workflow for the bounded metadata lookup. Fresh advisory work uses historical root full-deny, binary installation and fuzz-licence durations as a proxy, plus one extra minute for network work; this is an unmeasured allowance, not a runtime bound. Gates run in parallel. No queue relief from fewer jobs or the encoder-trigger removal is credited.

| Conditional queue scenario | Projected median / p90 minutes |
|---|---|
| All four workflows prove the tested tree and base | 3.31 / 10.51 |
| All miss, assumed effective 1.5× npm parity | 21.57 / 42.93 |
| All miss, assumed effective 2× npm parity | 20.16 / 40.91 |

These are counterfactuals, not observed latencies or an overall projection. Seven of 13 cache-identifiable groups had matching prior tested trees and bases, but 37 of 50 lack a prior PR association, including all ten slowest projected cohorts. Branch association and incomplete run coverage add selection uncertainty. This cannot establish a reuse probability, especially in the tail. Mixed hit/miss workflows lie between the conditional paths for each cohort; their aggregate median and p90 cannot be inferred by averaging these quantiles. Measure actual receipt hits, fallbacks, failures, unfinished runs and queue latency after deployment before claiming an overall sub-30-minute p90.

## Atomic implementation and verification

This branch updates the required-check table, workflow dependencies and triggers, `workflow_check.py` tier and merge-group rules, scheduling tests, and affected agent rules together. The first commit remains the historical plan-only commit. Enforcement claims below refer to implemented tests, not production timing evidence.

Preserve the prescribed filtered-workflow per-job groups, matrix suffixes, PR-only cancellation, independent main/merge-group run groups, report behavior and deny's existing whole-run exception. New required jobs use the same forms. A main push may reuse only a successful same-tree result of the same workflow. That reuse covers the reduced gate, never the full tier. Required context names stay `ci-report`, `deny`, `python-report` and `npm-report`.

Deny, licence checks, DCO, pr-text, workflow checking, unsafe-site checking, and strict Clippy configurations remain. Bench-guard remains a head-versus-base comparison in PRs and merge groups and a baseline comparison on main. The reductions are full H.264 and composed-transcode work on each PR and queue run, VP9's per-PR fuzz duration, and VP9 corpus work on each PR revision. Their original full predicates remain in the full tier. Legacy R3 stays frozen and unchanged. No new runners are introduced.

Verification includes complete workflow-checker self-tests, main-plan scheduling and matrix tests, changed-path tests, exact smoke membership and result-completeness tests, shared-fixture binding, reporter trust/idempotence tests, and final-head evidence tests. Test the worker executor on synthetic children for exact case coverage, isolated outputs, deterministic failures, bounded output, launch errors and cleanup. On Actions, compare one-worker and parallel npm runs over identical generated bytes and require equal complete case coverage. Run the resulting ordinary PR gate and observe actual queue outcomes. Keep all heavy verification on Actions.
