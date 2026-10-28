# `coreav-text` on sources with several media edits

Results of `docs/design/edit-list-writers.md`, "`coreav-text` timing",
against the pass criteria fixed in that note before these runs. Code:
`crates/coreav-text/src/timing.rs`. Oracle: `scripts/text_check.py` with
the pinned ffprobe, ffmpeg (7.0.2) and MediaInfo (26.10).

## Fixtures

`scripts/gen-fixtures.sh`, every MP4, MOV and M4A fixture, at the commit
this branch starts at and at its head:

| | before | after |
|---|---|---|
| Subtitle tracks | 9 of 9 pass (1 directly, 8 through known divergences) | **13 of 13** (5 directly, 8 through known divergences) |
| Chapter lists | 6 of 6 pass (5 directly, 1 through `KD-T-CHPL`) | **8 of 8** (7 directly, 1 through `KD-T-CHPL`) |

The six several-edit inputs the oracle builds are the whole difference; no
other result changes, and no new known divergence is used. Each is
`text_sized.mp4`'s or `chapters_sized.mp4`'s timed text track — 1,000,000 Hz
media in a 1,000 Hz movie, cues at 0.25–0.9, 1.0–1.6 and 1.6–2.0 s — with
its edit list replaced:

| Input | Edit list (movie ticks : media ticks) | What comes out |
|---|---|---|
| **splice**, text and chapters | `700:0`, `700:1300000` | 0.250–0.700, 0.700–1.000, 1.000–1.400: the first cue is cut at the first edit's end, the second is shown from the middle of the second edit, and the sample between them is not shown |
| **contiguous**, text and chapters | `500:0`, `500:500000`, `500:1000000`, `500:1500000` | **byte for byte the unedited extraction**: four edits that play the media back to back present exactly what no edit list does |
| **boundary** | `1000:0`, `1000:1000000` | the same, with the edits' boundary on a sample boundary |
| **split-cue** | `400:0`, `200:-1`, `1600:500000` | 0.250–0.400, 0.600–1.000, 1.100–1.700, 1.700–2.100: **one sample comes out as two cues**, at the two places the two edits show it |

The chapter lists come out the same way, in exact ticks, with the titles the
track's own samples hold. All four shapes agree with the edit list applied
by ISO/IEC 14496-12 8.6.6, computed in the oracle from the track's own
`stts`, `elst`, `mdhd` and `mvhd` and its sample texts.

**What the oracle caught.** An earlier draft cut each sample to the one
media edit that maps it, which is `docs/design/edit-lists.md` decision 2's
rule for a *packet*. On the **contiguous** input that showed the first cue
for 250 ms of its 650, and on a hundred 20 ms edits it would have shown it
for 10 ms. The gate above, which intersects each sample with every window
as 8.6.6 does, fails that writer; the input is kept for it.

**Second view (reported).** ffmpeg writes 3, 5, 3 and 4 cues for the four
text inputs against CoreAV's 3, 3, 3 and 4: it anchors a later edit on
decode times, rounds durations to whole ticks and repeats samples at the
boundaries (`docs/design/edit-lists.md`, "What `ffprobe` lists", rules 1 to
9). Its cue list is therefore reported and not gated for such a track.

**Mutation audit.** `tools/oracle/writer_kd_audit_cli.py --oracle
text` on the six several-edit inputs: 42 caught, 70 not applicable, **nothing
covered or absorbed**: every bug-shaped change to a cue's times, its text
or the track's headers shows as a new problem.

## Where else the packet rule could have been applied

The bug above came from applying `docs/design/edit-lists.md` decision 2 —
the rule for a packet's one time — to a cue, which is an interval. Every
other consumer of that mapping was checked for the same mistake:

| Consumer | What it does with a list of several media edits |
|---|---|
| `coreav probe` packet listing (`Packets`, `Span::owner`) | one time per packet: the rule's own use |
| `Track::presentation`, `Track::duration` (`movie.rs`) | computed from the edit list entries, not from the mapping: the span runs from the leading empty edits through the last media edit, and the duration adds the media edits up (decision 5) |
| `coreav-policy` | reads probe's durations, above |
| clip | carries every window into the output's own edit list (clip decision 4), and `presented_until`/`media` walk the windows |
| M4A extraction | copies the entries; nothing is mapped |
| split, join | refuse such a list (`Timeline::of`) |
| the HLS writers | refuse such a list |
| `coreav thumbnail` | its seek path takes only `None` and `Simple`; the listing path picks a frame by its packet time, one time per packet |
| `coreav decode-audio` | ignores the list with a warning |
| remux, sanitize | copy the edit boxes as bytes |
| `coreav chapters` in `coreav-cli`, the MCP server and the Python package | all call `read_chapters` in `coreav-text`, fixed here |
| `scripts/clip_check.py`, `scripts/extract_check.py` | `iso_presented` already lists a frame per window it overlaps |

