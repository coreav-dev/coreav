# R4 timing with strict-safe refusals

Status: the original refusal accounting is implemented in
coreav-dev/coreav#127. This oracle-failure diagnostic amendment must merge
before its separate tools implementation and any final round.

This plan changes only R4's timing set, log validity and coverage reporting
in the [0.1.0 round](README.md). It replaces the rule that any strict-safe refusal
log line voids R4. It does not change R1, R2 or R3, or turn a refusal into a
correctness pass. The tools implementing this plan must merge separately,
after this plan and before the final run.

## Decide refusals during the run

Start with every eligible command/file pair under the existing container,
map and `input_without` rules. A pair counts as refused only when its
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
must not suppress another failure of the same pair. The diagnostic
exception below preserves the oracle failure rather than turning it into
a refusal or success. Other arm-fatal conditions keep their existing void
or abort disposition.

## Retain diagnostic refusals on oracle-failed pairs

When an oracle side fails during the untimed qualification runs, the pair
stays `oracle_failed`. It has no ratio and is not counted as refused or
timed. This outcome takes precedence even when CoreAV also refuses.

A CoreAV refusal on that pair may be retained as a diagnostic without
voiding the arm only when all of these conditions hold:

- The CoreAV invocation exits with the strict-safe refusal status, 4.
- Every nonempty line in its log is valid JSON, belongs to that invocation
  and names its tool. Each record has `not_run: strict_safe`, an explicit
  `ran: null` and a nonblank reason string.
- There is no other log event on the pair. An oracle-side log line cannot
  qualify as a CoreAV diagnostic.

Diagnostic qualification inherits the complete existing refusal predicate,
including a terminating `\n` in the raw log and exact equality between each
record's `tool` and the invocation tool.

Retain the oracle failure and the CoreAV refusal reasons. This exception
changes only whether those valid CoreAV lines void the arm. It does not
give the pair a timing ratio or a correctness pass, change its outcome,
or add to refused-pair or refusal-group counts. A successful CoreAV
invocation with no log beside a failing oracle keeps its ordinary
oracle-failed handling.

An executed fallback, a malformed or unattributed line, or any other
nonqualifying log event still voids the arm. A valid refusal line beside
such an event cannot hide it. A CoreAV failure without a corresponding
valid refusal record also voids the arm on an oracle-failed pair, even
when CoreAV wrote no log. Infrastructure errors and unreadable logs keep
their existing abort handling.

When the oracle succeeds, this diagnostic exception does not apply. A
log line on a pair that did not qualify under the original refused-pair
rule still voids the arm. The null arm has no oracle side, so its mixed
outcomes keep their existing handling. No diagnostic exception applies
after a pair has qualified for timing; later failures and log events keep
their existing incomplete or void disposition.

The run decides this diagnostic status from the pair's own invocations
and logs. There is no pre-listed set of affected files or commands, and
no removal after inspecting timing samples. The committed reason table
is unchanged; an unknown reason can qualify as a diagnostic under the
same record checks.

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
explained. A pair occupies only one outcome total. For oracle-failed pairs,
publish separately how many carry diagnostic CoreAV refusals and their
logged reasons. Count a diagnostic pair once, and each distinct reason
once per pair, even when log records repeat. A pair with several reasons
can appear under each reason; reason counts are not extra pair outcomes.
These diagnostics do not add to refused-pair or refusal-group totals.
Other raw refusal events from mixed outcomes remain separate from those
totals. In an aborted arm, unprocessed
eligible pairs remain explicitly unprocessed, not refused or timed.
Reports list commands with no timed pair instead of omitting
them. A group with no timing ratio remains unmeasured and cannot pass;
the existing no-measurement and partial-coverage rules still apply.

The public summary places the refusal count immediately beside every
speed number, with its eligible count, timed count and timed share.
The oracle-failed count, the count with diagnostic refusals and their
reason counts appear beside the speed numbers at the same total,
command, class and family levels as the other coverage fields.
Per-command and per-reason-group refused-pair counts remain separate.
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

Beyond the refusal timing set, this plan changes only the specified
oracle-failure diagnostic log exception and its reporting. It does not
rescore historical reports, allow a fresh draw to be opened early, permit
rerunning a missed arm, or waive the existing unmeasured-group and
incomplete-run failures. A speed result with refusals is explicitly a
result for the supported timed subset, with the coverage next to it.

## Prerequisites

1. Merge this docs-only amendment before its separate tools PR and
   before any final run.
2. Keep the merged timer correction in coreav-dev/coreav#122 and original
   refusal tools in coreav-dev/coreav#127. Merge separate tools implementing
   this amendment. The tools must test positive logged
   refusals, failures without a refusal record, crashes, timeouts,
   incorrect outputs, oracle failures and complete coverage accounting.
   Include both side orders of a null-arm refusal paired with success or
   another failure, and reason-table cases for each group, unmatched
   reasons and different groups on the two sides. Assert exclusive
   outcome totals and that refusal-group counts sum to refused pairs.
   Add regressions for a valid diagnostic on an oracle-failed pair;
   repeated and unknown reasons; an executed fallback; malformed,
   unattributed and oracle-side log lines; a valid refusal mixed with
   another log event; a CoreAV failure without a refusal record, including
   no log; and a log on a nonqualifying pair whose oracle succeeded.
   Preserve successful CoreAV with failed oracle, null-arm mixed outcomes
   and late timed failures. Assert no ratio or refused-pair double count,
   nonpassing invalid cases, and diagnostic counts and reasons beside
   speed at every coverage level.
3. After both amendment and tools PRs merge, rehearse main on spent inputs
   and report its verdict and coverage without changing these rules from
   the observed results.
4. Name the exact release-candidate commit after all release-scoped
   prerequisites merge, then follow the existing R1, gate, R4, R2 and R3
   order. No final run is authorized by this document alone.
