# MediaInfo oracle: a second, informational black box

Test-only tool for MediaInfo as a second, informational oracle. It runs the pinned MediaInfo command-line binary next to `ffprobe`, maps its
stream-level fields into the vocabulary of the stream oracle
([`tools/oracle/SPEC.md`](../oracle/SPEC.md), level `streams`) and writes a
three-way report: CoreAV, `ffprobe`, MediaInfo.

**It never gates.** The oracle's agreement rates, denominators and thresholds
stay defined against `ffprobe`. A MediaInfo reading never covers a disagreement and is never
on its own a reason to change CoreAV; it only orders triage.

Python 3 standard library only, outside the Cargo workspace. MediaInfo is a
black box: only its binary output and built-in help were used, never its
source.

## Install the pinned binary

`ffprobe` is the stream oracle's pinned 7.0.2 build.

```bash
scripts/install-oracle-mediainfo.sh "$PWD/target/oracle/bin"   # MediaInfo CLI 26.10, SHA-256 pinned
scripts/install-oracle-ffmpeg.sh "$PWD/target/oracle/bin"      # ffprobe 7.0.2
export PATH="$PWD/target/oracle/bin:$PATH"
```

## Fixtures

```bash
cargo build --locked --release -p coreav-oracle -p coreav-isobmff-adapter
scripts/gen-fixtures.sh /tmp/fixtures
scripts/oracle-capture.sh --media /tmp/fixtures --out /tmp/3way/ffprobe --levels streams
scripts/oracle-capture.sh --media /tmp/fixtures --out /tmp/3way/coreav \
  --adapter target/release/isobmff-adapter --levels streams
python3 -I tools/mediainfo-oracle/mediainfo_oracle.py capture --media /tmp/fixtures --out /tmp/3way/mediainfo
python3 -I tools/mediainfo-oracle/mediainfo_oracle.py report --ffprobe /tmp/3way/ffprobe \
  --coreav /tmp/3way/coreav --mediainfo /tmp/3way/mediainfo --out /tmp/3way/report.json --markdown /tmp/3way/report.md
```

The `mediainfo-oracle` workflow does the same on every push to `main`, on
PRs that change this tool, and on demand, and puts the Markdown report in
the job summary. It exits 0 whatever the agreement.

## Corpus v1

```bash
COREAV_ORACLE=$PWD/target/release/coreav-oracle python3 -I tools/mediainfo-oracle/corpus.py run \
  --manifest manifest.json --captures /tmp/mi-corpus/captures --work /tmp/mi-corpus/work \
  --r2-env /path/to/r2.env --aws /path/to/aws --split tuning --jobs 8
python3 -I tools/mediainfo-oracle/corpus.py report --manifest manifest.json \
  --captures /tmp/mi-corpus/captures --split tuning --out report.json --markdown report.md
```

`run` downloads one file at a time per worker, captures all three sides and
deletes the media. `--split tuning` (the default) selects the `edge` files and
the non-holdout `real` files; `--split holdout` selects only holdout files.
The report publishes counts only. Captures stay local.

`run` writes `PROVENANCE.json` in the captures directory: the MediaInfo and
`ffprobe` versions, the SHA-256 of the CoreAV adapter and comparator binaries,
and the tool's commit and file hashes. Rerunning into a directory captured
with other binaries is refused; a changed tool only prints a warning. The
report copies the provenance. Unmapped MediaInfo names are captured as
`mi:<raw>`, and the mapping tables are applied again when the report is
written, so extending a table needs no new capture.

## Normalized MediaInfo record (`coreav-mediainfo/streams/1`)

Capture: `mediainfo --Output=JSON --Full FILE`, no shell, 120 s timeout.

| Key | From MediaInfo |
|---|---|
| `probe_ok` | output is a JSON object whose `General` track has `Format` (MediaInfo exits 0 even when it recognizes nothing) |
| `failure` | `timeout`, `exit-N`, `signal-N`, `invalid-json`, `unrecognized`, or `null` |
| `format_duration_s` | `General.Duration`, decimal seconds as printed |
| `truncated`, `conformance_errors` | `General.extra.IsTruncated == "Yes"`; any `Conformance*` key in a track's `extra` |
| `streams[].index` | `StreamOrder` when it is a plain integer; other tracks go to `unpaired_tracks` |
| `kind` | `Video`/`Image` → `video`, `Audio` → `audio`, `Text` → `subtitle`, `Other` → `data` |
| `codec_name` | `Format` through the codec table (MPEG audio uses `Format_Profile` for the layer) |
| `codec_fourcc` | `CodecID` before the first `-`, when exactly four characters |
| `width`, `height` | `Width`, `Height` |
| `rotation` | `Rotation` (clockwise) → counter-clockwise in `[0, 360)`; video without it → `0`; non-integer → `null` |
| `duration_s`, `source_duration_s` | `Duration`, `Source_Duration` |
| colour fields | `colour_primaries`, `transfer_characteristics`, `matrix_coefficients`, `colour_range` through H.273 |
| `sample_rate`, `channels` | `SamplingRate`, `Channels` |
| `language` | `Language_String3` (ISO 639-2); `und` → `null` |
| aspect ratios | `PixelAspectRatio`, `DisplayAspectRatio` |

A value outside the mapping tables, or a number MediaInfo prints in another
form (for example `2 / 1` channels), is kept as `mi:<raw>` so that it shows up
as a disagreement and in the report's "outside the mapping tables" list.

Not observable from MediaInfo: `timescale`, `handler_name`.

## Comparison

- CoreAV against `ffprobe` uses the stream oracle's own exact rules.
- Against MediaInfo: `pcm_*` codec names compare as `pcm`; matrix
  coefficients 5 and 6 (`bt470bg`, `smpte170m`), which H.273 defines
  identically, compare as one class; `und` languages as `null`, and the
  twenty ISO 639-2 bibliographic codes as their terminology twins (`ger` as
  `deu`); durations
  within 1 ms (MediaInfo prints milliseconds); aspect ratios within 0.0005.
- Classes per field: `all-agree`, `coreav-alone`, `ffprobe-alone`,
  `mediainfo-alone`, `mediainfo-between` (MediaInfo within tolerance of two
  different values), `mediainfo-split` (CoreAV and `ffprobe` agree by the
  stream oracle's exact rule, MediaInfo matches only one of them, for example
  equal duration ticks in different time bases), `all-differ`, `not-comparable` (a side failed or lacks
  the stream).
- The triage table takes each CoreAV and `ffprobe` disagreement on a gating
  field and gives MediaInfo's reading. `Source_Duration`, when MediaInfo
  prints it, is reported as a second reading of stream duration and never
  changes a class.

## Tests

```bash
python3 -m unittest discover -s tools/mediainfo-oracle/tests -p 'test_*.py'
```

`tests/snapshots/` holds MediaInfo 26.10 output on the generated fixtures,
trimmed to the fields the normalizer reads, and the matching `ffprobe` 7.0.2
and CoreAV stream records. No media.

## Results

`results/` holds the reports committed with the tool: the
fixture report, and corpus v1 reports for the development split (edge plus
non-holdout real files) and the holdout split, each with the tool commit it
was run with.
