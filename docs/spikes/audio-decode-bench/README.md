# Audio decoding against FFmpeg: agreement and speed

Throwaway measurement code, outside the Cargo workspace. It measures `coreav_audio` and `coreav decode-audio` against
FFmpeg under the bars of [`docs/design/audio-decode.md`](../../design/audio-decode.md),
fixed there, per codec, in a commit before the codec's runs: for FLAC in
`45a62b7` (on the branch as merged; the speed run measured `66d50b7`,
the agreement run its binary too); for MP3 in `bf3d432` (the speed run
measured `51a76e9`, the agreement run of record the final binary of the
pull request, `597fce4`); for resampling and downmixing in `b5c7245`
("Resampling and downmixing").

## Hypothesis and bars (FLAC)

CoreAV's native FLAC decoder, written from RFC 9639, decodes FLAC in MP4
and Matroska bit-exactly and at least as fast as FFmpeg. The bars, from the
design note ("FLAC bars"):

- **Conformance**: the IETF CELLAR FLAC test files decode to their
  streaminfo MD5 (all 64 of `subset`, `uncommon` 05 to 09); the rest as the
  note says. Checked by `crates/coreav-audio/tests/flac.rs` in CI.
- **Agreement**: bit-exact against `ffmpeg` at 32 bits and in the default
  16-bit WAV, on 100% of the inputs `ffmpeg` decodes without an error.
- **Speed**: the median per-input ratio CoreAV / FFmpeg at most 1.00 on the
  held-out set, overall and per class (MP4, Matroska), for the library
  (one thread each, against libavformat, libavcodec and libswresample
  in-process) and for the command (against the `ffmpeg` command, both with
  their defaults).

## Results at a glance (FLAC)

| Bar | Result | Status |
|---|---|---|
| Conformance: `subset` 64 of 64, `uncommon` 05 to 09 bit-exact | all; `uncommon` 01 to 04 refused, 10 and 11 need an earlier change; `faulty` 11 of 11 without a panic | **Met** |
| Agreement: bit-exact at 32 bits and in the 16-bit WAV, on every input `ffmpeg` decodes without an error | **527 of 527** (516 derived, 11 corpus) | **Met** |
| Library, median ours / libav at most 1.00, held-out, overall and per class | **0.881** (95% CI 0.872 to 0.891), 516 inputs; MP4 0.887, Matroska 0.875 | **Met**, faster |
| Command, median ours / `ffmpeg` at most 1.00, held-out, overall and per class | **0.588** (95% CI 0.569 to 0.607), 516 inputs; MP4 0.587, Matroska 0.589 | **Met**, faster |

The upper bounds of both intervals are below 1.00, so CoreAV is faster
than FFmpeg on these inputs in both pairs. Share of
held-out inputs where CoreAV is faster: library **86.6%** (MP4 86.0%,
Matroska 87.2%), command **95.2%** (MP4 94.2%, Matroska 96.1%).

## Speed (`evidence/flac-perf.json`, derived set, held out)

| Pair | Class | Inputs | Median | 95% CI | Faster (ratio < 1.00) | Max |
|---|---|---:|---:|---|---:|---:|
| library (gates) | all | 516 | 0.881 | 0.872 to 0.891 | 86.6% | 1.671 |
| library (gates) | mp4 | 258 | 0.887 | 0.873 to 0.901 | 86.0% | 1.671 |
| library (gates) | matroska | 258 | 0.875 | 0.867 to 0.886 | 87.2% | 1.408 |
| command (gates) | all | 516 | 0.588 | 0.569 to 0.607 | 95.2% | 1.255 |
| command (gates) | mp4 | 258 | 0.587 | 0.561 to 0.611 | 94.2% | 1.255 |
| command (gates) | matroska | 258 | 0.589 | 0.563 to 0.616 | 96.1% | 1.243 |
| library vs `find_stream_info` (reference) | all | 516 | 0.877 | 0.869 to 0.886 | 88.0% | 1.402 |
| decoder alone (reported) | all | 516 | 0.823 | 0.817 to 0.833 | 96.3% | 1.613 |

The corpus set (13 inputs measured: the 17 corpus FLAC files less 2 IAMF
and 2 encrypted ones, which CoreAV refuses) gives library 0.868, command
0.576, decoder alone 0.876 (`summarize.py --set corpus`); it is not held
out and gates nothing.

By bit depth, the library pair is 0.86 to 0.87 on the 16-bit inputs (392)
and 0.98 to 1.01 on the 24-bit ones (124), which are 55 of the 69 inputs
where CoreAV is slower. The decoder alone is 0.90 on 24-bit inputs, so the
rest is the conversion of 24-bit samples to the 16-bit output, a speed
item for the next change of this crate (with a new run of this pair).

## Agreement (`evidence/flac-agree.json`)

- 527 inputs counted, 527 bit-exact in both comparisons.
- Listed apart, as the bar says: 4 corpus files `ffmpeg` decodes with
  errors (two damaged Matroska files, whose decodable part CoreAV writes
  bit-exactly too, and two encrypted MP4 entries), 2 IAMF streams CoreAV
  refuses (design note, FLAC choices), and the 2 derived files of one
  source that hold no packets.
- The first agreement run, before the edit-list change of the design note
  ("Trimming"), differed on 5 derived MP4 inputs, each only by the
  samples after the edit's duration; and failed on the two damaged files,
  before the end-of-file rule. Both rules were set after that run, with
  their reasons, and the bar was not changed.

## Hypothesis and bars (MP3)

CoreAV's native Layer III decoder, written from ISO/IEC 11172-3, 13818-3
and Fraunhofer IIS's syntax document, decodes MP3 in MP4 and Matroska
within twice the standard's full accuracy of FFmpeg's float decoder and at
least as fast as FFmpeg. The bars, from the design note ("MP3 bars"):

- **Conformance**: `compl.bit` of the Layer III compliance package within
  full accuracy of its reference output; every other stream of the
  package decodes without an error. Checked by
  `crates/coreav-audio/tests/mp3.rs` in CI.
- **Agreement**: against `ffmpeg -f f32le`, RMS of the difference at most
  1.8 × 10^-5 and no difference above 1.2 × 10^-4, with lengths that
  differ only by the trimming one applies and the other does not, on 100%
  of the inputs `ffmpeg` decodes without an error.
- **Speed**: as FLAC's, on the MP3 sets, with the share of held-out
  inputs on which CoreAV is faster reported.

## Results at a glance (MP3)

| Bar | Result | Status |
|---|---|---|
| Conformance: `compl.bit` within full accuracy; the other 10 streams decode | `compl.bit` RMS 5.4 × 10^-8 (bar 8.8 × 10^-6), largest 4.2 × 10^-7 (bar 6.1 × 10^-5); all 10 decode | **Met** |
| Agreement within twice full accuracy, on every input `ffmpeg` decodes without an error | **605 of 607** (516 of 516 derived, 89 of 91 corpus); the 2 others differ by recorded choices, below. Counting the 11 `ms\0U` inputs, whose refusal was recorded after run 1, as failures: 605 of 618 | **Not met**: 2 inputs, each a recorded divergence |
| Library, median ours / libav at most 1.00, held-out, overall and per class | **0.940** (95% CI 0.930 to 0.949), 512 inputs; MP4 0.947, Matroska 0.938 | **Met**, faster |
| Command, median ours / `ffmpeg` at most 1.00, held-out, overall and per class | **0.185** (95% CI 0.174 to 0.203), 512 inputs; MP4 0.181, Matroska 0.190 | **Met**, faster |

