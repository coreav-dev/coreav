# Known-divergence audit: the writers

Status: Accepted. Data: [`kd-audit-writers.json`](kd-audit-writers.json).
[`kd-audit.md`](kd-audit.md) attacked the probe entries. This audit
attacks the writers' entries the same way: clip (KD-C), split
(KD-S), remux and sanitize (KD-R), extraction (KD-X), text (KD-T), HLS
(KD-HLS) and policy (KD-P). The question for each entry: can it cover a
real CoreAV bug? Each attack injects a bug-shaped change into what CoreAV
wrote, then lets the writer's own oracle judge it again. Every such change
must stay visible.

## Method

There are two tools, one for each kind of oracle.

`tools/oracle/writer_kd_audit.py` reads a writer oracle's manifest (clip,
split, remux, sanitize, extract). It changes one value of an output file
in place, so no box moves, and runs the oracle's `check()` on the changed
file. The changes, on the first video or audio track (ISO/IEC 14496-12):
- sample timing: the first `stts` entry's duration +1 or −1, the first
  `ctts` offset +512;
- sample tables: the first sample size +1, the first chunk offset +1, a
  sync sample moved one sample later;
- the edit list: the media edit's media time +1, the first entry's
  duration +1 or −1;
- headers: `mdhd` duration +1, `mdhd` timescale +1, `tkhd` duration +1,
  `mvhd` duration +7;
- the sample entry: video width +16, audio channel count +1, audio sample
  rate halved, the AAC object type swapped (2 and 5);
- the samples: a byte of the first sample flipped.

`tools/oracle/writer_kd_audit_cli.py` does the same for the oracles that
run `coreav` themselves: text, HLS and policy. These run unchanged against
a wrapper. The wrapper runs the real `coreav`, then changes its output and
keeps the command's own summary consistent with it. That makes the change
a bug in what CoreAV writes, not a mismatch a summary check would see. The
changes:
- WebVTT: a cue's start or end 1 ms later, the last cue's end 1 ms
  earlier, a letter changed, the first cue left out or written twice, an
  escape undone, a style tag dropped, a line break joined;
- chapters: the first chapter's start or end one tick later, its title
  changed, the last chapter left out, two titles swapped;
- HLS, per rendition: the first segment's `tfdt` +1, its first sample's
  duration or size +1, its first composition offset +512, its first sync
  flag cleared, a byte of its first sample flipped, the first EXTINF
  +0.1 s, the init segment's `mdhd` timescale +1, its width +16 or its
  channel count +1;
- policy: the probe document's duration +25%, video width +16, bit rate
  doubled, video codec renamed, audio channels +1 or rate doubled, the
  last stream left out. The verdict of `check` is derived again from the
  changed document (`tools/oracle/policy_ref.py`).

Verdicts compare the oracle's problems and covered differences before and
after the change:
- **caught:** a new problem.
- **COVERED:** no new problem, but a covered difference the unchanged
  output did not have. An entry absorbed the bug: a loophole.
- **ABSORBED:** nothing new, but with the known divergences switched off
  the change adds a problem. An entry that already covered the output hid
  it. Also a loophole: an oracle that names only the entries it used shows
  one only this way.
- **agrees:** only differences removed.
- **no effect:** the same result with and without the known divergences.
  The oracle does not see the change. That is not an entry's fault; see
  "What the oracles cannot see".
- **n/a:** the change does not apply (no such track or box).

Inputs:
- the generated fixtures, through each writer as CI runs it;
- for each entry, the smallest stage 3 corpus file it covers, from the
  committed corpus evidence, through the same writer. Where that file no
  longer reaches the entry with the current writer, the next files it
  covers were checked and the first that does was added (two clip files
  for KD-C-DTS-SHIFT, one policy file for KD-P-UNIDENTIFIED-DURATION), and
  the Matroska files for the policy entries of Matroska. Remux has no
  per-file records committed, so its corpus inputs are the smallest files
  with the tags its entries need, checked first for the entries they
  reach: fragmented, edit lists, timecode, variable frame rate, QuickTime
  sound, `moov` at the end.

