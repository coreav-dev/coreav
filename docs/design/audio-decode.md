# Audio decoding to PCM

Status: Proposed. Scope: "audio decoding with resampling and downmixing to
PCM and WAV, for transcription pipelines", a job of roadmap step 1, with the patent stance on each
codec. This note
covers the crate, the pipeline and the command, and each codec in the
order of the issue: FLAC, then MP3, then Opus, then AAC-LC, each with its
bars fixed here before its measured run, and resampling and downmixing
("Resampling and downmixing").

## What it does

    coreav decode-audio [--stream N] [--format s16|s24|s32|f32] [--rate HZ] [--channels N] [--threads N] [--force] INPUT OUTPUT

decodes one audio track of an MP4/MOV/M4A, Matroska or WebM file (local, or
at an HTTP(S) URL as for the other commands) to a WAV file, and prints a
`coreav.decode-audio/1` summary (`docs/schema/decode-audio-v1.md`). The
work is the library's: `coreav_audio::decode` with a `WavSink`. The
command follows the conventions of `coreav extract-audio`
(`docs/design/extract-audio-cli.md`): exit codes 0, 1, 2, 3 and 141,
`coreav.error/1` errors, OUTPUT `-` for standard output with the summary
on standard error, a temporary file renamed into place, `--force`.

## Crates

- **`coreav-audio`** (safe Rust, `forbid(unsafe_code)`, no third-party
  dependencies): the native decoders, the output conversion, WAV, and the
  pipeline over `coreav-isobmff` and `coreav-mkv`. Decoders are sans-IO:
  one codec frame per packet in, planar samples out, into a reused `Pcm`
  buffer, so a stream allocates once.
- **`coreav-opus`**: the Opus decoder, in its own crate for the notice ("Opus").
- **Codecs that need a C library** get a decoder-backend crate
  (`coreav-backend-*`) that `coreav-audio` uses behind a cargo
  feature; the opt-in AAC-LC decoder is a separate crate behind a feature
  that is off by default. Both are added with their codecs below.
- Rejected: one crate per codec (`coreav-flac`, `coreav-mp3`, ...). The
  decoders share the bit reader, the buffer, the conversion and the
  pipeline, and a crate boundary would make those public API. Rejected:
  the pipeline in `coreav-cli`, as `extract-audio`'s glue is. A Rust user
  (and the Python binding) needs "decode this file's audio" without the
  command.

## Library API

- `coreav_audio::decode(source, &Options, &Limits, &mut dyn Sink) ->
  Result<Summary, Error>`: any `coreav_isobmff::Source` (a file, bytes in
  memory, the HTTP range reader). `Options` names the stream (by its
  position among all tracks, `ffprobe`'s `index`; default the first audio
  stream whose codec is decoded) and the output `SampleFormat` (`S16`
  default, `S24`, `S32`, `F32`). The `Sink` receives the `OutputFormat`
  once, then interleaved little-endian whole sample frames.
- The decoders are public too (`coreav_audio::flac::FlacDecoder`), for a
  caller with its own demuxer, such as the raw-container demuxers of an earlier change.
- Interface with an earlier change (the raw MP3, FLAC, Ogg, ADTS and WAV demuxers):
  per track a codec, the codec configuration
  in the form MP4 and Matroska carry it (FLAC: the 34-byte streaminfo
  body; Opus: the RFC 7845 `OpusHead`; AAC: the `AudioSpecificConfig`;
  MP3: none), the container's sample rate and channel count when stated,
  one codec frame per packet (an MP3 frame with its header, Xing/Info/VBRI
  frames marked), timestamps in samples, and encoder delay and end
  trimming in samples. Neither side depends on the other's crate.
- Rejected: an iterator over decoded blocks instead of a sink. The
  MP4 packet iterator borrows the parsed movie and its work budget, so an
  iterator owning both would be self-referential; a sink keeps the
  borrows inside one call.

## Pipeline

- **Container**: Matroska by its EBML magic, else ISO base media
  (`NotIsobmff` becomes `unsupported_container`). Both are probed with
  every audio codec allowed, so a stream that is not decoded is named in
  the error ("codec ac3 is not decoded") instead of reported missing.
- **Stream choice**: `--stream N` must name an audio stream with a decoded
  codec; without it, the first audio stream with one. Rejected: picking
  the stream `ffmpeg` would pick. Its rule is not specified anywhere
  public, and `--stream` makes the choice explicit.
- **Container and codec configuration**: the codec configuration gives the
  sample rate and channels; a container that states others gets a
  `ContainerMismatch` warning, and the configuration wins. An MP4 sample
  entry cannot hold rates of 65,536 Hz and above, so they are not
  compared.
- **Trimming**: an MP4 edit list gives the presentation timeline, empty
  edits as silence and media edits as sample-exact windows ("Edit lists"
  below). Before an earlier change only the simple shape was applied, its end kept to
  the end of a frame as `ffmpeg` does; the agreement runs of FLAC and MP3
  measured that rule.
  Matroska's `CodecDelay` drops samples from the start and `DiscardPadding`
  from the end of its frame. Timestamps do not otherwise move samples:
  gaps between packets are not filled, overlaps not cut, which is what
  `ffmpeg -i IN OUT.wav` writes too.
- **End of the file**: a demuxer error after the first packet (a truncated
  file, a damaged end) ends the stream there with a `DemuxStopped` warning
  and keeps what was decoded, as `ffmpeg` does; before the first packet it
  is the request's error, and an I/O error always is. Rejected: failing,
  which loses every sample of a long recording to a damaged end (two
  corpus files end in such damage).
