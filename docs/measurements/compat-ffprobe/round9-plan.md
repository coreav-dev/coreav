# ffprobe round 9 validation plan

This is a new validation arm, not a rerun of round 8. Its plan merges
first, its measurement tools merge second, and its results are published
only after the run. The code fixes remain a separate change. No round 9
measurement starts before the plan and tools are on `main`.

The corpus known-divergence amendment below must merge before a separate
comparator-and-audit tools change, and that change must merge before the
run. The already registered B4 method is unchanged. This amendment does
not apply retrospectively to round 8 or to another registered arm.

## Round 8 remains a miss

Round 8 compared main `f278e25` and candidate `f715de8` with the same
pinned FFmpeg 9.0.2. Both compared 1,315 files, including six structural
rejections covered by the independent known-divergence check. The counts
below exclude tags and keys found in fewer than five files.

| Container | Keys below 98%, base | Keys below 98%, candidate | Eligible keys |
|---|---:|---:|---:|
| ISO base media | 28 | 25 | 106 |
| Matroska and WebM | 25 | 22 | 113 |
| MPEG-TS | 44 | 39 | 177 |

Round 8 misses B3a. These counts stay attached to the measured candidate,
not to later fixes. A missing upload-harness import invalidated six
baseline rows; only those rows were repaired with the same binary and
oracle. None of the valid rows was rerun.

Its B4 host passed both calibration arms. Of 5,301 command/input pairs,
4,976 were timed successfully, 285 were strict-safe refusals of five
refused-by-design commands, and 40 failed during warm-up. All 40 forced
MPEG-TS on non-TS inputs: both tools rejected 32; CoreAV rejected eight
that FFmpeg accepted. No timed pair failed. The fallback log contained
the refusals, not an executed FFmpeg fallback, but the job treated every
entry as fatal and exited nonzero.

The successful-pair medians were 0.079876 overall, 0.079542 for ISO base
media, 0.079767 for Matroska/WebM, and 0.214164 for MPEG-TS. No measured
command's median exceeded 1.00. This is not a clean verdict for the whole
B4 protocol: its forced-format input selection was wrong, and its
confidence intervals resampled command/input ratios rather than files.
Those intervals remain historical output, not independent-file
inference. Round 8 is not rerun or re-scored with the method below.

## The bars and inputs do not change

