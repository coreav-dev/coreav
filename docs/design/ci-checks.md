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
  Its first job, `<workflow>-changes`, decides from the event's changed
  files whether the rest has to run; every other job carries
  `needs: <workflow>-changes` and
  `if: needs.<workflow>-changes.outputs.run == 'true'`, next to whatever
  condition it had.
- **It ends in `<workflow>-report`,** which carries `if: always()`, waits
  for every other job of the workflow, prints their results and fails when
  one failed or was cancelled. A job the filter skipped reports `skipped`
  and passes. That name is the required check: it appears on every pull
  request, including one that changes documentation only.
- Workflows that are **not** required keep their trigger filters: the
  informational `mediainfo-oracle`, the spikes (`spike-build`, `h264-n1`,
  `h264-n2`, `h264-n2-arm-speed`, which build spike code and are kept
  with it out of the public tree), the nightly `pr-redirect` and `labels`.

`scripts/workflow_check.py` runs in `deny.yml` — the workflow nothing
filters — and fails when the required-check table and the workflows
disagree: a listed name no job reports, a report job missing from the
table, a required workflow that grew a trigger filter again or lost its
`pull_request:` trigger, a report job without `if: always()` or missing a
dependency, two jobs that would report the same check name, a filter
pattern the action would read differently from GitHub, a filter written
in a form the checker cannot read, or a step (in a `.yml` or `.yaml` workflow,
in a local action, or in any local action or workflow a step calls with
`./...`) that uses an action by a tag or branch rather than a full
lowercase commit sha: a tag can be moved to other code, so every step names
the commit and keeps the version as a comment. The pin check reads the
files as YAML block mappings, skipping comments and scripts, and fails on
a form it does not decode (an escaped or tagged key, an alias, an explicit
`?` key, a flow collection that nests or spans lines) rather than passing it. A workflow that carries the line
`# workflow-check: not required` is outside the table: its report job need
not be listed, and the table may not list it.

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
`workflow_check.py` enforces both halves: a report job must cover every
job that is not label-gated, and must cover none that is.

## The filter

`.github/actions/changed-paths` is the shared decision. It takes one
`paths` or `paths-ignore` list, in GitHub's own filter-pattern syntax and
with GitHub's own semantics (`paths`: run when any changed file matches;
`paths-ignore`: run unless every changed file matches), so moving a list
from the trigger into the action does not change which events run.

The changed files come from git, which is why each `<workflow>-changes` job
checks out with `fetch-depth: 2`:

- a pull request: the merge commit against its first parent, the base —
  the diff `ci.yml`'s fuzz job already used to choose its targets;
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

## Default builds and patents

Default builds contain no implementation of a still-patented codec, and CI
checks it. `scripts/patent_free_check.py` is that
check. It runs in `deny.yml`, beside the licence checks, so no filter
skips it. It holds the list of crates that implement such a codec
(`coreav-h264`, `coreav-kernels-h264` and `coreav-backend-h264` today, all
H.264), runs `cargo tree --target all -e normal,build` for the default
features of every other workspace member, and for each build a package
script makes with other features (the npm command's
`--no-default-features --features cli`), and fails when one of those
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
| `ci-report` | `ci.yml` | `cargo fmt`, `clippy`, the workspace tests, the MSRV build and the fuzz run |
| `fixtures-report` | `fixtures.yml` | The ISO base media oracle: ffprobe, MediaInfo, the command and the writers |
| `mkv-fixtures-report` | `mkv-fixtures.yml` | The Matroska and WebM oracle |
| `ts-fixtures-report` | `ts-fixtures.yml` | The transport-stream oracle |
| `bench-guard-report` | `bench-guard.yml` | The instruction-count guard |
| `python-report` | `python.yml` | The abi3 wheel and its tests |
| `npm-report` | `npm.yml` | The npm packages, the launcher and the smoke tests |
| `h264-conformance-report` | `h264-conformance.yml` | Bar C1 of `docs/design/h264.md`: every in-scope JVT conformance bitstream decodes bit-exactly |

## Before opening a pull request

Two checks run on the agent's machine, not in CI: this repository holds no
cross-repository token and no private secret.

- **The pull request's text.** `scripts/pr_text_check.py --title TITLE --body-file BODY.md` fails on Korean text, a session or run identifier and spend details, and, given the private list of names and message forms in `PR_TEXT_TERMS` (one case-insensitive regular expression per line), on those too. Run it with that list before opening the pull request and after editing its text.
- **The code-similarity check.** A change to `crates/coreav-h264`, `crates/coreav-kernels-h264` or `crates/coreav-opus` is compared, before it merges, with the other implementations' sources by the check kept in the operations repository, run there on the pull request's head; its result (pass or fail, and the commit checked) goes into the pull request's Evidence.

## Cost

Two small jobs per required workflow, about a minute each billed, so about
twenty minutes of runner time per pull-request event, whatever the change
touches; a documentation-only pull request paid about one before. Three
smaller additions on top: `python.yml` and `npm.yml` now also start on a
push to main and on a label whatever the paths, where their trigger filter
used to stop them; and because the report jobs carry `if: always()`, a run
cancelled by a new push starts its report jobs too, which then report the
cancellation as a failure on the superseded commit. `!cancelled()` would
avoid that, at the price of saying nothing when a heavy job is genuinely
cancelled, which is the case worth seeing.

The heavy jobs are still skipped, which is where the minutes of an earlier change and
of the shorter pull request fuzz runs actually are. The whole cost falls to nothing when the
repository is public and standard runners are free.
