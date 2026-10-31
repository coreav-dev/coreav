# R4 timing with strict-safe refusals

Status: pre-registered plan; implementation and the final round have not run.

This plan changes only R4's timing set and coverage reporting in the
[0.1.0 round](README.md). It replaces the rule that any strict-safe refusal
log line voids R4. It does not change R1, R2 or R3, or turn a refusal into a
correctness pass. The tools implementing this plan must merge separately,
after this plan and before the final run.

## Decide refusals during the run

Start with every eligible command/file pair under the existing container,
map and `input_without` rules. A pair leaves the timing set only when its
CoreAV invocation actually refuses it in strict-safe mode, exits with the
refusal status, and writes a corresponding refusal record with its logged
reason. The record must say `not_run: strict_safe` and `ran: null`; a
nonzero exit code alone is not evidence of a refusal.

The refusal groups have stable report codes: `codec_container` for an
unsupported codec or container mapping, `option` for an unsupported
option, `layout` for a refused file or stream layout, and `unknown` for an
unmatched reason. The tools PR must commit one exact-match table from
logged reason text to these codes before any run. Each table key names
exactly one group; no match means `unknown`, not a manual classification. Retain
all logged reasons beneath the group. If both null-arm sides qualify but
their reasons map to different groups, count the pair once in `unknown`
and retain both reasons. Group counts therefore sum to refused pairs.

This table classifies reasons for reporting, not files or pairs for
exclusion. A pair's own invocation and log decide whether it is a refusal,
including when its reason maps to `unknown`; neither the table nor a
previous rehearsal can select exclusions. The table is fixed with the
tools commit and cannot be edited after inspecting timing results.

Classify each pair before collecting its timing samples. The run itself
decides the refusals under this rule; there is no pre-listed refusal set,
manual override or removal after inspecting timings. Both gate arms and
the measurement arm use this same rule when they invoke CoreAV. An arm
without CoreAV has no CoreAV refusals. A new refusal or failure after a
pair has qualified for timing is not a reason to discard its completed
samples and continue; the existing timed-failure rule applies.
In the null arm, both CoreAV sides must satisfy the refusal rule; a
refusal on only one side does not qualify the pair for this exclusion.

Pair outcomes are exclusive. A non-refusal failure takes precedence over
any refusal observed on the other side. A one-sided null-arm refusal
paired with success is also a failed pair, not a refused pair. Both cases
count only in failures and follow the existing untimed-failure handling;
if either occurs after qualification for timing, the pair instead has the
existing timed-failure disposition and makes the run incomplete. Count
that pair once as failed, with the incomplete diagnostic, never again as
refused or timed. Swapping sides does not change the outcome. Oracle
failures retain their existing accounting rather than becoming refusals.

## Keep failures separate

A failure with no corresponding strict-safe refusal record, a crash, a
timeout or a wrong output is not a refusal. It remains a failure and
voids or fails the run exactly as the existing rules require. In
particular, a timed failure after both untimed invocations succeed makes
the run incomplete; it cannot be removed to improve the remaining ratios.
Infrastructure errors abort rather than becoming excluded child failures.
The pinned oracle's failures keep their existing handling.

An executed fallback, a malformed or unreadable log, or a log entry that
cannot be attributed to its invocation does not qualify for the refusal
exception and cannot produce a passing native result. Refusal records
must not suppress another failure of the same pair. Arm-fatal conditions
keep their existing void or abort disposition; a refusal cannot downgrade
them to a pair exclusion.

## Publish speed and coverage together

Every arm's report includes all of the following, in total and for every
command, command class and container family it reports:

- Eligible pairs, before excluding any strict-safe refusal.
- Refused pairs, with per-command counts, per-reason-group counts and the
  total. Count pairs, not repeated log lines or invocations.
- Timed pairs with complete valid timing samples.
- The share of eligible pairs timed: timed pairs divided by eligible
  pairs. A zero denominator is unmeasured, not 100% coverage.

Keep failed and oracle-failed pairs visible under their existing
accounting, so a gap between eligible, refused and timed counts is
explained. A pair occupies only one outcome total. Raw refusal events
from a mixed-outcome pair may remain in diagnostics but do not add to
refused-pair or refusal-group totals. In an aborted arm, unprocessed
eligible pairs remain explicitly unprocessed, not refused or timed.
Reports list commands with no timed pair instead of omitting
them. A group with no timing ratio remains unmeasured and cannot pass;
the existing no-measurement and partial-coverage rules still apply.

The public summary places the refusal count immediately beside every
speed number, with its eligible count, timed count and timed share.
Per-command and per-reason-group refusal counts accompany the summary.
The speed number describes only the timed pairs, never all eligible pairs
or all files CoreAV can encounter. Public evidence contains no file names,
hashes or other private input identifiers.

## Leave the rest of the round unchanged

The registered bars, draw and gate selections, command set, command
order, host controls, calibration bounds, five alternating timed rounds,
ratio calculation and confidence intervals are unchanged. Use the shared
event-driven timing method corrected in coreav-dev/coreav#122, including
its deadline, child reaping and infrastructure-error handling. Comparisons
use the shared FFmpeg 9.0.2 pin and retain its exact build version.

This plan changes no pass criterion beyond the timing set. It does not
rescore historical reports, allow a fresh draw to be opened early, permit
rerunning a missed arm, or waive the existing unmeasured-group and
incomplete-run failures. A speed result with refusals is explicitly a
result for the supported timed subset, with the coverage next to it.

## Prerequisites

1. Merge this docs-only registration before the implementation PR and
   before any final run.
2. Merge the separate timer correction in coreav-dev/coreav#122 and the
   tools PR implementing this plan. The tools must test positive logged
   refusals, failures without a refusal record, crashes, timeouts,
   incorrect outputs, oracle failures and complete coverage accounting.
   Include both side orders of a null-arm refusal paired with success or
   another failure, and reason-table cases for each group, unmatched
   reasons and different groups on the two sides. Assert exclusive
   outcome totals and that refusal-group counts sum to refused pairs.
3. Rehearse the tools on spent inputs without changing these rules from
   the observed results.
4. Name the exact release-candidate commit after all release-scoped
   prerequisites merge, then follow the existing R1, gate, R4, R2 and R3
   order. No final run is authorized by this document alone.
