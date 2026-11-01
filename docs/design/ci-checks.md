# CI checks: unique names, job-level path filters, report jobs

How the workflows under `.github/workflows` are shaped so that a branch
ruleset can require them. The exact list of names it requires is the table
under "Required status checks" below.

## The problem

Branch protection matches a required status check by **name**, and the name
of a job that sets no `name:` is its job id. Two things followed from that
and from the path filters an earlier change put on the heavy workflows:

1. **A workflow a trigger filter skips reports nothing at all.** Not a
   skipped check, no check: GitHub leaves the pull request waiting for a
   required check that will never arrive. (A job skipped by its own `if:`
   is different: that reports as a success.) So before this change exactly
   one check could be required, `deny`, the only workflow without a filter.
2. **Job ids repeated across workflows cannot be required**, because the
   name is ambiguous. `fixtures` was a job in four workflows, `guard` in
   two, `build` in three and `fuzz` in two.

## What the workflows do now

- **Every job id is unique across all workflows.** `fixtures` became
  `isobmff-fixtures`, `mkv-fixtures`, `ts-fixtures` and `python-fixtures`;
  `guard` became `bench-guard`; every job of the three
  H.264 spike workflows carries its spike's prefix (`h264-n1-build`,
  `h264-n2-build`, `h264-n2-arm-build` and so on).
- **A workflow that is required filters in its jobs, not in its trigger.**
  Its first job decides from the event's changed files whether the rest
  has to run; every other job carries `needs:` on it and an `if:` on its
  output, next to whatever condition it had. In `ci.yml` that job is
  `ci-changes`, and it gives one output per group of jobs (`run`,
  `h264-conformance`, `bench-guard`; "One changes job" below);
  `python.yml` and `npm.yml` keep their own `<workflow>-changes`.
- **Each required workflow ends in a report job** (`ci-report`,
  `python-report`, `npm-report`), which carries `if: always()`, waits for
  the changes job and every other job, prints their results and fails when
  one failed or was cancelled. A job the filter skipped reports `skipped`
  and passes. That name is the required check: it appears on every pull
  request, including one that changes documentation only. Every job of a
  workflow with a required report job is waited for by one of them.
- Workflows that are **not** required keep their trigger filters: the
  informational `mediainfo-oracle`, the spikes (`spike-build`, `h264-n1`,
  `h264-n2`, `h264-n2-arm-speed`, which build spike code and are kept
  with it out of the public tree), the nightly `pr-redirect`, `labels`,
  `pr-text`, `docs-site`, and `release`, whose pull-request run is the dry
  run of a change to the release build (`docs/design/release.md`).

`scripts/workflow_check.py` runs in `deny.yml` — the workflow nothing
filters — and fails when the required-check table and the workflows
disagree: a listed name no job reports, a report job missing from the
table, a required workflow that grew a trigger filter again or lost its
`pull_request:` trigger, a report job without `if: always()`, a job of a
required workflow that no report job waits for, a job without its own
concurrency group in a workflow that cancels job by job ("Concurrency"
below), two jobs that would report the same check name, a filter
pattern the action would read differently from GitHub, a filter written
in a form the checker cannot read, a workflow that runs the oracle
FFmpeg's installer whose trigger filter a change of the pin
(`scripts/oracle-ffmpeg.env`, `scripts/install-oracle-ffmpeg.sh`) does not
pass, or a step (in a `.yml` or `.yaml` workflow,
in a local action, or in any local action or workflow a step calls with
`./...`) that uses an action by a tag or branch rather than a full
lowercase commit sha: a tag can be moved to other code, so every step names
the commit and keeps the version as a comment. The pin check parses each
file with Ruby's standard YAML library (Psych, on the runners) and walks
the node tree, not the objects a loader would build: every mapping key
written `uses` is checked wherever it is, its value read through an alias
if it is one, so YAML 1.1's boolean keys (`on`, `yes`), merge keys and
duplicate keys hide no step. A file that is not one readable YAML
document, an alias that names no anchor and a `uses` value that is not a
string fail.
A workflow that carries the line `# workflow-check: not required` is
outside the table: its report job need not be listed, and the table may
not list it.

## What a report job must not hide

GitHub judges a required check by the **last** check run of that name on
the commit. So a later event that reports green over an earlier failure of
the same commit opens the gate, and the gate is only as good as the rule
that decides when a job may be skipped.

A job the filter skipped is benign: the event changed nothing that job
reads, so it has nothing to say. A job skipped for a reason that has
nothing to do with the change is not. That is why `python.yml` and
`npm.yml` no longer skip their work on a `labeled` event whose label is
not `wheels` or `binaries`: with that condition, adding any unrelated
label to a pull request whose wheel build had just failed started a run in
which every job was skipped and `python-report` went green on the same
commit, and nothing ever fired again to correct it. A label event now does
the same work as a push; the two labels still select the extra wheels and
binaries, read from the pull request's label set rather than from the
event, so they work on every event. Labels are rare, and the path filter
still decides whether anything runs at all.

Two kinds of skip remain, and neither may reach a required check.

