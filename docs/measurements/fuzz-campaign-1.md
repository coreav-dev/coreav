# Fuzz campaign 1: bug-finding pass

Plan: [`docs/design/fuzz-campaign.md`](../design/fuzz-campaign.md). Data:
[`fuzz-campaign-1.json`](fuzz-campaign-1.json) (`campaign.py report`). This
pass ran the plan on the code as it stood, to find defects before the gate
run on the release commit. It is not the gate.

- **Code:** `main` at 0bca9b9; campaign tool at 975cd71 (the fuzz targets
  of `main`, with the `coreav-wasm` fuzz lock file brought up to date).
- **Targets:** 17, 5 shards of 30 minutes each, 85 shards, 42.5 CPU-hours
  planned.
- **Seeds:** 62 MP4, MOV and M4A fixtures, 39 Matroska and WebM fixtures,
  186 hostile-suite files.
- **Completed:** 84 of 85 shards ran. `ops` shard 3 got no result because
  the launching shell ended. The 78 shards that ended without a finding
  total 39.05 CPU-hours.

## Findings

| Issue | Target, shard | Kind | Cause | Fix |
|---|---|---|---|---|
| Change A | `coreav-wasm` `ops`, 2 | crash (harness assertion) | An I/O error while reading the iTunes gapless item read as "no item", so extraction succeeded with a different edit list. | Change B: the error propagates. |
| Change A | `coreav-wasm` `ops`, 0, 1, 4 | hang | A `trun` of over a billion empty samples runs to `max_sample_work` (about 3.7 s in release); the harness used the library defaults, unlike the other targets. | Change B: the harness bounds sample work and table bytes. |
| Change A | `coreav-policy` `check`, 1 | hang | The same kind of input in the frame-size scan. The scan's work budget was per track, so many video tracks multiplied the bound. | Change B: one budget per file; the harness bounds its work. |

`coreav-text` `sample` shard 2 also reported a timeout, but its artifact was
`ops` shard 0's input, byte for byte. The cloud host had run both shards in one
container, and the shard function used fixed directories. The tool now
gives each shard fresh directories. That report is not a finding of the
`sample` target, and its shard's 30 minutes are not counted.

## Per target

CPU-hours count completed shards without a finding.

| Target | Shards | CPU-hours | Executions | Coverage (`cov`) |
|---|---|---|---|---|
| `coreav-isobmff` `frame_sizes` | 5 | 2.50 | 48,252,447 | 4,610 |
| `coreav-isobmff` `packets` | 5 | 2.50 | 8,585,946 | 3,038 |
| `coreav-isobmff` `probe` | 5 | 2.50 | 105,182,055 | 3,683 |
| `coreav-isobmff-write` `clip` | 5 | 2.50 | 22,605,913 | 5,302 |
| `coreav-isobmff-write` `extract_audio` | 5 | 2.50 | 57,086,944 | 4,951 |
| `coreav-isobmff-write` `mkv_remux` | 5 | 2.50 | 42,781,480 | 7,401 |
| `coreav-isobmff-write` `remux` | 5 | 2.50 | 27,515,359 | 5,874 |
| `coreav-isobmff-write` `sanitize` | 5 | 2.50 | 25,775,609 | 6,503 |
| `coreav-isobmff-write` `split` | 5 | 2.50 | 27,377,763 | 5,520 |
| `coreav-mkv` `block` | 5 | 2.50 | 2,680,531,737 | 219 |
| `coreav-mkv` `packets` | 5 | 2.50 | 117,923,777 | 4,296 |
| `coreav-mkv` `probe` | 5 | 2.50 | 153,765,931 | 4,048 |
| `coreav-mkv` `scan` | 5 | 2.50 | 104,781,454 | 4,672 |
| `coreav-policy` `check` | 4 | 2.00 | 9,103,048 | 9,634 |
| `coreav-text` `sample` | 4 | 2.00 | 444,377,175 | 467 |
| `coreav-text` `subtitles` | 5 | 2.50 | 80,198,801 | 4,457 |
| `coreav-wasm` `ops` | 0 | 0 | 0 | 0 |

## Corpus

The shards' new units, minimized per target with libFuzzer's `-merge=1`
together with the seeds, are kept with the finding inputs in the private corpus bucket at
`fuzz/corpus-0bca9b9/fuzz-corpus.tar.zst`. The archive is
32,908,749 bytes, SHA-256
`50c8cdffbb9140acf79cd57a46eaa54513ea53484f42312afe61e1cd4935a8ef`. It
holds 42,688 units, about 650 MiB before compression. The gate run seeds
from it: `campaign.py prepare --corpus DIR/corpus --corpus DIR/findings`
after extracting it to `DIR`.