Both intervals end below 1.00, so CoreAV is faster than FFmpeg on these
inputs in both pairs. Share of held-out inputs
where CoreAV is faster: library **79.9%** (MP4 76.4%, Matroska 83.3%),
command **100%**.

## Speed (`evidence/mp3-perf.json`, derived set, held out)

| Pair | Class | Inputs | Median | 95% CI | Faster (ratio < 1.00) | Max |
|---|---|---:|---:|---|---:|---:|
| library (gates) | all | 512 | 0.940 | 0.930 to 0.949 | 79.9% | 1.647 |
| library (gates) | mp4 | 254 | 0.947 | 0.930 to 0.958 | 76.4% | 1.611 |
| library (gates) | matroska | 258 | 0.938 | 0.920 to 0.946 | 83.3% | 1.647 |
| command (gates) | all | 512 | 0.185 | 0.174 to 0.203 | 100.0% | 0.690 |
| command (gates) | mp4 | 254 | 0.181 | 0.169 to 0.214 | 100.0% | 0.690 |
| command (gates) | matroska | 258 | 0.190 | 0.171 to 0.212 | 100.0% | 0.684 |
| library vs `find_stream_info` (reference) | all | 512 | 0.936 | 0.923 to 0.946 | 81.2% | 1.618 |
| decoder alone (reported) | all | 512 | 0.978 | 0.970 to 0.987 | 63.9% | 3.361 |

512 distinct files: four pairs of sources give byte-identical encodes,
each measured once. The command decodes on one thread per CPU, with each
job primed by the frames before it (design note, "Threads"); `ffmpeg`
decodes MP3 on one. The corpus set (98 inputs; not held out, gates
nothing) gives library 0.961 (78.4% faster) and command 0.161
(`summarize.py --set corpus`).

By encoder setting, the library pair is fastest on mono and VBR (0.81 to
0.91, 92 to 100% faster) and closest on high-rate stereo: 320 kbit/s stereo
0.992 (57% faster), 192 kbit/s stereo 0.960 (68%). Those spend most of
their time in Huffman decoding, the next speed item for this crate (with
a new run of this pair). The decoder alone is 0.978: most of the library
pair's lead is the pipeline around the decoder.

## Agreement (`evidence/mp3-agree.json`)

- 643 inputs; 607 counted, 605 within the tolerance. Largest RMS among
  them 3.1 × 10^-7 and largest difference 7.6 × 10^-6 (derived), 2.7 ×
  10^-7 and 5.8 × 10^-6 (corpus); in 16-bit steps at most 1, and 53 of
  them bit-exact in the default WAV. 259 Matroska inputs are compared from
  their `CodecDelay` on (CoreAV applies it, `ffmpeg` does not for MP3).
