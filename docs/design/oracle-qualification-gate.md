# Oracle qualification gate

## Goal and baseline

Complete the remaining non-frozen oracle migration, account for every discovered invocation, and require qualification evidence in CI. Delivery is split into two dependent changes. This change contains the evidence mechanism and the 43-site migration, with a plan commit before implementation. It remains report-only. Full classification, any remaining bypass repairs and the required gate follow only after this evidence mechanism is independently reviewed and merged. No comparison tolerance, test predicate, pin, installer, runtime fallback, native admission, media input or measured result changes.

The earlier trusted census identified a lower bound of 43 bypass locations in seven files: 35 shell locations and eight Rust locations. The shell files are `scripts/gen-fixtures.sh`, `scripts/gen-fixtures-mkv.sh`, `scripts/gen-fixtures-ts.sh`, `scripts/oracle-ffprobe.sh` and `packages/cli/test/release-clips.sh`. The Rust files are `crates/coreav-cli/tests/ffmpeg_mkv_aac_copy.rs` and `crates/coreav-cli/tests/frame_handoff.rs`.

The merged baseline `f10b07324ef399d82dd7a0f2066c6f60c4b69ea9` discovers 5,722 rows with exact reconciliation. Its global snapshot is invalid: 4,126 locations in 401 files are unknown, and the zero qualified and confirmed counts are invalidated evidence, not migration progress. Re-establish evidence before reporting a new trusted census.

## Qualification ownership

Shell consumers call the existing `scripts/test_oracle.py --resolve` before creating output directories, manifests, scratch directories or clips. A plain command-substitution assignment propagates owner failure through `set -e`; process substitution, an assignment inside `local` or `export`, and error suppression must not hide that failure. Validate the complete two-line response and retain both executable paths. Encoder discovery and all later commands use the retained FFmpeg. Preserve explicit `FFMPEG` and `FFPROBE` overrides in the probe harness by passing them to the owner for qualification.

The two Rust consumers use the existing process-cached adapter and its returned FFmpeg and ffprobe paths. Missing or unverified selections fail rather than returning a passing skip. Keep optional codec and fixture selection separate from oracle availability. Add the real frame-handoff test to the existing explicit oracle-free test selection; that selection is not a registered measurement roster.

Check every caller's deployed source closure, fixture-cache inputs and platform installation. Release and package fixture jobs must provide both programs, including when the executable directory is selected through PATH. Do not change installers or weaken pin verification to accommodate a caller.

## Source-bound catalogue

Replace the global content snapshot with a versioned, per-file exact-site catalogue. A site is identified by its path, line, language kind, scope and complete discovered program expression. Each verdict has a closed evidence class, an explanation and explicit required source dependencies. Source and dependency digests bind the reviewed bytes. Import-resolution witnesses include candidate modules and their absence, so a newly added shadow module invalidates affected evidence even when that new file has no process calls.

The complete repository-file inventory is closed. A new, renamed or otherwise unaccounted-for executable source fails; an unknown suffix, executable data file or outward symbolic link cannot disappear from the inventory. Keep historical measurement exclusions bound to their existing complete path-and-digest record.

Every delegated row has its own evidence. Record and verify its complete discovered source closure, but do not inherit a callee's verdict automatically. Unresolved invocation or import evidence remains unknown. This is a finite, reviewed catalogue, not a general-purpose value-flow proof. Imports, variable names and namespace resemblance do not qualify an executable.

Keep explicit `fold_to` origins for overlapping discovery rows. A target must be a catalogued site in the same file and cannot conceal a more severe verdict. Re-expressing existing evidence must reproduce its per-file row and location counts before any separately explained classification change. A refresh command may propose changed, added and removed sites outside the checkout; CI never consumes that proposal as approved evidence.

Bind the classifier and discovery implementation, qualification owners, pins and historical record as evidence anchors. Stale sources, changed dependencies, missing import witnesses, new sites, stale or duplicate catalogue entries and inconsistent classes fail closed. A harmless edit outside a proof's dependency and import context must not re-approve or invalidate all other files.

## Protected sources

Keep all 71 protected source files, their registered rosters and historical evidence byte-identical to merged main. The initial diagnostic-record rebinding covers `crates/coreav-cli/tests/ffmpeg.rs`: its old diagnostic digest is `14c7db0ca9a7fdd8d349dc807c2293ee6a84198519b1615a537f544ef59f9d5c`; the initial merged baseline digest is `623f8beac57dbc7afd5ced2bc5be2fceaff691b4bcfbee816c63cfd28cb74e97`. Main changed those bytes through coreav-dev/coreav#435 and coreav-dev/coreav#436. The later merged-main diagnostic update is recorded below; it changes no protected source or historical evidence.

Re-derive the initial 190 Rust rows against their exact source text and position. A moved or changed site remains unknown until classified again; path membership never carries its old verdict forward. Update only the classifier's diagnostic records, not the source, `FROZEN`, judging registrations or any other registered roster. Check that the old digest occurs only in the diagnostic record. Future changes continue to fail exact-digest and exact-site checks. Name the old and new digests and their reason in the pull request body.

