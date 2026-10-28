# Known-divergence audit

Status: Accepted. Data: [`kd-audit.json`](kd-audit.json). The question,
for each entry of [`docs/known-divergences.md`](../known-divergences.md):
can it cover a real CoreAV bug? The answer has to come from attacks, not
from rereading the predicates. Each attack injects a bug-shaped change
into CoreAV's captures of a file, then replays the comparison and the
overlay. Every such change must stay visible.

## Method

`tools/oracle/kd_audit.py` applies each mutation alone to the candidate
documents of one file. It runs on every file, including those that agree
directly, which a change could pull into an entry.

The mutations, on streams:
- `duration_ts` ±1 or +1000;
- one more channel, a doubled sample rate, a wider picture;
- another codec or profile, or a stream of another codec named `mov_text`;
- the media header's duration reported as the stream's;
- the stream left out, or listed as dropped (under its own code or a
  disallowed one);
- `format_duration_us` +1 or +1000;
- the movie header duration +7;
- a valid file rejected;
- a simple edit list called unsupported, with the stream left out of
  CoreAV's span;
- a parsing bug that moves a value together with CoreAV's own evidence for
  it: the media header, `stts` sum, media edit duration, first `tfdt` or
  `sidx` end with `duration_ts`, the media edit in `edits` and
  `edit_entries`, the presentation offset with every timestamp, or the
  media edit left unreported with the media header's duration.

On packets, at the first, middle and last packet of each stream:
- `pts` or `dts` +1, alone or with the media time behind it;
- `size` +1;
- one offset made 512 ticks more negative, with its media time;
- the keyframe flag flipped;
- every `pts` +1, every `pts` and `dts` +1, or every timestamp and media
  time +1;
- a packet lost, or listed twice;
- discarded samples invented past the last one.

The verdicts compare both the overlay's counts and the comparison's gating
differences:
- **caught:** the change adds an uncovered item.
- **covered:** nothing new is uncovered, yet an entry covers more than
  before, or the comparison shows a difference it did not show before (an
  entry absorbed it). This is a loophole.
- **unlisted:** the change only removed differences, including items the
  reference does not list (see "What the comparison cannot see").
- **agrees:** the change only removed differences.
- **no effect:** neither the comparison nor the overlay changed.

Inputs: 26 stage 3 corpus files, the smallest that each entry covers (with
full downloads for the entries only large files exercise), plus one
crafted file: fragmented H.264 written by `ffmpeg`, with negative `trun`
offsets. The 91 generated fixtures run the same audit in CI.

## Results

1,317 mutations over the 27 files:

| Overlay | Caught | No effect | Unlisted | Covered |
|---|---|---|---|---|
| Before: `main` at d7d79c2 | 1,142 | 86 | 4 | **85** |
| After: this change | 1,272 | 45 | 0 | **0** |