**The draft condition** is safe by construction: a draft cannot be merged,
and `ready_for_review` is in every trigger's list, so marking it ready
runs the jobs again. The trap in it: GitHub reports a job skipped by its `if:` as a
successful required check, so if a push lands while the pull request is still a
draft and it is marked ready without another push, those checks can show green
without having run. Toggle the pull request back to draft and ready again, or push
once more, and confirm the runs exist before merging.

**The label-gated jobs** are not safe, and are therefore **not covered by
a report job**. `binaries` and `smoke-platforms` in `npm.yml`, and
`linux-arm64` and `macos-arm64` in `python.yml`, run only when the pull
request carries `binaries` or `wheels`. That condition belongs to the
pull request, not to the commit: remove the label after one of them has
failed, and the next event of the same commit — another label, a reopen,
a draft toggle — skips it, and a report that covered it would go green
over the failure, with no event left to correct it. So `npm-report` waits
for `package` and `smoke` only, and `python-report` for
`python-fixtures` and `linux-x86_64` only. The platform builds still
report under their own names, which is what the merge button and a
release read; they are opt-in builds, not a gate on every pull request.
`workflow_check.py` enforces both halves: every job that is not
label-gated must be covered by a report job of its workflow, and no report
job may cover one that is.

## The filter

`.github/actions/changed-paths` is the shared decision. It takes one
`paths` or `paths-ignore` list, in GitHub's own filter-pattern syntax and
with GitHub's own semantics (`paths`: run when any changed file matches;
`paths-ignore`: run unless every changed file matches), so moving a list
from the trigger into the action does not change which events run.

The changed files come from git, which is why each changes job checks out
with `fetch-depth: 2`:

- a pull request: the merge commit against its first parent, the base —
  the diff `ci-changes` also uses to choose the fuzz targets;
- a push: the event's `before` commit against the event's commit;
- a manual or scheduled run, an unreachable commit, or a failing diff:
  everything runs, the safe direction.