A verdict compares the oracle's messages, so a message that does not
change can hide a change; the text oracle's did (see below), and now
digests every difference. For the same reason, on an output that already
has a problem before the change, any changed message counts as caught,
even one an entry has partly absorbed; every fixture output passes before
the change, so this weakens only the few corpus inputs that do not.

The join entries (`docs/known-divergences-join.md`) are not in this
audit: they came after an earlier change was opened, and their oracle has its own
mutation tests.

## Results

The "before" runs use `main`'s oracles (at fff1725) with the same tools,
inputs and `coreav` binary; "after" uses this change's. Where an oracle is
unchanged, one run stands for both. After each fix, the outputs it could
affect were audited again, and those results replace the earlier ones for
the same output and change. n/a counts are left out;
`kd-audit-writers.json` has every count by mutation and every loophole.

Fixtures (the generated set, as CI writes it):

| Oracle | Before | After |
|---|---|---|
| clip, 308 clips | 4,262 caught, 3,032 no effect, **42 covered** | 4,329 caught, 3,007 no effect |
| split, 120 splits | 2,948 caught, 1,944 no effect | the same |
| text | 70 caught, **24 covered** | 94 caught |
| HLS | 661 caught, 135 no effect, **79 covered** | 740 caught, 135 no effect |
| remux, 76 outputs | 1,393 caught, 254 no effect | unchanged oracle |
| sanitize, 76 outputs | 1,393 caught, 254 no effect | unchanged oracle |
| extract, 42 outputs | 450 caught, 168 no effect | unchanged oracle |
| policy, ISOBMFF and Matroska | 811 caught, 2,260 no effect, 1 agrees | unchanged oracle |

Corpus:

| Oracle | Inputs | Before | After |
|---|---|---|---|
| clip | 32 clips of 12 files | **4 covered** (first run, already with this change's KD-C-AUDIO-RESTART and KD-C-EMPTY-EDIT-DURATION) | 435 caught, 253 no effect |
| split | 6 files | 182 caught, 113 no effect, **1 covered** | 184 caught, 112 no effect |
| text | 19 subtitle streams and chapters of 15 files | 77 caught, 5 no effect, **6 covered, 14 absorbed** | 102 caught |
| HLS | 8 files | **1 covered** (run on the smallest file KD-HLS-3 covers only) | 78 caught, 15 no effect |
| remux | 27 files | 522 caught, 114 no effect | unchanged oracle |
| extract | 5 files | 54 caught, 21 no effect | unchanged oracle |
| policy | 11 files | 55 caught, 87 no effect, 2 agree | unchanged oracle |

Each run covers the entries listed in `reach` of the data. Of the 66
entries, 62 are reached by at least one input; the other four are under
"Entries no input reached".

## Loopholes found and fixed

Every loophole had the same cause. The entry read the value it excuses
from the output alone, so a wrong value agreed with itself. Each entry now
compares that value with one read from the source file, with a box reader
apart from CoreAV's.

| Entry | Absorbed | Now also requires |
|---|---|---|
| KD-T-ROUND | a cue's start or end 1 ms off: 17 on fixtures, 1 on the corpus, and 1 more absorbed | each cue's times to be the source sample's exact times, from its sample tables (or fragments) and edit, rounded as CoreAV rounds |
| KD-T-LAST-END | the last cue's end 1 ms early: 7 on fixtures, 5 on the corpus, and 5 more absorbed | the last cue's end to be the source's exact end |
| KD-T-INVALID | cues around an invalid text sample changed or left out: 8 absorbed on two corpus streams | each dropped cue to match an invalid sample of the source, by text and times |
| KD-T-ZERO-END | any end where `ffmpeg` writes a zero-length cue (found by reading the predicate after an unexplained "no effect") | CoreAV's end to be the source sample's exact end; for fragmented sources, from the fragments' runs |
| KD-C-AUDIO-RESTART | the audio edit's media time +1: 40 on fixtures | the kept packets, the edit's media time and the decode durations to be the source's |
| KD-C-EMPTY-EDIT-DURATION | the media timescale +1: 2 on fixtures | the media timescale and the decode durations to be the source's (`mdhd`, `stts`) |
| KD-C-AUDIO-EDIT-RATE | the audio edit's media time +1: 2 on the corpus | the audio timing to be the source's, as KD-C-AUDIO-RESTART |
| KD-C-AUDIO-JITTER | the audio edit's media time +1 or a decode duration −1: 2 on the corpus | the same |
| KD-S-AUDIO-RESTART | a decode duration −1: 1 on the corpus | the segment's decode durations to be the source's |
| KD-HLS-3 | `tfdt` +1: 79 on fixtures, 1 on the corpus | the rendition's composition times and the init segment's edit to be the source's (`ffprobe` with edit lists ignored, and the source's edit list) |
| KD-C-DTS-SHIFT, KD-S-DTS-SHIFT | any constant decode-time shift (found by reading the predicates) | the shift to be the source's smallest composition offset less the output's, from both files' `ctts`, the relation every corpus case shows |

