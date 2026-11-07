# GitHub command-shape coverage

CoreAV counts **54 of 200 shapes (27.0%)** in the registered public-GitHub sample. Their repository weights total **528 of 1,470 selected repository-shape pairs (35.9%)**. A further 14 shapes (136 pairs) are `compatible_aac`: qualified AAC decode-tolerance output, reported separately and never added to the exact count. No shape is `compatible_avc1`. These are observed results on fixed generated representatives, not worldwide usage estimates or a claim about every value and input that a shape can represent. There is no coverage bar or release verdict.

The measurement has run twice under the [registration](../design/github-shapes-200.md). The first run, on 2026-11-04 at main commit `7fe3c6c67fadc5826e26b4602cadfaf0e9d285fa`, used the method as first registered; it counted 46 shapes and 485 pairs, its evidence is in [github-shapes-200/](github-shapes-200/) and its record is kept below. The second run, on 2026-11-06 at main commit `521a0d396c670294bf79a674c42aedbee1776db3`, is the current result. It used the later method registered on 2026-11-06 before execution, which adds the separate `compatible_avc1` and `compatible_aac` outcomes and the AAC exact-credit guard; the two manifests therefore bind different method file digests, and the first run is not rejudged under the later method. The exact predicate, the frozen set, the fixture recipes and the first run's evidence files are the same in both runs, and this report changes no registered method. Its [workflow](https://github.com/coreav-dev/coreav/actions/runs/42200130332) completed both acquisition and full evidence replay. Neither run was repeated to improve coverage.

## Population and scope

The frozen set contains 151 ffmpeg and 49 ffprobe shapes from the 2026-11-03 public-code presence sample. The original sample has 1,818 repositories, 1,981 normalized shapes and 3,338 repository-shape pairs. Each repository contributes at most one vote to a shape. The selected 200 account for 1,470/3,338 pairs (44.0383%). A repository can contribute to several shapes, so these are not counts of distinct users, repositories or executions.

The counted weight is 528/1,470 = 35.9184% of selected pairs. It is also 528/3,338 = 15.8179% of all sample pairs, the registered whole-sample lower bound under this shape-coverage definition. Unselected shapes were not tested; that lower bound is not a bound on real-world execution compatibility.

| Tool | Counted shapes | Counted selected pairs | `compatible_aac` shapes | `compatible_aac` pairs |
|---|---:|---:|---:|---:|
| ffmpeg | 10/151 | 57/922 | 14 | 136 |
| ffprobe | 44/49 | 471/548 | 0 | 0 |
| Total | 54/200 | 528/1,470 | 14 | 136 |

Eight of the ten counted ffmpeg shapes copy streams, two of them as input-seek clips. The other two encode H.264 with x264 at CRF 23, one with a literal crop and one as a plain video-only encode; their decoded pixels and timing equal the same-command reference's under the registered exact predicate. That is equality against the pinned reference with the same encoder settings, not lossless encoding, and it does not extend to any other encoder, setting or input. The compatible AAC shapes decode one AAC track to raw or WAV PCM within the separately registered tolerance; they establish numerical compatibility, not exact output. An unknown input container uses the registered MP4 representative, abstract values use fixed representatives where possible, and unresolved construction remains a miss. All 200 shapes and their weights remain in the denominator.

GitHub search ranking, capped first pages, selected languages and paths, excluded source directories, dynamic construction and unobserved private code limit inference. The sample measures code presence, not invocation frequency, and supplies no population confidence interval.

## Outcomes

| Outcome | Shapes | Selected pairs |
|---|---:|---:|
| Counted | 54 | 528 |
| `compatible_aac` (a miss for the exact headline) | 14 | 136 |
| `compatible_avc1` (a miss for the exact headline) | 0 | 0 |
| `uninstantiable` | 42 | 250 |
| `reference_failed` | 4 | 11 |
| `timeout` | 0 | 0 |
| `refused` | 64 | 293 |
| `failed` | 0 | 0 |
| `unexpected_program` | 5 | 64 |
| `no_oracle` | 3 | 71 |
| `different` | 14 | 117 |
| Total | 200 | 1,470 |

The 42 uninstantiable shapes are the same 42 as in the first run: 11 lost constructions, 12 lost values, 12 lost filters, three missing values and four external-input cases. They were not removed after observation. A reference failure, refusal, missing predicate or differing result remains a miss rather than an exclusion.

The external-start rule is conservative. All five `unexpected_program` shapes invoked FFmpeg's AAC encoder-help query, which is not an encoder-only invocation under the registration. Those starts do not satisfy the registration's rules, so the shapes remain misses. They are not evidence that whole-command fallback ran. No whole-command fallback executed in any of the 200 shapes.

