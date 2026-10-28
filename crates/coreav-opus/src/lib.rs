//! Memory-safe Opus decoding (`docs/design/audio-decode.md`, "Opus").
//!
//! This crate holds what does not depend on the codec's layers, and the
//! layers as they land, written from RFC 6716 and its reference code. So far:
//!
//! - [`Decoder`] decodes SILK-only and CELT-only packets to 48 kHz, mono or
//!   stereo. The SILK layer (`silk`, RFC 6716 4.2) reads its bitstream,
//!   ending every SILK-only packet of the RFC 8251 vectors without
//!   redundancy in the reference decoder's range state, and reconstructs it
//!   in the reference's integer arithmetic; `resample` brings it to 48 kHz
//!   as the reference decoder does. The output of the SILK-only vectors (02
//!   to 04), stereo and mono, is the reference decoder's, sample for sample,
//!   and `opus_compare` scores it 100.0 against the stereo reference. The
//!   CELT layer (`celt`, RFC 6716 4.3) ends every packet of the CELT-only
//!   vectors (01, 07, 11) in the reference's range state and synthesizes
//!   in `f32`; `opus_compare` scores those vectors 99.8 to 99.9 in stereo
//!   and 98.1 to 98.4 in mono. Hybrid packets, SILK frames with a redundant
//!   CELT frame and the cross-fades of mode switches are
//!   [`Error::Unsupported`] or missing until the hybrid layer lands; lost
//!   frames decode to silence until concealment does.
//!
//! - [`Head`]: the identification header as Matroska and Ogg carry it
//!   (`OpusHead`, RFC 7845 5.1) and as MP4 does (the body of the `dOps`
//!   box of the Opus-in-ISOBMFF mapping, the same fields big-endian), with
//!   the channel mapping (RFC 7845 5.1.1; RFC 8486 for family 2) and the
//!   [`Layout`] of the output in WAV's order.
//! - [`Packet`]: the packet layer of RFC 6716 section 3 (the TOC byte and
//!   the four frame-count codes) and the self-delimiting variant of its
//!   Appendix B; [`for_each_stream`] cuts a multistream sample into its
//!   streams' packets.
//!
//! Choices, beyond what the RFCs fix:
//! - Every rule \[R1\] to \[R7\] of RFC 6716 section 3.4 is checked, and a
//!   packet that breaks one is [`Error::Invalid`]: the RFC reserves such
//!   packets and lets a decoder discard them.
//! - Mapping family 3 (RFC 8486, ambisonics with a demixing matrix) is
//!   refused ([`Error::Unsupported`]); families 4 to 254 are read as 255,
//!   as RFC 7845 5.1.1.4 asks of a demuxer.
//! - Family 1's channels are given in WAV's order with WAV's speaker mask
//!   (the masks `ffmpeg` writes for 3 to 8 channels, a black-box check);
//!   families 2 and 255 keep the coded order with no mask.
//!
//! Nothing here allocates; every input is bounded by the formats
//! themselves (255 channels, 48 frames of at most 1,275 bytes, 120 ms per
//! packet).
//!
//! Prior art: the code of RFC 6716 Appendix A with RFC 8251's changes, the
//! specification, read for
//! - its normative tables: the SILK tables of `tables/silk.rs`, which
//!   `scripts/opus_tables.py` generates from it (the entropy coding, LSF,
//!   pitch and long-term prediction tables of RFC 6716 4.2.7);
//! - the order of the SILK layer's reads where the prose of 4.2 leaves it
//!   to the code: which frames code their first gain, lag and LTP scaling
//!   independently, how the stage-2 LSF contexts are packed, the shifted
//!   pulse-count table after ten LSBs, and which blocks code signs
//!   (`silk/dec_API.c`, `decode_indices.c`, `NLSF_unpack.c`,
//!   `decode_pulses.c`, `shell_coder.c`, `code_signs.c`);
//! - the integer arithmetic of SILK's reconstruction and stereo unmixing
//!   (4.2.7.9, 4.2.8), and which state a new rate, a new side channel or a
//!   switch between mono and stereo clears (`silk/decode_core.c`,
//!   `stereo_MS_to_LR.c`, `decode_frame.c`, `decode_parameters.c`,
//!   `decoder_set_fs.c`, `init_decoder.c`, `dec_API.c`, with the
//!   arithmetic of `macros.h`, `SigProc_FIX.h`, `Inlines.h` and
//!   `LPC_analysis_filter.c`).
//!
//! - the CELT layer (4.3), whose prose leaves tables and procedures to the
//!   code: the tables of `tables/celt.rs`, generated (band edges, the
//!   allocation vectors, the pulse cache and caps, the coarse energy's
//!   model and predictors, the small symbol tables); the order and budget
//!   tests of the frame's symbols, the time-frequency change, the boosts
//!   and the post-filter's state (`celt/celt.c`); the allocation
//!   (`rate.c`, `rate.h`); the energies and the Laplace decoder
//!   (`quant_bands.c`, `laplace.c`); the bands' splitting, angles, stereo,
//!   folding and anti-collapse, with the fixed-point cosine and log-tangent
//!   that decide the splits (`bands.c`, `mathops.c`, `mathops.h`); the
//!   pulse vectors' normalization, rotation and collapse masks (`vq.c`),
//!   and their enumeration (`cwrs.c`, whose order the decoder's own
//!   recurrence was checked against); and the inverse MDCT's output, its
//!   window and scale (`mdct.c`, matched as a definition, not as code).
//!
//! And its non-normative SILK resampler, for the upsampling
//! to 48 kHz that the conformance test's reference output has passed
//! through: its tables (`tables/resampler.rs`, generated), its integer
//! arithmetic, its input delays and its batch boundaries
//! (`silk/resampler.c`, `resampler_private_IIR_FIR.c`,
//! `resampler_private_up2_HQ.c`, `resampler_rom.c`). Only the path from 8,
//! 12 and 16 kHz to 48 kHz: the decoder always outputs 48 kHz, and other
//! rates are `coreav-audio`'s resampling.
//!
//! The structure is CoreAV's own (a whole Opus frame per call, the state of
//! relative coding in locals, one buffer per signal from the history into
//! the frame, an allpass chain as one function; CELT's choices are in the
//! `celt` module's documentation); the range decoder and the packet layer
//! are written from the RFCs' prose. `NOTICE` carries the code's BSD
//! notice.
//!
//! ```
//! use coreav_opus::Packet;
//!
//! // One 20 ms narrowband SILK frame (RFC 6716 Figure 8).
//! let packet = Packet::parse(&[1 << 3, 0xAB, 0xCD]).unwrap();
//! assert_eq!(packet.samples(), 960);
//! assert!(Packet::parse(&[]).is_err());
//! ```