- The 2 others, each a divergence the design note records:
  - an MPEG-2 stream whose intensity positions are 1-bit (the right
    channel's `scalefac_compress` gives `slen` 1): CoreAV decodes the
    largest position as illegal (13818-3 2.4.3.2), `ffmpeg` as a position,
    so its right channel is √2 times CoreAV's in those bands;
  - a MOV file with an edit list of several edits, which CoreAV does not
    apply (warning `EditListIgnored`), so its output starts earlier.
- Listed apart, as the bar says: 13 inputs `ffmpeg` decodes with errors;
  4 Layer II streams and 1 stream whose configuration says AAC (refused
  by name); 11 QuickTime `ms\0U` entries (refused by name; their packets
  need the QuickTime version-1 sound description, which the MP4 demuxer
  does not read, recorded after run 1); 7 truncated MP4 and MOV files the
  demuxer fails on before the first packet (the end-of-file rule).

## Runs (MP3)

Every run measured the same bars; the decoder changed between the
agreement runs, and run 3 is the agreement run of record.

1. **Agreement run 1** (`f09bb29`, the decoder of the bars commit; `evidence/mp3-agree-run1.json`): 595 of 614
   counted. It found frame-parallel jobs of MPEG-2 frames primed by one
   frame where the synthesis filter needs two granules (2 derived inputs);
   MP3 stored in MP4 samples of any size (1 corpus file); frames past an
   MP4 edit's end that `ffmpeg` drops (2 derived); and two spliced corpus
   files whose frames reach into earlier granules' data or hold a
   `big_values` beyond their `part2_3_length`, which decoded as noise.
   Fixed in `b8aa676`.
2. **Agreement run 2** (`b8aa676`; `evidence/mp3-agree-run2.json`): 602 of 607. The edit's end, applied to
   decoded positions, dropped frames `ffmpeg` keeps by their packet
   timestamps (2 derived), and the reframer dropped a last frame cut short
   (1 corpus). Fixed in `51a76e9`.
3. **Agreement run 3** (`51a76e9`; `evidence/mp3-agree-run3.json`): 605 of
   607.
4. **Speed** (`51a76e9`): one run of all inputs, stopped after 239 for the
   start-up fix of an earlier change (the builds would have loaded the machine) and
   resumed (`perf.py --resume`) on the same binaries; each input is
   measured within one session.
5. **Review fixes** (`654a5c3`): bounded priming (each channel's two
   granules, at most 64 packets and 256 KiB) and reframing (free-format
   ends within 8 KiB, junk concealed, a frame of another rate refused),
   missing reservoir bytes replaced in place instead of shifted, a silent
   granule's scalefactors zeroed, limits fatal after the first packet,
   Layer I named `mp1`. **Agreement run 4** on that binary
   (`evidence/mp3-agree-run4.json`): 605 of 607,
   the same two; 3 inputs' values moved (two listed apart, and the multi-edit file).
   **Two speed checks** on that binary (`evidence/mp3-perf-check.json`
   and `mp3-perf-check2.json`, `perf.py --every 13`, 48 inputs; not a
   bar): library 0.970 and 0.940, command 0.169 and 0.162, where
   the record gives 0.947 and 0.164 on the same inputs: within
   run-to-run noise (the decoder alone: 0.990 and 0.980, record 0.980).
6. **Second review** (reframing of streams cut anywhere that start off a
   frame, and one concealed packet per run of junk): only packets that are
   not whole frames change, so the corpus set was run again on that binary
   (`evidence/mp3-agree-corpus-run5.json`): the same 89 of 91, no input's comparison changed;
   the derived files are whole frames each.
7. **No `libm`** (`597fce4`): the trigonometric constants became generated
   literals and |x|^(4/3) a Newton cube root (bit-identical in f32 to
   `powf`), because linking the C math library cost every start of
   `coreav` 38,000 instructions. **Agreement run 6** on that binary is the
   record (`evidence/mp3-agree.json`): 605 of 607, the same two; every
   comparison as in run 4 (`evidence/mp3-agree-run4.json`), except that one
   corpus MP4 listed apart (`ffmpeg` errors) now stops at
   `max_decode_errors` instead of at a frame of another sample rate, from
   the reframer's round-2 change.

The edit-end rule changes FLAC in MP4 too, so the FLAC agreement was
repeated on `51a76e9` (`evidence/flac-mp4-recheck.json`): the 258 derived
FLAC Matroska files remuxed into MP4 by stream copy (the remux writes the
same millisecond edit lists as the encodes; one source would not remux),
258 of 258 bit-exact at 32 bits and in the 16-bit WAV.

## Edit lists

Hypothesis: `decode-audio` writes an MP4 track's presentation timeline
sample-exact (empty edits as silence, media edits as sample-exact
windows), every difference from `ffmpeg` is one of a few documented ways
`ffmpeg` applies edits at packet granularity, and the change costs no speed.
The bars are "Edit list bars" in the design note, fixed in `9cac71f`
before the code and the runs.

- **Fixtures** (`make_edit_fixtures.py`, `evidence/edit-fixtures.json`): a
  24-bit FLAC ramp (sample n has the value n) and an MP3 encode of pink
  noise, each in MP4 with its edit list rewritten into the six shapes of bar
  1.
- **Bars 1 and 2**: `scripts/decode_audio_check.py` per MP4 input (the
  model from `ffmpeg -ignore_editlist 1` and the edit list `Facts` reads;
  the entries of `docs/known-divergences-decode-audio.md` against `ffmpeg`'s
  default output), the three decodes compared in one streaming pass.
- **The mutation audit** runs on every MP4 input with an edit list whose full
  decode is at most 256 MiB (`AUDIT_LIMIT`; the audit holds the decodes in
  memory), all fixtures included. An entry's verdict depends on the edit
  list's shape and the packets, not on the input's length, so the run also
  reports whether the entries applied to each input above the limit, taken
  together, also apply to an audited input. Fixed here before the run.
- **Bar 3**: every Matroska input, and every MP4 input without an edit list,
  decoded at 32 bits (FLAC) or as f32 (MP3) by the binary of `18e6178`, the
  commit before the change, and by the change's, compared by SHA-256.
- **Bar 4** (`edit_perf.py`): the derived MP4 records of
  `evidence/flac-agree.json` and `evidence/mp3-agree.json` with status ok,
  in their order, every 4th from the first (65 FLAC and 65 MP3 inputs), the
  FLAC ones remuxed from their Matroska files by stream copy, measured by
  `perf.py` with its method on the change's binaries, after the other runs
  and on an otherwise idle machine. Bars: the library and command medians at
  most 1.00 for each codec, with the share of inputs faster.
- `edit_check.py` runs bars 1 to 3 on both sets and the fixtures (the
  derived FLAC MP4s remuxed from their Matroska files by stream copy, as for
  `evidence/flac-mp4-recheck.json`) and writes `evidence/edit-lists.json`.

### Results (edit lists)

| Bar | Result | Status |
|---|---|---|
| 1. Model: output and `timeline` equal the presentation model, every MP4 input `ffmpeg` decodes without an error | **560 of 560** (FLAC 258 derived, 6 corpus, 6 fixtures; MP3 258 derived, 26 corpus, 6 fixtures); 552 with an edit list, all applied | **Met** |
| 2. Every difference from `ffmpeg`'s default output covered by an entry; audit 0 covered | 174 equal, **386 covered, 0 uncovered**; audit **5,132 mutations caught, 0 covered, 0 missed** on 466 inputs | **Met** |
| 3. Matroska and MP4 inputs without an edit list unchanged | **606 of 606** byte-identical to `18e6178` | **Met** |
| 4. FLAC, MP4, 65 held-out inputs: library / command median at most 1.00 | library **0.898** (95% CI 0.868 to 0.932), command **0.591** (0.562 to 0.616) | **Met**, faster |
| 4. MP3, MP4, 65 held-out inputs: library / command median at most 1.00 | library **0.919** (0.881 to 0.940), command **0.194** (0.145 to 0.264) | **Met**, faster |

Share of inputs where CoreAV is faster (bar 4,
`evidence/edit-lists-flac-perf.json`, `evidence/edit-lists-mp3-perf.json`):
FLAC library 86.2%, command 98.5%; MP3 library 92.3%, command 100%. The
slowest inputs: FLAC library 1.149, command 1.005; MP3 library 1.113. Before
the change the MP4 library medians were 0.887 (FLAC, 258 inputs, the
original encodes rather than remuxes) and 0.947 (MP3, 254 inputs): no cost
of the timeline shows, though these are other runs on a superset, not a
paired comparison.

- **Entries applied** (inputs): `KD-A-EDIT-END` 385 (every `ffmpeg`-written
  MP4 whose millisecond edit ends inside the last packet: 105 FLAC, 252 MP3,
  16 MP3 corpus, 12 fixtures), `KD-A-LATER-EDIT-START` 7,
  `KD-A-LEADING-EMPTY` 4, `KD-A-INNER-EMPTY` 2. The fixtures use all four,
  each as observed in the probes of an earlier change.
- **Not compared** (34 inputs, all listed apart in the FLAC or MP3
  agreement run with the same reason): MP4 inputs CoreAV refuses by a
  recorded choice (the `ms\0U` entry, MPEG-1 Layer II under an MP3 tag,
  truncated files the demuxer rejects, IAMF and encrypted tracks) or that
  `ffmpeg` cannot decode, and 3 Matroska files neither binary decodes. One
  derived FLAC source does not remux into MP4, as in the recheck.
- **The audit** ran on 466 of the 552 inputs with an edit list; 86 have a
  full decode above 256 MiB, and the entries applied to each of them also
  apply, in the same combination, to an audited input. 29 mutations, all
  of an MP3 edit list misread by one tick of media time (a shift of one
  sample) in quiet audio, compare equal to the original within the MP3
  tolerance, so neither bar can see them, with or without the entries;
  they are reported apart, not judged.
- **Run 1** (`evidence/edit-lists-run1.json`): the same bars 1 to 3
  results, but the MP3 audit's mutations were too small to test anything:
  a one-bit change of a float sample, and those one-sample shifts, stayed
  inside the MP3 tolerance (356 reported covered, 33 missed). The float
  mutation now moves a sample by 0.25, and mutations the comparison cannot
  tell from the original are reported apart (`fdbac13`); the MP3 inputs
  were rerun (run 2, the record), the FLAC records kept, their audit code
  unchanged for integer samples.

## Resampling and downmixing

Hypothesis: `decode-audio --rate` and `--channels` convert as `ffmpeg`'s
resampler does in the band where both filters are designed flat, at the
lengths the design note gives, without moving any other output, and with
`--rate 16000 --channels 1` at least as fast as FFmpeg. The bars are
"Resampling bars" 1 to 7 in the design note, fixed in `b5c7245` with the
code, before every run here. Run 1 measured that commit's binaries; rounds
2 and 3, new code for the speed arms the run before missed, are fixed below
before their runs. The commits named here (`b5c7245`, `8949d3b`, `10e2058`)
are those the measured binaries were built from, on
before its rebase onto `main`, which changed no audio code.

- **Inputs**: the FLAC and MP3 sets of "Inputs", rebuilt from corpus v1 by
  `make_inputs.py` and `make_mp3_inputs.py` (`evidence/resample-flac-inputs.json`,
  `evidence/resample-mp3-inputs.json`): the same inputs and counts as the
  recorded sets. Matroska files differ in bytes (the muxer writes random
  segment UIDs), as do 2 FLAC and 1 MP3 MP4 file; MP4 files are otherwise
  byte-identical. The scripts match the recorded agreement runs' records
  to the rebuilt files by set, source and class.
- **Bars 2 and 3** (`resample_agree.py`): per input and conversion, four
  processes side by side and read in step: CoreAV's stream as decoded
  (`--format f32`), piped into `ffmpeg`'s conversion (the bar's oracle);
  CoreAV's conversion; and `ffmpeg` decoding and converting the file
  (bar 3). Inputs: the agreement runs' records with status ok and no
  `ffmpeg` error. Hann blocks of 32,768 samples, half overlapping, the last
  block aligned to the end. For MP3 in Matroska, bar 3's `ffmpeg` command
  adds `-af atrim=start_sample=D` (D the `CodecDelay` in samples), as the
  MP3 agreement run aligns it, since `ffmpeg` does not apply it; otherwise
  the commands are the bar's.
