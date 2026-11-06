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

Keep all 71 protected source files, their registered rosters and the 4,882 historical records unchanged. The sole diagnostic-record rebinding covers `crates/coreav-cli/tests/ffmpeg.rs`: its old diagnostic digest is `14c7db0ca9a7fdd8d349dc807c2293ee6a84198519b1615a537f544ef59f9d5c`; the merged baseline digest is `623f8beac57dbc7afd5ced2bc5be2fceaff691b4bcfbee816c63cfd28cb74e97`. Main changed those bytes through coreav-dev/coreav#435 and coreav-dev/coreav#436.

Re-derive all 190 current Rust rows against their exact source text and position. A moved or changed site remains unknown until classified again; path membership never carries its old verdict forward. Update only this classifier's diagnostic record, not the source, `FROZEN`, judging registrations or any other registered roster. Check that the old digest occurs only in that diagnostic record. Future changes continue to fail exact-digest and exact-site checks. Name the old and new digests and their reason in the pull request body.

## Verification and delivery

1. Land the catalogue representation and freshness tests without migrating consumers. Preserve the trusted 43-location, 50-row historical lower bound when re-expressing unchanged evidence; independently classify changed frozen sites.
2. Migrate shell and Rust consumers. Require wrong-version, missing-sibling, substituted-file and malformed-owner-response controls to fail before output creation. Run affected real-pin tests and the strict oracle-free selection without changing test predicates.
3. In the dependent follow-up, review unknowns by file and evidence class. Publish exact row reconciliation and the reason for every disposition. New bypasses are repaired, not relabelled as qualified; unplaced sites remain failures. Stop for a scope decision if a repair would change protected sources or require an incoherent second implementation track.
4. Prove stale-source, helper-edit, anchor-edit, shadow-module, new-source, extensionless-script, executable-data, symlink, duplicate-entry and fold-origin negative controls. Preserve or strengthen every existing adversarial test.
5. In that follow-up, only after trusted evidence has zero unknown and zero bypass rows, add the classifier to the unfiltered `deny-checks` job under its existing required report. Run it before checkout-writing steps and retain its report outside the checkout. No ruleset change or new check name is needed.

The complete change requires affected tests, final-head CI, exact-head similarity evidence and independent review. Source and generated regression checks are sufficient; do not start media-corpus, performance, remote-infrastructure or additional fuzz campaigns for this migration.

## Report-only migration receipt

The representation-only commit preserves every previously trusted per-file verdict count: 43 bypass locations and 50 rows, 36 qualified locations and 69 rows, and 1,127 excluded locations and 1,320 rows. It does not classify unknowns. Its 5,774 discovered rows exceed the merged baseline's 5,722 by two classifier boundaries and 50 new classifier-regression boundaries. The closed inventory contributes another 290 unresolved file boundaries.

The separate consumer migration re-derives the same 50 bypass rows at their current exact sites, including Rust byte spans, and changes those 43 locations to qualified references. It also re-derives the moved CoreAV-under-test exclusions without changing their counts. The new shell regressions add nine discovered rows. The resulting report accounts for all 6,073 rows: 5,783 discovered rows and 290 closure rows, each mapped exactly once.

Integrating main `ebe4021` adds 12 discovered rows in the input-summary checker and two in its tests. All 14 remain unknown. Its two new Rust files and design note enter the closed inventory without inheriting a verdict. The integrated report maps all 6,087 rows: 5,797 discovered and 290 closure rows. No previously trusted proof becomes stale and no additional site is classified.

The missing-index-file, gitlink, root-normalization and complete workflow-trigger regressions add two more discovered rows, both unknown. After those guard changes, every retained source digest, exact site and Rust span is checked again before rebinding the discovery and classifier anchors. The qualified and excluded counts do not change. The final report maps all 6,089 rows: 5,799 discovered and 290 closure rows.

The Bash 3.2 shell regression adds three discovered rows, all unknown. The probe harness's four qualified rows are re-derived against the guarded array expressions and read loop without changing the 119 qualified rows. Release and AVI trigger coverage now follows all shared qualification inputs; AVI also covers its fixture generators. The final report maps all 6,092 rows: 5,802 discovered and 290 closure rows.

The current catalogue has 79 qualified locations and 119 rows, no confirmed bypass rows, and 4,215 unknown locations in 662 files containing 4,653 rows. Zero confirmed bypasses does not establish that no bypass remains among those unknowns. The original unresolved rows, their moved or changed sites, the new regression boundaries and the unscanned-file boundaries all remain obligations of the dependent follow-up. The classifier exits unsuccessfully for those unknowns; this change does not make it required in CI.
