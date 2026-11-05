# GitHub command-shape coverage

CoreAV counts **46 of 200 shapes (23.0%)** in the registered public-GitHub sample. Their repository weights total **485 of 1,470 selected repository-shape pairs (33.0%)**. These are observed results on fixed generated representatives, not worldwide usage estimates or a claim about every value and input that a shape can represent. There is no coverage bar or release verdict.

The measurement ran once on 2026-11-04 at main commit `7fe3c6c67fadc5826e26b4602cadfaf0e9d285fa`, after the [registration and tools](../design/github-shapes-200.md) merged. The [workflow](https://github.com/coreav-dev/coreav/actions/runs/41794034132) completed both acquisition and full evidence replay successfully. It was not repeated to improve coverage.

## Population and scope

The frozen set contains 151 ffmpeg and 49 ffprobe shapes from the 2026-11-03 public-code presence sample. The original sample has 1,818 repositories, 1,981 normalized shapes and 3,338 repository-shape pairs. Each repository contributes at most one vote to a shape. The selected 200 account for 1,470/3,338 pairs (44.0383%). A repository can contribute to several shapes, so these are not counts of distinct users, repositories or executions.

The counted weight is 485/1,470 = 32.9932% of selected pairs. It is also 485/3,338 = 14.5297% of all sample pairs, the registered whole-sample lower bound under this shape-coverage definition. Unselected shapes were not tested; that lower bound is not a bound on real-world execution compatibility.

| Tool | Counted shapes | Counted selected pairs |
|---|---:|---:|
| ffmpeg | 2/151 | 14/922 |
| ffprobe | 44/49 | 471/548 |
| Total | 46/200 | 485/1,470 |

The two counted ffmpeg shapes copy audio without video or video without audio. The result does not establish passing lossy encodes. An unknown input container uses the registered MP4 representative, abstract values use fixed representatives where possible, and unresolved construction remains a miss. All 200 shapes and their weights remain in the denominator.

GitHub search ranking, capped first pages, selected languages and paths, excluded source directories, dynamic construction and unobserved private code limit inference. The sample measures code presence, not invocation frequency, and supplies no population confidence interval.

## Outcomes

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

The 42 uninstantiable shapes comprise 11 lost constructions, 12 lost values, 12 lost filters, three missing values and four external-input cases. They were not removed after observation. A reference failure, refusal, missing predicate or differing result remains a miss rather than an exclusion.

The external-start rule is conservative. Five of the nine `unexpected_program` shapes invoked FFmpeg's AAC encoder-help query, and four invoked its MP3 encoder-help query. The MP3 paths also made Matroska-intermediate encoder invocations outside the registered allowlist. Those starts do not satisfy the registration's encoder-only invocation rules, so the shapes remain misses. They are not evidence that whole-command fallback ran. No whole-command fallback executed.

`different` records a failed registered equality check, not an inferred codec defect. `no_oracle` records missing applicability or an unregistered output predicate. No known-divergence allowance, quality threshold or N/A result changed a miss into a pass.

## Build, reference and host

CoreAV used `cargo build --release --locked -p coreav-cli --features release-full` on Linux x86-64. This enables `cli,net,dav1d,libvpx,libopus,native-h264,aac`, without the FFmpeg decode-backend feature. Linked versions were dav1d 1.4.1, libvpx 1.14.0 and libopus 1.4. Operator-installed x264 and narrowly classified encoder-only FFmpeg starts were permitted by the registration; this is not an end-to-end-native coverage claim.

The reference was the pinned FFmpeg 9.0.2 build `n9.0.2-17-g2a571b6068-20260930`, with the x264 identity from `scripts/oracle-ffmpeg.env`. The [manifest](github-shapes-200/manifest.json) retains program and linked-library SHA-256 values, the exact x264 commit, build command, source identities and generated-input inventory. The CoreAV binary SHA-256 is `e080e15ff427e61ef11b7e0416bf9a15aedad41b0ba7ae12d0c3c235d1d74cdf`.

The host was a GitHub-hosted Ubuntu 24.04 x86-64 runner, image `20261026.354.1`, reporting four logical CPUs and an Intel Xeon Platinum 8370C at 2.80 GHz. Its kernel was `7.0.0-1017-azure`. No speed measurement or fixed-host performance claim follows from this run.

## Equality and validation

Both primary commands must exit successfully. Probe stdout must be nonempty and byte-identical in the requested writer. A separate, bound pinned-oracle JSON witness establishes substantive requested data for the same selection. Broad sections may contain unavailable optional fields; explicitly requested unavailable fields or empty sections cannot count. The witness never replaces comparison of the original serialized output.

Media outputs require the registered exact decoded samples, pixels, format fields and rational timing. Copied streams additionally require equal packet payloads and timing. No lossy quality tolerance substitutes for exact equality. Encoder provenance and fallback rules apply independently of output equality.

The registered job replayed every outcome before writing the summary. An independent bundle audit then verified the complete file inventory, merged method identities, build record, pinned reference hashes, fixture copies, invocation and fallback receipts, every probe judgment and all weighted arithmetic. Full media replay ran on the capture host; it was not repeated on another host. The bundle records binary identities but does not bundle the programs or shared libraries, so another full replay requires matching programs and libraries.

The public evidence was committed before these coverage claims were written:

- [Summary and all 200 outcomes](github-shapes-200/summary.json), SHA-256 `fe69dcb0070aca781d3ec07538f3c6bf869d37495f834e5f5eb715a5d5e4d99b`.
- [Per-command receipts](github-shapes-200/records.json), including unresolved reasons and both primary outcomes.
- [Manifest](github-shapes-200/manifest.json), [build record](github-shapes-200/build.json), [host record](github-shapes-200/host.json) and [validation record](github-shapes-200/validation.json).
- [Immutable frozen shapes and weights](https://github.com/coreav-dev/coreav/blob/7fe3c6c67fadc5826e26b4602cadfaf0e9d285fa/tools/compat-corpus/github-shapes-200/shapes.jsonl), SHA-256 `0ca6bcb9b983ade0625f795bd5098cffa12ee638d5ddf94a37c1cefb782d9045`.

The downloadable workflow artifact contains generated media, outputs and logs for replay; the maintainer retains the complete bundle. No sampled repository source excerpts or repository URLs were added. Held-out media, existing judging records, bars and tolerances are unchanged.