Over the 91 fixtures, after this change: 5,084 caught, 418 no effect, 5
unlisted, none covered. With the two mutations added after the KD-TEXT-END
audit (a stream of another codec named `mov_text`, and the media header's
duration reported as the stream's), on `main` at 1666715: 5,267 caught,
418 no effect, 5 unlisted, 1 agrees, none covered. CI runs this audit on every change (`fixtures.yml`,
"Audit the known divergences").

Per entry: what it covers, the holes found (with how many of the 85
covered mutations each accounts for), and the fix. The last two columns
count the stage 3 files (streams level) and packets the entry covers, on
the 9589d6e baseline with an earlier change. They are the same before and after this
change.

| Entry | Covers | Holes found | Fix | Files | Packets |
|---|---|---|---|---|---|
| KD-STRICT | rejection of a defective file | none: a valid file rejected is caught | — | 6 | 61,580 |
| KD-STTS-SHORT | `stts` sum below the declared duration | from the review: a media edit CoreAV failed to report, or misread together with the duration, removing or moving the duration rule's cap by the media edit (the mutations count these under KD-EDIT-DURATION) | media header, `stts` sum, media edit duration and the list's shape checked | 34 | — |
| KD-EDIT-DURATION | `ffprobe`'s edit-list duration rule | a misread media edit duration moved with `duration_ts` (5) | stored edit entries and the media edit duration checked | 12 | — |
| KD-FRAG-START | duration from decode time 0 | before change A's review: any CoreAV value below the `sidx` end | Change A: CoreAV's value is its sample sum; here: first `tfdt` and `sidx` end checked | 12 | — |
| KD-PCM-END | PCM end at the edit or the movie header | before change A's review: any CoreAV duration; here: a misread media header moved with the duration (1) | Change A: the declared duration rule's value; here: headers checked | 3 | — |
| KD-EDIT-UNPLACED | a stream outside the edit model, left out of CoreAV's span | a simple list called unsupported (4) | the stored list must be outside the model | 1 | — |
| KD-EDIT-DISCARD | samples outside the edit, discarded | their `pts` and `dts`, which the reference cannot check, and invented samples (18) | timestamps must be the edit's mapping of the media times; samples checked in both packet units | — | 764 |
| KD-CTTS | `dts` shifted by the smallest signed offset | none | samples checked; shift on every packet | — | 328,998 |
| KD-CTTS-UNSIGNED | ISO version 0 offsets read signed | before change A's review: a QuickTime file read unsigned | Change A: brand checked; here: samples checked, shift on every packet | — | 33,933 |
| KD-EDIT-SNAP | video edit start moved to the next composition time | every timestamp and media time +1 (9) | samples checked; shift on every packet | — | 16,220 |
| KD-TRUN-SHIFT | `pts` plus minus the smallest `trun` offset | one packet given a negative offset (2) | samples checked; shift on every packet | — | 36,000 |
| KD-EDIT-MULTI | several edits applied exactly | none (the edit list is already read from the file) | samples checked | — | 500,044 |
| KD-PCM | `ffprobe`'s PCM grouping | samples invented past the end, in the reference packet unit (from the review) | samples checked in both packet units | — | 197,885 |
| KD-AAC-IMPLICIT-SBR | implicit SBR named by `ffprobe` | by construction: a missed explicit SBR signal (the predicate took CoreAV's reading) | the file's AudioSpecificConfig must be plain LC | 5 | — |
| KD-AAC-LC-UNNAMED | no LC profile named by `ffprobe` | as above | as above | 4 | — |
| KD-QT-TEXT | QuickTime text tracks CoreAV drops | none | — | 11 | — |
| KD-NOT-ALLOWED | streams the allowlist drops | none: an allowed stream dropped, under any code, is caught | — | 215 | 1,755,788 |
| KD-COVER-ART | cover pictures `ffprobe` lists | none | — | 14 | 16 |
| KD-CHAPTER-TEXT | chapter text tracks | chapter samples listed twice, and their `pts`, `dts` and keyframe flags, which the reference cannot check, and a misread empty edit moving them (46) | samples, sync flags and the presentation offset checked; timestamps must be the edit's mapping | 1 | 24 |
| KD-DECLARED-SIZE | picture size from the sample entry | none | — | 6 | — |

## Fixes

- **Evidence.** The overlay reads the raw values it takes from CoreAV from
  the file itself, with a reader written apart from CoreAV's parser
  (`Facts` in `scripts/oracle_kd.py`). An entry applies only where the two
  agree (`docs/known-divergences.md`, "Evidence the overlay checks"). This
  closes a whole class of bugs: those that move a value and its evidence
  together. The reader works in runs of regular samples, so long PCM
  tracks cost no more than their tables, and it fails closed on anything
  it cannot read.
- **Packets the reference does not list.** Discarded samples and chapter
  samples have no reference packet to compare with. Their timestamps must
  be the edit's mapping of their media times, and their samples and sync
  flags those of the file.
- **Whole-stream shifts.** KD-CTTS, KD-CTTS-UNSIGNED, KD-TRUN-SHIFT and
  KD-EDIT-SNAP model one shift for the whole stream. They now apply only
  when no paired packet agrees on the shifted field.
- **CoreAV's own value.** KD-PCM-END and the `sidx` clause of KD-FRAG-START
  pin CoreAV's value (from its review).

The reader agrees with CoreAV on every stream of the audit files and the
fixtures. On the corpus it first disagreed on one file, whose movie box is
compressed (`cmov`), and 4,101 KD-CTTS packets lost coverage. The reader
was missing the inflation, which it now has (the same zlib data CoreAV
inflates, `docs/design/compressed-movie.md`), with a test. The packets are
covered again, now on checked evidence.

## Regression

The stage 3 baseline at 9589d6e with an earlier change was rechecked with this
overlay: every file covered through an entry, plus the uncovered ones (404
files). For MP4/MOV, files stay 1,043 of 1,049 and packets 27,843,947 of
27,843,948. The pre-bar numbers stay 738 files and 89.47% of packets.
Every entry covers what it covered before (`kd-audit.json`,
`corpus_regression`).

## Transport streams

The transport stream overlay (`scripts/oracle_kd_ts.py`) is built
the same way from the start: every entry checks CoreAV's values against
the file, read by its own reader (`Facts`), and `kd_audit.py --overlay ts`
runs on the 45 transport stream fixtures in CI (`ts-fixtures`) with the
mutations above, and four of its own: a stream's end moved by one unit, or
with the unit CoreAV reports (`end_unit_ticks`); a duplicate CoreAV claims
but the file does not hold, with a packet that then differs; and two
streams whose packets, or whose records, are each other's, which on a file
of several programs looks like the reordering `KD-TS-STREAM-ORDER`
excuses.

The first run found two loopholes, closed before the entries were merged:
a wrong CoreAV format duration covered whenever the reference's matched
the streams (the predicate pins CoreAV's format duration to what its own
rule gives from its streams, and needs a stream the file confirms as an
end estimate, unless no stream differs at all and the two file durations
are the microsecond apart that their arithmetic differs by), and
timestamps and keyframe flags not pinned in the duplicate entry (it now
compares every field, for ADTS AAC streams only).

Part 3 of an earlier change took the overlay from two entries to eight. Two reviews of
that pull request found four more loopholes, each closed before it was
merged: a stream's end covered whenever the reader *failed* to work it out
rather than showed the file gives none (the entry now needs
`Facts.untimed` or `Facts.absent_from_the_tail`); a stream order covered
from a permutation found in CoreAV's own capture, so that packets routed
to the wrong stream produced the permutation that hid them (every packet
must now start in a PES packet of the PID CoreAV gives its stream,
`Facts.pes_offsets`); a compat-only entry still read with the stream at
the same index on a file whose streams the two readers order differently;
and the sampling rate of an AAC stream taken from the reference rather
than from the file's own configuration. With the fixes and the two new
mutations: 2,845 mutations, 2,777 caught, 68 no effect, none covered.

Part 4 took it to nine entries: `KD-TS-DECODE-TIME` is new, and
`KD-TS-FRAME-TIME` and `KD-TS-STREAM-ORDER` are wider, with six fixtures
for the structures behind them. Each pins CoreAV's side to the file: the
decoding time to the PES header at the packet's own position
(`Facts.stated`), the inferred presentation time of sound to the
distance that header states between the two times (`Facts.units`), and
the wider stream order to the PIDs the tables declare and the PIDs the
file carries PES packets on (`Facts.pes_offsets`). A review of that pull
request found one more loophole, closed before it was merged: the Event
Information Table paired in the permutation on each side's own codec
name alone, so a stream CoreAV named `epg` at any PID would pair; it
must now be at the PID the standard fixes. On the 45 fixtures: 3,456
mutations, 3,372 caught, 84 no effect, none covered.

## What the comparison cannot see

These are limits of comparing with `ffprobe`, not of the overlay, and no
entry covers them:
- `discard` is informational in the comparison (`tools/oracle/SPEC.md`).
- A sample that `ffprobe` does not list can be lost by CoreAV without the
  comparison noticing: for example, the text sample outside the edit in
  five fixtures (the "unlisted" verdicts). When an entry still applies to
  the stream, the sample check catches the loss.
- For H.264, the keyframe flag comes from the coded data on both sides
  (`tools/oracle/SPEC.md`, "Keyframes"), so the reader does not check it
  against the sync sample table.
