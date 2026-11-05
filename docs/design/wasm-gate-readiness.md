# WASM package gate readiness repair

## Scope

Restore trustworthy installed-package gate startup detection and keep one open release blocker for repeated failures on main. This changes neither the registered memory bars nor the gate ledger. A crash, timeout or genuine startup failure remains fatal and is not retried or retuned to overturn a result.

## Investigation

Compare several failing main runs and retain their job logs outside the repository. Separate runtime launch, HTTP readiness and response validation. A successful example probe in accumulated logs does not by itself establish that the current worker started. If the failure is a real worker startup defect, stop before implementing a detection change.

Reproduce the surviving detection mechanism with real output from the pinned workerd runtime. Add a focused regression that fails before the repair, alongside refusal cases for genuine startup failures. Keep the existing startup deadline and isolate/build validation.

## Implementation

Fix the first incorrect detection decision, without changing worker behavior or acceptance policy. Test readiness through the production polling owner rather than an independent predicate.

Make the failure reporter find one bot-owned open tracking issue independently of the failing commit. Append the commit and run to that issue. Preserve release-blocking wording, fork safety and API failure propagation. Restrict automatic reporting to main, and prevent concurrent reporters from creating separate tracking issues. Add behavioral tests and update the applicable agent-rule enforcement rows.

## Verification and after-merge work

Run short regression and workflow checks locally. Run the complete installed-package and memory checks in GitHub Actions. An independent reviewer checks this plan commit before the implementation.

After merge, require a green main gate run before commenting on and closing duplicate automatically filed WASM package blockers. Point each comment to the fix and retain at most one open package-gate tracking issue. Do not close the goal-level tracking issues numbered 22 through 48.