- **Bar 4** (`resample_same.py`): every input of both rebuilt sets, the
  default WAV and `--format f32` without `--rate` or `--channels`, by the
  binaries of `212915b` (the commit before) and `b5c7245`, compared by
  SHA-256 and exit code.
- **Bar 5** (`resample_perf.py`), run 1: `perf.py --every 4`'s inputs
  (every 4th record with status ok of the agreement run, from the first),
  of which the held-out derived ones, with `perf.py`'s method, after the
  other runs and on an otherwise idle machine. The records alternate
  between a source's Matroska and MP4 files, so this stride kept MP4 files
  only (129 per codec): the Matroska class went unmeasured in run 1.

### Results (resampling)

| Bar | Result | Status |
|---|---|---|
| 2. Every channel E_rel at most -70 dB or E_abs at most -100 dB, length ceil(n × to / from) | FLAC **527 of 527** for (a), (b), (c), **109 of 109** for (d); MP3 **607 of 607** for (a), (b), (c), **217 of 217** for (d) | **Met** |
| 4. Default `s16` and `f32` unchanged | FLAC **535 of 535**, MP3 **643 of 643** inputs byte-identical to `212915b` (including the exit code of those CoreAV refuses) | **Met** |
| 5. Speed, run 1 (MP4 inputs only), median ours / FFmpeg at most 1.00 | (a) library FLAC **0.960**, MP3 **1.007**; (a) command FLAC **0.815**, MP3 **0.402**; (b) command FLAC **1.014**, MP3 **0.603** | **Not met** for MP3 (a) library and FLAC (b) command; Matroska not measured. Round 2 below |
| 5. Speed, round 2 (`8949d3b`, both classes), overall and per class | (a) library FLAC **0.989** (MP4 **1.007**, Matroska 0.984), MP3 **0.963**; (a) command FLAC **0.691**, MP3 **0.378**; (b) command FLAC **0.831**, MP3 **0.438** | **Not met** for FLAC (a) library, MP4 class; every other arm and class met |
| 5. Speed, round 3 (`10e2058`, held-out sources, both classes), overall and per class | (a) library FLAC **0.914** (MP4 0.922, Matroska 0.909), MP3 **0.935**; (a) command FLAC **0.640**, MP3 **0.348**; (b) command FLAC **0.881**, MP3 **0.528** | **Met**, every gating arm and class |

Agreement with `ffmpeg`'s conversion of the same samples (bar 2), the
worst channel per input:

| Conversion | FLAC E_rel median / worst | MP3 E_rel median / worst | E_abs worst (FLAC, MP3) |
|---|---|---|---|
| (a) 16 kHz mono | -91.4 / -87.2 dB | -91.4 / -87.2 dB | -99.6, -99.9 dB |
| (b) 48 kHz | -96.2 / -89.5 dB | -96.5 / -89.7 dB | -104.1, -104.4 dB |
| (c) mono | -143.1 / -141.4 dB | identical | -174.2 dB, identical |
| (d) stereo (not stereo inputs) | -140.2 / -139.8 dB | identical | -170.0 dB, identical |

- (b) on a 48 kHz input and (c) on a mono input change nothing, in both
  programs (identical output). (c) and (d) on MP3 are bit-identical to
  `ffmpeg`'s: its inputs are mono or stereo, whose gains (0.5 and 0.5;
  0.7071 for mono to stereo) round the same. The 5.1 FLAC inputs (9) differ
  by the float rounding of BS.775's gains. No input has a back centre or
  top back centre position, so none is listed apart.
- Lengths: CoreAV's is ceil(n × to / from) on every input. `ffmpeg`'s is
  the same for (b), and one sample shorter for (a) on 134 FLAC and 160 MP3
  inputs (it rounds the end differently).
