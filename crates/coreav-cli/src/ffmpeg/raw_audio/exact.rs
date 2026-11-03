use super::DecoderEligibilityKind;
use coreav_audio::SampleFormat;

pub(super) enum Presentation {
    CompletePackets { frames: fn(&[u8]) -> Option<u32> },
}

pub(super) struct Decoder {
    pub codec: coreav_audio::Codec,
    pub bits: u32,
    pub packing: SampleFormat,
    pub presentation: Presentation,
    pub eligibility: DecoderEligibilityKind,
    pub evidence: &'static str,
}

pub(super) const DECODERS: &[Decoder] = &[
    Decoder {
        codec: coreav_audio::Codec::Flac,
        bits: 16,
        packing: SampleFormat::S16,
        presentation: Presentation::CompletePackets {
            frames: coreav_audio::flac::frame_samples,
        },
        eligibility: DecoderEligibilityKind::Exact,
        evidence: "raw_audio_pipe_check: FLAC16 to S16 identity",
    },
    Decoder {
        codec: coreav_audio::Codec::Flac,
        bits: 24,
        packing: SampleFormat::S16,
        presentation: Presentation::CompletePackets {
            frames: coreav_audio::flac::frame_samples,
        },
        eligibility: DecoderEligibilityKind::Exact,
        evidence: "raw_audio_pipe_check: FLAC24 to S16 identity",
    },
    Decoder {
        codec: coreav_audio::Codec::Flac,
        bits: 16,
        packing: SampleFormat::F32,
        presentation: Presentation::CompletePackets {
            frames: coreav_audio::flac::frame_samples,
        },
        eligibility: DecoderEligibilityKind::Exact,
        evidence: "raw_audio_pipe_check: FLAC16 to F32 identity",
    },
    Decoder {
        codec: coreav_audio::Codec::Flac,
        bits: 24,
        packing: SampleFormat::F32,
        presentation: Presentation::CompletePackets {
            frames: coreav_audio::flac::frame_samples,
        },
        eligibility: DecoderEligibilityKind::Exact,
        evidence: "raw_audio_pipe_check: FLAC24 to F32 identity",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Implementation {
    Identity,
    S16Compatibility,
    F32Compatibility,
}

struct Rate {
    packing: SampleFormat,
    output: SampleFormat,
    channels: &'static [u16],
    platform: (&'static str, &'static str),
    implementation: Implementation,
    evidence: &'static str,
    input_rates: &'static [u32],
    output_rates: &'static [u32],
}

const RATES: &[Rate] = &[
    Rate {
        packing: SampleFormat::S16,
        output: SampleFormat::S16,
        channels: &[1, 2],
        platform: ("linux", "x86_64"),
        implementation: Implementation::S16Compatibility,
        evidence: "audio_swr_parity_check and raw_audio_pipe_check: unchanged mono/stereo S16",
        input_rates: &[],
        output_rates: &[],
    },
    Rate {
        packing: SampleFormat::F32,
        output: SampleFormat::F32,
        channels: &[1],
        platform: ("linux", "x86_64"),
        implementation: Implementation::F32Compatibility,
        evidence: "mono F32 compatibility rate stage, pinned 9.0.2 complete output comparison",
        input_rates: &[44_100, 48_000],
        output_rates: &[16_000],
    },
    Rate {
        packing: SampleFormat::F32,
        output: SampleFormat::S16,
        channels: &[1],
        platform: ("linux", "x86_64"),
        implementation: Implementation::F32Compatibility,
        evidence: "mono F32 compatibility rate stage with final S16 quantization, pinned 9.0.2",
        input_rates: &[44_100, 48_000],
        output_rates: &[16_000],
    },
];

pub(super) fn rate(
    packing: SampleFormat,
    output: SampleFormat,
    channels: u16,
    input_rate: u32,
    output_rate: u32,
) -> Option<Implementation> {
    if !SERIALIZERS.contains(&(packing, output)) {
        return None;
    }
    if input_rate == output_rate {
        return Some(Implementation::Identity);
    }
    RATES
        .iter()
        .find(|row| {
            row.packing == packing
                && row.output == output
                && row.channels.contains(&channels)
                && row.platform == (std::env::consts::OS, std::env::consts::ARCH)
                && (row.input_rates.is_empty() || row.input_rates.contains(&input_rate))
                && (row.output_rates.is_empty() || row.output_rates.contains(&output_rate))
                && !row.evidence.is_empty()
        })
        .map(|row| row.implementation)
}

const SERIALIZERS: &[(SampleFormat, SampleFormat)] = &[
    (SampleFormat::S16, SampleFormat::S16),
    (SampleFormat::S16, SampleFormat::F32),
    (SampleFormat::F32, SampleFormat::F32),
    (SampleFormat::F32, SampleFormat::S16),
];

struct Channel {
    input: u16,
    output: u16,
    source_bits: Option<u32>,
    packing: SampleFormat,
    platform: (&'static str, &'static str),
    evidence: &'static str,
}

const CHANNEL_CONVERSIONS: &[Channel] = &[];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChannelImplementation {
    Identity,
    DecoderMix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ChannelPlan {
    pub input: u16,
    pub output: u16,
    implementation: ChannelImplementation,
}

impl ChannelPlan {
    pub fn decoder_channels(self) -> Option<u16> {
        match self.implementation {
            ChannelImplementation::Identity => None,
            ChannelImplementation::DecoderMix => Some(self.output),
        }
    }
}

pub(super) fn channels(
    input: u16,
    output: u16,
    source_bits: Option<u32>,
    packing: SampleFormat,
) -> Option<ChannelPlan> {
    if matches!((input, output), (1, 1) | (2, 2)) {
        return Some(ChannelPlan {
            input,
            output,
            implementation: ChannelImplementation::Identity,
        });
    }
    CHANNEL_CONVERSIONS
        .iter()
        .find(|row| {
            row.input == input
                && row.output == output
                && row.source_bits == source_bits
                && row.packing == packing
                && row.platform == (std::env::consts::OS, std::env::consts::ARCH)
                && !row.evidence.is_empty()
        })
        .map(|row| ChannelPlan {
            input,
            output,
            implementation: if row.input == row.output {
                ChannelImplementation::Identity
            } else {
                ChannelImplementation::DecoderMix
            },
        })
}
