# 0.1.0 compatibility round: the plan

Status: planned. Nothing in this round has run.

This round measures the bars R1 to R4 of
[`design/compat-cli.md`](../../design/compat-cli.md) ("The 0.1.0 bars,
pre-registered") for the 33 command shapes of `commands-0.1.0`. The bars,
their method and the R4 host are that note's and are not restated here.
This page fixes, before any run, what each bar runs on, the order of the
runs, the hosts and the tools the round still needs. A change to it after
the first run of the round is recorded below as a deviation, not edited in.

## The commit

Every bar runs on one commit of `main`, the release candidate. A pull
request that only adds its SHA to this page names it, after every
prerequisite below has merged and before the first run. The binary is
`coreav` built from that commit with `cargo build --locked --profile dist
-p coreav-cli` and default features, so no decoder backend is linked;
its SHA-256 is recorded with the results. Every comparison runs the pinned
FFmpeg 9.x that `scripts/install-oracle-ffmpeg.sh` installs at that
commit, `ffmpeg` and `ffprobe` alike; the pin moves from 7.0.2 to the
latest stable 9.x before the round, and the record names the exact
version. Draw 6's sets were sorted with `ffprobe` 7.0.2, as the corpus
rules then stated; R2 and R3 run on every `real` file of the draw,
with the 9.x build as the reference.

Every run sets `COREAV_STRICT_SAFE=1` and `COREAV_FALLBACK_LOG`, as the
note requires; a run whose log holds a line is void. Every run of the pinned
`ffmpeg` or `ffprobe` sets `GCONV_PATH` to a directory with no gconv
modules, as `scripts/oracle-ffprobe.sh` does, and CoreAV's runs get the
same environment: the static 7.0.2 build crashed on transport streams
with broadcast text when it loaded a newer system's gconv modules, and a
static 9.x build is kept to the same environment
([`design/mpegts.md`](../../design/mpegts.md)), which `run.py` and
`speed.py` do not guard against today. A command that is
refused in strict-safe mode has no output and no time, so it misses R1 or
R3 and is not counted as faster or slower in R4.

## Before the round

A round that misses a bar is final, and the draw is spent by the first
round reported from it. So the round starts only when nothing it needs is
still being built:

- **Every command of the set runs natively.** Today the clips of audio
  alone (C15, C19) have no writer, and the rest of #25 and #24 is open.
  A command still refused when the round runs misses its bar.
- **The tools below exist, each merged through its own pull request** and
  tested on the fixtures and on spent sets:
  1. `tools/compat-corpus/run.py`: `commands` and `keys` restricted to
     `commands-0.1.0` (its `ffprobe` commands, and for `keys` only the
     keys the set prints), with the per-family report and the key-counting
     rule of R2.
  2. An R3 runner for media other than the fixtures: it fills each
     `ffmpeg` command's placeholders with a file the command can take
     (`input_container`, `input_without`, the streams its `-map`
     selectors need, and for `-f concat` a list that names the file twice,
     as the fixture test does), runs the pinned `ffmpeg` and `coreav
     ffmpeg`, and passes the outputs to `copy_check.py`. It counts only the
     pairs on which `ffmpeg` succeeds and reports the commands no file can
     take.
  3. An R4 runner, `tools/compat-corpus/speed.py` extended to the
     `ffmpeg` commands of the set: each output written to the VM's local
     disk and deleted after every run, the R4 order, the A/A and null
     arms, and a report per command class and container family with the
     share of pairs on which CoreAV is faster and the per-command medians.
  4. A driver for the R4 host over `scripts/gcp_vm_job.py`, as
     `scripts/h264_parity_gcp.py` is for the H.264 rounds: it uploads the
     two binaries, the VM reads the files from the corpus store with
     read-only credentials, runs the gate and then R4 on the same VM, and
     the VM is deleted. It has a hard time limit and a spend cap in the
     script, within the project's per-task limit.
  5. A `workflow_dispatch` workflow that runs R2 and R3 on GitHub-hosted
     runners with the corpus credentials the repository already holds, at a
     commit given as input. It publishes counts per key and per command,
     never a file's name or hash; the per-file records go to the private
     evidence store.
- **A rehearsal on spent sets.** Each tool runs end to end on spent files
  (corpus v1's held-out files and draws 1 to 5), never on draw 6. Its
  numbers are not reported as bars; it exists to find the faults of the
  tools before they can spend the draw.

## Inputs

| | Files |
|---|---|
| Fixtures | the files `scripts/gen-fixtures.sh`, `gen-fixtures-mkv.sh` and `gen-fixtures-ts.sh` generate at the commit |
| The draw | [`v2-heldout` draw 6](../../corpus.md#draw-6), its `real` files, per container family |
| R4 selection | from each family of draw 6, the first 20 `real` files of at most 256 MB in the order `sha256("coreav-compat-r4:" + sha256)` |
| R4 gate | the first 20 `real` files of at most 256 MB per family, in the same order, of the spent held-out sets: corpus v1 for ISO base media and for Matroska and WebM, `v1-ts-audio` for transport streams |

Each command runs only on the files of its own containers (`input_container`
and the `Inputs` column of the set), so the copy commands of `clip`,
`join`, `split` and `extract-audio` run on the ISO base media files only,
and `remux` on ISO base media and on Matroska and WebM.

## The bars

| Bar | Runs | Host | Opens the draw |
|---|---|---|---|
| R1 | `run.py commands` on the 0.1.0 `ffprobe` commands, each fixture family, with no baseline file: a failure a baseline lists is still a miss | GitHub-hosted runners, in the fixture workflows (`fixtures.yml`, `mkv-fixtures.yml`, `ts-fixtures.yml`) of the commit | no |
| R2 | `run.py keys` on every `real` file of draw 6, both tools, counting only the set's keys as the note says | GitHub-hosted runners, the workflow of item 5 | yes |
| R3 | the R3 runner on every `real` file of draw 6 that each command can take, then `copy_check.py`; and the fixture half as `fixtures.yml` and `mkv-fixtures.yml` run it today (`the_0_1_0_commands_on_the_fixtures`, then `copy_check.py`) | GitHub-hosted runners, the workflow of item 5 | yes |
| R4 | the gate, then the R4 selection: 5 rounds, the tools alternating, wall time of the whole process, file in the page cache after one untimed run of each | one Compute Engine `c4-standard-16` VM with one thread per core, both tools pinned to the same four cores, on-demand provisioning | yes |

R1 to R3 are correctness bars and run on shared runners, where timing does
not matter. R4 runs on the host the note registered; a shared runner pool
does not qualify for it.

## Order

1. **R1**, in the fixture workflows of the release candidate. It reads no
   file of the draw, so a miss here ends the round before the draw is
   opened, and the draw stays unspent for the next candidate.
2. **The R4 gate**, on the VM, on the spent files: the A/A arm (FFmpeg
   against itself) and the null arm (CoreAV against itself), each over
   the 0.1.0 commands. A failed gate is a host fault: the VM is deleted
   and the round goes on with a new VM of the same class.
3. **R4 on draw 6**, on the same VM, right after the gate passes. This is
   the first read of the draw. The record names the host class, the CPU
   model and the gate's numbers before the draw's results.
4. **R2 and R3 on draw 6**, in one run of the workflow, at the same commit.
5. **The record**: `docs/measurements/compat-0.1.0.md` with each bar, its
   result and its verdict, the evidence beside it in this directory, and
   draw 6 marked spent in `docs/corpus.md` and in its manifest.

A run is repeated only after a recorded host fault (a failed gate, a VM
lost or preempted, a crashed run), never after a narrow miss. R4 runs
before R2 and R3 because its gate must pass before the draw is opened.

## Expected size of the runs

R4 times at most 5,940 (command, file) pairs per arm (133 commands over
the families each runs on, 20 files a family), 12 processes a pair, in
three arms: the gate's two and the draw's. The draw's R4 selection is
2.6 GB over 60 files (median 14 MB), so most pairs are dominated by
process start-up; that gives an estimated four to six hours on the VM,
the gate's larger spent files included. The driver's time limit is set
from this estimate before the run. R1 runs within the fixture workflows' own time. R2 and R3 read
draw 6 once each and fit one workflow run.

## Deviations

None yet.