#![forbid(unsafe_code)]

use core::fmt;

mod celt;
mod decoder;
mod head;
mod packet;
mod range;
mod resample;
mod silk;
mod tables {
    //! The tables of the reference code, the normative ones and the SILK
    //! resampler's, which `scripts/opus_tables.py` generates; CI
    //! (`audio-conformance.yml`) checks that these files are what the script
    //! writes.

    #[rustfmt::skip]
    pub(crate) mod celt;
    #[rustfmt::skip]
    pub(crate) mod resampler;
    #[rustfmt::skip]
    pub(crate) mod silk;

    #[cfg(test)]
    mod tests {
        use super::silk::*;

        #[test]
        fn icdf_tables_fall_to_zero() {
            // An inverse CDF falls and ends at 0 (RFC 6716 4.1.3.3).
            let mut tables: Vec<&[u8]> = vec![
                &DELTA_GAIN_ICDF,
                &LSB_ICDF,
                &PITCH_LAG_HIGH_ICDF,
                &PITCH_DELTA_ICDF,
                &PITCH_CONTOUR_ICDF,
                &PITCH_CONTOUR_NB_ICDF,
                &PITCH_CONTOUR_10MS_ICDF,
                &PITCH_CONTOUR_10MS_NB_ICDF,
                &LTP_PERIODICITY_ICDF,
                &LTP_FILTER_ICDF_0,
                &LTP_FILTER_ICDF_1,
                &LTP_FILTER_ICDF_2,
                &LTP_SCALE_ICDF,
                &TYPE_OFFSET_ACTIVE_ICDF,
                &TYPE_OFFSET_INACTIVE_ICDF,
                &LBRR_FLAGS_40MS_ICDF,
                &LBRR_FLAGS_60MS_ICDF,
                &STEREO_STAGE1_ICDF,
                &STEREO_MID_ONLY_ICDF,
                &UNIFORM3_ICDF,
                &UNIFORM4_ICDF,
                &UNIFORM5_ICDF,
                &UNIFORM6_ICDF,
                &UNIFORM8_ICDF,
                &NLSF_EXTENSION_ICDF,
                &NLSF_INTERP_ICDF,
            ];
            tables.extend(GAIN_MSB_ICDF.iter().map(|t| &t[..]));
            tables.extend(RATE_LEVEL_ICDF.iter().map(|t| &t[..]));
            tables.extend(PULSE_COUNT_ICDF.iter().map(|t| &t[..]));
            for t in &tables {
                assert_eq!(t.last(), Some(&0), "{t:?}");
                assert!(t.windows(2).all(|w| w[0] >= w[1]), "{t:?}");
            }
            // The rows of the tables that pack one context per row.
            for t in [&NLSF_NB_STAGE1_ICDF[..], &NLSF_WB_STAGE1_ICDF[..]] {
                for row in t.chunks(32) {
                    assert_eq!(row.last(), Some(&0));
                }
            }
        }

        #[test]
        fn shapes() {
            // RFC 6716 4.2.7.5.1: 32 stage-1 vectors of 10 (NB, MB) or 16
            // (WB) coefficients.
            assert_eq!(NLSF_NB_STAGE1.len(), 32 * 10);
            assert_eq!(NLSF_WB_STAGE1.len(), 32 * 16);
            assert_eq!(LSF_COS_Q12.len(), 129);
        }
    }
}

pub use decoder::Decoder;
pub use head::{Head, Layout, Source};
pub use packet::{Bandwidth, MAX_FRAMES, MAX_PACKET_SAMPLES, Mode, Packet, Toc, for_each_stream};

/// Why a configuration or a packet could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data breaks its specification.
    Invalid(&'static str),
    /// Valid, but outside what this crate implements.
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(what) => write!(f, "invalid bitstream: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl std::error::Error for Error {}