Patterns are translated to regular expressions: `*` stops at a slash and
`**` does not, and that is all. GitHub's filter patterns are not ordinary
globs — in them `?` and `+` quantify the **preceding** character (`*.jsx?`
matches `.js` and `.jsx`), `\` escapes and `[]` is a character class — so
every one of those is **refused** rather than guessed at, and no pattern
can be read one way by GitHub and another by the action. None is in use. The unit tests are in
`.github/actions/changed-paths/tests/`.

Filter lists were **merged**: `bench-guard` carried a
longer list on `push` than on `pull_request`. One list per workflow is the
shape the action supports, so each took the union of the two. The effect is
that a pull request changing the fixture generator, the pinned FFmpeg
installer or the web-upload policy now measures as well — rare, and the
safe direction.

## The platform jobs

`check-macos` (Apple Silicon) and `check-windows` (x86-64, MSVC) run the tests and `clippy` configurations of `check` that build there. They are the slowest jobs of `ci.yml` and contend for the smallest runner pools, so a pull request runs them only when it changes code that differs between the platforms. `ci-changes` decides it with a second `paths` list beside its `paths-ignore` list, and a push to main or a manual run always runs them.

- **What is on the list.** The sandbox and the processes it starts (`coreav-worker`, `coreav-transcode`, `coreav-cli`, whose launcher entry, external programs and file outputs have Unix and non-Unix halves), the dav1d and libvpx backends (the macOS job links them against Homebrew's libraries), positioned file reads (`coreav-isobmff/src/source.rs`), the crates with code for one target architecture (`coreav-h264`, whose default `fast` feature has x86-64 paths and whose decode example has a Unix half, `coreav-kernels-h264`, with x86-64 and AArch64 kernels, and `coreav-kernels-audio`, with x86-64 kernels), every `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml`, `ci.yml` and the filter action. A file outside it that grows a `cfg(unix)`, `cfg(windows)`, `target_os` or `target_arch` condition belongs on it.
- **Covered by `ci-report`.** A job the filter skipped has nothing to say, so the report may cover it (section "What a report job must not hide"). The condition reads only the changed files, which belong to the commit.
- **No label to force them.** A label such as `ci:all-platforms` would make the condition read the pull request's labels, which is the case that section rules out and `workflow_check.py` refuses for a covered job. To run them on a pull request outside the list, start `ci.yml` by hand on its branch: a manual run is never filtered.
- **Every step of `check-windows` runs**, even after one fails (`if: ${{ !cancelled() }}`), so one run lists every failure there; the job still fails.

## One changes job

`ci.yml` holds the jobs of what used to be six workflows: itself,
`fixtures.yml`, `mkv-fixtures.yml`, `ts-fixtures.yml`,
`h264-conformance.yml` and `bench-guard.yml`. Each had its own changes
job, so every pull request push queued six one-minute jobs that each
checked out the repository and diffed the same two commits. Now
`ci-changes` runs the changed-paths action once per filter list, in one
job, and the moved jobs read its outputs: `run` for the three fixture
oracles, whose list was the same as `ci.yml`'s, and `h264-conformance`
and `bench-guard` for the two that had their own. The fuzz job's list of
targets is its `fuzz-plan` output, which a job of its own,
`fuzz-changes`, used to compute. The job ids did not
change, so neither did a required check name: the ruleset matches a check
by its job's name, not by its workflow.

- **One report job for the whole workflow.** `ci-report` waits for every
  job of `ci.yml`; five reports, one per group of jobs, cost five more
  jobs on every push and five more required names. The price is that a
  failing H.264 conformance run shows as a red `ci-report`, not under its
  own name; the job's own check, and the table in `ci-report`'s summary,
  still name it.
- **A list names `ci.yml` where it named its old workflow file,** so a
  change to `ci.yml` runs the H.264 conformance suite and the guard. That
  is the safe direction: a change to those jobs runs them.
- **A manual run of `ci.yml` runs everything,** the H.264 conformance suite
  included, which had no manual trigger before; `update` keeps the
  guard's counts as an artifact, as `bench-guard.yml`'s input did.
- **Not moved:** `deny.yml` is one job, so moving it saves nothing, and
  folding its steps into `ci-changes` would drop the required `deny` name
  and hold every heavy job back by its run time. `python.yml` and
  `npm.yml` also run on `labeled`, so in `ci.yml` a label would rerun, and
  cancel, every job of the pull request. `pr-text.yml` runs on `edited`, for
  the same reason. The workflows that are not required keep their trigger
  filters, so they queue nothing on a pull request that does not touch
  them; moved here, they would add a skipped check to every one.
- **Rejected: calling the old workflows from `ci.yml`** as reusable
  workflows. A called workflow's job reports as `<caller job> / <job>`,
  which renames every required check it holds.

## The FFmpeg pin

Current comparisons use FFmpeg 9.0.2, the build in `scripts/oracle-ffmpeg.env`. The installer verifies its archive hash, and `scripts/oracle_pin.py --verify-bin DIR` rejects another FFmpeg or ffprobe version, including on a binary-cache hit.

Every oracle audio decode reads the input options in `scripts/oracle-audio-args.txt`, through `oracle_audio.INPUT_ARGS` in Python, directly in shell, and through `include_str!` in Rust tests. The options go before the input, after any general decoder-thread option. They select one audio decoding thread without changing video decoding or output encoding threads. FFmpeg 9.0.2's frame-threaded FLAC and ALAC decoders apply a start skip longer than the first frame several frames late. In the reproduced FLAC clip starting at 0.6 s, the first 4,096 samples are 128 samples early and 128 samples are dropped after them. One decoding thread gives the source's samples; FFmpeg 7.0.2 gives them on any thread count. This is an oracle execution constraint, not an acceptance-rule or threshold change. The clip, split and join checks share the clip audio reader. Audio extraction hashes decoded PCM frames, not copied packets, and its frame reader uses the shared input options too. The HLS, audio-decode and AAC comparisons, the audio-capable Matroska hash helper, and the mixed audio/video shell frame oracle use the same options. Remux and sanitize explicitly select `-c copy` for packet hashes; transcode compares copied audio packets and decodes only video. The compat and corpus runners reuse these checkers. `scripts/oracle_pin.py --check` checks audio command ownership and names the shared arguments when a new path bypasses them. It recognizes explicit copy codecs rather than allowing packet-copy exemptions. Its tests exercise the commands and reject missing input options, and the real FLAC/ALAC edit-list regression checks extraction as well as the clip reader.

The sole historical-extraction exception is `training-identity`: `docs/measurements/h264-training/es.json` records bytes extracted by FFmpeg 7.0.2. Extracting one recorded SD stream twice with that version reproduced its hash and 10,750,164 bytes; 9.0.2 consistently produced 10,750,126 bytes. In that reproduction, the per-type NAL payload sequences were identical, but 9.0.2 placed SPS/PPS before SEI and changed Annex-B framing. The historical extractor therefore retains its own archive SHA-256 and version check in `docs/measurements/h264-training/tools/install-extractor.sh`. It installs only FFmpeg under `RUNNER_TEMP/coreav-training-extractor`, never on PATH or in an oracle directory, and only the historical extraction helper reads it. The recorded hashes and exact-byte check remain unchanged; no comparison uses this extractor as its oracle. The pin check rejects references to this installer or directory outside the training job and its measurement directory.

## The fixture jobs

`isobmff-fixtures` ran every check of the ISO base media fixtures in one job: a median of 34.9 minutes over the successful jobs of the last 60 completed pull request runs, against 15.2 for `fuzz`, 11.6 for `bench-guard` and under 9 for every other job, so it alone set how long a pull request waited. Its steps now run in six jobs, with the same commands, bars and thresholds:

- `isobmff-fixtures`: fixture generation, the oracle self-checks, the stream and packet oracles against `ffprobe` and their known-divergence audit, `coreav probe` and `coreav ffprobe`, the output schemas, the hostile suite through the `coreav` binary, `coreav check`, audio extraction, subtitles and chapters, and the text known-divergence audit. It generates its own fixtures on every run, as before, so every run exercises generation. Its artifact is `oracle-output`.
- `isobmff-fixture-set`: the one set of fixtures that the three jobs below check, an artifact of the run. Generation is not byte for byte reproducible (a WebM fixture generated twice with the same FFmpeg differs), so jobs, and B9 shards, that generated their own would check different bytes, and `--merge` refuses shards whose fixtures differ.
- `isobmff-writers`: clip, remux, video replacement, split, sanitize, HLS, join and the 0.1.0 `ffmpeg` commands. Their reports are the `writer-output` artifact.
- `isobmff-decode`: the build with the FFmpeg fallback, dav1d and libvpx, then decoding, thumbnails and `coreav transcode` against the pinned FFmpeg, and the hostile suite through `coreav thumbnail` and `coreav transcode`.
- `b9-shard`, three legs, and `b9`: bar B9 of `docs/design/transcode.md`.

The generated-fixture and oracle-binary cache keys include `scripts/oracle-ffmpeg.env`, so changing the oracle invalidates both. Each B9 shard runs the pinned FFmpeg and ffprobe. After merging their evidence, `b9` checks its recorded version and each shard's FFmpeg and ffprobe hashes against the installed pin; it does not rerun the encodes. The command-coverage jobs use `tools/compat-corpus/ffmpeg9.json`, including its ratio-based bit-rate rules, unchanged by the split.

**B9 in three shards, judged once.** B9 was 13.2 minutes of the old job (median): 12 command lines on 83 fixtures, 998 results and 3,078 seconds of encoding over four parallel jobs, 45% of it line x07, which scales to 1920 columns. `scripts/transcode_check.py --shard K/3` runs every third fixture and judges no bar: it keeps its results, calls and manifest as evidence. It fails when a result is missing, and, as a whole run does, when its preflight or setup fails; a failed bar never fails it. `b9` puts the three together with `--merge`, which refuses a missing or repeated shard, and judges every bar over all of them as the single run did. Its `b9-evidence` artifact holds what `oracle-output` held as `coreav-b9/evidence`. Split three ways, the evidence of a single run's artifact merges to the same summary, run record, calls and manifest, and to the same exit status; its shards carry 998, 1,045 and 1,035 seconds of encoding.

## The fuzz runs

Fuzzing runs in two places, and a push to main is neither.

- **A pull request** fuzzes the targets it can affect. `ci-changes` lists every target of each fuzz project whose own directory, or a workspace crate it depends on, the change touches (`scripts/fuzz_plan.py --changed`); `fuzz` runs only when that list is not empty, so a pull request that reaches no fuzz project starts no fuzz job and `ci-report` still covers it. The targets share 480 seconds, 5 to 30 each: a change to one container parser (7 to 24 targets) gets 20 to 30 seconds per target, and a change that reaches every project (the lock file, `ci.yml`, `scripts/fuzz_plan.py`) gets a pass of 6 seconds over each of the 70 targets instead of the 20 each that took about 38 minutes.
- **The pull request's fuzz job runs in two shards** of whole projects, side by side: `scripts/fuzz_run.sh shard` gives each project, in the plan's order, to the shard with fewer targets so far, so each project builds in one shard only, and every target runs for the seconds the whole plan gives it, as in one job. In one job, a change that reaches every project took a median of 15.2 minutes (build 5.7, run 6.3), past the 15 the rest of the pull request's checks take.
- **`fuzz-nightly.yml`**, once a day and by hand, runs every target (`scripts/fuzz_plan.py --all`) for 15 minutes, in shards of six that run side by side, six at a time. It is not a required check: it never runs on a pull request.

Main stopped fuzzing on push because the run had outgrown its job. Every target for 60 seconds was 66 minutes of fuzzing in a job with a 50-minute timeout, so every push to main ended cancelled, `ci-report` red, after about 40 targets, and the last 26 were never fuzzed on main. A timed-out job also skips its cache step, so no fuzz build was ever cached. The daily run gives every target 15 minutes a day; a day of pushes gave about 40 of them 60 seconds per push.

**One build, cached.** `scripts/fuzz_run.sh` builds every project into one target directory, `target/fuzz`, with a pinned nightly toolchain (the pin is in the script). The daily run's first job builds every project, removes what is rebuilt from the checkout anyway (CoreAV's own crates and the fuzz binaries) and caches the rest; its shards, and every pull request's fuzz job, restore that cache, keyed by the script and the lock files. The pin keeps the cache valid from one day to the next; an unpinned nightly would change the compiler, and the cache with it, every day. `rust-cache` is not used here: it cleans a target directory once per workspace and keeps only that workspace's dependencies, and the 21 fuzz projects share one directory.

**Nothing a fuzzer prints reaches the log.** Each target's output goes to a file. A target that ends in a finding leaves that output and its libFuzzer artifacts aside, and the log names the target only (libFuzzer prints a failing input as bytes and as base64, and `cargo fuzz` prints its `Debug` form). The daily run uploads them to the private bucket and logs the object key; a pull request's run keeps nothing, and the author finds the input again locally with the same command (docs/design/fuzz-campaign.md, "Findings").

## Concurrency

A new push to a pull request supersedes the run of the previous one.
`ci.yml` cancels that run job by job: every job but `ci-report` carries

```yaml
concurrency:
  group: ${{ github.workflow }}-<job id>-${{ github.event_name == 'pull_request' && github.ref || github.run_id }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

When the new run's job joins its group, GitHub cancels the old run's job
of the same id. A push to main and a manual run get a group of their own
run, so nothing on main is cancelled or dropped.

- **Not one group for the workflow.** That was the earlier shape. A
  cancelled run is over only when every job has ended, and `always()`
  starts `ci-report` in a cancelled run too, so the new run waited, with
  no job created, until the old report had found a runner and ended. On
  2026-10-30, 163 of 680 runs in four hours waited more than two minutes
  for their first job, a median of 6.5 minutes and up to 25. With the
  groups on the jobs the old report still runs, on the superseded commit,
  but nothing waits for it.
- **Not `if: ${{ !cancelled() }}` on the report.** It frees the next run as
  well, and saves the old report's runner. But a run cancelled by hand on
  a pull request's head would then report `ci-report` as skipped, which
  the ruleset counts as passing. With `always()` that run's `ci-report`
  fails, as before.
- **`ci-changes` has a group too.** GitHub does not order the jobs of one
  group by run: whichever joins last cancels the other. Every other job
  waits for `ci-changes`, so an old run's jobs join their groups when its
  `ci-changes` ends. If that is before the new run starts, they join before
  the new run's jobs; if not, the new run's `ci-changes` cancels the old
  one and the old run's jobs never start. Either way an old job never
  cancels a new one.
- **The cost.** An old run's job now runs until the new run's job of the
  same id joins its group, one changes job after the push (a median of
  5.5 minutes), and a job the new run skips is not cancelled at all. Over
  the same four hours that would have been about 106 more runner-minutes,
  against 1,254 minutes that pull requests waited on a held run.
- **A job with a `strategy:`** (`fuzz`, `b9-shard`) puts `-${{ strategy.job-index }}` after its id in the group: its legs would otherwise share one group and cancel each other. `workflow_check.py` takes only that form for such a job.
- **Re-running an old run's job** cancels the head's job of the same id,
  whose `ci-report` then fails: re-run the head's run instead.
- `github.job` is not available where GitHub reads a job's `concurrency`,
  so each group names its job id. `workflow_check.py` fails on a job
  without its group, a group other than exactly this one, a report job
  with a group (in another job's group it could cancel that job), and a
  workflow group beside job groups. It names `ci.yml` as a workflow that
  cancels job by job, so removing every group fails too. `python.yml` and
  `npm.yml` keep one group for the workflow.

## Caches

The repository has 10 GB of Actions cache; beyond it GitHub evicts the
least recently used entries. `Swatinem/rust-cache` saves on main only
(`save-if`), so a pull request restores what main last built. Its default
key is one entry per job id, toolchain and `Cargo.lock`.

- **The other ISO base media jobs restore it too** (`isobmff-writers`,
  `isobmff-decode`, `b9-shard`). `isobmff-decode` and `b9-shard` use the build
  with the FFmpeg fallback, dav1d and libvpx (`target/fallback`), which
  `isobmff-fixtures` runs on main only, after its checks, so that its entry
  keeps holding it: one entry for the four jobs instead of a new one for
  `target/fallback`, with the repository already at its 10 GB.
- **`isobmff-fixture-set` keeps its fixtures between runs**, saved on main
  only, under a key of everything that generates them:
  `scripts/gen-fixtures.sh`, every file of `scripts/` it runs (found in the
  script itself, so a new helper joins the key without an edit here) and
  `scripts/install-oracle-ffmpeg.sh`. A pull request that changes one of
  them generates the fixtures on every push.
- **The Matroska and transport stream oracles restore `isobmff-fixtures`'
  cache** (`shared-key: isobmff-fixtures`, `save-if: false`). That job
  builds every crate, profile and feature the other two build, so their
  own entries, 164 MB each per `Cargo.lock`, repeated part of its 257 MB.
  The key is the one `isobmff-fixtures` already writes, so no run starts
  cold.
- **`check` keeps its own entry.** Clippy's metadata-only builds are not
  what the fixture jobs build, and the other way round.
- **bench-guard's decoder build is under `target/`** (`target/decoder`), so
  rust-cache keeps its dependencies; in `$RUNNER_TEMP` it rebuilt every
  dependency on every run. The base commit's two builds stay in
  `$RUNNER_TEMP`, seeded from `target/` and `target/decoder`: main never
  builds them, so no cache would hold them, and a target directory shared
  with the head would let cargo take the base's build for the head's.
- **bench-guard saves on failure too** (`cache-on-failure`). On main the
  job fails when a count is over the tolerance, after its builds, and
  without it main's only bench-guard entry was two `Cargo.lock` versions
  old. Its key carries `decoder`: an entry saved before the decoder build
  moved holds no `target/decoder`, and rust-cache saves nothing over an
  exact hit, so an old key could have kept it out for as long as the
  `Cargo.lock` did not change.

## DCO sign-off

Every commit a pull request adds carries a Developer Certificate of Origin
sign-off by its author. `scripts/dco_check.py` is that check; it runs in
`deny.yml`, which nothing filters, and its tests in
`scripts/dco_check_selftest.py` run in the same job. A commit passes when
one of its `Signed-off-by` trailers carries the author's email, compared
without regard to case; a merge commit is judged like any other. On a
failure it names the commit and how to fix it:
`git commit --amend -s --no-edit` for the last commit. For earlier
commits, `git rebase --signoff <base>` signs every commit of the branch,
but it rewrites history, so the branch is pushed with
`--force-with-lease`, and it drops merge commits. On a branch that uses
merge commits, `git rebase --signoff --rebase-merges <base>` keeps them,
but the merges it recreates carry no sign-off, so an unsigned merge is
redone with `git merge --signoff`.

- **The email, not the name and email.** GitHub writes a squash merge with
  the account's display name as its author and signs it with the account's
  login, so on `main` the two names differ while the email is the same.
  The email is what identifies the person; the name is free text.
- **Commits already on `main` are skipped.** The check reads
  `BASE..HEAD` minus everything reachable from `origin/main`. A stacked
  pull request's base is another branch, and once that branch merges
  `main` in, `BASE..HEAD` holds `main`'s own commits, which the pull
  request did not write and cannot sign. The checkout fetches every branch
  (`fetch-depth: 0`), so `origin/main` is there; when it is not, the
  check fails rather than judging nothing.

## Default builds and patents

Default builds contain no implementation of a still-patented codec, and CI
checks it. `scripts/patent_free_check.py` is that
check. It runs in `deny.yml`, beside the licence checks, so no filter
skips it. It holds the list of crates that implement such a codec
(`coreav-h264`, `coreav-kernels-h264` and `coreav-backend-h264` today, all
H.264), runs `cargo tree --target all -e normal,build` for the default
features of every other workspace member, and for each build a package
script makes with other features (the npm command's
`--no-default-features --features cli`, and the release's default build,
`--features release-default`, which the native npm packages carry too),
and fails when one of those
graphs holds a listed crate. It also fails when a listed crate is no
workspace member, so a renamed crate cannot leave the list checking
nothing. Opting in stays a feature (`native-h264` of `coreav-cli`), which
the check leaves alone.

- **The list lives in the script, not in each crate's manifest**
  (`[package.metadata]`). A marker in the manifest would let a new codec
  crate declare itself, but a crate that forgets the marker passes either
  way, and the list is the patent review's result. It
  belongs with the check and changes in the pull request that adds such a
  crate.
- **`cargo tree`, not `cargo metadata`'s resolve.** The workspace resolve
  unifies every member's features, so it cannot say which member's default
  build pulls a crate in; one tree per member can. All 26 trees take
  about 3 seconds.
- **Normal and build edges over every target.** Dev-dependencies never
  reach a shipped artefact, and `--target all` covers the wasm builds'
  target-specific dependencies.

## Measurement analyzers

`deny.yml` runs `python3 -I -m unittest discover -s scripts/tests`: the
tests of `scripts/h264_parity_bars.py`, the one analyzer of an H.264
round's speed bars, and the check that no new file named `analyze*.py`
is added under `docs/measurements` (it reads names only; the ones there
are listed in
`scripts/tests/analyzers-grandfathered.txt`; `docs/design/h264.md`, "A
round's bars: one analyzer, complete reports"). It runs there, not in
`ci.yml`, because a round's measurements often change `docs/measurements`
alone, which `ci.yml`'s filter skips. A new step of an existing job, so
the required-check list does not change.

## Issue references

The code here started in another repository, and its comments, tests,
scripts, notes and file names cited that repository's issues and pull
requests by number. Those numbers mean nothing here, or name an unrelated
issue of this repository. `deny.yml` runs `scripts/issue_refs_check.py`
and its tests: every tracked text file and every tracked path fails on a
bare `#N` and on `issue N`, `PR #N`, `prN`, `pull request N` and
the like, a name wrapped onto the next line included. The error names the
fix: say what the issue or pull request is about, or, for one of this
repository, write `coreav-dev/coreav#N`.

- **By form, not by number.** Both repositories number from 1, so no
  number or range tells them apart, and a list of the other repository's
  numbers would publish them. Naming the repository is the one form that
  is unambiguous, and it costs a few characters in a comment.
- **Not references:** libFuzzer's status lines, `uname`'s build number,
  ffmpeg's stream specifiers, ffprobe's read intervals, the host gate's run
  counters, and the salts of held-out draws, whose text fixes the draw.
  The test data of the pull request text checks, where GitHub reads `#N`
  as this repository's, are exempt by file.
- **Not caught:** a number alone in a file name or a build label
  (`summary-123.json`, `mbl-123`), which names hold for counts, dates
  and sizes too, and the contents of compressed files. Review stops those.
- **In `deny.yml`,** which nothing filters: a comment can change in any
  file, a documentation-only one included.

## The documentation site

Three checks keep the documentation site in `docs-site/` true to the code (`docs/design/docs-site.md`, "Checks"), each in a workflow that already runs where its input changes:

- `crates/coreav-cli/tests/docs_site.rs` compares each command's `--help` with the help text its page shows, and fails on a command without a page. `cargo test --workspace` runs it in the default build; one more step of `ci.yml`'s `check` job runs it with `--features ffmpeg-fallback`, for `coreav thumbnail` and `coreav transcode`.
- `scripts/tests/test_docs_site.py` checks the page list, the links into the repository and between pages, the measurements index and the public text. It is one of the tests `deny.yml` already discovers in `scripts/tests`, so it runs on every pull request, including one that only renames a design note a page links.
- `docs-site.yml` runs Mintlify's strict build and its link check with the CLI's npm package at a pinned version and no account, on changes to `docs-site/`. It is not required (`# workflow-check: not required`): requiring it would change the branch ruleset, and the two checks above, which are, cover what a broken page costs most often.

None of them adds a required check name, so the table below is unchanged.

## The 0.1.0 compatibility round

`compat-0.1.0.yml` runs R2 and R3 of the 0.1.0 round (`docs/measurements/compat-0.1.0/README.md`) by hand, at a commit given as input, on spent files (`rehearsal`) or on draw 6 (`draw6`, which also needs `open_draw`), split over four runners. The R2 and R3 shards receive the corpus credentials only in their fetch steps. The R4 rehearsal and scan steps combine fetching with tool execution and retain those read-only credentials in their environment. It publishes counts only, in its job summary and the `compat-0.1.0-summary` artifact; the per-file records are sealed to `tools/compat-corpus/evidence-public.pem` before they leave a runner, because an artifact of a public repository is readable by anyone. A pull request that changes the workflow runs the rehearsal, which also runs the R4 host's job on a runner (`scripts/compat_r4_gcp.py --local`) and scans which R4 pairs of the gate's and the rehearsal's files write to the fallback log (`tools/compat-corpus/r4_scan.py`): any such line voids R4, so the scan names, before the round, the commands and files that would void it. It is not required (`# workflow-check: not required`): it is a measurement, not a gate. R1 runs as a step of the three fixture jobs in `ci.yml` (`isobmff-fixtures`, `mkv-fixtures`, `ts-fixtures`), `run.py commands --set 0.1.0` with no baseline; its verdict goes to their job summaries and its report to their artifacts, and a miss or a void run does not fail them, so a change of the oracle's output is reported there without blocking unrelated pull requests; a run that measured nothing fails them. The workflow's report step likewise takes a miss or a void run as a result and fails on a run that measured nothing.

## Labels

The repository's labels are kept as code in `.github/labels.yml`: area labels (`area:h264`, `area:compat` and so on), kind labels (`bug`, `perf`, `feature`, `docs`, `tracking`) and resolution labels (`duplicate`, `wontfix`). `labels.yml` in `.github/workflows` runs `scripts/label_sync.py` on a push to main that changes the file, the script or the workflow, and by hand: it creates or updates every label in the file and deletes the ones listed under `retired` (GitHub's default labels this set replaces). `deny.yml` runs `label_sync.py --check` on every pull request, so a file the script would refuse, or an issue form that sets a label the file does not define (GitHub drops such a label silently), fails before it merges, not after.

- **Retired labels are named, not inferred.** A label in neither list is left alone and printed. A strict mirror would delete every label not in the file, including the ones workflows own: `pr-redirect` finds the issues it already opened by `from-pull-request`, and `python.yml` and `npm.yml` read `wheels` and `binaries`. Deleting a label also strips it from every issue that carries it, so deletion happens only for a name someone wrote down.
- **The built-in token with `issues: write`, and nothing else.** The labels API needs that permission and no secret; the workflow runs only on a push to main or by hand, never on a pull request's code. The job runs only on main and only in this repository, so a manual run on another branch does nothing and a fork's labels are left alone. It is not a required check (`# workflow-check: not required`): it never runs on a pull request.
- **A short standard-library script with `gh`, not a third-party action.** The label-sync actions in common use would each need an entry in the Actions allowlist, for what is one short script. The file is written in a narrow shape the script parses itself, as `workflow_check.py` does, rather than adding a YAML library to the runner.
- **The issue forms set a kind label** (`bug`, `feature`, and `perf` for a benchmark result); areas are added by whoever triages, since a form cannot know them. The sample-media form sets none: an offered file is not yet a bug or a feature.
- **`good first issue` and `help wanted` are retired.** Code pull requests are not accepted (`CONTRIBUTING.md`), so labels that invite them would bring contributions that end up turned into issues.

## Rejected alternatives

- **Require the heavy jobs directly, with no report job.** It works once
  the filters are on the jobs, because an `if:`-skipped job reports a
  success, and it is one job cheaper per workflow. Rejected because the
  ruleset would then name a dozen job ids and every matrix combination
  (`smoke (20)`, `smoke (22)`, `smoke (24)`), and renaming a job or adding
  a matrix entry would silently leave a required check unreported. The
  report job is one stable name per workflow.
- **Keep the trigger filters and add a companion workflow with the inverse
  filter**, the shape GitHub's own documentation suggests. It is cheaper
  still — the companion runs only when the real workflow does not — but it
  duplicates every filter list in a second file, where the two can drift
  apart without anything noticing until a pull request hangs.
- **A filter step inside each heavy job that exits early.** No
  `<workflow>-changes` job, but every heavy job then starts a runner, which
  costs more than the one job it saves as soon as a workflow has two.
- **A third-party action for the changed files** (`dorny/paths-filter`).
  It would need a new entry in the Actions allowlist for forty lines of Python.

## Required checks

### Required status checks (exact names)

These are the names a branch ruleset requires, exactly as written. Only a check that
**appears on every pull request** can be required: a workflow its path filter skips
reports nothing, and a required check that never reports leaves the pull request
pending for ever. Each required workflow therefore filters in its jobs, not in its
trigger, and ends in one report job that always runs and fails when a job above it
failed.

| Check | Workflow | Note |
|---|---|---|
| `deny` | `deny.yml` | `cargo deny check`, the fuzz projects' own graphs, and the DCO sign-off. No filter at all: it runs in full on every pull request |
| `ci-report` | `ci.yml` | Every job of `ci.yml`: `cargo fmt`, `clippy` and the workspace tests on Linux, `clippy` and the workspace tests on macOS arm64 and Windows x86-64, the MSRV build, on a pull request the fuzz run of the targets it can affect, the ISO base media, Matroska and transport stream oracles, H.264 conformance (bar C1 of `docs/design/h264.md`) and the instruction-count guard |
| `python-report` | `python.yml` | The abi3 wheel and its tests |
| `npm-report` | `npm.yml` | The npm packages, the launcher and the smoke tests |

## The pull request's shape

`pr-text.yml` runs on every pull request event, an edit of the title or body included, and fails when the text breaks a rule of the pull request template that a script can judge. Review used to check these by hand on every pull request, and missed them: fourteen of the first twenty-two commit subjects on `main` are longer than 72 characters.

- `scripts/pr_text_check.py` without the private list: Korean text, a session or run identifier, spend details.
- `scripts/pr_shape_check.py`: a title of the form `type(scope): summary` (a breaking change's `!` before the colon allowed) with one of the template's types, a summary of at most 50 characters and a title under 72; commit subjects of that form (merge commits excepted) and at most 72 characters. `Co-Authored-By` and "Generated with" lines are not checked. Each failure says what to do instead.
- Not checked, because they need judgment: one outcome per title, the template's sections, hard-wrapped body lines.
- Not a required check (`# workflow-check: not required`): requiring it would change the branch ruleset. Its failure still shows on the pull request, and a pull request is merged with every check green.
- Rejected: putting it in `deny.yml`, which does not run when only the title or body is edited.

## Before opening a pull request

Two checks run on the agent's machine, not in CI: this repository holds no
cross-repository token and no private secret.

- **The pull request's text.** `scripts/pr_text_check.py --title TITLE --body-file BODY.md` fails on Korean text, a session or run identifier and spend details, and, given the private list of names and message forms in `PR_TEXT_TERMS` (one case-insensitive regular expression per line), on those too. Run it with that list before opening the pull request and after editing its text; `pr-text.yml` runs it without the list. `scripts/pr_shape_check.py --title TITLE --commits FILE` (FILE from `git log --format='%H%x00%P%x00%B%x00' origin/main..HEAD`) is the shape check `pr-text.yml` runs.
- **The code-similarity check.** A change to `crates/coreav-h264`, `crates/coreav-kernels-h264` or `crates/coreav-opus` is compared, before it merges, with the other implementations' sources by the check kept in the operations repository, run there on the pull request's head; its result (pass or fail, and the commit checked) goes into the pull request's Evidence.

## Cost

A report job and a changes job per required workflow (`ci.yml`'s used to be
six of each), about a minute each billed, so about six minutes of runner time
per pull-request event, whatever the change touches; a documentation-only
pull request paid about one before the report jobs. Three smaller additions
on top: `python.yml` and `npm.yml` now also start on a push to main and on a
label whatever the paths, where their trigger filter used to stop them; and
because the report jobs carry `if: always()`, a run superseded by a new push
starts its report job too, which then reports the cancellation as a failure
on the superseded commit ("Concurrency" above says why not `!cancelled()`).

Measured on 2026-10-30, a pull request push that changes `ci.yml` (every job
of `ci.yml` runs, and the changes and report jobs of `python.yml` and
`npm.yml`): 25 jobs started, 11 of them changes or report jobs, with one
report per group of `ci.yml`'s jobs; 20 jobs, 6 of them changes or report
jobs, with one `ci-report`.

The heavy jobs are still skipped, which is where the minutes of an earlier change and
of the shorter pull request fuzz runs actually are. The whole cost falls to nothing when the
repository is public and standard runners are free.