## Verification and delivery

1. Land the catalogue representation and freshness tests without migrating consumers. Preserve the trusted 43-location, 50-row historical lower bound when re-expressing unchanged evidence; independently classify changed frozen sites.
2. Migrate shell and Rust consumers. Require wrong-version, missing-sibling, substituted-file and malformed-owner-response controls to fail before output creation. Run affected real-pin tests and the strict oracle-free selection without changing test predicates.
3. In the dependent follow-up, review unknowns by file and evidence class. Publish exact row reconciliation and the reason for every disposition. New bypasses are repaired, not relabelled as qualified; unplaced sites remain failures. Stop for a scope decision if a repair would change protected sources or require an incoherent second implementation track.
4. Prove stale-source, helper-edit, anchor-edit, shadow-module, new-source, extensionless-script, executable-data, symlink, duplicate-entry and fold-origin negative controls. Preserve or strengthen every existing adversarial test.
5. In that follow-up, only after trusted evidence has zero unknown and zero bypass rows, add the classifier to the unfiltered `deny-checks` job under its existing required report. Run it before checkout-writing steps and retain its report outside the checkout. No ruleset change or new check name is needed.

The complete change requires affected tests, final-head CI, exact-head similarity evidence and independent review. Source and generated regression checks are sufficient; do not start media-corpus, performance, remote-infrastructure or additional fuzz campaigns for this migration.

## Pre-rebase report-only migration receipt

The representation-only commit preserves every previously trusted per-file verdict count: 43 bypass locations and 50 rows, 36 qualified locations and 69 rows, and 1,127 excluded locations and 1,320 rows. It does not classify unknowns. Its 5,774 discovered rows exceed the merged baseline's 5,722 by two classifier boundaries and 50 new classifier-regression boundaries. The closed inventory contributes another 290 unresolved file boundaries.

The separate consumer migration re-derives the same 50 bypass rows at their current exact sites, including Rust byte spans, and changes those 43 locations to qualified references. It also re-derives the moved CoreAV-under-test exclusions without changing their counts. The new shell regressions add nine discovered rows. The resulting report accounts for all 6,073 rows: 5,783 discovered rows and 290 closure rows, each mapped exactly once.

Integrating main `ebe4021` adds 12 discovered rows in the input-summary checker and two in its tests. All 14 remain unknown. Its two new Rust files and design note enter the closed inventory without inheriting a verdict. The integrated report maps all 6,087 rows: 5,797 discovered and 290 closure rows. No previously trusted proof becomes stale and no additional site is classified.

The missing-index-file, gitlink, root-normalization and complete workflow-trigger regressions add two more discovered rows, both unknown. After those guard changes, every retained source digest, exact site and Rust span is checked again before rebinding the discovery and classifier anchors. The qualified and excluded counts do not change. The final report maps all 6,089 rows: 5,799 discovered and 290 closure rows.

The Bash 3.2 shell regression adds three discovered rows, all unknown. The probe harness's four qualified rows are re-derived against the guarded array expressions and read loop without changing the 119 qualified rows. Release and AVI trigger coverage now follows all shared qualification inputs; AVI also covers its fixture generators. The final report maps all 6,092 rows: 5,802 discovered and 290 closure rows.

The pre-rebase catalogue has 79 qualified locations and 119 rows, no confirmed bypass rows, and 4,215 unknown locations in 662 files containing 4,653 rows. Zero confirmed bypasses does not establish that no bypass remains among those unknowns. The original unresolved rows, their moved or changed sites, the new regression boundaries and the unscanned-file boundaries all remain obligations of the dependent follow-up. The classifier exits unsuccessfully for those unknowns; this change does not make it required in CI.

## Merged-main diagnostic update

The diagnostic records now describe merged main `1cfe2a526262e0e78b3af91ec9f76455e91a08bf`. The consumer migration and checker code are unchanged by this update. Every changed source has a merged-main cause; no source, judging registration, historical document or evidence file is edited here.

The historical snapshot changes exactly seven entries: coreav-dev/coreav#445 changed the existing compatibility README and judging registration, while coreav-dev/coreav#432 changed the frame-handoff measurement document and added its four `phase-b-*` evidence files. The snapshot grows from 4,882 to 4,886 files. Its complete digest is `744203702fb44827d16bd4bcdba64e6f031b2e722c8c5c9edec4b6c853ecf358`. No historical media or measurement is rerun or rejudged.

Fifteen discovered-source paths change against the pre-rebase receipt:

