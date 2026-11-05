use super::{MatroskaRemuxError, SAMPLE_BYTES};
use coreav_isobmff::{AacFrameConfig, Profile, aac_frame_config, aac_has_explicit_no_sbr};
use coreav_mkv::{Allowlist, Codec, Encoding, Packet, Segment, Source, Track};

const FRAME_NS: u64 = 32_000_000;
const FRAME_TICKS: i64 = 32;

pub(super) struct AacCopyClock {
    qualified: bool,
    packets: u32,
}

impl AacCopyClock {
    pub(super) fn new(segment: &Segment, track: &Track, only_track: bool) -> Self {
        let private = track.codec_private.as_deref().unwrap_or_default();
        let configured = aac_frame_config(private);
        Self {
            qualified: only_track
                && segment.warnings.is_empty()
                && track.warnings.is_empty()
                && segment.timestamp_scale == 1_000_000
                && track.timestamp_scale_one
                && track.codec_delay == FRAME_NS
                && track.default_duration == Some(FRAME_NS)
                && track.seek_pre_roll == 0
                && track.encoding == Encoding::None
                && matches!(
                    track.profile,
                    Some(Profile::Aac {
                        object_type: 2,
                        sbr: false,
                        ps: false
                    })
                )
                && aac_has_explicit_no_sbr(private)
                && configured.is_some_and(|c| {
                    matches!(
                        c,
                        AacFrameConfig {
                            sample_rate: 32_000,
                            channels: 1 | 2,
                            frame_length: 1024
                        }
                    ) && track.audio.is_some_and(|a| {
                        a.sample_rate() == Some(c.sample_rate)
                            && a.channels == u64::from(c.channels)
                            && a.output_sampling_frequency.is_none()
                    })
                }),
            packets: 0,
        }
    }

    pub(super) fn observe(
        &mut self,
        packet: &Packet,
        segment: &Segment,
    ) -> Result<(), MatroskaRemuxError> {
        let timestamp = i64::from(self.packets) * FRAME_TICKS;
        let ns = i128::from(timestamp - FRAME_TICKS) * 1_000_000;
        self.qualified &= packet.block_timestamp == timestamp
            && packet.pts == Some(timestamp - FRAME_TICKS)
            && packet.dts == packet.pts
            && packet.pts_ns(segment) == Some(ns)
            && packet.frames_in_block == 1
            && packet.lace == 0
            && packet.block_duration.is_none_or(|d| d == 32)
            && packet.discard_padding.is_none_or(|d| d == 0)
            && packet.block_additions.is_none()
            && !packet.codec_state
            && !packet.invisible
            && !packet.discardable
            && packet.keyframe
            && packet.stored_size != 0;
        self.packets = self
            .packets
            .checked_add(1)
            .ok_or(coreav_isobmff::Error::Overflow {
                offset: packet.offset,
            })?;
        Ok(())
    }

    pub(super) fn finish(&self, segment: &Segment) -> Result<(), MatroskaRemuxError> {
        if self.qualified
            && self.packets > 1
            && segment.duration == Some(f64::from(self.packets) * 32.0)
        {
            return Ok(());
        }
        Err(coreav_mkv::Error::Unsupported {
            reason: "Matroska AAC copy whose packet clocks are not qualified",
        }
        .into())
    }
}

/// Checks selected AAC copy clocks before compatibility output is staged.
/// The compatibility writer repeats this guard during its own packet listing.
pub fn check_matroska_aac_copy<S: Source>(
    mut source: S,
    demux: &coreav_mkv::Limits,
    mux: &coreav_isobmff::Limits,
    allowlist: &Allowlist,
    streams: &[usize],
) -> Result<(), MatroskaRemuxError> {
    if u64::try_from(streams.len()).unwrap_or(u64::MAX) > u64::from(mux.max_tracks) {
        return Err(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::Tracks,
            offset: 0,
        }
        .into());
    }
    let segment = coreav_mkv::probe(&mut source, demux, allowlist)?;
    let Some(track) = segment
        .tracks
        .iter()
        .find(|t| t.codec == Codec::Aac && streams.contains(&t.stream_index))
    else {
        return Ok(());
    };
    let mut clock = AacCopyClock::new(&segment, track, streams.len() == 1);
    let mut samples = 0u64;
    for packet in segment.packets(&mut source, demux) {
        let packet = packet?;
        if segment
            .tracks
            .get(packet.track)
            .is_some_and(|t| streams.contains(&t.stream_index))
        {
            samples = samples
                .checked_add(1)
                .ok_or(coreav_isobmff::Error::Overflow {
                    offset: packet.offset,
                })?;
            let bytes =
                samples
                    .checked_mul(SAMPLE_BYTES)
                    .ok_or(coreav_isobmff::Error::Overflow {
                        offset: packet.offset,
                    })?;
            if bytes > mux.max_table_bytes {
                return Err(coreav_isobmff::Error::LimitExceeded {
                    limit: coreav_isobmff::LimitKind::TableBytes,
                    offset: packet.offset,
                }
                .into());
            }
        }
        if packet.track == track.index {
            clock.observe(&packet, &segment)?;
        }
    }
    clock.finish(&segment)
}
