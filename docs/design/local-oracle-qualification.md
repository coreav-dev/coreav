# Local oracle qualification

Every non-frozen Rust and Python oracle consumer must qualify its selected executable before comparing output or generating fixtures. Executable permissions alone do not prove the shared pin.

## Plan

Use one Python selection and qualification owner, `scripts/test_oracle.py`, which delegates the predicates to the existing platform verifier. A shared Rust source adapter invokes this owner once per test process and retains its result in `OnceLock`. Python serializes initial qualification and retains both success and failure. Selection cannot change within a process. Keep the pin file, version matcher, installer, runtime fallback selection and comparison predicates unchanged.

Require the selected FFmpeg file to resolve to the verified directory's `ffmpeg` file. This prevents a renamed local override from qualifying a different executable in its parent directory. A renamed standalone override without that installed pair is not qualified test evidence.

Migrate every non-frozen real oracle entry point. Missing, incomplete or unverified selections fail; they never become passing skips. Keep mock-only tests explicitly oracle-free. Fixture or backend test selection is separate from oracle availability. Provision the pin before real Python discovery. Platforms without a registered oracle run an explicitly oracle-free selection rather than unsetting CI to permit silent skips.

Test a valid pair, a wrong version, a malformed or failed version command, a missing sibling and a different selected executable beside a valid pair. Check concurrent single initialization, cached failure after repair, recovery in a fresh process and rejection of a second selection. Derive positive fake banners from the shared pin rather than adding another version constant. Test failures must identify the oracle-qualification stage before any fixture output is created. Run the non-frozen suites with system FFmpeg first on PATH and require qualification failures, then require success with the pin.

## Scope

This is oracle qualification only. It does not change runtime fallback, native admission, generated media, command registrations, comparison tolerances or measured coverage. The registered frozen harness and Python driver retain workflow-level verification because their dependency closure is immutable. FROZEN, JUDGING, their registered hashes and all their source files remain unchanged. No media corpus or performance round is needed.