- **Damaged packets**: a packet that fails to decode is replaced by
  silence of its duration (the container's, else the previous packet's,
  at most 65,536 samples), so the output keeps its timing, and counted in
  a `Concealed` warning; past `max_decode_errors` (100) the request fails.
  A refusal by the decoder (`unsupported_feature`, such as a FLAC frame
  that changes the stream's format) ends the request: it is not damage. Rejected: failing on
  the first bad packet, which loses a long recording to one damaged frame;
  dropping the packet, which shifts everything after it.
- **Output conversion**: integers move to the output width by shifting
  (24-bit to 16-bit drops the 8 low bits, 12-bit to 16-bit adds 4 zero
  bits); floating point is scaled by the full scale (32768 for 16 bits),
  rounded to the nearest integer, ties to even, and clipped. `F32` divides
  integers by their full scale. Rejected: dithering when narrowing, which
  makes output differ from run to run and from `ffmpeg`'s default.
- **WAV**: `WAVE_FORMAT_EXTENSIBLE` with the channel mask for more than two
  channels or more than 16 bits, else the plain PCM or IEEE float format
  (Microsoft's WAVE notes), a 44- or 68-byte header with no other chunks.
  The channel order is the codec's (FLAC's default order is WAV's). To a
  file the sizes are written once the length is known; to standard output
  they are `0xFFFFFFFF`, as `ffmpeg` writes to a pipe. An output past the
  32-bit sizes keeps `0xFFFFFFFF` with a `WavTooLarge` warning. Rejected:
  RF64 for large outputs, which many WAV readers do not open; it can be
  added when asked for.

## Threads

A request may decode on several threads (`Options::threads`, `--threads`):
the library's default is 1, the command's is one per CPU (at most 16), as
`ffmpeg` decodes FLAC on several threads by default. The caller's thread
reads packets and hands them out in jobs of about 2^18 samples; a pool of
`std` scoped threads decodes each job and converts it to the output
format; the caller's thread writes the jobs back in stream order as they
come, while the threads go on. Decoding, trimming, concealment and limits
are the same as on one thread, and so are the bytes (tests compare them
for 1, 2, 3, 8 threads and one per CPU, for FLAC and for MP3).

- **Frames that depend on the ones before them**: FLAC frames stand
  alone. An MP3 frame needs the main data of the frames before it (its
  `main_data_begin` reaches up to 511 bytes back) and the state of the
  filter banks. The IMDCT overlap is the second half of the last granule,
  and the synthesis filter reads 16 time slots back, so the two last
  granules of each channel set that state entirely: one MPEG-1 frame, or
  two MPEG-2 or 2.5 frames (one granule each), further back while one
  channel is absent from frames (the mode changes between mono and stereo;
  the second channel counts only once a stereo frame has passed, as its
  state is the reset one before). So a job after the first starts with
  copies of the packets before it, the frames that hold those granules
  and, before them, frames with at least 511 bytes of main data, which its
  thread decodes from a reset decoder and drops; a granule left silent
  zeroes its scalefactors, so that a later `scfsi` does not reuse an older
  frame's, the one state the reset could not reproduce. Its first
  frame then decodes as it does in one pass, bit for bit. The copies cost
  about 4% more decoding at 128 kbit/s (5 frames for 110). The copies are
  at most 64 packets and 256 KiB a job, so a stream that needs more
  (frames without main data, long runs of one channel) costs bounded work,
  and its job boundaries may then differ from one pass; so may a stream
  whose priming frames fail to decode, which leaves less reservoir than
  one pass had. Tests compare the bytes on 1, 2, 3 and 8 threads for valid
  streams, mode changes, two streams cut and joined, MPEG-2 frames of
  mixed sizes and frames without main data.
  Rejected: one thread for MP3 (the first version), which leaves the
  command at the decoder's speed; splitting at frames whose
  `main_data_begin` is 0, which encoders rarely write.
- **Memory**: the pool holds at most 64 MiB, plus one job of about 2 MiB,
  in packets read and samples decoded or being decoded and not yet
  written. A packet costs its bytes, plus 8 bytes per sample it decodes to
  (its planar buffer and its output), plus 128 bytes of bookkeeping; the
  sample count is read from its frame header before it is decoded, so the
  bound holds whatever a frame codes and however large the packets are.
  Packet buffers are reused only up to 1 MiB each and 16 MiB in all, so a
  buffer that once held a large packet does not keep its memory outside
  the bound. Pinned tests: 400 packets of 15 MiB at one offset, and 15 MiB
  packets among runs of tiny ones.
- **Failure**: a thread that stops early (none should: production code has
  no panic paths, and arithmetic is bounded) sends a message as it
  unwinds, and the request ends with an error instead of waiting for the
  job it held; the scope joins every thread before `decode` returns.
- Rejected: decoding batches and writing them between batches, which
  leaves the threads idle while the caller's thread writes (the first
  version: 0.21 s against 0.13 s for the pipelined pool on a 72 MB input,
  `ffmpeg` 0.40 s); a thread pool crate, which the standard library makes
  unnecessary.

## Edit lists

Status: Accepted. The rules and bars were fixed before the code and the
runs, in the commits before them.

`decode-audio` writes the presentation timeline of an MP4 track (ISO/IEC
14496-12 8.6.5, 8.6.6), sample-exact: the output's sample n is the
sound at presentation time n / rate, the time `coreav probe` and the
writers report for the same file. Decoded audio then lines up with the
video presentation times those commands give.

- **Empty edits are silence**, a leading one included, of their duration.
- **Media edits** present the decoded samples of their window,
  `[media_time, media_time + duration)` in media ticks, cut sample-exact
  at both ends; consecutive windows follow one another.
- **Which lists**: those `coreav-isobmff` applies (`EditList::Simple` and
  `EditList::Multiple`, `docs/design/edit-lists.md`: empty edits and
  media edits at rate 1, in increasing media order, without overlap). Other
  lists (dwells, other rates, overlapping windows) are not applied, with
  the `EditListIgnored` warning, as today.
- **Rounding**: entry k starts at the exact sum of the durations before
  it; each boundary, in samples, is that time times the rate, rounded to
  the nearest sample (halves up), and every span is the difference of its
  two boundaries, so lengths never drift. A window's media start is
  `media_time` converted the same way from the first packet's timestamp;
  a media time before it (a fragment's decode time above 0) starts the
  window before the media, which still ends where the edit ends.
- **Missing media is not padded**: the clock holds while the media covers
  each window. A window that starts before the first packet, or reaches
  past the last sample, writes only the samples there are, so the output
  after it is earlier by the missing length. Rejected: padding with
  silence, which keeps the clock but invents output for a damaged or
  inconsistent file. The oracle's model makes the same choice, so bar 1
  does not test it.
- **Decode time for media time**: `media_time` is in composition time
  (8.6.6), and the windows use the packets' decode times. They are the
  same unless the track has composition offsets (`ctts`), which audio
  codecs do not need (every packet is presented in decode order); a track
  that has them is presented shifted by them. A known limit, not measured;
  the oracle's model makes the same choice, so bar 1 does not test it.
- **Where decoding stops**: packets that start at or after the last media
  edit's end, by their timestamps, and MP3 frames cut from packets, by
  their own positions, are not decoded (once a packet was cut, whole
  packets after it are judged by those positions too). A last window that
  runs to the media's end sets no stop. The stop is known before decoding,
  so the summary's packet count, warnings and errors are the same on any
  number of threads ("Threads").
- **Reported**: the summary's `timeline` gives the output samples of the
  first presented media sample (`start`: the leading silence) and every
  span of silence an empty edit wrote (`silence`: output sample and
  length), so a caller can drop the leading silence, or any span, without
  reading the file again. The WAV writer writes the timeline as it is,
  silence included: a WAV file has no timestamps to carry a start.
- **Matroska** is unchanged: `CodecDelay` and `DiscardPadding` (RFC
  9559) are its timeline; it has no edit list.
- **Limits**: an empty edit's silence is output the caller did not
  send: a 2,392,423 ms leading empty edit (an `edge` corpus file) is 40
  minutes of PCM from a small file. Silence is written in chunks of at
  most 4 KiB, never allocated whole, and counts against
  `max_output_samples` (2^33 samples, about 24 hours of 48 kHz stereo)
  with the samples: past it the request fails with
  `LimitExceeded(OutputSamples)` before writing more. Boundaries are
  computed in 128 bits and saturate, so no duration and timescale
  overflow them (a `segment_duration` of 2^64 − 1 at a movie timescale
  of 1 is a limit error, not a wrapped count). Pinned by a test and a
  fuzz seed with such an edit.

Measured on the corpus before the change
(`docs/spikes/audio-decode-bench/evidence/corpus-edit-lists.json`, every audio
track of the 1,393 corpus v1 ISOBMFF files with audio, read by `coreav
probe`; 86 files did not probe): 69 of 1,142 audio tracks (6.0%) start
with an empty edit, median 18 ms. In `real` files: 48 of 623 (7.7%),
median 16 ms, at most 584 ms; in `edge` files: 21 of 519 (4.0%), median
67 ms, at most 2,392,423 ms (a crafted file). 9 tracks have several media
edits, 2 an empty edit between media edits, 7 a list outside the applied
shapes.

Rejected: (B) `ffmpeg`'s output (black box, `ffmpeg` 7.0.2): no
silence for a leading empty edit; a single edit's end kept to its last
frame's end; several edits cut at packet boundaries (a later edit starts
at its first whole packet); an empty edit between media edits filled
with samples from the start of the media. It shifts every sample after a
leading empty edit (in 7.7% of the `real` tracks) or a cut off its
presentation time, and the inner fill is wrong. `ffmpeg`-equivalent output
is a job for the `ffmpeg` compatibility layer, not an option of the
native command. Also rejected: a command option for it now.

### Edit list bars

Fixed before the runs; evidence in `docs/spikes/audio-decode-bench/`.

1. **The model (exact).** On every MP4 input of the FLAC and MP3 sets
   (derived and corpus) and on the edit-list fixtures below, CoreAV's
   output equals the presentation model built from `ffmpeg -ignore_editlist
   1` (its decode of every sample, `-f s32le` for FLAC, `-f f32le` for MP3)
   and the edit list as the oracle's independent reader reads it from the
   file (`Facts` in `scripts/oracle_kd.py`): FLAC bit-exact, MP3 within
   the MP3 agreement tolerance, lengths exact. Fixtures: written by the
   spike from a 24-bit FLAC ramp and an MP3 encode with edit lists
   rewritten in place: a leading empty edit; a start inside a frame; an
   end inside a frame; a cut (two media edits with a gap); an empty edit
   between media edits; three media edits. Bar: 100% of the inputs
   `ffmpeg` decodes without an error.
2. **Against `ffmpeg`'s default output.** Every difference from `ffmpeg
   -i IN` (edits applied its way) is covered by an entry of
   `docs/known-divergences-decode-audio.md` (its JSON twin beside it),
   whose condition the raw edit list read by `Facts` meets and whose
   predicate checks that `ffmpeg`'s output is what the entry says it
   writes; no difference is left uncovered, and no entry is a tolerance.
   The entries are dated as added after these bars. Their gating is
   audited as the probe overlay's is: a mutation audit changes CoreAV's
   output, and the raw edit-list values with it, and no entry may cover a
   mutation (0 covered), as `tools/oracle/kd_audit.py` requires of the
   probe entries. Bar: 100% of the inputs `ffmpeg` decodes without an
   error, and 0 covered.
3. **Nothing else moves.** Matroska inputs, and MP4 inputs without an edit
   list, decode to the same bytes as on the commit before the change, on
   every input of both sets.
4. **Speed.** The library and command bars of FLAC and MP3
   (median ours / FFmpeg at most 1.00, per class) on the MP4 class of
   both held-out derived sets, every 4th input (`perf.py --every 4`), with
   the share of inputs faster reported.

Results (`docs/spikes/audio-decode-bench/`, "Edit lists"): all four met. Bar 1:
560 of 560 MP4 inputs equal the model, 552 of them with an edit list.
Bar 2: 174 equal to `ffmpeg`, 386 covered by the four entries of
`docs/known-divergences-decode-audio.md` (`KD-A-EDIT-END` alone on 373 of
them: `ffmpeg`'s own millisecond edit ends inside the last packet), 0
uncovered; the audit caught 5,132 mutations and the entries covered none.
Bar 3: 606 of 606 inputs unchanged. Bar 4: library 0.898 (FLAC) and 0.919
(MP3), command 0.591 and 0.194, CoreAV faster on 86% to 100% of the
inputs.

## Resampling and downmixing

Status: Accepted. The design and the bars below were fixed before the
measured runs, in the commit that adds them; every gating bar is met
("Resampling bars", results).

`--rate HZ` and `--channels N` (`Options::sample_rate`,
`Options::channels`) give the output another sample rate or channel
count, for example 16 kHz mono for speech recognition. Without them, or
with the stream's own values, nothing changes: the stream is written as
before, byte for byte.

- **Where**: after the stream's output, trimmed and in stream order
  ("Edit lists", "Threads"): a sink in front of the caller's
  (`conversion.rs`) that downmixes, then resamples, then converts to the
  asked sample format. The stream is written to it in 32-bit floating
  point, so the conversion rounds once, at the end, as "Output conversion"
  rounds. It runs on the caller's thread, after the decoding threads have
  put the stream in order, so the output is the same on any number of
  threads. Rejected: converting inside the decoding jobs, which would need
  each job to carry the filter's history across its boundary. Integer
  samples (FLAC) become floats on one or two channels in single precision,
  in a loop the compiler vectorizes: the scale is a power of two, so `v as
  f32 × 2^(1 - bits)` is the exact quotient rounded once, the value the
  double-precision path gives (a test covers every bit depth); the sink
  reads one or two channels back with the frame's size known to the
  compiler.
- **Precision**: 32-bit floating point holds 24-bit samples exactly; a
  32-bit FLAC stream converted loses what lies below about -144 dB.
- **Summary**: `output` gives the converted format; `samples` counts
  output samples; `timeline` positions are the output's, each the first
  output sample at or after the stream's position, ceil(n × to / from).

### The resampler (`resample.rs`)

Output sample n is the input, band-limited, at time n / output rate:
the resampler adds no delay, so the timeline of "Edit lists" keeps its
times, and its output has ceil(input samples × to / from) samples, one for
each output time before the input's end (the input before its first
sample and after its last is silence). `ffmpeg` has no delay either
(black box: an impulse at 0.5 s peaks at sample 8,000 of 16,000 Hz) and
the same lengths or one sample fewer (13,246,575 samples at 44.1 kHz give
4,806,014 at 16 kHz and 14,418,041 at 48 kHz in both; in the agreement
runs, one fewer at 16 kHz on 134 of 527 FLAC and 160 of 607 MP3 inputs,
the same at 48 kHz on all). At the stream's two ends `ffmpeg` extends the
signal where ours sees silence, so the first and last 16 output samples
at the lower rate differ (a sine starting at 0 gives 152 in our first 16
kHz sample, 304 in `ffmpeg`'s, and the same values from sample 13 on).
Rejected: extending the signal ourselves, which would invent sound before
the stream's start, where the timeline says there is silence.

- **Filter**: a windowed sinc, sinc(fc·u)·w(u/16), u in samples of the
  lower of the two rates, fc = 0.97 of its Nyquist frequency, w a Kaiser
  window of β = 9: 32 taps at the lower rate (32 × from / to taps per
  output sample when downsampling). Each phase's taps are scaled to sum to
  1, so a constant stays constant. Computed response: flat within 0.002 dB
  up to 0.80 of the lower Nyquist frequency, -6 dB at 0.97, and at least
  89 dB down from 1.16; what aliases (input between 1.00 and 1.16 of the
  output's Nyquist frequency) lands between 0.84 and 1.00, above the flat
  band.
- **Why these corners**: they are FFmpeg's resampler defaults as its
  documentation and `ffmpeg -h full` give them (`filter_size` 32,
  `cutoff` 0.97, `kaiser_beta` 9), whose response we measured black-box
  (flat to 0.80, -1 dB at 0.90, -6 dB at 0.97, -43 dB at 1.10, at most -91
  dB from 1.15). Speech recognition models are trained on audio that
  `ffmpeg -ar 16000` resampled (OpenAI's Whisper loads audio by running
  it), so a resampler with the same band edges hands them the band they
  were trained on; and 32 taps cost what FFmpeg's filter costs. The
  implementation is ours: the kernel, the phases and the edges follow from
  the definition above, not from FFmpeg's (no FFmpeg source was read).
- **Phases**: the rates' ratio in lowest terms, `step` input samples for
  `phases` output samples, gives each output's position exactly in
  integers. Where `phases × taps` is at most 65,536 coefficients (every
  pair of the common rates: at most 21,000), each phase's taps are
  computed once; otherwise (a rate prime to the other, such as 44,056 Hz)
  the kernel is sampled 1,024 times per zero crossing and each output's
  taps are interpolated linearly from it (error about 10^-6 of the peak,
  below the stopband), then scaled to sum to 1.
- **No C math library**: the kernel's sine and Bessel function are
  computed from their series (`sin_pi`, `bessel_i0`), so the resampler
  adds no `libm` call to the binary (the MP3 decoder's tables avoid it for
  the start-up cost it had, "MP3").
- **The table's cost**: the table is built once per stream, 160 rows of 96
  taps for 44.1 to 16 kHz, a fixed cost that is about 15% of a 10-second
  input. The taps are evaluated eight at a time (`kernel_block`): each
  tap's series run in step with the others', which overlaps their
  latencies and lets the compiler use vector instructions, and the
  window's scale I0(β) is computed once. Every tap keeps its value bit for
  bit (each series has the same operations in the same order; a test
  checks 360,000 points against the one-at-a-time evaluation). Measured
  (development, not a bar): 1.86 ms before, 0.84 ms after for 44.1 to 16
  kHz. Rejected: series with the divisions turned into multiplications by
  stored reciprocals, or a shorter series (another two to four times
  faster, but the taps change in their last bits, so the outputs would no
  longer be the ones bars 2 and 3 measured); using the kernel's symmetry
  (row p and row `phases` - p hold the same values reversed only up to the
  rounding of the phase, so not bit for bit).
- **The filter loop** (`coreav-kernels-audio`, a kernel crate under the
  `unsafe` policy's SIMD exception): each output is the dot product of an input
  window with a row of taps, in eight lanes. On x86-64 with AVX the loop
  runs four outputs at a time in 256-bit registers, their sums in one loop
  so that their additions overlap (`unsafe`, listed in
  `docs/unsafe-sites.md`, after one branch-free check that every window
  lies inside its slices); elsewhere, and in tests as the reference, the
  portable loop. Both multiply and add separately and sum the lanes in the
  same order, so every CPU writes the same bits (a differential test and
  the fuzz target `fir`). The table starts on a 32-byte boundary, so no
  load of a row straddles a cache line; the windows repeat every `phases`
  outputs, `step` input samples later, so they are listed from one stored
  period, and with one phase (rates in an integer ratio, 48 to 16 kHz) in
  one loop. Measured on the workspace machine (a microbenchmark,
  development, not a bar), ns per output: 32 taps 10.5 portable, 4.1 to
  4.4 AVX; 96 taps 20 and 9.2 to 9.5 (with one output per loop and an
  unaligned table, the first version: 6.1 and 17.1 AVX). Rejected: eight
  outputs per loop (no faster); computing outputs grouped by phase so a
  row stays in the L1 cache (slower: the input windows then miss).
  Rejected: fused multiply-add (3.5 ns at 32 taps), which rounds once
  instead of twice, so CPUs with and without it would write different
  bits; the portable loop alone, half the speed (the speed bars below).
- Rejected: **sample-exact agreement with `ffmpeg`** as the goal; it
  would mean reproducing its implementation (its phase count and
  interpolation), not a specification. The bars compare the band where
  both filters are designed flat. A **sharper filter** (stopband from
  1.00, 100 dB): three to four times the taps, and a band edge models
  were not trained with. **Linear or cubic interpolation**: aliasing at
  -20 to -40 dB. **libsoxr or speexdsp** in a backend crate: C on the
  path for a few hundred lines of arithmetic (and libsoxr is LGPL). A
  **resampler crate** (rubato): its sinc resamplers fix chunking and
  delay their own way, where the timeline needs exact positions, zero
  delay and the length above; the resampler is 400 lines.

### Downmixing (`mix.rs`)

To one or two channels, from the stream's speaker positions (the WAV
channel mask the stream reports: FLAC's default layouts, MP3's mono or
stereo):

- **Gains**: ITU-R BS.775-4, Annex 4, Table 2 for 3/2 sources (stereo: L
  + 0.7071 C + 0.7071 Ls; mono: 0.7071 L + 0.7071 R + C + 0.5 Ls + 0.5 Rs)
  and Annex 5, 1.1 for mono to stereo (both at -3 dB). Other positions
  take the role of a BS.775 channel: front left and right of centre and
  top front left and right as L and R; top centre and top front centre as
  C; side and back left and right, top back left and right as surrounds;
  back centre and top back centre as a mono surround split equally (0.5
  into each side, 0.7071 to mono). LFE is left out, as Table 2 leaves it.
  The mono gain of each position is 0.7071 times the sum of its two
  stereo gains, which gives Table 2's mono row.
- **Scaled so that it cannot clip**: when a row's absolute gains sum to
  more than 1, every gain is divided by the largest sum (stereo to mono
  becomes 0.5 L + 0.5 R; 5.1 to stereo L / 2.414 + ...). The output's
  level is then the same in every sample format. `ffmpeg` (black box)
  uses the same gains and scales them the same way when it writes
  integers, and not for floating point, where its 5.1 downmix can exceed
  full scale. Rejected: unscaled gains, which clip integer output;
  scaling only for integer formats, as `ffmpeg` does, which makes the
  level depend on `--format`.
- **Without speaker positions** (a mask that does not name every channel,
  beyond one and two channels): mono is the mean of the channels; stereo
  is refused. Other channel counts than 1, 2 and the stream's are refused
  (`unsupported_feature`).
- Measured differences from `ffmpeg` (black box): back centre to mono is
  0.5 there, 0.7071 here (the split above).

### Limits

- Output rates 1,000 to 768,000 Hz (`--rate` refuses others as a usage
  error, the library with `unsupported_feature`), and at most 1,024 input
  samples per output sample (32,768 taps per output sample).
- Output samples count against `max_output_samples` at the output rate:
  upsampling cannot write past the limit the stream would have. The
  stream's decoded samples, at its rate and channels, still count against
  it too, since they bound the decoding's work, which a lower rate or a
  downmix does not reduce.
- A downmix alone keeps any rate the stream has; the range of output rates
  applies to a rate that is resampled to.
- Memory: the phase table at most 256 KiB, the interpolated kernel 128
  KiB, the filter's history one window per channel, plus what one packet
  decodes to; the converted output is written at most 65,536 samples per
  channel at a time, however far a packet is upsampled.

### Resampling bars

Fixed before the measured runs; the evidence goes to
`docs/spikes/audio-decode-bench` ("Resampling").

1. **Quality (CI).** For every ordered pair of 8, 11.025, 16, 22.05, 32,
   44.1, 48 and 96 kHz, and 44.1 to 44.056, 7.919 to 16, 192 to 16 and 16
   to 7.919 kHz (the interpolated kernel): a tone at 0.05, 0.3, 0.6 and
   0.8 of the lower Nyquist frequency keeps its level within ±0.005 dB,
   with everything else at least 85 dB below it; when downsampling, a tone
   at 1.17, 1.3 and 1.6 of it leaves at most -85 dB. The unit test
   `resample::tests::passband_and_stopband`.
2. **Agreement with `ffmpeg`'s conversion (gating).** On every input of
   the FLAC and MP3 agreement sets ("FLAC sets", "MP3 sets") that `ffmpeg`
   decodes without an error, for each conversion (a) `--rate 16000
   --channels 1`, (b) `--rate 48000`, (c) `--channels 1`, and (d)
   `--channels 2` on inputs that are not stereo: CoreAV's `--format f32`
   output against `ffmpeg -f f32le -ar RATE -ac N -channel_layout MASK -i
   - -ar R -ac C -rematrix_maxval 1 -f f32le -` fed CoreAV's own output at
   the stream's rate and channels (`--format f32`, which the codec bars
   compare with `ffmpeg`'s decoders), so that the comparison is of the
   conversion alone; `-rematrix_maxval 1` makes `ffmpeg` scale its gains
   for floating point as it does for integers. Measure, per channel: the
   difference in the band from 0 to 0.80 of the lower Nyquist frequency,
   where both filters are designed flat (Hann-windowed blocks of 32,768
   samples, half overlapping): its energy relative to `ffmpeg`'s in the
   band (E_rel), and its power relative to full scale (E_abs). Bar: every
   channel of every input has E_rel at most -70 dB or E_abs at most -100
   dB (below the rounding noise of 16-bit output, -101 dB); CoreAV's
   length is exactly ceil(samples × to / from); `ffmpeg`'s length is
   reported. Two filters each flat within 0.002 dB differ by at most 2.3 ×
   10^-4 (-73 dB) in the band; development runs gave -88 to -101 dB. An
   input whose layout has a position whose gains differ from `ffmpeg`'s
   (back centre, top back centre) is listed with the reason and does not
   count.
3. **End to end (reported).** The same conversions and measure against
   `ffmpeg -i IN -map 0:a:K -ar R -ac C -rematrix_maxval 1 -f f32le -`,
   which adds the decoders' and the timelines' differences the codec and
   edit-list bars record; and the default `--format s16` WAV of (a)
   against `ffmpeg -i IN -map 0:a:K -ar 16000 -ac 1 OUT.wav`: the share of
   inputs whose samples differ by at most one step in the band, and the
   largest difference.
4. **Nothing else moves (gating).** Without `--rate` and `--channels`,
   every input of both sets decodes to the same bytes as on the commit
   before the change (default `s16` and `--format f32`).
5. **Speed.** On the held-out derived sets of FLAC and MP3,
   every 4th input (`perf.py --every 4`), with the method of "FLAC sets"
   (one warm-up, 5 rounds with the engines' order rotated, an input's time
   the median of its rounds, the median per-input ratio with a bootstrap
   95% interval of 10,000 resamples, the share of inputs faster reported).
   Bars, each at most **1.00** for each codec's set, overall and per class
   (MP4, Matroska):
   - **(a) 16 kHz mono, library**: `coreav_audio::decode` with
     `sample_rate` 16,000 and `channels` 1 to interleaved `s16` in memory
     against libavformat, libavcodec and libswresample 7.0.2 in-process
     doing the same (`swr_alloc_set_opts2` to 16 kHz, the default mono
     layout, `s16`, drained at the end; `baseline/lav_audio.c`), one
     thread each;
   - **(a) 16 kHz mono, command**: `coreav decode-audio --rate 16000
     --channels 1 IN OUT.wav` against `ffmpeg -i IN -map 0:a:K -ar 16000
     -ac 1 OUT.wav`, each with its defaults;
   - **(b) 48 kHz, command**: `coreav decode-audio --rate 48000 IN
     OUT.wav` against `ffmpeg -i IN -map 0:a:K -ar 48000 OUT.wav`.
   Reported, not gated: **(b) 48 kHz, library**. Development measurements
   on one 5-minute 192 kbit/s stereo MP3 at 44.1 kHz (the decoder's
   slowest kind of input, "MP3") put the library pair at about 0.95 for
   (a) and 1.15 to 1.2 for (b): upsampling 44.1 to 48 kHz stereo costs 32
   taps for each of 96,000 output samples a second, FFmpeg's resampler
   does it in about half our time per output sample on this machine, and
   our filter loop keeps output bit-identical across CPUs (no fused
   multiply-add, above). (a) is the use the roadmap names for audio
   decoding ("for transcription pipelines"); the per-sample cost of (b) is
   the next speed item for this crate, with a new run of this pair. The
   command pairs gate both, as a user runs them.
6. **Safety.** The `decode` fuzz target with conversions (16 kHz mono, 48
   kHz, 7,919 Hz mono, stereo); the `fir` target of
   `coreav-kernels-audio` (the AVX loop against the portable one, bit for
   bit); 60 seconds each in CI. Pinned tests: refused rates and channel
   counts, the output limit reached by upsampling, the same bytes on 1, 2
   and 8 threads, the stream's own rate and channels unchanged.
7. **Regression guard.** `decode-audio-16k-mono` (the MP3 fixture to 16
   kHz mono) and `decode-audio-resample` (the FLAC fixture, mono 44.1 kHz,
   to 48 kHz stereo) in the CI benchmark guard, with their baselines in
   `bench/guard/baseline.json` (19.4 and 22.9 million instructions beyond
   start-up, counted by the guard's update run on the code that merges).

Results so far (`docs/spikes/audio-decode-bench`, "Resampling"). Run 1, on
`b5c7245`: bars 2 and 4 met on both sets (FLAC 527 of 527 inputs for (a)
to (c) and 109 of 109 for (d), MP3 607 of 607 and 217 of 217; 535 and 643
inputs unchanged without conversion). Bar 5 measured MP4 files only (the
stride of `perf.py --every 4` fell on one class) and missed two arms, MP3
(a) library 1.007 and FLAC (b) command 1.014, final for that code; the
other gating arms were 0.40 to 0.96. Round 2, fixed in the spike's README
before its run: the filter loop and window changes above, which keep the
output bit for bit (the conversions of bars 2 and 3 give `b5c7245`'s bytes
on every input of both sets), and bar 5 again on every arm and both
classes, on `8949d3b`: met for MP3 (a) library (0.963) and FLAC (b)
command (0.831), and for every command arm (0.38 to 0.83); missed for FLAC
(a) library on its MP4 class (1.007; overall 0.989, Matroska 0.984), final
for that code. Round 3, fixed before its run: the table, integer-to-float
and one-phase window changes above, every output byte unchanged on both
sets, and bar 5 again on every arm, on 64 sources per codec that no
earlier run measured, on `10e2058`: met for every gating arm overall and
in each class, (a) library FLAC 0.914 (MP4 0.922, Matroska 0.909) and MP3
0.935, (a) command 0.640 and 0.348, (b) command 0.881 and 0.528, each
interval's upper end below 1.00; (b) library, reported, 1.070 and 1.048.

## Limits

`coreav_audio::Limits`, with the
demuxers' own limits inside it:

| Limit | Default | Why |
|---|---|---|
| `max_channels` | 32 | FLAC has at most 8; Opus mapping families go higher. Checked for every codec when its decoder is made: FLAC's streaminfo count, MP3's first frame's (or the count the decoder is given). |
| `max_packet_bytes` | 16 MiB | Eight times the largest FLAC frame (65,535 verbatim 32-bit samples on 8 channels). |
| `max_output_samples` | 2^33 | About 24 hours of 48 kHz stereo. Decoding multiplies sizes (a 49-byte FLAC frame codes 2 MiB, RFC 9639 section 11), so the output is bounded by itself, not by the input's size. |
| `max_decode_errors` | 100 | Concealed packets before the request fails. |

Memory: on one thread, one packet, one decoded packet and its converted
bytes at a time; with several, the pool's bound ("Threads").

## FLAC

Native, written from RFC 9639 (`coreav_audio::flac`).

- **Native, not libFLAC.** The format is small and completely specified,
  lossless output makes correctness binary (bit-exact or wrong), and a
  native decoder removes C code from the untrusted-input path, which is
  the point of the roadmap's safety criterion. Rejected: libFLAC (BSD) in a
  backend crate, which would add `unsafe` and C for no capability the
  native decoder lacks.
- **Configuration**: Matroska's `CodecPrivate` (`fLaC` and the metadata
  blocks, RFC 9639 10.2), MP4's `dfLa` payload (version, flags, then the
  metadata blocks), or the bare 34-byte streaminfo body. FLAC in
  MP4 needed `coreav-isobmff` to name `fLaC` sample entries
  (`Codec::Flac`, configuration box `dfLa`); it is not on the demuxer's
  default allowlist, so `coreav probe` output changes only in the codec
  name of a dropped FLAC stream (`other` before, `flac` now).
- **Decoding**: frame header with its CRC-8; constant, verbatim, fixed
  (orders 0 to 4) and linear (orders 1 to 32) subframes; wasted bits;
  4- and 5-bit Rice parameters with escaped partitions; left-side,
  side-right and mid-side stereo; the CRC-16 footer.
- **Choices beyond the RFC**:
  - A frame whose channel count, sample rate or bit depth differs from the
    streaminfo block is refused (`unsupported_feature`): a stream's output
    has one format. RFC 9639 section 9 allows a decoder to stop there.
  - A decoded sample outside its subframe's bit depth is an error, which
    keeps every intermediate value inside the bounds of Appendix A, so
    nothing can overflow. The prediction is accumulated in 64 bits, the
    exact width for 32 coefficients of 15 bits on 33-bit samples.
  - A CRC-8 mismatch is an error (the header cannot be trusted); a CRC-16
    mismatch keeps the samples, as the frame parsed, with a `FrameCrc`
    warning.
  - The side channel of 32-bit audio needs 33 bits; only that subframe is
    decoded with 64-bit storage.
  - Only FLAC sample entries (`fLaC`) and `A_FLAC` tracks are FLAC. An
    encrypted entry (`enca`) cannot be decoded without its keys, and an
    IAMF stream (`iamf`, AOMedia's Immersive Audio Model and Formats) that carries FLAC
    is a format of its own, outside the roadmap's scope: both are refused with
    their names. Two corpus files of each kind exist.
  - The streaminfo MD5 is not checked while decoding (the conformance
    tests check it); `ffmpeg` does not check it either.

### FLAC bars

Fixed before the measured runs; the evidence goes to
`docs/spikes/audio-decode-bench/` (README, results) and the tests named below.

1. **Conformance (bit-exact).** The IETF CELLAR FLAC test files
   (`ietf-wg-cellar/flac-test-files`, CC0, pinned commit in the workflow):
   all 64 files of `subset` decode to the MD5 in their streaminfo block.
   Of `uncommon`, files 05 to 09 (32-bit, 768 kHz, 15-bit, block size
   65,535, Rice partition order 15) decode to their MD5; 01 to 04 (changes
   of sample rate, channels or bit depth mid-stream) are refused with
   `unsupported_feature` by the choice above; 10 and 11 start without the
   `fLaC` signature and need the raw FLAC demuxer. Every `faulty`
   file fails or decodes without a panic. Bar: all of this, in CI
   (`.github/workflows/audio-conformance.yml`).
2. **Agreement with `ffmpeg` (bit-exact).** FLAC is lossless: the only
   correct output is the encoder's input, so the tolerance is zero. On
   every input of the two sets below, the samples of `coreav decode-audio
   --format s32` equal `ffmpeg -i IN -map 0:a:K -f s32le -` (the native
   precision of every FLAC bit depth up to 32), and the data of the default
   `--format s16` WAV equals that of `ffmpeg -i IN -map 0:a:K OUT.wav`.
   Bar: 100% of the inputs `ffmpeg` decodes without an error. An input
   that `ffmpeg` decodes with an error, or that CoreAV refuses by a choice
   above, is listed with its reason and does not count against the bar.
   Since an earlier change, MP4 inputs with an edit list are judged by "Edit list bars"
   instead (CoreAV presents the edits sample-exact, `ffmpeg` at packet
   granularity); the recorded result measured the binary before it.
3. **Speed.** The median of the per-input ratios CoreAV /
   FFmpeg at most **1.00** on the held-out set, overall and for each class
   (MP4, Matroska), for both:
   - **library**: `coreav_audio::decode` to interleaved 16-bit samples in
     memory against libavformat and libavcodec 7.0.2 in-process doing the
     same (`av_read_frame`, `avcodec_send_packet`/`avcodec_receive_frame`,
     `swr_convert` to interleaved `s16`; the pinned LGPL package of Spike
     C, written from the public API documentation), one thread each;
   - **command**: `coreav decode-audio IN OUT.wav` against `ffmpeg -i IN
     -map 0:a:K OUT.wav`, both timed from outside with process start-up,
     `ffmpeg` with its default options (its default thread count).
   A "faster" claim needs the upper bound of the 95% bootstrap confidence
   interval below 1.00. Reported, not gating: the decoder alone
   (`FlacDecoder::decode` over packets already in memory against
   `avcodec_send_packet`/`avcodec_receive_frame`).
4. **Safety.** Fuzz targets `flac_frame` (the decoder on arbitrary frames
   under arbitrary streaminfo values) and `decode` (the whole pipeline on
   arbitrary files) in `crates/coreav-audio/fuzz`, 60 seconds each in CI; a pinned test that a flood of constant subframes stops
   at `max_output_samples`; no panics.
5. **Regression guard.** A `decode-audio-flac` case of the CI benchmark
   guard (`docs/design/benchmarks.md`) on a generated FLAC fixture.

### FLAC sets

Corpus v1 holds no `real` FLAC file (its 19 FLAC files are `edge`, from
public sample collections), so the held-out set is derived from real
audio:

- **Derived (gating for speed, counted for agreement).** Every `real`
  corpus v1 file with `holdout: true` and an audio stream: its first audio
  stream decoded and encoded by the pinned `ffmpeg` 7.0.2 to FLAC, audio
  only, once into Matroska (`.mka`) and once into MP4 (`.m4a`). The
  encoding parameters vary with the source's SHA-256, so the set covers
  the encoder's range without choosing per file: the first byte modulo 3
  picks compression level 0, 5 or 8; its next byte modulo 4 picks 24 bits
  (`-sample_fmt s32 -bits_per_raw_sample 24`) for 0, else 16 bits
  (`-sample_fmt s16`); channels and rate are the source's. Sources that
  `ffmpeg` cannot decode are skipped and counted.
- **Corpus FLAC (counted for agreement, reported for speed).** The 17
  corpus v1 FLAC files in Matroska and MP4 (the two in Ogg need an earlier change).
- Method: one input at a time, sources in the page cache, outputs on a
  tmpfs, the development machine otherwise idle; per input one warm-up run
  of each engine, then 5 rounds with the engines' order rotated; an
  input's time per engine is the median of its rounds; ratios per input;
  medians with a bootstrap 95% interval (10,000 resamples). Results carry
  the corpus SHA-256, class and parameters, no file names.

## MP3

Native, written from public documents (`coreav_audio::mp3`):

- ISO/IEC 11172-3 (MPEG-1 audio), Layer III: syntax, semantics and the
  decoding process, from the published copies of the international
  standard and of its committee draft (the draft prints the formulas as
  text: requantization, IMDCT, windows, MS and intensity stereo), and
  Annex C's analysis window C[i] (Table 3-C.1);
- ISO/IEC 13818-3 (MPEG-2 audio), 2.4.3.2: the lower sampling frequencies
  (scalefactor partitions, intensity stereo);
- "MPEG-Layer3 Bitstream Syntax and Decoding" (M. Sieler and R.
  Sperschneider, Fraunhofer IIS, issue 1.2, 1997): the Huffman code
  tables, the scalefactor bands of every sample rate, the preemphasis and
  aliasing tables, and MPEG 2.5 (8, 11.025 and 12 kHz, outside ISO).

Choices:

- **Native, not a C library.** MP3 is the most common audio codec of
  untrusted inputs after AAC, its patents have expired, and the decoder is a few thousand lines of arithmetic with no
  format extensions to track. Rejected: libmpg123 (LGPL, outside the
  license policy); a permissive C decoder in a backend crate, which would
  put C on the untrusted-input path for no capability the native decoder
  lacks.
- **Tables**: `crates/coreav-audio/tables/gen_mp3.py` writes
  `src/mp3/tables.rs` from the two PDFs it pins by SHA-256 and checks
  them: every Huffman table complete and prefix-free (Kraft sum 1), every
  band table contiguous and summing to 576 or 192 lines, the preemphasis
  table and the aliasing coefficients as printed. The synthesis window
  D[i] (Table 3-B.3) is not printed in any copy; it is 32 C[i] (the two
  windows are the same prototype filter, scaled for the synthesis gain),
  which the compliance stream checks to full accuracy. The tables are
  regenerated by hand from the pinned documents when the generator
  changes (CI does not fetch them: one is a copy of ISO text); CI
  regenerates the unrolled transforms and requires them byte-identical. `gen_dct32.py` and
  `gen_imdct.py` write the unrolled transforms below, each checked
  against the direct sum, and `gen_trig.py` the windows and the other
  trigonometric constants, so that the decoder computes none at run time
  (|x|^(4/3) is x times a cube root by Newton's method): computing them
  with `std`'s `sin`, `tan` and `powf` links the C math library into every
  build of `coreav`, and loading it cost every process 38,000 instructions
  of start-up (bench-guard: +8.7%).
- **Scope**: Layer III of MPEG-1, MPEG-2 and MPEG 2.5, every mode, free
  format, CRC. Layers I and II (`mp1`, `mp2`) are refused with their name:
  they are not in the roadmap's list of audio codecs.
- **Format**: the first frame is the stream's configuration (MP3 has no
  other). Its sample rate holds for the stream: a frame with another is
  refused (`unsupported_feature`), as a FLAC frame that changes the
  format. Its channel count holds too, but the mode may change from frame
  to frame, so a frame with the other count is mapped: one channel is
  copied to both, two are averaged, 0.5 (L + R), the gain `ffmpeg` uses
  for 16-bit output. A container that states another channel count gets
  the `ContainerMismatch` warning, as for FLAC. Output is floating point,
  full scale ±1.0, 1,152 (MPEG-1) or 576 samples per channel and frame.
- **Bit reservoir**: the last 511 bytes of main data, all a
  `main_data_begin` can reach, of which only the bytes after the last
  granule's data may start a later frame's main data. The bytes before
  them are replaced by zeros in place, so the granules after them keep
  their offsets, and a granule whose data starts in them is silent: at the start of a cut stream, the
  bytes the decoder has not seen; in a stream cut and joined, bytes that
  hold earlier granules' data (a frame whose `main_data_begin` reaches
  past what the frames before it left). The filter banks go on. A
  granule's data is its `part2_3_length` bits: a `big_values` that they
  cannot hold leaves the other lines zero. Both rules came out of the
  first agreement run (spliced corpus files, where `ffmpeg` writes the
  same silence); without them the bytes of other granules decode as
  noise.
- **Reframing**: some MOV and MP4 writers store MP3 in samples of any
  size, not one frame each (`ffmpeg` parses those again too). A packet
  that starts with a frame header but is not that frame, or that holds a
  header of the stream past its start (a stream cut anywhere), is
  buffered, and the bytes are cut at frame headers: at a frame boundary
  any Layer III header, so that a frame of another sample rate reaches the
  decoder and is refused (a header damaged into another valid rate then
  ends the request rather than being concealed); past bytes that are not
  a frame, only a header of the stream's version and sample rate. One run
  of such bytes, however many packets it spans, goes to the decoder as one
  damaged packet (concealed and counted), unless it is shorter than 32
  bytes, stray padding, which is dropped; the frames' own headers give the
  stream's timing, so a run is not counted per packet. A last frame cut
  short is decoded as far as it goes. Frames that are whole, and packets
  without any header of the stream while nothing is buffered (damaged
  ones), pass through untouched. The work is bounded: the buffer holds at
  most a packet and 8 KiB, within which a free-format frame's end or a
  run's end is looked for. Known limits: a free-format stream cut anywhere
  whose packet boundary falls on a frame boundary passes that packet whole
  (its header gives no length), and the packets after it, without a header,
  pass as damaged until one holds a header again; a damaged packet of an
  ordinary stream that happens to hold a sync word of the stream is
  buffered, and the false frame can take the start of the next real one
  (two or three frames concealed instead of one).
- **QuickTime `ms\0U` entries** (MP3 under its WAVE format tag 0x0055)
  are not decoded: their samples follow the QuickTime version-1 sound
  description (one sample per PCM frame, packets defined by bytes per
  packet), which `coreav-isobmff`'s packet reader does not implement, and
  they stay `other` in `coreav probe`. Eleven corpus `edge` files have
  them; the first agreement run found them, and the choice is recorded
  after it. Reading them needs that sound description in the MP4
  demuxer, a change of its own.
- **Intensity stereo of MPEG-2**: the largest value of a band's
  intensity position (its `slen` bits all set) is illegal, and the band
  is decoded as middle/side, or as two channels without middle/side
  (13818-3 2.4.3.2). `ffmpeg` 7.0.2 decodes a 1-bit position of 1 as a
  legal position instead (a black-box test: its right channel is the
  left one times √2 where CoreAV's equals it), so on such streams the two
  differ by design.
- **CRC**: checked when present; a mismatch keeps the samples (the frame
  parsed) with a `FrameCrc` warning, as for FLAC's CRC-16.
- **Trimming**: the container's (the edit list, `CodecDelay`,
  `DiscardPadding`; "Pipeline"). The LAME tag of a raw MP3 file is an earlier change's
  to turn into delay and padding.
- **Speed** (the bars below are against FFmpeg's float decoder with its
  x86 assembly, from portable Rust): the 18-point DCT-IV inside the long
  IMDCT and the synthesis filter's 32-point DCT are unrolled from
  even/odd factorizations, and both run four lanes at a time (four
  subbands, four time slots) in a type the compiler maps to vector
  registers; requantization happens during Huffman decoding, with the
  gain of each scalefactor band precomputed; the bit reader refills
  without a branch, and sign bits are applied without one. Rejected:
  `unsafe` SIMD intrinsics with run-time CPU detection, which would need
  a kernel crate under the `unsafe` policy; to be measured if a bar is
  missed.

### MP3 bars

Fixed before the measured runs; the evidence goes to
`docs/spikes/audio-decode-bench/` and the tests named below.

1. **Conformance (ISO/IEC 11172-4).** The Layer III compliance package
   (Fraunhofer IIS, v2.2, 1994; `layer3.tar.gz`, pinned by SHA-256 in the
   workflow): `compl.bit` decodes to its reference output `compl.hex`
   within full accuracy, the RMS of the difference below 2^-15 / √12
   (8.8 × 10^-6) and no difference above 2^-14 (6.1 × 10^-5), relative to
   full scale; every other stream of the package (`he_32khz`, `he_44khz`,
   `he_48khz`, `he_free`, `he_mode`, `hecommon`, `si`, `si_block`,
   `si_huff`, `sin1k0db`) decodes without an error. Bar: all of this, in
   CI (`.github/workflows/audio-conformance.yml`).
2. **Agreement with `ffmpeg` (within twice full accuracy).** On every input
   of the two sets below, `coreav decode-audio --format f32` against
   `ffmpeg -i IN -map 0:a:K -f f32le -` (its default MP3 decoder,
   `mp3float`): the RMS of the difference at most 2 × 2^-15 / √12
   (1.8 × 10^-5) and no difference above 2 × 2^-14 (1.2 × 10^-4). Two
   decoders that each meet full accuracy can differ by the sum of the
   bounds, so twice them is the tolerance the standard justifies. The
   comparison aligns the trimming: CoreAV applies Matroska's `CodecDelay`
   (RFC 9559); `ffmpeg` 7.0.2 does not for MP3 (a black-box test), so its
   output is compared from `CodecDelay` samples on, and the outputs'
   lengths must differ by exactly the trimming that one applies and the
   other does not. Bar: 100% of the inputs `ffmpeg` decodes without an
   error. An input `ffmpeg` decodes with an error, that CoreAV refuses by a
   choice above, or whose frames change the channel count (the mapping is
   a choice; at `f32`, `ffmpeg`'s resampler uses other gains) is listed
   with its reason and does not count against the bar. Reported: the
   share of inputs whose default `--format s16` WAV data equals `ffmpeg`'s,
   and the largest difference in 16-bit steps.
   Since an earlier change, MP4 inputs with an edit list are judged by "Edit list
   bars" instead; the recorded result measured the binary before it.
3. **Speed.** As FLAC's bar 3, on the MP3 sets: the median of
   the per-input ratios CoreAV / FFmpeg at most **1.00** on the held-out
   set, overall and for each class (MP4, Matroska), for the library
   (`coreav_audio::decode` to interleaved `s16` in memory against
   libavformat and libavcodec 7.0.2 in-process, `mp3float` and
   `swr_convert` to `s16`, one thread each) and for the command (`coreav
   decode-audio IN OUT.wav` against `ffmpeg -i IN -map 0:a:K OUT.wav`,
   each with its default threads). Reported with each: the share of
   held-out inputs on which CoreAV is faster (ratio below 1.00), overall
   and per class. Reported, not gating: the decoder alone, and the corpus
   set.
4. **Safety.** A fuzz target `mp3_frame` (the decoder on arbitrary frames,
   with arbitrary earlier frames in its reservoir) beside `decode` (the
   whole pipeline), 60 seconds each in CI; no panics.
5. **Regression guard.** A `decode-audio-mp3` case of the CI benchmark
   guard on a generated MP3 fixture.

### MP3 sets

- **Derived (gating for speed, counted for agreement).** The sources of the
  FLAC derived set (every `real` corpus v1 file with `holdout: true` and an
  audio stream), from their Matroska FLAC encode (the source's first audio
  stream, decoded): encoded by the pinned `ffmpeg` 7.0.2 with `libmp3lame`,
  audio only, once into Matroska (`.mka`) and once into MP4 (`.mp4`). The
  source's SHA-256 picks the parameters: its first byte modulo 5 picks
  128, 192 or 320 kbit/s CBR, or VBR quality 2 or 5; its second byte
  modulo 4 picks mono for 0, else the source's channels (at most 2); the
  sample rate is the source's when MP3 has it, else 48 kHz above 48 kHz
  and 44.1 kHz otherwise. `libmp3lame` lowers a bit rate that a sample
  rate does not allow; the record keeps what was written.
- **Corpus MP3 (counted for agreement, reported for speed).** The corpus
  v1 files with an MP3 stream in MP4, MOV or Matroska, as stored, and the
  raw MP3 files remuxed by `ffmpeg` (stream copy) into Matroska until
  an earlier change's demuxer reads them as they are.
- Method: as for FLAC.

## Opus

Scope: Opus tracks of MP4 (`Opus`/`dOps`, the Xiph "Encapsulation of Opus
in ISO Base Media File Format") and Matroska/WebM (`A_OPUS`, `OpusHead` in
`CodecPrivate`), decoded to 48 kHz, the rate of every Opus stream's
timestamps and of `ffmpeg`'s output. Specifications: RFC 6716 as updated
by RFC 8251 (the codec), RFC 7845 (`OpusHead`, pre-skip, channel mapping
families 0, 1, 2 and 255).
Output channels: family 0 and 1 streams in WAV order with their speaker
mask (family 1's Vorbis order moved to WAV's), so that a downmix reads
the layout from the mask; families 2 (ambisonics) and 255 (no layout)
in coded order with no mask. Prior art: no implementation's source was
read for this note; libopus and `ffmpeg` ran as black boxes. The
decoder's design note, in `coreav-opus`, names the reference code read.

### The decoder: native or libopus

Two ways meet the roadmap's rule ("CoreAV's own or permissively licensed
backends"): a native decoder in safe Rust, or libopus (BSD-3-Clause, the
reference implementation) in a decoder-backend crate run in the sandboxed
worker.

**What a native decoder needs.** RFC 6716 makes its reference source code
(Appendix A) the normative specification (section 1, section 6: "should
the description contradict the source code of the reference
implementation, the latter shall take precedence"), and a compliant
decoder must end each packet in the reference's range decoder state
(section 6). The prose is complete for the range decoder, the packet
layer (section 3) and SILK (section 4.2, with its tables), but not for
CELT, where every gap changes which symbols are read next, so the decoder
desynchronizes on the first frame that uses it:

- tables the prose names but does not print: `cache_caps50` ("Implementations
  are expected to simply use the same table data", 4.3.3), `e_prob_model`
  (4.3.2.1), `LOG2_FRAC_TABLE` (4.3.3) and the pulse cache that limits the
  values of K for each N (4.3.4.1);
- procedures stated in a sentence: the inter-frame prediction
  coefficients of coarse energy for each frame size (4.3.2.1), split
  decoding with its gain parameter, its probability model and the bits
  given to each half (4.3.4.4, one paragraph), anti-collapse's
  pseudo-random signal and renormalization (4.3.5).

The decoder is therefore written from that code (RFC 6716 Appendix A
with the changes of RFC 8251 sections 3 to 10), which is the
specification: it takes the code's normative tables, constants, integer
arithmetic and order of reads, while its structure, data layout,
floating-point arithmetic and speed work are CoreAV's own, with no
function-by-function translation. Where a bar compares with the reference
output, it also matches the code's non-normative parts (the SILK
resampler, for example): the conformance test's 48 kHz output has passed
through the reference's SILK resampler, whose phase response no
linear-phase design of ours matched.

The measurements that led to matching the reference's resampler
(testvector02, NB, before the resampler was read; `opus_compare -s -r 48000` against `testvector02m.dec`, which
passes at an internal error below about 0.27):
- `opus_compare` at 48 kHz is waveform-sensitive: the reference output
  itself, delayed by 1 sample, scores 93.6; by 2 samples, 87.5; with white
  noise added at 48 dB SNR, 88.3.
- Our SILK-rate output, in floating point, was within 48 dB SNR of the
  reference decoder's at 8 kHz, but every linear-phase upsampler designed
  for it (windowed sinc, least-squares designs) failed outright, with
  errors from 0.28 to 2.4: the reference resampler's measured group delay
  varies by tens of samples across 0 to 4 kHz.
- A least-squares FIR per output phase, fitted from the reference's
  bit-exact SILK-rate output to its 48 kHz output (a black-box
  identification), reached 96.0, 96.7 and 97.1 on vectors 02 to 04, an
  approximation that matching the reference's resampler made unnecessary.

With the reconstruction in the reference's integer arithmetic and its
resampler (`coreav-opus`'s crate documentation), the SILK-only vectors'
output equals the reference decoder's, built from the RFC's code, sample
for sample at 48 kHz, stereo and mono; `opus_compare` scores the stereo
decodes 100.0 and the mono ones 96.2 to 96.7 (against the stereo
reference, as section 6.1 runs it).

**Comparison.**

| | Native (safe Rust) | libopus backend |
|---|---|---|
| Effort | Large: range decoder, SILK, CELT, hybrid, transitions and redundancy, multistream; several thousand lines | Small: FFI to the multistream decoder, an audio path in `coreav-worker`, a build of libopus for every target |
| Conformance | Earned: RFC 8251 vectors, final range state per packet | By construction (it is the reference) |
| Speed | Unmeasured. The MP3 decoder in portable Rust met its bars against FFmpeg's assembly-backed `mp3float` (library 0.940, decoder alone 0.978, `docs/spikes/audio-decode-bench/`) | No faster than FFmpeg's decoder, in two indicative probes (`evidence/opus-backend-probe.json`, not held-out): the distribution's libopus 1.3.1, its decode loop alone, took 0.85 to 1.08 of `ffmpeg`'s whole command with FFmpeg's own decoder (5 inputs; the comparison favours libopus, which is spared demuxing, writing and process start); inside the oracle `ffmpeg` build, whose libopus build flags are unknown, libopus took a median 3.0 times FFmpeg's own decoder (13 inputs). The sandbox adds a process start and a pipe per request, which the library bar's short inputs pay in full |
| Safety | No C on the untrusted-input path; panics and limits as every other crate | The first C decoder of a default build; it runs in the worker, which gives crash isolation and resource limits but no confinement yet (`decode.md`, "The sandboxed worker"); in the calling process, none |
| Deployment | Pure Rust: wheels, `coreav-wasm` and static builds unchanged | A C toolchain or a system libopus on every default-build target, the WebAssembly build included |

**Choice: native.** Opus is the audio of WebM uploads, browser and
WebRTC recordings and voice messages, untrusted inputs by nature, and
removing C from that path is CoreAV's first value, safety.
The speed probe gives libopus no advantage to set against that, and the
sandbox costs it time on short inputs. The effort is the price. Rejected:
libopus as the default decoder, for the safety and deployment rows;
libopus now and native later, which builds the worker's audio path and a
C build for every target only to remove them; a port of the reference
decoder, license-compatible but without the design freedom the speed bars
need.

**Crate: `coreav-opus`** (MIT OR Apache-2.0, `forbid(unsafe_code)`, no
dependencies), which `coreav-audio` uses as it uses its own decoders:
the identification header, the packet layer and the decoder. A crate of
its own because the `NOTICE` entry (the BSD notices the reference code
carries, added by the first change that takes content from it) is scoped
to it. Rejected: a module of `coreav-audio`, which would put
the notice on the FLAC and MP3 decoders too.

### Opus bars

Fixed before the measured runs; the evidence goes to
`docs/spikes/audio-decode-bench/` and the tests named below. They hold for
either decoder.

1. **Conformance (RFC 6716 section 6, RFC 8251 section 11).** The RFC 8251
   test vectors (the archive section 11 names, checked against the SHA-1
   hashes it prints and pinned by SHA-256 in the workflow),
   `testvector01` to `testvector12`, by the procedure of RFC 6716 section
   6.1 at 48 kHz: each decoded to stereo and to mono, the final range
   decoder state after every packet equal to the one the `.bit` file
   records, and each output passing `opus_compare -r 48000` (with `-s`
   for stereo) against the reference output of one of section 11's two
   sets (`.dec` or `m.dec`, the same set for all vectors), with a quality
   above 90, the value section 6.1 recommends for 48 kHz. `opus_compare`
   is built in CI from libopus 1.6.1, and run as a black box; its source is not read. Bar: all 24 decodes, in CI
   (`.github/workflows/audio-conformance.yml`).
2. **Agreement with the reference, through `ffmpeg`.** On every input of
   the sets below, `coreav decode-audio --format s16` against `ffmpeg
   -c:a libopus -i IN -map 0:a:K -f s16le -` (libopus is the normative
   decoder): passing `opus_compare -r 48000`, with `-s` for two channels,
   each channel alone for more than two. The tolerance is the specification's own conformance criterion applied to
   real inputs; Opus decoders are not bit-exact (floating point, a
   non-normative SILK resampler). The lengths must be equal: pre-skip,
   `CodecDelay`, `DiscardPadding` and the edit list trim the same samples.
   Bar: 100% of the inputs `ffmpeg` decodes without an error; an input
   CoreAV refuses by a choice of this note is listed with its reason and
   does not count. Reported, against `ffmpeg`'s default decoder (`opus`,
   the one its users get): the share within `opus_compare`, the SNR and
   the largest difference in 16-bit steps.
3. **Speed.** As MP3's bar 3, against FFmpeg's default Opus
   decoder (`opus`): library (`coreav_audio::decode` to `s16` in memory
   against libavformat and libavcodec 7.0.2 in-process with `opus` and
   `swr_convert` to `s16`, one thread each) and command (`coreav
   decode-audio IN OUT.wav` against `ffmpeg -i IN -map 0:a:K OUT.wav`,
   default threads), median per-input ratio at most **1.00** on the
   held-out set, overall and for each class (MP4, Matroska/WebM),
   reported with the share of inputs on which CoreAV is faster. Reported,
   not gating: the decoder alone, the corpus set, and each Opus mode
   (SILK, hybrid, CELT) of the derived set.
4. **Safety.** A fuzz target `opus_packet` (the decoder on arbitrary
   packets after arbitrary earlier ones, every channel mapping) beside
   `decode`, 60 seconds each in CI; no panics. Limits, checked before
   any decoding: `OpusHead` as RFC 7845 5.1.1 bounds it (output channels
   1 to 255 and at most `max_channels`; at least one stream; coupled
   streams at most the streams, and the two together at most 255; every
   mapping index below their sum or 255; each family's channel counts),
   packets at most 120 ms, every stream of a sample of the same
   duration, and the frame sizes of RFC 6716 section 3.4.
5. **Regression guard.** A `decode-audio-opus` case of the CI benchmark
   guard on a generated Opus fixture.

### Opus sets

- **Derived (gating for speed, counted for agreement).** The sources of
  the FLAC derived set, from their Matroska FLAC encode, encoded by the
  pinned `ffmpeg` 7.0.2 with `libopus`, audio only, once into WebM and
  once into MP4, VBR. The parameters go round in turn over the sources
  sorted by SHA-256, so that every value occurs about equally often
  whatever the set's size: the i-th source takes 12, 24, 32, 64, 128 or
  256 kbit/s by i modulo 6 (SILK, Hybrid and CELT all occur), frames of
  2.5, 5, 10, 20, 40 or 60 ms by (i / 6) modulo 6, `-application voip`
  when i modulo 4 is 0 and `audio` otherwise, and mono when i modulo 5 is
  0, else the source's channels (at most 2). The record keeps the mode
  and frame size of each packet's TOC byte, and reports the speed per
  mode.
- **Multichannel (counted for agreement).** The first 14 sources of the
  derived set, by SHA-256, upmixed by `ffmpeg`'s `aformat` to a layout
  and encoded with `libopus` into WebM at 64 kbit/s per channel: mapping
  family 1 at 3, 4, 5, 6, 7 and 8 channels (two sources each) and
  mapping family 255 at 2 and 3 channels (one each); with the corpus files
  of families 2 and 255, they exercise multistream packets and the WAV
  channel order.
- **Corpus Opus (counted for agreement, reported for speed).** The corpus
  v1 files with an Opus stream in MP4, MOV, Matroska or WebM, as stored
  (104 files; the 4 Ogg ones once an earlier change reads them).
- Method: as for FLAC.

## AAC-LC

Native, opt-in, written from ISO/IEC 14496-3:2009 (subpart 1: the
`AudioSpecificConfig`; subpart 4: the general audio syntax, the decoding
process and the tables of Annex 4.A), with ISO/IEC 13818-7:2004 (MPEG-2
AAC) as a cross-check of the tables the two share. Crate `coreav-aac`;
`coreav-audio` and `coreav-cli` reach it through a cargo feature `aac`
that is off by default.

### Packaging: opt-in, separately packaged, with a disclaimer

The sources disagree on whether patents on AAC-LC are still in force,
so AAC-LC ships as H.264 does
(`docs/design/h264.md`, "Packaging"):

1. No default build contains the decoder. `coreav-audio`'s feature `aac`
   and `coreav-cli`'s feature `aac` turn it on; nothing else depends on
   `coreav-aac`. A default build reports an AAC stream as today: `codec
   aac is not decoded` (`unsupported_feature`). The crate is on
   the patented list of `scripts/patent_free_check.py`, so CI fails when
   the default build of any workspace member, or a package script's
   build, links it (`cargo tree`).
2. `crates/coreav-aac/README.md` and the crate's documentation state the
   patent position and that the crate is not enabled by default; the
   feature's comment in each `Cargo.toml` repeats it.
3. Publishing anything that contains it, and enabling it by default, are
   the maintainer's, after the patents expire and the project's patent
   review.

The crate is a workspace member, so CI builds, lints, tests and fuzzes it.
`coreav-aac` depends on nothing (it has its own bit reader and error
type), so the decoder can be packaged without the rest of CoreAV, and
`coreav-audio` wraps it as it wraps its own decoders. Rejected: a module
of `coreav-audio` behind a feature, which a separate package could not
hold apart; one crate with the pipeline, which would make the patent
question every pipeline user's.

### Scope

- **Decoded**: audio object type 2 (AAC LC), and MPEG-2 AAC LC (the MP4
  object type indications 0x40 and 0x67), with frames of 1,024 or 960
  samples (`frameLengthFlag`), every sample rate of Table 1.18, channel
  configurations 1 to 7, the configurations 11 (6.1), 12 (7.1 with back
  surrounds) and 14 (7.1 with front heights) that ISO/IEC
  14496-3:2009/Amd.4:2013 adds (as ITU-R BS.1196-6 Table 2 quotes its
  Table 1.19), and a `program_config_element` (configuration 0).
  Configuration 13 (22.2) is refused by name. The elements: SCE, CPE, LFE, CCE (coupling, 4.6.8.3), DSE and FIL
  (skipped), PCE (only the configuration's counts), END. The tools:
  section data, scalefactors with intensity and noise positions, pulse
  data, spectral data of codebooks 1 to 11 with escapes, M/S stereo,
  intensity stereo, perceptual noise substitution (PNS), temporal noise
  shaping (TNS), and the filterbank with all four window sequences and
  both window shapes (sine and Kaiser-Bessel derived).
- **Refused by name** (`codec ... is not decoded`): AAC Main, SSR and LTP
  (object types 1, 3, 4), the error-resilient and scalable types, and
  HE-AAC: SBR or PS signalled explicitly (object types 5 and 29, or the
  backward-compatible extension 0x2b7 with `sbrPresentFlag` set) or
  implicitly (an SBR extension payload, `EXT_SBR_DATA` or
  `EXT_SBR_DATA_CRC`, in the first packet). Decoding only the LC core of
  an HE-AAC stream would write half the sample rate, which is not the
  stream. HE-AAC goes to the sandboxed fallback
  once `decode-audio` has one; until then it is refused. An SBR payload
  in a later packet ends the request (`unsupported_feature`).
  `dependsOnCoreCoder` is refused as unsupported.
- **Containers**: MP4/MOV/M4A (`esds`) and Matroska/WebM, the containers
  `decode-audio` reads. ADTS and LATM framing (raw `.aac` files, MPEG-TS)
  are not read by the pipeline, so they are not decoded yet.
- **Not applied**: dynamic range control and the PCE's matrix mixdown
  (both optional for a decoder, 4.5.2.7 and 4.5.1.2.2), ancillary data.
  Every channel is written, at the stream's own level.
- **Configuration**: the `AudioSpecificConfig` in MP4's `esds` (its
  `DecoderSpecificInfo`, which `coreav_isobmff::esds_decoder_specific_info`
  returns) or in Matroska's `CodecPrivate`. A Matroska track with a legacy
  codec ID (`A_AAC/MPEG4/LC`, `A_AAC/MPEG2/LC`) and no `CodecPrivate` gets
  the configuration its ID, sample rate and channels state (channel
  configurations 1 to 6 for 1 to 6 channels, 7 for 8); the legacy IDs of
  SBR (`.../LC/SBR`), AAC Main, SSR and LTP are refused by name. The
  output's WAV channel mask is the configuration's (above), not the
  default for its channel count.

### Choices

- **Channel order and elements**: output is in WAV's order (front left,
  right, centre, LFE, back, side), the order `ffmpeg` writes:
  configuration 3 (C, L, R) becomes L, R, C, configuration 6 L, R, C,
  LFE, Ls, Rs, and so on (Table 1.19); configuration 11's surround pair
  beside its back centre is WAV's side pair, and 12's pairs at ±90° and
  ±135° are the side and back pairs, so 6.1 and 7.1 come out as
  `ffmpeg` writes them. A PCE's channels go front, side,
  back, then LFE, in the PCE's order within each; its WAV mask is that of
  configurations 1 to 7 when the layout is one of theirs, else 0. An
  element goes to the slot its configuration names its
  `element_instance_tag` for (a PCE names a tag per element; the
  conformance stream `al17` needs this), else to the first free slot of
  its type, so elements without named tags, and streams whose tags
  disagree with a channel configuration's, go in their order of
  appearance. Rejected: tags only, which fails such streams; order only,
  which swaps the channels of `al17`. A frame that lacks an element of
  the configuration, or holds one too many, is damaged (concealed).
- **Coupling channels**: a CCE's spectrum is added to its targets' by
  gain cc_scale^(-gain), with cc_scale = 2^(2^gain_scale / 8) (4.6.8.3.3,
  Table 4.154). The standard's pseudo-code writes cc_scale^gain; the
  conformance references (`al07`) attenuate as the gain element grows,
  so they decide, and the exponent is negated. Bands the CCE codes as
  zero (`ZERO_HCB`) are skipped, as the second `couple_channel()` of
  4.6.8.3.3 does, even where the CCE's own TNS filter has spread energy
  into them; `al07` at 48 kHz and up needs this (round 1 missed those
  eight streams without it). A CCE whose target element is not in the
  frame couples into nothing.
- **Numerics**: 32-bit floating point throughout, full scale ±1.0 (the
  standard's 16-bit full scale is 32,768). |q|^(4/3) comes from a table of
  the 8,192 values a codeword can carry, and 2^((sf - 100) / 4) from the
  exponent bits times one of four constants; the windows, the FFT's
  twiddles and the table are generated (`tables/gen_aac.py`), so the
  decoder links no C math library (the start-up cost recorded under MP3).
- **IMDCT**: a DCT-IV of N/2 points through a complex FFT of N/4 points
  (512 and 64 for frames of 1,024; 480 and 60 for 960) between a pre- and
  a post-twiddle. The FFT is decimation in frequency, in place, by passes
  of radix 2, 3, 4 and 5 whose butterflies combine values a span apart, on
  blocks of four complex values (four real parts, then four imaginary
  parts), so a radix-4 butterfly reads four blocks and one block of
  twiddles per output. The radices are ordered so that every in-place span
  is a multiple of four: a butterfly then runs on four values at a time,
  in arrays of four floats that the compiler keeps in vector registers.
  Each butterfly (and each step of the twiddles) is a function the
  compiler does not inline into its loop: inlined, the loop vectorizer
  may vectorize across iterations instead, gathering lanes one by one,
  which it did for two of the passes under link-time optimization. The
  last pass, of radix 4 and span 1, reads its blocks in the order of their
  outputs, transposes four blocks at a time and writes the outputs in
  natural order, so no reordering pass (bit reversal) runs. The DCT-IV's
  output stays in two arrays, its even and its reversed odd values; a long
  block is windowed and added to the overlap straight from them, two
  samples at a time, the unfolding to 2N samples folded into the windowing
  loops. The windows of a long block's halves (long, or LONG_STOP's and
  LONG_START's zeros, short slope and ones) are built once per stream, so
  the four long window sequences run one loop. Rejected: a self-sorting
  (Stockham) FFT out of place, the first version, which spent about 20 µs
  per long IMDCT on element-wise copies and index arithmetic; separate
  arrays of real and imaginary parts, which made a radix-4 pass walk 14
  pointers and spend a third of its instructions moving them between
  registers; a reordering pass, a gather per value that the transposed last
  pass avoids.
- **Noise substitution**: 4.6.13 fills a noise band with random values
  scaled to the band's energy; the generator is the decoder's choice.
  CoreAV seeds a 32-bit linear congruential generator per frame from a
  hash of the packet's bytes and the packet's position in the stream, so a
  frame's noise is a function of the packet and its index: output is the
  same on every thread count and on every run. The position matters
  because encoders write the same packet frame after frame for stationary
  noise (`al18` of the conformance package does); seeded by the bytes
  alone, the noise would repeat every frame, a periodic buzz (round 1 of
  the conformance bar found it). No two decoders write the same noise, so the bars below compare
  those samples by level only. A pair of channels whose band is noise in
  both with `ms_used` set shares one random vector (4.6.13.3).
- **Intensity under an all-ones M/S mask**: in a channel pair whose
  `ms_mask_present` is 2 (`ms_used` all ones), an intensity-coded band
  keeps its direction: ISO/IEC 13818-7 12.2.3 defines
  `invert_intensity(group, sfb)` as 1 - 2 × `ms_used` only when
  `ms_mask_present` is 1, and +1 otherwise. `ffmpeg`'s decoder reverses
  those bands (black box: on its own encoder's `-aac_ms 1` output, its
  right channel's intensity bands have the opposite sign of CoreAV's, and
  the two agree to 6 × 10^-8 when CoreAV reverses them too). CoreAV follows
  the standard and reports the bands per frame
  (`Frame::intensity_all_ms_bands`); the bars compare those frames by
  level ("AAC-LC bars"). Rejected: matching `ffmpeg`, which departs from
  the standard's pseudo-code.
- **Damage**: a frame that fails to decode resets the decoder's state
  (the overlap is zeroed, the window shape is the sine), so the state
  after any packet depends on that packet alone. A frame of an LC stream
  that sets `predictor_data_present` or `gain_control_data_present`
  (tools of AAC Main and SSR, which an LC stream cannot carry) is damage,
  not a refusal of the stream.
- **Threads**: a frame depends on the one before only through the overlap
  (the second half of its windowed IMDCT output) and its window shape,
  both set by that one frame. A job after the first is therefore primed
  with one packet and told its first packet's position
  (`Decoder::set_position`, for the noise seed), and its first frame
  decodes as in one pass, bit for bit. Tests compare 1, 2, 3 and 8 threads, with noise bands and with
  damaged packets.
- **Limits**: `max_channels` bounds the configuration (a PCE can name 48
  channels plus LFEs); every frame codes 1,024 or 960 samples per channel,
  so `max_output_samples` bounds the work; the number of elements in a
  `raw_data_block` is bounded by its bits and by the configuration (one
  more element of a type than configured is damage), and each fill
  element by its 15 + 255 bytes.
- **Speed** (portable Rust, no claim against FFmpeg; "AAC-LC bars"): Huffman codewords are read with two-level lookup tables
  (a first level of at most 9 bits, a second level per long prefix). The
  spectrum's codewords are read through a 64-bit register that a refill
  tops up to 56 to 63 bits without a branch, once every two codewords (a
  codeword and its sign bits take at most 20 bits), so the lookup of the
  next codeword waits on a shift rather than a load; one register serves
  the whole of `spectral_data()`. All spectrum codebooks share one lookup
  table and one table of value vectors, both of a power-of-two size, so a
  masked index needs no bounds check and the tables need no register. A
  codeword's leaf holds its vector's place, a bit per non-zero value and
  the number of sign bits that follow; the vector holds |v|^(4/3) of the
  codeword's values (signed in the signed codebooks), and the sign bits
  select a mask of sign bits from a table of 256, so a codeword's values
  are inverse quantized, given their signs and scaled by the band's gain
  with no branch and no per-value lookup. The only branch on data per
  codeword is the second-level lookup; signs are random, so a branch on
  them would mispredict half the time. A band holding a pulse (4.6.3.3,
  which adds to quantized magnitudes) decodes to integers first: the
  index's digits, the signs, the pulses, then |q|^(4/3) from the table.
  Tests pin both paths to the same bits and the same values on random bits
  through every codebook. TNS filters in place, reading the outputs already
  written as its state. The noise generator's seed, a hash of the packet's
  bytes, is computed on the first noise band of a frame, as most frames
  have none. Rejected: decoding every band to integers and inverse
  quantizing in a second pass (a pass over the spectrum and a lookup per
  value).

### AAC-LC bars

Fixed before the measured runs, which run on GitHub Actions
(`.github/workflows/audio-conformance.yml`, job `aac`); the evidence goes
to `docs/spikes/audio-decode-bench/` and the tests named below. The
reference is the pinned FFmpeg 9.0.2 (the build `scripts/oracle-ffmpeg.env`
names once that pin lands; the job installs the same archive by its SHA-256
until then), its default AAC decoder `aac`, floating point. Two rules hold
for every comparison:

- **Noise bands**: a frame k that codes a noise band (codebook 13) in any
  channel makes output blocks k and k + 1 (its samples and the overlap
  into the next frame) random by definition. The decoder reports, per
  frame, how many noise bands it decoded (`Frame::noise_bands`, a raw
  value of the bitstream). Those blocks are excluded from the sample
  comparisons and counted. The energy of a noise band is specified and its
  phase is not, so they are compared by level: per channel, the level of
  all its noise blocks over the whole stream, |10 log10 (energy CoreAV /
  energy other)|, must be at most **0.1 dB** (one step of a coded noise
  energy is 1.5 dB) on every channel with at least **100** noise blocks;
  a channel with fewer is reported, since the overlap of a noise block
  with its neighbours adds a random term that a few blocks do not average
  out. The median over blocks of the per-block |20 log10 (RMS CoreAV / RMS
  other)| is reported, not gating.
- **Intensity under an all-ones mask**: a frame k with intensity bands
  under an `ms_mask_present` of 2 (`Frame::intensity_all_ms_bands`, a raw
  value) makes blocks k and k + 1 differ in sign where a decoder reads that
  mask the other way ("Choices"), so they are treated as noise blocks:
  excluded from the sample comparisons, counted, and compared by level
  with the rule above. Added after the bars above and before the first
  run, from a development encode on which `ffmpeg` reverses those bands.
- **Tolerance**: twice full accuracy, per channel: the RMS of the
  difference at most 2 × 2^-15 / √12 (1.8 × 10^-5) and no difference above
  2 × 2^-14 (1.2 × 10^-4) of full scale, as for MP3.

1. **Conformance (ISO/IEC 14496-26:2010, the bitstreams of the MPEG-4
   audio conformance of 14496-4, public at standards.iso.org, pinned by
   SHA-256 in `crates/coreav-aac/tests/conformance.sha256`).** Every AAC LC
   stream in MP4, `compressedMp4/al00` to `al19` at every sample rate and
   with both frame lengths (`alNN_RR` and `alNNsf_RR`), against its
   waveform in `referencesWav` (one file per channel for multichannel
   streams): per channel, the RMS of the difference at most 2^-15 / √12
   (8.8 × 10^-6) and no difference above 2^-14 (6.1 × 10^-5) of full
   scale, the full-accuracy bounds of the MP3 bar. The references do not
   all start with the decoder's first sample, so each stream is aligned at
   the offset, within ±4,096 samples, with the smallest RMS of the
   difference over the reference's loudest 8,192 samples, and compared over
   the overlap; a stream whose frames are mostly noise bands is aligned on
   its log-energy envelope instead (64-sample hops). The references are
   integer PCM, so CoreAV's output is clipped to their range before the
   difference; channels are matched to the reference files by the smallest
   RMS difference, the files being in the package's order, not WAV's. Every
   HE-AAC stream of the package in MP4 (`al_sbr_*`, `al960_sbr_*`) is
   refused by name. Nothing panics. Bar: every counted stream, in CI.
2. **The decoder against FFmpeg 9.0.2 (gating).** On every stream of bar 1
   and every MP4 input of the generated set ("AAC-LC sets"): CoreAV's
   `coreav_aac::Decoder` on every packet in order, nothing trimmed,
   against `ffmpeg -ignore_editlist 1 -i IN -map 0:a:0 -f f32le -`, which
   writes every packet's samples from the first packet on. Sample 0 is
   compared with sample 0; both lengths must be the packets times the frame
   length; channels are compared in their order (WAV's), except a program
   configuration (channel configuration 0), whose channels are matched by
   the smallest RMS difference as in bar 1 and the match reported. The
   tolerance and the noise rule above; CoreAV damages no frame. Bar: 100%
   of the inputs `ffmpeg` decodes without an error. An input that `ffmpeg`
   decodes with an error, or that CoreAV refuses by a choice above, is
   listed with its reason and does not count. A divergence found in a run
   is a known divergence only where a raw value of the input shows its
   condition, never by file. Reported: the largest difference, the share
   of samples in noise blocks, the noise level per block.
3. **Edit lists (gating).** On every MP4 input of the generated set
   without noise bands or intensity bands under an all-ones mask (every
   frame's `Frame::noise_bands` and `Frame::intensity_all_ms_bands` are 0
   in bar 2's decode): `scripts/decode_audio_check.py`'s presentation model
   ("Edit list bars", bar 1): CoreAV's `decode-audio --format f32` output
   equals `ffmpeg -ignore_editlist 1`'s decode cut by the edit list as the
   oracle's own reader reads it, within the tolerance, the length exact,
   and the summary's `timeline` is the model's. Bar: 100% of those inputs.
   Reported: the same on the inputs with noise bands (their samples differ
   by definition), and every input's comparison with `ffmpeg`'s default
   output (edits applied its way: `equal`, `covered` by an entry of
   `docs/known-divergences-decode-audio.md`, or `uncovered`).
4. **Matroska (reported).** Every Matroska input of the generated set:
   `coreav decode-audio --format f32` against `ffmpeg -i IN -map 0:a:0 -f
   f32le -`, sample 0 to sample 0 over the common length, with the noise
   rule; both lengths.
5. **Threads (gating).** On every input of the generated set, both
   containers, the output of `--threads 8` is byte-identical to that of
   `--threads 1`; tests pin 1, 2, 3 and 8 threads on fixtures with noise
   bands and damaged packets.
6. **Safety.** Fuzz targets `aac_frame` (the decoder on arbitrary frames
   under arbitrary configurations, `AudioSpecificConfig` and program
   configuration parsing included) and the `decode` target with the `aac`
   feature (the `esds` descriptor walk and the pipeline), for the fuzz
   job's time per target in CI; pinned tests that a flood of frames stops
   at `max_output_samples` and that a configuration past `max_channels` is
   refused; no panics.
7. **Regression guard.** `decode-audio-aac-mp4` and `decode-audio-aac-mkv`
   cases of the CI benchmark guard, in its decoder build (which gains the
   `aac` feature), on the AAC audio of `basic.mp4` copied into MP4 and
   Matroska.

No speed against FFmpeg is claimed for AAC-LC: a claim needs its own bars
and a timed run on a fixed host ("Speed" above records the design).

### AAC-LC sets

- **Conformance**: the streams of bar 1.
- **Generated**: written in the job by the pinned `ffmpeg`'s `aac` encoder
  from sources it synthesizes, 10 seconds each, into MP4 (`.m4a`, with the
  edit list `ffmpeg` writes) and, by stream copy, into Matroska
  (`.mka`). `scripts/aac_agreement.py` holds the list. The sources:
  `tones` (220, 1,000 and 5,000 Hz, amplitude-modulated at 3 Hz), `noise`
  (pink noise), `clicks` (a 440 Hz tone with a full-scale 1 ms click every
  250 ms, which makes the encoder switch to short windows) and `sweep` (a
  linear sweep from 50 Hz to 16 kHz); multichannel inputs give each
  channel its own frequency. The groups:
  - **Rates**: each source at 8, 11.025, 16, 22.05, 24, 32, 44.1, 48, 64,
    88.2 and 96 kHz, mono and stereo, PNS off (88 encodes);
  - **PNS**: `noise` and `tones`, stereo, at every rate above, the
    encoder's defaults (PNS on; 22);
  - **Tools**: each source at 48 kHz stereo with intensity stereo off, M/S
    forced, TNS off, the fast coder, 32 kbit/s and 256 kbit/s, PNS off
    (24);
  - **Layouts**: `tones` with `clicks` at 48 kHz in the layouts 3.0, 4.0,
    5.0, 5.1, 6.1 and 7.1, with the channel configuration and with a
    program configuration (`-aac_pce 1`), PNS off (12).
  An encode the pinned `ffmpeg` refuses (a layout it has no
  configuration for) is listed and not counted.
- Not used: corpus files. ADTS and LATM are not read by the pipeline
  ("Scope"), so they have no set.

### AAC-LC results

**Round 1**, on `fa508ad`, in the `aac` job of `audio-conformance`
against FFmpeg `n9.0.2-17-g2a571b6068`: bar 1 met (336 LC streams within
full accuracy, 105 HE-AAC streams refused by name); bars 2, 3 and 5
missed, final for that code. Bar 2: 322 of 481 counted inputs. Every one
of the 145 generated MP4 inputs missed the length rule alone:
`ffmpeg -ignore_editlist 1` 9.0.2 writes the samples its packets'
durations state (481,024 for a 10-second input at 48 kHz, with the
encoder's 1,024 priming samples), not every packet's 1,024. The rule
assumed every packet's, as FFmpeg 8.1 writes. Their samples agreed to
4.8 × 10^-7 at most (RMS 5.8 × 10^-8), and the noise level to 0.0001 dB
on the 28 channels with 100 or more noise blocks. The 24 streams of
`al03` (every rate, both frame lengths; one channel, a program
configuration) missed on samples, with an RMS of 1.8 to 3.9 × 10^-3. Bar
1 found CoreAV within 3.5 × 10^-8 of the reference on these streams, so
FFmpeg is that far from the reference. One generated input, 7.1, was
refused: FFmpeg 9.0.2 writes 7.1 as channel configuration 12, which this
code did not read. Bar 3: 119 of 122. The 3 misses (6.1, and 6.1 and 7.1
with `-aac_pce 1`) are program configurations whose channels FFmpeg
orders differently ("Choices"). Bar 5: 290 of 292, the two being the
refused 7.1 input.

**Round 2**, fixed here before its run, on new code: channel
configurations 11, 12 and 14 ("Scope"). Three changes to the method,
each found after round 1:

1. Bar 2's lengths: CoreAV's is still the packets times the frame
   length. FFmpeg's must be the samples its packets' durations state
   (`ffprobe -ignore_editlist 1`, in the stream's time base), and at
   most the packets times the frame length. The samples are compared
   over FFmpeg's length.
2. Bar 2 lists a conformance stream as FFmpeg's divergence from the
   reference, with both measures, and does not count it, when CoreAV's
   output is within bar 1's full accuracy of the stream's reference and
   FFmpeg's is not, measured as bar 1 measures.
3. Bar 3 reports, and does not gate, an input with a program
   configuration whose channels bar 2 matched to FFmpeg's in another
   order: a PCE's channel order is the decoder's choice ("Choices").

The tolerances, the level rule, the sets and the other bars are
unchanged.

Round 2 results, on `7deca1b`, against the same FFmpeg build: every bar
met. Bar 1: 336 of 336 LC streams. Bar 2: 458 of 458 counted inputs (146
generated, 312 conformance). The largest difference was 3.9 × 10^-5
(RMS at most 4.1 × 10^-7), and the noise level agreed to 0.0006 dB on the
76 channels with 100 or more noise blocks. Not counted: 105 HE-AAC
streams (95 refused by name, and 10 that `ffmpeg` decodes with errors),
and the 24 streams of `al03`, which FFmpeg decodes off the reference
(RMS 1.8 to 3.9 × 10^-3 from the reference; CoreAV's at most
3.5 × 10^-8). Bar 3: 120 of 120 gating inputs. Reported: 23 inputs with
noise bands or intensity under an all-ones mask, and the program
configurations 6.1, 6.1 with `-aac_pce 1` and 7.1 with `-aac_pce 1`,
whose channels FFmpeg orders differently. `ffmpeg`'s default output
equals the presentation model on 120 inputs. Bar 4: 146 Matroska inputs,
lengths equal to `ffmpeg`'s on all of them; on the 123 without noise or
intensity frames, samples agreed within the tolerance except on those
three program configurations (channel order). Bar 5: 292 of 292.