The [B1–B6 table](../../design/compat-cli.md#the-bars-pre-registered)
remains normative: B3a requires at least 98% agreement per key after
documented known divergences, and B4 requires the median ratio to be at
most 1.00 per command, overall and per class. The fixture requirement, raw
corpus population, key-eligibility counts and experimental label do not
change. Only the validated row exclusions below adjust a gated
denominator. A container-level breakdown is reported without inventing a
new bar.

B3a uses the same 1,315 files as round 8. B4 uses the existing frozen
selector in `tools/compat-corpus/speed.py`: up to 20 held-out files per
container within its size limit and fixed ordering. That selects 20 ISO
base media, 20 Matroska/WebM and 17 MPEG-TS files. The 93 native ffprobe
commands of `commands-v1` are unchanged. The five documented refusals
remain in the report as refusals with no runtime ratio; they do not
satisfy B2 acceptance or become speed wins. A command with no successful
timed input is reported as unmeasured, never as a B4 pass.

The development and held-out partitions remain separate in the report.
Inputs inspected while developing these fixes are not independent
validation. No new file or command is chosen because its round 8 values
were favourable. Known divergences require the file's bytes and the
specification; a mismatch is not excluded merely because it misses a bar.

## Corpus key-row exclusions

A known divergence may exclude a B3a corpus key row only when all three
conditions hold:

1. A cited specification clause shows that pinned ffprobe 9.0.2's value
   is wrong, or that CoreAV's differing value is the specification's
   defined reading. An implementation preference or an undecidable
   interpretation is not enough.
2. A predicate validates that reading from the file's bytes. It binds
   the exact differing occurrences to their stream or packet and checks
   both observed values against the byte-derived readings. CoreAV's own
   output, a file name or a container label alone is not evidence.
   Missing, unsupported or unreadable byte evidence covers nothing.
3. The predicate and its tests are committed before the run, through the
   separate comparator-and-audit tools change. A prose record alone does
   not activate coverage.

A row is one file's agreement result for one key, in one container, not
one occurrence of that key. Exclude it only if every differing occurrence
is validated by an eligible predicate. An uncovered difference in the
same key, another stream or another packet must remain a disagreement.
An agreeing row is never excluded. No entire file is removed merely
because one of its keys has a known divergence.

### Freeze the records before implementing the hook

The cutoff is the authoritative commit on `main` that introduces this
plan amendment. Record that commit and the eligible KD identifiers,
specification clauses and field/case scopes in the run manifest. Only
records present at that cutoff may be implemented for this round. A new
KD or a wider scope under an existing identifier after the cutoff is not
eligible; it needs a separately registered future arm. The tools change
may implement the frozen records, but may not add cases. Its predicates,
tests and audit must all be committed before the first comparison.

Initial candidate records are `KD-TS-VBI-AS-AUDIO` (VBI data sniffed as
MP3), `KD-TS-PROGRAM-WRAP` (cross-program 33-bit timestamp unwrapping)
and `KD-TS-ANNEX-D-DOWNMIX` (AC-3 Annex D multichannel coded levels),
documented in [the transport-stream records](../../known-divergences-ts.md).
Being named here does not grant coverage: each record must be present at
the cutoff and independently satisfy every condition and audit above.
The existing `KD-TS-DECODE-TIME` design is not widened by this amendment.

### Attack every exclusion

Use the adversarial method of the
[known-divergence audit](../kd-audit.md), not a review of predicates alone.
Committed tests attack every predicate on generated fixtures and permitted
development data, including matching, directly agreeing and nonmatching
cases. Every exclusion used by the run must also pass that audit on its
captured comparison before it can affect the verdict.

Inject bug-shaped changes in the candidate captures: wrong values, lost
or duplicated streams or packets, changed identities, and values moved
together with CoreAV's own claimed evidence. Any change that introduces
or worsens a raw gating difference must remain uncovered; it must not be
absorbed by an exclusion or activate one on an otherwise agreeing row.
Byte-evidence mutations and negative fixtures must also show that a
changed or inapplicable file cannot retain coverage. A covered mutation
is a loophole and blocks use of that exclusion. Audit failures in the run
make the result nonpassing, not an opportunity to add a KD or re-score
the arm. The audit's known comparison blind spots are reported, never
counted as successful attacks. Do not use round-9 held-out inputs to
develop or tune predicates or mutations.

### Publish raw and excluded rows separately

Keep each raw per-file/key result and publish the excluded rows separately
using opaque file aliases, never media hashes or source paths, with
counts per KD, per key and per container. Keep development and held-out
breakdowns. A row supported by several KDs counts once in the excluded
total; publish attribution overlaps rather than double-counting it.

Report the literal raw agreement numerator and denominator beside the
gated agreement. Preserve the existing byte-validated `KD-STRICT`
structural-rejection treatment: its covered rows remain in the gated
denominator and count through that record, separately from direct
agreement and from the new exclusions. Thus the exclusive row totals are
direct agreement, existing covered agreement, new exclusions and uncovered
disagreement. The gated numerator is the first two totals; its denominator
is the raw row count minus the new exclusions. Key eligibility is decided
from raw numbers of files containing each key, not recomputed after
exclusions. A key with no remaining rows is unmeasured, not a pass.
The 98% bar is unchanged.

## Method corrections fixed before the tools change

**Forced-format eligibility.** The sole native forced-format command is
`-f mpegts`. It takes only the MPEG-TS family, so the 40 non-TS
command/input pairs are marked ineligible before either tool runs.
Unforced commands take all selected families. The runner reports the
eligible, ineligible, refused, failed and successfully timed pair counts
separately. This prevents an input-selection error from looking like a
tool defect or a fast rejection. It does not relabel a command, remove a
selected file, or turn an ineligible pair into agreement.

**File-cluster confidence intervals.** A file is the resampling unit,
because several commands and repeated calls on that file are correlated.
For each reported group, draw contributing file identifiers with
replacement within each contributing container, keeping that group's
number of files in each container fixed. Each drawn file carries all its
command/input ratios in the group; drawing it twice repeats that entire
cluster twice. Compute the median over the resampled ratios, without
changing the point estimate's weighting. Use 10,000 resamples, seed 0,
and the existing percentile convention for the 95% interval. A group
with no ratios has no interval. This replaces the old pair-level
bootstrap, not the median-ratio bar.

**Failures and fallback records.** A failed or timed-out warm-up or timed
call has no ratio. Both exit statuses and the failure phase are recorded.
A timed failure makes the arm incomplete; a failed warm-up stays a
reported failed pair rather than disappearing from coverage. A
strict-safe refusal record is distinguished from an executed fallback.
Any executed fallback invalidates the native measurement. These checks
address the failure and refusal cases already reported for round 8.

The tools change adds tests for forced-format family selection, complete
file-cluster resampling, deterministic intervals, failure accounting and
fallback classification. It does not use the per-key results to select a
new comparison rule. The later corpus comparator-and-audit change
implements only the records frozen by this amendment, under the rules
above; it introduces no new KD cases.

## Run identity and host controls

Before launch, record the candidate commit, its base commit, the merged
tooling commit, binary checksums, oracle pin and frozen input/command
selection in a run manifest. The candidate must be a fixed `main` commit
with fully green CI, including the instruction guard and the required
integration, guard and container key fixes. Record its fetched-main
baseline too. The container key-fix changes must be merged or their
remaining gaps reported unclosable before launch. Neither revision
changes between comparisons. Both sides of B3a use the same oracle and
comparison tools. The manifest is fixed
before the first comparison, not edited to describe a better result.

Every oracle comparison uses `scripts/oracle-ffmpeg.env`; the installed
binaries pass `scripts/oracle_pin.py --verify-bin`, including cache hits.
This arm compares listings, not decoded PCM. Separate FLAC/ALAC audio
decoding checks use one oracle decoder thread to avoid the 9.0.2
frame-threaded initial-skip problem; no unregistered thread option is
injected into a timed ffprobe command.

B4 uses the c4-standard-8 host class with SMT disabled and both tools on
the same pinned physical core. Each pair has one untimed run of each
tool, then five alternating timed pairs, one process at a time, with the
whole process timed. CoreAV is a release build in strict-safe mode. The
selected inputs are checked against the frozen manifest before timing.

Before B4, run FFmpeg/FFmpeg and CoreAV/CoreAV calibration on the first
selected file of each container with `-show_format -show_streams` and
with `-show_packets`. Each case has one untimed pair and five timed
pairs. Each arm must have an overall median ratio in [0.98, 1.02] and a
95% interval containing 1.00; any process failure fails calibration.
Failed calibration prevents B4 from starting. Calibration also clusters
its repeated calls by file, but resamples the three files together,
without container strata: with one file per container, a stratified
draw could only reproduce the same three files and would give a
degenerate interval. This exception applies only to the host gate, not
the performance groups.

Heavy runs execute remotely, with a hard timeout. A host fault is
recorded before any replacement run. A missed valid arm is final: no
rerun for a narrow miss, a method change, or a different subset. Results
include raw per-call times, failure counts, the share of timed pairs
below 1.00, per-command and per-container breakdowns, and the unchanged
bar verdicts. Public evidence omits media identifiers, source paths and
host-instance identifiers.