`different` records a failed registered equality check, not an inferred codec defect. `no_oracle` records missing applicability or an unregistered output predicate. No known-divergence allowance, quality threshold or N/A result changed a miss into a pass.

## Changes since the first run

Exact coverage rose by 8 shapes and 43 pairs. Every shape that was exact in the first run is exact in the second. The other 170 shapes kept their outcome. The table names the merged changes whose documented scope covers each moved command; the attribution follows the retained command receipts and those changes' descriptions. No per-commit acquisition or bisection was run, so a single change is not shown to be sufficient on its own.

| Ranks | First run | Second run | Pairs | Associated merged changes |
|---|---|---|---:|---|
| 21 | `refused` | counted | 13 | coreav-dev/coreav#444 (literal crop), coreav-dev/coreav#441 (declared square pixels) |
| 35, 75, 93 | `different` | counted | 17 | coreav-dev/coreav#383 (copied header durations), coreav-dev/coreav#400 (packet order) |
| 81 | `different` | counted | 4 | coreav-dev/coreav#387 (first-input maps), coreav-dev/coreav#400 |
| 94, 172 | `different` | counted | 6 | coreav-dev/coreav#440 (audio-bearing input seek) |
| 145 | `different` | counted | 3 | coreav-dev/coreav#441 |
| 5, 13, 43, 53, 67, 89, 168 | `refused` | `compatible_aac` | 101 | coreav-dev/coreav#405 (AAC PCM qualification), coreav-dev/coreav#420 (PNS qualification), coreav-dev/coreav#450 (separate category) |
| 33, 57, 72, 77, 98, 139, 174 | `refused` | `compatible_aac` | 35 | the same three and coreav-dev/coreav#431 (WAV encoder metadata) |
| 23, 25, 55, 125 | `unexpected_program` | `different` | 34 | coreav-dev/coreav#451 (encoder-only MP3 starts) |
| 15, 188 | `refused` | `different` | 21 | coreav-dev/coreav#405, coreav-dev/coreav#420; pipe input is outside the compatible scope |
| 44 | `refused` | `different` | 7 | coreav-dev/coreav#439 (audio-only PCM in MP4); PCM in MP4 is outside the compatible scope |
| 3 | `refused` | `no_oracle` | 67 | coreav-dev/coreav#329 (native bare-probe summary); the summary is on standard error and the registered probe predicate reads standard output |

The four MP3 shapes now pass the external-start rule and fail the exact decoded comparison. Their source audio is AAC with noise substitution, which the registration never credits as exact for a non-copy output, and no MP3 tolerance is registered. This result does not isolate an MP3 encoder defect.

## Build, reference and host

CoreAV used `cargo build --release --locked -p coreav-cli --features release-full` on Linux x86-64. This enables `cli,net,dav1d,libvpx,libopus,native-h264,aac`, without the FFmpeg decode-backend feature. Linked versions were dav1d 1.4.1, libvpx 1.14.0 and libopus 1.4. Operator-installed x264 and narrowly classified encoder-only FFmpeg starts were permitted by the registration; this is not an end-to-end-native coverage claim. The AAC compatibility scan used the opt-in `pns_diagnostic` example of `coreav-aac` built at the same commit.

The reference was the pinned FFmpeg 9.0.2 build `n9.0.2-17-g2a571b6068-20260930`, with the x264 identity from `scripts/oracle-ffmpeg.env`. The [manifest](github-shapes-200-2026-11-06/manifest.json) retains program and linked-library SHA-256 values, the exact x264 commit, the diagnostic build command, build command, source identities and generated-input inventory. The CoreAV binary SHA-256 is `4894f5ae5ea0e2cb453b2d55addbcfeefb39b2433090b5701bdb0d56771dbdea`.

The host was a GitHub-hosted Ubuntu 24.04 x86-64 runner, image `20261101.360.1`, reporting four logical CPUs and an Intel Xeon Platinum 8573C. Its kernel was `7.0.0-1017-azure`. No speed measurement or fixed-host performance claim follows from this run.

## Equality and validation

Both primary commands must exit successfully. Probe stdout must be nonempty and byte-identical in the requested writer. A separate, bound pinned-oracle JSON witness establishes substantive requested data for the same selection. Broad sections may contain unavailable optional fields; explicitly requested unavailable fields or empty sections cannot count. The witness never replaces comparison of the original serialized output.

Media outputs require the registered exact decoded samples, pixels, format fields and rational timing. Copied streams additionally require equal packet payloads and timing. No lossy quality tolerance substitutes for exact equality. Encoder provenance and fallback rules apply independently of output equality. `compatible_aac` requires the complete registered AAC qualification, with each measured payload equal to its own decoder's qualified bytes and every byte outside the PCM payload exact; `compatible_avc1` requires byte-identical samples after removal of redundant parameter sets. Both are reported beside, never inside, the exact count.

