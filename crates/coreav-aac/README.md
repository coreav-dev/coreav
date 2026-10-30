# coreav-aac

An AAC LC decoder in Rust, written from ISO/IEC 14496-3:2009 (MPEG-4
audio), with ISO/IEC 13818-7:2004 (MPEG-2 AAC) as a cross-check of the
tables the two share.

## Read this first

**This decoder is opt-in and is never part of a default CoreAV build.**

Decoding AAC may require a patent licence depending on where you are and
what you do with it. The sources disagree on whether patents on AAC LC are
still in force, and pool licences for AAC exist. CoreAV therefore ships
this decoder as a separate crate that an operator enables deliberately,
through the non-default Cargo feature `aac` of `coreav-audio` and
`coreav-cli` (`docs/design/audio-decode.md`, "AAC-LC"). Without it,
`coreav decode-audio` names an AAC stream and refuses it: `codec aac is not
decoded`. CI fails if a default build of any workspace crate links this
one.

Nothing here is legal advice. Deciding whether you may use this decoder is
yours to make.

## What it decodes

- AAC LC (audio object type 2, and MPEG-2 AAC LC), frames of 1,024 or 960
  samples, every sample rate, channel configurations 1 to 7 and program
  configurations: single channel, channel pair, LFE and coupling channel
  elements; M/S and intensity stereo, perceptual noise substitution,
  temporal noise shaping, every window sequence and both window shapes.
  Output is planar 32-bit floating point in WAV's channel order.
- Refuses by name, rather than writing a wrong signal: HE-AAC and HE-AAC v2
  (SBR and parametric stereo, signalled in the configuration or by an SBR
  payload in the stream), AAC Main, SSR and LTP, and the error-resilient,
  scalable and low-delay object types.
- Does not apply dynamic range control or a program configuration's matrix
  mixdown, both optional for a decoder.

Every AAC LC stream in MP4 of the ISO/IEC 14496-26 conformance package
(336 streams, `al00` to `al19`) decodes within full accuracy of its
reference waveform, and every HE-AAC stream of the package is refused by
name; `.github/workflows/audio-conformance.yml` checks it.

## How it is built

- **Sans-IO.** It takes an `AudioSpecificConfig` and `raw_data_block()`s, as
  MP4 and Matroska store them, and returns samples. It opens no files and
  starts no threads.
- **The same output on every run and thread count.** Noise substitution
  draws from a generator seeded by the packet's bytes and its position in
  the stream; a caller that decodes a stream in parts sets the position
  (`Decoder::set_position`), and a frame's state depends on the frame
  before alone, so a part primed with one packet decodes as in one pass.
- **Bounded.** `Limits` bound the channels; every frame codes a fixed
  number of samples; hostile input gives an error, never a panic or
  unbounded work.
- **No `unsafe`**, no dependencies, and no C math library: its windows,
  tables and FFT twiddles are generated (`tables/gen_aac.py`).