| Source | Previous rows | Current rows | Merged cause |
|---|---:|---:|---|
| `.github/workflows/ci.yml` | 20 | 20 | coreav-dev/coreav#441, coreav-dev/coreav#444 |
| `.github/workflows/deny.yml` | 4 | 4 | coreav-dev/coreav#445 |
| `crates/coreav-cli/tests/ffmpeg.rs` | 190 | 178 | coreav-dev/coreav#437 |
| `crates/coreav-cli/tests/ffmpeg_concat_primed.rs` | 0 | 8 | coreav-dev/coreav#437 |
| `crates/coreav-cli/tests/ffmpeg_pcm_mp4.rs` | 0 | 34 | coreav-dev/coreav#439 |
| `crates/coreav-cli/tests/legacy_encoders.rs` | 2 | 3 | coreav-dev/coreav#441 |
| `crates/coreav-cli/tests/support/concat_presentation.rs` | 0 | 2 | coreav-dev/coreav#437 |
| `crates/coreav-cli/tests/transcode.rs` | 115 | 129 | coreav-dev/coreav#441, coreav-dev/coreav#444 |
| `scripts/compat_sources.py` | 26 | 35 | coreav-dev/coreav#445 |
| `scripts/tests/test_compat_sources.py` | 48 | 74 | coreav-dev/coreav#445 |
| `scripts/tests/test_compat_workflow.py` | 4 | 4 | coreav-dev/coreav#445 |
| `scripts/tests/test_main_ci_plan.py` | 4 | 6 | coreav-dev/coreav#441, coreav-dev/coreav#444 |
| `tools/compat-corpus/decoded_output.py` | 7 | 11 | coreav-dev/coreav#433 |
| `tools/compat-corpus/github_shapes.py` | 27 | 29 | coreav-dev/coreav#433 |
| `tools/compat-corpus/tests/test_github_shapes.py` | 56 | 103 | coreav-dev/coreav#433 |

The closed inventory adds sixteen merged-main files, including the three new discovered Rust paths above. Its other executable sources are `crates/coreav-cli/src/ffmpeg/raw_audio/mp4.rs` from coreav-dev/coreav#439, `crates/coreav-cli/src/ffprobe/avc_summary_tests.rs` from coreav-dev/coreav#442, `crates/coreav-cli/tests/support/sar_delta.rs` from coreav-dev/coreav#441 and `scripts/tests/test_h264_g1_search_reference.py` from coreav-dev/coreav#446. Each contributes zero discovered rows. The nine new design documents and data files add no invocation rows.

The diagnostic protected-source map changes only the digests of `ffmpeg.rs`, `compat_sources.py` and `judging.json`, recording their merged-main bytes. The obsolete 190-row and 26-row exclusion proofs are removed rather than carried forward. All 178 current `ffmpeg.rs` rows and all 35 current `compat_sources.py` rows remain unknown. The source roster still has 71 files.

The existing qualified call at `legacy_encoders.rs:72` is re-derived as the identical `Command::new(test_oracle::require().ffmpeg())`, after coreav-dev/coreav#441 inserted seven bytes in an earlier derive attribute. Its exact program span moves from `2386:2417` to `2393:2424`. The enclosing fixture command, qualification predicate and owner bytes are unchanged; the dependency context now includes `support/sar_delta.rs`. The helper's new `ffmpeg` invocation remains unknown.

The new report has valid evidence and exact reconciliation of 6,229 rows: 5,939 discovered and 290 closure rows. Qualified evidence is unchanged at 79 locations in 25 files and 119 rows. Exclusions decrease to 993 locations in 161 files and 1,104 rows; unknowns increase to 4,470 locations in 667 files and 5,006 rows. The 137 additional discovered rows and withdrawal of 216 stale exclusion rows explain the 353-row increase in unknowns. This is baseline drift, not classification progress. Confirmed bypasses remain zero, and the checker still exits unsuccessfully for unresolved rows.

## Completion plan after the evidence mechanism

coreav-dev/coreav#438 merged as `1d8c263806b31f5ff99d0978fa04c9c02d0a85e0`. This follow-up classifies every row at its exact site, repairs the qualification bypasses it can change, and makes the existing classifier required only once the predicate below holds. The plan is committed before implementation. The reviewed schema-2 mechanism remains the evidence owner; no new general-purpose flow analyzer or automatic verdict inheritance is introduced.

### Baseline and completion predicate

Keep three baselines distinct. The original census has 5,722 discovered rows, including 4,758 unresolved rows. The preceding section describes the pre-merge 6,229-row receipt. The actual merged tree adds `.github/workflows/ci-full.yml` through coreav-dev/coreav#449, so its fresh report has 6,230 rows: 5,939 discovered and 291 closure rows, including 5,007 unknown rows and 119 qualified rows. Evidence is invalid only because the new workflow has no inventory entry. Its sole manual-dispatch command is `:`; admitting the inspected zero-invocation source removes that provisional boundary, not an invocation or a classification obligation.

Completion requires valid source and dependency bindings, exact reconciliation, zero unknown rows and zero confirmed bypass rows at the final tree. Account for all original 4,758 unresolved rows, all moved or rewritten descendants, every subsequently discovered invocation and every unscanned boundary. Zero confirmed bypasses alone is insufficient. Never reduce the discovery set, drop unknowns, weaken a test or comparison, or relabel an actual bypass to satisfy this predicate.

### Runtime inputs that are not available

Seventeen audited rows select runtime inputs whose admitted code or input is not available: eight cache-observer rows, five metric-template rows and four external-checker rows. Retain each as unknown with an explicit input-not-available reason, its exact site and the in-repository selecting sources bound through the existing schema-2 proof. The closed `unavailable-input` evidence class maps only to `unknown`; it grants neither qualification nor exclusion. Source, dependency, anchor, occurrence, span, dead-entry and fold checks remain unchanged. Continue other repairs without treating these missing inputs as evidence.