The registered job replayed every outcome before writing the summary. An independent audit then verified the downloaded artifact's digest against GitHub's record, the complete file inventory, every method file against the measured checkout, the build and source binding, all 200 receipts and regenerated commands, staged fixture copies, fallback and program-start records, every probe judgment and all weighted arithmetic, without running any media command. Full media replay ran on the capture host; it was not repeated on another host. The bundle records binary identities but does not bundle the programs or shared libraries, so another full replay requires matching programs and libraries.

The public evidence enters in the same commit as these coverage claims:

- [Summary and all 200 outcomes](github-shapes-200-2026-11-06/summary.json), SHA-256 `3b076bb9972053c291acb76c04040d2bada7d6abe1b7efaca824374b65ca415f`.
- [Per-command receipts](github-shapes-200-2026-11-06/records.json), including unresolved reasons and both primary outcomes.
- [Manifest](github-shapes-200-2026-11-06/manifest.json), [build record](github-shapes-200-2026-11-06/build.json), [host record](github-shapes-200-2026-11-06/host.json) and [validation record](github-shapes-200-2026-11-06/validation.json).
- [Immutable frozen shapes and weights](https://github.com/coreav-dev/coreav/blob/521a0d396c670294bf79a674c42aedbee1776db3/tools/compat-corpus/github-shapes-200/shapes.jsonl), SHA-256 `0ca6bcb9b983ade0625f795bd5098cffa12ee638d5ddf94a37c1cefb782d9045`.

The downloadable workflow artifact (SHA-256 `fd24199f8df0c24f743e2217c80527ce4591a734aa3330b92119ab2e65797ddd`, 40,350,865 bytes) contains generated media, outputs and logs for replay; a digest-matched complete copy is retained outside the repository. No sampled repository source excerpts or repository URLs were added. Held-out media, existing judging records, bars and tolerances are unchanged.

## First run, 2026-11-04

The first run counted **46 of 200 shapes (23.0%)** and **485 of 1,470 selected pairs (33.0%)**, or 485/3,338 = 14.5297% of all sample pairs. It ran once at main commit `7fe3c6c67fadc5826e26b4602cadfaf0e9d285fa` after the registration and tools merged, before the separate compatibility categories existed, so its outcomes have no compatible classes. Its [workflow](https://github.com/coreav-dev/coreav/actions/runs/41794034132) completed both acquisition and full evidence replay. Its evidence files are unchanged and keep their original method binding; they are not rejudged under the later categories.

| Tool | Counted shapes | Counted selected pairs |
|---|---:|---:|
| ffmpeg | 2/151 | 14/922 |
| ffprobe | 44/49 | 471/548 |
| Total | 46/200 | 485/1,470 |

| Outcome | Shapes | Selected pairs |
|---|---:|---:|
| Counted | 46 | 485 |
| `uninstantiable` | 42 | 250 |
| `reference_failed` | 4 | 11 |
| `timeout` | 0 | 0 |
| `refused` | 83 | 537 |
| `failed` | 0 | 0 |
| `unexpected_program` | 9 | 98 |
| `no_oracle` | 2 | 4 |
| `different` | 14 | 85 |
| Total | 200 | 1,470 |

The two counted ffmpeg shapes copied audio without video or video without audio. Five of the nine `unexpected_program` shapes invoked FFmpeg's AAC encoder-help query, and four invoked its MP3 encoder-help query with Matroska-intermediate encoder invocations outside the registered allowlist. No whole-command fallback executed.

The first run's CoreAV build used the same command and features, with dav1d 1.4.1, libvpx 1.14.0 and libopus 1.4, binary SHA-256 `e080e15ff427e61ef11b7e0416bf9a15aedad41b0ba7ae12d0c3c235d1d74cdf`, the same pinned FFmpeg build and x264 identity, on a GitHub-hosted Ubuntu 24.04 x86-64 runner, image `20261026.354.1`, four logical CPUs, Intel Xeon Platinum 8370C at 2.80 GHz, kernel `7.0.0-1017-azure`. Its evidence:

- [Summary and all 200 outcomes](github-shapes-200/summary.json), SHA-256 `fe69dcb0070aca781d3ec07538f3c6bf869d37495f834e5f5eb715a5d5e4d99b`.
- [Per-command receipts](github-shapes-200/records.json), [manifest](github-shapes-200/manifest.json), [build record](github-shapes-200/build.json), [host record](github-shapes-200/host.json) and [validation record](github-shapes-200/validation.json).
