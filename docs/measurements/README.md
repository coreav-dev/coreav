# Measurements and their evidence

Each measurement here is a Markdown page with its numbers and the raw
files the numbers come from: JSON results, oracle captures, run logs and
the metadata of the machine that ran it. Spike measurements keep the same
shape under `docs/spikes/<name>/evidence/`. The pass bars are committed before
the run, so the page
states a bar, the result and the verdict, and the evidence next to it is
what the result was computed from.

The evidence is committed as the run produced it. It is never edited to
make a result look different, and a corrected run is a new run with its own
evidence; a failed or superseded run stays where it is, marked as such.

Branches that the records name (`measured/…`, `rejected/…`, `exp/…`,
`probe/…`) held the code a round measured. They are kept in the
repository where the work was done and not published: what a round
measured and concluded is here, in its page and its evidence, and a
rejected change is kept as its diff where the record needs one. A change
measured from now on is measured on a branch, and its evidence lands with
the pull request that gates or merges it.

## Redactions

The things below are replaced in the committed evidence, everywhere they
occur. No number, time, hash, outcome or status is changed by any of them,
and nothing else in these files is changed. They are listed here so that a
reader can tell a redaction from the output of a tool.

| In the evidence | Replaced by | Where |
|---|---|---|
| the home directory of the machine that ran the measurement, `/home/<user>` | `$HOME` | 36 paths in 11 files under `docs/measurements/` and `docs/spikes/*/evidence/`, then 26,668 more in 907 files under `docs/` and the spikes' code (2,478 in plain files, 24,190 inside 10 archives), then 34 more in 13 files of `docs/measurements/h264-rounds/first-touch/`, then 32 more in 13 files of `docs/measurements/h264-rounds/hints-8t/`, then 4 more in 1 file of `docs/spikes/audio-decode-bench/evidence/`, then 48 more in 19 files of `docs/measurements/h264-rounds/bundle3/`, then 4 more in 2 files of `docs/measurements/h264-rounds/bundle4/local/`, then 8 more in 3 files of `docs/measurements/h264-rounds/bundle4/gating/`, then 2 more in 1 file of `docs/measurements/h264-rounds/bundle4/gating-1t-gcp/` |
| that machine's account name, where it stands as a word: the owner columns of `ls -l`, and the owner and group of each member of a tar archive | `$USER` | 6,176 in 14 files (2 logs and 12 archives) |
| a scratch directory of the checkout that git ignores, as a path component (`.../<dir>/bin/...`, where the runs kept their builds) | `$WORK` | 23 paths in 13 files under `docs/measurements/h264-*` |
| the name of the machine's provider in "`<provider>` VM" or "`<provider>` cloud VM" | "development VM" | 41 in 39 files under `docs/measurements/` and `docs/spikes/*/evidence/`, the round notes beside the logs included, then 4 more in 4 files (three round notes under `docs/measurements/h264-rounds/` and `docs/design/h264.md`) |

- Records of cloud-host runs that name accounts or cost (run and deployment URLs, application, image
  and task ids, billing reports, budget ledgers, launch scripts) are not published; the ids that
  stood as fields in the JSON evidence were removed, the platform's name in other field names and
  texts became `remote` or "cloud", and nothing else in those files changed.
- Box-drawn log output (`│ … │`) keeps its width: the four characters the
  shorter `$HOME` frees are padded back before the closing border, so the
  box still lines up.
- `docs/spikes/clip-bench/evidence/commit-speed-185/speed.py` is a script, not a
  log, so it reads `os.environ['HOME']` where the path was: `$HOME` in a
  Python string literal would be a path that does not exist.
- The later rows were made by a redaction script, which can be run again on new evidence. It takes the strings to redact as
  arguments, so that the tree does not name them, and changes nothing when
  the SHA-256 or git blob hash of a file it would change is recorded
  anywhere in the tree (none was). Compressed evidence is redacted inside:
  a `.gz` or `.xz` file is written again with the same header name and
  time, a tar with each member's name, order, mode, time and ids, at a
  fixed compression level, so the archive's bytes differ but rerunning the
  script on the same input, with the same zlib and liblzma, gives the same
  bytes (a rewritten gzip header also records the writer's OS byte and
  compression flag).
- In the scripts kept as they were run (`ab8.py`, `run-1t.sh` and the
  like), the replacement is textual too: Python reads `$HOME` in a string
  literal as it stands. These scripts name the run machine's directories
  (`$HOME/h264work/bins/...`) throughout and are edited before any rerun
  anyway; `speed.py` above predates this rule.
- The configuration string that the third-party `ffmpeg` builds report
  (`--prefix=... --cc=/home/conda/feedstock_root/...`) is not redacted. It
  is the build identity of the binary the measurement compares against,
  published by whoever built it, not a path of ours.

These redactions cover personal and account details.