Report unknown reasons per row after applying the existing location severity. Keep each unknown row's own reason so an explained row cannot hide an unmatched row at the same location. A less severe row folded into an unknown location receives the report-only reason `folded-to-unknown`, which is not an admissible evidence class. The reason counts must sum to the reported unknown-row count. Explained unknowns still fail the classifier, so the required classifier step remains unwired while any unknown or bypass remains. This does not change the completion predicate.

Keep a deterministic row-lineage receipt outside the checkout. Match exact sites first, then exact content identities with occurrences for moved sites. Rewritten or removed sites need explicit old-to-new pairings and a recorded merged-main or repair cause; they cannot use a fuzzy expression or namespace match. Reconcile the original, pre-merge, fresh merged and final row sets separately. Compare base and final discovery on the same final tree to detect accidental census shrinkage. Publish the receipt digest, regeneration command and aggregate dispositions, while CI continues to consume the schema-2 catalogue rather than a second mutable verdict source.

### File-by-file evidence and repairs

Review whole files, grouped by their delegated dependency closure. Read-only classification proposals contain exact sites, occurrences, Rust byte spans, verdicts, evidence classes, explanations and cited source paths. The integrator derives hashes and import witnesses from the actual checkout through `Bindings.context`, never from a proposal's claimed digest. Each delegated row has its own explanation and complete closure. A callee verdict does not automatically qualify its caller.

A qualified verdict requires the executed program value to come from the shared owner's returned pair. Exclusions identify a concrete non-oracle mechanism, a deliberate negative control, a registered immutable source, or inert data. Names, imports, suffixes and historical path membership do not prove qualification. Bound data records include their actual consuming harness or analyzer where applicable; never execute or decode hostile regression inputs during this audit.

Repair each confirmed non-frozen bypass through the existing shared owner, preserving its arguments, comparison predicates and failure behavior. Re-discover and classify the repaired sites rather than moving an old verdict to a new line. Include every new regression boundary in the final audit. Correct the composed-audio fixture cache's missing verifier dependency without changing fixture generation or the audio policy.

Three inward license and notice links in `crates/coreav-py` need explicit link-and-target evidence. Bind their link text and tracked regular target inside the checkout, and include the target in the dependency context. Retargeting or target edits invalidate the proof; outward links, missing targets and unreviewed links remain failures. Unscanned C, module-TypeScript and MDX sources require source inspection rather than a suffix exclusion.

### Development observer qualification

The current tooling copy of `crates/coreav-cli/tests/ffmpeg.rs` differs from the immutable candidate observer. Its `oracle()` helper checks only that two files exist. A generated control compiles the actual helper unchanged: with `CI=1`, an executable pair reporting version `0.0.0` is accepted, while the shared owner rejects that same pair for a pin digest mismatch. Without either required-mode variable, a missing pair returns a passing skip. No media is needed to reproduce either behavior.

Move only this development helper to the existing shared Rust owner. Preserve all callers and comparison assertions. `verify_input_seek_refusal` in the same development copy selects its reference FFmpeg from `COREAV_FFMPEG` or `PATH` on its own; replace only that selection with the path `oracle()` returns, leaving its arguments and assertions unchanged. Add regressions that run the helper with `CI=1` and require rejection of both the fake version-`0.0.0` pair and a missing pair. Bind the helper's owner module within this change; do not copy another verifier or create a separate pin predicate.

The module-graph regression must inspect the registered candidate observer's fixed source graph, not require its unrelated development copy to have the same imports. Retain an adversarial graph-change control against the candidate subject. The candidate commit, its observer Git objects, the `FROZEN` and `OBSERVER` rosters, the registered common module, pins and all judging predicates remain unchanged. This helper repair does not change `judging.json`. Discovery must distinguish development observer copies from registered immutable sources so that their invocations cannot inherit a `registered-source` exclusion.

### Six tooling-consumed frozen paths

Add a separate, non-registered `compat_frozen_check.py` to the existing unfiltered deny workflow. It delegates to the unchanged `compat_sources.py plan` source-identity checks in their non-measurement pull-request mode, without producing artifacts or authorizing a dispatch. That owner already compares the six tooling-consumed paths with the registered candidate: the FFmpeg environment, the two installers, the audio argument file, the command tree and `round_files.py`. Derive this scope from `FROZEN` minus `OBSERVER`, not a second six-item allowlist. Compare complete Git objects, including the command directory's tree. Require a clean tooling checkout and full source history for this check; the separate shallow-bundle verifier remains unchanged.

The two observer paths stay bound to the registered candidate. Changes to their unused tooling copies do not become frozen-input drift, while a substituted candidate object, changed role, missing source or incomplete roster still fails. No candidate, source roster, artifact role, command set or source-manifest schema change is needed.