Stored decode durations are read from the source's `stts` and, for a
fragmented source, its fragments' `trun` boxes (ISO/IEC 14496-12 8.8).
`clip_check.fragment_samples` does this; `text_check` uses it too. Unit
tests pin each fix with the near miss that used to pass
(`tools/oracle/tests/test_{text,clip,split,hls}_kd.py`).

Two loopholes did not show as COVERED or ABSORBED. The text oracle's
message named only the first three differing cues, so with the known
divergences off a further difference left it unchanged, and the audit saw
"no effect". Its subtitle and chapter messages now also give the number of
differing items and a digest of all of them, which makes any further
difference visible. The DTS-shift predicates were found by reading them
against the method; no input reached KD-S-DTS-SHIFT.

## Regression

Narrowing must not lose a real divergence. Each narrowed entry was
re-checked on the corpus files the stage 3 evidence has it cover, and on
the fixtures:
- **Text entries:** the 19 corpus streams and chapters cover the same
  entries as before, and so do the fixtures.
- **KD-C-AUDIO-RESTART, KD-C-AUDIO-JITTER, KD-C-AUDIO-EDIT-RATE and
  KD-C-EMPTY-EDIT-DURATION:** 133 corpus files of clip `check-04`; 188, 1,
  5 and 65 clips covered before and after, no new problem.
  - A first version of the KD-C-EMPTY-EDIT-DURATION fix compared the
    output with `ffprobe`'s packet durations, and 47 of its 65 clips
    failed: `ffprobe` can report a duration other than the stored one.
    The fix reads the source's `stts` instead.
  - A first version of the stored-duration check read `stts` alone, and
    three fixture clips of a fragmented source failed. It now reads the
    fragments too.
- **KD-C-DTS-SHIFT:** 8 corpus files of `check-04`. 15 clips were covered
  there and 14 are now; the 15th agrees directly with the current writer.
  No new problem.
- **KD-S-AUDIO-RESTART:** 105 corpus files of split `check-03`, 2,044
  covered differences before and after, no new problem.
- **KD-HLS-3:** 13 of its 14 corpus files give the same results with
  `main`'s oracle and this change's. The 14th (1.9 GB) was not run.
- **Fixtures:** clip (308 of 308) and split (120 of 120) pass with the
  same result per output as with `main`'s oracles; remux, sanitize,
  extract, text and HLS pass.

## Entries no input reached

No mutation could test these entries, because nothing in the audit's
inputs reaches them. Each was reviewed against the method instead: does
it compare raw values from both files?
- **KD-R-PROBE-WINDOW, KD-R-LAST-DURATION, KD-R-VFR-RATE** (4, 1 and 1
  corpus files in remux `check-03`, whose per-file records are not kept).
  Each compares the two files' own boxes (`stts`, `mdhd`) or a whole-file
  `ffprobe` of each, and requires them equal. A bug in the output alone
  breaks the equality.
- **KD-S-DTS-SHIFT** (no corpus file in split `check-03`). Narrowed as
  KD-C-DTS-SHIFT, which the corpus reaches, and pinned by a unit test.

## In CI

`fixtures.yml` runs the text audit on the fixtures (about two minutes)
and fails on a COVERED or ABSORBED verdict, or when nothing is caught
(the wrapper or the oracle did not run). The other audits take up to an
hour each and run on demand; the unit tests above guard their fixes.

## Blind spots

