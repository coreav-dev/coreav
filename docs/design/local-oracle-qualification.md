# Local oracle qualification

Every non-frozen Rust and Python oracle consumer must qualify its selected executable before comparing output or generating fixtures. Executable permissions alone do not prove the shared pin.

## Plan

Use one Python selection and qualification owner, `scripts/test_oracle.py`, which delegates the predicates to the existing platform verifier. A shared Rust source adapter invokes this owner once per test process and retains its result in `OnceLock`. Python serializes initial qualification and retains both success and failure. Selection cannot change within a process. Keep the pin file, version matcher, installer, runtime fallback selection and comparison predicates unchanged.

Require the selected FFmpeg file to resolve to the verified directory's `ffmpeg` file. This prevents a renamed local override from qualifying a different executable in its parent directory. A renamed standalone override without that installed pair is not qualified test evidence.

Migrate every non-frozen real oracle entry point. Missing, incomplete or unverified selections fail; they never become passing skips. Keep mock-only tests explicitly oracle-free. Fixture or backend test selection is separate from oracle availability. Provision the pin before real Python discovery. Platforms without a registered oracle run an explicitly oracle-free selection rather than unsetting CI to permit silent skips.

Test a valid pair, a wrong version, a malformed or failed version command, a missing sibling and a different selected executable beside a valid pair. Check concurrent single initialization, cached failure after repair, recovery in a fresh process and rejection of a second selection. Derive positive fake banners from the shared pin rather than adding another version constant. Test failures must identify the oracle-qualification stage before any fixture output is created. Run the non-frozen suites with system FFmpeg first on PATH and require qualification failures, then require success with the pin.

## Scope

This is oracle qualification only. It does not change runtime fallback, native admission, generated media, command registrations, comparison tolerances or measured coverage. The registered frozen harness and Python driver retain workflow-level verification because their dependency closure is immutable. FROZEN, JUDGING, their registered hashes and all their source files remain unchanged. No media corpus or performance round is needed.

## Implemented boundaries

Real callers use the common owner before fixture generation or comparison. Literal-only generated fixtures and mocked subprocess tests stay oracle-free; optional fixture and non-FFmpeg backend selection remains separate from oracle availability. The Linux CI job provisions its pair before any real backend test, and Python discovery provisions it before real fixture consumers. Registered macOS arm64 tests provision the existing macOS pair; other platform selections name and exclude their real oracle-dependent tests explicitly instead of treating a missing executable as a passing comparison.

The owner retains both success and failure for the process, rejects a changed selection and delegates all version predicates to the existing platform verifier. The Rust adapter retains that result once per test binary. Remote source bundles retain the normal repository-relative `scripts` layout, both pin files, the common owner and its transitive local imports. Their binary selection and comparison arguments do not change. Bundle checks run with synthetic banner-only programs; no remote media or performance run is required.

Registered measurement drivers remain exceptions: B4's `b4_round10.TOOL_FILES`, VP9's `CAPTURE_SCRIPTS`, audio-decode's `JUDGING_INPUTS`, and the registered H.264 PGO diagnostic and acquisition sources keep their existing pin, provenance and workflow verification. Their code and registered source rosters are unchanged. Adding an owner dependency to those drivers would change a registered source closure.
