# WASM package gate readiness repair

## Scope

Keep one open release blocker per failing gate class and make startup failures attributable to their own process. This changes neither the registered memory bars nor the gate ledger. A crash, timeout or genuine startup failure remains fatal and is not retried or retuned to overturn a result.

## Investigation

The initial investigation compared the available main-run inventory and linked issue runs, retaining job logs outside the repository. The inventory did not support the assumption of repeated main readiness failures. Several linked issues came from PR runs and different failure classes. The accessible readiness failure included output accumulated from earlier workers; a successful example probe did not establish that the failing worker started.

No readiness-detection defect was established. The implementation therefore leaves polling, deadlines and isolate/build validation unchanged. It captures process-specific output and exit status so the next failure can be classified without confusing it with earlier work. A regression uses the pinned workerd's real startup-error output.

## Implementation

Replace accumulated-output startup errors with per-process diagnostics. Retain stdout and stderr on the runner, capture startup exit state before shutdown, and keep public output limited to the worker, reason and exit status.

Make the failure reporter find one bot-owned open tracking issue per failing step independently of the commit. Append the commit and run once. Preserve release-blocking wording and API failure propagation. Report only main pushes and explicit manual verdict runs. PR and merge-group failures already appear on their PR. Reporter jobs run independently, since GitHub can discard an older pending job even when running jobs are not cancelled. After creation, recheck for concurrent matches and record this run on the oldest issue before commenting on and closing this reporter's higher-numbered duplicate. A reporting API failure leaves the new issue visible and fails the reporter. Add behavioral tests and update the applicable agent-rule enforcement rows.

## Verification and after-merge work

Run short regression and workflow checks locally. Run the complete installed-package and memory checks in GitHub Actions. An independent reviewer checks this plan commit before the implementation.

After merge, triage the existing automatically filed WASM package blockers by run type, failure class and later main successes. Close PR and merge-group reports and failures superseded by a passing main commit with a neutral comment pointing to the fix. Consolidate the WebP parity reports into one issue naming that failure. Do not close goal-level tracking issues.
