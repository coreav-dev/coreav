# 0.1.0 codec coverage: V1 to V3

The combined V1–V3 run is void under the registration because 42 V1 files have no final result. V1 is incomplete; V2's observed count is below its threshold, but no registered V2 verdict follows from this void run. V3 reports a descriptive share without a bar. This is the judged run of release candidate `4062842fc42babef9e6661a37bbb18f7e375a80c`, not the tools rehearsal. Workflow success does not establish run validity or a release verdict.

| Measurement | Result | Registered bar | Verdict |
|---|---|---|---|
| V1, whole-stream decode agreement | 835 / 1,142 reference-bearing files in the completed portion (73.12%); 42 files unresolved | At least 74% of the final denominator, every frame agreeing | Incomplete; no pass or miss verdict |
| V2, encode-set command coverage | 24 / 488 (4.92%) | At least 196 / 488 (40% or more) | Observed shortfall; no registered verdict because the combined run is void |
| V3, filter-free command coverage | 24 / 630 (3.81%) | None | Reported, not a pass/fail bar |

## Registration and evidence

The [registered plan](compat-0.1.0/codec-coverage-plan.md) fixes the inputs, stream selection, reference commands, counting rules, time limits and recovery host. The [candidate registration](compat-0.1.0/README.md#the-commit) preceded the run. The [Actions execution](https://github.com/coreav-dev/coreav/actions/runs/41372781795) ran on 2026-11-02, dispatched from main `c569a2dfebbcf5c29247adb1d01fe0047641ceee`, with the explicit candidate above and `set=v1`. That set runs V2 and V3 as well as V1; no second dispatch was made.

The [manifest](compat-0.1.0/codec-coverage/manifest.json) records the candidate, binary hashes, build features, static-library versions, five fixture hashes, evidence-key fingerprint and all hosts. Its `problems` list is empty. That validates the build record, not V1 completion: its shard counts explicitly retain pending files.

The [run-wide validity rule](../design/compat-cli.md#when-they-run-and-what-voids-a-run) voids the combined run when any V1 file has no final result and makes only a nonvoid run final for its commit. The V2/V3 job's own summary has no local void reasons and records `met=false` for V2; those fields describe that job's observations, not a registered verdict for the combined run. They are retained unchanged as evidence.

| Item | Recorded identity |
|---|---|
| CoreAV | Full archive build, `release-full`, profile `dist`, target `x86_64-unknown-linux-musl` |
| Decode oracle | Same candidate, profile `dist`, features `native-h264,dav1d,libvpx`, same musl target |
| Static libraries | dav1d 1.5.3, libvpx 1.14.1, libopus 1.6.1 |
| FFmpeg and ffprobe | `n9.0.2-17-g2a571b6068-20260930`, the candidate's shared pin |
| x264 | `0480cb05fa188d37ae87e8f4fd8f1aea3711f7ee` |
| Hosts | GitHub-hosted Ubuntu 24.04 x86-64, image `20261026.354.1`, four logical CPUs and about 16 GB memory; kernel `7.0.0-1017-azure` |
| CPU models across V1 shards | AMD EPYC 7763, 9V45 and 9V74; Intel Xeon Platinum 8573C |
| V2/V3 host CPU | AMD EPYC 7763 |

These are correctness and coverage measurements, not timing comparisons. Shared runner CPU variation does not support a speed claim. The reference and CoreAV receive the plan's empty-gconv environment; V1 uses strict-safe decode. The reference's no-conversion pin and whole-stream frame agreement rule are unchanged.

Public evidence beside this page:

- [V1 count summary](compat-0.1.0/codec-coverage/v1-summary.json), replayed with the candidate's unchanged summarizer; exit status 2, `void=true`, `meets_bar=null`.
- [V2/V3 summary](compat-0.1.0/codec-coverage/v2-v3-summary.json) and [all 630 command records](compat-0.1.0/codec-coverage/v2-v3.jsonl), including fixture assignments, refusals and encoder starts. The summary replays from these records without media.
- [Build and host manifest](compat-0.1.0/codec-coverage/manifest.json). Program and fixture hashes in the V2/V3 summary agree with it.

V1's per-file identities and frame records remain private and sealed in the run artifacts. All 40 shards have records for their exact roster slices, totaling 1,351 unique files, and their sealed metadata counts agree with their public counts and the manifest. No file was removed from the registered roster.

## V1: incomplete decode breadth

Of the 1,351 registered files, 1,309 have completed results. The reference excludes 159 with `no_stream` and eight with `reference_failed` (seven AV1 and one H.264), leaving 1,142 reference-bearing files in the completed portion. CoreAV counts on 835 of those files, comparing 8,852,465 frames. The remaining 42 files failed during fetch, before a reference or CoreAV decode result. They are host faults, not decoder timeouts or successful decodes.

The registered recovery was not performed. The frozen summarizer consequently reports `no_final_result` for all 42 and leaves the bar verdict unset. Its 73.12% share describes only the completed portion; it is neither a final V1 miss nor a pass.

### What the unresolved files could change

Let `k` of the 42 unresolved files acquire a reference and let `p` of those count. Then `0 ≤ p ≤ k ≤ 42`, and the final share would be `(835 + p) / (1,142 + k)`. Reaching 74% requires `100p ≥ 1,008 + 74k`, or `p ≥ ceil(10.08 + 0.74k)`.

| Unresolved files entering the denominator (`k`) | Counted files required among them (`p`) |
|---|---|
| 0 to 38 | Impossible, even if every one counts |
| 39 | 39 |
| 40 | 40 |
| 41 | 41 |
| 42 | 42 |

Across every possible completion, the share ranges from 835/1,184 (70.52%) to 877/1,184 (74.07%). Thus the missing results could still change the bar outcome, but only if at least 39 enter the denominator and every one of those counts. This is an arithmetic bound, not an estimate of their outcomes or a verdict.

### Completed portion by codec

| Group | Reference-bearing files | Counted | Share |
|---|---:|---:|---:|
| H.264 | 692 | 631 | 91.18% |
| AV1 | 82 | 82 | 100% |
| VP9 | 73 | 72 | 98.63% |
| VP8 | 50 | 50 | 100% |
| Other codecs | 245 | 0 | 0% |
| Total | 1,142 | 835 | 73.12% |

These rows exclude the 42 unresolved files and the 167 without a reference. They do not redefine V1's denominator.

| Group | Miss code | Files | Sub-codes |
|---|---|---:|---|
| H.264 | `not_read` | 17 | `other` 16, `rejected` 1 |
| H.264 | `out_of_scope` | 37 | `other` 25, `10bit` 10, `chroma` 2 |
| H.264 | `md5` | 6 | Equal frame counts, differing samples |
| H.264 | `decode_error` | 1 | `invalid_data` 1 |
| VP9 | `decode_error` | 1 | `invalid_data` 1 |
| Other codecs | `codec_not_in_build` | 139 | HEVC 116, ProRes 23 |
| Other codecs | `not_read` | 106 | `other` 74, `container` 27, `rejected` 5 |

All 307 misses retain the first applicable frozen code. There are no completed `frame_count` or `timeout` misses. Out-of-scope H.264 remains in the denominator; no post-run divergence or file-specific reference adjustment changes a count. The JSON summary retains the full other-codec listing and no-reference breakdown.

## V2: observed encode coverage shortfall

The reference tries the five registered fixtures in the plan's assignment order for each of the 488 media commands. A counted command must run through CoreAV, agree on output stream kinds and codecs, and pass every registered copy or encoder oracle. An encoder start is allowed; FFmpeg decoding or whole-command fallback is not.

Only 24 commands count, all with MP4 output: 22 copy-only commands and two x264 commands. The 464 misses are 171 `no_input` and 293 `refused`. `no_input` means no fixture assignment succeeded in the reference; it remains a miss in the fixed denominator, not an exclusion. No command has a timeout miss, a later oracle failure or a missing record. The summary has no fallback, tool fault or void reason.

| Mapping group | Commands | Counted | Misses |
|---|---:|---:|---|
| x264 | 72 | 2 | 70 refused |
| libopus | 6 | 0 | 6 refused |
| AAC | 83 | 0 | 83 refused |
| Other encoders | 127 | 0 | 127 refused |
| Copy only | 104 | 22 | 82 refused |

Mapping groups overlap when a command encodes several streams, and commands without a successful reference assignment have no mapping group. These rows are not an additive partition of 488. The [summary](compat-0.1.0/codec-coverage/v2-v3-summary.json) gives every output-container count and every refused option. The most frequent refusals are a non-MP4/M4A output container (39), `-r` (20), several inputs (19), a selected stream not covered by a copy option (16), and `-ar` (11). No encoder-support claim follows from a command that was refused.

## V3: coverage share, without a bar

The full 630-command set includes V2's media commands and 142 commands of other classes. It yields the same 24 counted commands: 24/630 (3.81%). There are 301 `no_input` and 305 `refused` misses, with no missing command or tool fault.

| Class | Commands | Counted |
|---|---:|---:|
| Media | 488 | 24 |
| Unknown | 93 | 0 |
| Other | 29 | 0 |
| Decode-only | 12 | 0 |
| Subtitles | 8 | 0 |

For comparison, the static classifier labels 79 commands native; 22 of those count in this run. A classifier label is not observed oracle agreement and does not replace the 24/630 measurement. V3 has no registered threshold, so this is not a failed or passed bar.

## Limits of the result

V1 remains incomplete because the registered host-fault recovery was not executed; no substitute host, reduced roster or changed deadline was used. Those missing final results void the combined run under the registration, so V2's observed 24/488 shortfall against 196 required is not a final registered verdict. V3 remains descriptive. No recovery or duplicate run was started after reading the results. The measurements do not change the registered candidate, acceptance rules or release scope, and do not authorize a release.
