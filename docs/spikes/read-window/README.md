# The read window's in-place refill, measured

Throwaway measurement code, outside the Cargo workspace. It runs
two `coreav` binaries on corpus v1: the branch of an earlier change
(`docs/design/read-window.md`) and a control built from `main` at the
branch's base, on one machine in one run.

Python only: `run.py` needs `boto3` (Apache-2.0) for the R2 downloads and
imports the policy-check spike's runner (downloads, run caps) and
`scripts/policy_ref.py` (whether an ISO base media file is fragmented).
No Rust code, so there is no Cargo graph to license-check.

## Hypothesis and bars

Refilling `BufferedSource`'s window in place, and reading frame headers
into stack buffers, removes the zeroing that was 27% of `check`'s
instructions on a fragmented file, changes no output, and makes no
command slower. The bars, quoted from the design note and committed
before the run:

| Bar | Set | Threshold |
|---|---|---|
| R1 identity | every corpus v1 file (2,970) | 100%: `check --policy P` for each of the three example policies, `probe --scan-frame-sizes` and `remux FILE OUT` print the same standard output and error and exit with the same code as the control; `remux` writes the same bytes (SHA-256) |
| R2 instructions | development files with an ISO base media, Matroska or WebM extension, for `check --policy web-upload.json` and `probe --scan-frame-sizes`; those up to 200 MB for `remux` | the whole process's instructions (cachegrind, cache simulation off): the total over the set and the median per-file ratio to the control below 1.00 for each command, and no file above 1.01 (Rust's randomly seeded hash maps move counts slightly from run to run) |
| R3 wall time | the sets of R2 | per file, the median of 5 rounds after one warm-up, the order of the binaries alternating, warm page cache; the median per-file ratio to the control at most 1.00 for each command, overall and per class (ISO base media fragmented, not fragmented; Matroska and WebM) |

"R1 and R2 misses block the merge. An R3 miss is investigated; it blocks
the merge when the 95% bootstrap interval of the median ratio lies wholly
above 1.00 (a measurable slowdown), and is otherwise reported as a miss
within the noise."

Reported, not bars: the reductions, and for `check` and `probe
--scan-frame-sizes` the share of development files on which `coreav` is
faster than `ffprobe -v error -count_packets -show_streams`, with the
control and with the branch. Development files are `real` files not held
out: a development measurement, not a claim.

## How it runs

    cargo build --profile dist -p coreav-cli            # in each tree
    python3 -m venv venv && venv/bin/pip install boto3
    venv/bin/python -I run.py identity --work WORK --control CTL --branch BR --ffprobe FFPROBE --jobs 6
    venv/bin/python -I run.py speed --work WORK --control CTL --branch BR --ffprobe FFPROBE
    python3 summarize.py --work WORK

R2 credentials come from `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` and
`R2_ENDPOINT`; the run caps downloads at 300 GiB and 10,000 requests per
phase. R2 charges no egress; the run is local.

## Results

Summary: `evidence/summary.json`; per file: `evidence/identity.jsonl.gz`,
`evidence/speed.jsonl.gz` (both binaries' wall times per round and
instruction counts, `ffprobe`'s times).

### Runs

One run of each phase on one sandbox machine (8 vCPU, Amazon Linux 2023,
16 GB), `coreav` built `--profile dist` with rustc 1.99.1 from this
change's production code and from `main` at `81f9ee2`; `ffprobe` 7.0.2.
The identity phase ran 6 files at a time; the speed phase one file at a
time, the next file downloading meanwhile (the order of the binaries
alternates by round, so that write lands on both alike). The speed phase
stopped once, after 503 of 886 files, when the shell that started it
ended; it was resumed with the same binaries, skipping the files it had
recorded. The two phases downloaded corpus v1 once and the development
files a second time (about 250 GB and 126 GB; R2 charges no egress, the
read requests cost under 0.01 USD).

### Against the bars

| Bar | Result | Status |
|---|---|---|
| R1 identity | 14,850 of 14,850 cases (2,970 files × 5 commands) the same | **Met** |
| R2 instructions | `check` −63.2% in total, median ratio 0.533; `probe --scan-frame-sizes` −64.2%, 0.513; `remux` (730 files) −12.2%, 0.974; highest file 1.0003 | **Met** |
| R3 `check` | 0.949 (95% CI 0.942 to 0.957); fragmented 0.957 (81), not fragmented 0.957 (665), Matroska and WebM 0.920 (140) | **Met** |
| R3 `probe --scan-frame-sizes` | 0.923 (0.916 to 0.930); 0.926, 0.929, 0.904 | **Met** |
| R3 `remux` | 0.9999 (0.9972 to 1.0019); fragmented 0.997 (76), Matroska and WebM 0.989 (112), not fragmented 1.0018 (0.9996 to 1.0038, 542) | **Met** overall and in two classes; **missed within the noise** in one |

The `remux` miss is investigated in the design note: it is the branch
binary's start-up (code layout: `coreav --version` 1.9% slower with the
same instructions, about two more page faults), visible in the 1.3 ms
calls on files under 1 MB (1.0095) and not from 50 MB up (0.998).

### Against FFmpeg (development files; not a claim)

`ffprobe -v error -count_packets -show_streams`, per-file ratio of the
medians:

| Command | Binary | Faster on | Median ratio | 95% CI |
|---|---|---:|---:|---|
| `check` | `main` | 883 of 886 | 0.138 | 0.132 to 0.144 |
| `check` | this change | 884 of 886 | 0.131 | 0.123 to 0.136 |
| `probe --scan-frame-sizes` | `main` | 884 of 886 | 0.136 | 0.129 to 0.140 |
| `probe --scan-frame-sizes` | this change | 884 of 886 | 0.123 | 0.117 to 0.130 |
