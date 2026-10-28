# Known divergences: decode-audio versus ffmpeg

`coreav decode-audio` writes the presentation timeline of an MP4 track,
sample-exact (`docs/design/audio-decode.md`, "Edit lists"). `ffmpeg` 7.0.2
applies an edit list its own way, at packet granularity. Bar 2 of "Edit list
bars" requires every difference between the two outputs to be documented
here, with a machine-readable twin:
[`known-divergences-decode-audio.json`](known-divergences-decode-audio.json).
`scripts/decode_audio_check.py` applies it with one predicate per entry, and
`tools/oracle/tests/test_decode_audio_kd.py` keeps this page, the twin and
the predicates in sync.

Each entry rewrites one kind of piece of CoreAV's output (an edit's samples)
into what `ffmpeg` writes for it, only where the edit list, as the oracle's
independent reader reads it from the file (`Facts`,
`scripts/oracle_kd.py`), meets the entry's condition. Every sample an entry
drops or replaces is checked first (silence must be zeros, dropped media
must be the media's samples in `ffmpeg`'s full decode), and an input counts
as covered only if the rewritten output equals `ffmpeg`'s (FLAC bit-exact,
MP3 within the MP3 agreement tolerance), so no entry is a tolerance. CoreAV's
own output is checked separately against the presentation model (bar 1).
Packets are the track's samples as the sample tables give them; the last one
ends where its decode ends.

The entries were written after the bars (`ADDED_AFTER_THE_BARS` in the
check script) and are audited by mutation: `--audit` changes CoreAV's output
and summary in bug-shaped ways (a sample changed at each edit's ends, added
or dropped, a silence span moved or not silent, the leading silence dropped,
the output of an edit list misread by one tick or an entry), and no entry
may cover a change.

The differences were observed on fixtures with known content (a FLAC ramp
whose sample n has the value n, and an MP3 encode), made by the audio
decode spike's fixture script.

## KD-A-LEADING-EMPTY

- Samples: the silence of an empty edit before the first media edit.
- What each writes: CoreAV writes the edit's duration as silence, so the
  output's sample n is the sound at presentation time n / rate. `ffmpeg`
  writes nothing for it; its output starts with the first media sample.
  Example: a 250 ms empty edit before 3 s of 48 kHz media; CoreAV writes
  12,000 zero samples first, `ffmpeg` none.
- Why: ISO/IEC 14496-12 8.6.6 (an empty edit is time with no media
  presented); the design note's rule: silence for every empty edit, a
  leading one included, reported in the summary's `timeline.start`.
- Coverage requires: an empty edit before any media edit that presents a
  sample, CoreAV's samples for it all zero, and `ffmpeg`'s output without
  them.

## KD-A-EDIT-END

- Samples: those after a media edit's end, to the end of the packet the end
  falls in.
- What each writes: CoreAV ends the edit at its exact end. `ffmpeg` keeps the
  whole packet: its output continues with the rest of that packet. Example: a
  2,000 ms edit from sample 0 of 48 kHz FLAC with 4,608-sample packets;
  CoreAV writes 96,000 samples, `ffmpeg` 96,768 (21 packets).
- Why: ISO/IEC 14496-12 8.6.6 (a media edit presents `segment_duration` of
  media); the design note's rule: windows cut sample-exact at both ends.
- Coverage requires: a media edit whose end, in samples, falls inside a
  packet before the end of the media, and `ffmpeg`'s output continuing
  with that packet's remaining samples of its full decode
  (`-ignore_editlist 1`).

## KD-A-LATER-EDIT-START

- Samples: the first samples of a media edit after the first one, up to the
  next packet's start.
- What each writes: CoreAV starts the edit at its exact media time. `ffmpeg`
  starts it at the first packet that starts at or after it. Example: a
  second edit from sample 96,000 of the 4,608-sample packets above; CoreAV
  starts at 96,000, `ffmpeg` at 96,768.
- Why: as `KD-A-EDIT-END`.
- Coverage requires: a media edit after one that presented samples, its
  media time inside a packet, CoreAV's samples up to the next packet's start
  equal to the media's, and `ffmpeg`'s output for the edit starting there.

## KD-A-INNER-EMPTY

- Samples: those of an empty edit after a media edit.
- What each writes: CoreAV writes the edit's duration as silence. `ffmpeg`
  writes the media's first whole packets in their place, those that start
  before the edit's length. Example: a 200 ms empty edit between two media
  edits of 48 kHz FLAC with 4,608-sample packets; CoreAV writes 9,600 zero
  samples, `ffmpeg` the media's first 13,824 samples (3 packets).
- Why: ISO/IEC 14496-12 8.6.6 (an empty edit is time with no media
  presented).
- Coverage requires: an empty edit after a media edit that presented
  samples, CoreAV's samples for it all zero, and `ffmpeg`'s output there
  equal to those packets of its full decode. For a lossy codec the first of
  them is compared in length only: `ffmpeg` decodes it after the packet
  before the empty edit, and the overlap-add of its first granule with that
  packet's (ISO/IEC 11172-3, Layer III's IMDCT) makes it differ from the
  fresh decode.
  CoreAV writes silence there, which is checked.