The six-input guard does not edit `compat_sources.py`, any other registered judging source, `judging.json` or its historical-snapshot digest. The separate entry point reuses the registered owner rather than copying its comparison logic. This supersedes the initial plan to change that owner and regenerate its registration. The nested-probe repair and the three HLS and Matroska repairs below are the only registered-source amendments in this follow-up. No prior result or historical evidence is edited or rejudged; CI never regenerates registration automatically.

### Registered nested probe qualification

`tools/oracle/kd_audit.py` calls the registered ISO overlay directly. Its `declared_size` predicate reaches `_first_frame_size` in `scripts/oracle_kd.py`, which launches a bare `ffprobe` independently of the caller's selected programs. A media-free reproduction compiles that actual Python function from its AST, without rewriting it: with `CI=1`, a PATH stand-in reporting version `0.0.0` returns dimensions `(12, 34)`, and the helper accepts them. The shared owner rejects the same pair for a macOS binary SHA-256 mismatch. The reproduced source has SHA-256 `255f193b6bc6e2f67f32f7fe40bd5f91179ce13c91ce462a6514ecd4b0f50d52`, identical to its current registration. No media is opened or decoded.

Change only this helper's oracle qualification and the required import. Retain the shared owner's complete pair and launch its returned `ffprobe` path. Use the existing platform identity checks, not another verifier. Preserve every probe argument, JSON field, decoded-dimension predicate and valid-oracle return value. An oracle-qualification failure must propagate rather than becoming `None` or a covered divergence. A fresh-process regression requires rejection of the fake version-`0.0.0` pair before its frame JSON can enter the predicate.

Commit the registered code change first. Generate registration explicitly from that immutable code commit in its own following commit, using the existing procedure. Exactly the `scripts/oracle_kd.py` hash may change in `judging.json`; the other 35 hashes and the complete observer registration stay identical. Rebind only that registration file's diagnostic historical-snapshot digest, not any historical result. Keep the candidate, observer objects, frozen rosters, pins, all comparison judgments and published numbers unchanged. Do not rerun or rejudge past inputs.

This repair is withdrawn. The latest Opus speed registration (round 4 in [audio-decode.md](audio-decode.md)) binds its capacity preflight to the exact digests of its judging code, and `bench/audio-decode/capacity.py check-registrations` refuses any change to them. That code includes `scripts/oracle_kd.py`. Changing the file therefore needs a new Opus registration with its own capacity preflight, and rewriting the round-4 binding would misstate the code that preflight ran. `scripts/oracle_kd.py` keeps its registered bytes and `judging.json` keeps its original hash, so `_first_frame_size` still launches a bare `ffprobe`. The catalogue records those rows as a confirmed bypass. CoreAV Codecs owns the follow-up: land the repair above together with the next Opus registration.

### Registered HLS and Matroska oracle qualification

Three more registered judging sources launch bare `ffprobe` or `ffmpeg` names from `PATH`, independently of the shared owner. `scripts/hls_check.py` does so for its stream, packet and composition-time probes, frame hashes, the KD-HLS-8 fragment copy and the KD-HLS-11 skip-sample probes. `scripts/mkv_remux_check.py` does so for its stream probe and packet hashes, and `scripts/oracle_kd_mkv.py` for `_first_frame_size`. Media-free reproductions with `CI=1` and a `PATH` stand-in reporting version `0.0.0` show each source accepting the stand-in's output: HLS stream JSON, Matroska overlay dimensions `(12, 34)`, and remux stream JSON with one packet hash. The shared owner rejects the same pair. The reproduced sources have SHA-256 `629a7bea8045fbc04d115d784dd6ec033163b045e7fe75c5b1c0bc38351f10d5`, `f03b407674c5faa955381f2439bac16f412591041f31326bab241b2f81526b27` and `cf81d3d0b7395219374208f2ae24099c4f155b582190f9988224f77a9066ab9a` respectively, identical to their current registrations. No media is opened or decoded.

The maintainer approved these three amendments on 2026-11-06. Change only each site's program selection and the required `test_oracle` import, launching the paths the shared owner returns. Preserve every argument, parse, predicate and valid-oracle result. A qualification failure must propagate rather than becoming a failed fragment-copy predicate or `None` dimensions; `hls_check.py`'s existing per-file handler reports it as a harness failure. Fresh-process regressions require rejection of the fake pair before any of its output is read.

Commit the registered code change first, then generate registration from that immutable code commit in its own following commit with the existing procedure. Exactly these three hashes may change in `judging.json`; every other hash and the complete observer registration stay identical. Rebind only that registration file's diagnostic historical-snapshot digest. The candidate, observer objects, frozen rosters, pins, comparison judgments and published numbers stay unchanged, and no past input is rerun or rejudged.

The judging roster requires every module a judging source imports to be a judging source itself. Since these repairs, `scripts/hls_check.py`, `scripts/mkv_remux_check.py` and `scripts/oracle_kd_mkv.py` import `scripts/test_oracle.py`, which imports `scripts/oracle_pin_macos.py`; neither is in `JUDGING`, so the existing roster regression fails. The maintainer approved adding exactly these two owner modules to `JUDGING` in `scripts/compat_sources.py` on 2026-11-06. Commit that roster change first, then register from it in its own following commit. The only `judging.json` changes are the two new entries and the new `compat_sources.py` hash, plus the registration file's diagnostic historical-snapshot digest. The roster regression must pass unchanged.