- Not counted (the agreement runs' records, rechecked with this binary):
  FLAC 4 refused by recorded choice (IAMF, encrypted), 2 `ffmpeg` errors,
  2 without a FLAC stream; MP3 30 refused (as listed in the MP3 agreement
  run) and 6 with `ffmpeg` errors.

End to end (bar 3, reported): `ffmpeg` decoding and converting the file.

- f32, the bar 2 rule applied: FLAC (a) 476 of 526 (one stream is empty),
  MP3 (a) 604 of 607, the same counts for (b) and (c). The others are
  timeline differences the edit-list and codec bars record: 50 FLAC MP4
  files whose stream starts after time 0, which CoreAV fills with silence
  (`timeline.silence`) and `ffmpeg` drops (its output is 44 to 1,077,216
  samples shorter, misaligned from the start); and three MP3 inputs: an
  MP4 file whose `ffmpeg` output with `-ar` is 3,840 samples shorter, the
  MOV file whose edit list CoreAV does not apply, and the MPEG-2 stream with
  1-bit intensity positions (its right channel at -10.7 dB). On the inputs
  whose timelines agree the medians equal bar 2's (FLAC decodes
  bit-exactly; MP3 adds the decoders' difference, (c) -122.6 dB).
- The default `s16` WAV of (a) against `ffmpeg -ar 16000 -ac 1 OUT.wav`,
  over the band after a linear-phase low-pass (1,023 taps, flat within
  0.001 dB to 0.80 of Nyquist, 87 dB down from 0.82), away from the first
  and last 16 output samples: on the inputs whose timelines agree, within
  one step on 53 of 466 FLAC and 37 of 338 MP3 inputs, 1 to 8 steps on 366
  and 263, more on 47 and 38 (largest 70 steps FLAC; 4,180 MP3, the
  intensity-position divergence). Not a conversion error: `ffmpeg`'s `s16`
  output is its `f32` output rounded (black box), and two filters each flat
  within 0.002 dB may differ by 2.3 × 10^-4 of a component's level, which
  for a component near full scale at the band's edge is 7 steps; bar 2
  allows for exactly this (-73 dB). A first pass with a shorter low-pass
  (255 taps, transition 0.83 to 0.87) gave 50 and 36 inputs within one
  step; the longer one measures the band the bar names.
- At the edges the outputs differ by up to a third of full scale: CoreAV's
  filter sees silence before the first and after the last sample (design
  note), and `ffmpeg` extends the signal; a sine starting at 0 gives 152
  against 304 in the first 16 kHz sample, the same values from sample 13
  on.
- The window: Hann blocks give the first and last half block little
  weight, so bar 2 does not see those edges; the `s16` measure does.

Speed, run 1 (`evidence/resample-flac-perf-run1.json`,
`evidence/resample-mp3-perf-run1.json`, MP4 files only):

| Pair | FLAC median (95% CI) | faster | MP3 median (95% CI) | faster |
|---|---|---:|---|---:|
| (a) library, gates | 0.960 (0.930 to 0.978) | 62.8% | **1.007** (0.979 to 1.056) | 48.8% |
| (a) command, gates | 0.815 (0.744 to 0.866) | 79.1% | 0.402 (0.352 to 0.450) | 97.7% |
| (b) command, gates | **1.014** (0.901 to 1.069) | 47.7% | 0.603 (0.534 to 0.693) | 93.8% |
| (b) library, reported | 1.143 (1.097 to 1.171) | 21.7% | 1.162 (1.084 to 1.197) | 25.6% |

129 inputs per codec (FLAC (b) command 128: one 3-hour input's 48 kHz WAV,
5.8 GiB, did not fit the 4 GiB tmpfs, and both programs failed). The two
misses are final for that code. Where they came from: the MP3 (a) library
pair was 0.93 to 1.02 on stereo inputs and 1.04 to 1.33 on mono ones, where
the conversion is all resampling; the FLAC (b) command was 0.39 to 0.69 on
48 kHz inputs (nothing to resample) and 1.15 to 1.45 on 44.1 kHz ones,
whose resampling runs on the caller's thread while FLAC decodes on all of
them. Profiles of our side (CoreAV only) put the filter loop at 17 to 20% of the time
and the loop that lists the filter windows at 4 to 16%.

### Round 2 (resampling speed)

Fixed here before its run. The change, in `coreav-kernels-audio` and
`resample.rs`, keeps every output bit for bit (each output's products are
added in the same order):

- the AVX loop computes four outputs in one loop, so their additions
  overlap instead of each waiting for the one before (microbenchmark on
  this machine, ns per output: 32 taps 6.1 before, 4.1 to 4.4 after; 96
  taps 17.1 before, 9.2 to 9.5 after, with the next point);
- the coefficient table starts on a 32-byte boundary, so no 8-float load of
  a row straddles a cache line (about 15% of the loop's time);
- the filter windows repeat every period of `phases` outputs, `step` input
  samples later, so they are listed from one stored period instead of output
  by output, and the kernel checks them in one branch-free pass.

Evidence that the output is unchanged (`resample_same.py --conversions`,
`evidence/resample-*-same-conversions.json`): the f32 output of (a) to (d) and the s16 WAV of (a) are byte-identical to
`b5c7245`'s on every input, FLAC 535 of 535 and MP3 643 of 643 (exit codes
included). Bars 2 to 4
therefore hold for the new code as measured above.

Round 2 measures bar 5, every arm, with the bars and method of run 1, on
the new code's binaries: every 4th derived source of each set's agreement
records (in their order, from the first) with both its MP4 and its
Matroska file (`resample_perf.py --sources-every 4`; 65 sources, 130 files
per codec), the tmpfs at 8 GiB. Outputs: `evidence/resample-flac-perf-round2.json`,
`evidence/resample-mp3-perf-round2.json` (named `resample-*-perf.json` until round 3). The run 1 misses stay recorded; round 2
decides the bars for the code that merges.

Results, round 2 (`evidence/resample-flac-perf-round2.json`,
`evidence/resample-mp3-perf-round2.json`; 130 inputs per codec, 65 per class,
none failed):

| Pair | Codec | All (95% CI) | MP4 | Matroska | Faster |
|---|---|---|---:|---:|---:|
| (a) library, gates | FLAC | 0.989 (0.966 to 1.019) | **1.007** | 0.984 | 53.1% |
| (a) library, gates | MP3 | 0.963 (0.941 to 1.000) | 0.976 | 0.957 | 58.5% |
| (a) command, gates | FLAC | 0.691 (0.644 to 0.729) | 0.682 | 0.698 | 85.4% |
| (a) command, gates | MP3 | 0.378 (0.334 to 0.436) | 0.387 | 0.370 | 99.2% |
| (b) command, gates | FLAC | 0.831 (0.735 to 0.906) | 0.838 | 0.826 | 69.2% |
| (b) command, gates | MP3 | 0.438 (0.371 to 0.531) | 0.453 | 0.435 | 100% |
| (b) library, reported | FLAC | 1.106 (1.095 to 1.132) | 1.106 | 1.105 | 15.4% |
| (b) library, reported | MP3 | 1.019 (1.003 to 1.045) | 1.029 | 1.017 | 40.8% |

The two arms run 1 missed are met. FLAC (a) library misses on its MP4
class (1.007; overall 0.989, Matroska 0.984), final for this code; run 1
had 0.960 for this arm on other MP4 files (every other source's), so the
arm sits near 1.00 and the input sample decides it. By stream: 44.1 kHz
mono 1.151, 44.1 kHz stereo 1.057, 48 kHz 0.96 to 1.00, 22.05 kHz 0.94 to
0.99. A profile of our side on a 44.1 kHz stereo input: FLAC decoding 72%,
the filter loop 10%, writing the stream as 32-bit floats and the output as
16 bits 9% (no vector path for integer samples to floats), the resampler's
table 2% (a fixed cost, larger on short inputs).

### Round 3 (resampling speed)

Fixed here before its run. New code for the arm round 2 missed, FLAC (a)
library. Round 2's per-input results put the losses on short 44.1 kHz
inputs, and a profile of our side found three costs outside FLAC decoding:

- the filter table, built once per stream (160 rows of 96 taps for 44.1 to
  16 kHz: 1.86 ms, about 15% of a 10-second input), now evaluated eight
  taps at a time with I0(β) computed once: 0.84 ms, every tap the same
  value bit for bit (a unit test checks 360,000 points against the
  one-at-a-time series);
- FLAC's integer samples to the stream's 32-bit floats had no vector loop:
  one or two channels now convert in single precision, which gives the
  same value (the scale is a power of two; a unit test covers every bit
  depth), and the conversion sink reads one or two channels back with the
  frame's size known to the compiler;
- with one phase (48 to 16 kHz) the filter windows were listed one output
  per loop iteration; they are now listed in one loop.

Development A/B (this machine, inputs of round 2's sample only, not a
bar): our time against round 2's binary 0.89 to 0.96 on inputs near
round 2's median.

Evidence that the output is unchanged (`resample_same.py`, the binaries
of `b5c7245` and of this round's code; `evidence/resample-*-same-conversions-round3.json`
and `evidence/resample-*-same-round3.json`): the f32 output of (a) to (d)
and the s16 WAV of (a), and the default s16 WAV and `--format f32`
without conversion, are byte-identical on every input, FLAC 535 of 535
and MP3 643 of 643 (exit codes included). Bars 2 to 4 therefore hold for
the new code as measured in run 1.

Round 3 measures bar 5 with the bars and method of round 2 on the new
code's binaries (`10e2058`), every gating arm ((a) library, (a) command, (b) command)
on both codecs, overall and per class, (b) library reported. Bars
unchanged: each gating arm's median per-input ratio, ours / FFmpeg, at
most 1.00 overall and in each class. The inputs change, so that the
measured inputs were not used in development: every 4th derived source
from the third (`resample_perf.py --sources-every 4 --sources-offset 2`),
with both its MP4 and Matroska file, 64 sources and 128 files per codec.
Run 1 measured the odd sources' MP4 files and round 2 every 4th source
from the first; these 64 sources were measured by neither and no file of
theirs was used in development. Outputs: `evidence/resample-flac-perf.json`,
`evidence/resample-mp3-perf.json`. A miss is final for this code and is
recorded.

Results, round 3 (`evidence/resample-flac-perf.json`,
`evidence/resample-mp3-perf.json`; 128 inputs per codec, 64 per class,
none failed):

| Pair | Codec | All (95% CI) | MP4 | Matroska | Faster |
|---|---|---|---:|---:|---:|
| (a) library, gates | FLAC | **0.914** (0.899 to 0.940) | 0.922 | 0.909 | 82.0% |
| (a) library, gates | MP3 | **0.935** (0.915 to 0.958) | 0.940 | 0.917 | 73.4% |
| (a) command, gates | FLAC | **0.640** (0.603 to 0.691) | 0.640 | 0.640 | 91.4% |
| (a) command, gates | MP3 | **0.348** (0.297 to 0.429) | 0.343 | 0.351 | 100% |
| (b) command, gates | FLAC | **0.881** (0.839 to 0.950) | 0.865 | 0.891 | 63.3% |
| (b) command, gates | MP3 | **0.528** (0.488 to 0.553) | 0.532 | 0.520 | 100% |
| (b) library, reported | FLAC | 1.070 (1.055 to 1.091) | 1.086 | 1.061 | 18.8% |
| (b) library, reported | MP3 | 1.048 (1.031 to 1.065) | 1.052 | 1.047 | 31.2% |

Every gating arm is met overall and in each class, each with the upper
end of its interval below 1.00: bar 5 is met for the code that merges.
The (b) library pair, reported, is 1.070 and 1.048 (round 2: 1.106 and
1.019 on other sources); its per-sample filter cost stays the next speed
item, as the design note says.

## Inputs

`make_inputs.py` builds the derived set of the design note: every `real`
corpus v1 file with `holdout: true` and an audio stream (263), its first
audio stream encoded by the pinned `ffmpeg` 7.0.2 to FLAC at the level and
bit depth its SHA-256 picks, into Matroska and MP4. 5 sources gave no input
(2 without an audio decoder in `ffmpeg`, 2 whose audio `ffmpeg` cannot
decode, 1 whose encoded files hold no packets), so the derived set has 258
sources, 516 files. The corpus set is the 17 corpus v1 FLAC files in Matroska and MP4.
`evidence/flac-inputs.json` lists every input with its parameters and
hashes.

`make_mp3_inputs.py` builds the MP3 sets: the derived set from the FLAC
derived set's Matroska files (the sources' first audio streams, decoded),
encoded by the pinned `ffmpeg` with `libmp3lame` at the mode, channels
and rate the source's SHA-256 picks, into Matroska and MP4: 258 sources,
516 files (5 sources without a FLAC input). The corpus set is the 127
corpus v1 files with an MP3 stream, 68 in MP4, MOV, F4V or Matroska as
stored and 59 raw MP3 files remuxed into Matroska by stream copy.
`evidence/mp3-inputs.json` lists them with their parameters and hashes.

## Method

- Machine: the development machine (8 vCPU), otherwise idle during the speed
  run reported here; sources in the page cache, outputs on a tmpfs.
- `agree.py`: per input, `coreav decode-audio --format s32` against `ffmpeg
  -map 0:K -f s32le`, and the data chunk of `coreav decode-audio`'s default
  WAV against `ffmpeg -map 0:K -f wav`'s, both through SHA-256 of the
  streamed output.
- `perf.py`: per input one warm-up run of every engine, then 5 rounds with
  the engines' order rotated; an input's time per engine is the median of
  its rounds. Engines: `abench lib` (`coreav_audio::decode` to s16 in
  memory, one thread) against `lav_audio decode` (libavformat 7.0.2,
  libavcodec, `swr_convert` to s16, `thread_count = 1`), the gating
  library pair; `coreav decode-audio IN OUT.wav` against `ffmpeg -i IN -map
  0:K OUT.wav`, the gating command pair, timed from outside; and, reported,
  `lav_audio decode-info` (after `avformat_find_stream_info`, as the
  command does) and the decoders alone (`abench decoder`, `lav_audio
  decoder`, packets already in memory).
- `summarize.py`: medians of the per-input ratios, overall and per class,
  with a bootstrap 95% interval (10,000 resamples, seed 206).
- The baseline `baseline/lav_audio.c` is written from the FFmpeg 7.0
  public API documentation and built against the pinned LGPL package of
  Spike C; no FFmpeg source
  was read.

## Runs

Every run measured with the same bars; the design changed between runs 1
to 3, and run 3 is the measurement of record.

1. **Stopped after 21 inputs** (`evidence/flac-perf-run1-partial.json`):
   the command decoded on one thread, and `ffmpeg` decodes FLAC on several,
   so on long inputs the command took 2 to 3.4 times `ffmpeg`'s time (the
   library pair, one thread each, was at 0.88 to 0.93). Frame-parallel
   decoding was added.