So `coreav-text` was the only one, and the rule in decision 2 now says it
is for packets only.

## Corpus

Of the 59 corpus files whose edit list is outside the simple model, seven
have a text or chapter track: six `edge` MOV files and one `real` M4A. All
seven were extracted and checked:

| File | Text or chapter track | Edit list on it |
|---|---|---|
| 11e274bc53af | QuickTime text, stream 9 | one media edit |
| 398a59d2692e | QuickTime text, stream 11 | one media edit |
| 554464073ddc | QuickTime text, streams 1 and 2 | a rate other than 1 (refused, `KD-T-EDIT-RATE`) |
| 67a6b2492ada | six timed text tracks and a chapter list | one media edit |
| 88ba79019cd6 | a chapter list | one media edit |
| a9a660bad19c | QuickTime text, stream 2 | one media edit |
| ceb17f01023a | `c708`, dropped | — |

**The whole corpus, on the commit that merges.** The text gate was run
over the same 112-file subset the stage 3 text run used
(`scripts/corpus_subset.py OUT --all c:isobmff --any subtitle-track
chapters`, 22.6 GB), with this branch and with `main` built from the commit
it starts at, on one machine:

| | this branch | `main` |
|---|---|---|
| Subtitle streams | **59 of 61** pass (25 directly, 34 through known divergences) | 59 of 61, the same |
| Chapter lists | **35 of 35** pass (17 directly, 18 through known divergences) | 35 of 35, the same |
| `KD-T-INVALID` | **3 streams**, its recorded count | 3 |
| Every other entry | `KD-T-EDIT-ALIGN` 16, `KD-T-LAST-END` 6, `KD-T-EMPTY` 5, `KD-T-CR` 5, `KD-T-ZERO-END` 5, `KD-T-STYLE` 4, `KD-T-ESCAPE` 3, `KD-T-QT-STYLE` 2, `KD-T-ROUND` 1, `KD-T-EDIT-RATE` 1, `KD-T-MI-HOURS` 1; chapters `KD-T-CHPL` 10, `KD-T-CHAP-ORDER` 7, `KD-T-HMMT` 1 | the same |
| Harness failures | **0** | 0 |

Every item agrees with `main`'s, file by file and stream by stream, so this
change moves nothing on the corpus. `KD-T-INVALID` matters on its own: it
is reached only by corpus files with text that is not valid UTF-8, of which
there is no fixture, and an earlier draft of this change had disabled it
without any fixture run noticing.

The two streams that fail do so identically on `main`: both were among the
eight files `coreav probe` rejected when
`docs/measurements/text-corpus-v1.md` was written, probe accepts them now,
and their QuickTime text tracks then disagree with ffmpeg. That is an earlier change,
not this change.

**No corpus text or chapter track has several media edits**: every file
above carries its multi-edit list on an audio or a video track. So the
corpus bar is met in the only way it can be — 10 of 10 subtitle tracks and
2 of 2 chapter lists pass, the same result as before this change — and the
fixtures carry the evidence for the new code. Should such a file join the
corpus, `scripts/text_check.py` now judges it on its own boxes.

## Speed (development measurement)

Neither `coreav extract-subtitles` nor `coreav chapters` has a speed bar
against FFmpeg today, and no held-out corpus input reaches the new code, so this
is a development measurement: one machine, engines interleaved, median of
15 runs after one warm-up each, `main` built from the commit this branch
starts at and timed in the same run. Measured on the code as it merges,
after the walk gained its step per media edit.

| Input | ours ÷ ffmpeg | ours | `main` | ffmpeg |
|---|---|---|---|---|
| `text_sized.mp4`, the splice input | **0.3235** | 0.0012 s | — (refused) | 0.0036 s |
| `text_sized.mp4`, one media edit | 0.3139 | 0.0012 s | 0.0012 s | 0.0037 s |
| 67a6b2492ada stream 1, one media edit | 0.1659 | 0.0012 s | 0.0012 s | 0.0073 s |

Share faster than FFmpeg: 3 of 3. Not worse than `main`: equal to
the millisecond on the two inputs both accept. The walk adds one cursor
over the track's edit list entries, moved forward at most once per sample
and at most once per entry over the whole track; a sample of no duration
stops at the first media edit that starts after it, so it walks no further
than one with a duration does.

Evidence: [`text-several-edits.corpus.json.gz`](text-several-edits.corpus.json.gz),
this branch's report for every file of the subset.

Spend: 0 USD (local runs; R2 egress is free, and the corpus subset was
downloaded once, 22.6 GB).