The "no effect" verdicts above were limits of the oracles. An earlier change closes
them with checks that read both files' own boxes
([`writer-oracle-boxes.md`](../design/writer-oracle-boxes.md), whose pass
criteria were committed before these runs). Data:
[`kd-audit-writers-324.json`](kd-audit-writers-324.json), counts only.
"Before" is `main`'s oracles, "after" this change's, with the same outputs,
`coreav` binary and black boxes (`ffmpeg` and `ffprobe` 7.0.2, MediaInfo
26.10) on one machine.

Mutation audit, no effect before → after (COVERED and ABSORBED are 0 after
in every set):

| Oracle | Fixtures | Corpus inputs |
|---|---|---|
| clip | 3,007 → 0 | 110 → 0 |
| split | 1,944 → 0 | 94 → 0 |
| split, several audio edits | 66 → 0 (and 4 COVERED → 0) | |
| remux | 254 → 0 | 32 → 0 |
| sanitize | 254 → 0 | 21 → 0 |
| extract | 176 → 0 | 52 → 0 |
| HLS | 135 → 0 | 20 → 0 |

The corpus inputs are those the design note's rule picks from the corpus
run (per writer, the smallest real files it writes, with and without
audio, and the remux and extraction cases an earlier change names); for clip, 10 files:
each file's head clip and all four clips of one. The HLS audits ran again
with the final oracle and the binary after an earlier change's fix, whose packages
start a trimmed source's video at a later sync sample; every count,
mutation by mutation, is the same as with the earlier binary (both are in
the data).

Fixtures: every writer's outputs pass every oracle (100%).

Corpus v1, every real ISOBMFF file (1,049) through every writer, the full
oracles (`tools/oracle/writer_corpus.py`):

| Writer | Written | Pass | Fail, also with `main`'s oracle | Black box crashed | Refused |
|---|---|---|---|---|---|
| clip (4 per file) | 2,568 | 2,564 | 0 | 4 | 302 |
| split (every 10 s) | 830 | 827 | 2 | 1 | 219 |
| remux | 931 | 931 | 0 | 0 | 118 |
| sanitize | 931 | 931 | 0 | 0 | 118 |
| extract | 520 | 520 | 0 | 0 | 529 |
| HLS (6 s) | 884 | 881 | 3 | 0 | 165 |

- No output fails with this change's oracles and passes with `main`'s, and
  none of the 337 sampled passing outputs fails with `main`'s. No writer
  crashed.
- The five failures are four files whose outputs fail the same way with
  `main`'s oracles. The black-box crashes are one file on which the
  pinned `ffmpeg` aborts while decoding the source, so its five outputs
  have no verdict from the full oracles; their box checks pass.
- The run used a binary of `main` before an earlier change's fix for 1,029 files. The
  other 20 ran again with the binary after it: the five files an earlier change named
  (they now pass every writer), the files whose run a full disk stopped,
  the three largest files (up to 6.3 GB), and every file with a failing
  output.
- The box checks alone, on the final commit with the binary after an earlier change's
  fix, over all 1,049 files: every written output passes (2,568 clips, 830
  splits, 931 remuxes, 931 sanitized files, 520 extractions, 884 HLS
  packages), and every output is written or refused as in the full run.
- Pass criterion 4 asks for the run on the final commit. The full oracles
  of most files are this change's before one fix: the HLS box check then
  started an audio rendition by the 0.5 s pre-roll rule `hls.md`
  gave before change A's fix, which failed two of the five files change A named;
  it now starts at the source's first sample, as the packager does. That
  rule is in the box checks, which ran on the final commit over every
  file; the other oracle code differs from the final commit's only in the
  wording of comments and known-divergence bases. The full run was not
  repeated for it.

## What the oracles cannot see

Since an earlier change the oracles read the header durations, the edits, the sync
samples, the sound sample entry, the audio timing and the samples outside
the edit from both files' own boxes ("Blind spots"). What remains:
- **Policy.** A probe value that changes without crossing a rule leaves
  the verdict unchanged, and `policy_check` judges verdicts. The values
  themselves are the probe oracle's (`kd-audit.md`).
- What [`writer-oracle-boxes.md`](../design/writer-oracle-boxes.md) lists
  under "Not checked".