2. **Stopped after 45 inputs** (`evidence/flac-perf-run2-partial.json`):
   decoding in batches, written between batches, left the threads idle
   while the caller's thread wrote; partial medians already met both bars
   (command 0.53, library 0.88) but the slowest long inputs reached 1.31.
   The batches became a pipelined pool.
3. **All inputs** (`evidence/flac-perf.json`): commit `66d50b7`.
4. **A check after the review fixes** (`evidence/flac-perf-check.json`,
   `perf.py --every 13`, 40 held-out inputs; not a bar): the fixed
   predictor's bounded history and the pool's byte bound changed the code
   after run 3. On the same 40 inputs, run 3 gave library 0.869 and command
   0.630; the shipped code gives 0.876 and 0.638, within run-to-run noise.
   The agreement run was repeated on the shipped binary: 527 of 527.

## Reproduce

    python3 -m venv venv && venv/bin/pip install boto3
    venv/bin/python -I make_inputs.py --out DIR          # R2 credentials in the environment
    cargo build --release                                 # abench
    cc -O2 -o work/lav_audio baseline/lav_audio.c -I$LAV/include -L$LAV/lib \
       -Wl,-rpath,$LAV/lib -lavformat -lavcodec -lswresample -lavutil
    cargo build --profile dist -p coreav-cli --manifest-path ../../Cargo.toml
    venv/bin/python -I agree.py flac --inputs evidence/flac-inputs.json --media DIR \
       --coreav ../../target/dist/coreav --out evidence/flac-agree.json
    venv/bin/python -I perf.py --agree evidence/flac-agree.json --inputs evidence/flac-inputs.json \
       --media DIR --abench target/release/abench --lav work/lav_audio \
       --coreav ../../target/dist/coreav --outputs /mnt/tmpfs/ab --out evidence/flac-perf.json
    python3 summarize.py evidence/flac-perf.json
    # MP3: the derived set from the FLAC set's Matroska files, the corpus from R2
    venv/bin/python -I make_mp3_inputs.py --flac FLACDIR --out DIR
    venv/bin/python -I agree.py mp3 --inputs evidence/mp3-inputs.json --media DIR \
       --coreav ../../target/dist/coreav --out evidence/mp3-agree.json
    venv/bin/python -I perf.py --agree evidence/mp3-agree.json --inputs evidence/mp3-inputs.json \
       --media DIR --abench target/release/abench --lav work/lav_audio \
       --coreav ../../target/dist/coreav --outputs /mnt/tmpfs/ab --out evidence/mp3-perf.json
    python3 summarize.py evidence/mp3-perf.json
    # Resampling: the sets rebuilt as above; BEFORE and AFTER two builds of coreav
    venv/bin/python -I resample_agree.py flac --agree evidence/flac-agree.json \
       --inputs evidence/resample-flac-inputs.json --media DIR --coreav ../../target/dist/coreav \
       --out evidence/resample-flac-agree.json      # then --s16-only for the s16 report
    venv/bin/python -I resample_same.py --inputs evidence/resample-flac-inputs.json --media DIR \
       --before BEFORE --after AFTER --out evidence/resample-flac-same.json   # [--conversions]
    venv/bin/python -I resample_perf.py --agree evidence/flac-agree.json \
       --inputs evidence/resample-flac-inputs.json --media DIR --abench target/release/abench \
       --lav work/lav_audio --coreav ../../target/dist/coreav --outputs /mnt/tmpfs/ab \
       --out evidence/resample-flac-perf.json --sources-every 4 --sources-offset 2
    python3 summarize.py evidence/resample-flac-perf.json   # the same for mp3

