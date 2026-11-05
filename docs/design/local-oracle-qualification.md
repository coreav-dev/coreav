# Local oracle qualification

Every non-frozen Rust and Python oracle consumer must qualify its selected executable before comparing output or generating fixtures. Executable permissions alone do not prove the shared pin.

## Plan

Use one Python selection and qualification owner, `scripts/test_oracle.py`, which delegates the predicates to the existing platform verifier. A shared Rust source adapter invokes this owner once per test process and retains its result in `OnceLock`. Python serializes initial qualification and retains both success and failure. Selection cannot change within a process. Keep the pin file, version matcher, installer, runtime fallback selection and comparison predicates unchanged.

Require the selected FFmpeg file to resolve to the verified directory's `ffmpeg` file. This prevents a renamed local override from qualifying a different executable in its parent directory. A renamed standalone override without that installed pair is not qualified test evidence.

Migrate every non-frozen real oracle entry point. Missing, incomplete or unverified selections fail; they never become passing skips. Keep mock-only tests explicitly oracle-free. Fixture or backend test selection is separate from oracle availability. Provision the pin before real Python discovery. Platforms without a registered oracle run an explicitly oracle-free selection rather than unsetting CI to permit silent skips.

Test a valid pair, a wrong version, a malformed or failed version command, a missing sibling and a different selected executable beside a valid pair. Check concurrent single initialization, cached failure after repair, recovery in a fresh process and rejection of a second selection. Derive positive fake banners from the shared pin rather than adding another version constant. Test failures must identify the oracle-qualification stage before any fixture output is created. Run the non-frozen suites with system FFmpeg first on PATH and require qualification failures, then require success with the pin.

## Scope

This is oracle qualification only. It does not change runtime fallback, native admission, generated media, command registrations, comparison tolerances or measured coverage. The registered frozen harness and Python driver retain workflow-level verification because their dependency closure is immutable. FROZEN, JUDGING, their registered hashes and all their source files remain unchanged. No media corpus or performance round is needed.

On arm64, FFmpeg 9.0.2's default SIMD F32→S16 path rounds half-LSB ties away from even, so the conversion reference test pins the scalar path to preserve CoreAV's ties-to-even contract; registered measurements run on x86-64.

## Implemented boundaries

Migrated callers use the common owner before fixture generation or comparison. Literal-only generated fixtures and mocked subprocess tests stay oracle-free; optional fixture and non-FFmpeg backend selection remains separate from oracle availability. The Linux CI job provisions its pair before any real backend test, and Python discovery provisions it before real fixture consumers. Registered macOS arm64 tests provision the existing macOS pair; other platform selections name and exclude their real oracle-dependent tests explicitly instead of treating a missing executable as a passing comparison.

The owner retains both success and failure for the process, rejects a changed selection and delegates all version predicates to the existing platform verifier. The Rust adapter retains that result once per test binary. Remote source bundles retain the normal repository-relative `scripts` layout, both pin files, the common owner and its transitive local imports. Their binary selection and comparison arguments do not change. Bundle checks run with synthetic banner-only programs; no remote media or performance run is required.

Registered measurement drivers remain exceptions: B4's `b4_round10.TOOL_FILES`, VP9's `CAPTURE_SCRIPTS`, audio-decode's `JUDGING_INPUTS`, and the registered H.264 PGO diagnostic and acquisition sources keep their existing pin, provenance and workflow verification. Their code and registered source rosters are unchanged. Adding an owner dependency to those drivers would change a registered source closure.

## Invocation discovery plan

Discover process boundaries and oracle command construction across the tracked source tree, including separate tests, generated programs, qualified Rust constructor names, shell command substitutions and workflow run blocks. Do not use an owner import or an executable's variable name as a qualification proof. Retain opaque boundaries until their actual caller and selected executable have been traced.

Historical measurement records under `docs/measurements/` retain their exact bytes because they record what earlier runs executed. Record the complete path set and SHA-256 digests outside that boundary. Check the current files against that record before excluding historical source from invocation discovery. A missing, added, renamed or edited file fails; a directory prefix alone never grants an exception. Keep the separately registered protected source files and their rosters unchanged.

Classify each remaining discovery hit as a confirmed bypass, already routed through qualification, or a false positive, with the actual boundary and the evidence for its disposition. Keep unknown hits failing rather than counting them as bypasses or qualified calls. Bind reviewed dispositions to their source and qualification dependencies so a new invocation or changed proof requires review. Cover explicit executable identity, early dispatch, source bundles and relocated production adapters separately from local source-pattern matches.

Report the confirmed-bypass count before changing any consumers. If the remaining migration cannot stay coherent with the existing deployment and dispatch repairs, split only after a scope decision. Run only short local source and generated regression checks during discovery; no media corpus, performance or remote infrastructure run is required.

The classifier matches concrete call patterns against source-bound dispositions rather than inferring qualification from a general-purpose value-flow model. Each disposition identifies its source locations, executable expression, verdict and evidence; the repository input snapshot and qualification dependencies must still match their recorded digests. A changed helper, added file, shadowed binding or executable override therefore invalidates the prior disposition instead of inheriting it. Unmatched locations remain unknown and fail. Reports count source locations, folding overlapping constructor, completion and delegated rows at the same location; they do not count runtime calls or claim an exhaustive bypass total while unknown locations remain. Generated reports stay outside the checkout so they cannot alter the input snapshot.

This census is report-only, not a required check. Remaining confirmed bypasses and unclassified sites prevent a total qualification claim. The current repair batch covers the AVI selected-pair paths, transcode selection dispatch, the TS command bundle and the relocated decode adapter. It also moves the standalone FLAC test's qualification to the shared platform owner because the merged main's Linux-only verifier rejected the registered macOS pair in CI. The remaining shell and Rust migration and unknown-site review are follow-up work. A required check is deferred until unknown sites have been resolved.
