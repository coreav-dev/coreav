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

The refusal groups are unsupported codec or container mapping, unsupported
option, and refused file or stream layout. These groups summarize the
observed reasons; they are not lists of files or pairs to exclude. Retain
the logged reason beneath its group. An unrecognized reason remains
visible rather than being discarded. Whether a pair is a refusal is
decided from its own invocation and log, not from membership in a reason
group or from a previous rehearsal.

Classify each pair before collecting its timing samples. The run itself
decides the refusals under this rule; there is no pre-listed refusal set,
manual override or removal after inspecting timings. Both gate arms and
the measurement arm use this same rule when they invoke CoreAV. An arm
without CoreAV has no CoreAV refusals. A new refusal or failure after a
pair has qualified for timing is not a reason to discard its completed
samples and continue; the existing timed-failure rule applies.
In the null arm, both CoreAV sides must satisfy the refusal rule; a
refusal on only one side does not qualify the pair for this exclusion.

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
must not suppress another failure of the same pair.

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
explained. Reports list commands with no timed pair instead of omitting
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
3. Rehearse the tools on spent inputs without changing these rules from
   the observed results.
4. Name the exact release-candidate commit after all release-scoped
   prerequisites merge, then follow the existing R1, gate, R4, R2 and R3
   order. No final run is authorized by this document alone.