`$LAV` is the prefix of the pinned LGPL package.

## Opus

The bars of the design note (`docs/design/audio-decode.md`, "Opus bars"
and "Opus sets"). Method, fixed before the runs:

- `make_opus_inputs.py` builds the three sets by the rule of "Opus sets"
  with the pinned `ffmpeg` 7.0.2 and the libopus it links. The record keeps,
  per input, its parameters, its hash and the mode and frame size of every
  packet's TOC byte (read from an Ogg remux by stream copy).
- `agree_opus.py`, bar 2: per input, its first Opus stream through `coreav
  decode-audio --format s16` against libopus's float output (`ffmpeg
  -request_sample_fmt flt -c:a libopus -i IN -map 0:K -f f32le -`) rounded
  to 16-bit with hard clipping. Matroska and WebM: the lengths equal, and passing
  `opus_compare -r 48000` (`-s` for two channels, each channel alone above
  two; a mono reference is given as two equal channels, since
  `opus_compare` reads its first file as stereo). MP4: the edit list is
  the timeline (design note, "Edit lists"), so CoreAV's output is held to
  the presentation model of `scripts/decode_audio_check.py`: libopus's decode of
  every sample (`-ignore_editlist 1`, which drops the pre-skip), as above, cut by the edit
  list as `scripts/oracle_kd.py` reads it, the priming samples never
  presented. The length must equal the model's and `opus_compare` must
  pass against it; `ffmpeg`'s own output must be as long as the entries of
  `docs/known-divergences-decode-audio.md` give for the list, so a length
  difference from `ffmpeg` is a recorded divergence, never a tolerance.
  Reported against `ffmpeg`'s default decoder (`opus`): within
  `opus_compare`, the SNR and the largest difference in 16-bit steps, over
  the samples both write from the start.
- Hosts: the agreement run, not timed, on one GCP `c4-highcpu-48`, and
  the inputs its time limit left unrun, resumed from its results with the
  same binaries, on one `c4-standard-16` and then one `n2-highmem-48`. The speed run, `perf.py` as
  for FLAC and MP3 (5 rounds after a warm-up, outputs on a tmpfs), against
  `lav_audio` built against the LGPL package of FFmpeg 9.0.2 (conda-forge
  `ffmpeg-9.0.2-lgpl_hceb5412_805`) and the pinned `ffmpeg` 9.0.2
  (`n9.0.2-17-g2a571b6068`), on GCP `c4-standard-4` VMs with simultaneous
  multithreading off (2 cores, one vCPU each), each otherwise idle: the
  inputs are split into shards of about equal decode time, one VM a shard,
  and each VM rebuilds its shard's inputs with `make_opus_inputs.py` and
  checks their hashes against the agreement run's record. The per-mode
  report puts each input in the mode most of its packets carry.

## Results at a glance (Opus)

| Bar | Result | Status |
|---|---|---|
| Conformance: the 24 decodes of the RFC 8251 vectors | All 24, in CI (`.github/workflows/audio-conformance.yml`) | **Met** |
| Agreement with libopus on every input `ffmpeg` decodes without an error | **606 of 607** counted (504 of 504 derived, 90 of 90 corpus, 12 of 13 multichannel); lowest quality 99.7; 4 derived MP4 inputs not run | **Not met**: 1 input, below; 4 inputs not run |
| Library, median ours / libav at most 1.00, held-out, overall and per class | **1.002** (95% CI 0.992 to 1.009), 512 inputs; MP4 1.004, Matroska 0.997 | **Not met**: overall and MP4 |
| Command, median ours / `ffmpeg` at most 1.00, held-out, overall and per class | **0.523** (95% CI 0.505 to 0.543), 512 inputs; MP4 0.523, Matroska 0.523 | **Met**, faster |

Share of held-out inputs where CoreAV is faster: library **49.2%** (MP4
46.5%, Matroska 51.9%), command **94.7%**. The command's interval ends
below 1.00, so the command is faster than `ffmpeg` on these inputs; the
library pair is level with libavcodec's `opus` decoder, and CELT is where
it loses (below).

## Speed (`evidence/opus-perf.json`, derived set, held out)

| Pair | Class | Inputs | Median | 95% CI | Faster (ratio < 1.00) | Max |
|---|---|---:|---:|---|---:|---:|
| library (gates) | all | 512 | 1.002 | 0.992 to 1.009 | 49.2% | 1.264 |
| library (gates) | mp4 | 254 | 1.004 | 0.993 to 1.015 | 46.5% | 1.249 |
| library (gates) | matroska | 258 | 0.997 | 0.980 to 1.010 | 51.9% | 1.264 |
| command (gates) | all | 512 | 0.523 | 0.505 to 0.543 | 94.7% | 1.065 |
| command (gates) | mp4 | 254 | 0.523 | 0.496 to 0.574 | 94.9% | 1.062 |
| command (gates) | matroska | 258 | 0.523 | 0.499 to 0.551 | 94.6% | 1.065 |
| library vs `find_stream_info` (reference) | all | 512 | 1.017 | 1.006 to 1.022 | 42.6% | 1.263 |
| decoder alone (reported) | all | 512 | 1.059 | 1.052 to 1.063 | 23.0% | 2.060 |

By mode (the mode most of an input's packets carry; reported, not
gating), medians:

| Mode | Inputs | Library | Command | Decoder alone | Library faster |
|---|---:|---:|---:|---:|---:|
| SILK | 60 | 0.777 | 0.423 | 0.784 | 100.0% |
| Hybrid | 66 | 0.943 | 0.612 | 1.033 | 57.6% |
| CELT | 386 | 1.015 | 0.520 | 1.069 | 39.9% |

The CELT decoder is the next speed item for this crate: 6.7% of the CELT
inputs decode faster than with libavcodec's decoder in the decoder-alone
pair, and the library pair's lead elsewhere comes from the pipeline around
the decoder. By bitrate, the library pair runs from 0.794 at 12 kbit/s
(SILK) to 1.045 at 128 kbit/s and 1.037 at 256 kbit/s.

The 512 held-out inputs are the derived inputs bar 2 passed: the 4 MP4
inputs bar 2 did not run are not timed, and the 8 listed apart for
`ffmpeg`'s message (below), which passed, are. The shards rebuilt every
input: 604 of 604 have the agreement run's packets and 345 its bytes; the
others are the 258 Matroska files, whose muxer writes a random segment UID,
and one MP4 file. Hosts: 8 `c4-standard-4` VMs, SMT off, all Intel Xeon
Platinum 8581C. The corpus set (92 inputs; not held out, gates nothing)
gives library 1.035 (32.6% faster) and command 0.812 (96.7%)
(`summarize.py --set corpus`). Commit of the measured binaries: `22a0a9d`.

## Agreement (`evidence/opus-agree.json`)

- 630 inputs (516 derived, 14 multichannel, 100 corpus; the corpus's 4 Ogg
  files are not read yet, and 2 derived encodes have no audio stream). 626
  have a result; 607 are counted and 606 pass, the lowest quality 99.7. MP4
  lengths match the presentation model through the entries `KD-A-EDIT-END`
  and `KD-A-LEADING-EMPTY`.
- The miss: the multichannel input of mapping family 255 at 3 channels
  (`d51c01fc…`). Family 255 gives no channel layout, so its channels have
  no order beyond the coded one; libopus, through `ffmpeg`, reorders 3
  channels as Vorbis does (channels 1 and 2 swapped), while CoreAV and
  `ffmpeg`'s default `opus` decoder give them in coded order and agree
  channel for channel. With the channels matched it passes (quality 99.9,
  at most 1 step). Recorded as a miss, with the decoder and the method
  unchanged.
- Not run: 4 derived MP4 inputs of 1.6 to 3.8 hours each (`3d8f6047…`,
  `6024f3b8…`, `9af8b16f…`, `b7d3b1ab…`; `not_run` in the record),
  which the last resume's time limit left. Their Matroska counterparts, the
  same encodes, pass.
- Listed apart, as the bar says (19): 10 corpus inputs `ffmpeg` decodes
  with errors; 2 corpus MP4 inputs refused by name (IAMF); 9
  inputs (8 derived Matroska, 1 multichannel) whose only message is the
  `f32le` muxer's on the reference side, a timestamp equal to the one
  before it. All 9 of those pass.
- Against `ffmpeg`'s default decoder (`opus`; reported): 549 of 607
  within `opus_compare`, every length equal, SNR median 92.5 dB, lowest
  -0.95 dB, largest difference 49,677 steps.

## Runs (Opus)

1. **Agreement run 1: crashed**, no record.
2. **Agreement run 2**: stopped by its time limit, then resumed from its
   results by three runs on the same binaries, the second of them
   preempted. Results are keyed by set, source and class (a rebuilt
   Matroska file differs in its segment UID only). After the first resume,
   run 2's corpus copies were found to take their source's hash as file
   name, so a corpus file that is also a derived source had been
   overwritten by its derived WebM: 17 of run 2's corpus results and 18 of the first resume's were on
   the wrong file. They are dropped (`dropped` in the record) and those
   inputs measured again with the copies named `corpus-<hash>`.
3. **Speed run**: all 604 inputs, every engine without a failure.

## Opus round 2

The one judged run of round 2 (`docs/design/audio-decode.md`, "Opus speed, round 2", whose bars were committed before draw 8 was fetched), for the change that speeds up CELT decoding with the same decoded bits. Binaries: dist builds of the change's head `9419b72` and of its base `9f7dd93`. Set: the derived set of `v2-heldout` draw 8's 144 `real` files, one MP4 and one Matroska/WebM input each, built by `make_opus_inputs.py` with the pinned `ffmpeg` 7.0.2 (`evidence/opus-round2-inputs.json`). Hosts: 8 GCP `c4-standard-4` VMs, SMT off, all Intel Xeon Platinum 8581C; each built its shard's inputs, ran bar 2 and the identity check on every input, and timed those that passed both (`perf.py`, 5 rounds after a warm-up, outputs on a tmpfs, against `lav_audio` built against the LGPL package of FFmpeg 9.0.2 and the pinned `ffmpeg` 9.0.2). The run was launched twice: the first launch stopped on every VM at its CPU check, before building any input, because the image's `lscpu` prints the model name in capitals and the check's match was case-sensitive; the second launch, with the match made case-insensitive, is the run below.

| Bar | Result | Status |
|---|---|---|
| Same bits: head and base `coreav decode-audio --format s16` outputs equal byte-for-byte | **288 of 288** identical, no error (`evidence/opus-round2-identity.json`) | **Met** |
| Minimum: at least 90 timed inputs per class, no binary error | MP4 144, Matroska/WebM 144; no engine failed on any input | **Met** |
| Library, median ours / libav at most 1.00, overall and per class | **0.994** (95% CI 0.976 to 1.003), 288 inputs; MP4 0.995, Matroska 0.991 | **Met** |

The intervals' upper ends are above 1.00, so the library pair is level with libavcodec's `opus` decoder on these inputs, not faster. Share of inputs where CoreAV is faster: library **53.1%** (MP4 52.8%, Matroska 53.5%), command **91.3%**.

| Pair | Class | Inputs | Median | 95% CI | Faster (ratio < 1.00) | Max |
|---|---|---:|---:|---|---:|---:|
| library (gates) | all | 288 | 0.994 | 0.976 to 1.003 | 53.1% | 1.137 |
| library (gates) | mp4 | 144 | 0.995 | 0.969 to 1.006 | 52.8% | 1.134 |
| library (gates) | matroska | 144 | 0.991 | 0.958 to 1.007 | 53.5% | 1.137 |
| command (reported) | all | 288 | 0.613 | 0.561 to 0.675 | 91.3% | 1.054 |
| library vs `find_stream_info` (reference) | all | 288 | 1.002 | 0.990 to 1.014 | 48.3% | 1.142 |
| decoder alone (reported) | all | 288 | 1.038 | 1.028 to 1.046 | 31.6% | 1.178 |

By mode (the mode most of an input's packets carry; reported, not gating), medians:

| Mode | Inputs | Library | Command | Decoder alone | Library faster |
|---|---:|---:|---:|---:|---:|
| SILK | 34 | 0.791 | 0.602 | 0.807 | 100.0% |
| Hybrid | 44 | 1.004 | 0.864 | 1.021 | 47.7% |
| CELT | 210 | 1.003 | 0.554 | 1.052 | 46.7% |

Bar 2 (`evidence/opus-round2-agree.json`): 288 inputs, 283 counted, all 283 pass, lowest quality 99.7; MP4 lengths match the presentation model through `KD-A-EDIT-END`. Listed apart (5): Matroska inputs whose only message is the reference side's `f32le` muxer, a timestamp equal to the one before it; all 5 pass, and as in round 1 they are timed. Over the 283 counted inputs alone the library medians are 0.993 overall, 0.995 MP4 and 0.988 Matroska. Against `ffmpeg`'s default decoder (reported): 234 of 283 within `opus_compare`, every length equal, SNR median 99.1 dB.

## Opus: the decoder-choice probe

Indicative, not a bar: one input of the choice between a native Opus
decoder and libopus (`docs/design/audio-decode.md`, "The decoder: native
or libopus"). On the 13 largest non-held-out corpus files with an Opus
stream that fit the disk (40 s and longer, the audio remuxed to `.mka`),
`opus_probe/ffmpeg_decoders.py` timed `ffmpeg -threads 1 -c:a opus` and
`-c:a libopus` (the oracle build links libopus 1.3.1, build flags
unknown): libopus took a median 3.0 times FFmpeg's own decoder. Because
that build may be the cause, `opus_probe/libopus_loop.c` timed the
distribution's libopus 1.3.1 (Amazon Linux 2023) decoding packets held
in memory, decode time only, on 5 of them: 0.85 to 1.08 of FFmpeg's
whole command with its own decoder. Record:
`evidence/opus-backend-probe.json`, with the commands that rebuild the inputs (`reproduce`). The probe has no warm-up run and one run per input above 1,000 s, and its two comparisons are unequal (the loop is spared demuxing, writing and process start): an indication, not a measurement.

## Spend

FLAC, MP3 and resampling: none; every run was on the development machine,
and the corpus was read from R2 (no egress charge). Opus: the runs above,
on GCP VMs.