### VideoToolbox tool freeze

The macOS encoder tools reconstruct oracle paths from a directory or run the pin verifier without retaining the owner's pair. Repairing `baseline_smoke.py`, `preparation.py`, `producer.py`, `registration.py`, `smoke.py`, `timing.py` and their tests changes files whose hashes `scripts/encoders/macos/videotoolbox-sources.json` freezes under `tools`. The maintainer approved a forward-only move of those hashes on 2026-11-06. The repair and the new hashes land in one commit, so the freeze always matches the tools beside it. Only the `tools` values of changed files move; inputs, selection, native inputs, commands, bars and the seal validator stay unchanged. Results recorded before the move stay bound to the old hashes and are not rerun or rejudged.

### Codec-coverage oracle owner checkout

The manual codec-coverage jobs check out the measured candidate and run its own scripts. The legacy registered candidate has no `scripts/test_oracle.py`, so its drivers execute the installed paths after only its pin verifier, and a dispatched image candidate cannot be assumed to retain the owner. Those workflow invocations are bypasses on the dispatch path, although pull requests run their own qualified head. Each job that passes `--ffmpeg` or `--ffprobe` to candidate scripts gains one sparse checkout of the five owner files (`test_oracle.py`, `oracle_pin.py`, `oracle_pin_macos.py` and the two pin env files) from the run's own commit. The step moves that checkout outside the candidate tree. Each invoking step resolves the pair with `test_oracle.py --resolve`, following the existing CI pattern, and passes the returned paths. Candidate checkouts, candidate bytes, host records and pin verification stay unchanged. The pin env, installer and verifier files are byte-identical between the legacy candidate and the tooling, so measured results do not move. `test_registered_candidate.py` narrows its checkout guard to allow exactly this owner-only checkout. Regressions refuse a second tooling checkout, extra paths, any other ref, and a `git fetch` route.

The manual GitHub-shape measurement has the same shape: it checks out any merged main commit and runs that commit's driver with the installed paths. Before switching to the candidate, the job copies the same five owner files from its own dispatching main commit outside the tree. The replay step resolves the pair with `test_oracle.py --resolve` and passes the returned paths. The candidate selection, its scripts and the installed programs stay unchanged. Because this reuses the existing checkout, no second checkout or guard change is needed.

### AV1 process-reference oracle pair

