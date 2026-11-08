# GitHub Shapes 200 PNS-free registration

This registers `github-shapes-200-pns-free`, a separate measurement requested by the maintainer. It uses the [original registration](github-shapes-200.md), with only the fixture change and generation gate below. This document supplies a method, not results. No judged command or judged fixture generation runs before this registration and its tools merge.

## Purpose and unchanged scope

The original registration refuses exact credit for AAC-decoded output whose input contains perceptual noise substitution, or PNS. A separately registered fixture disables that encoder tool without changing CoreAV's AAC decoder or the output predicates. This isolates the fixture's effect on this generated representative. It does not establish decoder conformance or compatibility on other inputs.

The frozen roster, its digest, all 200 shapes and 1,470 repository-shape pairs stay unchanged. Instantiation, value and filter representatives, unresolved construction, denominators, outcome order, weights, exact decoded-output and copied-packet predicates, probe witnesses and replay rules are inherited unchanged. `compatible_aac` and `compatible_avc1` remain separate misses for the exact headline. The original registration and its retained results are not relabeled or replaced.

## Fixture recipe

Only command 0 of `tools/compat-corpus/github_shapes_fixtures.py` changes for the new registration. Insert `-aac_pns 0` immediately after `-c:a aac`:

```text
-f lavfi -i testsrc2=size=64x48:rate=10:duration=2 -f lavfi -i sine=frequency=440:sample_rate=48000:duration=2 -c:v libx264 -pix_fmt yuv420p -c:a aac -aac_pns 0 source.mp4
```

All other settings retain their defaults. The Matroska copy, concat list, PNG sequence and raw BGR24 remain derived from that source by the unchanged commands. The WAV, MP3, raw S16, PNG, JPEG and GIF recipes do not change. The original recipe's command list remains available unchanged under `github-shapes-200`.

## Generation gate and retained receipts

Before any judged command, scan the AAC in both `source.mp4` and `source.mkv` with the existing opt-in `pns_diagnostic` built by:

```sh
cargo build --locked --release -p coreav-aac --features diagnostics --example pns_diagnostic
```

Require a complete `coreav.aac-pns-diagnostic/1` scan under the existing `github_shapes_aac.noise_count` rules. Packet count must equal the frame-list length, every channel must be present, and the number of PNS bands must be zero. Independently require that packet count equals the pinned ffprobe's audio packet count. A container adapter may copy AAC into a diagnostic MP4 only if the complete ordered packet payloads and AAC configuration remain identical to the bound input. It must not decode or re-encode audio.

Any missing or failed scan, PNS band, incomplete channel or packet-count mismatch is `fixture_generation`. It voids the acquisition before any shape runs, rather than counting as an output miss. The bundle retains scan metadata, packet-count evidence, any adapter receipts, input digests, the diagnostic binary digest and its build command. The manifest binds those receipts and the selected registration. Replay independently repeats the scan on the bound fixtures and refuses missing, changed or nonzero-PNS evidence.

The per-command `exact_without_pns` guard stays unchanged. If it fails, that command remains `different`; passing the generation gate does not grant exact output credit.

## Paired acquisition

One main-only workflow dispatch selects both recipes at one clean merged main SHA. It acquires each recipe once in separate evidence directories using the same build and pinned programs. The original recipe is a pairing control, not a retry of an earlier acquisition. The workflow accepts only the paired selection. The CLI selects each registration explicitly, and every bundle and summary names its selected registration so evidence cannot be judged under the other recipe.

The host remains GitHub-hosted `ubuntu-24.04`, Linux x86-64. The build remains `cargo build --release --locked -p coreav-cli --features release-full`. FFmpeg 9.0.2, ffprobe and x264 retain the original pins and qualification. Build identities, dependencies, timeouts, fallback restrictions, program-start rules, accounting and failure handling stay unchanged. Failures remain misses. A replacement acquisition is allowed only for a recorded infrastructure or harness fault, with the failed evidence retained.

The method manifest additionally binds this design note. Tool-file digests change with implementation, as in the original registration's later method versions. Historical bundles remain replayable at their own measured commits, not under substituted method digests.

## Controls before merge

PR CI uses only independent controls, never the judged fixture generator or a command from the frozen roster. Encode a separate pink-noise control with the pinned encoder twice. Default settings must produce PNS bands; `-aac_pns 0` must produce none. Exercise the MP4 and Matroska gate paths. Mutation controls must reject one PNS band, an incomplete scan, missing channels, packet-count mismatch, changed adapter packets or configuration, missing receipts and a substituted registration. Each generation-gate failure must occur before a judged command can start.

## Order and evidence custody

1. Check the premise by counting PNS on the retained generated source from the completed original acquisition. This diagnostic judges no command. Stop if no PNS is found.
2. Commit this plan first, then the tools, workflow selection, generation gate, replay checks and controls in the same change.
3. Pass CI, independent review, exact-head similarity and the independent PNS controls before merge.
4. Dispatch the paired acquisition from main at one merged SHA, then replay each bound bundle.
5. Retain all acquisition evidence and summaries in restricted storage. No acquisition evidence, summaries or per-shape outcomes are uploaded as public workflow artifacts or printed in public logs. Credentials exist only in the workflow's retention step and never reach media child processes.

This change adds no public results page, README text, site text or documentation-index entry. Results are not announced and are not used in outward copy. Any separately retained comparison must name both fixtures and commits. A difference between them is a fixture effect, not a CoreAV improvement, a replacement for the registered coverage number or a claim about other inputs. Outcomes may fall as well as rise because the no-PNS PCM bar is stricter. Lossy encoded output still needs the unchanged exact predicate; removing PNS does not establish bit-identical AAC decode.