Merged main adds the AV1 process-reference driver and its CI smoke check (coreav-dev/coreav#457). The smoke check rebuilds `ffmpeg` and `ffprobe` paths from its `--oracle` directory, and the capture driver runs its `--oracle` and `--probe` arguments directly, so neither passes through the shared owner. The decode-only diagnostic also accepts CoreAV's raw output when it equals the output of the separately built FFmpeg comparator, which makes that unqualified build the correctness reference. Route both scripts through `test_oracle.require`. The smoke check qualifies its directory's pair once and uses the returned paths. The capture driver qualifies `--oracle` and `--probe` as one pair before any call, and uses the returned paths for every probe and PSNR measurement. The comparator built by `build_av1_reference_ffmpeg.sh` stays the timed FFmpeg arm, because the pinned oracle has no SVT-AV1 encoder. The qualified pair judges its encoded outputs. In the decode diagnostic, CoreAV's raw stream and the comparator's raw stream must each equal the qualified oracle's raw stream before any timed call. The recorded results in `bench/encoders/av1-process-reference` are not rerun or rejudged, and `summarize` is unchanged.

### HEVC reference oracle pair

Merged main adds the decoder-only HEVC reference driver (coreav-dev/coreav#447). It calls `test_oracle.require` on its `--oracle-bin` pair but discards the result. It then rebuilds `ffmpeg` from that directory for input preparation, pixel verification and the timed FFmpeg arms, and derives `ffprobe` as a sibling of that path. A verify-only call and a rebuilt path are not proof that the qualified pair ran. Keep the single `test_oracle.require` call before any input fetch, and pass its returned pair to `inputs`, `measure` and `verify`. Every preparation, probe, framemd5 and timed oracle call then uses the returned `ffmpeg` or `ffprobe`. The arms, thresholds, inputs and summary are unchanged, and no reference run is started.

### Composed audio oracle verification

The composed Opus-to-AAC check (`scripts/composed_audio.py`) receives the pair that `transcode_check.py` or `composed_audio_selftest.py` qualified through `test_oracle.require`. Its `verify_oracle` then resolves both paths and runs `oracle_pin.py --verify-bin` on the directory of the resolved `ffmpeg`. That verifier executes `<directory>/ffmpeg` and `<directory>/ffprobe`, which are siblings rebuilt from a name and not necessarily the returned programs: an owner pair installed as symlinks to `decoder` and `probe` resolves to those targets, and the verifier then runs a different file. Replace the sibling check and the verifier child with `test_oracle.require(args.ffmpeg, args.ffprobe)`, and store both returned paths back in `args`, so every later probe, decode and encode call in the check uses them. `oracle_pin.py` and `transcode_check.py` are unchanged. Because `composed_audio.py` is part of the composed check's judging-source digest, every verdict produced after this change carries a new `judging_sha256`. The change is forward-only. Past verdicts and receipts are not rerun, rejudged or relabeled, and the registration JSON, its version and its `registration_sha256` are unchanged.

### Implementation order and verification

1. Commit this plan. Capture the invalid fresh merged baseline and the original-row obligations without claiming classification progress.
2. Commit this narrowed guard plan before its implementation. Add the non-registered entry point and regressions, reusing the existing role-aware source-identity owner without changing its bytes or registration.
3. Add observer-copy and inward-link handling with freshness controls. Repair the demonstrated development helper and preserve candidate-graph protection. Complete the file-by-file audit, all other repairs and the original-row lineage receipt.
4. Require the existing classifier in unfiltered `deny-checks`, after its pinned parser is installed and before checkout-writing tests or generators. Write the report outside the checkout and retain it with failure diagnostics. Preserve the existing required report and same-tree reuse policy; add no ruleset, required-context or measurement-job change.
5. Re-derive the complete final catalogue after every code, test, registration and workflow input is settled. The real command must report valid evidence, exact reconciliation, unknown zero and bypass zero. Verify the independent row receipt and unchanged protected material before publishing the result.

Negative controls cover each of the six tooling paths, absence and additional command-tree files, permitted development observer differences, rejected candidate-object substitutions, inward-link retargeting and target edits, outward links, changed source and relay dependencies, import shadows, new zero-invocation sources, stale anchors, duplicate or dead evidence, and unknown/bypass exit failure. Execute the workflow step with controlled success and failure to prove that neither an unfiltered trigger nor a failed classifier can be bypassed. Preserve every existing adversarial test.

Use the pinned `@ast-grep/cli@0.50.0`, Python and Ruby discovery tools. Source audit, media-free controls and implementation run on the native development host. Real Linux-only oracle or process controls belong in the existing Linux CI jobs. Do not start corpus, held-out, performance, remote-infrastructure or additional fuzz campaigns. Final-head CI, canonical similarity evidence and independent review must cover the completed change.

## Merged-main classification update

Classification merges main ten times, integrating `73e073d1df8eebd4eb50d64589d7a96249fb7a7e`, `51a6e5891a1dae2c00eea20b197ff3ab07431452`, `7c1f23473321a2dfd330870a874562e8b6479fbe`, `60c2d5775f44404c40d62d2a3a6e790bd5f159d3`, `9df0b933d858adace8351fca877b91bc317163f8`, `1ed0568430d4f7e554271425b9357cf6c630ddcd`, `b95d327e00c7eede892757c94a8a4329b1109908`, `5ef3072e12107befffd9536c6f02885e60b54bae`, `28c36a0e6edb3f676343e962aec96d64b8fa6874` and `eeeb4bbc46b045e54f5c7ffd6ff70a6eab5cd861`. Each merge re-derives every row whose source or cited dependency moved. A verdict is carried only when a check shows that every line it cites is byte-identical, as for rows that cite this note above an appended section. The historical snapshot rebinds five entries changed or added by coreav-dev/coreav#440 and coreav-dev/coreav#443 and grows to 4,889 files. No historical media or measurement is rerun or rejudged.

The final catalogue reconciles all 6,567 rows exactly: 6,277 discovered and 290 closure rows. It has 2,068 qualified rows at 1,626 locations, 4,459 excluded rows at 4,187 locations, 23 confirmed-bypass rows at 23 locations in 6 files, and 17 unknown rows, all `unavailable-input`. Nineteen bypass rows lead to the bare `ffprobe` in `_first_frame_size`, whose repair is withdrawn above: the chain inside `scripts/oracle_kd.py` and its callers in `tools/oracle/kd_audit.py`, `tools/stage3-report/corpus.py` and `tools/stage3-report/run.py`. The other four relay into the frozen `tools/compat-corpus/run.py`, which launches `ffprobe` by name. `tools/compat-corpus/corpus_kd.py` (strict rejection) and `tools/compat-corpus/current_ci.py` (the reporter) put the qualified directory first on `PATH` and check that lookup, but a checked lookup is not the returned program carried to execution, and passing that program needs a change to the protected reporter. The bypass and unknown rows still fail the classifier, so the required classifier step stays unwired.

The row-lineage receipt from the fresh merged base `1d8c26380` accounts for its 6,230 rows: 5,007 unknown, 119 qualified and 1,104 excluded. On the final tree, 4,209 rows keep their exact site, 527 move, 1,494 are rewritten or removed and 1,831 are new. Every rewritten, removed or new row has a cause: a commit since the base that changed its file; for the delegated `composed_audio.verify_oracle` row in `scripts/transcode_check.py`, the commit that changed that callee so it no longer starts a process; or, for the provisional `ci-full.yml` boundary, its admission to the reviewed inventory. Of the 5,007 base unknown rows, 1,320 are now qualified, 2,285 excluded, 11 confirmed bypasses and 3 remain unavailable-input unknowns; the other 1,388 were rewritten or removed. The remaining 14 unknown rows are new rows of the same reviewed kind. Running the base discovery code and the current discovery code on the final tree yields the same 6,277 row identities, so the census did not shrink.

Against the pre-merge catalogue's 6,377 rows, these paths have new, rewritten or removed rows. Rows in `.github/workflows/ci.yml`, `.github/workflows/h264-encoder.yml`, `crates/coreav-transcode/src/audio.rs`, `scripts/notices/main.mjs` and `scripts/tests/test_hevc_workflow.py` only move.

| Source | Pre-merge rows | Final rows | Cause |
|---|---:|---:|---|
| `.github/workflows/github-shapes.yml` | 1 | 4 | coreav-dev/coreav#450; GitHub-shape owner pair |
| `.github/workflows/hevc-conformance.yml` | 1 | 4 | coreav-dev/coreav#429, coreav-dev/coreav#453, coreav-dev/coreav#455 |
| `crates/coreav-cli/tests/ffmpeg.rs` | 178 | 178 | coreav-dev/coreav#440 |
| `crates/coreav-cli/tests/ffmpeg_input_seek.rs` | 18 | 30 | coreav-dev/coreav#440; input-seek audio probe |
| `crates/coreav-cli/tests/mp3_worker.rs` | 0 | 1 | coreav-dev/coreav#451 |
| `scripts/aac_pcm_eligibility.py` | 41 | 41 | coreav-dev/coreav#450 |
| `scripts/av1_measurement_smoke.py` | 0 | 7 | coreav-dev/coreav#457; AV1 reference pair |
| `scripts/av1_process_reference.py` | 0 | 19 | coreav-dev/coreav#457; AV1 reference pair |
| `scripts/build_av1_reference_ffmpeg.sh` | 0 | 1 | coreav-dev/coreav#457 |
| `scripts/composed_audio.py` | 13 | 11 | composed audio owner pair |
| `scripts/encoders/macos/test_hevc_smoke.py` | 6 | 6 | macOS test import order |
| `scripts/h264_g1_simd_identity.py` | 0 | 2 | coreav-dev/coreav#454 |
| `scripts/hevc_conformance.py` | 21 | 22 | coreav-dev/coreav#429 |
| `scripts/hevc_decode.py` | 0 | 16 | coreav-dev/coreav#429, coreav-dev/coreav#464; HEVC owner pair |
| `scripts/hevc_reference.py` | 0 | 22 | coreav-dev/coreav#447; HEVC reference pair |
| `scripts/hevc_smoke.py` | 0 | 5 | coreav-dev/coreav#453; HEVC owner pair |
| `scripts/mp3_worker_check.py` | 0 | 8 | coreav-dev/coreav#451 |
| `scripts/oracle_kd.py` | 24 | 24 | withdrawn nested probe repair |
| `scripts/tests/test_av1_measurement.py` | 0 | 3 | coreav-dev/coreav#457; AV1 reference pair |
| `scripts/tests/test_bench_guard.py` | 12 | 13 | coreav-dev/coreav#440 |
| `scripts/tests/test_composed_audio.py` | 17 | 17 | composed audio owner pair |
| `scripts/tests/test_h264_counts.py` | 1 | 3 | explicit probe selects its own pair |
| `scripts/tests/test_hevc_decode.py` | 0 | 2 | coreav-dev/coreav#429; HEVC owner pair |
| `scripts/tests/test_oracle_kd_qualification.py` | 5 | 0 | withdrawn nested probe repair |
| `scripts/tests/test_reference_hevc.py` | 0 | 16 | coreav-dev/coreav#447; HEVC reference pair |
| `scripts/tests/test_shell_oracle_qualification.py` | 12 | 12 | composed fixture cache key |
| `scripts/tests/test_test_oracle.py` | 32 | 33 | remux checker pair |
| `scripts/tests/test_thumbnail_oracle.py` | 8 | 9 | thumbnail probe pair |
| `scripts/thumbnail_oracle.py` | 52 | 52 | thumbnail probe pair |
| `scripts/transcode_check.py` | 111 | 110 | composed audio owner pair |
| `scripts/vp9_filter_coverage.py` | 12 | 12 | owner bound to the capture sources |
| `tools/compat-corpus/copy_check.py` | 28 | 39 | coreav-dev/coreav#440 |
| `tools/compat-corpus/github_shapes.py` | 29 | 32 | coreav-dev/coreav#450 |
| `tools/compat-corpus/github_shapes_aac.py` | 0 | 4 | coreav-dev/coreav#450; AAC shape owner pair |
| `tools/compat-corpus/tests/test_copy_check.py` | 3 | 22 | coreav-dev/coreav#440 |
| `tools/compat-corpus/tests/test_encode_breadth.py` | 107 | 107 | owner cache isolation in one test |
| `tools/compat-corpus/tests/test_github_shapes.py` | 103 | 105 | coreav-dev/coreav#450; GitHub-shape owner pair |
| `tools/compat-corpus/tests/test_github_shapes_aac.py` | 0 | 33 | coreav-dev/coreav#450; AAC shape owner pair |
